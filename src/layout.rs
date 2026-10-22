use serde::Serialize;

use crate::cli::Preset;
use crate::error::{CliError, usage};

/// Claude splits images into 28x28 px patches; one patch costs one visual token.
const PATCH: u32 = 28;
/// Largest value `--max-sheets` accepts.
pub const MAX_SHEETS: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_edge: u32,
    pub max_tokens: u32,
}

impl Limits {
    pub fn new(preset: Preset, max_edge: Option<u32>, max_tokens: Option<u32>) -> Limits {
        let base = match preset {
            Preset::Compat => Limits {
                max_edge: 1568,
                max_tokens: 1568,
            },
            Preset::Hires => Limits {
                max_edge: 2576,
                max_tokens: 4784,
            },
        };
        Limits {
            max_edge: max_edge.unwrap_or(base.max_edge),
            max_tokens: max_tokens.unwrap_or(base.max_tokens),
        }
    }
}

pub fn token_cost(width: u32, height: u32) -> u32 {
    width.div_ceil(PATCH) * height.div_ceil(PATCH)
}

/// Same check Claude uses to decide whether an image must be downscaled.
pub fn fits(width: u32, height: u32, limits: Limits) -> bool {
    width.div_ceil(PATCH) * PATCH <= limits.max_edge
        && height.div_ceil(PATCH) * PATCH <= limits.max_edge
        && token_cost(width, height) <= limits.max_tokens
}

/// Fixed space around the cells: outer padding, gaps, header band and the label strip under each cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chrome {
    pub padding: u32,
    pub gap: u32,
    pub header: u32,
    pub label: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Grid {
    pub cols: u32,
    pub rows: u32,
    pub cell_width: u32,
    pub cell_height: u32,
}

impl Grid {
    pub fn sheet_size(&self, chrome: Chrome) -> (u32, u32) {
        let width = 2 * chrome.padding + self.cols * self.cell_width + (self.cols - 1) * chrome.gap;
        let height = chrome.header
            + 2 * chrome.padding
            + self.rows * (self.cell_height + chrome.label)
            + (self.rows - 1) * chrome.gap;
        (width, height)
    }

    /// Top-left corner of the image slot for the cell at `index` (row-major).
    pub fn cell_origin(&self, chrome: Chrome, index: usize) -> (u32, u32) {
        let col = index as u32 % self.cols;
        let row = index as u32 / self.cols;
        let x = chrome.padding + col * (self.cell_width + chrome.gap);
        let y =
            chrome.header + chrome.padding + row * (self.cell_height + chrome.label + chrome.gap);
        (x, y)
    }

    fn long_edge(&self) -> u32 {
        self.cell_width.max(self.cell_height)
    }

    /// Shrinks cells (keeping their aspect ratio) so they are no larger than `max`.
    fn shrink_to(self, max: (u32, u32)) -> Grid {
        let scale = (max.0 as f64 / self.cell_width as f64)
            .min(max.1 as f64 / self.cell_height as f64)
            .min(1.0);
        Grid {
            cell_width: ((self.cell_width as f64 * scale).round() as u32).max(1),
            cell_height: ((self.cell_height as f64 * scale).round() as u32).max(1),
            ..self
        }
    }
}

