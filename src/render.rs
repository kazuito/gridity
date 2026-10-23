use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;

use crate::cli::{Format, InputKind, Preset, RenderArgs, SelectionArgs, SourceArgs};
use crate::compose::{Cell, Header, Ruler, Style, compose, header_height, place};
use crate::detect::detect;
use crate::error::{CliError, Reason, input, usage};
use crate::layout::{
    Chrome, Limits, MAX_CAPACITY, PlanOptions, SheetPlan, capacity, check_sheets, median_aspect,
    plan, token_cost,
};
use crate::output::{
    CellInfo, FileInfo, LimitsInfo, MANY_IMAGES_MAX_EDGE, MAX_BASE64_BYTES, Manifest,
    PageSizeGroup, SCHEMA_VERSION, SheetInfo, SourceInfo, Target, base64_len, check_not_input,
    encode, format_from_extension, write_atomic,
};
use crate::ruler::{Steps, choose_steps, steps_label};
use crate::select::{
    Crop, Sampling, format_time, min_interval, parse_crop, parse_pages, parse_time, sample_count,
    sample_times,
};
use crate::source::{Item, ItemKey, Source, Warnings, file_name, fit_size};
use crate::template::{expand_template, validate_template};
use crate::text::Fonts;

/// Animation timestamps collapse into frames, so they are limited by count rather than by sheets.
const MAX_ANIM_SAMPLES: usize = 1_000_000;

pub fn preset_name(preset: Preset) -> &'static str {
    match preset {
        Preset::Compat => "compat",
        Preset::Hires => "hires",
    }
}

/// Everything that decides how a sheet looks, shared by `render` and `probe`.
pub struct Layout {
    pub limits: Limits,
    pub chrome: Chrome,
    pub style: Style,
    pub font: Fonts,
    pub options: PlanOptions,
}

pub fn run(args: RenderArgs) -> Result<()> {
    let (json, dry_run, quiet) = (args.json, args.dry_run, args.quiet);
    let rendered = execute(args)?;
    let manifest = &rendered.manifest;
    if !quiet {
        for warning in &manifest.warnings {
            eprintln!("warning: {warning}");
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(manifest)?);
    } else if dry_run {
        print_dry_run(manifest, rendered.item_count);
    } else {
        for sheet in &manifest.sheets {
            if let Some(path) = &sheet.path {
                println!("{path}");
            }
        }
    }
    Ok(())
}

/// What a render produced: the manifest and the number of selected items.
pub struct RenderOutcome {
    pub manifest: Manifest,
    pub item_count: usize,
}

