use image::{Rgba, RgbaImage, imageops};
use imageproc::drawing::draw_hollow_rect_mut;
use imageproc::rect::Rect;

use crate::cli::LabelPos;
use crate::color::is_dark;
use crate::layout::{Chrome, Grid};
use crate::text::{Fonts, draw_text, fit_middle, fit_text, line_height, text_width};

pub const HEADER_SIZE: f32 = 17.0;

#[derive(Debug, Clone)]
pub struct Style {
    pub bg: Rgba<u8>,
    pub label_pos: Option<LabelPos>,
    pub label_size: f32,
    pub label_color: Rgba<u8>,
    pub label_bg: Option<Rgba<u8>>,
    pub header_color: Rgba<u8>,
    pub border: Rgba<u8>,
}

impl Style {
    pub fn new(
        bg: Rgba<u8>,
        label_pos: Option<LabelPos>,
        label_size: f32,
        label_color: Option<Rgba<u8>>,
        label_bg: Option<Rgba<u8>>,
    ) -> Style {
        let dark = is_dark(bg);
        let text_on_bg = if dark {
            Rgba([235, 235, 235, 255])
        } else {
            Rgba([30, 30, 30, 255])
        };
        let (default_color, default_bg) = match label_pos {
            Some(LabelPos::Overlay) => (Rgba([255, 255, 255, 255]), Some(Rgba([0, 0, 0, 170]))),
            _ => (text_on_bg, None),
        };
        Style {
            bg,
            label_pos,
            label_size,
            label_color: label_color.unwrap_or(default_color),
            label_bg: label_bg.or(default_bg),
            header_color: text_on_bg,
            border: if dark {
                Rgba([90, 90, 90, 255])
            } else {
                Rgba([200, 200, 200, 255])
            },
        }
    }

    /// Height of the strip reserved under each cell for its label.
    pub fn label_strip(&self, font: &Fonts) -> u32 {
        match self.label_pos {
            Some(LabelPos::Below) => line_height(font, self.label_size) + 6,
            _ => 0,
        }
    }
}

pub fn header_height(font: &Fonts) -> u32 {
    line_height(font, HEADER_SIZE) + 6
}

/// Header line: `name` (file name or `--title`) is shortened first so `details` stays readable.
pub struct Header {
    pub name: String,
    /// Range, crop and sheet number, e.g. "p.1–4 of 42 · sheet 1/11".
    pub details: String,
}

const SEPARATOR: &str = " · ";

/// The header text that fits in `max_width`, shortening the name before the details.
pub fn header_line(font: &Fonts, header: &Header, max_width: u32) -> String {
    if header.details.is_empty() {
        return fit_middle(font, HEADER_SIZE, &header.name, max_width);
    }
    let full = format!("{}{SEPARATOR}{}", header.name, header.details);
    if text_width(font, HEADER_SIZE, &full) <= max_width {
        return full;
    }
    let tail = format!("{SEPARATOR}{}", header.details);
    let room = max_width.saturating_sub(text_width(font, HEADER_SIZE, &tail));
    let name = fit_middle(font, HEADER_SIZE, &header.name, room);
    if name.chars().count() > 1 {
        return name + &tail;
    }
    fit_text(font, HEADER_SIZE, &full, max_width)
}

pub struct Cell {
    pub image: RgbaImage,
    pub label: Option<String>,
}

/// Rectangle of a placed image on the sheet: x, y, width, height.
pub type CellRect = [u32; 4];

/// Where an image of `size` goes inside cell `index`: centered, or resting on the label strip.
pub fn place(
    grid: Grid,
    chrome: Chrome,
    index: usize,
    size: (u32, u32),
    label_pos: Option<LabelPos>,
) -> CellRect {
    let (slot_x, slot_y) = grid.cell_origin(chrome, index);
    let (w, h) = size;
    let free_y = grid.cell_height.saturating_sub(h);
    let y = match label_pos {
        Some(LabelPos::Below) => slot_y + free_y,
        _ => slot_y + free_y / 2,
    };
    [slot_x + grid.cell_width.saturating_sub(w) / 2, y, w, h]
}

