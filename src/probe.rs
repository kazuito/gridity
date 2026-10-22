use anyhow::Result;
use clap::Parser;
use schemars::JsonSchema;
use serde::Serialize;

use crate::cli::{Cli, Command, Detail, InputKind, Preset, ProbeArgs, RenderArgs};
use crate::detect::detect;
use crate::error::{Reason, usage};
use crate::layout::{capacity, plan};
use crate::output::{SCHEMA_VERSION, SourceInfo};
use crate::pagetext::{
    Content, PageSummary, READABLE_PX, SKIMMABLE_PX, document_body_pt, summarize,
};
use crate::render::{
    Layout, build_layout, compact_ranges, kind_name, native_bounds, preset_name, round3,
};
use crate::select::{format_time, spread_pages};
use crate::source::{Source, fit_size};

/// Pages measured for the body text size when `--per-page` is not given.
const SAMPLE_PAGES: usize = 8;

const DETAILS: [(Detail, &str); 3] = [
    (Detail::Low, "low"),
    (Detail::Medium, "medium"),
    (Detail::High, "high"),
];

/// `probe --json`.
#[derive(Serialize, JsonSchema)]
pub struct ProbeReport {
    pub schema_version: u32,
    pub source: SourceInfo,
    pub preset: String,
    pub estimates: Vec<Estimate>,
    /// Ready-to-run commands; heuristic starting points.
    pub suggestions: Vec<String>,
    /// Median body text size over the measured pages (PDF).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_text_pt: Option<f64>,
    /// Every page with `--per-page` (PDF).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<Vec<PageReport>>,
    pub warnings: Vec<String>,
}

/// Sheets and scale for one `--detail` level, as a plain `gridity render` would plan them.
#[derive(Serialize, JsonSchema)]
pub struct Estimate {
    pub detail: String,
    pub per_sheet: usize,
    /// Sheets for every page or image; absent for video and animations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sheets: Option<usize>,
    /// Sheet pixels per PDF point, or displayed / native pixels.
    pub scale: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_12pt_px: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_text_px: Option<f64>,
    /// Scale when only the pages worth rendering are rendered (`--per-page`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visual_scale: Option<f64>,
    /// Set on the detail level `render` uses by default.
    #[serde(default, skip_serializing_if = "is_false")]
    pub default: bool,
}

#[derive(Serialize, JsonSchema)]
pub struct PageReport {
    pub page: usize,
    #[serde(flatten)]
    pub summary: PageSummary,
}

fn is_false(value: &bool) -> bool {
    !value
}

pub fn run(args: ProbeArgs) -> Result<()> {
    let json = args.json;
    let report = report(args)?;
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_text(&report);
    }
    Ok(())
}

pub fn report(args: ProbeArgs) -> Result<ProbeReport> {
    let kind = detect(&args.inputs, args.kind)?;
    let source = Source::open(kind, &args.inputs, &args.source)?;
    let (count, shapes) = item_shapes(&source);
    let aspect = median_aspect(&shapes);
    let pages = page_summaries(&source, args.per_page)?;
    let body_pt = pages
        .as_ref()
        .and_then(|p| document_body_pt(&p.iter().map(|(_, s)| s.clone()).collect::<Vec<_>>()));

    let per_page = pages.as_deref().filter(|_| args.per_page);
    let visual = per_page.map(visual_pages);
    let visual_shapes: Option<Vec<Shape>> = match (&source, &visual) {
        (Source::Pdf(pdf), Some(visual)) if !visual.is_empty() => Some(
            visual
                .iter()
                .map(|(page, _)| page_shape(pdf.page_sizes[page - 1]))
                .collect(),
        ),
        _ => None,
    };

    let mut estimates = Vec::new();
    for (detail, name) in DETAILS {
        let mut layout = build_layout(&default_render_args(args.preset, name), kind)?;
        layout.options.max_cell = native_bounds(&source);
        layout.options.max_sheets = usize::MAX;
        let per_sheet = capacity(aspect, layout.limits, layout.chrome, &layout.options, 200);
        let scale = sheet_scale(&layout, count.unwrap_or(per_sheet), &shapes)?;
        let visual_scale = match &visual_shapes {
            Some(shapes) => Some(round3(sheet_scale(&layout, shapes.len(), shapes)?)),
            None => None,
        };
        estimates.push(Estimate {
            detail: name.to_string(),
            per_sheet,
            sheets: count.map(|count| count.div_ceil(per_sheet)),
            scale: round3(scale),
            text_12pt_px: (kind == InputKind::Pdf).then(|| round1(12.0 * scale)),
            body_text_px: body_pt.map(|body| round1(body * scale)),
            visual_scale,
            default: detail == Detail::Medium,
        });
    }
    Ok(ProbeReport {
        schema_version: SCHEMA_VERSION,
        source: crate::render::source_info(&source, true),
        preset: preset_name(args.preset).to_string(),
        suggestions: suggestions(&source, &args, &estimates, body_pt, per_page),
        estimates,
        body_text_pt: body_pt,
        pages: per_page.map(|pages| {
            pages
                .iter()
                .map(|(page, summary)| PageReport {
                    page: *page,
                    summary: summary.clone(),
                })
                .collect()
        }),
        warnings: source.open_warnings(),
    })
}