/// Plans and (unless `--dry-run`) writes the sheets without printing to stdout; `-v` progress
/// still goes to stderr.
pub fn execute(args: RenderArgs) -> Result<RenderOutcome> {
    let started = Instant::now();
    let log = |message: String| {
        if args.verbose {
            eprintln!("[{:>6.2}s] {message}", started.elapsed().as_secs_f64());
        }
    };

    let kind = detect(&args.inputs, args.kind)?;
    check_flags(kind, &args.selection, &args.source, &[])?;
    let template = match (&args.style.label, args.style.no_label) {
        (_, true) => None,
        (Some(t), _) => Some(t.clone()),
        (None, _) => Some(kind.default_label().to_string()),
    };
    if let Some(t) = &template {
        validate_template(t, "label", kind.label_vars(), kind)?;
    }

    let mut layout = build_layout(&args, kind)?;
    let source = Source::open(kind, &args.inputs, &args.source)?;
    log(format!("opened {} input", kind.name()));
    let crop = args.selection.crop.as_deref().map(parse_crop).transpose()?;
    layout.options.max_cell = native_bounds(&source).map(|size| match crop {
        Some(crop) => {
            let (_, _, w, h) = crop.pixels(size);
            (w, h)
        }
        None => size,
    });

    let frames = frame_limits(&source, crop, &layout);
    let items = select_items(&source, &args.selection, crop, &frames)?;
    let aspect = median_aspect(items.iter().map(|item| item.aspect));
    let sheets = plan(
        items.len(),
        aspect,
        layout.limits,
        layout.chrome,
        &layout.options,
    )?;
    log(format!(
        "{} items on {} sheet(s)",
        items.len(),
        sheets.len()
    ));

    let requested_format = args
        .format
        .or_else(|| args.output.as_deref().and_then(format_from_extension));
    let format = requested_format.unwrap_or_else(|| kind.default_format());
    let target = Target::resolve(
        args.output.as_deref(),
        &output_stem(&source, &args.inputs),
        &fingerprint(
            kind,
            &args.inputs,
            format!(
                "{:?}|{:?}|{:?}|{:?}|{}",
                args.selection, args.layout, args.style, args.format, args.quality
            ),
        ),
    )?;

    let warnings = Warnings::default();
    for warning in source.open_warnings() {
        warnings.push(warning);
    }
    let mut missing_chars = BTreeSet::new();
    let mut sheet_infos = Vec::with_capacity(sheets.len());
    let mut written = Vec::new();
    let input_paths = input_files(&source, &args.inputs);
    if !args.dry_run {
        target.create_dir()?;
    }

    let ruler_request = args.style.ruler_request();
    for (sheet_index, sheet) in sheets.iter().enumerate() {
        let sheet_items = &items[sheet.start..sheet.start + sheet.len];
        let planned_rects = predicted_rects(sheet, sheet_items, &layout);
        let ruler_steps = ruler_request.map(|requested| {
            let smallest = planned_rects
                .iter()
                .map(|r| (r[2], r[3]))
                .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1)))
                .unwrap_or((1, 1));
            choose_steps(crop, smallest, requested)
        });
        let labels: Vec<Option<String>> = sheet_items
            .iter()
            .map(|item| template.as_deref().map(|t| expand_label(t, item)))
            .collect();
        let header = (!args.style.no_header).then(|| {
            header_text(
                &source,
                &items,
                sheet,
                sheet_index,
                sheets.len(),
                args.style.title.as_deref(),
                ruler_steps,
            )
        });
        let drawn_text = labels
            .iter()
            .flatten()
            .chain(header.iter().flat_map(|h| [&h.name, &h.details]));
        for text in drawn_text {
            missing_chars.extend(layout.font.missing(text));
        }
        let (width, height) = sheet.grid.sheet_size(layout.chrome);
        let boxes = vec![(sheet.grid.cell_width, sheet.grid.cell_height); sheet.len];

        let mut errors = vec![None; sheet.len];
        let (rects, path, bytes) = if args.dry_run {
            (planned_rects, None, None)
        } else {
            let rendered = source.render(sheet_items, &boxes, &warnings)?;
            log(format!(
                "rendered sheet {}/{}",
                sheet_index + 1,
                sheets.len()
            ));
            let mut cells = Vec::with_capacity(rendered.len());
            for ((cell, label), (item, error)) in rendered
                .into_iter()
                .zip(&labels)
                .zip(sheet_items.iter().zip(&mut errors))
            {
                if let Some(message) = &cell.error {
                    warnings.push(format!(
                        "item {} is drawn as an empty cell: {message}",
                        item.vars.index
                    ));
                }
                *error = cell.error;
                cells.push(Cell {
                    image: cell.image,
                    label: label.clone(),
                    ruler: ruler_steps.map(|steps| Ruler {
                        steps,
                        crop: item.crop,
                    }),
                });
            }
            let (canvas, rects) = compose(
                sheet.grid,
                layout.chrome,
                &layout.style,
                &layout.font,
                header.as_ref(),
                cells,
            );

            let mut sheet_format = format;
            let mut data = encode(&canvas, sheet_format, args.quality)?;
            let too_large = |data: &[u8]| base64_len(data.len()) > MAX_BASE64_BYTES;
            if too_large(&data) && sheet_format == Format::Png && requested_format.is_none() {
                warnings.push(format!(
                    "sheet {} exceeded 5 MB as base64 PNG and was written as JPEG instead",
                    sheet_index + 1
                ));
                sheet_format = Format::Jpeg;
                data = encode(&canvas, sheet_format, args.quality)?;
            }
            if too_large(&data) {
                warnings.push(format!(
                    "sheet {} is {:.1} MB as base64; Amazon Bedrock and Google Cloud reject images over 5 MB",
                    sheet_index + 1,
                    base64_len(data.len()) as f64 / 1e6
                ));
            }
            let path = target.sheet_path(sheet_index, sheets.len(), sheet_format);
            check_not_input(&path, &input_paths)?;
            write_atomic(&path, &data)?;
            written.push(path.clone());
            (rects, Some(path), Some(data.len()))
        };

        if width.max(height) > MANY_IMAGES_MAX_EDGE {
            warnings.push(format!(
                "sheets over {MANY_IMAGES_MAX_EDGE} px are rejected in requests with more than 20 images; use --preset compat for those"
            ));
        }
        sheet_infos.push(SheetInfo {
            path: path.map(|p| p.display().to_string()),
            width,
            height,
            est_tokens: token_cost(width, height, layout.limits.rule),
            bytes,
            grid: sheet.grid,
            cells: sheet_items
                .iter()
                .zip(labels)
                .zip(rects)
                .zip(errors)
                .map(|(((item, label), rect), error)| CellInfo {
                    error,
                    ..cell_info(item, label, rect, natural_width(&source, item))
                })
                .collect(),
        });
    }
    if !args.dry_run {
        target.remove_stale(&written);
    }
    if !missing_chars.is_empty() {
        let listed: Vec<String> = missing_chars
            .iter()
            .map(|c| format!("{c} (U+{:04X})", *c as u32))
            .collect();
        warnings.push(format!(
            "no font can draw {}; they appear as boxes. Pass --font with a font that has them",
            listed.join(", ")
        ));
    }
    log("done".to_string());

    let manifest = Manifest {
        schema_version: SCHEMA_VERSION,
        source: source_info(&source, false),
        limits: LimitsInfo {
            preset: preset_name(args.layout.preset).to_string(),
            rule: layout.limits.rule.name().to_string(),
            max_edge: layout.limits.max_edge,
            max_tokens: layout.limits.max_tokens,
        },
        sheets: sheet_infos,
        warnings: warnings.take(),
    };
    Ok(RenderOutcome {
        manifest,
        item_count: items.len(),
    })
}