pub fn compose(
    grid: Grid,
    chrome: Chrome,
    style: &Style,
    font: &Fonts,
    header: Option<&Header>,
    cells: Vec<Cell>,
) -> (RgbaImage, Vec<CellRect>) {
    let (width, height) = grid.sheet_size(chrome);
    let mut canvas = RgbaImage::from_pixel(width, height, style.bg);

    if let Some(header) = header {
        let text = header_line(font, header, width.saturating_sub(2 * chrome.padding));
        draw_text(
            &mut canvas,
            font,
            HEADER_SIZE,
            chrome.padding as i32,
            chrome.padding as i32,
            style.header_color,
            &text,
        );
    }

    let mut rects = Vec::with_capacity(cells.len());
    for (index, cell) in cells.into_iter().enumerate() {
        let (slot_x, slot_y) = grid.cell_origin(chrome, index);
        let [x, y, w, h] = place(
            grid,
            chrome,
            index,
            cell.image.dimensions(),
            style.label_pos,
        );

        draw_hollow_rect_mut(
            &mut canvas,
            Rect::at(x as i32 - 1, y as i32 - 1).of_size(w + 2, h + 2),
            style.border,
        );
        imageops::overlay(&mut canvas, &cell.image, x as i64, y as i64);
        rects.push([x, y, w, h]);

        if let Some(label) = cell.label.as_deref().filter(|l| !l.is_empty()) {
            match style.label_pos {
                Some(LabelPos::Below) => {
                    let strip_y = slot_y + grid.cell_height;
                    draw_label_below(
                        &mut canvas,
                        font,
                        style,
                        label,
                        slot_x,
                        strip_y,
                        grid.cell_width,
                        chrome.label,
                    );
                }
                Some(LabelPos::Overlay) => {
                    draw_label_overlay(&mut canvas, font, style, label, x, y, w)
                }
                None => {}
            }
        }
    }
    (canvas, rects)
}

#[allow(clippy::too_many_arguments)]
fn draw_label_below(
    canvas: &mut RgbaImage,
    font: &Fonts,
    style: &Style,
    label: &str,
    x: u32,
    y: u32,
    width: u32,
    strip: u32,
) {
    let text = fit_text(font, style.label_size, label, width);
    let text_w = text_width(font, style.label_size, &text);
    let text_h = line_height(font, style.label_size);
    if let Some(bg) = style.label_bg {
        fill_rect(canvas, x, y + 2, width, strip.saturating_sub(2), bg);
    }
    let tx = x + width.saturating_sub(text_w) / 2;
    let ty = y + strip.saturating_sub(text_h) / 2 + 1;
    draw_text(
        canvas,
        font,
        style.label_size,
        tx as i32,
        ty as i32,
        style.label_color,
        &text,
    );
}

fn draw_label_overlay(
    canvas: &mut RgbaImage,
    font: &Fonts,
    style: &Style,
    label: &str,
    x: u32,
    y: u32,
    max_width: u32,
) {
    let pad_x = 5;
    let pad_y = 2;
    let text = fit_text(
        font,
        style.label_size,
        label,
        max_width.saturating_sub(2 * pad_x),
    );
    let text_w = text_width(font, style.label_size, &text);
    let text_h = line_height(font, style.label_size);
    if let Some(bg) = style.label_bg {
        fill_rect(canvas, x, y, text_w + 2 * pad_x, text_h + 2 * pad_y, bg);
    }
    draw_text(
        canvas,
        font,
        style.label_size,
        (x + pad_x) as i32,
        (y + pad_y) as i32,
        style.label_color,
        &text,
    );
}

/// Alpha-blends a solid rectangle onto the canvas.
fn fill_rect(canvas: &mut RgbaImage, x: u32, y: u32, width: u32, height: u32, color: Rgba<u8>) {
    let alpha = color[3] as f32 / 255.0;
    let x_end = (x + width).min(canvas.width());
    let y_end = (y + height).min(canvas.height());
    for py in y..y_end {
        for px in x..x_end {
            let pixel = canvas.get_pixel_mut(px, py);
            for c in 0..3 {
                pixel[c] =
                    (color[c] as f32 * alpha + pixel[c] as f32 * (1.0 - alpha)).round() as u8;
            }
            pixel[3] = pixel[3].max(color[3]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_keeps_range_and_sheet_number() {
        let font = Fonts::load(&[]).unwrap();
        let header = Header {
            name:
                "2024_Annual_Financial_Report_of_the_Company_Final_Revised_v3_for_board_review.pdf"
                    .to_string(),
            details: "p.1–2 of 12 · sheet 1/6".to_string(),
        };
        let line = header_line(&font, &header, 520);
        assert!(line.ends_with(" · p.1–2 of 12 · sheet 1/6"), "{line}");
        assert!(line.contains('…') && line.contains(".pdf"), "{line}");
        assert!(text_width(&font, HEADER_SIZE, &line) <= 520);

        let short = Header {
            name: "deck.pdf".to_string(),
            details: "p.1–4 of 42".to_string(),
        };
        assert_eq!(header_line(&font, &short, 520), "deck.pdf · p.1–4 of 42");
    }

    #[test]
    fn long_title_keeps_sheet_number() {
        let font = Fonts::load(&[]).unwrap();
        let header = Header {
            name: "A very long custom title that goes on and on describing the material".repeat(2),
            details: "sheet 2/3".to_string(),
        };
        let line = header_line(&font, &header, 400);
        assert!(line.ends_with(" · sheet 2/3"), "{line}");
        assert!(text_width(&font, HEADER_SIZE, &line) <= 400);
    }
}
