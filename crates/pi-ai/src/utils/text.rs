//! Text helpers (match TS `packages/ai/src/utils/text.ts`).

use crate::types::ContentBlock;

/// Number of UTF-16 code units (JS `String.prototype.length`).
#[must_use]
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// JS `String.prototype.slice(0, max_units)` for a Rust `&str`: at most
/// `max_units` UTF-16 code units, never splitting a surrogate pair.
///
/// JS can produce a lone surrogate when the cut lands inside an astral
/// character; Rust `String` cannot represent that. Dropping the whole
/// character is the closest safe behaviour (same as v1.5.0 `sliceHead`).
#[must_use]
pub fn truncate_utf16(text: &str, max_units: usize) -> &str {
    let mut units = 0;
    for (index, ch) in text.char_indices() {
        let width = ch.len_utf16();
        if units + width > max_units {
            return &text[..index];
        }
        units += width;
    }
    text
}

/// Rounds a non-negative, finite `f64` to the nearest tenth using the
/// ECMAScript `toFixed(1)` rule: an exact tie picks the larger tenth.
///
/// The IEEE-754 decomposition (`value == mantissa * 2^exponent`) is used so the
/// tie decision never depends on a second, rounded floating-point multiply.
#[must_use]
pub fn round_one_decimal(value: f64) -> u64 {
    debug_assert!(value.is_finite() && value >= 0.0);
    let bits = value.to_bits();
    let exponent_bits = ((bits >> 52) & 0x7ff) as i32;
    let fraction = bits & ((1u64 << 52) - 1);
    let (mantissa, exponent) = if exponent_bits == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1u64 << 52), exponent_bits - 1075)
    };
    // `value * 10 == scaled * 2^exponent`, held exactly in a u128.
    let scaled = u128::from(mantissa) * 10;
    if exponent >= 0 {
        let shift = u32::try_from(exponent).unwrap_or(u32::MAX);
        if shift >= u128::BITS {
            return u64::MAX;
        }
        return (scaled << shift) as u64;
    }
    let shift = (-exponent) as u32;
    if shift >= 127 {
        // value * 10 < 2^-70: rounds to zero tenths.
        return 0;
    }
    // floor(value * 10 + 0.5) == (2*scaled + 2^shift) / 2^(shift+1)
    let numerator = scaled * 2 + (1u128 << shift);
    (numerator >> (shift + 1)) as u64
}

/// JS `toFixed(digits)` for finite values: exact ties round away from zero,
/// unlike Rust's default half-to-even formatting.
#[must_use]
pub fn to_fixed(value: f64, digits: usize) -> String {
    let factor = 10f64.powi(digits as i32);
    format!("{:.*}", digits, (value * factor).round() / factor)
}

/// Extract and join text from message content (match TS `contentText`).
///
/// Only `text` blocks are included; thinking/tool-call/image blocks are skipped.
/// A plain string is returned unchanged.
pub fn content_text(content: &[ContentBlock], separator: &str) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(separator)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_text_joins_text_blocks() {
        let content = vec![
            ContentBlock::Text {
                text: "hello".into(),
                text_signature: None,
            },
            ContentBlock::Text {
                text: "world".into(),
                text_signature: None,
            },
        ];
        assert_eq!(content_text(&content, "\n"), "hello\nworld");
    }

    #[test]
    fn test_content_text_skips_non_text_blocks() {
        let content = vec![
            ContentBlock::Text {
                text: "only".into(),
                text_signature: None,
            },
            ContentBlock::Thinking {
                thinking: "hidden".into(),
                thinking_signature: None,
                redacted: None,
            },
        ];
        assert_eq!(content_text(&content, "\n"), "only");
    }

    #[test]
    fn test_content_text_empty() {
        assert_eq!(content_text(&[], "\n"), "");
    }

    #[test]
    fn test_utf16_len_counts_js_units() {
        assert_eq!(utf16_len("abc"), 3);
        assert_eq!(utf16_len("你好"), 2);
        // Astral characters are two UTF-16 code units, one scalar value.
        assert_eq!(utf16_len("😀"), 2);
    }

    #[test]
    fn test_round_one_decimal_matches_js_to_fixed() {
        // 1280 / 1024 == 1.25 exactly. JS toFixed(1) picks the larger tenth
        // ("1.3"); Rust's `{:.1}` would round half to even ("1.2").
        assert_eq!(round_one_decimal(1.25), 13);
        assert_eq!(round_one_decimal(6.25), 63);
        assert_eq!(round_one_decimal(1.15), 11);
        assert_eq!(round_one_decimal(1.0), 10);
        assert_eq!(round_one_decimal(0.0), 0);
    }

    #[test]
    fn test_truncate_utf16_never_splits_surrogate_pair() {
        assert_eq!(truncate_utf16("abc", 10), "abc");
        assert_eq!(truncate_utf16("你好世界", 2), "你好");
        assert_eq!(truncate_utf16("a😀b", 3), "a😀");
        // A cut at 2 units would land inside the emoji, so it is dropped.
        assert_eq!(truncate_utf16("a😀b", 2), "a");
        assert_eq!(truncate_utf16("😀", 0), "");
    }
}