pub fn build_layout(args: &RenderArgs, kind: InputKind) -> Result<Layout> {
    let font = Fonts::load(&args.style.font)?;
    let label_pos = (!args.style.no_label).then(|| {
        args.style
            .label_pos
            .unwrap_or_else(|| kind.default_label_pos())
    });
    let style = Style::new(
        args.style.bg,
        label_pos,
        args.style.label_size,
        args.style.label_color,
        args.style.label_bg,
    );
    let chrome = Chrome {
        padding: args.layout.padding,
        gap: args.layout.gap,
        header: if args.style.no_header {
            0
        } else {
            header_height(&font)
        },
        label: style.label_strip(&font),
    };
    let limits = Limits::new(
        args.layout.preset,
        args.layout.max_edge,
        args.layout.max_tokens,
    );
    let options = PlanOptions {
        per_sheet: args.layout.per_sheet,
        cols: args.layout.cols,
        rows: args.layout.rows,
        min_cell: kind.min_cell(args.layout.detail),
        max_cell: None,
        max_sheets: args.layout.max_sheets,
    };
    if let (Some(n), Some(c), Some(r)) = (options.per_sheet, options.cols, options.rows)
        && n > (c * r) as usize
    {
        return Err(usage(
            Reason::DoesNotFit,
            format!("--per-sheet {n} does not fit in a {c}x{r} grid"),
        )
        .into());
    }
    Ok(Layout {
        limits,
        chrome,
        style,
        font,
        options,
    })
}

