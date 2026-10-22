use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::cli::InputKind;
use crate::error::{Reason, input, usage};
use crate::source::images::has_image_extension;

const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "flv", "mpg", "mpeg", "ts", "m2ts", "mts",
    "3gp", "ogv",
];

/// Decides the input type from `--type`, the number of inputs, magic bytes and extensions.
pub fn detect(inputs: &[PathBuf], forced: Option<InputKind>) -> Result<InputKind> {
    for path in inputs {
        if !path.exists() {
            return Err(input(
                Reason::NotFound,
                format!("{} does not exist", path.display()),
            )
            .into());
        }
    }
    let several = inputs.len() > 1 || inputs[0].is_dir();

    if let Some(kind) = forced {
        if several && kind != InputKind::Images {
            return Err(usage(
                Reason::InvalidArgument,
                "several inputs or a directory can only be used with images",
            )
            .into());
        }
        return Ok(kind);
    }
    if several {
        for path in inputs.iter().filter(|p| p.is_file()) {
            if !has_image_extension(path) {
                return Err(usage(Reason::InvalidArgument, format!(
                    "{} is not an image; several inputs or a directory are only supported for images",
                    path.display()
                ))
                .into());
            }
        }
        return Ok(InputKind::Images);
    }

    let path = &inputs[0];
    let head = read_head(path, 1024)?;
    if head.starts_with(PNG_SIGNATURE) && is_apng(path)? {
        return Ok(InputKind::Anim);
    }
    if let Some(kind) = kind_from_head(&head) {
        return Ok(kind);
    }
    if has_extension(path, VIDEO_EXTENSIONS) {
        return Ok(InputKind::Video);
    }
    if has_image_extension(path) {
        return Ok(InputKind::Images);
    }
    Err(input(
        Reason::UnknownType,
        format!("cannot detect the type of {}", path.display()),
    )
    .with_hint("pass --type pdf|video|anim|images")
    .into())
}

const PNG_SIGNATURE: &[u8] = &[0x89, b'P', b'N', b'G'];

/// Signatures at the start of the file win; a PDF header may also appear anywhere in the
/// first 1024 bytes, so that search comes last and cannot claim a JPEG mentioning "%PDF-".
fn kind_from_head(head: &[u8]) -> Option<InputKind> {
    if head.starts_with(b"%PDF-") {
        return Some(InputKind::Pdf);
    }
    if head.starts_with(b"GIF8") {
        return Some(InputKind::Anim);
    }
    let is_image_magic = head.starts_with(PNG_SIGNATURE)
        || head.starts_with(&[0xFF, 0xD8, 0xFF])
        || (head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP"))
        || head.starts_with(b"BM")
        || head.starts_with(b"II*\0")
        || head.starts_with(b"MM\0*");
    if is_image_magic {
        return Some(InputKind::Images);
    }
    let is_video_magic = head.get(4..8) == Some(b"ftyp")
        || head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3])
        || (head.starts_with(b"RIFF") && head.get(8..11) == Some(b"AVI"))
        || head.starts_with(b"OggS");
    if is_video_magic {
        return Some(InputKind::Video);
    }
    if head.windows(5).any(|w| w == b"%PDF-") {
        return Some(InputKind::Pdf);
    }
    None
}

fn read_head(path: &Path, len: u64) -> Result<Vec<u8>> {
    let mut head = Vec::new();
    File::open(path)
        .and_then(|f| f.take(len).read_to_end(&mut head))
        .with_context(|| format!("failed to read {}", path.display()))?;
    Ok(head)
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| extensions.contains(&e.to_ascii_lowercase().as_str()))
}

/// An APNG declares an `acTL` chunk before its first `IDAT`.
fn is_apng(path: &Path) -> Result<bool> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut signature = [0u8; 8];
    reader.read_exact(&mut signature)?;
    loop {
        let mut header = [0u8; 8];
        if reader.read_exact(&mut header).is_err() {
            return Ok(false);
        }
        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as u64;
        match &header[4..8] {
            b"acTL" => return Ok(true),
            b"IDAT" | b"IEND" => return Ok(false),
            _ => {
                std::io::copy(&mut (&mut reader).take(length + 4), &mut std::io::sink())?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leading_signatures_win_over_a_pdf_mention() {
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xFE, 0x00, 0x12];
        jpeg.extend(b"%PDF-1.4 mention");
        assert_eq!(kind_from_head(&jpeg), Some(InputKind::Images));

        let mut mp4 = b"\0\0\0\x18ftypisom".to_vec();
        mp4.extend(b"%PDF-1.7");
        assert_eq!(kind_from_head(&mp4), Some(InputKind::Video));

        let mut pdf = vec![b'x'; 100];
        pdf.extend(b"%PDF-1.7\n");
        assert_eq!(kind_from_head(&pdf), Some(InputKind::Pdf));
        assert_eq!(kind_from_head(b"%PDF-2.0"), Some(InputKind::Pdf));
        assert_eq!(kind_from_head(b"hello"), None);
    }

    proptest::proptest! {
        #[test]
        fn sniffing_never_panics(head in proptest::collection::vec(proptest::num::u8::ANY, 0..64)) {
            let _ = kind_from_head(&head);
        }

        #[test]
        fn apng_scan_never_panics(chunks in proptest::collection::vec(proptest::num::u8::ANY, 0..256)) {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("x.png");
            let mut bytes = PNG_SIGNATURE.to_vec();
            bytes.extend([0x0D, 0x0A, 0x1A, 0x0A]);
            bytes.extend(chunks);
            std::fs::write(&path, bytes).unwrap();
            let _ = is_apng(&path);
        }
    }
}
