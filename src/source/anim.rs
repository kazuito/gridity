use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::imageops::{self, FilterType};
use image::{AnimationDecoder, Frames, ImageDecoder, RgbaImage};

use super::{Item, ItemKey, LabelVars, Rendered, decode_error, decode_limits, fit_size};
use crate::error::{Reason, input};

/// Browsers show GIF and APNG frames with a delay of 10 ms or less for 100 ms; do the same so
/// timestamps match.
const MAX_IGNORED_DELAY_MS: f64 = 10.0;
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
    /// Set when decoding stopped early; only the frames before the damage are used.
    pub damaged: Option<String>,
}

impl AnimSource {
    pub fn open(path: &Path) -> Result<AnimSource> {
        let format = sniff_format(path)?;
        let mut starts = Vec::new();
        let mut elapsed = 0.0;
        let mut size = (0, 0);
        let mut damaged = None;
        // First pass reads only the timing so memory stays bounded for long animations.
        for frame in frames(path, format)? {
            let frame = match frame {
                Ok(frame) => frame,
                Err(e) if !starts.is_empty() => {
                    damaged = Some(format!(
                        "only the first {} frames could be decoded: {:#}",
                        starts.len(),
                        anyhow::Error::from(decode_error(path, e))
                    ));
                    break;
                }
                Err(e) => return Err(decode_error(path, e).into()),
            };
            size = frame.buffer().dimensions();
            starts.push(elapsed);
            let (num, den) = frame.delay().numer_denom_ms();
            elapsed += frame_delay_ms(num, den) / 1000.0;
        }
        if starts.is_empty() {
            return Err(input(
                Reason::NoFrames,
                format!("{} contains no frames", path.display()),
            )
            .into());
        }
        Ok(AnimSource {
            path: path.to_path_buf(),
            format,
            size,
            starts,
            duration: elapsed,
            damaged,
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
                crop: None,
            })
            .collect()
    }

    pub fn render(&self, items: &[Item], boxes: &[(u32, u32)]) -> Result<Vec<Rendered>> {
        let wanted: Vec<usize> = items
            .iter()
            .map(|item| match item.key {
                ItemKey::Frame(index, _) => index,
                _ => unreachable!("animation items are frames"),
            })
            .collect();
        let last = wanted.iter().copied().max().unwrap_or(0);
        let mut images: Vec<Option<RgbaImage>> = vec![None; items.len()];

        let mut failure = None;
        for (index, frame) in frames(&self.path, self.format)?.enumerate().take(last + 1) {
            let frame = match frame {
                Ok(frame) => frame,
                Err(e) => {
                    failure = Some(format!(
                        "{:#}",
                        anyhow::Error::from(decode_error(&self.path, e))
                    ));
                    break;
                }
            };
            for (slot, _) in wanted.iter().enumerate().filter(|(_, w)| **w == index) {
                let (w, h) = fit_size(items[slot].aspect, boxes[slot], items[slot].native);
                let buffer = frame.buffer();
                let (x, y, cw, ch) = match items[slot].crop {
                    Some(crop) => crop.pixels(buffer.dimensions()),
                    None => (0, 0, buffer.width(), buffer.height()),
                };
                let region = imageops::crop_imm(buffer, x, y, cw, ch).to_image();
                images[slot] = Some(imageops::resize(&region, w, h, FilterType::CatmullRom));
            }
        }
        Ok(images
            .into_iter()
            .enumerate()
            .map(|(slot, image)| match image {
                Some(image) => Rendered::ok(image),
                None => Rendered::failed(
                    fit_size(items[slot].aspect, boxes[slot], items[slot].native),
                    failure
                        .clone()
                        .unwrap_or_else(|| "the animation ended before this frame".to_string()),
                ),
            })
            .collect())
    }
}

fn frame_delay_ms(num: u32, den: u32) -> f64 {
    let delay = num as f64 / den.max(1) as f64;
    if delay <= MAX_IGNORED_DELAY_MS {
        DEFAULT_DELAY_MS
    } else {
        delay
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
        _ => Err(input(
            Reason::WrongType,
            format!("{} is not a GIF or APNG file", path.display()),
        )
        .into()),
    }
}

fn frames(path: &Path, format: AnimFormat) -> Result<Frames<'static>> {
    let reader = BufReader::new(
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?,
    );
    let invalid = |e| decode_error(path, e);
    Ok(match format {
        AnimFormat::Gif => {
            let mut decoder = GifDecoder::new(reader).map_err(invalid)?;
            decoder.set_limits(decode_limits()).map_err(invalid)?;
            decoder.into_frames()
        }
        AnimFormat::Apng => PngDecoder::with_limits(reader, decode_limits())
            .map_err(invalid)?
            .apng()
            .map_err(invalid)?
            .into_frames(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_delays_follow_browsers() {
        assert!((frame_delay_ms(1000, 60) - 16.667).abs() < 0.001);
        assert_eq!(frame_delay_ms(20, 1), 20.0);
        assert_eq!(frame_delay_ms(10, 1), 100.0);
        assert_eq!(frame_delay_ms(0, 1), 100.0);
        assert_eq!(frame_delay_ms(1000, 0), 1000.0);
    }
}
