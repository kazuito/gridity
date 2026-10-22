use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use hayro::hayro_interpret::font::{Glyph as FontGlyph, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context as PageContext, Device, DrawMode, DrawProps, Image,
    ImageDrawProps, InterpreterCache, InterpreterSettings, InterpreterWarning, RectExt, SoftMask,
    TransformExt, interpret_page,
};
use hayro::hayro_syntax::{DecryptionError, LoadPdfError, Pdf};
use hayro::kurbo::{Affine, BezPath, Point, Rect, Shape};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::peniko::ImageAlphaType;
use hayro::vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit};
use hayro::{RenderCache, RenderSettings};
use image::RgbaImage;
use rayon::prelude::*;

use super::{Item, ItemKey, LabelVars, Rendered, Warnings, fit_size};
use crate::cli::PasswordArgs;
use crate::error::{CliError, input, usage};
use crate::pagetext::{BBox, Glyph, PageScan};
use crate::select::Crop;

pub struct PdfSource {
    pub path: PathBuf,
    /// Parsed once and shared by all workers; parsing per page made large documents quadratic.
    pdf: Pdf,
    /// Rendered size of each page in PDF points, rotation applied.
    pub page_sizes: Vec<(f32, f32)>,
    pub title: Option<String>,
}

impl PdfSource {
    pub fn open(path: &Path, password: Option<&str>) -> Result<PdfSource> {
        let data = Arc::new(
            std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?,
        );
        let pdf = load(&data, password.unwrap_or_default())?;
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
            pdf,
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
    ) -> Result<Vec<Rendered>> {
        items
            .par_iter()
            .zip(boxes)
            .map_init(RenderCache::new, |cache, (item, &bounds)| {
                let ItemKey::Page(index) = item.key else {
                    unreachable!("PDF items are pages")
                };
                let size = fit_size(item.aspect, bounds, None);
                let (image, page_warnings) = render_page(&self.pdf, cache, index, size, item.crop)?;
                for warning in page_warnings {
                    warnings.push(format!("page {}: {}", index + 1, describe_warning(warning)));
                }
                Ok(Rendered::ok(image))
            })
            .collect()
    }

    /// Measures what is visible on 1-based `pages`, in parallel.
    pub fn scan(&self, pages: &[usize]) -> Result<Vec<PageScan>> {
        pages
            .par_iter()
            .map_init(InterpreterCache::new, |cache, &page| {
                scan_page(&self.pdf, cache, page - 1)
            })
            .collect()
    }
}

/// Environment variable read when neither `--password` nor `--password-file` is given.
pub const PASSWORD_ENV: &str = "GRIDITY_PDF_PASSWORD";
const MAX_PASSWORD_FILE_BYTES: u64 = 4096;

/// The password from `--password`, `--password-file` or `GRIDITY_PDF_PASSWORD`, in that order.
pub fn resolve_password(args: &PasswordArgs) -> Result<Option<String>> {
    if let Some(password) = &args.password {
        return Ok(Some(password.clone()));
    }
    if let Some(path) = &args.password_file {
        return read_password_file(path).map(Some);
    }
    match std::env::var(PASSWORD_ENV) {
        Ok(password) if !password.is_empty() => Ok(Some(password)),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(usage(format!("{PASSWORD_ENV} is not valid UTF-8")).into())
        }
    }
}