/// Flags that only make sense for another input type are errors, not silently ignored.
/// `more_pdf_only` lists a command's own PDF flags and whether each was given.
pub fn check_flags(
    kind: InputKind,
    sel: &SelectionArgs,
    source: &SourceArgs,
    more_pdf_only: &[(&str, bool)],
) -> Result<(), CliError> {
    let mut pdf_only = vec![
        ("--pages", sel.pages.is_some()),
        ("--password", source.password.password.is_some()),
        ("--password-file", source.password.password_file.is_some()),
    ];
    pdf_only.extend_from_slice(more_pdf_only);
    let time_only = [
        ("--start", sel.start.is_some()),
        ("--end", sel.end.is_some()),
        ("--every", sel.every.is_some()),
        ("--count", sel.count.is_some()),
        ("--at", sel.at.is_some()),
    ];
    let offending: Vec<&str> = match kind {
        InputKind::Pdf => time_only.iter().filter(|f| f.1).map(|f| f.0).collect(),
        InputKind::Video | InputKind::Anim => {
            pdf_only.iter().filter(|f| f.1).map(|f| f.0).collect()
        }
        InputKind::Images => pdf_only
            .iter()
            .chain(&time_only)
            .filter(|f| f.1)
            .map(|f| f.0)
            .collect(),
    };
    if offending.is_empty() {
        return Ok(());
    }
    Err(usage(
        Reason::FlagNotForInput,
        format!(
            "{} cannot be used with {} input",
            offending.join(", "),
            kind.name()
        ),
    )
    .with_hint(format!("see: gridity guide {}", kind.guide_topic())))
}

fn expand_label(template: &str, item: &Item) -> String {
    let v = &item.vars;
    expand_template(template, |name| match name {
        "index" => Some(v.index.to_string()),
        "total" => Some(v.total.to_string()),
        "page" => Some(v.page.map(|p| p.to_string()).unwrap_or_default()),
        "time" => Some(v.time.clone().unwrap_or_default()),
        "frame" => Some(v.frame.map(|f| f.to_string()).unwrap_or_default()),
        "name" => Some(v.name.clone().unwrap_or_default()),
        _ => None,
    })
}

/// How a command bounds the frames of a video or animation selection.
pub struct FrameLimits {
    /// Frames taken when none of --every, --count and --at is given; `None` requires one.
    pub default_count: Option<usize>,
    pub limit: CountLimit,
}

/// Checked on the number of video timestamps before they are generated.
pub enum CountLimit {
    /// `render`: the frames must fit on `max_sheets` sheets of about `per_sheet` cells.
    Sheets { per_sheet: usize, max_sheets: usize },
    /// `export`: one file per item, at most this many.
    Files(usize),
}

impl CountLimit {
    pub fn check(&self, count: usize) -> Result<(), CliError> {
        match *self {
            // `per_sheet` is exact only below MAX_CAPACITY, so `plan` does the exact check.
            CountLimit::Sheets {
                per_sheet,
                max_sheets,
            } => {
                if count > max_sheets.saturating_mul(per_sheet.max(MAX_CAPACITY)) {
                    check_sheets(count, per_sheet, max_sheets)?;
                }
                Ok(())
            }
            CountLimit::Files(max_files) if count > max_files => Err(usage(
                Reason::TooManyFiles,
                format!("the selection has {count} items, more than --max-files {max_files}"),
            )
            .with_hint(format!(
                "narrow the selection (--pages, --start/--end, --count, --every) or pass --max-files {count}"
            ))),
            CountLimit::Files(_) => Ok(()),
        }
    }
}

/// Frames default to as many as fit on one sheet.
fn frame_limits(source: &Source, crop: Option<Crop>, layout: &Layout) -> FrameLimits {
    let frame_aspect = match source {
        Source::Video(video) => Some(video.aspect()),
        Source::Anim(anim) => Some(anim.aspect()),
        Source::Pdf(_) | Source::Images(_) => None,
    };
    let per_sheet = frame_aspect.map(|aspect| {
        capacity(
            aspect * crop.map_or(1.0, |c| c.width / c.height),
            layout.limits,
            layout.chrome,
            &layout.options,
            MAX_CAPACITY,
        )
    });
    FrameLimits {
        default_count: per_sheet,
        limit: CountLimit::Sheets {
            per_sheet: per_sheet.unwrap_or(1),
            max_sheets: layout.options.max_sheets,
        },
    }
}

