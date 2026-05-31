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
}