/// Finds the grid with the largest cells that holds `count` items of the given aspect ratio
/// (width / height) while the whole sheet stays within `limits`. Cells are capped at `max_cell`;
/// among equally large cells the grid with fewer empty slots, then the squarer sheet, wins.
pub fn best_grid(
    count: usize,
    aspect: f64,
    limits: Limits,
    chrome: Chrome,
    cols: Option<u32>,
    rows: Option<u32>,
    max_cell: Option<(u32, u32)>,
) -> Option<Grid> {
    let count = count.max(1) as u32;
    let candidates: Vec<u32> = match (cols, rows) {
        (Some(c), _) => vec![c],
        (None, Some(r)) => vec![count.div_ceil(r)],
        (None, None) => (1..=count).collect(),
    };

    let mut best: Option<(Grid, Rank)> = None;
    for cols in candidates {
        let rows = rows.unwrap_or_else(|| count.div_ceil(cols));
        if cols == 0 || rows == 0 || cols * rows < count {
            continue;
        }
        let Some(mut grid) = largest_cell(cols, rows, aspect, limits, chrome) else {
            continue;
        };
        if let Some(max) = max_cell {
            grid = grid.shrink_to(max);
        }
        let rank = Rank {
            area: grid.cell_width as u64 * grid.cell_height as u64,
            empty: cols * rows - count,
            skew: {
                let (w, h) = grid.sheet_size(chrome);
                (w as f64 / h as f64).ln().abs()
            },
        };
        if best.as_ref().is_none_or(|(_, current)| rank.beats(current)) {
            best = Some((grid, rank));
        }
    }
    best.map(|(grid, _)| grid)
}

struct Rank {
    area: u64,
    empty: u32,
    skew: f64,
}

impl Rank {
    fn beats(&self, other: &Rank) -> bool {
        (self.area, std::cmp::Reverse(self.empty)) > (other.area, std::cmp::Reverse(other.empty))
            || (self.area == other.area && self.empty == other.empty && self.skew < other.skew)
    }
}

