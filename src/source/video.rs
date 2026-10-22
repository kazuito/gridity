use std::ffi::OsString;
use std::io::{ErrorKind as IoErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use image::{ImageFormat, RgbaImage};
use rayon::prelude::*;
use serde_json::Value;

use super::{Item, ItemKey, LabelVars, Rendered, fit_size};
use crate::error::{Reason, dependency, input};
use crate::select::Crop;

const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const FRAME_TIMEOUT: Duration = Duration::from_secs(60);

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
                    "-protocol_whitelist",
                    "file",
                    "-i",
                ])
                .arg(input_url(path)),
            &ffprobe,
            PROBE_TIMEOUT,
        )?;
        if !output.status.success() {
            return Err(input(
                Reason::DecodeFailed,
                format!(
                    "ffprobe could not read {}: {}",
                    path.display(),
                    last_line(&output.stderr)
                ),
            )
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
            .ok_or_else(|| {
                input(
                    Reason::NoVideoStream,
                    format!("{} has no video stream", path.display()),
                )
            })?;

        let coded = (
            stream["width"].as_u64().unwrap_or(0) as u32,
            stream["height"].as_u64().unwrap_or(0) as u32,
        );
        if coded.0 == 0 || coded.1 == 0 {
            return Err(input(
                Reason::NoVideoStream,
                format!("{} has a video stream without a size", path.display()),
            )
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

    pub fn render(&self, items: &[Item], boxes: &[(u32, u32)]) -> Result<Vec<Rendered>> {
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
                        return Ok(Rendered::ok(frame));
                    }
                }
                Ok(Rendered::failed(
                    size,
                    format!("no frame could be decoded at {t:.2}s"),
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
                    "-protocol_whitelist",
                    "file",
                    "-i",
                ])
                .arg(input_url(&self.path))
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
            FRAME_TIMEOUT,
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
        .map(|dir| {
            dir.join("ffprobe")
                .with_extension(std::env::consts::EXE_EXTENSION)
        })
        .filter(|p| p.exists());
    let ffprobe = std::env::var_os("GRIDITY_FFPROBE")
        .map(PathBuf::from)
        .or(sibling)
        .unwrap_or_else(|| PathBuf::from("ffprobe"));
    (ffmpeg, ffprobe)
}

/// A `file:` URL with an absolute path, so ffmpeg reads names like `a:b.mp4` or `-clip.mp4`
/// as files rather than as a protocol or an option.
fn input_url(path: &Path) -> OsString {
    let mut url = OsString::from("file:");
    url.push(std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()));
    url
}

/// Runs a tool and collects its output, killing it when it runs longer than `timeout`.
fn run_tool(command: &mut Command, program: &Path, timeout: Duration) -> Result<Output> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| -> anyhow::Error {
            if e.kind() == IoErrorKind::NotFound {
                dependency(Reason::ToolMissing, format!("{} was not found", program.display()))
                    .with_hint("install ffmpeg (e.g. `brew install ffmpeg`) or pass --ffmpeg / set GRIDITY_FFMPEG")
                    .into()
            } else {
                anyhow::Error::new(e).context(format!("failed to run {}", program.display()))
            }
        })?;
    // Both pipes are drained while waiting; a full pipe would otherwise stall the tool.
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(input(Reason::Timeout, format!(
                "{} did not finish within {}s",
                program.display(),
                timeout.as_secs_f64()
            ))
            .with_hint("the video may be damaged or very slow to decode; re-encode it or pick other times with --at")
            .into());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
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
    fn input_urls_are_absolute_files() {
        let url = input_url(Path::new("-clip.mp4"));
        let url = url.to_str().unwrap();
        assert!(url.starts_with("file:"));
        assert!(Path::new(&url["file:".len()..]).is_absolute());
        assert!(url.ends_with("/-clip.mp4") || url.ends_with("\\-clip.mp4"));
    }

    #[cfg(unix)]
    #[test]
    fn tools_are_killed_after_the_timeout() {
        let started = Instant::now();
        let err = run_tool(
            Command::new("sleep").arg("5"),
            Path::new("sleep"),
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(err.to_string().contains("did not finish"));

        let output = run_tool(
            Command::new("head").args(["-c", "1000000", "/dev/zero"]),
            Path::new("head"),
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(output.stdout.len(), 1_000_000);
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
