use std::path::Path;

use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use anyhow::{Context, Result};
use image::{Rgba, RgbaImage};

const EMBEDDED_FONT: &[u8] = include_bytes!("../assets/fonts/BIZUDGothic-Regular.ttf");

pub fn load_font(path: Option<&Path>) -> Result<FontArc> {
    match path {
        Some(path) => {
            let data = std::fs::read(path)
                .with_context(|| format!("failed to read font {}", path.display()))?;
            FontArc::try_from_vec(data).map_err(|_| {
                crate::error::usage(format!("{} is not a usable TTF/OTF font", path.display()))
                    .into()
            })
        }
        None => Ok(FontArc::try_from_slice(EMBEDDED_FONT).expect("embedded font is valid")),
    }
}

/// Height of one line of text (ascent + descent) at `size` pixels.
pub fn line_height(font: &FontArc, size: f32) -> u32 {
    font.as_scaled(PxScale::from(size)).height().ceil() as u32
}

pub fn text_width(font: &FontArc, size: f32, text: &str) -> u32 {
    imageproc::drawing::text_size(PxScale::from(size), font, text).0
}

/// Shortens `text` with a trailing ellipsis until it fits in `max_width` pixels.
pub fn fit_text(font: &FontArc, size: f32, text: &str, max_width: u32) -> String {
    if text_width(font, size, text) <= max_width {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>() + "…";
        if text_width(font, size, &candidate) <= max_width {
            return candidate;
        }
    }
    String::new()
}

pub fn draw_text(
    canvas: &mut RgbaImage,
    font: &FontArc,
    size: f32,
    x: i32,
    y: i32,
    color: Rgba<u8>,
    text: &str,
) {
    imageproc::drawing::draw_text_mut(canvas, color, x, y, PxScale::from(size), font, text);
}
