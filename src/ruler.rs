//! Ruler ticks drawn over cells. Values are fractions of the whole page or frame, the same
//! coordinates `--crop` takes, so a value read off a cropped cell can be reused as is.
//! Steps are kept in hundredths so tick values are exact.

use crate::select::Crop;

/// Steps the automatic ruler chooses from, in hundredths.
const NICE_STEPS: [u32; 6] = [1, 2, 5, 10, 20, 50];
/// The automatic ruler aims for at most this many intervals across the shown range.
const MAX_INTERVALS: f64 = 10.0;
/// Closest two lines may be, in pixels; denser steps are coarsened.
pub const MIN_SPACING: u32 = 28;
pub const MIN_STEP: u32 = 1;
pub const MAX_STEP: u32 = 50;

/// A line at `pos` pixels from the start of the cell edge, for the fraction `value` / 100.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tick {
    pub pos: u32,
    pub value: u32,
}

/// Parses a step such as "0.05": a multiple of 0.01 between 0.01 and 0.5.
pub fn parse_step(text: &str) -> Result<u32, String> {
    let step: f64 = text
        .trim()
        .parse()
        .map_err(|_| format!("{text} is not a number"))?;
    let hundredths = (step * 100.0).round();
    if !step.is_finite() || (step * 100.0 - hundredths).abs() > 1e-6 {
        return Err("must be a multiple of 0.01, e.g. 0.05".to_string());
    }
    let hundredths = hundredths as u32;
    if !(MIN_STEP..=MAX_STEP).contains(&hundredths) || step < 0.0 {
        return Err("must be between 0.01 and 0.5".to_string());
    }
    Ok(hundredths)
}

/// Horizontal and vertical steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Steps {
    pub x: u32,
    pub y: u32,
}

/// The steps for cells showing `crop` (whole page when `None`) at no less than `width` x
/// `height` pixels, chosen per axis so a thin crop still gets lines along its short side.
pub fn choose_steps(
    crop: Option<Crop>,
    (width, height): (u32, u32),
    requested: Option<u32>,
) -> Steps {
    let crop = crop.unwrap_or(WHOLE);
    Steps {
        x: axis_step(crop.width, width, requested),
        y: axis_step(crop.height, height, requested),
    }
}

/// `requested`, or a step giving about ten intervals across `len` (a fraction), coarsened
/// until lines are at least `MIN_SPACING` apart on `pixels`.
fn axis_step(len: f64, pixels: u32, requested: Option<u32>) -> u32 {
    let span = len * 100.0;
    let px_per_hundredth = pixels as f64 / span;
    let dense_enough = |step: u32| span / step as f64 <= MAX_INTERVALS;
    let readable = |step: u32| step as f64 * px_per_hundredth >= MIN_SPACING as f64;
    let start = requested.unwrap_or_else(|| {
        NICE_STEPS
            .into_iter()
            .find(|&s| dense_enough(s))
            .unwrap_or(MAX_STEP)
    });
    if readable(start) {
        return start;
    }
    NICE_STEPS
        .into_iter()
        .filter(|&s| s > start)
        .find(|&s| readable(s))
        .unwrap_or(MAX_STEP.max(start))
}

/// Lines strictly inside the shown range `[start, start + len]` (fractions) mapped onto
/// `pixels`; lines that would fall on the cell border are left out.
pub fn ticks(start: f64, len: f64, pixels: u32, step: u32) -> Vec<Tick> {
    if pixels < 2 || len <= 0.0 || step == 0 {
        return Vec::new();
    }
    let (lo, hi) = (start * 100.0, (start + len) * 100.0);
    let first = (lo / step as f64).floor() as u32 + 1;
    (first..)
        .map(|k| k * step)
        .take_while(|&value| (value as f64) < hi)
        .filter_map(|value| {
            let pos = ((value as f64 - lo) / (hi - lo) * pixels as f64).round() as u32;
            (pos >= 1 && pos < pixels - 1).then_some(Tick { pos, value })
        })
        .collect()
}

/// ".1", ".35" or "1" for a value in hundredths.
pub fn tick_label(value: u32) -> String {
    match (value / 100, value % 100) {
        (whole, 0) => whole.to_string(),
        (0, frac) if frac % 10 == 0 => format!(".{}", frac / 10),
        (0, frac) => format!(".{frac:02}"),
        (whole, frac) => format!("{whole}.{frac:02}"),
    }
}

/// "0.1", or "0.05 × 0.01" (across × down) when the axes differ, as the header shows it.
pub fn steps_label(steps: Steps) -> String {
    let step = |hundredths: u32| {
        let label = tick_label(hundredths);
        if label.starts_with('.') {
            format!("0{label}")
        } else {
            label
        }
    };
    if steps.x == steps.y {
        step(steps.x)
    } else {
        format!("{} × {}", step(steps.x), step(steps.y))
    }
}

