use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use rayon::prelude::*;

use crate::cli::ExportArgs;
use crate::detect::detect;
use crate::error::{CliError, Reason, usage};
use crate::output::{
    ExportManifest, ExportedFile, SCHEMA_VERSION, Target, check_not_input, encode,
    format_from_extension, sanitize_file_name, write_atomic,
};
use crate::render::{
    CountLimit, FrameLimits, ItemRef, check_flags, fingerprint, input_files, item_ref,
    natural_width, output_stem, round3, select_items, source_info,
};
use crate::select::{Crop, format_file_time, parse_crop};
use crate::source::{Item, ItemKey, Source, Warnings, fit_size};
use crate::template::validate_template;

pub const DEFAULT_DPI: u32 = 150;
pub const DEFAULT_MAX_FILES: usize = 100;
pub const MAX_FILES: usize = 10_000;
/// Longest edge of an exported file in pixels.
pub const MAX_EDGE: u32 = 16_384;
const MAX_PIXELS: u64 = 100_000_000;
/// Pixels decoded and held in memory at once; larger selections are written in batches.
const BATCH_PIXELS: u64 = 64_000_000;
const BATCH_ITEMS: usize = 32;

pub fn run(args: ExportArgs) -> Result<()> {
    let (json, dry_run, quiet) = (args.json, args.dry_run, args.quiet);
    let manifest = execute(args)?;
    if !quiet {
        for warning in &manifest.warnings {
            eprintln!("warning: {warning}");
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&manifest)?);
        return Ok(());
    }
    if dry_run {
        println!("dry run: {} file(s)", manifest.files.len());
    }
    for file in &manifest.files {
        match (&file.path, dry_run) {
            (Some(path), true) => println!("{path} {}x{}", file.width, file.height),
            (Some(path), false) => println!("{path}"),
            (None, _) => {}
        }
    }
    Ok(())
}

/// Selects the items and (unless `--dry-run`) writes one file per item, without printing to
/// stdout; `-v` progress still goes to stderr.
pub fn execute(args: ExportArgs) -> Result<ExportManifest> {
    let started = Instant::now();
    let log = |message: String| {
        if args.verbose {
            eprintln!("[{:>6.2}s] {message}", started.elapsed().as_secs_f64());
        }
    };

    let kind = detect(&args.inputs, args.kind)?;
    check_flags(
        kind,
        &args.selection,
        &args.source,
        &[("--dpi", args.dpi.is_some())],
    )?;
    let file_output = args
        .output
        .as_deref()
        .is_some_and(|path| format_from_extension(path).is_some());
    if file_output && args.name.is_some() {
        return Err(usage(
            Reason::InvalidArgument,
            "--name cannot be used when -o names a file",
        )
        .with_hint("pass a directory to -o, or leave out --name")
        .into());
    }
    let template = args.name.as_deref().unwrap_or(kind.default_name());
    validate_template(template, "name", kind.name_vars(), kind)?;

    let source = Source::open(kind, &args.inputs, &args.source)?;
    log(format!("opened {} input", kind.name()));
    let crop = args.selection.crop.as_deref().map(parse_crop).transpose()?;
    let frames = FrameLimits {
        default_count: None,
        limit: CountLimit::Files(args.max_files),
    };
    let items = select_items(&source, &args.selection, crop, &frames)?;
    frames.limit.check(items.len())?;
    let dpi = args.dpi.unwrap_or(DEFAULT_DPI);
    let sizes = items
        .iter()
        .map(|item| export_size(&source, item, dpi, args.max_edge))
        .collect::<Result<Vec<_>, _>>()?;

    let format = args
        .format
        .or_else(|| args.output.as_deref().and_then(format_from_extension))
        .unwrap_or_else(|| kind.default_format());
    let stem = output_stem(&source, &args.inputs);
    let target = Target::resolve_export(
        args.output.as_deref(),
        &stem,
        &fingerprint(
            kind,
            &args.inputs,
            format!(
                "{:?}|{dpi}|{:?}|{:?}|{format:?}|{}",
                args.selection, args.max_edge, args.name, args.quality
            ),
        ),
    );
    let paths: Vec<PathBuf> = if file_output {
        (0..items.len())
            .map(|i| target.sheet_path(i, items.len(), format))
            .collect()
    } else {
        let names = file_names(template, &source, &items, &stem);
        check_unique(&names, template)?;
        names
            .iter()
            .map(|name| target.named_path(name, format))
            .collect()
    };
    let inputs = input_files(&source, &args.inputs);
    for path in &paths {
        check_not_input(path, &inputs)?;
    }
    log(format!("{} file(s)", items.len()));

    let warnings = Warnings::default();
    for warning in source.open_warnings() {
        warnings.push(warning);
    }
    let file_info = |index: usize, size: (u32, u32), path: Option<&PathBuf>| {
        let item = &items[index];
        let ItemRef {
            page,
            time,
            frame,
            file,
        } = item_ref(item);
        ExportedFile {
            index: item.vars.index,
            path: path.map(|p| p.display().to_string()),
            page,
            time,
            frame,
            file,
            width: size.0,
            height: size.1,
            scale: round3(size.0 as f64 / natural_width(&source, item)),
            crop: item.crop.map(|c| [c.x, c.y, c.width, c.height]),
            bytes: None,
            error: None,
        }
    };

    let mut files = Vec::with_capacity(items.len());
    if args.dry_run {
        for (index, (&size, path)) in sizes.iter().zip(&paths).enumerate() {
            files.push(file_info(index, size, Some(path)));
        }
    } else {
        target.create_dir()?;
        let mut written = Vec::new();
        for batch in batches(&sizes) {
            let rendered =
                source.render(&items[batch.clone()], &sizes[batch.clone()], &warnings)?;
            let outcomes: Vec<Outcome> = rendered
                .into_par_iter()
                .zip(&paths[batch.clone()])
                .map(|(cell, path)| {
                    if let Some(error) = cell.error {
                        return Ok(Outcome::Failed(error));
                    }
                    let data = encode(&cell.image, format, args.quality)?;
                    write_atomic(path, &data)?;
                    Ok(Outcome::Written {
                        bytes: data.len(),
                        size: cell.image.dimensions(),
                    })
                })
                .collect::<Result<_>>()?;
            for (index, outcome) in batch.clone().zip(outcomes) {
                let entry = match outcome {
                    Outcome::Written { bytes, size } => {
                        written.push(paths[index].clone());
                        ExportedFile {
                            bytes: Some(bytes),
                            ..file_info(index, size, Some(&paths[index]))
                        }
                    }
                    Outcome::Failed(error) => {
                        warnings.push(format!(
                            "item {} was not written: {error}",
                            items[index].vars.index
                        ));
                        ExportedFile {
                            error: Some(error),
                            ..file_info(index, sizes[index], None)
                        }
                    }
                };
                files.push(entry);
            }
            log(format!("wrote {}/{}", batch.end, items.len()));
        }
        target.remove_stale(&written);
    }
    log("done".to_string());

    Ok(ExportManifest {
        schema_version: SCHEMA_VERSION,
        source: source_info(&source, false),
        files,
        warnings: warnings.take(),
    })
}

