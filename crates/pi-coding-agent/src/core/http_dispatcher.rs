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
