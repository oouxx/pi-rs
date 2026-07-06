//! HTML entity decoding utilities.
//!
//! Mirrors packages/coding-agent/src/utils/html.ts

use std::collections::HashMap;
use std::sync::LazyLock;

/// Result of decoding an HTML entity.
#[derive(Debug, Clone)]
pub struct DecodedHtmlEntity {
    pub text: String,
    pub length: usize,
}

/// Decode a Unicode code point to a string.
fn decode_code_point(code_point: u32) -> Option<String> {
    if code_point > 0x10FFFF || (0xD800..=0xDFFF).contains(&code_point) {
        return None;
    }
    char::from_u32(code_point).map(|c| c.to_string())
}

static NAMED_ENTITIES: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    m.insert("amp", "&");
    m.insert("lt", "<");
    m.insert("gt", ">");
    m.insert("quot", "\"");
    m.insert("apos", "'");
    m.insert("nbsp", " ");
    m.insert("copy", "©");
    m.insert("reg", "®");
    m.insert("trade", "™");
    m.insert("mdash", "—");
    m.insert("ndash", "–");
    m.insert("hellip", "…");
    m
});

/// Decode a single HTML entity (without the leading `&` and trailing `;`).
/// E.g. `decode_html_entity("amp")` returns `Some("&")`.
pub fn decode_html_entity(entity: &str) -> Option<String> {
    // Named entities
    if let Some(&ch) = NAMED_ENTITIES.get(entity) {
        return Some(ch.to_string());
    }

    // Hex entities: &#xNN; or &#XNN;
    if entity.starts_with("#x") || entity.starts_with("#X") {
        let hex_str = &entity[2..];
        let code = u32::from_str_radix(hex_str, 16).ok()?;
        return decode_code_point(code);
    }

    // Decimal entities: &#NN;
    if entity.starts_with('#') {
        let dec_str = &entity[1..];
        let code = dec_str.parse::<u32>().ok()?;
        return decode_code_point(code);
    }

    None
}

/// Decode all HTML entities in a string.
pub fn decode_html_entities(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.char_indices().peekable();

    while let Some((_, c)) = chars.next() {
        if c == '&' {
            // Collect entity name until ;
            let mut entity = String::new();
            let mut valid = false;
            while let Some(&(_, next)) = chars.peek() {
                if next == ';' {
                    chars.next(); // consume ;
                    valid = true;
                    break;
                }
                if !next.is_alphanumeric() && next != '#' && next != 'x' && next != 'X' {
                    break;
                }
                entity.push(next);
                chars.next();
            }
            if valid {
                if let Some(decoded) = decode_html_entity(&entity) {
                    result.push_str(&decoded);
                    continue;
                }
            }
            // Fall through: keep original if decode failed
            result.push('&');
            result.push_str(&entity);
            if valid {
                result.push(';');
            }
        } else {
            result.push(c);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_named() {
        assert_eq!(decode_html_entity("amp"), Some("&".into()));
        assert_eq!(decode_html_entity("lt"), Some("<".into()));
        assert_eq!(decode_html_entity("gt"), Some(">".into()));
    }

    #[test]
    fn test_decode_hex() {
        assert_eq!(decode_html_entity("#x26"), Some("&".into()));
        assert_eq!(decode_html_entity("#X26"), Some("&".into()));
    }

    #[test]
    fn test_decode_decimal() {
        assert_eq!(decode_html_entity("#38"), Some("&".into()));
    }

    #[test]
    fn test_decode_entities_in_string() {
        let result = decode_html_entities("a &amp; b &lt; c &gt; d");
        assert_eq!(result, "a & b < c > d");
    }

    #[test]
    fn test_decode_no_entities() {
        assert_eq!(decode_html_entities("plain text"), "plain text");
    }

    #[test]
    fn test_decode_invalid_entity() {
        let result = decode_html_entities("&unknown; text");
        assert_eq!(result, "&unknown; text");
    }

    #[test]
    fn test_decode_nbsp() {
        assert_eq!(decode_html_entity("nbsp"), Some(" ".into()));
    }
}