enum Outcome {
    Written {
        bytes: usize,
        size: (u32, u32),
    },
    /// The item could not be decoded, so no file was written.
    Failed(String),
}

/// Pixel size of an exported item: PDF pages at `dpi`, raster sources at their native size,
/// both shrunk to `max_edge` when given.
fn export_size(
    source: &Source,
    item: &Item,
    dpi: u32,
    max_edge: Option<u32>,
) -> Result<(u32, u32), CliError> {
    let full = match (source, &item.key) {
        (Source::Pdf(pdf), ItemKey::Page(index)) => {
            pdf_pixels(pdf.page_sizes[*index], item.crop, dpi)
        }
        _ => item
            .native
            .unwrap_or_else(|| unreachable!("raster items have a native size")),
    };
    let size = match max_edge {
        Some(edge) => fit_size(item.aspect, (edge, edge), Some(full)),
        None => full,
    };
    let (w, h) = size;
    if w.max(h) > MAX_EDGE || u64::from(w) * u64::from(h) > MAX_PIXELS {
        let hint = match source {
            Source::Pdf(_) => "lower --dpi, pass --max-edge or narrow --crop",
            _ => "pass --max-edge or narrow --crop",
        };
        return Err(usage(
            Reason::DoesNotFit,
            format!(
                "item {} would be {w}x{h} px; files are limited to {MAX_EDGE} px per edge and {} megapixels",
                item.vars.index,
                MAX_PIXELS / 1_000_000
            ),
        )
        .with_hint(hint));
    }
    Ok(size)
}

/// Size of a page region of `page` points rendered at `dpi`.
fn pdf_pixels((width, height): (f32, f32), crop: Option<Crop>, dpi: u32) -> (u32, u32) {
    let (cw, ch) = crop.map_or((1.0, 1.0), |c| (c.width, c.height));
    let scale = f64::from(dpi) / 72.0;
    let px = |points: f32, fraction: f64| {
        (f64::from(points) * fraction * scale)
            .round()
            .clamp(1.0, f64::from(u32::MAX)) as u32
    };
    (px(width, cw), px(height, ch))
}

