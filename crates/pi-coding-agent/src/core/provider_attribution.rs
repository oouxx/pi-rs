pub const OPENROUTER_HOST: &str = "openrouter.ai";
pub const NVIDIA_NIM_HOST: &str = "integrate.api.nvidia.com";
pub const CLOUDFLARE_API_HOST: &str = "api.cloudflare.com";
pub const CLOUDFLARE_AI_GATEWAY_HOST: &str = "gateway.ai.cloudflare.com";
pub const OPENCODE_HOST: &str = "opencode.ai";

#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub provider: String,
    pub base_url: String,
}

pub fn matches_host(base_url: &str, expected_host: &str) -> bool {
    match url::Url::parse(base_url) {
        Ok(url) => url.host_str() == Some(expected_host),
        Err(_) => false,
    }
}

fn is_openrouter_model(model: &ModelInfo) -> bool {
    model.provider == "openrouter" || model.base_url.contains(OPENROUTER_HOST)
}

fn is_nvidia_nim_model(model: &ModelInfo) -> bool {
    model.provider == "nvidia" || matches_host(&model.base_url, NVIDIA_NIM_HOST)
}

fn is_cloudflare_model(model: &ModelInfo) -> bool {
    model.provider == "cloudflare-workers-ai"
        || model.provider == "cloudflare-ai-gateway"
        || matches_host(&model.base_url, CLOUDFLARE_API_HOST)
        || matches_host(&model.base_url, CLOUDFLARE_AI_GATEWAY_HOST)
}

pub fn get_default_attribution_headers(
    model: &ModelInfo,
    has_telemetry: bool,
) -> Option<Vec<(String, String)>> {
    if !has_telemetry {
        return None;
    }

    if is_openrouter_model(model) {
        return Some(vec![
            ("HTTP-Referer".to_string(), "https://pi.dev".to_string()),
            ("X-OpenRouter-Title".to_string(), "pi".to_string()),
            (
                "X-OpenRouter-Categories".to_string(),
                "cli-agent".to_string(),
            ),
        ]);
    }

    if is_nvidia_nim_model(model) {
        return Some(vec![(
            "X-BILLING-INVOKE-ORIGIN".to_string(),
            "Pi".to_string(),
        )]);
    }

    if is_cloudflare_model(model) {
        return Some(vec![(
            "User-Agent".to_string(),
            "pi-coding-agent".to_string(),
        )]);
    }

    None
}

pub fn get_session_headers(
    model: &ModelInfo,
    session_id: Option<&str>,
) -> Option<Vec<(String, String)>> {
    let session_id = session_id?;
    if model.provider != "opencode"
        && model.provider != "opencode-go"
        && !matches_host(&model.base_url, OPENCODE_HOST)
    {
        return None;
    }

    Some(vec![
        ("x-opencode-session".to_string(), session_id.to_string()),
        ("x-opencode-client".to_string(), "pi".to_string()),
    ])
}

pub fn merge_provider_attribution_headers(
    model: &ModelInfo,
    has_telemetry: bool,
    session_id: Option<&str>,
    extra_headers: &[Vec<(String, String)>],
) -> Option<Vec<(String, String)>> {
    let mut merged = Vec::new();

    if let Some(session_headers) = get_session_headers(model, session_id) {
        merged.extend(session_headers);
    }

    if let Some(default_headers) = get_default_attribution_headers(model, has_telemetry) {
        merged.extend(default_headers);
    }

    for headers in extra_headers {
        for (k, v) in headers {
            // Later headers override earlier ones with same key
            merged.retain(|(ek, _)| ek != k);
            merged.push((k.clone(), v.clone()));
        }
    }

    if merged.is_empty() {
        None
    } else {
        Some(merged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_host() {
        assert!(matches_host("https://openrouter.ai/v1", "openrouter.ai"));
        assert!(!matches_host("https://openai.com/v1", "openrouter.ai"));
        assert!(!matches_host("invalid-url", "example.com"));
    }

    #[test]
    fn test_openrouter_attribution() {
        let model = ModelInfo {
            provider: "openrouter".into(),
            base_url: "https://openrouter.ai/v1".into(),
        };
        let headers = get_default_attribution_headers(&model, true);
        assert!(headers.is_some());
        let h = headers.unwrap();
        assert!(h.contains(&("HTTP-Referer".to_string(), "https://pi.dev".to_string())));
    }

    #[test]
    fn test_attribution_disabled_without_telemetry() {
        let model = ModelInfo {
            provider: "openrouter".into(),
            base_url: "https://openrouter.ai/v1".into(),
        };
        assert!(get_default_attribution_headers(&model, false).is_none());
    }

    #[test]
    fn test_session_headers_for_opencode() {
        let model = ModelInfo {
            provider: "opencode".into(),
            base_url: "https://opencode.ai/api".into(),
        };
        let headers = get_session_headers(&model, Some("sess_123"));
        assert!(headers.is_some());
        let h = headers.unwrap();
        assert!(h.contains(&("x-opencode-session".to_string(), "sess_123".to_string())));
    }

    #[test]
    fn test_merge_extra_overrides_default() {
        let model = ModelInfo {
            provider: "nvidia".into(),
            base_url: "https://integrate.api.nvidia.com/v1".into(),
        };
        let extra = vec![vec![(
            "X-BILLING-INVOKE-ORIGIN".to_string(),
            "Custom".to_string(),
        )]];
        let headers = merge_provider_attribution_headers(&model, true, None, &extra);
        assert!(headers.is_some());
        let h = headers.unwrap();
        assert!(h.contains(&("X-BILLING-INVOKE-ORIGIN".to_string(), "Custom".to_string())));
    }
}
