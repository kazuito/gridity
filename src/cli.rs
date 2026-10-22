use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use image::Rgba;

use crate::color::parse_color;

const RENDER_EXAMPLES: &str = "\
Examples:
  gridity render report.pdf                      # all pages, 4 per sheet
  gridity render report.pdf --pages 3-8,12       # selected pages
  gridity render talk.mp4                        # one sheet of evenly spaced frames
  gridity render talk.mp4 --start 1:00 --end 2:30 --every 10s
  gridity render shots/ --cols 3                 # every image in a directory
  gridity render anim.gif --count 9 --json       # manifest with cell coordinates

Output: absolute paths of the written sheets, one per line (or a JSON manifest with --json).
For agents: run `gridity guide` for usage, recipes and the output schema.";

const ROOT_HELP: &str = "\
Run `gridity render --help` for all options.
For agents: run `gridity guide` for usage, recipes and the output schema.";

#[derive(Debug, Parser)]
#[command(
    name = "gridity",
    version,
    about = "Arrange PDF pages, video frames and images into labeled grid sheets for LLM vision",
    after_help = ROOT_HELP
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Render grid sheets from a PDF, a video, a GIF/APNG or image files
    #[command(after_help = RENDER_EXAMPLES)]
    Render(Box<RenderArgs>),
    /// Show page count, duration, resolution and sheet estimates for an input
    Probe(ProbeArgs),
    /// Print usage documentation for agents (Markdown)
    Guide(GuideArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InputKind {
    Pdf,
    Video,
    /// Animated GIF or APNG
    Anim,
    Images,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Preset {
    /// Long edge <= 1568px and <= 1568 visual tokens (works with every Claude model)
    Compat,
    /// Long edge <= 2576px and <= 4784 visual tokens (Claude 4.7 and later models)
    Hires,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Detail {
    /// More items per sheet, smaller cells
    Low,
    Medium,
    /// Fewer items per sheet, larger cells
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Png,
    Jpeg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LabelPos {
    /// Strip below each cell
    Below,
    /// Badge in the top-left corner of each cell
    Overlay,
}

#[derive(Debug, Clone, Args)]
pub struct RenderArgs {
    /// Input file(s). Several images or a directory of images are allowed
    #[arg(required = true, value_name = "INPUT")]
    pub inputs: Vec<PathBuf>,

    /// Input type (detected from the file when omitted)
    #[arg(long = "type", value_enum, value_name = "TYPE")]
    pub kind: Option<InputKind>,

    #[command(flatten)]
    pub source: SourceArgs,

    #[command(flatten)]
    pub selection: SelectionArgs,

    /// Output file or directory [default: $TMPDIR/gridity/<name>-<hash>/]
    #[arg(short, long, value_name = "PATH", help_heading = "Output")]
    pub output: Option<PathBuf>,

    /// Image format [default: png for PDF/images, jpeg for video/animations]
    #[arg(long, value_enum, help_heading = "Output")]
    pub format: Option<Format>,

    /// JPEG quality (1-100)
    #[arg(long, default_value_t = 85, value_parser = clap::value_parser!(u8).range(1..=100), help_heading = "Output")]
    pub quality: u8,

    /// Print a JSON manifest (paths, grid, cell rectangles, labels) instead of plain paths
    #[arg(long, help_heading = "Output")]
    pub json: bool,

    /// Plan the layout and print it without rendering anything
    #[arg(long, help_heading = "Output")]
    pub dry_run: bool,

    /// Suppress warnings on stderr
    #[arg(short, long, help_heading = "Output")]
    pub quiet: bool,

    /// Print progress details on stderr
    #[arg(short, long, help_heading = "Output")]
    pub verbose: bool,

    #[command(flatten)]
    pub layout: LayoutArgs,

    #[command(flatten)]
    pub style: StyleArgs,

    /// Number of parallel workers [default: min(CPU cores, 8)]
    #[arg(short, long, value_name = "N")]
    pub jobs: Option<usize>,
}

#[derive(Debug, Clone, Args)]
pub struct SourceArgs {
    /// Password for an encrypted PDF
    #[arg(long, help_heading = "PDF")]
    pub password: Option<String>,

    /// Path to ffmpeg (or set GRIDITY_FFMPEG); ffprobe is looked up next to it
    #[arg(long, value_name = "PATH", help_heading = "Video")]
    pub ffmpeg: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Args)]
pub struct SelectionArgs {
    /// Pages to include, 1-based: "1-3,5,10-", "-4" [default: all]
    #[arg(long, value_name = "LIST", help_heading = "PDF")]
    pub pages: Option<String>,

    /// Start time: "90", "1:30", "00:01:30.5", "1m30s" [default: 0]
    #[arg(long, value_name = "TIME", help_heading = "Video / animation")]
    pub start: Option<String>,

    /// End time [default: end of the media]
    #[arg(long, value_name = "TIME", help_heading = "Video / animation")]
    pub end: Option<String>,

    /// Take one frame every interval, e.g. "5s", "0.5", "1m"
    #[arg(long, value_name = "TIME", conflicts_with_all = ["count", "at"], help_heading = "Video / animation")]
    pub every: Option<String>,

    /// Take N evenly spaced frames [default: as many as fit on one sheet]
    #[arg(
        long,
        value_name = "N",
        conflicts_with = "at",
        help_heading = "Video / animation"
    )]
    pub count: Option<usize>,

    /// Take frames at explicit times: "0:10,1:30,2:00"
    #[arg(long, value_name = "TIMES", help_heading = "Video / animation")]
    pub at: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct LayoutArgs {
    /// Size limits of each sheet
    #[arg(long, value_enum, default_value = "compat", help_heading = "Layout")]
    pub preset: Preset,

    /// Override the preset's maximum sheet edge in pixels
    #[arg(long, value_name = "PX", help_heading = "Layout")]
    pub max_edge: Option<u32>,

    /// Override the preset's visual token budget per sheet (one token per 28x28 px)
    #[arg(long, value_name = "N", help_heading = "Layout")]
    pub max_tokens: Option<u32>,

    /// How large each cell must stay; decides how many items go on a sheet
    #[arg(long, value_enum, default_value = "medium", help_heading = "Layout")]
    pub detail: Detail,

    /// Items per sheet (overrides --detail)
    #[arg(long, value_name = "N", help_heading = "Layout")]
    pub per_sheet: Option<usize>,

    /// Columns per sheet
    #[arg(long, value_name = "N", help_heading = "Layout")]
    pub cols: Option<u32>,

    /// Rows per sheet
    #[arg(long, value_name = "N", help_heading = "Layout")]
    pub rows: Option<u32>,

    /// Fail instead of writing more sheets than this
    #[arg(long, value_name = "N", default_value_t = 10, help_heading = "Layout")]
    pub max_sheets: usize,

    /// Space between cells in pixels
    #[arg(long, value_name = "PX", default_value_t = 8, help_heading = "Layout")]
    pub gap: u32,

    /// Space around the grid in pixels
    #[arg(long, value_name = "PX", default_value_t = 12, help_heading = "Layout")]
    pub padding: u32,
}

#[derive(Debug, Clone, Args)]
pub struct StyleArgs {
    /// Background color
    #[arg(long, value_name = "COLOR", default_value = "#ffffff", value_parser = parse_color, help_heading = "Style")]
    pub bg: Rgba<u8>,

    /// Header text [default: source name, range and sheet number]
    #[arg(long, value_name = "TEXT", help_heading = "Style")]
    pub title: Option<String>,

    /// Do not draw the header
    #[arg(long, conflicts_with = "title", help_heading = "Style")]
    pub no_header: bool,

    /// Label template. Variables: {index} {total} {page} {time} {frame} {name}
    #[arg(long, value_name = "TEMPLATE", help_heading = "Style")]
    pub label: Option<String>,

    /// Do not draw labels
    #[arg(long, conflicts_with = "label", help_heading = "Style")]
    pub no_label: bool,

    /// Label position [default: below for PDF/images, overlay for video/animations]
    #[arg(long, value_enum, help_heading = "Style")]
    pub label_pos: Option<LabelPos>,

    /// Label font size in pixels
    #[arg(
        long,
        value_name = "PX",
        default_value_t = 15.0,
        help_heading = "Style"
    )]
    pub label_size: f32,

    /// Label text color [default: depends on --bg and --label-pos]
    #[arg(long, value_name = "COLOR", value_parser = parse_color, help_heading = "Style")]
    pub label_color: Option<Rgba<u8>>,

    /// Label background color [default: none below cells, translucent black for overlays]
    #[arg(long, value_name = "COLOR", value_parser = parse_color, help_heading = "Style")]
    pub label_bg: Option<Rgba<u8>>,

    /// TTF/OTF font for header and labels [default: embedded BIZ UDGothic]
    #[arg(long, value_name = "PATH", help_heading = "Style")]
    pub font: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
pub struct ProbeArgs {
    /// Input file, or several images / a directory
    #[arg(required = true, value_name = "INPUT")]
    pub inputs: Vec<PathBuf>,

    /// Input type (detected from the file when omitted)
    #[arg(long = "type", value_enum, value_name = "TYPE")]
    pub kind: Option<InputKind>,

    #[command(flatten)]
    pub source: SourceArgs,

    /// Size limits used for the sheet estimates
    #[arg(long, value_enum, default_value = "compat")]
    pub preset: Preset,

    /// Print JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct GuideArgs {
    /// Topic to print; omit for the index, "all" for everything
    pub topic: Option<String>,
}
