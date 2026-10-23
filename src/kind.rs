//! Defaults and names that depend on the input kind. Adding an `InputKind` means adding a
//! case to each method here.

use crate::cli::{Detail, Format, InputKind, LabelPos};

impl InputKind {
    pub fn name(self) -> &'static str {
        match self {
            InputKind::Pdf => "pdf",
            InputKind::Video => "video",
            InputKind::Anim => "anim",
            InputKind::Images => "images",
        }
    }

    pub fn guide_topic(self) -> &'static str {
        match self {
            InputKind::Pdf => "pdf",
            InputKind::Video | InputKind::Anim => "video",
            InputKind::Images => "images",
        }
    }

    pub fn default_format(self) -> Format {
        match self {
            InputKind::Pdf | InputKind::Images => Format::Png,
            InputKind::Video | InputKind::Anim => Format::Jpeg,
        }
    }

    pub fn default_label_pos(self) -> LabelPos {
        match self {
            InputKind::Pdf | InputKind::Images => LabelPos::Below,
            InputKind::Video | InputKind::Anim => LabelPos::Overlay,
        }
    }

    pub fn default_label(self) -> &'static str {
        match self {
            InputKind::Pdf => "p.{page}",
            InputKind::Video | InputKind::Anim => "{time}",
            InputKind::Images => "{name}",
        }
    }

    /// Variables a `--label` template may use.
    pub fn label_vars(self) -> &'static [&'static str] {
        match self {
            InputKind::Pdf => &["index", "total", "page"],
            InputKind::Video => &["index", "total", "time"],
            InputKind::Anim => &["index", "total", "time", "frame"],
            InputKind::Images => &["index", "total", "name"],
        }
    }

    /// `export --name` when it is not given.
    pub fn default_name(self) -> &'static str {
        match self {
            InputKind::Pdf => "{stem}-p{page}",
            InputKind::Video => "{stem}-{time}",
            InputKind::Anim => "{stem}-f{frame}",
            InputKind::Images => "{name}",
        }
    }

    /// Variables an `export --name` template may use.
    pub fn name_vars(self) -> &'static [&'static str] {
        match self {
            InputKind::Pdf => &["stem", "index", "total", "page"],
            InputKind::Video => &["stem", "index", "total", "time"],
            InputKind::Anim => &["stem", "index", "total", "time", "frame"],
            InputKind::Images => &["stem", "index", "total", "name"],
        }
    }

    /// Smallest long edge of a cell, in pixels. Text pages need the most room to stay legible.
    pub fn min_cell(self, detail: Detail) -> u32 {
        match (self, detail) {
            (InputKind::Pdf, Detail::Low) => 380,
            (InputKind::Pdf, Detail::Medium) => 580,
            (InputKind::Pdf, Detail::High) => 800,
            (InputKind::Video | InputKind::Anim, Detail::Low) => 330,
            (InputKind::Video | InputKind::Anim, Detail::Medium) => 400,
            (InputKind::Video | InputKind::Anim, Detail::High) => 560,
            (InputKind::Images, Detail::Low) => 330,
            (InputKind::Images, Detail::Medium) => 470,
            (InputKind::Images, Detail::High) => 630,
        }
    }
}
