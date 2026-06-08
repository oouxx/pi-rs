use std::path::Path;

const UNKNOWN_PROVIDER: &str = "unknown";

pub fn get_provider_login_help(docs_path: &str) -> String {
    format!(
        "Use /login to log into a provider via OAuth or API key. See:\n  {}\n  {}",
        Path::new(docs_path).join("providers.md").display(),
        Path::new(docs_path).join("models.md").display(),
    )
}

pub fn format_no_models_available_message(docs_path: &str) -> String {
    format!(
        "No models available. {}",
        get_provider_login_help(docs_path)
    )
}

pub fn format_no_model_selected_message(docs_path: &str) -> String {
    format!(
        "No model selected.\n\n{}\n\nThen use /model to select a model.",
        get_provider_login_help(docs_path)
    )
}

pub fn format_no_api_key_found_message(provider: &str, docs_path: &str) -> String {
    let provider_display = if provider == UNKNOWN_PROVIDER {
        "the selected model"
    } else {
        provider
    };
    format!(
        "No API key found for {}.\n\n{}",
        provider_display,
        get_provider_login_help(docs_path)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_provider_login_help_includes_paths() {
        let msg = get_provider_login_help("/some/docs");
        assert!(msg.contains("providers.md"));
        assert!(msg.contains("models.md"));
        assert!(msg.contains("/some/docs"));
    }

    #[test]
    fn test_format_no_models_available_message() {
        let msg = format_no_models_available_message("/docs");
        assert!(msg.contains("No models available"));
        assert!(msg.contains("/login"));
    }

    #[test]
    fn test_format_no_api_key_found_unknown() {
        let msg = format_no_api_key_found_message("unknown", "/docs");
        assert!(msg.contains("the selected model"));
        assert!(!msg.contains("unknown"));
    }

    #[test]
    fn test_format_no_api_key_found_known() {
        let msg = format_no_api_key_found_message("anthropic", "/docs");
        assert!(msg.contains("anthropic"));
    }
}
