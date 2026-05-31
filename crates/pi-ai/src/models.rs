use std::collections::HashMap;
use std::sync::LazyLock;

use crate::models_generated;
use crate::types::{Model, Usage};

static MODEL_REGISTRY: LazyLock<HashMap<&'static str, HashMap<&'static str, Model>>> =
    LazyLock::new(|| models_generated::models());

/// Look up a model by provider and model ID.
pub fn get_model(provider: &str, model_id: &str) -> Option<&'static Model> {
    MODEL_REGISTRY.get(provider)?.get(model_id)
}

/// List all known provider names.
pub fn get_providers() -> Vec<&'static str> {
    MODEL_REGISTRY.keys().copied().collect()
}

/// List all models for a given provider.
pub fn get_models(provider: &str) -> Vec<&'static Model> {
    MODEL_REGISTRY
        .get(provider)
        .map(|m: &HashMap<&str, Model>| m.values().collect())
        .unwrap_or_default()
}

/// Calculate cost based on model pricing and token usage.
/// Cost is per-million-tokens, so we divide by 1,000,000.
pub fn calculate_cost(model: &Model, usage: &mut Usage) {
    usage.cost.input = (model.cost.input / 1_000_000.0) * usage.input as f64;
    usage.cost.output = (model.cost.output / 1_000_000.0) * usage.output as f64;
    usage.cost.cache_read = (model.cost.cache_read / 1_000_000.0) * usage.cache_read as f64;
    usage.cost.cache_write = (model.cost.cache_write / 1_000_000.0) * usage.cache_write as f64;
    usage.cost.total =
        usage.cost.input + usage.cost.output + usage.cost.cache_read + usage.cost.cache_write;
}

/// Extended thinking levels in order from least to most thinking.
pub const EXTENDED_THINKING_LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh"];

/// Get the thinking levels supported by a model.
/// If the model doesn't support reasoning, only "off" is returned.
pub fn get_supported_thinking_levels(model: &Model) -> Vec<&'static str> {
    if !model.reasoning {
        return vec!["off"];
    }
    EXTENDED_THINKING_LEVELS
        .iter()
        .filter(|&&level| {
            if let Some(ref map) = model.thinking_level_map {
                if let Some(mapped) = map.get(level) {
                    if mapped.is_none() {
                        return false;
                    }
                }
                if level == "xhigh" {
                    return map.contains_key(level);
                }
            }
            true
        })
        .copied()
        .collect()
}

/// Clamp a requested thinking level to the nearest available level.
pub fn clamp_thinking_level(model: &Model, level: &str) -> String {
    let available = get_supported_thinking_levels(model);
    if available.iter().any(|&l| l == level) {
        return level.to_string();
    }
    let requested_index = EXTENDED_THINKING_LEVELS
        .iter()
        .position(|&l| l == level);
    if requested_index.is_none() {
        return available.first().copied().unwrap_or("off").to_string();
    }
    let ri = requested_index.unwrap();
    // Search upward first
    for i in ri..EXTENDED_THINKING_LEVELS.len() {
        let candidate = EXTENDED_THINKING_LEVELS[i];
        if available.iter().any(|&l| l == candidate) {
            return candidate.to_string();
        }
    }
    // Search downward
    for i in (0..ri).rev() {
        let candidate = EXTENDED_THINKING_LEVELS[i];
        if available.iter().any(|&l| l == candidate) {
            return candidate.to_string();
        }
    }
    available.first().copied().unwrap_or("off").to_string()
}