fn largest_cell(cols: u32, rows: u32, aspect: f64, limits: Limits, chrome: Chrome) -> Option<Grid> {
    let grid_for = |cell_width: u32| Grid {
        cols,
        rows,
        cell_width,
        cell_height: ((cell_width as f64 / aspect).round() as u32).max(1),
    };
    let fits_width = |cell_width: u32| {
        let (w, h) = grid_for(cell_width).sheet_size(chrome);
        fits(w, h, limits)
    };

    if !fits_width(1) {
        return None;
    }
    // Bounded so the sheet size arithmetic cannot overflow even for absurd limits.
    let (mut lo, mut hi) = (1, limits.max_edge.min(u16::MAX as u32) + 1);
    while lo + 1 < hi {
        let mid = lo + (hi - lo) / 2;
        if fits_width(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some(grid_for(lo))
}

#[derive(Debug, Clone, Copy)]
pub struct PlanOptions {
    pub per_sheet: Option<usize>,
    pub cols: Option<u32>,
    pub rows: Option<u32>,
    /// Smallest allowed long edge of a cell in pixels.
    pub min_cell: u32,
    /// Largest useful cell size: raster sources are never upscaled, so bigger cells only add empty space.
    pub max_cell: Option<(u32, u32)>,
    pub max_sheets: usize,
}

impl PlanOptions {
    fn effective_min_cell(&self) -> u32 {
        match self.max_cell {
            Some((w, h)) => self.min_cell.min(w.max(h)),
            None => self.min_cell,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SheetPlan {
    pub start: usize,
    pub len: usize,
    pub grid: Grid,
}

/// How many items fit on one sheet without cells shrinking below `min_cell`.
/// A fixed `--cols` or `--rows` always fills at least one full row or column.
pub fn capacity(
    aspect: f64,
    limits: Limits,
    chrome: Chrome,
    opts: &PlanOptions,
    upper: usize,
) -> usize {
    if let Some(n) = opts.per_sheet {
        return n.max(1);
    }
    if let (Some(c), Some(r)) = (opts.cols, opts.rows) {
        return (c.saturating_mul(r)).max(1) as usize;
    }
    // With one axis fixed, every count up to the next full row or column gives the same grid.
    let step = opts.cols.or(opts.rows).unwrap_or(1).max(1) as usize;
    let mut cap = step;
    for k in (step..=upper.max(1).div_ceil(step) * step).step_by(step) {
        match best_grid(
            k,
            aspect,
            limits,
            chrome,
            opts.cols,
            opts.rows,
            opts.max_cell,
        ) {
            Some(grid) if grid.long_edge() >= opts.effective_min_cell() => cap = k,
            _ => break,
        }
    }
    cap
}

/// Number of sheets for `count` items at `cap` per sheet, or an error when that exceeds `max_sheets`.
pub fn check_sheets(count: usize, cap: usize, max_sheets: usize) -> Result<usize, CliError> {
    let cap = cap.max(1);
    let sheets = count.div_ceil(cap).max(1);
    if sheets > max_sheets {
        return Err(usage(format!(
            "{count} items need {sheets} sheets at {cap} per sheet, more than --max-sheets {max_sheets}"
        ))
        .with_hint(if sheets <= MAX_SHEETS {
            format!(
                "narrow the selection (--pages, --start/--end, --count, --every), use --detail low, or pass --max-sheets {sheets}"
            )
        } else {
            "narrow the selection (--pages, --start/--end, a smaller --count or a longer --every) or use --detail low".to_string()
        }));
    }
    Ok(sheets)
}

/// Splits `count` items into evenly filled sheets.
pub fn plan(
    count: usize,
    aspect: f64,
    limits: Limits,
    chrome: Chrome,
    opts: &PlanOptions,
) -> Result<Vec<SheetPlan>, CliError> {
    let cap = capacity(aspect, limits, chrome, opts, count);
    let sheets = check_sheets(count, cap, opts.max_sheets)?;

    let base = count / sheets;
    let extra = count % sheets;
    let mut result = Vec::with_capacity(sheets);
    let mut start = 0;
    for i in 0..sheets {
        let len = base + usize::from(i < extra);
        let grid = best_grid(
            len,
            aspect,
            limits,
            chrome,
            opts.cols,
            opts.rows,
            opts.max_cell,
        )
        .ok_or_else(|| {
            usage(format!(
                "{len} items do not fit on one sheet within the size limits"
            ))
            .with_hint(
                "lower --per-sheet, --cols/--rows, --gap or --padding, or use --preset hires",
            )
        })?;
        result.push(SheetPlan { start, len, grid });
        start += len;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPAT: Limits = Limits {
        max_edge: 1568,
        max_tokens: 1568,
    };
    const CHROME: Chrome = Chrome {
        padding: 12,
        gap: 8,
        header: 32,
        label: 22,
    };

    fn options(min_cell: u32) -> PlanOptions {
        PlanOptions {
            per_sheet: None,
            cols: None,
            rows: None,
            min_cell,
            max_cell: None,
            max_sheets: 100,
        }
    }

    #[test]
    fn fits_matches_claude_limits() {
        assert!(fits(1568, 784, COMPAT));
        assert!(!fits(1569, 100, COMPAT));
        assert!(!fits(1568, 812, COMPAT));
        assert!(fits(1092, 1092, COMPAT));
        assert_eq!(token_cost(1092, 1092), 1521);
    }

    #[test]
    fn grids_never_exceed_limits() {
        for limits in [
            COMPAT,
            Limits {
                max_edge: 2576,
                max_tokens: 4784,
            },
        ] {
            for aspect in [0.2, 1.0 / 1.414, 1.0, 4.0 / 3.0, 16.0 / 9.0, 5.0] {
                for n in 1..=40 {
                    let grid = best_grid(n, aspect, limits, CHROME, None, None, None).unwrap();
                    let (w, h) = grid.sheet_size(CHROME);
                    assert!(fits(w, h, limits), "n={n} aspect={aspect} -> {w}x{h}");
                    assert!(grid.cols * grid.rows >= n as u32);
                }
            }
        }
    }

    #[test]
    fn a4_pages_four_per_sheet() {
        let grid = best_grid(4, 1.0 / 1.414, COMPAT, CHROME, None, None, None).unwrap();
        assert_eq!((grid.cols, grid.rows), (2, 2));
        assert!(grid.cell_height >= 600);
    }

    #[test]
    fn fixed_columns_are_respected() {
        let grid = best_grid(7, 1.0, COMPAT, CHROME, Some(3), None, None).unwrap();
        assert_eq!((grid.cols, grid.rows), (3, 3));
    }

    #[test]
    fn splits_evenly() {
        let mut opts = options(0);
        opts.per_sheet = Some(8);
        let sheets = plan(10, 1.0, COMPAT, CHROME, &opts).unwrap();
        assert_eq!(sheets.iter().map(|s| s.len).collect::<Vec<_>>(), vec![5, 5]);
        assert_eq!(sheets[1].start, 5);
    }

    #[test]
    fn capacity_follows_min_cell() {
        let a4 = 1.0 / 1.414;
        assert_eq!(capacity(a4, COMPAT, CHROME, &options(600), 50), 4);
        assert!(capacity(a4, COMPAT, CHROME, &options(400), 50) >= 9);
        assert_eq!(capacity(a4, COMPAT, CHROME, &options(5000), 50), 1);
    }

    #[test]
    fn fixed_columns_fill_at_least_one_row() {
        let mut opts = options(330);
        opts.cols = Some(5);
        opts.max_cell = Some((960, 600));
        let sheets = plan(5, 960.0 / 600.0, COMPAT, CHROME, &opts).unwrap();
        assert_eq!(sheets.len(), 1);
        assert_eq!((sheets[0].grid.cols, sheets[0].grid.rows), (5, 1));

        let a4 = 1.0 / 1.414;
        let mut opts = options(580);
        opts.cols = Some(4);
        assert!(capacity(a4, COMPAT, CHROME, &opts, 12) >= 4);
        let sheets = plan(12, a4, COMPAT, CHROME, &opts).unwrap();
        assert!(sheets.len() <= 3);
        assert!(sheets.iter().all(|s| s.grid.cols == 4));
    }

    #[test]
    fn fixed_rows_fill_at_least_one_column() {
        let mut opts = options(5000);
        opts.rows = Some(3);
        assert_eq!(capacity(1.0, COMPAT, CHROME, &opts, 50), 3);
        let mut opts = options(200);
        opts.rows = Some(2);
        let cap = capacity(1.0, COMPAT, CHROME, &opts, 50);
        assert!(cap >= 4 && cap.is_multiple_of(2), "cap={cap}");
    }

    #[test]
    fn huge_max_edge_does_not_overflow() {
        let limits = Limits {
            max_edge: u32::MAX,
            max_tokens: 1568,
        };
        let grid = best_grid(4, 1.0, limits, CHROME, None, None, None).unwrap();
        let (w, h) = grid.sheet_size(CHROME);
        assert!(fits(w, h, limits));
        assert!(grid.cell_width > 100);
    }

    #[test]
    fn hint_only_suggests_accepted_sheet_counts() {
        let err = check_sheets(20_000_000, 12, 10).unwrap_err();
        assert!(!err.hint.unwrap().contains("--max-sheets"));
        let err = check_sheets(120, 12, 5).unwrap_err();
        assert!(err.hint.unwrap().contains("--max-sheets 10"));
    }

    #[test]
    fn small_sources_get_small_cells() {
        let mut opts = options(400);
        opts.max_cell = Some((320, 240));
        assert!(
            capacity(4.0 / 3.0, COMPAT, CHROME, &opts, 50)
                > capacity(4.0 / 3.0, COMPAT, CHROME, &options(400), 50)
        );
        let sheets = plan(4, 4.0 / 3.0, COMPAT, CHROME, &opts).unwrap();
        let grid = sheets[0].grid;
        assert_eq!(
            (grid.cell_width, grid.cell_height, grid.cols, grid.rows),
            (320, 240, 2, 2)
        );
    }

    #[test]
    fn too_many_sheets_is_an_error() {
        let mut opts = options(600);
        opts.max_sheets = 2;
        let err = plan(42, 1.0 / 1.414, COMPAT, CHROME, &opts).unwrap_err();
        assert!(err.message.contains("11 sheets"));
        assert!(err.hint.unwrap().contains("--max-sheets 11"));
    }
}
