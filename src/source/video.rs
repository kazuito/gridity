use std::io::ErrorKind as IoErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result};
use image::{ImageFormat, Rgba, RgbaImage};
use rayon::prelude::*;
use serde_json::Value;

use super::{Item, ItemKey, LabelVars, Warnings, fit_size};
use crate::error::{dependency, input};
use crate::select::Crop;

pub struct VideoSource {
    pub path: PathBuf,
    ffmpeg: PathBuf,
    pub duration: Option<f64>,
    /// Size as shown to a viewer: sample aspect ratio and rotation applied.
    pub display: (u32, u32),
    pub coded: (u32, u32),
    pub rotation: i32,
    pub fps: Option<f64>,
    pub codec: Option<String>,
}

impl VideoSource {
    pub fn open(path: &Path, ffmpeg: Option<&Path>) -> Result<VideoSource> {
        let (ffmpeg, ffprobe) = resolve_tools(ffmpeg);
        let output = run_tool(
            Command::new(&ffprobe)
                .args([
                    "-v",
                    "error",
                    "-print_format",
                    "json",
                    "-show_format",
                    "-show_streams",
                ])
                .arg(path),
            &ffprobe,
        )?;
        if !output.status.success() {
            return Err(input(format!(
                "ffprobe could not read {}: {}",
                path.display(),
                last_line(&output.stderr)
            ))
            .into());
        }
        let info: Value =
            serde_json::from_slice(&output.stdout).context("ffprobe returned invalid JSON")?;
        let stream = info["streams"]
            .as_array()
            .and_then(|streams| {
                streams.iter().find(|s| {
                    s["codec_type"] == "video"
                        && s["disposition"]["attached_pic"].as_i64().unwrap_or(0) == 0
                })
            })
            .ok_or_else(|| input(format!("{} has no video stream", path.display())))?;

        let coded = (
            stream["width"].as_u64().unwrap_or(0) as u32,
            stream["height"].as_u64().unwrap_or(0) as u32,
        );
        if coded.0 == 0 || coded.1 == 0 {
            return Err(input(format!(
                "{} has a video stream without a size",
                path.display()
            ))
            .into());
        }
        let sar = stream["sample_aspect_ratio"]
            .as_str()
            .and_then(parse_ratio)
            .unwrap_or(1.0);
        let rotation = rotation_of(stream);
        let mut display = ((coded.0 as f64 * sar).round() as u32, coded.1);
        if rotation % 180 != 0 {
            display = (display.1, display.0);
        }
        let duration = parse_seconds(&info["format"]["duration"])
            .or_else(|| parse_seconds(&stream["duration"]))
            .filter(|d| *d > 0.0);
        let fps = stream["avg_frame_rate"]
            .as_str()
            .and_then(parse_ratio)
            .filter(|f| *f > 0.0);
        let codec = stream["codec_name"].as_str().map(str::to_string);

        Ok(VideoSource {
            path: path.to_path_buf(),
            ffmpeg,
            duration,
            display,
            coded,
            rotation,
            fps,
            codec,
        })
    }

    pub fn aspect(&self) -> f64 {
        self.display.0 as f64 / self.display.1 as f64
    }

    pub fn items(&self, times: &[f64], labels: &[String]) -> Vec<Item> {
        times
            .iter()
            .zip(labels)
            .enumerate()
            .map(|(i, (&t, label))| Item {
                aspect: self.aspect(),
                native: Some(self.display),
                key: ItemKey::Time(t),
                vars: LabelVars {
                    index: i + 1,
                    total: times.len(),
                    time: Some(label.clone()),
                    ..LabelVars::default()
                },
                crop: None,
            })
            .collect()
    }

    pub fn render(
        &self,
        items: &[Item],
        boxes: &[(u32, u32)],
        warnings: &Warnings,
    ) -> Result<Vec<RgbaImage>> {
        items
            .par_iter()
            .zip(boxes)
            .map(|(item, &bounds)| {
                let ItemKey::Time(t) = item.key else {
                    unreachable!("video items are timestamps")
                };
                let size = fit_size(item.aspect, bounds, item.native);
                // Seeking exactly to the end returns no frame, so fall back to slightly earlier positions.
                for back in [0.0, 0.25, 1.0, 3.0] {
                    if back > 0.0 && t - back < 0.0 {
                        break;
                    }
                    if let Some(frame) = self.extract_frame((t - back).max(0.0), size, item.crop)? {
                        return Ok(frame);
                    }
                }
                warnings.push(format!(
                    "no frame could be decoded at {t:.2}s; the cell is left blank"
                ));
                Ok(RgbaImage::from_pixel(
                    size.0,
                    size.1,
                    Rgba([128, 128, 128, 255]),
                ))
            })
            .collect()
    }

