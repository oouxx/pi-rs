use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use pi_agent_core::pi_ai_types::Model;

use crate::config;
use pi_agent_core::pi_ai_types::get_env_api_key;

use serde::Deserialize;

/// Resolver consulted for API-key resolution (auth.json), matching TS `getAuth`.
type ApiKeyResolver = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

pub struct ModelRegistry {
    models: RwLock<Vec<Model>>,
    registered_providers: Arc<RwLock<HashMap<String, ProviderConfig>>>,
    /// Provider configs loaded from models.json (provider-level settings like baseUrl, apiKey, headers, etc.)
    models_json_providers: RwLock<HashMap<String, ProviderConfig>>,
    /// Path of the models.json file, kept for hot reload (match TS #6999).
    models_path: Option<std::path::PathBuf>,
    /// Credential resolver consulted for API-key resolution, matching TS
    /// `getAuth` (auth.json is the canonical place GUI-set keys live).
    api_key_resolver: Option<ApiKeyResolver>,
}

impl Clone for ModelRegistry {
    fn clone(&self) -> Self {
        Self {
            models: RwLock::new(self.models.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()),
            registered_providers: Arc::clone(&self.registered_providers),
            models_json_providers: RwLock::new(self.models_json_providers.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()),
            models_path: self.models_path.clone(),
            api_key_resolver: self.api_key_resolver.clone(),
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
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
        let models_path = config::get_models_path();
        let models_json_providers = Self::load_models_from_path(&mut models, &models_path);
        Self {
            models: RwLock::new(models),
            registered_providers: Arc::new(RwLock::new(HashMap::new())),
            models_json_providers: RwLock::new(models_json_providers),
            models_path: Some(models_path),
            api_key_resolver: None,
        }
    }

    /// Attach a credential resolver (auth.json) so API-key resolution can
    /// consult keys set via the GUI / `/login` (match TS `getAuth`).
    pub fn set_api_key_resolver(&mut self, resolver: ApiKeyResolver) {
        self.api_key_resolver = Some(resolver);
    }

    /// Resolve a stored API key from the attached credential resolver, if any.
    fn resolved_stored_key(&self, provider: &str) -> Option<String> {
        self.api_key_resolver.as_ref().and_then(|r| r(provider))
    }

    /// Create a new ModelRegistry with models from a specific models.json path.
    /// Used by tests to avoid relying on environment variables.
    #[cfg(test)]
    pub fn new_with_models_path(builtin_models: Vec<Model>, models_path: &std::path::Path) -> Self {
        let mut models = builtin_models;
        let models_json_providers = Self::load_models_from_path(&mut models, models_path);
        Self {
            models: RwLock::new(models),
            registered_providers: Arc::new(RwLock::new(HashMap::new())),
            models_json_providers: RwLock::new(models_json_providers),
            models_path: Some(models_path.to_path_buf()),
            api_key_resolver: None,
        }
    }

    /// Reload models.json configuration (match TS `ModelRegistry.refresh()`, #6999).
    /// Re-reads the models.json file and upserts its models into the registry,
    /// so updated config (baseUrl/compat/new models) takes effect without a restart.
    pub fn refresh(&self) {
        let Some(path) = self.models_path.clone() else {
            return;
        };
        if !path.exists() {
            return;
        }
        let mut models = self.models.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        let providers = Self::load_models_from_path(&mut models, &path);
        let mut models_guard = self.models.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        *models_guard = models;
        let mut providers_guard = self
            .models_json_providers
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *providers_guard = providers;
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
        let models = self.models.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        models
            .iter()
            .find(|m| m.provider == provider && m.id == model_id)
            .cloned()
    }

    pub fn get_models(&self) -> Vec<Model> {
        self.models.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// Merge remote-catalog models for a provider into the registry: replace
    /// same-id models, append new ones (match TS `mergeModels`).
    pub fn upsert_models(&self, provider: &str, models: &[Model]) -> usize {
        let mut models_lock = self.models.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut changed = 0;
        for model in models {
            if model.provider != provider {
                continue;
            }
            let existing = models_lock
                .iter_mut()
                .find(|m| m.provider == provider && m.id == model.id);
            match existing {
                Some(m) => {
                    if *m != *model {
                        *m = model.clone();
                        changed += 1;
                    }
                }
                None => {
                    models_lock.push(model.clone());
                    changed += 1;
                }
            }
        }
        changed
    }

    pub fn get_models_for_provider(&self, provider: &str) -> Vec<Model> {
        let models = self.models.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        models
            .iter()
            .filter(|m| m.provider == provider)
            .cloned()
            .collect()
    }

    pub fn get_providers(&self) -> Vec<String> {
        let models = self.models.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut providers: Vec<String> = models.iter().map(|m| m.provider.clone()).collect();
        providers.sort();
        providers.dedup();
        providers
    }

    pub fn get_available(&self) -> Vec<Model> {
        let models = self.models.read().unwrap_or_else(std::sync::PoisonError::into_inner);
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
        // Stored credentials (auth.json) count as configured auth.
        if self.resolved_stored_key(&model.provider).is_some() {
            return true;
        }
        // Check registered providers (from register_provider calls)
        let providers = self.registered_providers.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(config) = providers.get(&model.provider) {
            if config.api_key.is_some() {
                return true;
            }
            // 对齐 TS:provider 未声明 authHeader（不需要 Authorization）时
            // 无凭据也可用（如本地端点），请求时不会附加认证头。
            if config.auth_header != Some(true) {
                return true;
            }
        }
        // Check models.json provider configs
        let json_providers = self.models_json_providers.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(config) = json_providers.get(&model.provider) {
            if config.api_key.is_some() {
                return true;
            }
            if config.auth_header != Some(true) {
                return true;
            }
        }
        false
    }

    /// Check if the model uses OAuth authentication, matching the original isUsingOAuth().
    ///
    /// The Rust port does not implement the OAuth login flow
    /// (`AuthStorage::login` returns "not yet implemented" and
    /// `get_oauth_provider` returns `None`), so no provider can be configured
    /// with OAuth credentials — always `false`. (TS checks the auth snapshot's
    /// credential type; the `!has_key` heuristic previously used here was
    /// wrong: it classified every keyless provider as OAuth.)
    pub fn is_using_oauth(&self, _model: &Model) -> bool {
        false
    }

    /// Get API key for a provider, checking env vars, stored credentials
    /// (auth.json), registered providers, and models.json provider configs in order.
    pub fn get_api_key_for_provider(&self, provider: &str) -> Option<String> {
        // Check env first
        if let Some(key) = get_env_api_key(provider) {
            return Some(key);
        }
        // Check stored credentials (auth.json) — keys set via the GUI / /login.
        if let Some(key) = self.resolved_stored_key(provider) {
            return Some(key);
        }
        // Check registered providers (from register_provider calls)
        let providers = self.registered_providers.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(config) = providers.get(provider) {
            if let Some(key) = &config.api_key {
                // `${ENV}` template resolution (match TS ConfigValue).
                if let Some(resolved) =
                    crate::core::resolve_config_value::resolve_config_value(key)
                {
                    return Some(resolved);
                }
            }
        }
        drop(providers);

        // Check models.json provider configs
        let json_providers = self.models_json_providers.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(config) = json_providers.get(provider) {
            if let Some(key) = &config.api_key {
                if let Some(resolved) =
                    crate::core::resolve_config_value::resolve_config_value(key)
                {
                    return Some(resolved);
                }
            }
        }
        drop(json_providers);

        None
    }

    pub async fn get_api_key_and_headers(&self, model: &Model) -> Result<ApiKeyResult, String> {
        let mut api_key = get_env_api_key(&model.provider);
        // Stored credentials (auth.json) — keys set via the GUI / /login.
        if api_key.is_none() {
            api_key = self.resolved_stored_key(&model.provider);
        }
        let mut headers: HashMap<String, String> = HashMap::new();
        // Whether the provider requires an Authorization header (models.json
        // `authHeader`, matching TS `resolveCompatibilityRequestConfig`).
        let mut needs_auth_header = false;

        // Check registered providers first (higher priority)
        let providers = self.registered_providers.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(config) = providers.get(&model.provider) {
            if api_key.is_none() {
                // models.json / registered `apiKey` supports `${ENV}` templates
                // (match TS ConfigValue).
                api_key = config
                    .api_key
                    .as_deref()
                    .and_then(crate::core::resolve_config_value::resolve_config_value);
            }
            if let Some(ref config_headers) = config.headers {
                headers.extend(config_headers.clone());
            }
            if config.auth_header == Some(true) {
                needs_auth_header = true;
            }
        }
        drop(providers);

        // Then check models.json provider configs (lower priority)
        let json_providers = self.models_json_providers.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(config) = json_providers.get(&model.provider) {
            if api_key.is_none() {
                api_key = config
                    .api_key
                    .as_deref()
                    .and_then(crate::core::resolve_config_value::resolve_config_value);
            }
            if let Some(ref config_headers) = config.headers {
                headers.extend(config_headers.clone());
            }
            if config.auth_header == Some(true) {
                needs_auth_header = true;
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
            None => {
                // 对齐 TS getApiKeyAndHeaders：无凭据时只有 provider 声明了
                // `authHeader: true`（需要 Authorization）才报错；否则放行，
                // 不带认证头请求（本地端点，如 Ollama localhost）。
                if needs_auth_header {
                    Ok(ApiKeyResult {
                        ok: false,
                        api_key: String::new(),
                        headers: None,
                        error: format!(
                            "No API key configured for provider '{}'. Set the appropriate environment variable or configure it via /login.",
                            model.provider
                        ),
                    })
                } else {
                    Ok(ApiKeyResult {
                        ok: true,
                        api_key: String::new(),
                        headers: if headers.is_empty() {
                            None
                        } else {
                            Some(headers)
                        },
                        error: String::new(),
                    })
                }
            }
        }
    }

    pub fn register_provider(&self, provider_name: &str, config: ProviderConfig) {
        let mut providers = self.registered_providers.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        providers.insert(provider_name.to_string(), config);
    }

    pub fn unregister_provider(&self, provider_name: &str) {
        let mut providers = self.registered_providers.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        providers.remove(provider_name);
    }

    /// Get the config for a registered provider (from `register_provider`).
    #[must_use]
    pub fn get_provider_config(&self, provider_name: &str) -> Option<ProviderConfig> {
        let providers = self.registered_providers.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        providers.get(provider_name).cloned()
    }

    /// List all registered provider names (from `register_provider`).
    #[must_use]
    pub fn get_registered_providers(&self) -> Vec<String> {
        let providers = self.registered_providers.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        providers.keys().cloned().collect()
    }

    fn load_models_from_path(
        models: &mut Vec<Model>,
        models_path: &std::path::Path,
    ) -> HashMap<String, ProviderConfig> {
        if !models_path.exists() {
            return HashMap::new();
        }
        match std::fs::read_to_string(models_path) {
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
                                                tiers: vec![],
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
                                if let Some(model) = models
                                    .iter_mut()
                                    .find(|m| m.provider == provider_name && m.id == *model_id)
                                {
                                    if let Some(ref name) = override_def.name {
                                        model.name = name.clone();
                                    }
                                    if let Some(reasoning) = override_def.reasoning {
                                        model.reasoning = reasoning;
                                    }
                                    if let Some(ref thinking_level_map) =
                                        override_def.thinking_level_map
                                    {
                                        let mut merged =
                                            model.thinking_level_map.clone().unwrap_or_default();
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
                            tiers: vec![],
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
        provider: "test-provider-xyz".into(),
        api: "openai-completions".into(),
        id: "free".into(),
        context_window: 128000,
        max_tokens: 16384,
        cost: pi_agent_core::pi_ai_types::ModelCost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
                    tiers: vec![],
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]
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

        assert!(
            models.len() >= 2,
            "Expected at least 2 models, got {}",
            models.len()
        );

        let llama = registry.find("ollama", "llama3.2");
        assert!(llama.is_some(), "llama3.2 should be found");
        let llama = llama.unwrap();
        assert_eq!(llama.base_url, "http://localhost:11434/v1");
        assert_eq!(llama.api, "openai-completions");
        assert!(
            llama.reasoning,
            "llama3.2 reasoning should be true (from modelOverrides)"
        );

        let ds = registry.find("ollama", "deepseek-r1:7b");
        assert!(ds.is_some(), "deepseek-r1:7b should be found");
        let ds = ds.unwrap();
        assert_eq!(ds.base_url, "http://localhost:11434/v1");
        assert!(ds.reasoning, "deepseek-r1:7b should have reasoning=true");

        let json_providers = registry.models_json_providers.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        let ollama_config = json_providers.get("ollama");
        assert!(
            ollama_config.is_some(),
            "ollama provider config should be stored"
        );
        let ollama_config = ollama_config.unwrap();
        assert_eq!(
            ollama_config.base_url.as_deref(),
            Some("http://localhost:11434/v1")
        );
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
                        tiers: vec![],
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

    #[test]
    fn test_api_key_resolver_consulted_for_auth_and_key() {
        use std::sync::Arc;

        let mut registry = ModelRegistry::new(vec![]);
        // No env key, no models.json entry → no auth by default.
        assert!(!registry.has_configured_auth(&Model {
            id: "m".into(),
            name: "m".into(),
            api: "openai-completions".into(),
            provider: "test-provider-xyz".into(),
            base_url: String::new(),
            reasoning: false,
            thinking_level_map: None,
            input: vec!["text".to_string()],
            cost: pi_agent_core::pi_ai_types::ModelCost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
                tiers: vec![],
            },
            context_window: 0,
            max_tokens: 0,
            headers: None,
            compat: None,
        }));

        // Attach a resolver returning a stored key → auth + key resolution work.
        registry.set_api_key_resolver(Arc::new(|provider| {
            (provider == "test-provider-xyz").then(|| "sk-stored-123".to_string())
        }));
        assert!(registry.has_configured_auth(&Model {
            id: "m".into(),
            name: "m".into(),
            api: "openai-completions".into(),
            provider: "test-provider-xyz".into(),
            base_url: String::new(),
            reasoning: false,
            thinking_level_map: None,
            input: vec!["text".to_string()],
            cost: pi_agent_core::pi_ai_types::ModelCost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
                tiers: vec![],
            },
            context_window: 0,
            max_tokens: 0,
            headers: None,
            compat: None,
        }));
        assert_eq!(registry.get_api_key_for_provider("test-provider-xyz"), Some("sk-stored-123".to_string()));
        assert_eq!(registry.get_api_key_for_provider("anthropic"), None);
    }
}