const WHOLE: Crop = Crop {
    x: 0.0,
    y: 0.0,
    width: 1.0,
    height: 1.0,
};

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn crop(x: f64, y: f64, width: f64, height: f64) -> Option<Crop> {
        Some(Crop {
            x,
            y,
            width,
            height,
        })
    }

    #[test]
    fn steps_parse_in_hundredths() {
        assert_eq!(parse_step("0.1"), Ok(10));
        assert_eq!(parse_step("0.05"), Ok(5));
        assert_eq!(parse_step(".5"), Ok(50));
        assert!(parse_step("0.025").is_err());
        assert!(parse_step("0").is_err());
        assert!(parse_step("0.6").is_err());
        assert!(parse_step("-0.1").is_err());
        assert!(parse_step("NaN").is_err());
        assert!(parse_step("ten").is_err());
    }

    fn same(step: u32) -> Steps {
        Steps { x: step, y: step }
    }

    #[test]
    fn automatic_step_follows_the_shown_range() {
        assert_eq!(choose_steps(None, (800, 800), None), same(10));
        assert_eq!(
            choose_steps(crop(0.2, 0.2, 0.2, 0.2), (800, 800), None),
            same(2)
        );
        assert_eq!(
            choose_steps(crop(0.0, 0.3, 0.5, 0.7), (500, 700), None),
            Steps { x: 5, y: 10 }
        );
    }

    #[test]
    fn dense_steps_are_coarsened() {
        assert_eq!(choose_steps(None, (200, 200), None), same(20));
        assert_eq!(choose_steps(None, (800, 800), Some(1)), same(5));
        assert_eq!(choose_steps(None, (20, 20), None), same(MAX_STEP));
        assert_eq!(choose_steps(None, (800, 800), Some(5)), same(5));
        assert_eq!(choose_steps(None, (572, 200), None), Steps { x: 10, y: 20 });
    }

    #[test]
    fn thin_crops_get_lines_along_both_sides() {
        let wide = crop(0.53, 0.76, 0.42, 0.1);
        let steps = choose_steps(wide, (1400, 333), None);
        assert!(ticks(0.53, 0.42, 1400, steps.x).len() >= 3);
        assert!(ticks(0.76, 0.1, 333, steps.y).len() >= 3, "{steps:?}");

        let tall = crop(0.5, 0.0, 0.05, 1.0);
        let steps = choose_steps(tall, (200, 2000), None);
        assert!(ticks(0.5, 0.05, 200, steps.x).len() >= 3, "{steps:?}");
        assert!(ticks(0.0, 1.0, 2000, steps.y).len() >= 3);
    }

    #[test]
    fn ticks_use_page_coordinates_inside_a_crop() {
        let values: Vec<u32> = ticks(0.07, 0.8, 800, 10).iter().map(|t| t.value).collect();
        assert_eq!(values, vec![10, 20, 30, 40, 50, 60, 70, 80]);
        let first = ticks(0.07, 0.8, 800, 10)[0];
        assert_eq!(first.pos, 30);
        let edges: Vec<u32> = ticks(0.0, 1.0, 1000, 10).iter().map(|t| t.value).collect();
        assert_eq!(edges, (1..10).map(|k| k * 10).collect::<Vec<_>>());
    }

    #[test]
    fn labels_drop_the_leading_zero() {
        assert_eq!(tick_label(10), ".1");
        assert_eq!(tick_label(35), ".35");
        assert_eq!(tick_label(5), ".05");
        assert_eq!(tick_label(100), "1");
        assert_eq!(steps_label(same(5)), "0.05");
        assert_eq!(steps_label(same(10)), "0.1");
        assert_eq!(steps_label(Steps { x: 5, y: 1 }), "0.05 × 0.01");
    }

    proptest! {
        #[test]
        fn ticks_stay_inside_the_cell(
            start in 0.0f64..1.0,
            len in 0.0001f64..1.0,
            pixels in 0u32..5000,
            step in 1u32..=50,
        ) {
            let len = len.min(1.0 - start).max(1e-6);
            for tick in ticks(start, len, pixels, step) {
                prop_assert!(tick.pos >= 1 && tick.pos < pixels.saturating_sub(1));
                prop_assert!(tick.value % step == 0);
                prop_assert!((tick.value as f64) > start * 100.0 - 1e-9);
                prop_assert!((tick.value as f64) < (start + len) * 100.0 + 1e-9);
            }
        }

        #[test]
        fn chosen_steps_are_in_range(
            w in 0.001f64..1.0,
            h in 0.001f64..1.0,
            px in (1u32..4000, 1u32..4000),
            requested in proptest::option::of(1u32..=50),
        ) {
            let steps = choose_steps(crop(0.0, 0.0, w, h), px, requested);
            for step in [steps.x, steps.y] {
                prop_assert!((MIN_STEP..=MAX_STEP).contains(&step));
                if let Some(r) = requested {
                    prop_assert!(step >= r);
                }
            }
        }
    }
}
