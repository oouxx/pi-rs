use std::collections::HashMap;
use std::sync::RwLock;

use crate::types::{Model, ModelCost, ModelCostTier, Usage};

static MODEL_REGISTRY: std::sync::LazyLock<RwLock<HashMap<String, HashMap<String, Model>>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

/// Register a single model in the runtime registry.
pub fn register_model(model: Model) {
    let mut reg = MODEL_REGISTRY.write().unwrap_or_else(std::sync::PoisonError::into_inner);
    reg.entry(model.provider.clone())
        .or_default()
        .insert(model.id.clone(), model);
}

/// Register multiple models.
pub fn register_models(models: Vec<Model>) {
    for model in models {
        register_model(model);
    }
}

/// Look up a model by provider and model ID.
pub fn get_model(provider: &str, model_id: &str) -> Option<Model> {
    let reg = MODEL_REGISTRY.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    reg.get(provider)?.get(model_id).cloned()
}

/// List all known provider names.
pub fn get_providers() -> Vec<String> {
    let reg = MODEL_REGISTRY.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    reg.keys().cloned().collect()
}

/// List all models for a given provider.
pub fn get_models(provider: &str) -> Vec<Model> {
    let reg = MODEL_REGISTRY.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    reg.get(provider)
        .map(|m| m.values().cloned().collect())
        .unwrap_or_default()
}

/// Calculate cost based on model pricing and token usage.
/// Cost is per-million-tokens, so we divide by 1,000,000.
///
/// Mirrors `calculateCost` in `packages/ai/src/models.ts`:
/// - request-wide input-token pricing tiers (`model.cost.tiers`), the highest
///   matching input threshold applies to the full request;
/// - Anthropic 1h cache writes are priced at 2x base input (`cacheWrite1h`).
struct CostRates {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}

impl CostRates {
    fn from_cost(cost: &ModelCost) -> Self {
        Self {
            input: cost.input,
            output: cost.output,
            cache_read: cost.cache_read,
            cache_write: cost.cache_write,
        }
    }
    fn from_tier(tier: &ModelCostTier) -> Self {
        Self {
            input: tier.input,
            output: tier.output,
            cache_read: tier.cache_read,
            cache_write: tier.cache_write,
        }
    }
}

pub fn calculate_cost(model: &Model, usage: &mut Usage) {
    let input_tokens = usage.input + usage.cache_read + usage.cache_write;
    let mut rates = CostRates::from_cost(&model.cost);
    let mut matched_threshold: i64 = -1;
    for tier in &model.cost.tiers {
        if input_tokens > tier.input_tokens_above && tier.input_tokens_above as i64 > matched_threshold {
            rates = CostRates::from_tier(tier);
            matched_threshold = tier.input_tokens_above as i64;
        }
    }
    let long_write = usage.cache_write_1h.unwrap_or(0) as i64;
    let short_write = usage.cache_write as i64 - long_write;
    usage.cost.input = (rates.input / 1_000_000.0) * usage.input as f64;
    usage.cost.output = (rates.output / 1_000_000.0) * usage.output as f64;
    usage.cost.cache_read = (rates.cache_read / 1_000_000.0) * usage.cache_read as f64;
    usage.cost.cache_write =
        (rates.cache_write * short_write as f64 + rates.input * 2.0 * long_write as f64) / 1_000_000.0;
    usage.cost.total =
        usage.cost.input + usage.cost.output + usage.cost.cache_read + usage.cost.cache_write;
}

/// Extended thinking levels in order from least to most thinking.
pub const EXTENDED_THINKING_LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// Get the thinking levels supported by a model.
#[must_use] 
pub fn get_supported_thinking_levels(model: &Model) -> Vec<&'static str> {
    if !model.reasoning {
        return vec!["off"];
    }
    EXTENDED_THINKING_LEVELS
        .iter()
        .filter(|&&level| {
            // Mirrors TS getSupportedThinkingLevels: a null mapping marks the
            // level unsupported, and "xhigh"/"max" are only supported when the
            // model's thinkingLevelMap declares them explicitly.
            match model.thinking_level_map.as_ref().and_then(|m| m.get(level)) {
                None => level != "xhigh" && level != "max",
                Some(None) => false,
                Some(Some(_)) => true,
            }
        })
        .copied()
        .collect()
}

