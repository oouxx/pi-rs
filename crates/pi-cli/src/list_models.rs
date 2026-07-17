//! CLI command to list available models in a formatted table.
//!
//! Provides table-formatted output with context/max-token formatting
//! and optional fuzzy search filtering.

use pi_coding_agent::pi_ai_types::Model;
use pi_coding_agent::core::model_registry::ModelRegistry;

const EXIT_SUCCESS: i32 = 0;
const EXIT_FAILURE: i32 = 1;

const IMAGE_SUPPORTING_APIS: &[&str] = &[
    "anthropic-messages",
    "openai-completions",
    "google-generative-ai",
    "vertex-ai-anthropic",
];

fn supports_images(model: &Model) -> bool {
    model.input.iter().any(|i| i == "image")
}

/// Column width information for the model list table.
pub struct ColumnWidths {
    pub provider: usize,
    pub model: usize,
    pub context: usize,
    pub max_out: usize,
    pub thinking: usize,
    pub images: usize,
}

/// Format a token count in human-readable abbreviated form.
///
/// Examples:
/// - 500     -> "500"
/// - 1500    -> "1.5K"
/// - 200000  -> "200K"
/// - 1048576 -> "1M"
pub fn format_token_count(count: u64) -> String {
    if count >= 1_000_000 {
        let mill = count as f64 / 1_000_000.0;
        if mill.fract() < 0.05 {
            format!("{}M", mill as u64)
        } else {
            format!("{:.1}M", mill)
        }
    } else if count >= 1_000 {
        let k = count as f64 / 1_000.0;
        if k.fract() < 0.05 {
            format!("{}K", k as u64)
        } else {
            format!("{:.1}K", k)
        }
    } else {
        count.to_string()
    }
}

/// Calculate column widths for the table based on model data,
/// ensuring column headers always fit.
pub fn calculate_column_widths(models: &[Model]) -> ColumnWidths {
    let mut widths = ColumnWidths {
        provider: "provider".len(),
        model: "model".len(),
        context: "context".len(),
        max_out: "max-out".len(),
        thinking: "thinking".len(),
        images: "images".len(),
    };

    for model in models {
        widths.provider = widths.provider.max(model.provider.len());
        widths.model = widths.model.max(model.id.len());
        widths.context = widths
            .context
            .max(format_token_count(model.context_window).len());
        widths.max_out = widths
            .max_out
            .max(format_token_count(model.max_tokens).len());
        let thinking_str = if model.reasoning { "yes" } else { "no" };
        widths.thinking = widths.thinking.max(thinking_str.len());
        let images_str = if supports_images(model) { "yes" } else { "no" };
        widths.images = widths.images.max(images_str.len());
    }

    widths
}

