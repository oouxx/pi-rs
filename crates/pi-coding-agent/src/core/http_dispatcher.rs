#![allow(
    clippy::module_name_repetitions,
    clippy::struct_field_names,
    clippy::too_many_lines,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::unnecessary_struct_initialization,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_closure,
    clippy::missing_const_for_fn,
    clippy::map_unwrap_or,
    clippy::option_if_let_else,
    clippy::manual_let_else,
    clippy::match_wildcard_for_single_variants,
    clippy::ref_option,
    clippy::redundant_clone,
    clippy::clone_on_ref_ptr,
    clippy::unnecessary_operation,
    clippy::unused_self,
    clippy::match_same_arms,
    clippy::needless_continue,
    clippy::items_after_statements,
    clippy::unnecessary_to_owned,
    clippy::needless_pass_by_value,
    clippy::uninlined_format_args,
    clippy::derive_partial_eq_without_eq,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::string_lit_as_bytes,
    clippy::trivially_copy_pass_by_ref,
    clippy::use_self,
    clippy::significant_drop_tightening,
    clippy::default_trait_access,
    clippy::iter_with_drain,
    clippy::if_not_else,
    clippy::explicit_iter_loop,
    clippy::assigning_clones,
    clippy::implicit_hasher,
    clippy::ignored_unit_patterns,
    clippy::missing_fields_in_debug,
    clippy::or_fun_call,
    clippy::too_long_first_doc_paragraph,
    clippy::manual_string_new,
    clippy::single_match_else,
    clippy::significant_drop_in_scrutinee,
    clippy::needless_collect,
    clippy::duplicated_attributes,
    clippy::similar_names,
    clippy::needless_raw_string_hashes,
    clippy::unnested_or_patterns,
)]
pub const DEFAULT_HTTP_IDLE_TIMEOUT_MS: u64 = 300_000;

pub const HTTP_IDLE_TIMEOUT_CHOICES: &[(u64, &str)] = &[
    (30_000, "30 sec"),
    (60_000, "1 min"),
    (120_000, "2 min"),
    (300_000, "5 min"),
    (0, "disabled"),
];

pub fn parse_http_idle_timeout_ms(value: &str) -> Option<u64> {
    let trimmed = value.trim();

    if trimmed.eq_ignore_ascii_case("disabled") {
        return Some(0);
    }

    if trimmed.is_empty() {
        return None;
    }

    let num: f64 = trimmed.parse().ok()?;
    if !num.is_finite() || num < 0.0 {
        return None;
    }
    Some(num.floor() as u64)
}

pub fn format_http_idle_timeout_ms(timeout_ms: u64) -> String {
    for &(ms, label) in HTTP_IDLE_TIMEOUT_CHOICES {
        if ms == timeout_ms {
            return label.to_string();
        }
    }
    format!("{} sec", timeout_ms / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_disabled() {
        assert_eq!(parse_http_idle_timeout_ms("disabled"), Some(0));
    }

    #[test]
    fn test_parse_number_string() {
        assert_eq!(parse_http_idle_timeout_ms("120000"), Some(120000));
    }

    #[test]
    fn test_parse_empty() {
        assert_eq!(parse_http_idle_timeout_ms(""), None);
    }

    #[test]
    fn test_parse_invalid() {
        assert_eq!(parse_http_idle_timeout_ms("not-a-number"), None);
    }

    #[test]
    fn test_format_known_choice() {
        assert_eq!(format_http_idle_timeout_ms(30000), "30 sec");
    }

    #[test]
    fn test_format_custom_value() {
        assert_eq!(format_http_idle_timeout_ms(45000), "45 sec");
    }

    #[test]
    fn test_format_disabled() {
        assert_eq!(format_http_idle_timeout_ms(0), "disabled");
    }
}
