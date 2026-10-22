use crate::error::{CliError, usage};

/// Parses a 1-based page list such as "1-3,5,10-" or "-4" against `total` pages.
/// Order is preserved and duplicates are dropped.
pub fn parse_pages(spec: &str, total: usize) -> Result<Vec<usize>, CliError> {
    let mut pages = Vec::new();
    for raw in spec.split(',') {
        let part = raw.trim();
        if part.is_empty() {
            continue;
        }
        let (first, last) = match part.split_once('-') {
            Some((a, b)) => {
                let first = if a.trim().is_empty() {
                    1
                } else {
                    parse_page_number(a, spec)?
                };
                let last = if b.trim().is_empty() {
                    total
                } else {
                    parse_page_number(b, spec)?
                };
                (first, last)
            }
            None => {
                let page = parse_page_number(part, spec)?;
                (page, page)
            }
        };
        if first > last {
            return Err(usage(format!(
                "invalid page range '{part}': start is after end"
            )));
        }
        if last > total {
            return Err(usage(format!(
                "page {last} is out of range: the document has {total} pages"
            ))
            .with_hint(format!("use --pages within 1-{total}")));
        }
        for page in first..=last {
            if !pages.contains(&page) {
                pages.push(page);
            }
        }
    }
    if pages.is_empty() {
        return Err(usage(format!("--pages '{spec}' selects no pages")));
    }
    Ok(pages)
}

fn parse_page_number(s: &str, spec: &str) -> Result<usize, CliError> {
    match s.trim().parse::<usize>() {
        Ok(0) => Err(usage("page numbers start at 1")),
        Ok(n) => Ok(n),
        Err(_) => Err(usage(format!(
            "invalid page list '{spec}' (expected e.g. \"1-3,5,10-\")"
        ))),
    }
}

/// Part of a page or frame to show, as fractions of its displayed width and height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crop {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Crop {
    /// The crop inside an image of `size` pixels: x, y, width, height (at least 1x1).
    pub fn pixels(&self, (w, h): (u32, u32)) -> (u32, u32, u32, u32) {
        let edge = |start: f64, len: f64, total: u32| {
            let a = ((start * total as f64).round() as u32).min(total.saturating_sub(1));
            let b = (((start + len) * total as f64).round() as u32).clamp(a + 1, total.max(a + 1));
            (a, b - a)
        };
        let (x, width) = edge(self.x, self.width, w);
        let (y, height) = edge(self.y, self.height, h);
        (x, y, width, height)
    }
}

/// Parses `X,Y,W,H` fractions, e.g. "0,0.5,1,0.5" for the bottom half.
pub fn parse_crop(spec: &str) -> Result<Crop, CliError> {
    let bad = |why: &str| {
        usage(format!("invalid --crop '{spec}': {why}")).with_hint(
            "--crop takes X,Y,W,H as fractions of the page or frame, e.g. 0,0.5,1,0.5 for the bottom half",
        )
    };
    let values: Vec<f64> = spec
        .split(',')
        .map(|v| v.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .map_err(|_| bad("expected four numbers"))?;
    let [x, y, width, height] = values[..] else {
        return Err(bad("expected four numbers"));
    };
    if values.iter().any(|v| !(0.0..=1.0).contains(v)) {
        return Err(bad("values must be between 0 and 1"));
    }
    if width <= 0.0 || height <= 0.0 {
        return Err(bad("width and height must be greater than 0"));
    }
    if x + width > 1.0 + 1e-9 || y + height > 1.0 + 1e-9 {
        return Err(bad("the region extends past the edge"));
    }
    Ok(Crop {
        x,
        y,
        width,
        height,
    })
}

/// Parses "90", "90.5", "90s", "500ms", "1m30s", "1h2m", "1:30", "01:02:03.5" into seconds.
pub fn parse_time(s: &str) -> Result<f64, CliError> {
    let text = s.trim();
    let invalid = || {
        usage(format!(
            "invalid time '{s}' (expected e.g. \"90\", \"1:30\", \"00:01:30.5\", \"1m30s\")"
        ))
    };
    if text.is_empty() {
        return Err(invalid());
    }

    let seconds = if text.contains(':') {
        let mut total = 0.0;
        for part in text.split(':') {
            let value: f64 = part.parse().map_err(|_| invalid())?;
            total = total * 60.0 + value;
        }
        total
    } else if let Ok(value) = text.parse::<f64>() {
        value
    } else {
        parse_unit_time(text).ok_or_else(invalid)?
    };

    if !seconds.is_finite() || seconds < 0.0 {
        return Err(invalid());
    }
    Ok(seconds)
}

fn parse_unit_time(text: &str) -> Option<f64> {
    let mut total = 0.0;
    let mut number = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() || c == '.' {
            number.push(c);
            continue;
        }
        let value: f64 = number.parse().ok()?;
        number.clear();
        let factor = match c {
            'h' => 3600.0,
            'm' if chars.peek() == Some(&'s') => {
                chars.next();
                0.001
            }
            'm' => 60.0,
            's' => 1.0,
            _ => return None,
        };
        total += value * factor;
    }
    if !number.is_empty() {
        return None;
    }
    Some(total)
}

