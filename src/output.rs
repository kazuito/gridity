use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageFormat, RgbaImage};
use serde::Serialize;

use crate::cli::Format;
use crate::layout::Grid;

/// Anthropic's API rejects images larger than this.
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// Where sheets are written.
pub struct Target {
    dir: PathBuf,
    /// File name without extension; sheet numbers are appended when there are several.
    base: String,
    /// The directory belongs to gridity, so leftovers from earlier runs may be deleted.
    managed: bool,
}

impl Target {
    /// `-o` with an image extension is a file; any other `-o` is a directory.
    /// Without `-o`, sheets go to `$TMPDIR/gridity/<stem>-<hash>/`.
    pub fn resolve(output: Option<&Path>, stem: &str, fingerprint: &str) -> Result<Target> {
        let stem = sanitize(stem);
        let target = match output {
            Some(path) if format_from_extension(path).is_some() => Target {
                dir: path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new("."))
                    .to_path_buf(),
                base: path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or(stem),
                managed: false,
            },
            Some(dir) => Target {
                dir: dir.to_path_buf(),
                base: format!("{stem}.grid"),
                managed: false,
            },
            None => Target {
                dir: std::env::temp_dir()
                    .join("gridity")
                    .join(format!("{stem}-{:08x}", fnv1a(fingerprint) as u32)),
                base: format!("{stem}.grid"),
                managed: true,
            },
        };
        Ok(target)
    }

    pub fn sheet_path(&self, index: usize, total: usize, format: Format) -> PathBuf {
        let ext = extension(format);
        let name = if total > 1 {
            let width = total.to_string().len().max(2);
            format!("{}-{:0width$}.{ext}", self.base, index + 1)
        } else {
            format!("{}.{ext}", self.base)
        };
        absolute(&self.dir.join(name))
    }

    pub fn create_dir(&self) -> Result<()> {
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("failed to create {}", self.dir.display()))
    }

    /// Removes sheets from earlier runs that this run did not overwrite.
    pub fn remove_stale(&self, written: &[PathBuf]) {
        if !self.managed {
            return;
        }
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let ours = name.starts_with(&self.base) && format_from_extension(&path).is_some();
            if ours && !written.contains(&path) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

pub fn format_from_extension(path: &Path) -> Option<Format> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some(Format::Png),
        "jpg" | "jpeg" => Some(Format::Jpeg),
        _ => None,
    }
}

pub fn extension(format: Format) -> &'static str {
    match format {
        Format::Png => "png",
        Format::Jpeg => "jpg",
    }
}

pub fn encode(image: &RgbaImage, format: Format, quality: u8) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    match format {
        Format::Png => image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .context("failed to encode PNG")?,
        Format::Jpeg => {
            let rgb = DynamicImage::ImageRgba8(image.clone()).to_rgb8();
            JpegEncoder::new_with_quality(&mut bytes, quality)
                .encode_image(&rgb)
                .context("failed to encode JPEG")?;
        }
    }
    Ok(bytes)
}

fn sanitize(stem: &str) -> String {
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "gridity".to_string()
    } else {
        cleaned
    }
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Stable 64-bit FNV-1a hash, used to name output directories.
pub fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ byte as u64).wrapping_mul(0x100000001b3)
    })
}

#[derive(Debug, Serialize)]
pub struct Manifest {
    pub source: serde_json::Value,
    pub limits: LimitsInfo,
    pub sheets: Vec<SheetInfo>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct LimitsInfo {
    pub preset: String,
    pub max_edge: u32,
    pub max_tokens: u32,
}

#[derive(Debug, Serialize)]
pub struct SheetInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub width: u32,
    pub height: u32,
    /// Visual tokens under Claude's 28x28 px patch rule; other models count differently.
    pub est_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<usize>,
    pub grid: Grid,
    pub cells: Vec<CellInfo>,
}

#[derive(Debug, Serialize)]
pub struct CellInfo {
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Placed image on the sheet: [x, y, width, height].
    pub rect: [u32; 4],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_target_is_deterministic() {
        let a = Target::resolve(None, "my report", "x").unwrap();
        let b = Target::resolve(None, "my report", "x").unwrap();
        let c = Target::resolve(None, "my report", "y").unwrap();
        assert_eq!(
            a.sheet_path(0, 1, Format::Png),
            b.sheet_path(0, 1, Format::Png)
        );
        assert_ne!(
            a.sheet_path(0, 1, Format::Png),
            c.sheet_path(0, 1, Format::Png)
        );
        assert!(
            a.sheet_path(0, 1, Format::Png)
                .ends_with("my_report.grid.png")
        );
        assert!(a.sheet_path(0, 1, Format::Png).is_absolute());
    }

    #[test]
    fn sheet_numbering() {
        let t = Target::resolve(Some(Path::new("out/sheet.png")), "doc", "").unwrap();
        assert!(t.sheet_path(0, 1, Format::Png).ends_with("out/sheet.png"));
        assert!(
            t.sheet_path(2, 3, Format::Png)
                .ends_with("out/sheet-03.png")
        );
        assert!(
            t.sheet_path(4, 120, Format::Jpeg)
                .ends_with("out/sheet-005.jpg")
        );
        let d = Target::resolve(Some(Path::new("out")), "doc", "").unwrap();
        assert!(
            d.sheet_path(0, 2, Format::Png)
                .ends_with("out/doc.grid-01.png")
        );
    }
}