/// Clamp a requested thinking level to the nearest available level.
#[must_use] 
pub fn clamp_thinking_level(model: &Model, level: &str) -> String {
    let available = get_supported_thinking_levels(model);
    if available.contains(&level) {
        return level.to_string();
    }
    let requested_index = EXTENDED_THINKING_LEVELS.iter().position(|&l| l == level);
    let Some(ri) = requested_index else {
        return available.first().copied().unwrap_or("off").to_string();
    };
    for candidate in &EXTENDED_THINKING_LEVELS[ri..] {
        if available.contains(candidate) {
            return (*candidate).to_string();
        }
    }
    for candidate in EXTENDED_THINKING_LEVELS[..ri].iter().rev() {
        if available.contains(candidate) {
            return (*candidate).to_string();
        }
    }
    available.first().copied().unwrap_or("off").to_string()
}

/// Check if two models are equal by comparing both id and provider.
#[must_use] 
pub fn models_are_equal(a: Option<&Model>, b: Option<&Model>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.id == b.id && a.provider == b.provider,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ModelCost, ModelCostTier, Usage};

    fn make_model(cost: ModelCost) -> Model {
        Model {
            id: "test-model".into(),
            name: "Test Model".into(),
            api: "openai-completions".into(),
            provider: "test".into(),
            base_url: "https://example.com".into(),
            reasoning: true,
            thinking_level_map: None,
            input: vec!["text".into()],
            cost,
            context_window: 128_000,
            max_tokens: 16_384,
            headers: None,
            compat: None,
        }
    }

    #[test]
    fn test_calculate_cost_1h_cache_write_priced_at_2x_input() {
        let model = make_model(ModelCost {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 6.0,
            tiers: vec![],
        });
        let mut usage = Usage {
            input: 1000,
            output: 500,
            cache_read: 200,
            cache_write: 100,
            cache_write_1h: Some(40),
            reasoning: None,
            total_tokens: 1800,
            cost: Default::default(),
        };
        calculate_cost(&model, &mut usage);
        // short write = 100 - 40 = 60 at cacheWrite rate; long write = 40 at 2x input.
        let expected_cache_write = (6.0 * 60.0 + 3.0 * 2.0 * 40.0) / 1_000_000.0;
        assert!((usage.cost.cache_write - expected_cache_write).abs() < 1e-9);
        assert!((usage.cost.total - (usage.cost.input + usage.cost.output + usage.cost.cache_read + usage.cost.cache_write)).abs() < 1e-9);
    }

    #[test]
    fn test_calculate_cost_tiers_highest_matching_threshold_wins() {
        let model = make_model(ModelCost {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 6.0,
            tiers: vec![
                ModelCostTier {
                    input: 2.0,
                    output: 10.0,
                    cache_read: 0.2,
                    cache_write: 4.0,
                    input_tokens_above: 100_000,
                },
                ModelCostTier {
                    input: 1.0,
                    output: 5.0,
                    cache_read: 0.1,
                    cache_write: 2.0,
                    input_tokens_above: 200_000,
                },
            ],
        });
        let mut usage = Usage {
            input: 150_000,
            output: 1000,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: 151_000,
            cost: Default::default(),
        };
        calculate_cost(&model, &mut usage);
        // input+cacheRead+cacheWrite = 150_000 > 100_000 but not > 200_000 → first tier.
        let expected_input = (2.0 / 1_000_000.0) * 150_000.0;
        assert!((usage.cost.input - expected_input).abs() < 1e-9);
    }

    #[test]
    fn test_extended_thinking_levels_include_max() {
        assert!(EXTENDED_THINKING_LEVELS.contains(&"max"));
    }

    #[test]
    fn test_max_thinking_level_requires_explicit_map() {
        // "max" (like "xhigh") is only supported when the model's thinkingLevelMap
        // declares it explicitly.
        let model = make_model(ModelCost {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 6.0,
            tiers: vec![],
        });
        let levels = get_supported_thinking_levels(&model);
        assert!(!levels.contains(&"max"));
        assert!(!levels.contains(&"xhigh"));
    }
}