/// Reads a password file (or stdin for "-"), dropping one trailing line break.
fn read_password_file(path: &Path) -> Result<String> {
    let mut bytes = Vec::new();
    let read = if path == Path::new("-") {
        std::io::stdin()
            .take(MAX_PASSWORD_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
    } else {
        std::fs::File::open(path)
            .and_then(|f| f.take(MAX_PASSWORD_FILE_BYTES + 1).read_to_end(&mut bytes))
    };
    read.map_err(|e| {
        input(format!(
            "cannot read the password file {}: {e}",
            path.display()
        ))
    })?;
    if bytes.len() as u64 > MAX_PASSWORD_FILE_BYTES {
        return Err(usage(format!(
            "the password file {} is larger than {MAX_PASSWORD_FILE_BYTES} bytes",
            path.display()
        ))
        .into());
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| usage(format!("the password file {} is not UTF-8", path.display())))?;
    let password = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(&text);
    Ok(password.to_string())
}

// Workers share one `Pdf`; this fails to compile if hayro ever makes that unsound.
const _: fn() = || {
    fn shared_between_threads<T: Send + Sync>() {}
    shared_between_threads::<Pdf>();
};

fn load(data: &Arc<Vec<u8>>, password: &str) -> Result<Pdf, CliError> {
    Pdf::new_with_password(data.clone(), password).map_err(|e| match e {
        LoadPdfError::Decryption(DecryptionError::PasswordProtected) if password.is_empty() => {
            input("the PDF is password-protected").with_hint(format!(
                "pass --password-file <FILE> or set {PASSWORD_ENV}; avoid --password, which other processes can see"
            ))
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
fn render_page<'a>(
    pdf: &'a Pdf,
    cache: &RenderCache<'a>,
    index: usize,
    size: (u32, u32),
    crop: Option<Crop>,
) -> Result<(RgbaImage, Vec<InterpreterWarning>)> {
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
        cache,
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

/// Interprets a page without rendering it and measures what is visible; hayro's borrowed types
/// stay inside this function and `Collector`.
fn scan_page<'a>(pdf: &'a Pdf, cache: &InterpreterCache<'a>, index: usize) -> Result<PageScan> {
    let page = &pdf.pages()[index];
    let (width, height) = page.render_dimensions();
    let transform = page.initial_transform(true).to_kurbo();
    let page_box = Rect::new(0.0, 0.0, width as f64, height as f64);
    let mut context = PageContext::new(
        transform,
        page_box,
        cache,
        page.xref(),
        InterpreterSettings::default(),
    );
    let crop = transform
        .transform_rect_bbox(page.intersected_crop_box().to_kurbo())
        .intersect(page_box);
    let mut collector = Collector {
        clips: vec![crop],
        scan: PageScan {
            width: width as f64,
            height: height as f64,
            ..PageScan::default()
        },
    };
    interpret_page(page, &mut context, &mut collector);
    Ok(collector.scan)
}

/// Records visible glyphs and drawn areas. Clip paths are tracked by their bounding boxes.
struct Collector {
    clips: Vec<Rect>,
    scan: PageScan,
}

impl Collector {
    fn clip(&self) -> Rect {
        *self.clips.last().expect("the page clip is never popped")
    }

    fn visible_box(&self, bbox: Rect) -> Option<BBox> {
        let visible = bbox.intersect(self.clip());
        (visible.width() > 0.0 && visible.height() > 0.0)
            .then_some([visible.x0, visible.y0, visible.x1, visible.y1])
    }
}

impl<'a> Device<'a> for Collector {
    fn draw_path(&mut self, path: &BezPath, props: DrawProps<'a>, mode: &DrawMode) {
        let bbox = props.transform.transform_rect_bbox(path.bounding_box());
        let Some(visible) = self.visible_box(bbox) else {
            return;
        };
        match mode {
            DrawMode::Stroke(_) => self.scan.stroke_boxes.push(visible),
            DrawMode::Invisible => {}
            _ => self.scan.path_boxes.push(visible),
        }
    }

    fn push_clip_path(&mut self, clip_path: &ClipPath) {
        let clip = self.clip().intersect(clip_path.path.bounding_box());
        self.clips.push(clip);
    }

    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}

    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, mode: &DrawMode) {
        if matches!(mode, DrawMode::Invisible) {
            return;
        }
        let clip = self.clip();
        for glyph in run.glyphs() {
            let transform = props.transform * glyph.transform();
            let origin = transform * Point::ZERO;
            if !clip.contains(origin) {
                self.scan.hidden_glyphs += 1;
                continue;
            }
            let advance = match &**glyph {
                FontGlyph::Outline(outline) => outline.advance_width(),
                FontGlyph::Type3(_) => None,
            };
            // Glyph space uses 1000 units per em, so the transform scales by size / 1000.
            let end = transform * Point::new(advance.unwrap_or(500.0) as f64, 0.0);
            self.scan.glyphs.push(Glyph {
                start: (origin.x, origin.y),
                end: (end.x, end.y),
                size: transform.determinant().abs().sqrt() * 1000.0,
                text: glyph.as_unicode().map(|text| match text {
                    BfString::Char(c) => c.to_string(),
                    BfString::String(s) => s,
                }),
            });
        }
    }

    fn draw_image(&mut self, image: Image<'a, '_>, props: ImageDrawProps<'a>) {
        let pixels = Rect::new(0.0, 0.0, image.width() as f64, image.height() as f64);
        let bbox = props.transform.transform_rect_bbox(pixels);
        if let Some(visible) = self.visible_box(bbox) {
            self.scan.image_boxes.push(visible);
        }
    }

    fn pop_clip(&mut self) {
        if self.clips.len() > 1 {
            self.clips.pop();
        }
    }

    fn pop_transparency_group(&mut self) {}
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
