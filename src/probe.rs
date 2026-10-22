use anyhow::Result;
use clap::Parser;
use serde_json::{Value, json};

use crate::cli::{Cli, Command, Detail, InputKind, Preset, ProbeArgs, RenderArgs};
use crate::detect::detect;
use crate::layout::capacity;
use crate::render::{build_layout, kind_name, native_bounds, preset_name};
use crate::select::format_time;
use crate::source::Source;

const DETAILS: [(Detail, &str); 3] = [
    (Detail::Low, "low"),
    (Detail::Medium, "medium"),
    (Detail::High, "high"),
];

pub fn run(args: ProbeArgs) -> Result<()> {
    let kind = detect(&args.inputs, args.kind)?;
    let source = Source::open(kind, &args.inputs, &args.source)?;
    let info = crate::render::source_info(&source, true);
    let (count, aspect) = count_and_aspect(&source);

    let mut estimates = Vec::new();
    for (detail, name) in DETAILS {
        let mut layout = build_layout(&default_render_args(args.preset, name), kind)?;
        layout.options.max_cell = native_bounds(&source);
        let per_sheet = capacity(aspect, layout.limits, layout.chrome, &layout.options, 200);
        let mut estimate = json!({ "detail": name, "per_sheet": per_sheet });
        if let Some(count) = count {
            estimate["sheets"] = json!(count.div_ceil(per_sheet));
        }
        if detail == Detail::Medium {
            estimate["default"] = json!(true);
        }
        estimates.push(estimate);
    }
    let suggestions = suggestions(&source, &args, &estimates);

    if args.json {
        let report = json!({
            "source": info,
            "preset": preset_name(args.preset),
            "estimates": estimates,
            "suggestions": suggestions,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_text(&source, &info, args.preset, &estimates, &suggestions);
    }
    Ok(())
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

/// Number of items for a full render (None for time-based media) and their typical aspect ratio.
fn count_and_aspect(source: &Source) -> (Option<usize>, f64) {
    let median = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        values[values.len() / 2]
    };
    match source {
        Source::Pdf(pdf) => (
            Some(pdf.page_count()),
            median(
                pdf.page_sizes
                    .iter()
                    .map(|(w, h)| (*w / *h) as f64)
                    .collect(),
            ),
        ),
        Source::Images(images) => (
            Some(images.files.len()),
            median(
                images
                    .files
                    .iter()
                    .map(|f| f.size.0 as f64 / f.size.1 as f64)
                    .collect(),
            ),
        ),
        Source::Video(video) => (None, video.aspect()),
        Source::Anim(anim) => (None, anim.aspect()),
    }
}

fn suggestions(source: &Source, args: &ProbeArgs, estimates: &[Value]) -> Vec<String> {
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
            if sheets(1) <= 10 {
                list.push(format!("gridity render {input}"));
            } else {
                list.push(format!(
                    "gridity render {input} --pages 1-{}",
                    (per_sheet(1) * 2).min(pdf.page_count())
                ));
                list.push(format!(
                    "gridity render {input} --detail low --max-sheets {}",
                    sheets(0)
                ));
            }
            list.push(format!("gridity render {input} --pages 1 --detail high"));
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

fn nice_step(seconds: f64) -> u64 {
    [1, 2, 5, 10, 15, 30, 60, 120, 300, 600]
        .into_iter()
        .find(|&s| s as f64 >= seconds)
        .unwrap_or(900)
}

fn quote(s: &str) -> String {
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
    suggestions: &[String],
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
        println!(
            "  --detail {detail:<6} {} {unit}/sheet{sheets}{marker}",
            estimate["per_sheet"]
        );
    }
    println!("suggested commands ({} input):", kind_name(source.kind()));
    for s in suggestions {
        println!("  {s}");
    }
}