/// Smallest scale among `shapes` when `count` items are planned with `layout`, as `render`
/// would place them.
fn sheet_scale(layout: &Layout, count: usize, shapes: &[Shape]) -> Result<f64> {
    let sheets = plan(
        count,
        median_aspect(shapes),
        layout.limits,
        layout.chrome,
        &layout.options,
    )?;
    let grid = sheets[0].grid;
    Ok(shapes
        .iter()
        .map(|shape| {
            let (width, _) = fit_size(
                shape.aspect,
                (grid.cell_width, grid.cell_height),
                shape.native,
            );
            width as f64 / shape.width
        })
        .fold(f64::INFINITY, f64::min))
}

/// Pages worth rendering: those with figures, formulas or text that cannot be extracted.
fn visual_pages(pages: &[(usize, PageSummary)]) -> Vec<&(usize, PageSummary)> {
    pages
        .iter()
        .filter(|(_, s)| matches!(s.content, Content::Mixed | Content::Graphics))
        .collect()
}

/// Measures every page with `--per-page`, otherwise a few pages spread over the document.
fn page_summaries(source: &Source, per_page: bool) -> Result<Option<Vec<(usize, PageSummary)>>> {
    let Source::Pdf(pdf) = source else {
        if per_page {
            return Err(usage(
                Reason::FlagNotForInput,
                format!(
                    "--per-page works with PDF input only, not {}",
                    kind_name(source.kind())
                ),
            )
            .with_hint("see: gridity guide pdf")
            .into());
        }
        return Ok(None);
    };
    let pages = if per_page {
        (1..=pdf.page_count()).collect()
    } else {
        spread_pages(pdf.page_count(), SAMPLE_PAGES)
    };
    let scans = pdf.scan(&pages)?;
    Ok(Some(
        pages.into_iter().zip(scans.iter().map(summarize)).collect(),
    ))
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// Builds render arguments with every default, so estimates match a plain `gridity render`.
fn default_render_args(preset: Preset, detail: &str) -> RenderArgs {
    let cli = Cli::try_parse_from([
        "gridity",
        "render",
        "-",
        "--preset",
        preset_name(preset),
        "--detail",
        detail,
    ])
    .expect("default render arguments parse");
    match cli.command {
        Command::Render(args) => *args,
        _ => unreachable!(),
    }
}

/// Size of an item before scaling, used to report how much a sheet shrinks it.
struct Shape {
    aspect: f64,
    /// Points for PDF pages, pixels for raster sources.
    width: f64,
    native: Option<(u32, u32)>,
}

/// Number of items for a full render (None for time-based media) and their distinct shapes.
fn item_shapes(source: &Source) -> (Option<usize>, Vec<Shape>) {
    let raster = |(w, h): (u32, u32)| Shape {
        aspect: w as f64 / h as f64,
        width: w as f64,
        native: Some((w, h)),
    };
    match source {
        Source::Pdf(pdf) => (
            Some(pdf.page_count()),
            pdf.page_sizes
                .iter()
                .map(|&size| page_shape(size))
                .collect(),
        ),
        Source::Images(images) => (
            Some(images.files.len()),
            images.files.iter().map(|f| raster(f.size)).collect(),
        ),
        Source::Video(video) => (None, vec![raster(video.display)]),
        Source::Anim(anim) => (None, vec![raster(anim.size)]),
    }
}

fn page_shape((w, h): (f32, f32)) -> Shape {
    Shape {
        aspect: (w / h) as f64,
        width: w as f64,
        native: None,
    }
}

fn median_aspect(shapes: &[Shape]) -> f64 {
    let mut aspects: Vec<f64> = shapes.iter().map(|s| s.aspect).collect();
    aspects.sort_by(f64::total_cmp);
    aspects[aspects.len() / 2]
}

fn suggestions(
    source: &Source,
    args: &ProbeArgs,
    estimates: &[Estimate],
    body_pt: Option<f64>,
    pages: Option<&[(usize, PageSummary)]>,
) -> Vec<String> {
    let input = args
        .inputs
        .iter()
        .map(|p| quote(&p.display().to_string()))
        .collect::<Vec<_>>()
        .join(" ");
    let per_sheet = |i: usize| estimates[i].per_sheet;
    let sheets = |i: usize| estimates[i].sheets.unwrap_or(1);
    let mut list = Vec::new();
    match source {
        Source::Pdf(pdf) => {
            let detail = readable_detail(estimates, body_pt);
            if body_pt.is_some() {
                list.push(format!("gridity text {input}"));
            }
            match pages {
                Some(pages) => {
                    let visual = visual_pages(pages);
                    if visual.is_empty() {
                        return list;
                    }
                    let numbers: Vec<usize> = visual.iter().map(|(page, _)| *page).collect();
                    list.push(format!(
                        "gridity render {input} --pages {}{}{}",
                        compact_ranges(&numbers),
                        detail_flag(detail),
                        max_sheets(numbers.len().div_ceil(per_sheet(detail)))
                    ));
                    let scale = visual_scale(estimates, detail);
                    let close: Vec<usize> = visual
                        .iter()
                        .filter(|(_, s)| s.small_pt.is_some_and(|pt| pt * scale < SKIMMABLE_PX))
                        .map(|(page, _)| *page)
                        .collect();
                    match close.as_slice() {
                        [] => {}
                        [page] => list.push(format!(
                            "gridity render {input} --pages {page} --per-sheet 1"
                        )),
                        _ if detail + 1 < DETAILS.len() => list.push(format!(
                            "gridity render {input} --pages {} --detail high{}",
                            compact_ranges(&close),
                            max_sheets(close.len().div_ceil(per_sheet(DETAILS.len() - 1)))
                        )),
                        _ => list.push(format!(
                            "gridity render {input} --pages {} --per-sheet 1{}",
                            compact_ranges(&close),
                            max_sheets(close.len())
                        )),
                    }
                }
                None => {
                    let flag = detail_flag(detail);
                    if sheets(detail) <= 10 {
                        list.push(format!("gridity render {input}{flag}"));
                    } else {
                        list.push(format!(
                            "gridity render {input} --pages 1-{}{flag}",
                            (per_sheet(detail) * 2).min(pdf.page_count())
                        ));
                        list.push(format!("gridity probe {input} --per-page"));
                    }
                    list.push(format!("gridity render {input} --pages 1 --detail high"));
                }
            }
        }
        Source::Images(_) => {
            list.push(format!("gridity render {input}"));
            if sheets(1) > 10 {
                list.push(format!(
                    "gridity render {input} --detail low --max-sheets {}",
                    sheets(0)
                ));
            }
        }
        Source::Video(video) => {
            list.push(format!("gridity render {input}"));
            if let Some(d) = video.duration {
                let step = nice_step(d / (per_sheet(1) as f64 * 3.0));
                list.push(format!(
                    "gridity render {input} --every {step}s --max-sheets 3"
                ));
                list.push(format!(
                    "gridity render {input} --start 0:00 --end {} --detail high",
                    format_time((d / 4.0).max(1.0), d >= 3600.0, false)
                ));
            }
        }
        Source::Anim(_) => {
            list.push(format!("gridity render {input}"));
            list.push(format!("gridity render {input} --count {}", per_sheet(0)));
        }
    }
    list
}

/// Index into DETAILS of the lowest detail whose body text stays readable on the sheets that
/// will be rendered (only the visual pages with `--per-page`); medium when unknown.
fn readable_detail(estimates: &[Estimate], body_pt: Option<f64>) -> usize {
    let Some(body) = body_pt else {
        return 1;
    };
    (0..DETAILS.len())
        .find(|&detail| body * visual_scale(estimates, detail) >= READABLE_PX)
        .unwrap_or(DETAILS.len() - 1)
}

fn visual_scale(estimates: &[Estimate], detail: usize) -> f64 {
    let estimate = &estimates[detail];
    estimate.visual_scale.unwrap_or(estimate.scale)
}

/// `--max-sheets` for renders that need more sheets than the default limit of 10.
fn max_sheets(count: usize) -> String {
    if count > 10 {
        format!(" --max-sheets {count}")
    } else {
        String::new()
    }
}

fn detail_flag(detail: usize) -> String {
    match detail {
        1 => String::new(),
        d => format!(" --detail {}", DETAILS[d].1),
    }
}

fn nice_step(seconds: f64) -> u64 {
    [1, 2, 5, 10, 15, 30, 60, 120, 300, 600]
        .into_iter()
        .find(|&s| s as f64 >= seconds)
        .unwrap_or(900)
}

/// Quotes an argument for the shell suggested commands are pasted into: POSIX shells, or
/// cmd.exe and PowerShell on Windows (where `"` cannot occur in file names).
pub fn quote(s: &str) -> String {
    let safe = if cfg!(windows) { "-_./\\:" } else { "-_./" };
    if s.chars().all(|c| c.is_alphanumeric() || safe.contains(c)) {
        s.to_string()
    } else if cfg!(windows) {
        format!("\"{s}\"")
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn print_text(report: &ProbeReport) {
    let source = &report.source;
    let (kind, unit) = match source {
        SourceInfo::Pdf { .. } => ("pdf", "pages"),
        SourceInfo::Images { .. } => ("images", "images"),
        SourceInfo::Video { .. } => ("video", "frames"),
        SourceInfo::Anim { .. } => ("anim", "frames"),
    };
    match source {
        SourceInfo::Pdf {
            path,
            pages,
            title,
            page_sizes,
        } => {
            println!("{path}: PDF, {pages} pages");
            if let Some(title) = title {
                println!("  title: {title}");
            }
            for group in page_sizes.iter().flatten() {
                println!(
                    "  {}x{} pt {} x{} (p.{})",
                    group.width_pt, group.height_pt, group.orientation, group.count, group.pages
                );
            }
        }
        SourceInfo::Video {
            path,
            duration,
            width,
            height,
            coded_width,
            coded_height,
            rotation,
            fps,
            codec,
        } => {
            println!("{path}: video");
            let duration = duration
                .map(|d| format!("{} ({d:.2}s)", format_time(d, d >= 3600.0, false)))
                .unwrap_or_else(|| "unknown".to_string());
            println!("  duration: {duration}");
            println!(
                "  display size: {width}x{height} (coded {coded_width}x{coded_height}, rotation {rotation}°)"
            );
            if let Some(fps) = fps {
                println!("  fps: {fps:.3}");
            }
            if let Some(codec) = codec {
                println!("  codec: {codec}");
            }
        }
        SourceInfo::Anim {
            path,
            frames,
            duration,
            width,
            height,
        } => {
            println!("{path}: animation");
            println!("  {frames} frames, {duration:.2}s, {width}x{height}");
        }
        SourceInfo::Images { name, count, files } => {
            println!("{name}: {count} images");
            let files = files.as_deref().unwrap_or_default();
            for file in files.iter().take(20) {
                println!("  {} ({}x{})", file.path, file.width, file.height);
            }
            if files.len() > 20 {
                println!("  ... and {} more", files.len() - 20);
            }
        }
    }
    println!("sheets (preset {}):", report.preset);
    for estimate in &report.estimates {
        let marker = if estimate.default { " (default)" } else { "" };
        let sheets = estimate
            .sheets
            .map(|s| format!(" -> {s} sheet(s)"))
            .unwrap_or_default();
        let scale = estimate.scale;
        let scale = match (estimate.body_text_px, estimate.text_12pt_px) {
            (Some(px), _) => format!(
                "{scale:.2} px/pt (body {}pt text = {px:.0}px{})",
                report.body_text_pt.unwrap_or_default(),
                if px < READABLE_PX {
                    ", hard to read"
                } else {
                    ""
                }
            ),
            (None, Some(px)) => format!("{scale:.2} px/pt (12pt text = {px:.0}px)"),
            (None, None) => format!("{:.0}% of native size", scale * 100.0),
        };
        println!(
            "  --detail {:<6} {} {unit}/sheet{sheets}, {scale}{marker}",
            estimate.detail, estimate.per_sheet
        );
    }
    if let Some(pages) = &report.pages {
        println!("pages:");
        for page in pages {
            println!("  p.{:<4} {}", page.page, page.summary.describe());
        }
    }
    println!(
        "suggested commands ({kind} input; heuristic starting points, adjust after looking at the sheets):"
    );
    for s in &report.suggestions {
        println!("  {s}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_arguments_stay_unquoted() {
        assert_eq!(quote("docs/report-v2.pdf"), "docs/report-v2.pdf");
    }

    #[cfg(unix)]
    #[test]
    fn posix_quoting() {
        assert_eq!(quote("my file.pdf"), "'my file.pdf'");
        assert_eq!(quote("it's.pdf"), "'it'\\''s.pdf'");
    }

    #[cfg(windows)]
    #[test]
    fn windows_quoting() {
        assert_eq!(quote(r"C:\docs\report.pdf"), r"C:\docs\report.pdf");
        assert_eq!(quote(r"C:\my docs\it's.pdf"), r#""C:\my docs\it's.pdf""#);
    }
}
