//! JS-compatible text measurements shared by the TUI components.
//!
//! v0.87.1 measures editor text with `String.length` (UTF-16 code units), not
//! Unicode scalar values. Astral characters (emoji, rare CJK) count as two.

/// UTF-16 code-unit length (match JS `String.length`).
pub(crate) fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// JS `toFixed(digits)` for non-negative values: exact ties round away from
/// zero, unlike Rust's default half-to-even formatting.
pub(crate) fn to_fixed(value: f64, digits: usize) -> String {
    let factor = 10f64.powi(digits as i32);
    format!("{:.*}", digits, (value * factor).round() / factor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_len_counts_astral_characters_as_two() {
        assert_eq!(utf16_len("abc"), 3);
        assert_eq!(utf16_len("😀"), 2);
        assert_eq!(utf16_len("a😀b"), 4);
    }

    #[test]
    fn to_fixed_rounds_ties_away_from_zero() {
        assert_eq!(to_fixed(1.25, 1), "1.3");
        assert_eq!(to_fixed(1.35, 1), "1.4");
        assert_eq!(to_fixed(0.125, 2), "0.13");
    }
}
