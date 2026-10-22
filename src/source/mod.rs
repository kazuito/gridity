pub mod anim;
pub mod images;
pub mod pdf;
pub mod video;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use image::{Rgba, RgbaImage};

use crate::cli::{InputKind, SourceArgs};
use crate::error::{CliError, Reason, input};
use crate::select::Crop;

const MAX_DECODE_BYTES: u64 = 512 * 1024 * 1024;

/// One thing to show in a cell: a PDF page, a video timestamp, an animation frame or an image file.
#[derive(Debug, Clone)]
pub struct Item {
    /// Display width / height.
    pub aspect: f64,
    /// Native display size; cells never upscale raster sources beyond it.
    pub native: Option<(u32, u32)>,
    pub key: ItemKey,
    pub vars: LabelVars,
    /// Shown part of the page or frame; aspect and native already describe the cropped part.
    pub crop: Option<Crop>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemKey {
    /// 0-based page index.
    Page(usize),
    Time(f64),
    /// 0-based frame index and its start time.
    Frame(usize, f64),
    File(PathBuf),
}

#[derive(Debug, Clone, Default)]
pub struct LabelVars {
    pub index: usize,
    pub total: usize,
    pub page: Option<usize>,
    pub time: Option<String>,
    pub frame: Option<usize>,
    pub name: Option<String>,
}

pub enum Source {
    Pdf(pdf::PdfSource),
    Video(video::VideoSource),
    Anim(anim::AnimSource),
    Images(images::ImagesSource),
}

impl Source {
    pub fn open(kind: InputKind, inputs: &[PathBuf], args: &SourceArgs) -> Result<Source> {
        Ok(match kind {
            InputKind::Pdf => Source::Pdf(pdf::PdfSource::open(
                single(inputs),
                pdf::resolve_password(&args.password)?.as_deref(),
            )?),
            InputKind::Video => Source::Video(video::VideoSource::open(
                single(inputs),
                args.ffmpeg.as_deref(),
            )?),
            InputKind::Anim => Source::Anim(anim::AnimSource::open(single(inputs))?),
            InputKind::Images => Source::Images(images::ImagesSource::open(inputs)?),
        })
    }

    pub fn kind(&self) -> InputKind {
        match self {
            Source::Pdf(_) => InputKind::Pdf,
            Source::Video(_) => InputKind::Video,
            Source::Anim(_) => InputKind::Anim,
            Source::Images(_) => InputKind::Images,
        }
    }

    /// Renders `items` so each fits inside the matching `(width, height)` box. An item that
    /// cannot be decoded becomes a placeholder of the planned size instead of failing the sheet.
    pub fn render(
        &self,
        items: &[Item],
        boxes: &[(u32, u32)],
        warnings: &Warnings,
    ) -> Result<Vec<Rendered>> {
        match self {
            Source::Pdf(s) => s.render(items, boxes, warnings),
            Source::Video(s) => s.render(items, boxes),
            Source::Anim(s) => s.render(items, boxes),
            Source::Images(s) => Ok(s.render(items, boxes)),
        }
    }

    /// Problems found while opening that left some inputs or frames out.
    pub fn open_warnings(&self) -> Vec<String> {
        match self {
            Source::Images(s) => s.skipped.clone(),
            Source::Anim(s) => s.damaged.clone().into_iter().collect(),
            Source::Pdf(_) | Source::Video(_) => Vec::new(),
        }
    }
}

/// A rendered cell image; `error` is set when the item could not be decoded and `image` is a
/// placeholder.
pub struct Rendered {
    pub image: RgbaImage,
    pub error: Option<String>,
}

impl Rendered {
    pub fn ok(image: RgbaImage) -> Rendered {
        Rendered { image, error: None }
    }

    pub fn failed(size: (u32, u32), error: impl Into<String>) -> Rendered {
        Rendered {
            image: RgbaImage::from_pixel(size.0, size.1, Rgba([128, 128, 128, 255])),
            error: Some(error.into()),
        }
    }
}

fn single(inputs: &[PathBuf]) -> &Path {
    &inputs[0]
}

/// Largest size with the item's aspect ratio inside `bounds`, never larger than `native`.
pub fn fit_size(aspect: f64, bounds: (u32, u32), native: Option<(u32, u32)>) -> (u32, u32) {
    let (mut max_w, mut max_h) = bounds;
    if let Some((w, h)) = native {
        max_w = max_w.min(w);
        max_h = max_h.min(h);
    }
    let (w, h) = if max_w as f64 / max_h as f64 > aspect {
        ((max_h as f64 * aspect).round() as u32, max_h)
    } else {
        (max_w, (max_w as f64 / aspect).round() as u32)
    };
    (w.clamp(1, max_w.max(1)), h.clamp(1, max_h.max(1)))
}

/// Caps what decoding an image or animation may allocate, so a tiny file cannot claim gigabytes.
pub fn decode_limits() -> image::Limits {
    let mut limits = image::Limits::no_limits();
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    limits
}

pub fn decode_error(path: &Path, error: image::ImageError) -> CliError {
    match error {
        image::ImageError::Limits(_) => input(
            Reason::TooLargeToDecode,
            format!("{} is too large to decode ({error})", path.display()),
        )
        .with_hint(format!(
            "resize or re-encode it: decoding may use at most {} MiB",
            MAX_DECODE_BYTES / 1024 / 1024
        )),
        _ => input(
            Reason::DecodeFailed,
            format!("cannot decode {}: {error}", path.display()),
        ),
    }
}

/// Warnings collected from parallel workers and reported once at the end.
#[derive(Default)]
pub struct Warnings(Mutex<Vec<String>>);

impl Warnings {
    pub fn push(&self, message: impl Into<String>) {
        let message = message.into();
        let mut list = self.0.lock().unwrap();
        if !list.contains(&message) {
            list.push(message);
        }
    }

    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_size_keeps_aspect() {
        assert_eq!(fit_size(2.0, (400, 400), None), (400, 200));
        assert_eq!(fit_size(0.5, (400, 400), None), (200, 400));
        assert_eq!(fit_size(16.0 / 9.0, (400, 400), Some((160, 90))), (160, 90));
    }
}
