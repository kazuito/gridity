use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use assert_cmd::Command;
use image::codecs::gif::GifEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::{Delay, Frame, Rgb, RgbImage, Rgba, RgbaImage};
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

fn gridity() -> Command {
    Command::cargo_bin("gridity").unwrap()
}

fn run_json(args: &[&str]) -> Value {
    let output = gridity().args(args).arg("--json").output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// Same rule as Claude's resizing check for the compat preset.
fn fits_compat(width: u32, height: u32) -> bool {
    width.div_ceil(28) * 28 <= 1568
        && height.div_ceil(28) * 28 <= 1568
        && width.div_ceil(28) * height.div_ceil(28) <= 1568
}

/// Writes a minimal PDF; each page is (width, height, rotate) in points.
fn write_pdf(path: &Path, pages: &[(u32, u32, u32)]) {
    let mut objects: Vec<Vec<u8>> =
        vec![b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec()];
    let pages_id = 1 + 2 * pages.len() + 1;
    let mut kids = Vec::new();
    for (i, &(w, h, rotate)) in pages.iter().enumerate() {
        let content = format!(
            "BT /F1 48 Tf 50 {} Td (Page {}) Tj ET\n0 0 1 rg 60 80 120 {} re f\n",
            h - 100,
            i + 1,
            40 + i * 20
        );
        objects.push(
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            )
            .into_bytes(),
        );
        let content_id = objects.len();
        objects.push(
            format!(
                "<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 {w} {h}] /Rotate {rotate} \
                 /Resources << /Font << /F1 1 0 R >> >> /Contents {content_id} 0 R >>"
            )
            .into_bytes(),
        );
        kids.push(format!("{} 0 R", objects.len()));
    }
    objects.push(
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            pages.len()
        )
        .into_bytes(),
    );
    assert_eq!(objects.len(), pages_id);
    objects.push(format!("<< /Type /Catalog /Pages {pages_id} 0 R >>").into_bytes());

    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n", i + 1).bytes());
        out.extend(object);
        out.extend(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for offset in offsets {
        out.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root {} 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1,
            objects.len()
        )
        .bytes(),
    );
    std::fs::write(path, out).unwrap();
}

fn write_png(path: &Path, width: u32, height: u32, shade: u8) {
    RgbImage::from_pixel(width, height, Rgb([shade, 100, 200]))
        .save(path)
        .unwrap();
}

/// JPEG with an EXIF APP1 segment carrying the given orientation tag.
fn write_exif_jpeg(path: &Path, width: u32, height: u32, orientation: u16) {
    let mut jpeg = Vec::new();
    JpegEncoder::new(&mut jpeg)
        .encode_image(&RgbImage::from_pixel(width, height, Rgb([200, 50, 50])))
        .unwrap();
    let mut tiff = b"MM\0*\0\0\0\x08\0\x01".to_vec();
    tiff.extend([0x01, 0x12, 0x00, 0x03, 0x00, 0x00, 0x00, 0x01]);
    tiff.extend(orientation.to_be_bytes());
    tiff.extend([0, 0, 0, 0, 0, 0]);
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend(tiff);
    let mut segment = vec![0xFF, 0xE1];
    segment.extend(((payload.len() + 2) as u16).to_be_bytes());
    segment.extend(payload);
    let mut out = jpeg[..2].to_vec();
    out.extend(segment);
    out.extend(&jpeg[2..]);
    std::fs::write(path, out).unwrap();
}

fn write_gif(path: &Path, frames: usize, delay_ms: u32) {
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = GifEncoder::new(file);
    for i in 0..frames {
        let image = RgbaImage::from_pixel(160, 120, Rgba([(i * 20) as u8, 80, 160, 255]));
        encoder
            .encode_frame(Frame::from_parts(
                image,
                0,
                0,
                Delay::from_numer_denom_ms(delay_ms, 1),
            ))
            .unwrap();
    }
}

fn have_ffmpeg() -> bool {
    StdCommand::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn write_video(path: &Path, extra_input_args: &[&str]) {
    let source = path.with_extension("src.mp4");
    let status = StdCommand::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=4:size=320x180:rate=10",
            "-c:v",
            "mpeg4",
        ])
        .arg(&source)
        .status()
        .unwrap();
    assert!(status.success());
    let status = StdCommand::new("ffmpeg")
        .args(["-v", "error", "-y"])
        .args(extra_input_args)
        .arg("-i")
        .arg(&source)
        .args(["-c", "copy"])
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