/// Expands `template` for every item. Numbers are zero-padded to the widest value so the
/// files sort in order, and times keep their tenths when any of them has one.
fn file_names(template: &str, source: &Source, items: &[Item], stem: &str) -> Vec<String> {
    let digits = |n: usize| n.to_string().len();
    let index_width = digits(items.len());
    let (page_width, frame_width, long) = match source {
        Source::Pdf(pdf) => (digits(pdf.page_count()), 0, false),
        Source::Video(video) => (0, 0, video.duration.is_some_and(|d| d >= 3600.0)),
        Source::Anim(anim) => (0, digits(anim.starts.len()), anim.duration >= 3600.0),
        Source::Images(_) => (0, 0, false),
    };
    let fine = items
        .iter()
        .filter_map(|item| item_ref(item).time)
        .any(|t| !((t * 10.0 + 1e-6).floor() as u64).is_multiple_of(10));
    items
        .iter()
        .map(|item| {
            let item_ref = item_ref(item);
            let name = match &item.key {
                ItemKey::File(path) => path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                _ => String::new(),
            };
            let expanded = template
                .replace("{stem}", stem)
                .replace("{index}", &format!("{:0index_width$}", item.vars.index))
                .replace("{total}", &items.len().to_string())
                .replace(
                    "{page}",
                    &item_ref
                        .page
                        .map(|p| format!("{p:0page_width$}"))
                        .unwrap_or_default(),
                )
                .replace(
                    "{time}",
                    &item_ref
                        .time
                        .map(|t| format_file_time(t, long, fine))
                        .unwrap_or_default(),
                )
                .replace(
                    "{frame}",
                    &item_ref
                        .frame
                        .map(|f| format!("{f:0frame_width$}"))
                        .unwrap_or_default(),
                )
                .replace("{name}", &name);
            sanitize_file_name(&expanded)
        })
        .collect()
}

/// Two items never share a file name; case is ignored since macOS and Windows ignore it.
fn check_unique(names: &[String], template: &str) -> Result<(), CliError> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (i, name) in names.iter().enumerate() {
        if let Some(first) = seen.insert(name.to_lowercase(), i) {
            return Err(usage(
                Reason::InvalidValue,
                format!(
                    "--name '{template}' gives items {} and {} the same file name '{name}'",
                    first + 1,
                    i + 1
                ),
            )
            .with_hint("add {index} to --name"));
        }
    }
    Ok(())
}

/// Consecutive runs of items that together stay within the memory budget.
fn batches(sizes: &[(u32, u32)]) -> Vec<Range<usize>> {
    let mut batches = Vec::new();
    let (mut start, mut pixels) = (0, 0u64);
    for (i, &(w, h)) in sizes.iter().enumerate() {
        let item_pixels = u64::from(w) * u64::from(h);
        if i > start && (pixels + item_pixels > BATCH_PIXELS || i - start >= BATCH_ITEMS) {
            batches.push(start..i);
            (start, pixels) = (i, 0);
        }
        pixels += item_pixels;
    }
    if start < sizes.len() {
        batches.push(start..sizes.len());
    }
    batches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_regions_scale_with_dpi() {
        assert_eq!(pdf_pixels((612.0, 792.0), None, 72), (612, 792));
        assert_eq!(pdf_pixels((612.0, 792.0), None, 150), (1275, 1650));
        let crop = Crop {
            x: 0.5,
            y: 0.0,
            width: 0.5,
            height: 0.25,
        };
        assert_eq!(pdf_pixels((612.0, 792.0), Some(crop), 144), (612, 396));
        assert_eq!(pdf_pixels((1.0, 1.0), Some(crop), 36), (1, 1));
    }

    #[test]
    fn batches_cover_every_item_once() {
        let small = vec![(100, 100); 70];
        let ranges = batches(&small);
        assert_eq!(ranges, vec![0..32, 32..64, 64..70]);

        let large = vec![(6_000, 5_000), (6_000, 5_000), (6_000, 5_000), (10, 10)];
        assert_eq!(batches(&large), vec![0..2, 2..4]);
        let huge = vec![(10_000, 10_000), (10, 10)];
        assert_eq!(batches(&huge), vec![0..1, 1..2]);
        assert!(batches(&[]).is_empty());
    }

    #[test]
    fn duplicate_names_are_rejected_ignoring_case() {
        let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(check_unique(&names(&["a-p1", "a-p2"]), "{stem}").is_ok());
        let error = check_unique(&names(&["shot", "x", "Shot"]), "{name}").unwrap_err();
        assert!(error.message.contains("items 1 and 3"), "{}", error.message);
    }
}