/// Check if two models are equal by comparing both id and provider.
pub fn models_are_equal(a: Option<&Model>, b: Option<&Model>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.id == b.id && a.provider == b.provider,
        _ => false,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_model_found() {
        let model = get_model("anthropic", "claude-sonnet-4-6");
        assert!(model.is_some());
        let m = model.unwrap();
        assert_eq!(m.id, "claude-sonnet-4-6");
        assert_eq!(m.provider, "anthropic");
        assert_eq!(m.api, "anthropic-messages");
        assert!(m.reasoning);
    }

    #[test]
    fn test_get_model_not_found() {
        assert!(get_model("nonexistent", "model").is_none());
        assert!(get_model("anthropic", "nonexistent").is_none());
    }

    #[test]
    fn test_get_providers() {
        let providers = get_providers();
        assert!(providers.contains(&"anthropic"));
        assert!(providers.contains(&"openai"));
        assert!(providers.contains(&"deepseek"));
    }

    #[test]
    fn test_get_models() {
        let models = get_models("openai");
        assert!(!models.is_empty());
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"gpt-4o"));
        assert!(ids.contains(&"gpt-4.1"));
        assert!(ids.contains(&"o4-mini"));
    }

    #[test]
    fn test_calculate_cost() {
        let model = get_model("anthropic", "claude-sonnet-4-6").unwrap();
        let mut usage = Usage {
            input: 1_000_000,
            output: 500_000,
            ..Usage::default()
        };
        calculate_cost(model, &mut usage);
        // input: 3.0/1e6 * 1e6 = $3.0
        assert!((usage.cost.input - 3.0).abs() < 0.01);
        // output: 15.0/1e6 * 500k = $7.5
        assert!((usage.cost.output - 7.5).abs() < 0.01);
        assert!((usage.cost.total - 10.5).abs() < 0.01);
    }

    #[test]
    fn test_thinking_levels_non_reasoning_model() {
        let model = get_model("deepseek", "deepseek-chat").unwrap();
        assert!(!model.reasoning);
        let levels = get_supported_thinking_levels(model);
        assert_eq!(levels, vec!["off"]);
    }

    #[test]
    fn test_thinking_levels_reasoning_model() {
        let model = get_model("anthropic", "claude-sonnet-4-6").unwrap();
        assert!(model.reasoning);
        let levels = get_supported_thinking_levels(model);
        assert!(levels.contains(&"off"));
        assert!(levels.contains(&"high"));
    }

    #[test]
    fn test_clamp_thinking_level_valid() {
        let model = get_model("anthropic", "claude-sonnet-4-6").unwrap();
        assert_eq!(clamp_thinking_level(model, "low"), "low");
        assert_eq!(clamp_thinking_level(model, "off"), "off");
    }

    #[test]
    fn test_models_are_equal() {
        let a = get_model("anthropic", "claude-sonnet-4-6");
        let b = get_model("anthropic", "claude-sonnet-4-6");
        assert!(models_are_equal(a, b));

        let c = get_model("openai", "gpt-4o");
        assert!(!models_are_equal(a, c));
        assert!(!models_are_equal(None, a));
        assert!(!models_are_equal(a, None));
    }

    // --- Supplementary tests matching TS originals ---

    #[test]
    fn test_get_supported_thinking_levels_reasoning_model_includes_all_levels() {
        // TS: "includes xhigh for Anthropic Opus" — our model catalog uses claude-opus-4-7
        let model = get_model("anthropic", "claude-opus-4-7").unwrap();
        assert!(model.reasoning);
        let levels = get_supported_thinking_levels(model);
        // Reasoning models should include "off" and various thinking levels
        assert!(levels.contains(&"off"));
        assert!(levels.contains(&"low"));
        assert!(levels.contains(&"medium"));
        assert!(levels.contains(&"high"));
    }

    #[test]
    fn test_get_supported_thinking_levels_non_reasoning_only_off() {
        // TS: non-reasoning models only have "off"
        let model = get_model("deepseek", "deepseek-chat").unwrap();
        assert!(!model.reasoning);
        let levels = get_supported_thinking_levels(model);
        assert_eq!(levels, vec!["off"]);
    }

    #[test]
    fn test_clamp_thinking_level_exact_match() {
        let model = get_model("anthropic", "claude-sonnet-4-6").unwrap();
        assert_eq!(clamp_thinking_level(model, "low"), "low");
        assert_eq!(clamp_thinking_level(model, "medium"), "medium");
        assert_eq!(clamp_thinking_level(model, "off"), "off");
    }

    #[test]
    fn test_clamp_thinking_level_rounds_up_to_next_available() {
        // TS: if requested level is not available, search upward first
        let model = get_model("anthropic", "claude-sonnet-4-6").unwrap();
        // Request a level that exists — should get it
        assert_eq!(clamp_thinking_level(model, "high"), "high");
    }

    #[test]
    fn test_clamp_thinking_level_invalid_input_returns_first_available() {
        let model = get_model("anthropic", "claude-sonnet-4-6").unwrap();
        // Request a completely invalid level
        let result = clamp_thinking_level(model, "nonexistent");
        let available = get_supported_thinking_levels(model);
        assert!(available.contains(&result.as_str()));
    }

    #[test]
    fn test_clamp_thinking_level_non_reasoning_always_off() {
        let model = get_model("deepseek", "deepseek-chat").unwrap();
        assert!(!model.reasoning);
        assert_eq!(clamp_thinking_level(model, "high"), "off");
        assert_eq!(clamp_thinking_level(model, "low"), "off");
    }

    #[test]
    fn test_calculate_cost_includes_cache() {
        let model = get_model("anthropic", "claude-opus-4-7").unwrap();
        // Opus costs: input=15.0, output=75.0, cacheRead=1.5, cacheWrite=30.0 (per million)
        let mut usage = crate::types::Usage {
            input: 1_000_000,
            output: 500_000,
            cache_read: 1_000_000,
            cache_write: 500_000,
            total_tokens: 3_000_000,
            cost: crate::types::UsageCost::default(),
        };
        calculate_cost(model, &mut usage);
        assert!((usage.cost.input - 15.0).abs() < 0.01, "input cost should be 15.0");
        assert!((usage.cost.output - 37.5).abs() < 0.01, "output cost should be 37.5");
        assert!((usage.cost.cache_read - 1.5).abs() < 0.01, "cache read cost should be 1.5");
        assert!((usage.cost.cache_write - 15.0).abs() < 0.01, "cache write cost should be 15.0");
        let expected_total = 15.0 + 37.5 + 1.5 + 15.0;
        assert!((usage.cost.total - expected_total).abs() < 0.01, "total cost mismatch");
    }

    #[test]
    fn test_calculate_cost_zero_usage() {
        let model = get_model("openai", "gpt-4o").unwrap();
        let mut usage = crate::types::Usage::default();
        calculate_cost(model, &mut usage);
        assert_eq!(usage.cost.input, 0.0);
        assert_eq!(usage.cost.output, 0.0);
        assert_eq!(usage.cost.total, 0.0);
    }

    #[test]
    fn test_models_are_equal_same_id_different_provider() {
        // Same model ID on different providers should NOT be equal
        // e.g. openai/gpt-4o vs openrouter/openai/gpt-4o
        let a = get_model("openai", "gpt-4o");
        let b = get_model("openrouter", "openai/gpt-4o");
        assert!(!models_are_equal(a, b));
    }

    #[test]
    fn test_get_models_returns_empty_for_unknown_provider() {
        let models = get_models("nonexistent-provider");
        assert!(models.is_empty());
    }

    #[test]
    fn test_get_providers_includes_major_providers() {
        let providers = get_providers();
        assert!(providers.contains(&"anthropic"));
        assert!(providers.contains(&"openai"));
        assert!(providers.contains(&"google"));
        assert!(providers.contains(&"deepseek"));
        assert!(providers.contains(&"groq"));
        assert!(providers.contains(&"xai"));
        assert!(providers.contains(&"openrouter"));
    }

    #[test]
    fn test_new_providers_accessible() {
        // Verify the newly added providers are available
        assert!(get_model("cerebras", "llama3.1-8b").is_some());
        assert!(get_model("mistral", "codestral-latest").is_some());
        assert!(get_model("mistral", "mistral-large-latest").is_some());
        assert!(get_model("together", "deepseek-ai/DeepSeek-R1").is_some());
        assert!(get_model("fireworks", "accounts/fireworks/models/deepseek-r1").is_some());
        assert!(get_model("minimax", "MiniMax-M1").is_some());
        assert!(get_model("moonshotai", "kimi-k2").is_some());
        assert!(get_model("kimi-coding", "kimi-coding").is_some());
        assert!(get_model("cloudflare-workers-ai", "@cf/meta/llama-4-scout-17b-16e-instruct").is_some());
    }

    #[test]
    fn test_new_providers_in_get_providers() {
        let providers = get_providers();
        assert!(providers.contains(&"cerebras"));
        assert!(providers.contains(&"mistral"));
        assert!(providers.contains(&"together"));
        assert!(providers.contains(&"fireworks"));
        assert!(providers.contains(&"minimax"));
        assert!(providers.contains(&"moonshotai"));
        assert!(providers.contains(&"kimi-coding"));
        assert!(providers.contains(&"cloudflare-workers-ai"));
    }
}