/// Formats seconds as m:ss or h:mm:ss, with one decimal when `fine` is set.
/// Truncates like a media player, so the label never runs ahead of the frame shown.
pub fn format_time(seconds: f64, long: bool, fine: bool) -> String {
    let tenths = (seconds.max(0.0) * 10.0 + 1e-6).floor() as u64;
    let whole = tenths / 10;
    let (h, m, s) = (whole / 3600, (whole / 60) % 60, whole % 60);
    let base = if long || h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    };
    if fine {
        format!("{base}.{}", tenths % 10)
    } else {
        base
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Sampling {
    Count(usize),
    Every(f64),
    At(Vec<f64>),
}

impl Sampling {
    pub fn from_args(
        every: Option<&str>,
        count: Option<usize>,
        at: Option<&str>,
    ) -> Result<Option<Sampling>, CliError> {
        if let Some(every) = every {
            let step = parse_time(every)?;
            if step <= 0.0 {
                return Err(usage("--every must be greater than 0"));
            }
            return Ok(Some(Sampling::Every(step)));
        }
        if let Some(count) = count {
            if count == 0 {
                return Err(usage("--count must be at least 1"));
            }
            return Ok(Some(Sampling::Count(count)));
        }
        if let Some(at) = at {
            let times = at
                .split(',')
                .filter(|t| !t.trim().is_empty())
                .map(parse_time)
                .collect::<Result<Vec<_>, _>>()?;
            if times.is_empty() {
                return Err(usage("--at needs at least one time"));
            }
            return Ok(Some(Sampling::At(times)));
        }
        Ok(None)
    }
}

/// Returns the timestamps to capture inside `[start, end)`.
/// `Count` takes the midpoint of each equal slice so the first frame is not a fade-in.
pub fn sample_times(start: f64, end: f64, sampling: &Sampling) -> Vec<f64> {
    let span = (end - start).max(0.0);
    match sampling {
        Sampling::Count(n) => (0..*n)
            .map(|i| start + span * (i as f64 + 0.5) / *n as f64)
            .collect(),
        Sampling::Every(step) => {
            let mut times = Vec::new();
            let mut t = start;
            while t < end || times.is_empty() {
                times.push(t);
                t = start + step * times.len() as f64;
            }
            times
        }
        Sampling::At(times) => times.clone(),
    }
}

/// Smallest gap between consecutive timestamps, used to decide whether labels need tenths.
pub fn min_interval(times: &[f64]) -> Option<f64> {
    let mut sorted = times.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|d| *d > 0.0)
        .min_by(f64::total_cmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_crop_accepts_fractions_within_the_page() {
        let crop = parse_crop("0, 0.5, 1, 0.5").unwrap();
        assert_eq!(crop.pixels((200, 100)), (0, 50, 200, 50));
        assert_eq!(
            parse_crop("0.25,0.25,0.5,0.5").unwrap().pixels((3, 3)),
            (1, 1, 1, 1)
        );
        for bad in [
            "0,0,1",
            "0,0,1,1,1",
            "a,0,1,1",
            "0,0,0,1",
            "0.5,0,0.6,1",
            "-0.1,0,1,1",
        ] {
            assert!(parse_crop(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn crop_pixels_stay_inside_the_image() {
        let crop = Crop {
            x: 0.999,
            y: 0.0,
            width: 0.001,
            height: 1.0,
        };
        assert_eq!(crop.pixels((100, 10)), (99, 0, 1, 10));
    }

    #[test]
    fn page_lists() {
        assert_eq!(
            parse_pages("1-3,5,10-", 12).unwrap(),
            vec![1, 2, 3, 5, 10, 11, 12]
        );
        assert_eq!(parse_pages("-3", 12).unwrap(), vec![1, 2, 3]);
        assert_eq!(parse_pages("4,2,4", 12).unwrap(), vec![4, 2]);
        assert_eq!(parse_pages(" 7 ", 12).unwrap(), vec![7]);
    }

    #[test]
    fn page_list_errors() {
        assert!(parse_pages("13", 12).is_err());
        assert!(parse_pages("0", 12).is_err());
        assert!(parse_pages("5-3", 12).is_err());
        assert!(parse_pages("a", 12).is_err());
        assert!(parse_pages(",", 12).is_err());
    }

    #[test]
    fn times() {
        assert_eq!(parse_time("90").unwrap(), 90.0);
        assert_eq!(parse_time("90.5s").unwrap(), 90.5);
        assert_eq!(parse_time("1:30").unwrap(), 90.0);
        assert_eq!(parse_time("01:02:03.5").unwrap(), 3723.5);
        assert_eq!(parse_time("1m30s").unwrap(), 90.0);
        assert_eq!(parse_time("1h2m").unwrap(), 3720.0);
        assert_eq!(parse_time("500ms").unwrap(), 0.5);
        assert!(parse_time("abc").is_err());
        assert!(parse_time("1x").is_err());
        assert!(parse_time("-3").is_err());
        assert!(parse_time("").is_err());
    }

    #[test]
    fn time_labels() {
        assert_eq!(format_time(75.0, false, false), "1:15");
        assert_eq!(format_time(75.25, false, true), "1:15.2");
        assert_eq!(format_time(59.96, false, false), "0:59");
        assert_eq!(format_time(0.3, false, true), "0:00.3");
        assert_eq!(format_time(3725.0, false, false), "1:02:05");
        assert_eq!(format_time(5.0, true, false), "0:00:05");
    }

    #[test]
    fn sampling() {
        assert_eq!(sample_times(0.0, 10.0, &Sampling::Count(2)), vec![2.5, 7.5]);
        assert_eq!(
            sample_times(0.0, 10.0, &Sampling::Every(4.0)),
            vec![0.0, 4.0, 8.0]
        );
        assert_eq!(sample_times(5.0, 5.0, &Sampling::Every(4.0)), vec![5.0]);
        assert_eq!(min_interval(&[0.0, 4.0, 1.0]), Some(1.0));
    }
}