/// Resolves the user's selection into the items to place, in order, cropped if asked.
pub fn select_items(
    source: &Source,
    sel: &SelectionArgs,
    crop: Option<Crop>,
    frames: &FrameLimits,
) -> Result<Vec<Item>> {
    let mut items = select_uncropped(source, sel, frames)?;
    if let Some(crop) = crop {
        apply_crop(&mut items, crop);
    }
    Ok(items)
}

fn select_uncropped(
    source: &Source,
    sel: &SelectionArgs,
    frames: &FrameLimits,
) -> Result<Vec<Item>> {
    match source {
        Source::Pdf(pdf) => {
            let pages = match &sel.pages {
                Some(spec) => parse_pages(spec, pdf.page_count())?,
                None => (1..=pdf.page_count()).collect(),
            };
            Ok(pdf.items(&pages))
        }
        Source::Images(images) => Ok(images.items()),
        Source::Video(video) => {
            let (start, end, sampling) = select_range(sel, video.duration, frames.default_count)?;
            frames.limit.check(sample_count(start, end, &sampling))?;
            let times = sample_times(start, end, &sampling);
            let long = video.duration.is_some_and(|d| d >= 3600.0);
            let labels = time_labels(&times, long);
            Ok(video.items(&times, &labels))
        }
        Source::Anim(anim) => {
            let (start, end, sampling) =
                select_range(sel, Some(anim.duration), frames.default_count)?;
            let count = sample_count(start, end, &sampling);
            if count > MAX_ANIM_SAMPLES {
                return Err(usage(
                    Reason::TooManySamples,
                    format!("the selection takes {count} timestamps, more than {MAX_ANIM_SAMPLES}"),
                )
                .with_hint("use a longer --every or a smaller --count")
                .into());
            }
            let mut seen = vec![false; anim.starts.len()];
            let mut frames: Vec<usize> = Vec::new();
            for t in sample_times(start, end, &sampling) {
                let frame = anim.frame_at(t);
                if !seen[frame] {
                    seen[frame] = true;
                    frames.push(frame);
                }
            }
            let starts: Vec<f64> = frames.iter().map(|&f| anim.starts[f]).collect();
            let labels = time_labels(&starts, anim.duration >= 3600.0);
            Ok(anim.items(&frames, &labels))
        }
    }
}

/// Narrows every item to the cropped region; raster sizes are rounded to whole pixels.
fn apply_crop(items: &mut [Item], crop: Crop) {
    for item in items {
        match item.native {
            Some(size) => {
                let (_, _, w, h) = crop.pixels(size);
                item.native = Some((w, h));
                item.aspect = w as f64 / h as f64;
            }
            None => item.aspect *= crop.width / crop.height,
        }
        item.crop = Some(crop);
    }
}

/// Resolves --start, --end and the sampling option against the media duration.
fn select_range(
    sel: &SelectionArgs,
    duration: Option<f64>,
    default_count: Option<usize>,
) -> Result<(f64, f64, Sampling), CliError> {
    let start = sel
        .start
        .as_deref()
        .map(parse_time)
        .transpose()?
        .unwrap_or(0.0);
    let end = sel.end.as_deref().map(parse_time).transpose()?;
    let sampling = match (
        Sampling::from_args(sel.every.as_deref(), sel.count, sel.at.as_deref())?,
        default_count,
    ) {
        (Some(sampling), _) => sampling,
        (None, Some(count)) => Sampling::Count(count),
        (None, None) => {
            return Err(usage(
                Reason::InvalidArgument,
                "choose the frames with --at, --every or --count",
            )
            .with_hint("e.g. --at 1:30,2:45 or --every 10s"));
        }
    };

    let end = match (end, duration) {
        (Some(end), Some(d)) if end > d + 0.5 => {
            return Err(usage(
                Reason::OutsideMedia,
                format!(
                    "--end {} is past the end of the media ({})",
                    format_time(end, false, false),
                    format_time(d, false, false)
                ),
            ));
        }
        (Some(end), Some(d)) => end.min(d),
        (Some(end), None) => end,
        (None, Some(d)) => d,
        (None, None) if matches!(sampling, Sampling::At(_)) => f64::INFINITY,
        (None, None) => {
            return Err(input(
                Reason::UnknownDuration,
                "the duration of the media is unknown",
            )
            .with_hint("pass --end or --at"));
        }
    };
    if start >= end {
        return Err(usage(
            Reason::OutsideMedia,
            "--start must be before --end and the end of the media",
        ));
    }
    if let Sampling::At(times) = &sampling
        && let Some(&bad) = times.iter().find(|&&t| t < start || t > end)
    {
        return Err(usage(
            Reason::OutsideMedia,
            format!(
                "--at {} is outside the selected range or the media",
                format_time(bad, false, true)
            ),
        ));
    }
    Ok((start, end, sampling))
}

