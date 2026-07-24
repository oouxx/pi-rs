use std::collections::HashMap;
use std::sync::RwLock;

use pi_agent_core::pi_ai_types::Model;

use crate::config;
use pi_agent_core::pi_ai_types::get_env_api_key;

use serde::Deserialize;

pub struct ModelRegistry {
    models: RwLock<Vec<Model>>,
    registered_providers: RwLock<HashMap<String, ProviderConfig>>,
    /// Provider configs loaded from models.json (provider-level settings like baseUrl, apiKey, headers, etc.)
    models_json_providers: RwLock<HashMap<String, ProviderConfig>>,
}

impl Clone for ModelRegistry {
    fn clone(&self) -> Self {
        Self {
            models: RwLock::new(self.models.read().unwrap().clone()),
            registered_providers: RwLock::new(self.registered_providers.read().unwrap().clone()),
            models_json_providers: RwLock::new(self.models_json_providers.read().unwrap().clone()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub api: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub auth_header: Option<bool>,
}

/// Input for registering a provider, matching the original ProviderConfigInput interface.
#[derive(Debug, Clone, Default)]
pub struct ProviderConfigInput {
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub api: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub auth_header: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct ModelRegistryEntry {
    pub model: Model,
    pub provider_config: Option<ProviderConfig>,
}

impl ModelRegistry {
    pub fn new(builtin_models: Vec<Model>) -> Self {
        let mut models = builtin_models;
        let models_json_providers = Self::load_models_from_file(&mut models);
        Self {
            models: RwLock::new(models),
            registered_providers: RwLock::new(HashMap::new()),
            models_json_providers: RwLock::new(models_json_providers),
        }
    }

    /// Create a new ModelRegistry with models from a specific models.json path.
    /// Used by tests to avoid relying on environment variables.
    #[cfg(test)]
    pub fn new_with_models_path(builtin_models: Vec<Model>, models_path: &std::path::Path) -> Self {
        let mut models = builtin_models;
        let models_json_providers = Self::load_models_from_path(&mut models, models_path);
        Self {
            models: RwLock::new(models),
            registered_providers: RwLock::new(HashMap::new()),
            models_json_providers: RwLock::new(models_json_providers),
        }
    }

    pub fn builtin_models_list() -> Vec<Model> {
        // Try to load from pi-ai generated models first (they have correct base_url, etc.)
        let pi_models = get_pi_ai_models();
        if !pi_models.is_empty() {
            return pi_models;
        }
        // Fall back to hardcoded models
        builtin_models()
    }

    pub fn find(&self, provider: &str, model_id: &str) -> Option<Model> {
        let models = self.models.read().unwrap();
        models
            .iter()
            .find(|m| m.provider == provider && m.id == model_id)
            .cloned()
    }

    pub fn get_models(&self) -> Vec<Model> {
        self.models.read().unwrap().clone()
    }

    pub fn get_models_for_provider(&self, provider: &str) -> Vec<Model> {
        let models = self.models.read().unwrap();
        models
            .iter()
            .filter(|m| m.provider == provider)
            .cloned()
            .collect()
    }

    pub fn get_providers(&self) -> Vec<String> {
        let models = self.models.read().unwrap();
        let mut providers: Vec<String> = models.iter().map(|m| m.provider.clone()).collect();
        providers.sort();
        providers.dedup();
        providers
    }

    pub fn get_available(&self) -> Vec<Model> {
        let models = self.models.read().unwrap();
        models
            .iter()
            .filter(|m| self.has_configured_auth(m))
            .cloned()
            .collect()
    }

    pub fn has_configured_auth(&self, model: &Model) -> bool {
        if get_env_api_key(&model.provider).is_some() {
            return true;
        }
        // Check registered providers (from register_provider calls)
        let providers = self.registered_providers.read().unwrap();
        if let Some(config) = providers.get(&model.provider) {
            if config.api_key.is_some() {
                return true;
            }
        }
        // Check models.json provider configs
        let json_providers = self.models_json_providers.read().unwrap();
        if let Some(config) = json_providers.get(&model.provider) {
            if config.api_key.is_some() {
                return true;
            }
        }
        false
    }

    /// Check if the model uses OAuth authentication, matching the original isUsingOAuth().
    pub fn is_using_oauth(&self, model: &Model) -> bool {
        // OAuth providers typically don't use API keys
        let has_key = get_env_api_key(&model.provider).is_some()
            || self
                .registered_providers
                .read()
                .unwrap()
                .get(&model.provider)
                .and_then(|c| c.api_key.as_ref())
                .is_some()
            || self
                .models_json_providers
                .read()
                .unwrap()
                .get(&model.provider)
                .and_then(|c| c.api_key.as_ref())
                .is_some();
        !has_key
    }

    /// Get API key for a provider, checking env vars, registered providers,
    /// and models.json provider configs in order.
    pub fn get_api_key_for_provider(&self, provider: &str) -> Option<String> {
        // Check env first
        if let Some(key) = get_env_api_key(provider) {
            return Some(key);
        }
        // Check registered providers (from register_provider calls)
        let providers = self.registered_providers.read().unwrap();
        if let Some(config) = providers.get(provider) {
            if let Some(key) = &config.api_key {
                return Some(key.clone());
            }
        }
        drop(providers);

        // Check models.json provider configs
        let json_providers = self.models_json_providers.read().unwrap();
        if let Some(config) = json_providers.get(provider) {
            if let Some(key) = &config.api_key {
                return Some(key.clone());
            }
        }
        drop(json_providers);

        None
    }

    pub async fn get_api_key_and_headers(&self, model: &Model) -> Result<ApiKeyResult, String> {
        let mut api_key = get_env_api_key(&model.provider);
        let mut headers: HashMap<String, String> = HashMap::new();

        // Check registered providers first (higher priority)
        let providers = self.registered_providers.read().unwrap();
        if let Some(config) = providers.get(&model.provider) {
            if api_key.is_none() {
                api_key = config.api_key.clone();
            }
            if let Some(ref config_headers) = config.headers {
                headers.extend(config_headers.clone());
            }
        }
        drop(providers);

        // Then check models.json provider configs (lower priority)
        let json_providers = self.models_json_providers.read().unwrap();
        if let Some(config) = json_providers.get(&model.provider) {
            if api_key.is_none() {
                api_key = config.api_key.clone();
            }
            if let Some(ref config_headers) = config.headers {
                headers.extend(config_headers.clone());
            }
        }
        drop(json_providers);

        match api_key {
            Some(key) => Ok(ApiKeyResult {
                ok: true,
                api_key: key,
                headers: if headers.is_empty() {
                    None
                } else {
                    Some(headers)
                },
                error: String::new(),
            }),
            None => Ok(ApiKeyResult {
                ok: false,
                api_key: String::new(),
                headers: None,
                error: format!(
                    "No API key configured for provider '{}'. Set the appropriate environment variable or configure it via /login.",
                    model.provider
                ),
            }),
        }
    }

    pub fn register_provider(&self, provider_name: &str, config: ProviderConfig) {
        let mut providers = self.registered_providers.write().unwrap();
        providers.insert(provider_name.to_string(), config);
    }

    pub fn unregister_provider(&self, provider_name: &str) {
        let mut providers = self.registered_providers.write().unwrap();
        providers.remove(provider_name);
    }

    /// Load models from models.json (TS-compatible format: { "providers": { "name": { ... } } })
    /// Returns the provider configs extracted from the file.
    fn load_models_from_file(models: &mut Vec<Model>) -> HashMap<String, ProviderConfig> {
        let models_path = config::get_models_path();
        Self::load_models_from_path(models, &models_path)
    }

    fn load_models_from_path(models: &mut Vec<Model>, models_path: &std::path::Path) -> HashMap<String, ProviderConfig> {
        if !models_path.exists() {
            return HashMap::new();
        }
        match std::fs::read_to_string(&models_path) {
            Ok(content) => match serde_json::from_str::<ModelsConfig>(&content) {
                Ok(file) => {
                    let mut provider_configs = HashMap::new();
                    for (provider_name, provider_def) in file.providers {
                        // Store provider-level config
                        let provider_config = ProviderConfig {
                            name: provider_def.name.clone(),
                            base_url: provider_def.base_url.clone(),
                            api_key: provider_def.api_key.clone(),
                            api: provider_def.api.clone(),
                            headers: provider_def.headers.clone(),
                            auth_header: provider_def.auth_header,
                        };
                        provider_configs.insert(provider_name.clone(), provider_config);

                        // Apply provider-level baseUrl and compat to existing built-in models for this provider
                        if let Some(ref base_url) = provider_def.base_url {
                            for model in models.iter_mut() {
                                if model.provider == provider_name {
                                    model.base_url = base_url.clone();
                                }
                            }
                        }
                        if let Some(ref compat) = provider_def.compat {
                            for model in models.iter_mut() {
                                if model.provider == provider_name {
                                    model.compat = Some(compat.clone());
                                }
                            }
                        }

                        // Create models from the provider's models array
                        if let Some(ref model_defs) = provider_def.models {
                            for model_def in model_defs {
                                let api = model_def
                                    .api
                                    .clone()
                                    .or_else(|| provider_def.api.clone())
                                    .unwrap_or_default();
                                let base_url = model_def
                                    .base_url
                                    .clone()
                                    .or_else(|| provider_def.base_url.clone())
                                    .unwrap_or_default();
                                let name = model_def
                                    .name
                                    .clone()
                                    .unwrap_or_else(|| model_def.id.clone());

                                // Check if this model already exists (by id) in the list
                                let existing_idx = models.iter().position(|m| {
                                    m.provider == provider_name && m.id == model_def.id
                                });

                                let new_model = Model {
                                    id: model_def.id.clone(),
                                    name,
                                    api,
                                    provider: provider_name.clone(),
                                    base_url,
                                    reasoning: model_def.reasoning.unwrap_or(false),
                                    thinking_level_map: model_def.thinking_level_map.clone(),
                                    input: model_def
                                        .input
                                        .clone()
                                        .unwrap_or_else(|| vec!["text".to_string()]),
                                    cost: pi_agent_core::pi_ai_types::ModelCost {
                                        input: model_def
                                            .cost
                                            .as_ref()
                                            .map(|c| c.input)
                                            .unwrap_or(0.0),
                                        output: model_def
                                            .cost
                                            .as_ref()
                                            .map(|c| c.output)
                                            .unwrap_or(0.0),
                                        cache_read: model_def
                                            .cost
                                            .as_ref()
                                            .map(|c| c.cache_read)
                                            .unwrap_or(0.0),
                                        cache_write: model_def
                                            .cost
                                            .as_ref()
                                            .map(|c| c.cache_write)
                                            .unwrap_or(0.0),
                                    },
                                    context_window: model_def.context_window.unwrap_or(128000),
                                    max_tokens: model_def.max_tokens.unwrap_or(16384),
                                    headers: model_def.headers.clone(),
                                    compat: model_def.compat.clone(),
                                };

                                if let Some(idx) = existing_idx {
                                    // Replace existing model (TS behavior: models.json models override built-in)
                                    models[idx] = new_model;
                                } else {
                                    models.push(new_model);
                                }
                            }
                        }

                        // Apply modelOverrides
                        if let Some(ref overrides) = provider_def.model_overrides {
                            for (model_id, override_def) in overrides {
                                if let Some(model) = models.iter_mut().find(|m| {
                                    m.provider == provider_name && m.id == *model_id
                                }) {
                                    if let Some(ref name) = override_def.name {
                                        model.name = name.clone();
                                    }
                                    if let Some(reasoning) = override_def.reasoning {
                                        model.reasoning = reasoning;
                                    }
                                    if let Some(ref thinking_level_map) = override_def.thinking_level_map {
                                        let mut merged = model.thinking_level_map.clone().unwrap_or_default();
                                        for (k, v) in thinking_level_map {
                                            merged.insert(k.clone(), v.clone());
                                        }
                                        model.thinking_level_map = Some(merged);
                                    }
                                    if let Some(ref input) = override_def.input {
                                        model.input = input.clone();
                                    }
                                    if let Some(ref cost) = override_def.cost {
                                        if let Some(v) = cost.input {
                                            model.cost.input = v;
                                        }
                                        if let Some(v) = cost.output {
                                            model.cost.output = v;
                                        }
                                        if let Some(v) = cost.cache_read {
                                            model.cost.cache_read = v;
                                        }
                                        if let Some(v) = cost.cache_write {
                                            model.cost.cache_write = v;
                                        }
                                    }
                                    if let Some(ctx) = override_def.context_window {
                                        model.context_window = ctx;
                                    }
                                    if let Some(mt) = override_def.max_tokens {
                                        model.max_tokens = mt;
                                    }
                                    if let Some(ref headers) = override_def.headers {
                                        let mut merged = model.headers.clone().unwrap_or_default();
                                        merged.extend(headers.clone());
                                        model.headers = Some(merged);
                                    }
                                    if let Some(ref compat) = override_def.compat {
                                        model.compat = Some(compat.clone());
                                    }
                                }
                            }
                        }
                    }
                    return provider_configs;
                }
                Err(e) => {
                    eprintln!("Warning: Failed to parse models.json: {}", e);
                }
            },
            Err(e) => {
                eprintln!("Warning: Failed to read models.json: {}", e);
            }
        }
        HashMap::new()
    }
}

#[derive(Debug, Clone)]
pub struct ApiKeyResult {
    pub ok: bool,
    pub api_key: String,
    pub headers: Option<HashMap<String, String>>,
    pub error: String,
}

// ============================================================================
// models.json deserialization structs (aligned with TS ModelConfig schema)
// ============================================================================

/// Top-level structure: { "providers": { "name": { ... } } }
#[derive(Debug, Deserialize)]
struct ModelsConfig {
    providers: HashMap<String, ProviderDefinition>,
}

/// Per-provider config, matching TS ProviderConfigSchema
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderDefinition {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    api: Option<String>,
    #[serde(default)]
    headers: Option<HashMap<String, String>>,
    #[serde(default)]
    auth_header: Option<bool>,
    #[serde(default)]
    models: Option<Vec<ModelDefinition>>,
    #[serde(default)]
    model_overrides: Option<HashMap<String, ModelOverrideDefinition>>,
    #[serde(default)]
    compat: Option<pi_agent_core::pi_ai_types::ModelCompat>,
}

/// Per-model definition, matching TS ModelDefinitionSchema
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelDefinition {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    api: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    reasoning: Option<bool>,
    #[serde(default)]
    thinking_level_map: Option<HashMap<String, Option<String>>>,
    #[serde(default)]
    input: Option<Vec<String>>,
    #[serde(default)]
    cost: Option<ModelCostDef>,
    #[serde(default)]
    context_window: Option<u64>,
    #[serde(default)]
    max_tokens: Option<u64>,
    #[serde(default)]
    headers: Option<HashMap<String, String>>,
    #[serde(default)]
    compat: Option<pi_agent_core::pi_ai_types::ModelCompat>,
}

/// Per-model override, matching TS ModelOverrideSchema
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelOverrideDefinition {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    reasoning: Option<bool>,
    #[serde(default)]
    thinking_level_map: Option<HashMap<String, Option<String>>>,
    #[serde(default)]
    input: Option<Vec<String>>,
    #[serde(default)]
    cost: Option<ModelOverrideCostDef>,
    #[serde(default)]
    context_window: Option<u64>,
    #[serde(default)]
    max_tokens: Option<u64>,
    #[serde(default)]
    headers: Option<HashMap<String, String>>,
    #[serde(default)]
    compat: Option<pi_agent_core::pi_ai_types::ModelCompat>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelCostDef {
    #[serde(default)]
    input: f64,
    #[serde(default)]
    output: f64,
    #[serde(default)]
    cache_read: f64,
    #[serde(default)]
    cache_write: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelOverrideCostDef {
    #[serde(default)]
    input: Option<f64>,
    #[serde(default)]
    output: Option<f64>,
    #[serde(default)]
    cache_read: Option<f64>,
    #[serde(default)]
    cache_write: Option<f64>,
}

/// Load models from the pi-ai generated model registry.
/// These models have correct base_url, names, etc. from the build-time generated data.
fn get_pi_ai_models() -> Vec<Model> {
    let providers = pi_agent_core::pi_ai::models::get_providers();
    let mut models = Vec::new();
    for provider in &providers {
        for m in pi_agent_core::pi_ai::models::get_models(provider) {
            models.push(Model {
                id: m.id.clone(),
                name: m.name.clone(),
                api: m.api.clone(),
                provider: m.provider.clone(),
                base_url: m.base_url.clone(),
                reasoning: m.reasoning,
                thinking_level_map: m.thinking_level_map.clone(),
                input: m.input.clone(),
                cost: pi_agent_core::pi_ai_types::ModelCost {
                    input: m.cost.input,
                    output: m.cost.output,
                    cache_read: m.cost.cache_read,
                    cache_write: m.cost.cache_write,
                },
                context_window: m.context_window,
                max_tokens: m.max_tokens,
                headers: m.headers.clone(),
                compat: None,
            });
        }
    }
    models
}

pub fn builtin_models() -> Vec<Model> {
    vec![Model {
        provider: "openrouter".into(),
        api: "openai-completions".into(),
        id: "free".into(),
        context_window: 128000,
        max_tokens: 16384,
        cost: pi_agent_core::pi_ai_types::ModelCost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
        reasoning: false,
        name: String::new(),
        base_url: String::new(),
        thinking_level_map: None,
        input: vec!["text".to_string()],
        headers: None,
        compat: None,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_registry_find() {
        pi_agent_core::pi_ai::providers::register_builtins::register_built_in_api_providers();
        let registry = ModelRegistry::new(ModelRegistry::builtin_models_list());
        let model = registry.find("anthropic", "claude-sonnet-4-6");
        assert!(model.is_some());
        let m = model.unwrap();
        assert_eq!(m.id, "claude-sonnet-4-6");
        assert_eq!(m.provider, "anthropic");
        assert!(m.reasoning);
    }

    #[test]
    fn test_model_registry_not_found() {
        let registry = ModelRegistry::new(builtin_models());
        assert!(registry.find("nonexistent", "model").is_none());
    }

    #[test]
    fn test_model_registry_providers() {
        // Register built-in providers so pi-ai models are available
        pi_agent_core::pi_ai::providers::register_builtins::register_built_in_api_providers();
        let registry = ModelRegistry::new(ModelRegistry::builtin_models_list());
        let providers = registry.get_providers();
        assert!(providers.contains(&"anthropic".to_string()));
        assert!(providers.contains(&"openai".to_string()));
        assert!(providers.contains(&"deepseek".to_string()));
    }

    #[test]
    fn test_model_registry_models_for_provider() {
        pi_agent_core::pi_ai::providers::register_builtins::register_built_in_api_providers();
        let registry = ModelRegistry::new(ModelRegistry::builtin_models_list());
        let models = registry.get_models_for_provider("openai");
        assert!(!models.is_empty());
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"gpt-4o"));
    }

    #[test]
    fn test_builtin_models_count() {
        pi_agent_core::pi_ai::providers::register_builtins::register_built_in_api_providers();
        let models = ModelRegistry::builtin_models_list();
        assert!(models.len() >= 10);
    }

    #[test]
    fn test_models_json_provider_config() {
        let tmp_dir = std::env::temp_dir().join("pi-rs-test-models-json");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let models_path = tmp_dir.join("models.json");
        let json_content = r#"{
            "providers": {
                "ollama": {
                    "name": "Ollama",
                    "baseUrl": "http://localhost:11434/v1",
                    "api": "openai-completions",
                    "models": [
                        {
                            "id": "llama3.2",
                            "contextWindow": 128000,
                            "maxTokens": 8192,
                            "reasoning": false,
                            "cost": { "input": 0, "output": 0 }
                        },
                        {
                            "id": "deepseek-r1:7b",
                            "contextWindow": 128000,
                            "maxTokens": 8192,
                            "reasoning": true,
                            "cost": { "input": 0, "output": 0 }
                        }
                    ],
                    "modelOverrides": {
                        "llama3.2": {
                            "reasoning": true
                        }
                    }
                }
            }
        }"#;
        std::fs::write(&models_path, json_content).unwrap();

        let registry = ModelRegistry::new_with_models_path(vec![], &models_path);
        let models = registry.get_models();

        assert!(models.len() >= 2, "Expected at least 2 models, got {}", models.len());

        let llama = registry.find("ollama", "llama3.2");
        assert!(llama.is_some(), "llama3.2 should be found");
        let llama = llama.unwrap();
        assert_eq!(llama.base_url, "http://localhost:11434/v1");
        assert_eq!(llama.api, "openai-completions");
        assert!(llama.reasoning, "llama3.2 reasoning should be true (from modelOverrides)");

        let ds = registry.find("ollama", "deepseek-r1:7b");
        assert!(ds.is_some(), "deepseek-r1:7b should be found");
        let ds = ds.unwrap();
        assert_eq!(ds.base_url, "http://localhost:11434/v1");
        assert!(ds.reasoning, "deepseek-r1:7b should have reasoning=true");

        let json_providers = registry.models_json_providers.read().unwrap();
        let ollama_config = json_providers.get("ollama");
        assert!(ollama_config.is_some(), "ollama provider config should be stored");
        let ollama_config = ollama_config.unwrap();
        assert_eq!(ollama_config.base_url.as_deref(), Some("http://localhost:11434/v1"));
        assert_eq!(ollama_config.api.as_deref(), Some("openai-completions"));

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_models_json_overrides_builtin() {
        let tmp_dir = std::env::temp_dir().join("pi-rs-test-models-override");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let models_path = tmp_dir.join("models.json");
        let json_content = r#"{
            "providers": {
                "openai": {
                    "baseUrl": "http://custom-proxy/v1",
                    "models": [
                        {
                            "id": "gpt-4o",
                            "contextWindow": 999999,
                            "maxTokens": 999999,
                            "cost": { "input": 0.5, "output": 1.5 }
                        }
                    ]
                }
            }
        }"#;
        std::fs::write(&models_path, json_content).unwrap();

        let builtin = vec![Model {
            provider: "openai".into(),
            api: "openai-completions".into(),
            id: "gpt-4o".into(),
            context_window: 128000,
            max_tokens: 16384,
            cost: pi_agent_core::pi_ai_types::ModelCost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
            },
            reasoning: false,
            name: "GPT-4o".into(),
            base_url: "https://api.openai.com".into(),
            thinking_level_map: None,
            input: vec!["text".to_string()],
            headers: None,
            compat: None,
        }];

        let registry = ModelRegistry::new_with_models_path(builtin, &models_path);
        let model = registry.find("openai", "gpt-4o");
        assert!(model.is_some());
        let model = model.unwrap();
        assert_eq!(model.context_window, 999999);
        assert_eq!(model.max_tokens, 999999);
        assert_eq!(model.cost.input, 0.5);
        assert_eq!(model.cost.output, 1.5);
        assert_eq!(model.base_url, "http://custom-proxy/v1");

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
}
