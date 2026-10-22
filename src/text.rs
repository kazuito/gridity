use std::path::PathBuf;

use ab_glyph::{Font, FontArc, GlyphId, OutlinedGlyph, PxScale, ScaleFont, point};
use anyhow::{Context, Result};
use image::{Pixel, Rgba, RgbaImage};

const EMBEDDED_FONT: &[u8] = include_bytes!("../assets/fonts/BIZUDGothic-Regular.ttf");

/// Fonts tried in order for every character: `--font` files, then the embedded BIZ UDGothic.
/// There is no shaping, so scripts that need it (Arabic, Thai, ...) show unjoined letters.
pub struct Fonts(Vec<FontArc>);

impl Fonts {
    pub fn load(paths: &[PathBuf]) -> Result<Fonts> {
        let mut fonts = Vec::with_capacity(paths.len() + 1);
        for path in paths {
            let data = std::fs::read(path)
                .with_context(|| format!("failed to read font {}", path.display()))?;
            let font = FontArc::try_from_vec(data).map_err(|_| {
                crate::error::usage(
                    crate::error::Reason::InvalidValue,
                    format!("{} is not a usable TTF/OTF/TTC font", path.display()),
                )
            })?;
            fonts.push(font);
        }
        fonts.push(FontArc::try_from_slice(EMBEDDED_FONT).expect("embedded font is valid"));
        Ok(Fonts(fonts))
    }

    /// The first font that can draw `c`; color-only glyphs (emoji) do not count.
    fn pick(&self, c: char) -> Option<(&FontArc, GlyphId)> {
        self.0.iter().find_map(|font| {
            let id = font.glyph_id(c);
            let drawable = id.0 != 0 && (c.is_whitespace() || font.outline(id).is_some());
            drawable.then_some((font, id))
        })
    }

    /// Characters of `text` that no font can draw; they appear as boxes.
    pub fn missing(&self, text: &str) -> Vec<char> {
        let mut missing: Vec<char> = text
            .chars()
            .filter(|&c| !c.is_control() && self.pick(c).is_none())
            .collect();
        missing.dedup();
        missing
    }

    fn ascent(&self, size: f32) -> f32 {
        self.0
            .iter()
            .map(|font| font.as_scaled(PxScale::from(size)).ascent())
            .fold(0.0, f32::max)
    }

    /// Places every glyph on one line whose top is at y = 0 and returns the advance width.
    fn layout(&self, size: f32, text: &str, mut place: impl FnMut(OutlinedGlyph)) -> f32 {
        let scale = PxScale::from(size);
        let baseline = self.ascent(size);
        let mut x = 0.0;
        let mut previous: Option<(&FontArc, GlyphId)> = None;
        for c in text.chars() {
            let (font, id) = self.pick(c).unwrap_or((&self.0[0], self.0[0].glyph_id(c)));
            let scaled = font.as_scaled(scale);
            if let Some((previous_font, previous_id)) = previous
                && std::ptr::eq(previous_font, font)
            {
                x += scaled.kern(previous_id, id);
            }
            let positioned = id.with_scale_and_position(scale, point(x, baseline));
            if let Some(glyph) = font.outline_glyph(positioned) {
                place(glyph);
            }
            x += scaled.h_advance(id);
            previous = Some((font, id));
        }
        x
    }
}

/// Height of one line of text (ascent + descent) at `size` pixels.
pub fn line_height(fonts: &Fonts, size: f32) -> u32 {
    let scale = PxScale::from(size);
    let descent = fonts
        .0
        .iter()
        .map(|font| font.as_scaled(scale).descent())
        .fold(0.0, f32::min);
    (fonts.ascent(size) - descent).ceil() as u32
}

pub fn text_width(fonts: &Fonts, size: f32, text: &str) -> u32 {
    if text.is_empty() {
        return 0;
    }
    1 + fonts.layout(size, text, |_| {}).ceil() as u32
}

/// Shortens `text` with a trailing ellipsis until it fits in `max_width` pixels.
pub fn fit_text(fonts: &Fonts, size: f32, text: &str, max_width: u32) -> String {
    if text_width(fonts, size, text) <= max_width {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>() + "…";
        if text_width(fonts, size, &candidate) <= max_width {
            return candidate;
        }
    }
    String::new()
}

/// Shortens `text` by replacing its middle with an ellipsis, keeping the end (e.g. a file extension).
pub fn fit_middle(fonts: &Fonts, size: f32, text: &str, max_width: u32) -> String {
    if text_width(fonts, size, text) <= max_width {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    for keep in (1..chars.len()).rev() {
        let tail = keep / 3;
        let candidate: String = chars[..keep - tail]
            .iter()
            .chain(std::iter::once(&'…'))
            .chain(&chars[chars.len() - tail..])
            .collect();
        if text_width(fonts, size, &candidate) <= max_width {
            return candidate;
        }
    }
    String::new()
}

pub fn draw_text(
    canvas: &mut RgbaImage,
    fonts: &Fonts,
    size: f32,
    x: i32,
    y: i32,
    color: Rgba<u8>,
    text: &str,
) {
    let (width, height) = (canvas.width() as i32, canvas.height() as i32);
    fonts.layout(size, text, |glyph| {
        let bounds = glyph.px_bounds();
        let (left, top) = (
            x + bounds.min.x.round() as i32,
            y + bounds.min.y.round() as i32,
        );
        glyph.draw(|gx, gy, coverage| {
            let (px, py) = (left + gx as i32, top + gy as i32);
            if (0..width).contains(&px) && (0..height).contains(&py) {
                let mut ink = color;
                ink.0[3] = (color.0[3] as f32 * coverage.clamp(0.0, 1.0)).round() as u8;
                canvas.get_pixel_mut(px as u32, py as u32).blend(&ink);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_characters_are_reported() {
        let fonts = Fonts::load(&[]).unwrap();
        assert_eq!(fonts.missing("日本語 abc_01.png"), Vec::<char>::new());
        assert_eq!(fonts.missing("한🎉"), vec!['한', '🎉']);
    }

    #[test]
    fn fitted_text_fits() {
        let fonts = Fonts::load(&[]).unwrap();
        let text = "a rather long file name for a label 長いファイル名.png";
        for max in [40, 120, 300] {
            assert!(text_width(&fonts, 15.0, &fit_text(&fonts, 15.0, text, max)) <= max);
            assert!(text_width(&fonts, 15.0, &fit_middle(&fonts, 15.0, text, max)) <= max);
        }
        assert_eq!(fit_text(&fonts, 15.0, "short", 300), "short");
    }

    #[test]
    fn text_is_drawn_inside_its_line() {
        let fonts = Fonts::load(&[]).unwrap();
        let height = line_height(&fonts, 20.0);
        let mut canvas = RgbaImage::from_pixel(200, height + 10, Rgba([255, 255, 255, 255]));
        draw_text(
            &mut canvas,
            &fonts,
            20.0,
            0,
            0,
            Rgba([0, 0, 0, 255]),
            "Ag日本",
        );
        let inked_rows: Vec<u32> = (0..canvas.height())
            .filter(|&y| (0..canvas.width()).any(|x| canvas.get_pixel(x, y).0[0] < 128))
            .collect();
        assert!(!inked_rows.is_empty());
        assert!(*inked_rows.last().unwrap() < height);
    }
}