fn time_labels(times: &[f64], long: bool) -> Vec<String> {
    let fine = needs_tenths(times);
    times.iter().map(|&t| format_time(t, long, fine)).collect()
}

/// Whether whole seconds would hide the difference between `times`.
fn needs_tenths(times: &[f64]) -> bool {
    min_interval(times).is_some_and(|d| d < 1.0)
        || times.iter().any(|t| t.fract() != 0.0 && *t < 1.0)
}

/// Largest native width and height among raster inputs; None for PDFs, which scale freely.
pub fn native_bounds(source: &Source) -> Option<(u32, u32)> {
    match source {
        Source::Pdf(_) => None,
        Source::Video(video) => Some(video.display),
        Source::Anim(anim) => Some(anim.size),
        Source::Images(images) => images
            .files
            .iter()
            .map(|f| f.size)
            .reduce(|a, b| (a.0.max(b.0), a.1.max(b.1))),
    }
}

fn predicted_rects(sheet: &SheetPlan, items: &[Item], layout: &Layout) -> Vec<[u32; 4]> {
    let grid = sheet.grid;
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let size = fit_size(
                item.aspect,
                (grid.cell_width, grid.cell_height),
                item.native,
            );
            place(grid, layout.chrome, i, size, layout.style.label_pos)
        })
        .collect()
}

/// Width of an item before scaling: points for PDF pages, pixels for raster sources.
pub fn natural_width(source: &Source, item: &Item) -> f64 {
    match (source, &item.key) {
        (Source::Pdf(pdf), ItemKey::Page(index)) => {
            pdf.page_sizes[*index].0 as f64 * item.crop.map_or(1.0, |c| c.width)
        }
        _ => item.native.map_or(1.0, |(w, _)| w as f64),
    }
}

/// What an item shows, as reported in JSON: 1-based page, time in seconds, 1-based frame or
/// absolute file path.
pub struct ItemRef {
    pub page: Option<usize>,
    pub time: Option<f64>,
    pub frame: Option<usize>,
    pub file: Option<String>,
}

pub fn item_ref(item: &Item) -> ItemRef {
    let (page, time, frame, file) = match &item.key {
        ItemKey::Page(index) => (Some(index + 1), None, None, None),
        ItemKey::Time(t) => (None, Some(round3(*t)), None, None),
        ItemKey::Frame(index, start) => (None, Some(round3(*start)), Some(index + 1), None),
        ItemKey::File(path) => (None, None, None, Some(absolute(path))),
    };
    ItemRef {
        page,
        time,
        frame,
        file,
    }
}

fn cell_info(item: &Item, label: Option<String>, rect: [u32; 4], natural_width: f64) -> CellInfo {
    let ItemRef {
        page,
        time,
        frame,
        file,
    } = item_ref(item);
    CellInfo {
        index: item.vars.index,
        label,
        page,
        time,
        frame,
        file,
        rect,
        scale: round3(rect[2] as f64 / natural_width),
        crop: item.crop.map(|c| [c.x, c.y, c.width, c.height]),
        error: None,
    }
}

