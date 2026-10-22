use crate::error::{Reason, usage};

const TOPICS: &[(&str, &str)] = &[
    ("index", include_str!("../docs/guide/index.md")),
    ("recipes", include_str!("../docs/guide/recipes.md")),
    ("pdf", include_str!("../docs/guide/pdf.md")),
    ("text", include_str!("../docs/guide/text.md")),
    ("video", include_str!("../docs/guide/video.md")),
    ("images", include_str!("../docs/guide/images.md")),
    ("layout", include_str!("../docs/guide/layout.md")),
    ("output", include_str!("../docs/guide/output.md")),
];

pub fn run(topic: Option<&str>) -> anyhow::Result<()> {
    match topic.unwrap_or("index") {
        "all" => {
            let all: Vec<&str> = TOPICS.iter().map(|(_, text)| text.trim_end()).collect();
            println!("{}", all.join("\n\n---\n\n"));
        }
        name => {
            let (_, text) = TOPICS.iter().find(|(n, _)| *n == name).ok_or_else(|| {
                let names: Vec<&str> = TOPICS.iter().map(|(n, _)| *n).collect();
                usage(
                    Reason::UnknownTopic,
                    format!("unknown guide topic '{name}'"),
                )
                .with_hint(format!("topics: {}, all", names.join(", ")))
            })?;
            println!("{}", text.trim_end());
        }
    }
    Ok(())
}
