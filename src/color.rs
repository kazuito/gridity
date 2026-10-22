use image::Rgba;

/// Parses `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa` or a few common color names.
pub fn parse_color(s: &str) -> Result<Rgba<u8>, String> {
    let named = match s.to_ascii_lowercase().as_str() {
        "white" => Some([255, 255, 255, 255]),
        "black" => Some([0, 0, 0, 255]),
        "gray" | "grey" => Some([128, 128, 128, 255]),
        "red" => Some([220, 38, 38, 255]),
        "transparent" => Some([0, 0, 0, 0]),
        _ => None,
    };
    if let Some(c) = named {
        return Ok(Rgba(c));
    }

    let hex = s.strip_prefix('#').unwrap_or(s);
    let digits: Vec<u8> = hex
        .chars()
        .map(|c| c.to_digit(16).map(|d| d as u8))
        .collect::<Option<_>>()
        .ok_or_else(|| format!("invalid color '{s}' (expected #rrggbb, #rrggbbaa or a name)"))?;

    let expand = |d: u8| d * 17;
    let pair = |i: usize| digits[i] * 16 + digits[i + 1];
    match digits.len() {
        3 => Ok(Rgba([
            expand(digits[0]),
            expand(digits[1]),
            expand(digits[2]),
            255,
        ])),
        4 => Ok(Rgba([
            expand(digits[0]),
            expand(digits[1]),
            expand(digits[2]),
            expand(digits[3]),
        ])),
        6 => Ok(Rgba([pair(0), pair(2), pair(4), 255])),
        8 => Ok(Rgba([pair(0), pair(2), pair(4), pair(6)])),
        _ => Err(format!(
            "invalid color '{s}' (expected #rrggbb, #rrggbbaa or a name)"
        )),
    }
}

pub fn is_dark(c: Rgba<u8>) -> bool {
    let [r, g, b, _] = c.0;
    let luminance = 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32;
    luminance < 128.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_and_names() {
        assert_eq!(parse_color("#fff").unwrap(), Rgba([255, 255, 255, 255]));
        assert_eq!(parse_color("#00000080").unwrap(), Rgba([0, 0, 0, 128]));
        assert_eq!(
            parse_color("1e293b").unwrap(),
            Rgba([0x1e, 0x29, 0x3b, 255])
        );
        assert_eq!(parse_color("White").unwrap(), Rgba([255, 255, 255, 255]));
        assert!(parse_color("#12345").is_err());
        assert!(parse_color("nope").is_err());
    }

    proptest::proptest! {
        #[test]
        fn parsing_never_panics(text in "\\PC{0,24}") {
            let _ = parse_color(&text);
        }
    }
}
