use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use hayro::hayro_interpret::{InterpreterSettings, InterpreterWarning, TransformExt};
use hayro::hayro_syntax::{DecryptionError, LoadPdfError, Pdf};
use hayro::kurbo::Affine;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::peniko::ImageAlphaType;
use hayro::vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit};
use hayro::{RenderCache, RenderSettings};
use image::RgbaImage;
use rayon::prelude::*;

use super::{Item, ItemKey, LabelVars, Warnings, fit_size};
use crate::error::{CliError, input};
use crate::select::Crop;

pub struct PdfSource {
    pub path: PathBuf,
    data: Arc<Vec<u8>>,
    password: String,
    /// Rendered size of each page in PDF points, rotation applied.
    pub page_sizes: Vec<(f32, f32)>,
    pub title: Option<String>,
}

impl PdfSource {
    pub fn open(path: &Path, password: Option<&str>) -> Result<PdfSource> {
        let data = Arc::new(
            std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?,
        );
        let password = password.unwrap_or_default().to_string();
        let pdf = load(&data, &password)?;
        let page_sizes: Vec<(f32, f32)> = pdf
            .pages()
            .iter()
            .map(|page| page.render_dimensions())
            .collect();
        if page_sizes.is_empty() {
            return Err(input(format!("{} has no pages", path.display())).into());
        }
        let title = pdf
            .metadata()
            .title
            .as_deref()
            .map(decode_pdf_string)
            .filter(|t| !t.trim().is_empty());
        Ok(PdfSource {
            path: path.to_path_buf(),
            data,
            password,
            page_sizes,
            title,
        })
    }

    pub fn page_count(&self) -> usize {
        self.page_sizes.len()
    }

    /// Builds items for 1-based page numbers.
    pub fn items(&self, pages: &[usize]) -> Vec<Item> {
        pages
            .iter()
            .enumerate()
            .map(|(i, &page)| {
                let (w, h) = self.page_sizes[page - 1];
                Item {
                    aspect: (w / h) as f64,
                    native: None,
                    key: ItemKey::Page(page - 1),
                    vars: LabelVars {
                        index: i + 1,
                        total: self.page_count(),
                        page: Some(page),
                        ..LabelVars::default()
                    },
                    crop: None,
                }
            })
            .collect()
    }

    pub fn render(
        &self,
        items: &[Item],
        boxes: &[(u32, u32)],
        warnings: &Warnings,
    ) -> Result<Vec<RgbaImage>> {
        items
            .par_iter()
            .zip(boxes)
            .map(|(item, &bounds)| {
                let ItemKey::Page(index) = item.key else {
                    unreachable!("PDF items are pages")
                };
                let size = fit_size(item.aspect, bounds, None);
                let (image, page_warnings) =
                    render_page(&self.data, &self.password, index, size, item.crop)?;
                for warning in page_warnings {
                    warnings.push(format!("page {}: {}", index + 1, describe_warning(warning)));
                }
                Ok(image)
            })
            .collect()
    }
}

fn load(data: &Arc<Vec<u8>>, password: &str) -> Result<Pdf, CliError> {
    Pdf::new_with_password(data.clone(), password).map_err(|e| match e {
        LoadPdfError::Decryption(DecryptionError::PasswordProtected) if password.is_empty() => {
            input("the PDF is password-protected").with_hint("pass --password <PASSWORD>")
        }
        LoadPdfError::Decryption(DecryptionError::PasswordProtected) => {
            input("the PDF password is incorrect")
        }
        LoadPdfError::Decryption(other) => input(format!("cannot decrypt the PDF ({other:?})")),
        LoadPdfError::Invalid => input("the file is not a valid PDF or is damaged"),
    })
}

/// Renders one page, or its `crop` region, so it is exactly `size` pixels. Only the region is
/// rasterized, so small crops stay cheap. hayro's borrowed types stay inside this function.
fn render_page(
    data: &Arc<Vec<u8>>,
    password: &str,
    index: usize,
    size: (u32, u32),
    crop: Option<Crop>,
) -> Result<(RgbaImage, Vec<InterpreterWarning>)> {
    let pdf = load(data, password)?;
    let page = &pdf.pages()[index];
    let (page_w, page_h) = page.render_dimensions();
    let crop = crop.unwrap_or(Crop {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    });
    let scale_x = size.0 as f64 / (page_w as f64 * crop.width);
    let scale_y = size.1 as f64 / (page_h as f64 * crop.height);
    let transform = Affine::translate((
        -crop.x * page_w as f64 * scale_x,
        -crop.y * page_h as f64 * scale_y,
    )) * Affine::scale_non_uniform(scale_x, scale_y)
        * page.initial_transform(true).to_kurbo();

    let collected = Arc::new(Mutex::new(Vec::new()));
    let sink = collected.clone();
    let settings = InterpreterSettings {
        warning_sink: Arc::new(move |warning| sink.lock().unwrap().push(warning)),
        ..InterpreterSettings::default()
    };
    let (width, height) = (
        u16::try_from(size.0).context("page image is too wide")?,
        u16::try_from(size.1).context("page image is too tall")?,
    );
    let mut ctx = RenderContext::new(width, height);
    hayro::render_into(
        page,
        &RenderCache::new(),
        &settings,
        &RenderSettings::default(),
        &mut ctx,
        transform,
    );
    ctx.flush();
    let mut pixmap = Pixmap::new(width, height);
    ctx.render_with(
        &mut pixmap,
        &mut Resources::default(),
        RasterizerSettings {
            target_init: TargetInit::Clear(WHITE),
            ..RasterizerSettings::default()
        },
    );
    let pixels = pixmap.take_rgba8(ImageAlphaType::Alpha);
    let image = RgbaImage::from_raw(size.0, size.1, pixels)
        .context("unexpected pixmap size from PDF renderer")?;
    let warnings = std::mem::take(&mut *collected.lock().unwrap());
    Ok((image, warnings))
}

fn describe_warning(warning: InterpreterWarning) -> String {
    match warning {
        InterpreterWarning::UnsupportedFont => {
            "unsupported font, some text may be missing".to_string()
        }
        InterpreterWarning::ImageDecodeFailure => "an embedded image failed to decode".to_string(),
        other => format!("{other:?}"),
    }
}

/// PDF text strings are UTF-16BE with a BOM or PDFDocEncoding (treated as Latin-1).
fn decode_pdf_string(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_be_bytes(*pair))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    bytes.iter().map(|&b| b as char).collect()
}