pub fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn absolute(path: &Path) -> String {
    std::path::absolute(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

fn header_text(
    source: &Source,
    items: &[Item],
    sheet: &SheetPlan,
    sheet_index: usize,
    sheet_count: usize,
    title: Option<&str>,
    ruler_steps: Option<Steps>,
) -> Header {
    let sheet_part =
        (sheet_count > 1).then(|| format!("sheet {}/{}", sheet_index + 1, sheet_count));
    let ruler_part = ruler_steps.map(|steps| format!("ruler {}", steps_label(steps)));
    let join = |parts: &[Option<String>]| {
        parts
            .iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ")
    };
    if let Some(title) = title {
        return Header {
            name: title.to_string(),
            details: join(&[ruler_part, sheet_part]),
        };
    }
    let first = &items[sheet.start];
    let last = &items[sheet.start + sheet.len - 1];
    let range = |a: String, b: String| if a == b { a } else { format!("{a}–{b}") };
    let (name, scope) = match source {
        Source::Pdf(pdf) => (
            file_name(&pdf.path),
            format!(
                "p.{} of {}",
                range(page_of(first), page_of(last)),
                pdf.page_count()
            ),
        ),
        Source::Video(video) => (
            file_name(&video.path),
            match video.duration {
                Some(d) => format!(
                    "{} of {}",
                    range(time_of(first), time_of(last)),
                    format_time(d, d >= 3600.0, false)
                ),
                None => range(time_of(first), time_of(last)),
            },
        ),
        Source::Anim(anim) => (
            file_name(&anim.path),
            format!(
                "{} of {} ({} frames)",
                range(time_of(first), time_of(last)),
                format_time(anim.duration, false, anim.duration < 10.0),
                anim.starts.len()
            ),
        ),
        Source::Images(images) => (
            images.name.clone(),
            format!(
                "{} of {}",
                range(first.vars.index.to_string(), last.vars.index.to_string()),
                items.len()
            ),
        ),
    };
    let crop_part = first.crop.map(|c| {
        format!(
            "crop {},{},{},{}",
            round3(c.x),
            round3(c.y),
            round3(c.width),
            round3(c.height)
        )
    });
    Header {
        name,
        details: join(&[Some(scope), crop_part, ruler_part, sheet_part]),
    }
}

fn page_of(item: &Item) -> String {
    item.vars.page.map(|p| p.to_string()).unwrap_or_default()
}

fn time_of(item: &Item) -> String {
    item.vars.time.clone().unwrap_or_default()
}

pub fn source_info(source: &Source, detailed: bool) -> SourceInfo {
    match source {
        Source::Pdf(pdf) => SourceInfo::Pdf {
            path: absolute(&pdf.path),
            pages: pdf.page_count(),
            title: pdf.title.clone(),
            page_sizes: detailed.then(|| page_size_groups(&pdf.page_sizes)),
        },
        Source::Video(video) => SourceInfo::Video {
            path: absolute(&video.path),
            duration: video.duration.map(round3),
            width: video.display.0,
            height: video.display.1,
            coded_width: video.coded.0,
            coded_height: video.coded.1,
            rotation: video.rotation,
            fps: video.fps.map(round3),
            codec: video.codec.clone(),
        },
        Source::Anim(anim) => SourceInfo::Anim {
            path: absolute(&anim.path),
            frames: anim.starts.len(),
            duration: round3(anim.duration),
            width: anim.size.0,
            height: anim.size.1,
        },
        Source::Images(images) => SourceInfo::Images {
            name: images.name.clone(),
            count: images.files.len(),
            files: detailed.then(|| {
                images
                    .files
                    .iter()
                    .map(|f| FileInfo {
                        path: absolute(&f.path),
                        width: f.size.0,
                        height: f.size.1,
                    })
                    .collect()
            }),
        },
    }
}

/// Groups pages of the same size, e.g. 595x842 pt for pages "1-12".
fn page_size_groups(sizes: &[(f32, f32)]) -> Vec<PageSizeGroup> {
    let mut groups: Vec<((u32, u32), Vec<usize>)> = Vec::new();
    for (i, &(w, h)) in sizes.iter().enumerate() {
        let key = (w.round() as u32, h.round() as u32);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, pages)) => pages.push(i + 1),
            None => groups.push((key, vec![i + 1])),
        }
    }
    groups
        .into_iter()
        .map(|((w, h), pages)| PageSizeGroup {
            width_pt: w,
            height_pt: h,
            orientation: if w > h { "landscape" } else { "portrait" }.to_string(),
            count: pages.len(),
            pages: compact_ranges(&pages),
        })
        .collect()
}

