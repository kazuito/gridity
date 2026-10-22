//! Pure analysis of what a PDF page shows: visible text, its size, and how much of the page
//! images and drawings cover. Coordinates are points on the displayed page, y pointing down.

use serde::Serialize;

/// One visible glyph: where its baseline starts and ends, and its font size in points.
#[derive(Debug, Clone, PartialEq)]
pub struct Glyph {
    pub start: (f64, f64),
    pub end: (f64, f64),
    pub size: f64,
    /// None when the font has no Unicode mapping for the glyph.
    pub text: Option<String>,
}

/// Bounding box: x0, y0, x1, y1.
pub type BBox = [f64; 4];

/// Raw measurements collected while interpreting a page.
#[derive(Debug, Clone, Default)]
pub struct PageScan {
    pub width: f64,
    pub height: f64,
    pub glyphs: Vec<Glyph>,
    /// Glyphs drawn outside the page or clipped away.
    pub hidden_glyphs: usize,
    /// Visible parts of drawn images, filled paths and stroke-only paths.
    pub image_boxes: Vec<BBox>,
    pub path_boxes: Vec<BBox>,
    pub stroke_boxes: Vec<BBox>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Content {
    /// Readable as extracted text.
    Text,
    /// Text plus figures, tables or diagrams worth looking at.
    Mixed,
    /// Little or unextractable text: look at the image.
    Graphics,
    Empty,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PageSummary {
    pub content: Content,
    /// Visible characters, whitespace excluded.
    pub chars: usize,
    /// Share of visible glyphs without a Unicode mapping.
    pub unmapped: f64,
    /// Median text size in points, weighted by characters.
    pub body_pt: Option<f64>,
    /// Size that 90% of the text is at least as large as.
    pub small_pt: Option<f64>,
    /// Sub- and superscript glyphs, a sign of formulas.
    pub scripts: usize,
    /// Share of text lines with at most two characters: scattered formula parts or labels.
    pub short_lines: f64,
    pub images: usize,
    /// Share of the page covered by images / by drawings.
    pub image_cover: f64,
    pub vector_cover: f64,
}

/// Text at least this many pixels high stays readable on a sheet.
pub const READABLE_PX: f64 = 8.0;
/// Below this, even small print (captions, subscripts) cannot be made out.
pub const SKIMMABLE_PX: f64 = 6.0;

/// Pages with fewer visible characters are treated as graphics.
const MIN_TEXT_CHARS: usize = 30;
/// Above this share of unmapped glyphs the extracted text is not trustworthy.
const MAX_UNMAPPED: f64 = 0.2;
const MIXED_IMAGE_COVER: f64 = 0.1;
const MIXED_VECTOR_COVER: f64 = 0.1;
/// Shapes covering nearly the whole page are backgrounds, not content.
const BACKGROUND_COVER: f64 = 0.9;
/// Outlines enclosing more than this share of the page are frames around the content.
const FRAME_COVER: f64 = 0.5;
const COVER_GRID: usize = 48;
const MIXED_SCRIPTS: usize = 5;
const MIXED_SHORT_LINES: f64 = 0.3;

pub fn summarize(scan: &PageScan) -> PageSummary {
    let mapped: Vec<&Glyph> = scan
        .glyphs
        .iter()
        .filter(|g| g.text.as_deref().is_some_and(|t| !t.trim().is_empty()))
        .collect();
    let unmapped_count = scan.glyphs.iter().filter(|g| g.text.is_none()).count();
    let chars = mapped
        .iter()
        .filter(|g| g.text.as_deref().is_some_and(|t| !is_private_use(t)))
        .count();
    let unmapped = if scan.glyphs.is_empty() {
        0.0
    } else {
        unmapped_count as f64 / scan.glyphs.len() as f64
    };
    let mut sizes: Vec<f64> = mapped.iter().map(|g| g.size).collect();
    sizes.sort_by(f64::total_cmp);
    let quantile = |q: f64| sizes.get((sizes.len() as f64 * q) as usize).copied();

    let image_cover = coverage(&scan.image_boxes, scan.width, scan.height);
    let page_area = scan.width * scan.height;
    let drawings: Vec<BBox> = scan
        .path_boxes
        .iter()
        .chain(
            scan.stroke_boxes
                .iter()
                .filter(|b| (b[2] - b[0]) * (b[3] - b[1]) < FRAME_COVER * page_area),
        )
        .copied()
        .collect();
    let vector_cover = coverage(&drawings, scan.width, scan.height);
    let scripts = count_scripts(&scan.glyphs);
    let text = assemble_text(&scan.glyphs);
    let lines: Vec<&str> = text.lines().collect();
    let short_lines = if lines.is_empty() {
        0.0
    } else {
        let short = lines
            .iter()
            .filter(|line| line.chars().filter(|c| !c.is_whitespace()).count() <= 2)
            .count();
        short as f64 / lines.len() as f64
    };
    let content = if chars == 0 && image_cover + vector_cover < 0.01 {
        Content::Empty
    } else if chars < MIN_TEXT_CHARS || unmapped > MAX_UNMAPPED {
        Content::Graphics
    } else if image_cover >= MIXED_IMAGE_COVER
        || vector_cover >= MIXED_VECTOR_COVER
        || scripts >= MIXED_SCRIPTS
        || short_lines >= MIXED_SHORT_LINES
    {
        Content::Mixed
    } else {
        Content::Text
    };
    PageSummary {
        content,
        chars,
        unmapped: round2(unmapped),
        body_pt: quantile(0.5).map(round1),
        small_pt: quantile(0.1).map(round1),
        scripts,
        short_lines: round2(short_lines),
        images: scan.image_boxes.len(),
        image_cover: round2(image_cover),
        vector_cover: round2(vector_cover),
    }
}

impl PageSummary {
    /// One-line description, e.g. `mixed · 1240 chars · body 11pt · drawings 30%`.
    pub fn describe(&self) -> String {
        let content = match self.content {
            Content::Text => "text",
            Content::Mixed => "mixed",
            Content::Graphics => "graphics",
            Content::Empty => "empty",
        };
        let mut parts = vec![content.to_string(), format!("{} chars", self.chars)];
        if let Some(body) = self.body_pt {
            parts.push(format!("body {body}pt"));
        }
        if self
            .small_pt
            .is_some_and(|small| Some(small) != self.body_pt)
        {
            parts.push(format!("small {}pt", self.small_pt.unwrap_or_default()));
        }
        if self.scripts >= MIXED_SCRIPTS || self.short_lines >= MIXED_SHORT_LINES {
            parts.push("formulas".to_string());
        }
        if self.images > 0 {
            parts.push(format!(
                "{} image(s) {:.0}%",
                self.images,
                self.image_cover * 100.0
            ));
        }
        if self.vector_cover >= 0.01 {
            parts.push(format!("drawings {:.0}%", self.vector_cover * 100.0));
        }
        if self.unmapped > 0.0 {
            parts.push(format!("unmapped {:.0}%", self.unmapped * 100.0));
        }
        parts.join(" · ")
    }
}

/// Typical body text size of a document: the median body size of its pages that have text.
pub fn document_body_pt(pages: &[PageSummary]) -> Option<f64> {
    let mut sizes: Vec<f64> = pages
        .iter()
        .filter(|p| p.chars >= MIN_TEXT_CHARS)
        .filter_map(|p| p.body_pt)
        .collect();
    sizes.sort_by(f64::total_cmp);
    sizes.get(sizes.len() / 2).copied()
}

/// Share of the page covered by the boxes, measured on a coarse grid so overlaps count once.
fn coverage(boxes: &[BBox], width: f64, height: f64) -> f64 {
    if width <= 0.0 || height <= 0.0 {
        return 0.0;
    }
    let mut covered = vec![false; COVER_GRID * COVER_GRID];
    let page_area = width * height;
    for &[x0, y0, x1, y1] in boxes {
        if (x1 - x0) * (y1 - y0) >= BACKGROUND_COVER * page_area {
            continue;
        }
        let col = |x: f64| ((x / width * COVER_GRID as f64).clamp(0.0, COVER_GRID as f64)) as usize;
        let row =
            |y: f64| ((y / height * COVER_GRID as f64).clamp(0.0, COVER_GRID as f64)) as usize;
        for r in row(y0)..row(y1).max(row(y0) + 1).min(COVER_GRID) {
            for c in col(x0)..col(x1).max(col(x0) + 1).min(COVER_GRID) {
                covered[r * COVER_GRID + c] = true;
            }
        }
    }
    covered.iter().filter(|&&c| c).count() as f64 / covered.len() as f64
}

/// Joins glyphs in drawing order into lines. A glyph off the previous glyph's baseline starts a
/// new line; a gap along the baseline becomes a space. Works for rotated text too.
pub fn assemble_text(glyphs: &[Glyph]) -> String {
    let mut out = String::new();
    let mut prev: Option<&Glyph> = None;
    for glyph in glyphs {
        let Some(text) = glyph.text.as_deref().filter(|t| !is_private_use(t)) else {
            continue;
        };
        if let Some(p) = prev {
            let (dx, dy) = (p.end.0 - p.start.0, p.end.1 - p.start.1);
            let len = (dx * dx + dy * dy).sqrt();
            let (ux, uy) = if len > 0.0 {
                (dx / len, dy / len)
            } else {
                (1.0, 0.0)
            };
            let size = p.size.max(glyph.size);
            let off_x = glyph.start.0 - p.start.0;
            let off_y = glyph.start.1 - p.start.1;
            let across = (off_x * uy - off_y * ux).abs();
            let along = (glyph.start.0 - p.end.0) * ux + (glyph.start.1 - p.end.1) * uy;
            if across > 0.5 * size || along < -size {
                out.push('\n');
            } else if along > 0.2 * size && !out.ends_with(char::is_whitespace) && text != " " {
                out.push(' ');
            }
        }
        if !(text == " " && out.ends_with(char::is_whitespace)) {
            out.push_str(text);
        }
        prev = Some(glyph);
    }
    out.lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Glyphs noticeably smaller than the glyph before them and shifted off its baseline.
fn count_scripts(glyphs: &[Glyph]) -> usize {
    glyphs
        .windows(2)
        .filter(|pair| {
            let (prev, glyph) = (&pair[0], &pair[1]);
            let shift = ((glyph.start.0 - prev.start.0).powi(2)
                + (glyph.start.1 - prev.start.1).powi(2))
            .sqrt();
            let baseline_shift = {
                let (dx, dy) = (prev.end.0 - prev.start.0, prev.end.1 - prev.start.1);
                let len = (dx * dx + dy * dy).sqrt().max(f64::EPSILON);
                ((glyph.start.0 - prev.end.0) * dy - (glyph.start.1 - prev.end.1) * dx).abs() / len
            };
            glyph.text.is_some()
                && glyph.size < 0.85 * prev.size
                && baseline_shift > 0.1 * prev.size
                && baseline_shift < 0.6 * prev.size
                && shift < 2.0 * prev.size
        })
        .count()
}

/// Fonts map pieces of large brackets and other drawing glyphs to the Private Use Area.
fn is_private_use(text: &str) -> bool {
    text.chars().all(|c| ('\u{E000}'..='\u{F8FF}').contains(&c))
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, x: f64, y: f64, size: f64) -> Vec<Glyph> {
        let advance = size * 0.5;
        text.chars()
            .enumerate()
            .map(|(i, c)| {
                let start = x + i as f64 * advance;
                Glyph {
                    start: (start, y),
                    end: (start + advance, y),
                    size,
                    text: Some(c.to_string()),
                }
            })
            .collect()
    }

    fn page(glyphs: Vec<Glyph>) -> PageScan {
        PageScan {
            width: 600.0,
            height: 800.0,
            glyphs,
            ..PageScan::default()
        }
    }

    #[test]
    fn assembles_lines_and_word_gaps() {
        let mut glyphs = line("Hello", 50.0, 100.0, 12.0);
        glyphs.extend(line("world", 50.0 + 6.0 * 6.0, 100.0, 12.0));
        glyphs.extend(line("next", 50.0, 120.0, 12.0));
        assert_eq!(assemble_text(&glyphs), "Hello world\nnext");
    }

    #[test]
    fn assembles_rotated_text() {
        let glyphs: Vec<Glyph> = "abc"
            .chars()
            .enumerate()
            .map(|(i, c)| Glyph {
                start: (100.0, 50.0 + i as f64 * 6.0),
                end: (100.0, 56.0 + i as f64 * 6.0),
                size: 12.0,
                text: Some(c.to_string()),
            })
            .collect();
        assert_eq!(assemble_text(&glyphs), "abc");
    }

    #[test]
    fn skips_private_use_and_unmapped_glyphs() {
        let mut glyphs = line("A=", 0.0, 10.0, 10.0);
        glyphs.push(Glyph {
            text: Some("\u{f8eb}".into()),
            ..glyphs[1].clone()
        });
        glyphs.push(Glyph {
            text: None,
            ..glyphs[1].clone()
        });
        assert_eq!(assemble_text(&glyphs), "A=");
        let summary = summarize(&page(glyphs));
        assert_eq!(summary.chars, 2);
        assert_eq!(summary.unmapped, 0.25);
    }

    #[test]
    fn classifies_pages() {
        let text = line(&"x".repeat(200), 50.0, 100.0, 11.0);
        let summary = summarize(&page(text.clone()));
        assert_eq!(summary.content, Content::Text);
        assert_eq!(summary.body_pt, Some(11.0));

        let mut figure = page(text);
        figure.path_boxes.push([0.0, 400.0, 600.0, 800.0]);
        assert_eq!(summarize(&figure).content, Content::Mixed);

        let mut background = page(line(&"x".repeat(200), 50.0, 100.0, 11.0));
        background.path_boxes.push([0.0, 0.0, 600.0, 800.0]);
        background.stroke_boxes.push([20.0, 20.0, 580.0, 780.0]);
        assert_eq!(summarize(&background).content, Content::Text);

        let mut photo = page(line("Fig. 1", 50.0, 100.0, 9.0));
        photo.image_boxes.push([0.0, 0.0, 600.0, 600.0]);
        let summary = summarize(&photo);
        assert_eq!(summary.content, Content::Graphics);
        assert_eq!(summary.image_cover, 0.75);

        assert_eq!(summarize(&page(Vec::new())).content, Content::Empty);
    }

    #[test]
    fn document_body_ignores_pages_without_text() {
        let text = summarize(&page(line(&"x".repeat(100), 0.0, 100.0, 11.0)));
        let title = summarize(&page(line("Title", 0.0, 100.0, 40.0)));
        assert_eq!(
            document_body_pt(&[title.clone(), text.clone(), text]),
            Some(11.0)
        );
        assert_eq!(document_body_pt(&[title]), None);
    }

    #[test]
    fn coverage_counts_overlaps_once() {
        let half = [0.0, 0.0, 300.0, 800.0];
        assert_eq!(coverage(&[half, half], 600.0, 800.0), 0.5);
    }

    #[test]
    fn small_and_body_sizes_are_weighted_by_characters() {
        let mut glyphs = line(&"a".repeat(90), 0.0, 100.0, 10.0);
        glyphs.extend(line(&"b".repeat(10), 0.0, 200.0, 6.0));
        let summary = summarize(&page(glyphs));
        assert_eq!(summary.body_pt, Some(10.0));
        assert_eq!(summary.small_pt, Some(10.0));
        let mut glyphs = line(&"a".repeat(80), 0.0, 100.0, 10.0);
        glyphs.extend(line(&"b".repeat(20), 0.0, 200.0, 6.0));
        assert_eq!(summarize(&page(glyphs)).small_pt, Some(6.0));
    }
}
