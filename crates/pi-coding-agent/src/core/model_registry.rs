use std::collections::HashMap;
use std::sync::RwLock;

use pi_agent_core::pi_ai_types::Model;

use crate::config;
use crate::core::env_api_keys::get_env_api_key;

use serde::Deserialize;

pub struct ModelRegistry {
    models: RwLock<Vec<Model>>,
    registered_providers: RwLock<HashMap<String, ProviderConfig>>,
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

#[derive(Debug, Clone)]
pub struct ModelRegistryEntry {
    pub model: Model,
    pub provider_config: Option<ProviderConfig>,
}

impl ModelRegistry {
    pub fn new(builtin_models: Vec<Model>) -> Self {
        let mut models = builtin_models;
        Self::load_models_from_file(&mut models);
        Self {
            models: RwLock::new(models),
            registered_providers: RwLock::new(HashMap::new()),
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
        let providers = self.registered_providers.read().unwrap();
        if let Some(config) = providers.get(&model.provider) {
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
                .is_some();
        !has_key
    }

    pub async fn get_api_key_and_headers(&self, model: &Model) -> Result<ApiKeyResult, String> {
        let mut api_key = get_env_api_key(&model.provider);
        let mut headers: HashMap<String, String> = HashMap::new();

        let providers = self.registered_providers.read().unwrap();
        if let Some(config) = providers.get(&model.provider) {
            if api_key.is_none() {
                api_key = config.api_key.clone();
            }
            if let Some(ref config_headers) = config.headers {
                headers.extend(config_headers.clone());
            }
        }

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

    fn load_models_from_file(models: &mut Vec<Model>) {
        let models_path = config::get_models_path();
        if !models_path.exists() {
            return;
        }
        match std::fs::read_to_string(&models_path) {
            Ok(content) => match serde_json::from_str::<ModelsFile>(&content) {
                Ok(file) => {
                    for model_def in file.models {
                        models.push(Model {
                            provider: model_def.provider,
                            api: model_def.api,
                            id: model_def.id,
                            context_window: model_def.context_window,
                            max_tokens: model_def.max_tokens,
                            cost: pi_agent_core::pi_ai_types::ModelCost {
                                input: model_def.cost.input,
                                output: model_def.cost.output,
                                cache_read: 0.0,
                                cache_write: 0.0,
                            },
                            name: String::new(),
                            base_url: String::new(),
                            thinking_level_map: None,
                            input: vec!["text".to_string()],
                            headers: None,
                            compat: None,
                            reasoning: model_def.reasoning,
                        });
                    }
                }
                Err(e) => {
                    eprintln!("Warning: Failed to parse models.json: {}", e);
                }
            },
            Err(e) => {
                eprintln!("Warning: Failed to read models.json: {}", e);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ApiKeyResult {
    pub ok: bool,
    pub api_key: String,
    pub headers: Option<HashMap<String, String>>,
    pub error: String,
}

#[derive(Debug, Deserialize)]
struct ModelsFile {
    models: Vec<ModelDefinition>,
}

#[derive(Debug, Deserialize)]
struct ModelDefinition {
    provider: String,
    api: String,
    id: String,
    context_window: u64,
    max_tokens: u64,
    reasoning: bool,
    cost: ModelCostDef,
}

#[derive(Debug, Deserialize)]
struct ModelCostDef {
    input: f64,
    output: f64,
}

/// Load models from the pi-ai generated model registry.
/// These models have correct base_url, names, etc. from the build-time generated data.
fn get_pi_ai_models() -> Vec<Model> {
    let providers = pi_ai::models::get_providers();
    let mut models = Vec::new();
    for provider in &providers {
        for m in pi_ai::models::get_models(provider) {
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
        let registry = ModelRegistry::new(builtin_models());
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
        let registry = ModelRegistry::new(builtin_models());
        let providers = registry.get_providers();
        assert!(providers.contains(&"anthropic".to_string()));
        assert!(providers.contains(&"openai".to_string()));
        assert!(providers.contains(&"deepseek".to_string()));
    }

    #[test]
    fn test_model_registry_models_for_provider() {
        let registry = ModelRegistry::new(builtin_models());
        let models = registry.get_models_for_provider("openai");
        assert!(!models.is_empty());
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"gpt-4o"));
    }

    #[test]
    fn test_builtin_models_count() {
        let models = builtin_models();
        assert!(models.len() >= 10);
    }
}
