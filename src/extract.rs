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
    })
}