fn sheet_paths(manifest: &Value) -> Vec<PathBuf> {
    manifest["sheets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| PathBuf::from(s["path"].as_str().unwrap()))
        .collect()
}

#[test]
fn pdf_renders_evenly_split_sheets_within_limits() {
    let dir = TempDir::new().unwrap();
    let pdf = dir.path().join("doc.pdf");
    write_pdf(&pdf, &[(595, 842, 0); 6]);
    let out = dir.path().join("out");

    let manifest = run_json(&["render", s(&pdf), "-o", s(&out)]);
    let sheets = manifest["sheets"].as_array().unwrap();
    assert_eq!(sheets.len(), 2);
    assert_eq!(sheets[0]["cells"].as_array().unwrap().len(), 3);
    assert_eq!(sheets[1]["cells"][0]["page"], 4);
    assert_eq!(sheets[1]["cells"][0]["label"], "p.4");
    assert_eq!(manifest["source"]["pages"], 6);

    for (sheet, path) in sheets.iter().zip(sheet_paths(&manifest)) {
        assert!(path.is_absolute());
        assert!(path.starts_with(std::fs::canonicalize(&out).unwrap()) || path.starts_with(&out));
        let (w, h) = image::image_dimensions(&path).unwrap();
        assert_eq!(
            (w, h),
            (
                sheet["width"].as_u64().unwrap() as u32,
                sheet["height"].as_u64().unwrap() as u32
            )
        );
        assert!(fits_compat(w, h), "{w}x{h}");
    }
    assert!(out.join("doc.grid-01.png").exists());
}

#[test]
fn plain_output_is_one_absolute_path_per_line() {
    let dir = TempDir::new().unwrap();
    let pdf = dir.path().join("doc.pdf");
    write_pdf(&pdf, &[(595, 842, 0), (842, 595, 0), (595, 842, 90)]);
    let target = dir.path().join("sheet.png");

    let output = gridity()
        .args(["render", s(&pdf), "-o", s(&target)])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1);
    assert!(Path::new(lines[0]).is_absolute());
    assert!(target.exists());
}

#[test]
fn rotated_pages_are_landscape() {
    let dir = TempDir::new().unwrap();
    let pdf = dir.path().join("rot.pdf");
    write_pdf(&pdf, &[(595, 842, 90)]);
    let manifest = run_json(&["render", s(&pdf), "--dry-run"]);
    let rect = &manifest["sheets"][0]["cells"][0]["rect"];
    assert!(rect[2].as_u64() > rect[3].as_u64(), "rect {rect}");
}

#[test]
fn page_out_of_range_reports_json_error() {
    let dir = TempDir::new().unwrap();
    let pdf = dir.path().join("doc.pdf");
    write_pdf(&pdf, &[(595, 842, 0); 3]);

    let output = gridity()
        .args(["render", s(&pdf), "--pages", "2-5", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["kind"], "usage");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("3 pages")
    );
}