/// List available models, optionally filtered by a search pattern.
///
/// The search performs a case-insensitive match against model provider and id
/// (not name). Results are sorted by provider then by id.
/// Returns `EXIT_SUCCESS` (0) or `EXIT_FAILURE` (1).
pub async fn list_models(
    model_registry: &ModelRegistry,
    search_pattern: Option<&str>,
) -> i32 {
    // NOTE: Unlike the TypeScript original (which filters to show only models
    // with configured auth via get_available()), this shows ALL known models
    // via get_models() to aid discovery. get_available() would respect env
    // vars and registered providers, but --list-models is meant for browsing
    // what the agent supports, not checking what's configured.
    let all_models = model_registry.get_models();

    // Filter by search pattern if provided
    let mut matched: Vec<Model> = if let Some(pattern) = search_pattern {
        let pattern_lower = pattern.to_lowercase();
        all_models
            .into_iter()
            .filter(|m| {
                m.id.to_lowercase().contains(&pattern_lower)
                    || m.provider.to_lowercase().contains(&pattern_lower)
                    || m.name.to_lowercase().contains(&pattern_lower)
            })
            .collect()
    } else {
        all_models
    };

    if matched.is_empty() {
        if let Some(pattern) = search_pattern {
            eprintln!("No models matching '{}'", pattern);
        } else {
            eprintln!("No models available.");
        }
        return EXIT_FAILURE;
    }

    // Sort by provider then by id
    matched.sort_by(|a, b| {
        a.provider
            .cmp(&b.provider)
            .then_with(|| a.id.cmp(&b.id))
    });

    let widths = calculate_column_widths(&matched);

    // ── Header ──────────────────────────────────────────────────────────
    println!(
        "{:<pw$}  {:<mw$}  {:>cw$}  {:>tw$}  {:<rw$}  {:<iw$}",
        "provider",
        "model",
        "context",
        "max-out",
        "thinking",
        "images",
        pw = widths.provider,
        mw = widths.model,
        cw = widths.context,
        tw = widths.max_out,
        rw = widths.thinking,
        iw = widths.images,
    );

    // ── Separator ───────────────────────────────────────────────────────
    let total = widths.provider
        + 2 + widths.model
        + 2 + widths.context
        + 2 + widths.max_out
        + 2 + widths.thinking
        + 2 + widths.images;
    println!("{}", "-".repeat(total));

    // ── Rows ────────────────────────────────────────────────────────────
    for model in &matched {
        let thinking_str = if model.reasoning { "yes" } else { "no" };
        let images_str = if supports_images(model) { "yes" } else { "no" };

        println!(
            "{:<pw$}  {:<mw$}  {:>cw$}  {:>tw$}  {:<rw$}  {:<iw$}",
            model.provider,
            model.id,
            format_token_count(model.context_window),
            format_token_count(model.max_tokens),
            thinking_str,
            images_str,
            pw = widths.provider,
            mw = widths.model,
            cw = widths.context,
            tw = widths.max_out,
            rw = widths.thinking,
            iw = widths.images,
        );
    }

    EXIT_SUCCESS
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pi_coding_agent::core::model_registry::ModelRegistry;

    fn sample_models() -> Vec<Model> {
        pi_coding_agent::pi_agent_core::pi_ai::providers::register_builtins::register_built_in_api_providers();
        ModelRegistry::builtin_models_list()
    }

    #[test]
    fn test_format_token_count_zero() {
        assert_eq!(format_token_count(0), "0");
    }

    #[test]
    fn test_format_token_count_small() {
        assert_eq!(format_token_count(500), "500");
        assert_eq!(format_token_count(999), "999");
    }

    #[test]
    fn test_format_token_count_thousands() {
        assert_eq!(format_token_count(1000), "1K");
        assert_eq!(format_token_count(1500), "1.5K");
        assert_eq!(format_token_count(200_000), "200K");
        assert_eq!(format_token_count(128_000), "128K");
        assert_eq!(format_token_count(131_072), "131.1K");
    }

    #[test]
    fn test_format_token_count_millions() {
        assert_eq!(format_token_count(1_000_000), "1M");
        assert_eq!(format_token_count(1_048_576), "1M");
        assert_eq!(format_token_count(1_500_000), "1.5M");
    }

    #[test]
    fn test_format_token_count() {
        assert_eq!(format_token_count(1000), "1K");
        assert_eq!(format_token_count(1500), "1.5K");
        assert_eq!(format_token_count(1_000_000), "1M");
        assert_eq!(format_token_count(1_500_000), "1.5M");
        assert_eq!(format_token_count(500), "500");
    }

    #[test]
    fn test_calculate_column_widths_empty() {
        let widths = calculate_column_widths(&[]);
        // Should be at least header widths
        assert!(widths.provider >= "provider".len());
        assert!(widths.model >= "model".len());
        assert!(widths.context >= "context".len());
        assert!(widths.max_out >= "max-out".len());
        assert!(widths.thinking >= "thinking".len());
        assert!(widths.images >= "images".len());
    }

    #[test]
    fn test_calculate_column_widths_with_data() {
        let models = sample_models();
        let widths = calculate_column_widths(&models);

        // Provider column should accommodate all provider names
        for model in &models {
            assert!(
                widths.provider >= model.provider.len(),
                "provider width {} < {}",
                widths.provider,
                model.provider.len()
            );
        }

        // Model column should accommodate all model ids
        for model in &models {
            assert!(
                widths.model >= model.id.len(),
                "model width {} < {}",
                widths.model,
                model.id.len()
            );
        }
    }

    #[test]
    fn test_list_models_search_no_match_returns_failure() {
        let registry = ModelRegistry::new(sample_models());
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(list_models(&registry, Some("zzzznotfound")));
        assert_eq!(result, EXIT_FAILURE);
    }

    #[test]
    fn test_list_models_search_by_provider() {
        let registry = ModelRegistry::new(sample_models());
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(list_models(&registry, Some("anthropic")));
        assert_eq!(result, EXIT_SUCCESS);
    }

    #[test]
    fn test_list_models_search_by_model_id() {
        let registry = ModelRegistry::new(sample_models());
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(list_models(&registry, Some("gpt-4o")));
        assert_eq!(result, EXIT_SUCCESS);
    }

    #[test]
    fn test_list_models_case_insensitive_search() {
        let registry = ModelRegistry::new(sample_models());
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(list_models(&registry, Some("ANTHROPIC")));
        assert_eq!(result, EXIT_SUCCESS);
    }

    #[test]
    fn test_list_models_with_data_returns_success() {
        let registry = ModelRegistry::new(sample_models());
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(list_models(&registry, None));
        assert_eq!(result, EXIT_SUCCESS);
    }
}