    fn extract_frame(
        &self,
        t: f64,
        size: (u32, u32),
        crop: Option<Crop>,
    ) -> Result<Option<RgbaImage>> {
        // ffmpeg rotates frames before the filter chain, so the crop applies to the displayed frame.
        let crop = crop
            .map(|c| format!("crop=iw*{}:ih*{}:iw*{}:ih*{},", c.width, c.height, c.x, c.y))
            .unwrap_or_default();
        let output = run_tool(
            Command::new(&self.ffmpeg)
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-nostdin",
                    "-ss",
                    &format!("{t:.3}"),
                    "-i",
                ])
                .arg(&self.path)
                .args([
                    "-frames:v",
                    "1",
                    "-an",
                    "-sn",
                    "-vf",
                    &format!("{crop}scale={}:{}:flags=lanczos,setsar=1", size.0, size.1),
                    "-pix_fmt",
                    "rgb24",
                    "-f",
                    "image2pipe",
                    "-c:v",
                    "png",
                    "-",
                ]),
            &self.ffmpeg,
        )?;
        if !output.status.success() || output.stdout.is_empty() {
            return Ok(None);
        }
        let image = image::load_from_memory_with_format(&output.stdout, ImageFormat::Png)
            .context("ffmpeg returned an undecodable frame")?;
        Ok(Some(image.to_rgba8()))
    }
}

/// ffmpeg comes from --ffmpeg, GRIDITY_FFMPEG or PATH; ffprobe from GRIDITY_FFPROBE, next to ffmpeg, or PATH.
fn resolve_tools(custom: Option<&Path>) -> (PathBuf, PathBuf) {
    let ffmpeg = custom
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("GRIDITY_FFMPEG").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("ffmpeg"));
    let sibling = ffmpeg
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join("ffprobe"))
        .filter(|p| p.exists());
    let ffprobe = std::env::var_os("GRIDITY_FFPROBE")
        .map(PathBuf::from)
        .or(sibling)
        .unwrap_or_else(|| PathBuf::from("ffprobe"));
    (ffmpeg, ffprobe)
}

fn run_tool(command: &mut Command, program: &Path) -> Result<Output> {
    command.output().map_err(|e| {
        if e.kind() == IoErrorKind::NotFound {
            dependency(format!("{} was not found", program.display()))
                .with_hint("install ffmpeg (e.g. `brew install ffmpeg`) or pass --ffmpeg / set GRIDITY_FFMPEG")
                .into()
        } else {
            anyhow::Error::new(e).context(format!("failed to run {}", program.display()))
        }
    })
}

fn rotation_of(stream: &Value) -> i32 {
    let from_side_data = stream["side_data_list"]
        .as_array()
        .and_then(|list| list.iter().find_map(|d| d["rotation"].as_f64()));
    let from_tags = stream["tags"]["rotate"]
        .as_str()
        .and_then(|r| r.parse::<f64>().ok());
    let degrees = from_side_data.or(from_tags).unwrap_or(0.0).round() as i32;
    degrees.rem_euclid(360)
}

fn parse_ratio(s: &str) -> Option<f64> {
    let (a, b) = s.split_once(['/', ':'])?;
    let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
    (a > 0.0 && b > 0.0).then_some(a / b)
}

fn parse_seconds(value: &Value) -> Option<f64> {
    value
        .as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| value.as_f64())
}

fn last_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("unknown error")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ratios() {
        assert_eq!(
            parse_ratio("30000/1001").map(|f| (f * 100.0).round()),
            Some(2997.0)
        );
        assert_eq!(parse_ratio("4:3"), Some(4.0 / 3.0));
        assert_eq!(parse_ratio("0:1"), None);
        assert_eq!(parse_ratio("N/A"), None);
    }

    #[test]
    fn rotation_from_side_data_or_tags() {
        assert_eq!(
            rotation_of(&json!({"side_data_list": [{"rotation": -90}]})),
            270
        );
        assert_eq!(rotation_of(&json!({"tags": {"rotate": "90"}})), 90);
        assert_eq!(rotation_of(&json!({})), 0);
    }
}