#[test]
fn too_many_sheets_fails_with_hint() {
    let dir = TempDir::new().unwrap();
    let pdf = dir.path().join("doc.pdf");
    write_pdf(&pdf, &[(595, 842, 0); 12]);
    gridity()
        .args([
            "render",
            s(&pdf),
            "--detail",
            "high",
            "--max-sheets",
            "3",
            "--dry-run",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--max-sheets 6"));
}

#[test]
fn flags_for_another_input_type_are_rejected() {
    let dir = TempDir::new().unwrap();
    let pdf = dir.path().join("doc.pdf");
    write_pdf(&pdf, &[(595, 842, 0)]);
    gridity()
        .args(["render", s(&pdf), "--every", "5s"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "--every cannot be used with pdf input",
        ));
    gridity()
        .args(["render", s(&pdf), "--label", "{time}"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("{page}"));
}

#[test]
fn missing_and_unknown_inputs_exit_3() {
    let dir = TempDir::new().unwrap();
    gridity()
        .args(["render", "/definitely/missing.pdf"])
        .assert()
        .code(3);
    let text = dir.path().join("notes.xyz");
    std::fs::write(&text, "hello").unwrap();
    gridity()
        .args(["render", s(&text)])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--type"));
}

#[test]
fn dry_run_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let pdf = dir.path().join("doc.pdf");
    write_pdf(&pdf, &[(595, 842, 0); 2]);
    let out = dir.path().join("out");
    gridity()
        .args(["render", s(&pdf), "-o", s(&out), "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dry run: 2 items on 1 sheet(s)"));
    assert!(!out.exists());
}

#[test]
fn images_apply_exif_orientation_and_natural_order() {
    let dir = TempDir::new().unwrap();
    let shots = dir.path().join("shots");
    std::fs::create_dir(&shots).unwrap();
    write_exif_jpeg(&shots.join("photo10.jpg"), 400, 200, 6);
    write_png(&shots.join("photo2.png"), 400, 200, 10);
    std::fs::write(shots.join("readme.txt"), "not an image").unwrap();

    let probe = run_json(&["probe", s(&shots)]);
    let files = probe["source"]["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert!(files[0]["path"].as_str().unwrap().ends_with("photo2.png"));
    assert_eq!(
        (files[1]["width"].as_u64(), files[1]["height"].as_u64()),
        (Some(200), Some(400))
    );

    let manifest = run_json(&["render", s(&shots), "-o", s(&dir.path().join("out"))]);
    let cells = manifest["sheets"][0]["cells"].as_array().unwrap();
    assert_eq!(cells[0]["label"], "photo2.png");
    let rotated = &cells[1]["rect"];
    assert!(
        rotated[3].as_u64() > rotated[2].as_u64(),
        "EXIF-rotated image should be portrait: {rotated}"
    );
}

#[test]
fn small_images_are_not_upscaled() {
    let dir = TempDir::new().unwrap();
    let a = dir.path().join("a.png");
    let b = dir.path().join("b.png");
    write_png(&a, 120, 90, 0);
    write_png(&b, 120, 90, 200);
    let manifest = run_json(&["render", s(&a), s(&b), "--dry-run"]);
    let grid = &manifest["sheets"][0]["grid"];
    assert_eq!(
        (grid["cell_width"].as_u64(), grid["cell_height"].as_u64()),
        (Some(120), Some(90))
    );
}

#[test]
fn gif_frames_follow_requested_times() {
    let dir = TempDir::new().unwrap();
    let gif = dir.path().join("anim.gif");
    write_gif(&gif, 10, 100);

    let probe = run_json(&["probe", s(&gif)]);
    assert_eq!(probe["source"]["frames"], 10);
    assert_eq!(probe["source"]["duration"], 1.0);

    let manifest = run_json(&["render", s(&gif), "--at", "0,0.25,0.95", "--dry-run"]);
    let cells = manifest["sheets"][0]["cells"].as_array().unwrap();
    let frames: Vec<u64> = cells.iter().map(|c| c["frame"].as_u64().unwrap()).collect();
    assert_eq!(frames, vec![1, 3, 10]);
    assert_eq!(cells[1]["label"], "0:00.2");

    let manifest = run_json(&[
        "render",
        s(&gif),
        "--count",
        "40",
        "-o",
        s(&dir.path().join("out")),
    ]);
    assert_eq!(
        manifest["sheets"][0]["cells"].as_array().unwrap().len(),
        10,
        "duplicates are merged"
    );
    gridity()
        .args(["render", s(&gif), "--pages", "1"])
        .assert()
        .code(2);
}

#[test]
fn video_sampling_and_rotation() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let video = dir.path().join("clip.mp4");
    write_video(&video, &[]);

    let manifest = run_json(&[
        "render",
        s(&video),
        "--every",
        "1s",
        "-o",
        s(&dir.path().join("out")),
    ]);
    let cells = manifest["sheets"][0]["cells"].as_array().unwrap();
    let times: Vec<f64> = cells.iter().map(|c| c["time"].as_f64().unwrap()).collect();
    assert_eq!(times, vec![0.0, 1.0, 2.0, 3.0]);
    assert_eq!(cells[2]["label"], "0:02");
    assert!(manifest["warnings"].as_array().unwrap().is_empty());
    let path = &sheet_paths(&manifest)[0];
    assert_eq!(path.extension().unwrap(), "jpg");
    let (w, h) = image::image_dimensions(path).unwrap();
    assert!(fits_compat(w, h));

    let rotated = dir.path().join("rotated.mp4");
    write_video(&rotated, &["-display_rotation", "90"]);
    let probe = run_json(&["probe", s(&rotated)]);
    assert_eq!(
        (
            probe["source"]["width"].as_u64(),
            probe["source"]["height"].as_u64()
        ),
        (Some(180), Some(320))
    );

    gridity()
        .args(["render", s(&video), "--end", "9"])
        .assert()
        .code(2);
}

#[test]
fn missing_ffmpeg_exits_4() {
    let dir = TempDir::new().unwrap();
    let video = dir.path().join("clip.mp4");
    std::fs::write(&video, b"\0\0\0\x18ftypmp42").unwrap();
    gridity()
        .args(["render", s(&video), "--ffmpeg", "/nonexistent/ffmpeg"])
        .env("GRIDITY_FFPROBE", "/nonexistent/ffprobe")
        .assert()
        .code(4)
        .stderr(predicate::str::contains("hint:"));
}

#[test]
fn guide_topics() {
    gridity()
        .arg("guide")
        .assert()
        .success()
        .stdout(predicate::str::contains("gridity guide recipes"));
    gridity()
        .args(["guide", "output"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rect"));
    gridity().args(["guide", "nope"]).assert().code(2);
    gridity()
        .args(["render", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("gridity guide"));
}

#[test]
fn argument_errors_are_json_with_json_flag() {
    let output = gridity()
        .args(["render", "x.pdf", "--every", "1", "--count", "3", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["kind"], "usage");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("--every")
    );
    gridity()
        .args(["render", "--help", "--json"])
        .assert()
        .success();
    gridity().arg("--version").assert().success();
}

#[test]
fn encrypted_pdf_needs_the_password() {
    let pdf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/encrypted.pdf");
    gridity()
        .args(["render", s(&pdf), "--dry-run"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--password"));
    gridity()
        .args(["render", s(&pdf), "--dry-run", "--password", "wrong"])
        .assert()
        .code(3);
    let dir = TempDir::new().unwrap();
    let manifest = run_json(&[
        "render",
        s(&pdf),
        "--password",
        "secret",
        "-o",
        s(&dir.path().join("out")),
    ]);
    assert_eq!(manifest["source"]["pages"], 1);
}