pub fn compact_ranges(numbers: &[usize]) -> String {
    let mut parts = Vec::new();
    let mut i = 0;
    while i < numbers.len() {
        let start = numbers[i];
        let mut end = start;
        while i + 1 < numbers.len() && numbers[i + 1] == end + 1 {
            i += 1;
            end = numbers[i];
        }
        parts.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
        i += 1;
    }
    parts.join(",")
}

/// Canonical paths of the files read, which outputs must never replace.
pub fn input_files(source: &Source, inputs: &[PathBuf]) -> Vec<PathBuf> {
    let paths: Vec<&Path> = match source {
        Source::Images(images) => images.files.iter().map(|f| f.path.as_path()).collect(),
        Source::Pdf(_) | Source::Video(_) | Source::Anim(_) => {
            inputs.iter().map(PathBuf::as_path).collect()
        }
    };
    paths
        .into_iter()
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .collect()
}

pub fn output_stem(source: &Source, inputs: &[PathBuf]) -> String {
    let stem_of = |p: &Path| {
        p.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    match source {
        Source::Images(_) if inputs.len() > 1 => "images".to_string(),
        _ => stem_of(&inputs[0]),
    }
}

/// Identifies "the same request" (inputs and options) so re-running it overwrites one output directory.
pub fn fingerprint(kind: InputKind, inputs: &[PathBuf], options: String) -> String {
    let mut parts = vec![format!("{kind:?}")];
    for path in inputs {
        parts.push(
            std::fs::canonicalize(path)
                .unwrap_or_else(|_| path.clone())
                .display()
                .to_string(),
        );
    }
    parts.push(options);
    parts.join("\n")
}

fn print_dry_run(manifest: &Manifest, item_count: usize) {
    println!(
        "dry run: {item_count} items on {} sheet(s), preset {} (max edge {}px, {} tokens)",
        manifest.sheets.len(),
        manifest.limits.preset,
        manifest.limits.max_edge,
        manifest.limits.max_tokens
    );
    for (i, sheet) in manifest.sheets.iter().enumerate() {
        let first = sheet
            .cells
            .first()
            .and_then(|c| c.label.clone())
            .unwrap_or_default();
        let last = sheet
            .cells
            .last()
            .and_then(|c| c.label.clone())
            .unwrap_or_default();
        println!(
            "sheet {}: {} items, {}x{} grid, cells {}x{}, sheet {}x{}, ~{} tokens, {} .. {}",
            i + 1,
            sheet.cells.len(),
            sheet.grid.cols,
            sheet.grid.rows,
            sheet.grid.cell_width,
            sheet.grid.cell_height,
            sheet.width,
            sheet.height,
            sheet.est_tokens,
            first,
            last
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::LabelVars;

    #[test]
    fn labels_expand() {
        let item = Item {
            aspect: 1.0,
            native: None,
            key: ItemKey::Page(4),
            vars: LabelVars {
                index: 2,
                total: 9,
                page: Some(5),
                ..LabelVars::default()
            },
            crop: None,
        };
        assert_eq!(expand_label("p.{page} {index}/{total}", &item), "p.5 2/9");
    }

    #[test]
    fn ranges_compact() {
        assert_eq!(compact_ranges(&[1, 2, 3, 5, 7, 8]), "1-3,5,7-8");
        assert_eq!(compact_ranges(&[4]), "4");
    }
}
