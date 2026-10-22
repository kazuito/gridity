use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::cli::{InputKind, TextArgs};
use crate::detect::detect;
use crate::error::{Reason, input};
use crate::output::{SCHEMA_VERSION, SourceInfo};
use crate::pagetext::{Content, PageSummary, page_text, summarize};
use crate::probe::quote;
use crate::render::{compact_ranges, source_info};
use crate::select::parse_pages;
use crate::source::Source;
use crate::source::pdf::{PdfSource, resolve_password};

/// `text --json`.
#[derive(Serialize, JsonSchema)]
pub struct TextReport {
    pub schema_version: u32,
    pub source: SourceInfo,
    pub pages: Vec<PageText>,
    /// Selected pages left out to keep the MCP result within `max_chars`; pass them as
    /// `pages` to read on. Never set by the CLI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_pages: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct PageText {
    pub page: usize,
    #[serde(flatten)]
    pub summary: PageSummary,
    pub text: String,
}

impl TextReport {
    /// Pages with formulas, figures or text that extraction cannot capture.
    pub fn visual_pages(&self) -> Vec<usize> {
        self.pages
            .iter()
            .filter(|p| matches!(p.summary.content, Content::Mixed | Content::Graphics))
            .map(|p| p.page)
            .collect()
    }

    /// Keeps the longest run of leading pages whose JSON fits in `max_chars`, at least one,
    /// and names the rest in `next_pages`. Returns the characters kept.
    pub fn fit(&mut self, max_chars: usize) -> Result<usize> {
        let sizes = self
            .pages
            .iter()
            .map(|page| Ok(serde_json::to_string(page)?.chars().count()))
            .collect::<Result<Vec<_>>>()?;
        let kept = pages_within(&sizes, max_chars);
        let rest: Vec<usize> = self.pages.drain(kept..).map(|p| p.page).collect();
        if !rest.is_empty() {
            self.next_pages = Some(compact_ranges(&rest));
        }
        Ok(json_array_len(&sizes[..kept]))
    }
}

/// How many leading items of these JSON sizes fit in a JSON array of `budget` characters,
/// at least one.
fn pages_within(sizes: &[usize], budget: usize) -> usize {
    let mut used = 0;
    let mut kept = 0;
    for &size in sizes {
        used += size + usize::from(kept > 0);
        if used > budget {
            break;
        }
        kept += 1;
    }
    kept.max(1).min(sizes.len())
}

fn json_array_len(sizes: &[usize]) -> usize {
    sizes.iter().sum::<usize>() + sizes.len().saturating_sub(1)
}

pub fn run(args: TextArgs) -> Result<()> {
    let json = args.json;
    let input = args.input.clone();
    let report = report(args)?;
    let visual = report.visual_pages();
    if !visual.is_empty() {
        eprintln!(
            "hint: p.{} show formulas, figures or text that extraction cannot capture; read them as images (gridity probe {} --per-page suggests the render commands)",
            compact_ranges(&visual),
            quote(&input.display().to_string())
        );
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    for (i, page) in report.pages.iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("--- p.{} · {} ---", page.page, page.summary.describe());
        if !page.text.is_empty() {
            println!("{}", page.text);
        }
    }
    Ok(())
}

pub fn report(args: TextArgs) -> Result<TextReport> {
    if detect(std::slice::from_ref(&args.input), None)? != InputKind::Pdf {
        return Err(input(
            Reason::WrongType,
            format!("{} is not a PDF", args.input.display()),
        )
        .with_hint("gridity text reads PDFs only; use gridity render for other inputs")
        .into());
    }
    let pdf = PdfSource::open(&args.input, resolve_password(&args.password)?.as_deref())?;
    let pages = match &args.pages {
        Some(spec) => parse_pages(spec, pdf.page_count())?,
        None => (1..=pdf.page_count()).collect(),
    };
    let scans = pdf.scan(&pages)?;
    let pages = pages
        .iter()
        .zip(&scans)
        .map(|(&page, scan)| PageText {
            page,
            summary: summarize(scan),
            text: page_text(&scan.glyphs),
        })
        .collect();
    Ok(TextReport {
        schema_version: SCHEMA_VERSION,
        source: source_info(&Source::Pdf(pdf), false),
        pages,
        next_pages: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_pages_that_fit_with_their_commas() {
        let sizes = [100, 200, 300];
        assert_eq!(pages_within(&sizes, 601), 2);
        assert_eq!(pages_within(&sizes, 602), 3);
        assert_eq!(pages_within(&sizes, 301), 2);
        assert_eq!(pages_within(&sizes, 300), 1);
        assert_eq!(pages_within(&sizes, 100_000), 3);
    }

    #[test]
    fn keeps_one_page_even_over_the_budget() {
        assert_eq!(pages_within(&[5000, 10], 1000), 1);
        assert_eq!(pages_within(&[5000], 1000), 1);
        assert_eq!(pages_within(&[], 1000), 0);
    }

    fn report(pages: &[usize], chars: usize) -> TextReport {
        TextReport {
            schema_version: SCHEMA_VERSION,
            source: SourceInfo::Pdf {
                path: "doc.pdf".to_string(),
                pages: 10,
                title: None,
                page_sizes: None,
            },
            pages: pages
                .iter()
                .map(|&page| PageText {
                    page,
                    summary: summarize(&Default::default()),
                    text: "x".repeat(chars),
                })
                .collect(),
            next_pages: None,
        }
    }

    #[test]
    fn next_pages_follow_the_selection_order() {
        let mut r = report(&[9, 3, 4, 5, 8], 1000);
        let one = serde_json::to_string(&r.pages[0]).unwrap().chars().count();
        assert_eq!(r.fit(one * 2 + 1).unwrap(), one * 2 + 1);
        assert_eq!(r.pages.iter().map(|p| p.page).collect::<Vec<_>>(), [9, 3]);
        let next = r.next_pages.as_deref().unwrap();
        assert_eq!(next, "4-5,8");
        assert_eq!(parse_pages(next, 10).unwrap(), [4, 5, 8]);

        let mut r = report(&[9, 3, 4], 1000);
        r.fit(one * 2).unwrap();
        assert_eq!(r.next_pages.as_deref(), Some("3-4"));
    }

    #[test]
    fn a_fitting_report_is_unchanged() {
        let mut r = report(&[1, 2, 3], 10);
        r.fit(100_000).unwrap();
        assert_eq!(r.pages.len(), 3);
        assert!(r.next_pages.is_none());
        assert!(!serde_json::to_string(&r).unwrap().contains("next_pages"));
    }
}
