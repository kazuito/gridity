use crate::error::{CliError, Reason, usage};

const TOPICS: &[(&str, &str)] = &[
    ("index", include_str!("../docs/guide/index.md")),
    ("recipes", include_str!("../docs/guide/recipes.md")),
    ("pdf", include_str!("../docs/guide/pdf.md")),
    ("text", include_str!("../docs/guide/text.md")),
    ("video", include_str!("../docs/guide/video.md")),
    ("images", include_str!("../docs/guide/images.md")),
    ("layout", include_str!("../docs/guide/layout.md")),
    ("output", include_str!("../docs/guide/output.md")),
    ("mcp", include_str!("../docs/guide/mcp.md")),
];

/// Every topic `text` accepts.
pub fn topic_names() -> Vec<&'static str> {
    let mut names: Vec<&str> = TOPICS.iter().map(|(name, _)| *name).collect();
    names.push("all");
    names
}

pub fn run(topic: Option<&str>) -> anyhow::Result<()> {
    println!("{}", text(topic)?);
    Ok(())
}

/// The Markdown of one topic, the index when `None`, or every topic for "all".
pub fn text(topic: Option<&str>) -> Result<String, CliError> {
    match topic.unwrap_or("index") {
        "all" => {
            let all: Vec<&str> = TOPICS.iter().map(|(_, text)| text.trim_end()).collect();
            Ok(all.join("\n\n---\n\n"))
        }
        name => {
            let (_, text) = TOPICS.iter().find(|(n, _)| *n == name).ok_or_else(|| {
                usage(
                    Reason::UnknownTopic,
                    format!("unknown guide topic '{name}'"),
                )
                .with_hint(format!("topics: {}", topic_names().join(", ")))
            })?;
            Ok(text.trim_end().to_string())
        }
    }
}
