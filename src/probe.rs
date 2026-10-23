use anyhow::Result;
use clap::Parser;
use schemars::JsonSchema;
use serde::Serialize;

use crate::cli::{Cli, Command, Detail, InputKind, Preset, ProbeArgs, RenderArgs};
use crate::detect::detect;
use crate::error::{Reason, usage};
use crate::layout::{MAX_CAPACITY, capacity, median_aspect, plan};
use crate::output::{SCHEMA_VERSION, SourceInfo};
use crate::pagetext::{
    Content, PageSummary, READABLE_PX, SKIMMABLE_PX, document_body_pt, summarize,
};
use crate::render::{Layout, build_layout, compact_ranges, native_bounds, preset_name, round3};
use crate::select::{Sampling, format_time, sample_count, spread_pages};
use crate::source::pdf::PdfSource;
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
    /// The suggestions as `gridity mcp` tool calls, in the same order; pass `arguments` as
    /// they are.
    pub suggested_calls: Vec<SuggestedCall>,
    /// Median body text size over the measured pages (PDF).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_text_pt: Option<f64>,
    /// Every page with `--per-page` (PDF).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<Vec<PageReport>>,
    /// Pages of content type `text` as a page list, e.g. "1-3,8" (`--per-page`); absent when
    /// there are none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_pages: Option<String>,
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

/// One suggested command as a `gridity mcp` tool call.
#[derive(Debug, PartialEq, Serialize, JsonSchema)]
pub struct SuggestedCall {
    pub tool: Tool,
    /// Parameters for the tool, complete and ready to pass unchanged.
    pub arguments: CallArguments,
}

/// The `gridity mcp` tool to call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    Render,
    Probe,
}

impl Tool {
    fn name(self) -> &'static str {
        match self {
            Tool::Render => "render",
            Tool::Probe => "probe",
        }
    }
}

