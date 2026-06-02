use std::collections::HashMap;
use std::sync::RwLock;

use crate::types::{Model, Usage};

static MODEL_REGISTRY: std::sync::LazyLock<RwLock<HashMap<String, HashMap<String, Model>>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

/// Register a single model in the runtime registry.
pub fn register_model(model: Model) {
    let mut reg = MODEL_REGISTRY.write().unwrap();
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
    let reg = MODEL_REGISTRY.read().unwrap();
    reg.get(provider)?.get(model_id).cloned()
}

/// List all known provider names.
pub fn get_providers() -> Vec<String> {
    let reg = MODEL_REGISTRY.read().unwrap();
    reg.keys().cloned().collect()
}

/// List all models for a given provider.
pub fn get_models(provider: &str) -> Vec<Model> {
    let reg = MODEL_REGISTRY.read().unwrap();
    reg.get(provider)
        .map(|m| m.values().cloned().collect())
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
    let requested_index = EXTENDED_THINKING_LEVELS.iter().position(|&l| l == level);
    if requested_index.is_none() {
        return available.first().copied().unwrap_or("off").to_string();
    }
    let ri = requested_index.unwrap();
    for i in ri..EXTENDED_THINKING_LEVELS.len() {
        let candidate = EXTENDED_THINKING_LEVELS[i];
        if available.iter().any(|&l| l == candidate) {
            return candidate.to_string();
        }
    }
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
