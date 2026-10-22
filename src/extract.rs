use anyhow::Result;
use serde::Serialize;

use crate::cli::{InputKind, TextArgs};
use crate::detect::detect;
use crate::error::input;
use crate::pagetext::{PageSummary, assemble_text, summarize};
use crate::render::source_info;
use crate::select::parse_pages;
use crate::source::Source;
use crate::source::pdf::PdfSource;

#[derive(Serialize)]
struct PageText {
    page: usize,
    #[serde(flatten)]
    summary: PageSummary,
    text: String,
}

pub fn run(args: TextArgs) -> Result<()> {
    if detect(std::slice::from_ref(&args.input), None)? != InputKind::Pdf {
        return Err(input(format!("{} is not a PDF", args.input.display()))
            .with_hint("gridity text reads PDFs only; use gridity render for other inputs")
            .into());
    }
    let pdf = PdfSource::open(&args.input, args.password.as_deref())?;
    let pages = match &args.pages {
        Some(spec) => parse_pages(spec, pdf.page_count())?,
        None => (1..=pdf.page_count()).collect(),
    };
    let scans = pdf.scan(&pages)?;
    let texts: Vec<PageText> = pages
        .iter()
        .zip(&scans)
        .map(|(&page, scan)| PageText {
            page,
            summary: summarize(scan),
            text: assemble_text(&scan.glyphs),
        })
        .collect();

    if args.json {
        let report = serde_json::json!({
            "source": source_info(&Source::Pdf(pdf), false),
            "pages": texts,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    for (i, page) in texts.iter().enumerate() {
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
