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

/// Runs a command expected to fail with exit 2 and returns its JSON `error` object.
fn run_error_json(args: &[&str]) -> Value {
    let output = gridity().args(args).arg("--json").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    body["error"].clone()
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
    let pages: Vec<(u32, u32, u32, String)> = pages
        .iter()
        .enumerate()
        .map(|(i, &(w, h, rotate))| {
            let content = format!(
                "BT /F1 48 Tf 50 {} Td (Page {}) Tj ET\n0 0 1 rg 60 80 120 {} re f\n",
                h - 100,
                i + 1,
                40 + i * 20
            );
            (w, h, rotate, content)
        })
        .collect();
    write_pdf_pages(path, &pages);
}

/// Writes a PDF whose pages are (width, height, rotate, content stream) with Helvetica as /F1.
fn write_pdf_pages(path: &Path, pages: &[(u32, u32, u32, String)]) {
    let mut objects: Vec<Vec<u8>> =
        vec![b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec()];
    let pages_id = 1 + 2 * pages.len() + 1;
    let mut kids = Vec::new();
    for (w, h, rotate, content) in pages {
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

/// A page of 11pt body text lines, plus a 12pt word that is clipped away.
fn text_page(lines: usize) -> String {
    text_page_sized(lines, 11)
}

fn text_page_sized(lines: usize, size: usize) -> String {
    let mut content = String::new();
    for i in 0..lines {
        content.push_str(&format!(
            "BT /F1 {size} Tf 50 {} Td (Line {i} of body text here) Tj ET\n",
            780 - i * (size + 3)
        ));
    }
    content.push_str("q 0 0 40 40 re W n BT /F1 12 Tf 300 300 Td (Clipped) Tj ET Q\n");
    content
}

fn write_png(path: &Path, width: u32, height: u32, shade: u8) {
    RgbImage::from_pixel(width, height, Rgb([shade, 100, 200]))
        .save(path)
        .unwrap();
}

/// Random pixels, so the PNG barely compresses.
fn write_noise_png(path: &Path, width: u32, height: u32) {
    let mut state: u32 = 0x1234_5678;
    RgbImage::from_fn(width, height, |_, _| {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let [r, g, b, _] = state.to_be_bytes();
        Rgb([r, g, b])
    })
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
fn probe_scale_matches_rendered_cells() {
    let dir = TempDir::new().unwrap();
    let pdf = dir.path().join("doc.pdf");
    write_pdf(&pdf, &[(595, 842, 0); 4]);

    let probe = run_json(&["probe", s(&pdf)]);
    let estimates = probe["estimates"].as_array().unwrap();
    let scales: Vec<f64> = estimates
        .iter()
        .map(|e| e["scale"].as_f64().unwrap())
        .collect();
    assert!(
        scales[0] <= scales[1] && scales[1] < scales[2],
        "{scales:?}"
    );
    let medium = &estimates[1];
    let text_px = medium["text_12pt_px"].as_f64().unwrap();
    assert!((text_px - 12.0 * scales[1]).abs() < 0.1);

    let manifest = run_json(&["render", s(&pdf), "--dry-run"]);
    for cell in manifest["sheets"][0]["cells"].as_array().unwrap() {
        let width = cell["rect"][2].as_f64().unwrap();
        assert!((cell["scale"].as_f64().unwrap() - width / 595.0).abs() < 0.001);
        assert!((cell["scale"].as_f64().unwrap() - scales[1]).abs() < 0.001);
    }
}

#[test]
fn text_prints_visible_text_per_page() {
    let dir = TempDir::new().unwrap();
    let pdf = dir.path().join("doc.pdf");
    write_pdf_pages(
        &pdf,
        &[
            (595, 842, 0, text_page(10)),
            (595, 842, 0, "0 0 1 rg 50 50 400 500 re f\n".to_string()),
        ],
    );

    let output = gridity().args(["text", s(&pdf)]).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("--- p.1 · text · "), "{stdout}");
    assert!(stdout.contains("Line 0 of body text here\nLine 1 of body text here"));
    assert!(!stdout.contains("Clipped"));
    assert!(stdout.contains("--- p.2 · graphics · 0 chars"));

    let report = run_json(&["text", s(&pdf), "--pages", "1"]);
    let page = &report["pages"][0];
    assert_eq!(page["page"], 1);
    assert_eq!(page["content"], "text");
    assert_eq!(page["body_pt"], 11.0);
    assert_eq!(report["pages"].as_array().unwrap().len(), 1);
}

/// A page of `size` pt text with `count` superscripts: "x^{2}" repeated, as formulas are.
fn formula_page(size: usize, count: usize) -> String {
    let mut content = text_page_sized(6, size);
    for i in 0..count {
        content.push_str(&format!(
            "BT 0 Ts /F1 {size} Tf {} 300 Td (x) Tj /F1 {} Tf {} Ts (2) Tj ET\n",
            50 + i * 4 * size,
            size * 6 / 10,
            size * 4 / 10
        ));
    }
    content
}

#[test]
fn text_marks_scripts() {
    let dir = TempDir::new().unwrap();
    let pdf = dir.path().join("doc.pdf");
    write_pdf_pages(&pdf, &[(595, 842, 0, formula_page(14, 6))]);

    let output = gridity().args(["text", s(&pdf)]).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("x^2"), "{stdout}");
    assert!(
        stdout.lines().next().unwrap().contains("formulas"),
        "{stdout}"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("p.1 show formulas"), "{stderr}");
}

#[test]
fn suggested_close_ups_render_for_papers_with_footnotes() {
    let dir = TempDir::new().unwrap();
    let pdf = dir.path().join("paper.pdf");
    let page = format!(
        "{}BT /F1 4 Tf 50 30 Td (footnote set in very small type, forty chars) Tj ET\n\
         0 0 1 rg 50 50 400 300 re f\n",
        formula_page(11, 6)
    );
    write_pdf_pages(&pdf, &vec![(595, 842, 0, page); 24]);

    let probe = run_json(&["probe", s(&pdf), "--per-page"]);
    let renders: Vec<&str> = probe["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap())
        .filter(|s| s.starts_with("gridity render"))
        .collect();
    assert!(renders.len() >= 2, "{renders:?}");
    for render in renders {
        let args: Vec<&str> = render.split_whitespace().skip(1).collect();
        gridity().args(&args).arg("--dry-run").assert().success();
    }
}

#[test]
fn probe_per_page_suggests_text_and_visual_pages() {
    let dir = TempDir::new().unwrap();
    let pdf = dir.path().join("doc.pdf");
    let figure = format!("{}0 0 1 rg 50 50 400 300 re f\n", text_page(10));
    write_pdf_pages(
        &pdf,
        &[
            (595, 842, 0, text_page(10)),
            (595, 842, 0, figure),
            (595, 842, 0, text_page(10)),
        ],
    );

    let probe = run_json(&["probe", s(&pdf), "--per-page"]);
    assert_eq!(probe["body_text_pt"], 11.0);
    let contents: Vec<&str> = probe["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["content"].as_str().unwrap())
        .collect();
    assert_eq!(contents, ["text", "mixed", "text"]);
    let medium = &probe["estimates"][1];
    let body_px = medium["body_text_px"].as_f64().unwrap();
    assert!((body_px - 11.0 * medium["scale"].as_f64().unwrap()).abs() < 0.1);
    let suggestions: Vec<&str> = probe["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap())
        .collect();
    assert!(suggestions[0].starts_with("gridity text "));
    assert!(
        suggestions.iter().any(|s| s.contains("--pages 2")),
        "{suggestions:?}"
    );
}

#[test]
fn suggested_detail_keeps_body_text_readable_on_rendered_pages() {
    let dir = TempDir::new().unwrap();
    let pdf = dir.path().join("doc.pdf");
    let figure = format!("{}0 0 1 rg 50 50 400 300 re f\n", text_page_sized(10, 14));
    let mut pages = vec![(595, 842, 0, text_page_sized(10, 14))];
    pages.extend(std::iter::repeat_n((595, 842, 0, figure), 9));
    write_pdf_pages(&pdf, &pages);

    let probe = run_json(&["probe", s(&pdf), "--per-page"]);
    let low = &probe["estimates"][0];
    assert!(
        low["visual_scale"].as_f64() < low["scale"].as_f64(),
        "fixture should plan the 9 visual pages tighter than all 10: {low}"
    );
    let render = probe["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap())
        .find(|s| s.starts_with("gridity render") && s.contains("--pages 2-10"))
        .expect("a render suggestion for the visual pages");
    let args: Vec<&str> = render.split_whitespace().skip(1).collect();
    let manifest = run_json(&[&args[..], &["--dry-run"]].concat());
    for cell in manifest["sheets"][0]["cells"].as_array().unwrap() {
        let body_px = 14.0 * cell["scale"].as_f64().unwrap();
        assert!(body_px >= 8.0, "{render}: body text at {body_px:.1}px");
    }
}

#[test]
fn image_only_pdf_suggests_no_text_extraction() {
    let dir = TempDir::new().unwrap();
    let pdf = dir.path().join("scan.pdf");
    write_pdf_pages(
        &pdf,
        &[(595, 842, 0, "0 0 1 rg 50 50 400 500 re f\n".into())],
    );
    let probe = run_json(&["probe", s(&pdf), "--per-page"]);
    let suggestions = probe["suggestions"].as_array().unwrap();
    assert!(
        suggestions
            .iter()
            .all(|s| !s.as_str().unwrap().starts_with("gridity text")),
        "{suggestions:?}"
    );
    assert!(probe.get("body_text_pt").is_none());
}

#[test]
fn text_and_per_page_reject_other_inputs() {
    let dir = TempDir::new().unwrap();
    let png = dir.path().join("a.png");
    write_png(&png, 40, 30, 0);
    gridity()
        .args(["text", s(&png)])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("not a PDF"));
    gridity()
        .args(["probe", s(&png), "--per-page"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "--per-page works with PDF input only",
        ));
}

#[test]
fn crop_shows_part_of_each_page_at_matching_size() {
    let dir = TempDir::new().unwrap();
    let pdf = dir.path().join("doc.pdf");
    write_pdf(&pdf, &[(595, 842, 0), (595, 842, 90)]);
    let args = ["render", s(&pdf), "--crop", "0,0.5,1,0.5"];

    let planned = run_json(&[&args[..], &["--dry-run"]].concat());
    let manifest = run_json(&[&args[..], &["-o", s(&dir.path().join("out"))]].concat());
    assert_eq!(
        planned["sheets"][0]["cells"],
        manifest["sheets"][0]["cells"]
    );
    let cells = manifest["sheets"][0]["cells"].as_array().unwrap();
    assert_eq!(cells[0]["crop"], serde_json::json!([0.0, 0.5, 1.0, 0.5]));
    let rect = |i: usize| {
        let r = &cells[i]["rect"];
        r[2].as_f64().unwrap() / r[3].as_f64().unwrap()
    };
    assert!((rect(0) - 595.0 / 421.0).abs() < 0.02, "{}", rect(0));
    assert!((rect(1) - 842.0 / 297.5).abs() < 0.05, "{}", rect(1));
    let scale = cells[0]["scale"].as_f64().unwrap();
    assert!((scale - cells[0]["rect"][2].as_f64().unwrap() / 595.0).abs() < 0.001);
    let path = manifest["sheets"][0]["path"].as_str().unwrap();
    let (w, h) = image::image_dimensions(path).unwrap();
    assert_eq!(
        (w as u64, h as u64),
        (
            manifest["sheets"][0]["width"].as_u64().unwrap(),
            manifest["sheets"][0]["height"].as_u64().unwrap()
        )
    );

    let other = run_json(&["render", s(&pdf), "--crop", "0,0,1,0.5"]);
    assert_ne!(other["sheets"][0]["path"], manifest["sheets"][0]["path"]);
}

#[test]
fn crop_keeps_raster_inputs_at_native_size() {
    let dir = TempDir::new().unwrap();
    let png = dir.path().join("a.png");
    write_png(&png, 120, 90, 0);
    let jpg = dir.path().join("b.jpg");
    write_exif_jpeg(&jpg, 400, 200, 6);

    let manifest = run_json(&["render", s(&png), "--crop", "0,0,0.5,1", "--dry-run"]);
    let rect = &manifest["sheets"][0]["cells"][0]["rect"];
    assert_eq!((rect[2].as_u64(), rect[3].as_u64()), (Some(60), Some(90)));

    let manifest = run_json(&["render", s(&jpg), "--crop", "0,0,1,0.25", "--dry-run"]);
    let rect = &manifest["sheets"][0]["cells"][0]["rect"];
    assert_eq!((rect[2].as_u64(), rect[3].as_u64()), (Some(200), Some(100)));

    let gif = dir.path().join("anim.gif");
    write_gif(&gif, 4, 100);
    let manifest = run_json(&[
        "render",
        s(&gif),
        "--crop",
        "0.5,0.5,0.5,0.5",
        "-o",
        s(&dir.path().join("gif")),
    ]);
    let sheet = &manifest["sheets"][0];
    assert_eq!(sheet["cells"].as_array().unwrap().len(), 4);
    assert_eq!(sheet["cells"][0]["rect"][2], 80);
}

#[test]
fn invalid_crop_is_a_usage_error() {
    let dir = TempDir::new().unwrap();
    let png = dir.path().join("a.png");
    write_png(&png, 40, 30, 0);
    gridity()
        .args(["render", s(&png), "--crop", "0.5,0,0.6,1"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("extends past the edge"));
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
    assert_eq!(error["schema_version"], 1);
    assert_eq!(error["error"]["kind"], "usage");
    assert_eq!(error["error"]["reason"], "page_out_of_range");
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

#[cfg(unix)]
#[test]
fn default_output_is_private_and_stable() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let pdf = dir.path().join("doc.pdf");
    write_pdf(&pdf, &[(595, 842, 0); 2]);
    let tmp = dir.path().join("tmp");
    std::fs::create_dir(&tmp).unwrap();

    let render = || {
        let output = gridity()
            .args(["render", s(&pdf)])
            .env("TMPDIR", &tmp)
            .output()
            .unwrap();
        assert!(output.status.success());
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
    };
    let first = render();
    assert_eq!(render(), first);
    assert!(first.is_file());
    let sheet_dir = first.parent().unwrap();
    let root = sheet_dir.parent().unwrap();
    assert_eq!(root.parent().unwrap(), tmp);
    assert!(
        root.file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("gridity-")
    );
    for d in [root, sheet_dir] {
        let mode = std::fs::metadata(d).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "{}", d.display());
    }
    let leftovers: Vec<_> = std::fs::read_dir(sheet_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty());
}

#[test]
fn jpeg_mentioning_pdf_is_an_image() {
    let dir = TempDir::new().unwrap();
    let mut jpeg = Vec::new();
    JpegEncoder::new(&mut jpeg)
        .encode_image(&RgbImage::from_pixel(64, 48, Rgb([200, 50, 50])))
        .unwrap();
    let comment = b"%PDF-1.4 mention";
    let mut out = jpeg[..2].to_vec();
    out.extend([0xFF, 0xFE]);
    out.extend(((comment.len() + 2) as u16).to_be_bytes());
    out.extend(comment);
    out.extend(&jpeg[2..]);
    let photo = dir.path().join("photo.jpg");
    std::fs::write(&photo, out).unwrap();

    let probe = run_json(&["probe", s(&photo)]);
    assert_eq!(probe["source"]["kind"], "images");
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
    assert_eq!(manifest["sheets"][0]["cells"][0]["scale"], 1.0);
}

#[test]
fn oversized_png_falls_back_to_jpeg_only_by_default() {
    let dir = TempDir::new().unwrap();
    let noise = dir.path().join("noise.png");
    write_noise_png(&noise, 1500, 1000);

    let manifest = run_json(&["render", s(&noise), "-o", s(&dir.path().join("out"))]);
    let path = manifest["sheets"][0]["path"].as_str().unwrap();
    assert!(path.ends_with(".jpg"), "{path}");
    assert!(
        manifest["warnings"][0]
            .as_str()
            .unwrap()
            .contains("written as JPEG")
    );

    let explicit = dir.path().join("sheet.png");
    let manifest = run_json(&["render", s(&noise), "-o", s(&explicit)]);
    assert_eq!(manifest["sheets"][0]["path"], s(&explicit));
    assert!(explicit.is_file());
    let bytes = manifest["sheets"][0]["bytes"].as_u64().unwrap();
    assert!(bytes.div_ceil(3) * 4 > 5_000_000, "{bytes} bytes");
    assert!(manifest["warnings"][0].as_str().unwrap().contains("5 MB"));
}

#[test]
fn hires_sheets_warn_about_many_image_requests() {
    let dir = TempDir::new().unwrap();
    let pdf = dir.path().join("doc.pdf");
    write_pdf(&pdf, &[(595, 842, 0)]);
    let manifest = run_json(&["render", s(&pdf), "--preset", "hires", "--dry-run"]);
    assert!(
        manifest["warnings"][0]
            .as_str()
            .unwrap()
            .contains("2000 px")
    );
    let manifest = run_json(&["render", s(&pdf), "--dry-run"]);
    assert_eq!(manifest["warnings"].as_array().unwrap().len(), 0);
}

#[test]
fn fixed_columns_put_one_row_on_a_sheet() {
    let dir = TempDir::new().unwrap();
    let shots = dir.path().join("shots");
    std::fs::create_dir(&shots).unwrap();
    for i in 0..5 {
        write_png(&shots.join(format!("shot{i}.png")), 960, 600, i * 40);
    }
    let manifest = run_json(&[
        "render",
        s(&shots),
        "--cols",
        "5",
        "--detail",
        "low",
        "--dry-run",
    ]);
    let sheets = manifest["sheets"].as_array().unwrap();
    assert_eq!(sheets.len(), 1);
    assert_eq!(sheets[0]["cells"].as_array().unwrap().len(), 5);
    assert_eq!(sheets[0]["grid"]["cols"], 5);

    let pdf = dir.path().join("doc.pdf");
    write_pdf(&pdf, &[(595, 842, 0); 12]);
    let manifest = run_json(&["render", s(&pdf), "--cols", "4", "--dry-run"]);
    let sheets = manifest["sheets"].as_array().unwrap();
    assert!(sheets.len() <= 3, "{} sheets", sheets.len());
    assert!(
        sheets
            .iter()
            .all(|s| s["cells"].as_array().unwrap().len() >= 4)
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

/// A 66-byte GIF that declares a 12000x12000 screen with two 1x1 frames.
fn write_gif_bomb(path: &Path) {
    let mut gif = b"GIF89a".to_vec();
    gif.extend(12000u16.to_le_bytes());
    gif.extend(12000u16.to_le_bytes());
    gif.extend([0x80, 0, 0, 0, 0, 0, 0xff, 0xff, 0xff]);
    for _ in 0..2 {
        gif.extend([0x21, 0xf9, 0x04, 0x00, 0x0a, 0x00, 0x00, 0x00]);
        gif.extend([0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0]);
        gif.extend([0x02, 0x02, 0x44, 0x01, 0x00]);
    }
    gif.push(0x3b);
    std::fs::write(path, gif).unwrap();
}

#[test]
fn oversized_images_are_rejected_before_decoding() {
    let dir = TempDir::new().unwrap();
    let bomb = dir.path().join("bomb.gif");
    write_gif_bomb(&bomb);
    let started = std::time::Instant::now();
    gridity()
        .args(["probe", s(&bomb)])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("too large to decode"));
    assert!(started.elapsed().as_secs() < 5);

    let panorama = dir.path().join("panorama.png");
    write_png(&panorama, 17000, 2, 0);
    gridity()
        .args(["render", s(&panorama), "-o", s(&dir.path().join("out"))])
        .assert()
        .success();
}

#[test]
fn fast_apng_keeps_its_duration() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let apng = dir.path().join("anim60.png");
    let status = StdCommand::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc=size=64x64:rate=60:duration=2")
        .args(["-plays", "0", "-f", "apng"])
        .arg(&apng)
        .status()
        .unwrap();
    assert!(status.success());
    let probe = run_json(&["probe", s(&apng)]);
    assert_eq!(probe["source"]["kind"], "anim");
    assert_eq!(probe["source"]["frames"], 120);
    let duration = probe["source"]["duration"].as_f64().unwrap();
    assert!((duration - 2.0).abs() < 0.05, "duration {duration}");
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

    let cropped = run_json(&[
        "render",
        s(&rotated),
        "--count",
        "2",
        "--crop",
        "0,0,1,0.5",
        "-o",
        s(&dir.path().join("crop")),
    ]);
    let sheet = &cropped["sheets"][0];
    assert_eq!(sheet["cells"].as_array().unwrap().len(), 2);
    let rect = &sheet["cells"][0]["rect"];
    assert_eq!((rect[2].as_u64(), rect[3].as_u64()), (Some(180), Some(160)));
    assert_eq!(sheet["cells"][0]["scale"], 1.0);

    gridity()
        .args(["render", s(&video), "--end", "9"])
        .assert()
        .code(2);
}

#[test]
fn video_file_names_are_not_parsed_by_ffmpeg() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let video = dir.path().join("clip.mp4");
    write_video(&video, &[]);
    for name in ["a:b.mp4", "-clip.mp4", "100%.mp4", "日本語 動画.mp4"] {
        std::fs::copy(&video, dir.path().join(name)).unwrap();
        let output = gridity()
            .current_dir(dir.path())
            .args(["render", "--count", "2", "-o", "out", "--json", "--", name])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn huge_samplings_fail_before_allocating() {
    let dir = TempDir::new().unwrap();
    let gif = dir.path().join("anim.gif");
    write_gif(&gif, 10, 100);

    let manifest = run_json(&["render", s(&gif), "--every", "0.001", "--dry-run"]);
    assert_eq!(manifest["sheets"][0]["cells"].as_array().unwrap().len(), 10);

    let started = std::time::Instant::now();
    for args in [["--count", "200000000"], ["--every", "1e-9"]] {
        gridity()
            .args(["render", s(&gif), "--dry-run"])
            .args(args)
            .assert()
            .code(2)
            .stderr(predicate::str::contains("timestamps"));
    }
    assert!(started.elapsed().as_secs() < 5);

    if !have_ffmpeg() {
        eprintln!("skipping video part: ffmpeg not available");
        return;
    }
    let video = dir.path().join("clip.mp4");
    write_video(&video, &[]);
    let started = std::time::Instant::now();
    for args in [["--every", "0.000001"], ["--count", "200000000"]] {
        let output = run_error_json(&["render", s(&video), "--dry-run", args[0], args[1]]);
        assert_eq!(output["reason"], "too_many_sheets");
        assert!(output["message"].as_str().unwrap().contains("--max-sheets"));
        assert!(
            !output["hint"].as_str().unwrap().contains("--max-sheets"),
            "the hint must not suggest more sheets than --max-sheets accepts"
        );
    }
    assert!(started.elapsed().as_secs() < 5);
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
    assert_eq!(error["error"]["reason"], "invalid_argument");
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
fn out_of_range_numbers_are_usage_errors() {
    let dir = TempDir::new().unwrap();
    let pdf = dir.path().join("doc.pdf");
    write_pdf(&pdf, &[(595, 842, 0)]);
    for args in [
        ["--label-size", "NaN"],
        ["--label-size", "-5"],
        ["--label-size", "inf"],
        ["--max-edge", "4294967295"],
        ["--max-edge", "20000"],
        ["--max-tokens", "400000"],
        ["--padding", "4000000000"],
        ["--cols", "0"],
        ["--per-sheet", "0"],
        ["--max-sheets", "0"],
    ] {
        let output = gridity()
            .args(["render", s(&pdf), "-o", s(&dir.path().join("out"))])
            .args(args)
            .arg("--json")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let error: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error["error"]["kind"], "usage", "{args:?}");
    }
    run_json(&[
        "render",
        s(&pdf),
        "--max-edge",
        "8000",
        "--label-size",
        "40",
        "--dry-run",
    ]);
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

#[test]
fn passwords_come_from_a_file_stdin_or_the_environment() {
    let pdf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/encrypted.pdf");
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("pw.txt");
    std::fs::write(&file, "secret\n").unwrap();
    let pages = |cmd: &mut Command| {
        let output = cmd.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["source"]["pages"].clone()
    };

    let mut cmd = gridity();
    cmd.args([
        "render",
        s(&pdf),
        "--dry-run",
        "--json",
        "--password-file",
        s(&file),
    ]);
    assert_eq!(pages(&mut cmd), 1);

    let mut cmd = gridity();
    cmd.args(["text", s(&pdf), "--json", "--password-file", "-"])
        .write_stdin("secret\r\n");
    assert_eq!(pages(&mut cmd), 1);

    let mut cmd = gridity();
    cmd.args(["probe", s(&pdf), "--json"])
        .env("GRIDITY_PDF_PASSWORD", "secret");
    assert_eq!(pages(&mut cmd), 1);

    gridity()
        .args(["render", s(&pdf), "--dry-run"])
        .env("GRIDITY_PDF_PASSWORD", "wrong")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("incorrect"));
    gridity()
        .args([
            "render",
            s(&pdf),
            "--password",
            "secret",
            "--password-file",
            s(&file),
        ])
        .assert()
        .code(2);

    let png = dir.path().join("a.png");
    write_png(&png, 40, 30, 100);
    gridity()
        .args(["render", s(&png), "--dry-run", "--password-file", s(&file)])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--password-file"));
}

#[test]
fn long_pdfs_keep_every_page_in_order() {
    let dir = TempDir::new().unwrap();
    let pdf = dir.path().join("long.pdf");
    let pages: Vec<(u32, u32, u32, String)> = (1..=300)
        .map(|i| {
            (
                595,
                842,
                0,
                format!("BT /F1 12 Tf 72 720 Td (Page {i} body) Tj ET\n"),
            )
        })
        .collect();
    write_pdf_pages(&pdf, &pages);

    let text = run_json(&["text", s(&pdf)]);
    let texts = text["pages"].as_array().unwrap();
    assert_eq!(texts.len(), 300);
    for (i, page) in texts.iter().enumerate() {
        assert_eq!(page["page"], i + 1);
        assert_eq!(page["text"], format!("Page {} body", i + 1));
    }

    let probe = run_json(&["probe", s(&pdf), "--per-page"]);
    assert_eq!(probe["pages"].as_array().unwrap().len(), 300);

    let manifest = run_json(&["render", s(&pdf), "--pages", "120,7,299", "--dry-run"]);
    assert_eq!(manifest["limits"]["rule"], "claude-28px-patch");
    let cells: Vec<&Value> = manifest["sheets"][0]["cells"]
        .as_array()
        .unwrap()
        .iter()
        .collect();
    let shown: Vec<u64> = cells.iter().map(|c| c["page"].as_u64().unwrap()).collect();
    assert_eq!(shown, [120, 7, 299]);
}

fn truncate(path: &Path, fraction: f64) {
    let bytes = std::fs::read(path).unwrap();
    let keep = (bytes.len() as f64 * fraction) as usize;
    std::fs::write(path, &bytes[..keep]).unwrap();
}

#[test]
fn dot_files_in_directories_are_ignored() {
    let dir = TempDir::new().unwrap();
    write_png(&dir.path().join("shot-01.png"), 64, 48, 10);
    write_png(&dir.path().join("shot-02.png"), 64, 48, 90);
    std::fs::write(
        dir.path().join("._shot-01.png"),
        b"\0\x05\x16\x07\0\x02\0\0",
    )
    .unwrap();

    let manifest = run_json(&["render", s(dir.path()), "-o", s(&dir.path().join("out"))]);
    let names: Vec<&str> = manifest["sheets"][0]["cells"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["label"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["shot-01.png", "shot-02.png"]);
    assert_eq!(manifest["warnings"], serde_json::json!([]));
}

#[test]
fn unreadable_images_are_skipped_or_left_blank() {
    let dir = TempDir::new().unwrap();
    let images = dir.path().join("images");
    std::fs::create_dir(&images).unwrap();
    write_png(&images.join("a.png"), 64, 48, 10);
    std::fs::write(images.join("b.png"), b"not a png at all").unwrap();
    write_noise_png(&images.join("c.png"), 200, 150);
    truncate(&images.join("c.png"), 0.5);

    let planned = run_json(&["render", s(&images), "--dry-run"]);
    let manifest = run_json(&["render", s(&images), "-o", s(&dir.path().join("out"))]);
    let cells = manifest["sheets"][0]["cells"].as_array().unwrap();
    assert_eq!(
        cells.len(),
        2,
        "b.png has no readable header and is skipped"
    );
    assert!(cells[0].get("error").is_none());
    assert!(cells[1]["file"].as_str().unwrap().ends_with("c.png"));
    assert!(cells[1]["error"].is_string());
    assert_eq!(cells[1]["rect"], planned["sheets"][0]["cells"][1]["rect"]);
    let warnings = manifest["warnings"].to_string();
    assert!(warnings.contains("b.png"), "{warnings}");
    assert!(warnings.contains("empty cell"), "{warnings}");

    let probe = run_json(&["probe", s(&images)]);
    assert_eq!(probe["source"]["count"], 2);
    assert!(probe["warnings"].to_string().contains("b.png"));

    gridity()
        .args(["render", s(&images.join("b.png")), "--type", "images"])
        .assert()
        .code(3);
}

#[test]
fn damaged_animations_keep_their_readable_frames() {
    let dir = TempDir::new().unwrap();
    let gif = dir.path().join("anim.gif");
    write_gif(&gif, 12, 100);
    truncate(&gif, 0.6);

    let manifest = run_json(&["render", s(&gif), "--dry-run"]);
    let frames = manifest["source"]["frames"].as_u64().unwrap();
    assert!((1..12).contains(&frames), "{frames} frames");
    assert!(
        manifest["warnings"]
            .to_string()
            .contains("could be decoded")
    );
    run_json(&["render", s(&gif), "-o", s(&dir.path().join("out"))]);
}

#[test]
fn errors_carry_a_reason() {
    let dir = TempDir::new().unwrap();
    let error = |args: &[&str], code: i32| {
        let output = gridity().args(args).arg("--json").output().unwrap();
        assert_eq!(output.status.code(), Some(code), "{args:?}");
        let body: Value = serde_json::from_slice(&output.stdout).unwrap();
        body["error"]["reason"].as_str().unwrap().to_string()
    };
    let pdf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/encrypted.pdf");
    assert_eq!(
        error(&["render", s(&pdf), "--dry-run"], 3),
        "password_required"
    );
    assert_eq!(
        error(&["probe", s(&pdf), "--password", "wrong"], 3),
        "password_incorrect"
    );
    let missing = dir.path().join("missing.pdf");
    assert_eq!(error(&["text", s(&missing)], 3), "not_found");

    let video = dir.path().join("clip.mp4");
    std::fs::write(&video, b"\0\0\0\x18ftypmp42").unwrap();
    let output = gridity()
        .args([
            "probe",
            s(&video),
            "--json",
            "--ffmpeg",
            "/nonexistent/ffmpeg",
        ])
        .env("GRIDITY_FFPROBE", "/nonexistent/ffprobe")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["error"]["reason"], "tool_missing");
}

#[test]
fn mcp_server_returns_sheets_as_images() {
    let dir = TempDir::new().unwrap();
    let pdf = dir.path().join("doc.pdf");
    write_pdf(&pdf, &[(595, 842, 0); 3]);
    let messages = [
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-11-25", "capabilities": {},
                       "clientInfo": {"name": "test", "version": "0"}}}),
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "render", "arguments": {"inputs": [s(&pdf)], "pages": "-2"}}}),
        serde_json::json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": {"name": "text", "arguments": {"input": s(&dir.path().join("none.pdf"))}}}),
    ];
    let stdin: String = messages.iter().map(|m| format!("{m}\n")).collect();
    let output = gridity().arg("mcp").write_stdin(stdin).output().unwrap();
    assert!(output.status.success());
    let responses: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let ids: Vec<i64> = responses
        .iter()
        .map(|r| r["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, [1, 2, 3, 4]);
    assert_eq!(responses[0]["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(responses[1]["result"]["tools"].as_array().unwrap().len(), 4);

    let render = &responses[2]["result"];
    assert_eq!(render["isError"], false);
    let manifest = &render["structuredContent"];
    assert_eq!(manifest["sheets"].as_array().unwrap().len(), 1);
    let cells: Vec<u64> = manifest["sheets"][0]["cells"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["page"].as_u64().unwrap())
        .collect();
    assert_eq!(cells, [1, 2]);
    let content = render["content"].as_array().unwrap();
    assert!(content[0]["text"].as_str().unwrap().contains("\"rect\""));
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["mimeType"], "image/png");
    assert!(content[1]["data"].as_str().unwrap().starts_with("iVBOR"));

    let text = &responses[3]["result"];
    assert_eq!(text["isError"], true);
    assert!(text.get("structuredContent").is_none());
    assert!(
        text["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("reason: not_found")
    );
}

#[test]
fn characters_without_a_font_are_reported_once() {
    let dir = TempDir::new().unwrap();
    write_png(&dir.path().join("한국어.png"), 64, 48, 10);
    write_png(&dir.path().join("日本語.png"), 64, 48, 90);
    let manifest = run_json(&["render", s(dir.path()), "--dry-run"]);
    let warnings = manifest["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let warning = warnings[0].as_str().unwrap();
    assert!(
        warning.contains("U+D55C") && warning.contains("--font"),
        "{warning}"
    );
    assert!(!warning.contains('日'));
}
