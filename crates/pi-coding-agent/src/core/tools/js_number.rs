//! JS-compatible number handling for tool parameters.
//!
//! TS tools declare `Type.Number()` for `offset`/`limit`, so fractional and
//! negative values reach array comparisons and `slice()`. Rust's `as_u64()`
//! would reject them and silently fall back to the default.

use serde_json::Value;

/// Read a JSON number the way TS sees it (`NaN`/non-number → `None`).
pub(crate) fn js_number(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64)
}

/// Format a number like JS `String(value)` (`2.5` → `"2.5"`, `500.0` → `"500"`).
pub(crate) fn js_number_display(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// JS `Array.prototype.slice` index conversion (ToIntegerOrInfinity): truncate
/// toward zero; negatives clamp to 0 for our forward slices.
pub(crate) fn to_slice_index(value: f64) -> usize {
    if !value.is_finite() || value <= 0.0 {
        0
    } else {
        value.trunc() as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn js_number_accepts_fractional_and_negative() {
        assert_eq!(js_number(Some(&json!(2.5))), Some(2.5));
        assert_eq!(js_number(Some(&json!(-1))), Some(-1.0));
        assert_eq!(js_number(Some(&json!("2"))), None);
    }

    #[test]
    fn js_number_display_matches_js_string() {
        assert_eq!(js_number_display(500.0), "500");
        assert_eq!(js_number_display(2.5), "2.5");
        assert_eq!(js_number_display(-1.0), "-1");
    }

    #[test]
    fn slice_index_truncates_toward_zero() {
        assert_eq!(to_slice_index(2.9), 2);
        assert_eq!(to_slice_index(-1.0), 0);
        assert_eq!(to_slice_index(f64::NAN), 0);
    }
}
