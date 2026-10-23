//! `--label` and `export --name` templates: text with `{variable}` placeholders.

use crate::cli::InputKind;
use crate::error::{CliError, Reason, usage};

/// Checks that `template` (of `--label` or `--name`, named by `flag`) only uses `allowed`
/// variables.
pub fn validate_template(
    template: &str,
    flag: &str,
    allowed: &[&str],
    kind: InputKind,
) -> Result<(), CliError> {
    match placeholders(template).find(|name| !allowed.contains(name)) {
        None => Ok(()),
        Some(name) => Err(usage(
            Reason::LabelVariableNotForInput,
            format!(
                "{flag} variable {{{name}}} is not available for {} input",
                kind.name()
            ),
        )
        .with_hint(format!(
            "available: {}",
            allowed
                .iter()
                .map(|v| format!("{{{v}}}"))
                .collect::<Vec<_>>()
                .join(" ")
        ))),
    }
}

/// Replaces each `{name}` with `value(name)` in one pass, so substituted text is never
/// expanded again. Placeholders `value` does not know are kept as they are.
pub fn expand_template(template: &str, value: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some((before, name, after)) = next_placeholder(rest) {
        out.push_str(before);
        match value(name) {
            Some(text) => out.push_str(&text),
            None => {
                out.push('{');
                out.push_str(name);
                out.push('}');
            }
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

fn placeholders(template: &str) -> impl Iterator<Item = &str> {
    let mut rest = template;
    std::iter::from_fn(move || {
        let (_, name, after) = next_placeholder(rest)?;
        rest = after;
        Some(name)
    })
}

/// The text before the next `{name}`, the name, and the text after it.
fn next_placeholder(text: &str) -> Option<(&str, &str, &str)> {
    let open = text.find('{')?;
    let after_open = &text[open + 1..];
    let close = after_open.find('}')?;
    Some((
        &text[..open],
        &after_open[..close],
        &after_open[close + 1..],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_only_accept_variables_of_the_input_kind() {
        let check = |template: &str, kind: InputKind| {
            validate_template(template, "label", kind.label_vars(), kind)
        };
        assert!(check("p.{page} ({index}/{total})", InputKind::Pdf).is_ok());
        assert!(check("{time}", InputKind::Pdf).is_err());
        assert!(check("{frame}", InputKind::Video).is_err());
        assert!(check("{frame} {time}", InputKind::Anim).is_ok());
        assert!(check("no variables", InputKind::Images).is_ok());
        assert!(check("open { brace", InputKind::Images).is_ok());
    }

    #[test]
    fn default_names_only_use_their_kind_variables() {
        for kind in [
            InputKind::Pdf,
            InputKind::Video,
            InputKind::Anim,
            InputKind::Images,
        ] {
            assert!(validate_template(kind.default_name(), "name", kind.name_vars(), kind).is_ok());
            assert!(
                validate_template(kind.default_label(), "label", kind.label_vars(), kind).is_ok()
            );
        }
    }

    #[test]
    fn substituted_text_is_not_expanded_again() {
        let value = |name: &str| match name {
            "stem" => Some("a{page}".to_string()),
            "page" => Some("1".to_string()),
            _ => None,
        };
        assert_eq!(expand_template("{stem}-p{page}", value), "a{page}-p1");
        assert_eq!(expand_template("{other} {page", value), "{other} {page");
        assert_eq!(expand_template("", value), "");
    }

    proptest::proptest! {
        #[test]
        fn expanding_never_panics(template in "\\PC{0,40}") {
            let _ = expand_template(&template, |name| (name == "x").then(|| "{x}".to_string()));
            let _ = validate_template(&template, "label", &["x"], InputKind::Pdf);
        }
    }
}