/// Tool parameters, named as in the tool's input schema. `render` calls set `max_sheets`
/// whenever they write more sheets than the tool allows by default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, JsonSchema)]
pub struct CallArguments {
    pub inputs: Vec<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<InputKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub every: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<Preset>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Detail>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_sheet: Option<usize>,
    /// Set when the render writes more sheets than the tool's default limit of 4.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_sheets: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_page: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_file: Option<String>,
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
    let aspect = median_aspect(shapes.iter().map(|s| s.aspect));
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
    let mut layouts = Vec::new();
    for (detail, name) in DETAILS {
        let mut layout = build_layout(&default_render_args(args.preset, name), kind)?;
        layout.options.max_cell = native_bounds(&source);
        layout.options.max_sheets = usize::MAX;
        let per_sheet = capacity(
            aspect,
            layout.limits,
            layout.chrome,
            &layout.options,
            MAX_CAPACITY,
        );
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
        layouts.push(layout);
    }
    let suggested = suggestions(&source, &estimates, &layouts, body_pt, per_page)?;
    let target = Target::new(&args);
    Ok(ProbeReport {
        schema_version: SCHEMA_VERSION,
        source: crate::render::source_info(&source, true),
        preset: preset_name(args.preset).to_string(),
        suggestions: suggested.iter().map(|s| s.command(&target)).collect(),
        suggested_calls: suggested.iter().map(|s| s.call(&target)).collect(),
        estimates,
        body_text_pt: body_pt,
        text_pages: per_page.and_then(text_pages),
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
        median_aspect(shapes.iter().map(|s| s.aspect)),
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

/// Pages that read as well from a text extractor as from an image.
fn text_pages(pages: &[(usize, PageSummary)]) -> Option<String> {
    let numbers: Vec<usize> = pages
        .iter()
        .filter(|(_, s)| s.content == Content::Text)
        .map(|(page, _)| *page)
        .collect();
    (!numbers.is_empty()).then(|| compact_ranges(&numbers))
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
                    source.kind().name()
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

/// A suggested command, written out both as a command line and as an MCP tool call.
struct Suggestion {
    tool: Tool,
    /// Selection and layout. `max_sheets` here is passed whatever the default limit is.
    arguments: CallArguments,
    /// Sheets the render writes; `max_sheets` is added when they exceed the default limit.
    sheets: usize,
}

/// What every suggestion repeats from the probe's own arguments.
struct Target {
    inputs: Vec<String>,
    kind: Option<InputKind>,
    /// Set unless it is the default, which `render` would use instead.
    preset: Option<Preset>,
    password_file: Option<String>,
}

impl Target {
    fn new(args: &ProbeArgs) -> Target {
        Target {
            inputs: args
                .inputs
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            kind: args.kind,
            preset: (args.preset != Preset::Compat).then_some(args.preset),
            password_file: args
                .source
                .password
                .password_file
                .as_ref()
                .map(|p| p.display().to_string()),
        }
    }
}

impl Suggestion {
    fn new(tool: Tool, arguments: CallArguments) -> Suggestion {
        Suggestion {
            tool,
            arguments,
            sheets: 0,
        }
    }

    fn render(sheets: usize, arguments: CallArguments) -> Suggestion {
        Suggestion {
            tool: Tool::Render,
            arguments,
            sheets,
        }
    }

    /// Complete arguments for a server or command line whose sheet limit defaults to
    /// `default_max_sheets`.
    fn arguments(&self, target: &Target, default_max_sheets: usize) -> CallArguments {
        CallArguments {
            inputs: target.inputs.clone(),
            kind: target.kind,
            preset: target.preset,
            password_file: target.password_file.clone(),
            max_sheets: self
                .arguments
                .max_sheets
                .or((self.sheets > default_max_sheets).then_some(self.sheets)),
            ..self.arguments.clone()
        }
    }

    fn call(&self, target: &Target) -> SuggestedCall {
        SuggestedCall {
            tool: self.tool,
            arguments: self.arguments(target, crate::mcp::DEFAULT_MAX_SHEETS),
        }
    }

    fn command(&self, target: &Target) -> String {
        let a = self.arguments(target, crate::layout::DEFAULT_MAX_SHEETS);
        let mut line = format!("gridity {}", self.tool.name());
        for input in &a.inputs {
            line.push(' ');
            line.push_str(&quote(input));
        }
        let mut flag = |name: &str, value: Option<String>| {
            if let Some(value) = value {
                line.push_str(&format!(" --{name} {value}"));
            }
        };
        flag("pages", a.pages);
        flag("start", a.start);
        flag("end", a.end);
        flag("every", a.every);
        flag("count", a.count.map(|n| n.to_string()));
        flag("detail", a.detail.map(|d| detail_name(d).to_string()));
        flag("per-sheet", a.per_sheet.map(|n| n.to_string()));
        flag("max-sheets", a.max_sheets.map(|n| n.to_string()));
        flag("type", a.kind.map(|k| k.name().to_string()));
        flag("preset", a.preset.map(|p| preset_name(p).to_string()));
        flag("password-file", a.password_file.as_deref().map(quote));
        if a.per_page == Some(true) {
            line.push_str(" --per-page");
        }
        line
    }
}

fn suggestions(
    source: &Source,
    estimates: &[Estimate],
    layouts: &[Layout],
    body_pt: Option<f64>,
    pages: Option<&[(usize, PageSummary)]>,
) -> Result<Vec<Suggestion>> {
    let per_sheet = |i: usize| estimates[i].per_sheet;
    let sheets = |i: usize| estimates[i].sheets.unwrap_or(1);
    let high = DETAILS.len() - 1;
    let mut list = Vec::new();
    match source {
        Source::Pdf(pdf) => {
            let page_sheets =
                |detail: usize, pages: &[usize]| planned_sheets(&layouts[detail], pdf, pages);
            let detail = readable_detail(estimates, body_pt);
            match pages {
                Some(pages) => {
                    let visual = visual_pages(pages);
                    if visual.is_empty() {
                        return Ok(list);
                    }
                    let numbers: Vec<usize> = visual.iter().map(|(page, _)| *page).collect();
                    list.push(Suggestion::render(
                        page_sheets(detail, &numbers)?,
                        CallArguments {
                            pages: Some(compact_ranges(&numbers)),
                            detail: non_default_detail(detail),
                            ..Default::default()
                        },
                    ));
                    let scale = visual_scale(estimates, detail);
                    let close: Vec<usize> = visual
                        .iter()
                        .filter(|(_, s)| s.small_pt.is_some_and(|pt| pt * scale < SKIMMABLE_PX))
                        .map(|(page, _)| *page)
                        .collect();
                    match close.as_slice() {
                        [] => {}
                        [page] => list.push(Suggestion::render(
                            1,
                            CallArguments {
                                pages: Some(page.to_string()),
                                per_sheet: Some(1),
                                ..Default::default()
                            },
                        )),
                        _ if detail < high => list.push(Suggestion::render(
                            page_sheets(high, &close)?,
                            CallArguments {
                                pages: Some(compact_ranges(&close)),
                                detail: Some(Detail::High),
                                ..Default::default()
                            },
                        )),
                        _ => list.push(Suggestion::render(
                            close.len(),
                            CallArguments {
                                pages: Some(compact_ranges(&close)),
                                per_sheet: Some(1),
                                ..Default::default()
                            },
                        )),
                    }
                }
                None => {
                    if sheets(detail) <= crate::layout::DEFAULT_MAX_SHEETS {
                        list.push(Suggestion::render(
                            sheets(detail),
                            CallArguments {
                                detail: non_default_detail(detail),
                                ..Default::default()
                            },
                        ));
                    } else {
                        let first: Vec<usize> =
                            (1..=(per_sheet(detail) * 2).min(pdf.page_count())).collect();
                        list.push(Suggestion::render(
                            page_sheets(detail, &first)?,
                            CallArguments {
                                pages: Some(format!("1-{}", first.len())),
                                detail: non_default_detail(detail),
                                ..Default::default()
                            },
                        ));
                        list.push(Suggestion::new(
                            Tool::Probe,
                            CallArguments {
                                per_page: Some(true),
                                ..Default::default()
                            },
                        ));
                    }
                    list.push(Suggestion::render(
                        1,
                        CallArguments {
                            pages: Some("1".to_string()),
                            detail: Some(Detail::High),
                            ..Default::default()
                        },
                    ));
                }
            }
        }
        Source::Images(_) => {
            list.push(Suggestion::render(sheets(1), CallArguments::default()));
            if sheets(1) > crate::layout::DEFAULT_MAX_SHEETS {
                list.push(Suggestion::render(
                    sheets(0),
                    CallArguments {
                        detail: Some(Detail::Low),
                        max_sheets: Some(sheets(0)),
                        ..Default::default()
                    },
                ));
            }
        }
        Source::Video(video) => {
            list.push(Suggestion::render(1, CallArguments::default()));
            if let Some(d) = video.duration {
                let step = nice_step(d / (per_sheet(1) as f64 * 3.0));
                let frames = sample_count(0.0, d, &Sampling::Every(step as f64));
                let every_sheets = frames.div_ceil(per_sheet(1)).max(3);
                list.push(Suggestion::render(
                    every_sheets,
                    CallArguments {
                        every: Some(format!("{step}s")),
                        max_sheets: Some(every_sheets),
                        ..Default::default()
                    },
                ));
                list.push(Suggestion::render(
                    1,
                    CallArguments {
                        start: Some("0:00".to_string()),
                        end: Some(format_time((d / 4.0).max(1.0), d >= 3600.0, false)),
                        detail: Some(Detail::High),
                        ..Default::default()
                    },
                ));
            }
        }
        Source::Anim(_) => {
            list.push(Suggestion::render(1, CallArguments::default()));
            list.push(Suggestion::render(
                per_sheet(0).div_ceil(per_sheet(1)),
                CallArguments {
                    count: Some(per_sheet(0)),
                    ..Default::default()
                },
            ));
        }
    }
    Ok(list)
}

/// Sheets `render` writes for these PDF pages, planned the way `render` plans them.
fn planned_sheets(layout: &Layout, pdf: &PdfSource, pages: &[usize]) -> Result<usize> {
    let shapes: Vec<Shape> = pages
        .iter()
        .map(|&page| page_shape(pdf.page_sizes[page - 1]))
        .collect();
    let sheets = plan(
        shapes.len(),
        median_aspect(shapes.iter().map(|s| s.aspect)),
        layout.limits,
        layout.chrome,
        &layout.options,
    )?;
    Ok(sheets.len())
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

/// The detail at this index into DETAILS, unless it is the default.
fn non_default_detail(detail: usize) -> Option<Detail> {
    (DETAILS[detail].0 != Detail::Medium).then_some(DETAILS[detail].0)
}

fn detail_name(detail: Detail) -> &'static str {
    DETAILS
        .iter()
        .find(|(d, _)| *d == detail)
        .map(|(_, name)| *name)
        .unwrap_or_default()
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
    if let Some(text_pages) = &report.text_pages {
        println!("text pages (a text extractor reads them for fewer tokens): {text_pages}");
    }
    if report.suggestions.is_empty() {
        println!("suggested commands: none, no page needs to be seen");
        return;
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

    fn target(input: &str) -> Target {
        Target {
            inputs: vec![input.to_string()],
            kind: None,
            preset: None,
            password_file: None,
        }
    }

    #[test]
    fn max_sheets_follow_each_default_limit() {
        let target = target("doc.pdf");
        let max_sheets = |sheets: usize, pinned: Option<usize>| {
            let suggestion = Suggestion::render(
                sheets,
                CallArguments {
                    max_sheets: pinned,
                    ..Default::default()
                },
            );
            (
                suggestion.command(&target),
                suggestion.call(&target).arguments.max_sheets,
            )
        };
        let plain = "gridity render doc.pdf".to_string();
        assert_eq!(max_sheets(4, None), (plain.clone(), None));
        assert_eq!(max_sheets(5, None), (plain.clone(), Some(5)));
        assert_eq!(max_sheets(10, None), (plain, Some(10)));
        assert_eq!(
            max_sheets(11, None),
            (
                "gridity render doc.pdf --max-sheets 11".to_string(),
                Some(11)
            )
        );
        assert_eq!(
            max_sheets(3, Some(3)),
            ("gridity render doc.pdf --max-sheets 3".to_string(), Some(3))
        );
    }

    #[test]
    fn command_and_call_carry_the_same_arguments() {
        let target = Target {
            kind: Some(InputKind::Pdf),
            preset: Some(Preset::Hires),
            password_file: Some("pw.txt".to_string()),
            ..target("doc.pdf")
        };
        let render = Suggestion::render(
            2,
            CallArguments {
                pages: Some("2,4-6".to_string()),
                detail: Some(Detail::High),
                ..Default::default()
            },
        );
        assert_eq!(
            render.command(&target),
            "gridity render doc.pdf --pages 2,4-6 --detail high --type pdf --preset hires --password-file pw.txt"
        );
        assert_eq!(
            render.call(&target),
            SuggestedCall {
                tool: Tool::Render,
                arguments: CallArguments {
                    inputs: vec!["doc.pdf".to_string()],
                    kind: Some(InputKind::Pdf),
                    pages: Some("2,4-6".to_string()),
                    preset: Some(Preset::Hires),
                    detail: Some(Detail::High),
                    password_file: Some("pw.txt".to_string()),
                    ..Default::default()
                },
            }
        );

        let probe = Suggestion::new(
            Tool::Probe,
            CallArguments {
                per_page: Some(true),
                ..Default::default()
            },
        );
        assert_eq!(
            probe.command(&target),
            "gridity probe doc.pdf --type pdf --preset hires --password-file pw.txt --per-page"
        );
        assert_eq!(probe.call(&target).arguments.per_page, Some(true));
    }

    #[cfg(unix)]
    #[test]
    fn calls_keep_paths_unquoted() {
        let target = Target {
            inputs: vec!["my shots".to_string(), "it's.png".to_string()],
            ..target("")
        };
        let render = Suggestion::render(1, CallArguments::default());
        assert_eq!(
            render.command(&target),
            "gridity render 'my shots' 'it'\\''s.png'"
        );
        assert_eq!(
            render.call(&target).arguments.inputs,
            ["my shots", "it's.png"]
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_quoting() {
        assert_eq!(quote(r"C:\docs\report.pdf"), r"C:\docs\report.pdf");
        assert_eq!(quote(r"C:\my docs\it's.pdf"), r#""C:\my docs\it's.pdf""#);
    }
}
