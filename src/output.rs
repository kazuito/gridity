use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageFormat, RgbaImage};
use schemars::JsonSchema;
use serde::Serialize;

use crate::cli::Format;
use crate::error::{CliError, ErrorKind, Reason};
use crate::layout::Grid;

// Image limits from https://platform.claude.com/docs/en/build-with-claude/vision (checked 2026-10-22).
/// Amazon Bedrock and Google Cloud reject images whose base64 encoding is larger (the API allows 10 MB).
pub const MAX_BASE64_BYTES: usize = 5_000_000;
/// Longest edge allowed for every image of a request that holds more than 20 images.
pub const MANY_IMAGES_MAX_EDGE: u32 = 2000;

pub fn base64_len(bytes: usize) -> usize {
    bytes.div_ceil(3) * 4
}

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
    /// Without `-o`, sheets go to `$TMPDIR/gridity-<uid>/<stem>-<hash>/`.
    pub fn resolve(output: Option<&Path>, stem: &str, fingerprint: &str) -> Result<Target> {
        let stem = sanitize(stem);
        let target = match output {
            Some(path) if format_from_extension(path).is_some() => Target::file(path, stem),
            Some(dir) => Target {
                dir: dir.to_path_buf(),
                base: format!("{stem}.grid"),
                managed: false,
            },
            None => Target {
                dir: default_root().join(format!("{stem}-{:016x}", fnv1a(fingerprint))),
                base: format!("{stem}.grid"),
                managed: true,
            },
        };
        Ok(target)
    }

    /// For `export`: `-o` with an image extension is a file name, numbered for several items;
    /// otherwise files named per item go to `-o` or `$TMPDIR/gridity-<uid>/<stem>-export-<hash>/`.
    pub fn resolve_export(output: Option<&Path>, stem: &str, fingerprint: &str) -> Target {
        let stem = sanitize(stem);
        match output {
            Some(path) if format_from_extension(path).is_some() => Target::file(path, stem),
            Some(dir) => Target {
                dir: dir.to_path_buf(),
                base: String::new(),
                managed: false,
            },
            None => Target {
                dir: default_root().join(format!("{stem}-export-{:016x}", fnv1a(fingerprint))),
                base: String::new(),
                managed: true,
            },
        }
    }

    fn file(path: &Path, fallback_stem: String) -> Target {
        Target {
            dir: path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
                .to_path_buf(),
            base: path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or(fallback_stem),
            managed: false,
        }
    }

    /// `name` (already a safe file name) with the format's extension, inside the directory.
    pub fn named_path(&self, name: &str, format: Format) -> PathBuf {
        absolute(&self.dir.join(format!("{name}.{}", extension(format))))
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
        if self.managed {
            if let Some(root) = self.dir.parent() {
                create_private_dir(root)?;
            }
            return create_private_dir(&self.dir);
        }
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
        let files = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.path());
        for path in files {
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

/// The default output root belongs to the current user, so other users can neither read the
/// sheets nor plant files or links where gridity writes.
fn default_root() -> PathBuf {
    #[cfg(unix)]
    let name = format!("gridity-{}", current_uid());
    #[cfg(not(unix))]
    let name = "gridity".to_string();
    std::env::temp_dir().join(name)
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
            return Err(e).with_context(|| format!("failed to create {}", dir.display()));
        }
        _ => {}
    }
    let meta = std::fs::symlink_metadata(dir)
        .with_context(|| format!("failed to inspect {}", dir.display()))?;
    if !meta.is_dir() || meta.uid() != current_uid() || meta.mode() & 0o077 != 0 {
        return Err(CliError::new(
            ErrorKind::Runtime,
            Reason::UnsafeOutputDir,
            format!(
                "{} is not a private directory owned by the current user",
                dir.display()
            ),
        )
        .with_hint("remove it, or pass -o to choose where sheets are written")
        .into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))
}

/// Writes through a temporary file and a rename: readers never see a half-written sheet, and a
/// link already at `path` is replaced instead of followed.
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&temp);
    let result = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .and_then(|mut file| file.write_all(data))
        .and_then(|()| std::fs::rename(&temp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.with_context(|| format!("failed to write {}", path.display()))
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

/// A file name made from user text: no path separators, no leading dot, never empty.
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned = sanitize(name);
    let visible = cleaned.trim_start_matches('.');
    if visible.is_empty() {
        "gridity".to_string()
    } else {
        visible.to_string()
    }
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

/// Refuses to write `path` when it is one of `inputs` (canonical paths), so an output never
/// replaces its own source.
pub fn check_not_input(path: &Path, inputs: &[PathBuf]) -> Result<(), CliError> {
    match std::fs::canonicalize(path) {
        Ok(existing) if inputs.contains(&existing) => Err(CliError::new(
            ErrorKind::Usage,
            Reason::InvalidValue,
            format!("{} is an input and would be overwritten", path.display()),
        )
        .with_hint("write to another -o directory or file, or change --name")),
        _ => Ok(()),
    }
}

/// Stable 64-bit FNV-1a hash, used to name output directories.
pub fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ byte as u64).wrapping_mul(0x100000001b3)
    })
}

/// Version of every JSON document gridity prints. Adding fields keeps it; removing, renaming
/// or retyping a field raises it.
pub const SCHEMA_VERSION: u32 = 2;

/// `render --json`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Manifest {
    pub schema_version: u32,
    pub source: SourceInfo,
    pub limits: LimitsInfo,
    pub sheets: Vec<SheetInfo>,
    pub warnings: Vec<String>,
}

