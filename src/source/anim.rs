use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::imageops::{self, FilterType};
use image::{AnimationDecoder, Frames, RgbaImage};

use super::{Item, ItemKey, LabelVars, fit_size};
use crate::error::input;

/// Browsers show frames with a delay of 10 ms or less for 100 ms; do the same so timestamps match.
const MIN_DELAY_MS: f64 = 20.0;
const DEFAULT_DELAY_MS: f64 = 100.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnimFormat {
    Gif,
    Apng,
}

pub struct AnimSource {
    pub path: PathBuf,
    format: AnimFormat,
    pub size: (u32, u32),
    /// Start time of each frame in seconds.
    pub starts: Vec<f64>,
    pub duration: f64,
}

impl AnimSource {
    pub fn open(path: &Path) -> Result<AnimSource> {
        let format = sniff_format(path)?;
        let mut starts = Vec::new();
        let mut elapsed = 0.0;
        let mut size = (0, 0);
        // First pass reads only the timing so memory stays bounded for long animations.
        for frame in frames(path, format)? {
            let frame =
                frame.map_err(|e| input(format!("cannot decode {}: {e}", path.display())))?;
            size = frame.buffer().dimensions();
            starts.push(elapsed);
            let (num, den) = frame.delay().numer_denom_ms();
            let mut delay = num as f64 / den.max(1) as f64;
            if delay < MIN_DELAY_MS {
                delay = DEFAULT_DELAY_MS;
            }
            elapsed += delay / 1000.0;
        }
        if starts.is_empty() {
            return Err(input(format!("{} contains no frames", path.display())).into());
        }
        Ok(AnimSource {
            path: path.to_path_buf(),
            format,
            size,
            starts,
            duration: elapsed,
        })
    }

    pub fn aspect(&self) -> f64 {
        self.size.0 as f64 / self.size.1 as f64
    }

    /// Index of the frame visible at time `t`.
    pub fn frame_at(&self, t: f64) -> usize {
        self.starts
            .partition_point(|&start| start <= t)
            .saturating_sub(1)
    }

    /// Builds items for already de-duplicated frame indices.
    pub fn items(&self, frames: &[usize], labels: &[String]) -> Vec<Item> {
        frames
            .iter()
            .zip(labels)
            .enumerate()
            .map(|(i, (&frame, label))| Item {
                aspect: self.aspect(),
                native: Some(self.size),
                key: ItemKey::Frame(frame, self.starts[frame]),
                vars: LabelVars {
                    index: i + 1,
                    total: frames.len(),
                    time: Some(label.clone()),
                    frame: Some(frame + 1),
                    ..LabelVars::default()
                },
            })
            .collect()
    }

    pub fn render(&self, items: &[Item], boxes: &[(u32, u32)]) -> Result<Vec<RgbaImage>> {
        let wanted: Vec<usize> = items
            .iter()
            .map(|item| match item.key {
                ItemKey::Frame(index, _) => index,
                _ => unreachable!("animation items are frames"),
            })
            .collect();
        let last = wanted.iter().copied().max().unwrap_or(0);
        let mut images: Vec<Option<RgbaImage>> = vec![None; items.len()];

        for (index, frame) in frames(&self.path, self.format)?.enumerate().take(last + 1) {
            let frame = frame.with_context(|| {
                format!(
                    "cannot decode frame {} of {}",
                    index + 1,
                    self.path.display()
                )
            })?;
            for (slot, _) in wanted.iter().enumerate().filter(|(_, w)| **w == index) {
                let (w, h) = fit_size(items[slot].aspect, boxes[slot], items[slot].native);
                images[slot] = Some(imageops::resize(
                    frame.buffer(),
                    w,
                    h,
                    FilterType::CatmullRom,
                ));
            }
        }
        images
            .into_iter()
            .map(|image| image.context("animation ended before a requested frame"))
            .collect()
    }
}

fn sniff_format(path: &Path) -> Result<AnimFormat> {
    let mut magic = [0u8; 4];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .with_context(|| format!("failed to read {}", path.display()))?;
    match &magic {
        b"GIF8" => Ok(AnimFormat::Gif),
        [0x89, b'P', b'N', b'G'] => Ok(AnimFormat::Apng),
        _ => Err(input(format!("{} is not a GIF or APNG file", path.display())).into()),
    }
}

fn frames(path: &Path, format: AnimFormat) -> Result<Frames<'static>> {
    let reader = BufReader::new(
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?,
    );
    let invalid = |e: image::ImageError| input(format!("cannot decode {}: {e}", path.display()));
    Ok(match format {
        AnimFormat::Gif => GifDecoder::new(reader).map_err(invalid)?.into_frames(),
        AnimFormat::Apng => PngDecoder::new(reader)
            .map_err(invalid)?
            .apng()
            .map_err(invalid)?
            .into_frames(),
    })
}
