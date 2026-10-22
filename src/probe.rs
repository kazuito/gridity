use anyhow::Result;
use clap::Parser;
use serde_json::{Value, json};

use crate::cli::{Cli, Command, Detail, InputKind, Preset, ProbeArgs, RenderArgs};
use crate::detect::detect;
use crate::error::usage;
use crate::layout::{capacity, plan};
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

pub fn run(args: ProbeArgs) -> Result<()> {
    let kind = detect(&args.inputs, args.kind)?;
    let source = Source::open(kind, &args.inputs, &args.source)?;
    let info = crate::render::source_info(&source, true);
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
        let mut estimate = json!({ "detail": name, "per_sheet": per_sheet });
        if let Some(count) = count {
            estimate["sheets"] = json!(count.div_ceil(per_sheet));
        }
        let scale = sheet_scale(&layout, count.unwrap_or(per_sheet), &shapes)?;
        estimate["scale"] = json!(round3(scale));
        if kind == InputKind::Pdf {
            estimate["text_12pt_px"] = json!(round1(12.0 * scale));
        }
        if let Some(body) = body_pt {
            estimate["body_text_px"] = json!(round1(body * scale));
        }
        if let Some(shapes) = &visual_shapes {
            estimate["visual_scale"] = json!(round3(sheet_scale(&layout, shapes.len(), shapes)?));
        }
        if detail == Detail::Medium {
            estimate["default"] = json!(true);
        }
        estimates.push(estimate);
    }
    let suggestions = suggestions(&source, &args, &estimates, body_pt, per_page);

    if args.json {
        let mut report = json!({
            "source": info,
            "preset": preset_name(args.preset),
            "estimates": estimates,
            "suggestions": suggestions,
        });
        if let Some(body) = body_pt {
            report["body_text_pt"] = json!(body);
        }
        if let Some(pages) = per_page {
            report["pages"] = json!(
                pages
                    .iter()
                    .map(|(page, summary)| {
                        let mut value = json!({ "page": page });
                        value
                            .as_object_mut()
                            .unwrap()
                            .extend(json!(summary).as_object().unwrap().clone());
                        value
                    })
                    .collect::<Vec<_>>()
            );
        }
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_text(&source, &info, args.preset, &estimates, body_pt, per_page);
        println!(
            "suggested commands ({} input; heuristic starting points, adjust after looking at the sheets):",
            kind_name(source.kind())
        );
        for s in &suggestions {
            println!("  {s}");
        }
    }
    Ok(())
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
            return Err(usage(format!(
                "--per-page works with PDF input only, not {}",
                kind_name(source.kind())
            ))
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
    estimates: &[Value],
    body_pt: Option<f64>,
    pages: Option<&[(usize, PageSummary)]>,
) -> Vec<String> {
    let input = args
        .inputs
        .iter()
        .map(|p| quote(&p.display().to_string()))
        .collect::<Vec<_>>()
        .join(" ");
    let per_sheet = |i: usize| estimates[i]["per_sheet"].as_u64().unwrap_or(1) as usize;
    let sheets = |i: usize| estimates[i]["sheets"].as_u64().unwrap_or(1) as usize;
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
fn readable_detail(estimates: &[Value], body_pt: Option<f64>) -> usize {
    let Some(body) = body_pt else {
        return 1;
    };
    (0..DETAILS.len())
        .find(|&detail| body * visual_scale(estimates, detail) >= READABLE_PX)
        .unwrap_or(DETAILS.len() - 1)
}

fn visual_scale(estimates: &[Value], detail: usize) -> f64 {
    let estimate = &estimates[detail];
    estimate["visual_scale"]
        .as_f64()
        .or(estimate["scale"].as_f64())
        .unwrap_or(1.0)
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

pub fn quote(s: &str) -> String {
    if s.chars().all(|c| c.is_alphanumeric() || "-_./".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn print_text(
    source: &Source,
    info: &Value,
    preset: Preset,
    estimates: &[Value],
    body_pt: Option<f64>,
    pages: Option<&[(usize, PageSummary)]>,
) {
    let unit = match source.kind() {
        InputKind::Pdf => "pages",
        InputKind::Images => "images",
        InputKind::Video | InputKind::Anim => "frames",
    };
    match source {
        Source::Pdf(pdf) => {
            println!(
                "{}: PDF, {} pages",
                info["path"].as_str().unwrap_or_default(),
                pdf.page_count()
            );
            if let Some(title) = &pdf.title {
                println!("  title: {title}");
            }
            for group in info["page_sizes"].as_array().into_iter().flatten() {
                println!(
                    "  {}x{} pt {} x{} (p.{})",
                    group["width_pt"],
                    group["height_pt"],
                    group["orientation"].as_str().unwrap_or_default(),
                    group["count"],
                    group["pages"].as_str().unwrap_or_default()
                );
            }
        }
        Source::Video(video) => {
            println!("{}: video", info["path"].as_str().unwrap_or_default());
            let duration = video
                .duration
                .map(|d| format!("{} ({d:.2}s)", format_time(d, d >= 3600.0, false)))
                .unwrap_or_else(|| "unknown".to_string());
            println!("  duration: {duration}");
            println!(
                "  display size: {}x{} (coded {}x{}, rotation {}°)",
                video.display.0, video.display.1, video.coded.0, video.coded.1, video.rotation
            );
            if let Some(fps) = video.fps {
                println!("  fps: {fps:.3}");
            }
            if let Some(codec) = &video.codec {
                println!("  codec: {codec}");
            }
        }
        Source::Anim(anim) => {
            println!("{}: animation", info["path"].as_str().unwrap_or_default());
            println!(
                "  {} frames, {:.2}s, {}x{}",
                anim.starts.len(),
                anim.duration,
                anim.size.0,
                anim.size.1
            );
        }
        Source::Images(images) => {
            println!("{}: {} images", images.name, images.files.len());
            for file in images.files.iter().take(20) {
                println!(
                    "  {} ({}x{})",
                    file.path.display(),
                    file.size.0,
                    file.size.1
                );
            }
            if images.files.len() > 20 {
                println!("  ... and {} more", images.files.len() - 20);
            }
        }
    }
    println!("sheets (preset {}):", preset_name(preset));
    for estimate in estimates {
        let detail = estimate["detail"].as_str().unwrap_or_default();
        let marker = if estimate["default"] == true {
            " (default)"
        } else {
            ""
        };
        let sheets = estimate["sheets"]
            .as_u64()
            .map(|s| format!(" -> {s} sheet(s)"))
            .unwrap_or_default();
        let scale = estimate["scale"].as_f64().unwrap_or_default();
        let scale = match (
            estimate["body_text_px"].as_f64(),
            estimate["text_12pt_px"].as_f64(),
        ) {
            (Some(px), _) => format!(
                "{scale:.2} px/pt (body {}pt text = {px:.0}px{})",
                body_pt.unwrap_or_default(),
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
            "  --detail {detail:<6} {} {unit}/sheet{sheets}, {scale}{marker}",
            estimate["per_sheet"]
        );
    }
    if let Some(pages) = pages {
        println!("pages:");
        for (page, summary) in pages {
            println!("  p.{page:<4} {}", summary.describe());
        }
    }
}