/// The input, by kind. `page_sizes` and `files` are only reported by `probe`.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SourceInfo {
    Pdf {
        path: String,
        pages: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        page_sizes: Option<Vec<PageSizeGroup>>,
    },
    Video {
        path: String,
        duration: Option<f64>,
        width: u32,
        height: u32,
        coded_width: u32,
        coded_height: u32,
        rotation: i32,
        fps: Option<f64>,
        codec: Option<String>,
    },
    Anim {
        path: String,
        frames: usize,
        duration: f64,
        width: u32,
        height: u32,
    },
    Images {
        name: String,
        count: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        files: Option<Vec<FileInfo>>,
    },
}

/// Consecutive or scattered pages sharing one size, e.g. 595x842 pt on pages "1-12".
#[derive(Debug, Serialize, JsonSchema)]
pub struct PageSizeGroup {
    pub width_pt: u32,
    pub height_pt: u32,
    pub orientation: String,
    pub count: usize,
    pub pages: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FileInfo {
    pub path: String,
    pub width: u32,
    pub height: u32,
}

/// `{"error": ...}` printed on stdout with `--json` when a command fails.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ErrorReport {
    pub schema_version: u32,
    pub error: ErrorInfo,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ErrorInfo {
    /// runtime, usage, input or dependency.
    pub kind: String,
    /// Exit code.
    pub code: i32,
    /// Machine-readable cause, e.g. page_out_of_range.
    pub reason: String,
    pub message: String,
    pub hint: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct LimitsInfo {
    pub preset: String,
    /// How sizes and tokens are counted, e.g. claude-28px-patch.
    pub rule: String,
    pub max_edge: u32,
    pub max_tokens: u32,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SheetInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub width: u32,
    pub height: u32,
    /// Visual tokens under the preset's rule (`limits.rule`).
    pub est_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<usize>,
    pub grid: Grid,
    pub cells: Vec<CellInfo>,
}

/// `export --json`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ExportManifest {
    pub schema_version: u32,
    pub source: SourceInfo,
    pub files: Vec<ExportedFile>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ExportedFile {
    pub index: usize,
    /// The written file; with --dry-run the file that would be written. Absent when the item
    /// could not be decoded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    pub width: u32,
    pub height: u32,
    /// Pixels per PDF point, or written / native pixels for raster sources.
    pub scale: f64,
    /// Exported region as [x, y, width, height] fractions of the page or frame.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crop: Option<[f64; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<usize>,
    /// Why the item could not be decoded; no file is written for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
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
    /// Sheet pixels per PDF point, or displayed / native pixels for raster sources.
    pub scale: f64,
    /// Shown region as [x, y, width, height] fractions of the page or frame.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crop: Option<[f64; 4]>,
    /// Why the item could not be decoded; the cell then holds a gray placeholder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
    fn default_target_is_per_user_with_full_hash() {
        let path = Target::resolve(None, "doc", "x")
            .unwrap()
            .sheet_path(0, 1, Format::Png);
        let dir = path.parent().unwrap();
        let hash = format!("{:016x}", fnv1a("x"));
        assert_eq!(
            dir.file_name().unwrap().to_str(),
            Some(&*format!("doc-{hash}"))
        );
        #[cfg(unix)]
        assert_eq!(
            dir.parent().unwrap().file_name().unwrap().to_str(),
            Some(&*format!("gridity-{}", current_uid()))
        );
    }

    #[cfg(unix)]
    #[test]
    fn writes_replace_links_instead_of_following_them() {
        let dir = tempfile::TempDir::new().unwrap();
        let victim = dir.path().join("victim.txt");
        std::fs::write(&victim, "keep").unwrap();
        let sheet = dir.path().join("doc.grid.png");
        std::os::unix::fs::symlink(&victim, &sheet).unwrap();

        write_atomic(&sheet, b"sheet").unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
        assert!(!std::fs::symlink_metadata(&sheet).unwrap().is_symlink());
        assert_eq!(std::fs::read(&sheet).unwrap(), b"sheet");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn shared_directories_are_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let shared = dir.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(create_private_dir(&shared).is_err());

        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&shared, &link).unwrap();
        assert!(create_private_dir(&link).is_err());

        let fresh = dir.path().join("fresh");
        create_private_dir(&fresh).unwrap();
        let mode = std::fs::metadata(&fresh).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    /// JSON Schemas of every document gridity prints, by name; committed as
    /// `docs/schema/<name>.schema.json`.
    fn schemas() -> Vec<(&'static str, schemars::Schema)> {
        vec![
            ("render", schemars::schema_for!(Manifest)),
            ("export", schemars::schema_for!(ExportManifest)),
            ("probe", schemars::schema_for!(crate::probe::ProbeReport)),
            ("error", schemars::schema_for!(ErrorReport)),
        ]
    }

    /// Regenerate with `GRIDITY_UPDATE_SCHEMA=1 cargo test committed_schemas`.
    #[test]
    fn committed_schemas_match_the_code() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/schema");
        let update = std::env::var_os("GRIDITY_UPDATE_SCHEMA").is_some();
        for (name, schema) in schemas() {
            let path = dir.join(format!("{name}.schema.json"));
            let generated = serde_json::to_string_pretty(&schema).unwrap() + "\n";
            if update {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(&path, &generated).unwrap();
            }
            let committed = std::fs::read_to_string(&path).unwrap_or_default();
            assert!(
                committed == generated,
                "{} is out of date; run GRIDITY_UPDATE_SCHEMA=1 cargo test committed_schemas",
                path.display()
            );
        }
    }

    #[test]
    fn base64_size() {
        assert_eq!(base64_len(0), 0);
        assert_eq!(base64_len(1), 4);
        assert_eq!(base64_len(3), 4);
        assert_eq!(base64_len(3_750_000), 5_000_000);
        assert_eq!(base64_len(3_750_001), 5_000_004);
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
