use std::collections::HashMap;
use std::sync::LazyLock;

use pi_agent_core::pi_ai_types::Model;

use super::model_registry::ModelRegistry;
use super::system_prompt::DEFAULT_THINKING_LEVEL;

pub type ThinkingLevel = String;

// ============================================================================
// Default model per provider
// ============================================================================

/// Default model IDs for each known provider, matching the original TypeScript.
pub static DEFAULT_MODEL_PER_PROVIDER: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| {
        let mut m = HashMap::new();
        m.insert("amazon-bedrock", "us.anthropic.claude-opus-4-6-v1");
        m.insert("ant-ling", "Ring-2.6-1T");
        m.insert("anthropic", "claude-opus-4-8");
        m.insert("openai", "gpt-5.5");
        m.insert("azure-openai-responses", "gpt-5.4");
        m.insert("openai-codex", "gpt-5.5");
        m.insert("nvidia", "nvidia/nemotron-3-super-120b-a12b");
        m.insert("deepseek", "deepseek-v4-pro");
        m.insert("google", "gemini-3.1-pro-preview");
        m.insert("google-vertex", "gemini-3.1-pro-preview");
        m.insert("github-copilot", "gpt-5.4");
        m.insert("openrouter", "moonshotai/kimi-k2.6");
        m.insert("vercel-ai-gateway", "zai/glm-5.1");
        m.insert("xai", "grok-4.20-0309-reasoning");
        m.insert("groq", "openai/gpt-oss-120b");
        m.insert("cerebras", "zai-glm-4.7");
        m.insert("zai", "glm-5.1");
        m.insert("zai-coding-cn", "glm-5.1");
        m.insert("mistral", "devstral-medium-latest");
        m.insert("minimax", "MiniMax-M2.7");
        m.insert("minimax-cn", "MiniMax-M2.7");
        m.insert("moonshotai", "kimi-k2.6");
        m.insert("moonshotai-cn", "kimi-k2.6");
        m.insert("huggingface", "moonshotai/Kimi-K2.6");
        m.insert("fireworks", "accounts/fireworks/models/kimi-k2p6");
        m.insert("together", "moonshotai/Kimi-K2.6");
        m.insert("opencode", "kimi-k2.6");
        m.insert("opencode-go", "kimi-k2.6");
        m.insert("kimi-coding", "kimi-for-coding");
        m.insert("cloudflare-workers-ai", "@cf/moonshotai/kimi-k2.6");
        m.insert("cloudflare-ai-gateway", "workers-ai/@cf/moonshotai/kimi-k2.6");
        m.insert("xiaomi", "mimo-v2.5-pro");
        m.insert("xiaomi-token-plan-cn", "mimo-v2.5-pro");
        m.insert("xiaomi-token-plan-ams", "mimo-v2.5-pro");
        m.insert("xiaomi-token-plan-sgp", "mimo-v2.5-pro");
        m
    });

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone)]
pub struct ScopedModel {
    pub model: Model,
    pub thinking_level: Option<ThinkingLevel>,
}

#[derive(Debug, Clone)]
pub struct ParsedModelResult {
    pub model: Option<Model>,
    pub thinking_level: Option<ThinkingLevel>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ModelScopeDiagnostic {
    pub message: String,
    pub pattern: String,
}

#[derive(Debug, Clone)]
pub struct ResolveModelScopeResult {
    pub scoped_models: Vec<ScopedModel>,
    pub diagnostics: Vec<ModelScopeDiagnostic>,
}

#[derive(Debug, Clone)]
pub struct ResolveCliModelResult {
    pub model: Option<Model>,
    pub thinking_level: Option<ThinkingLevel>,
    pub warning: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InitialModelResult {
    pub model: Option<Model>,
    pub thinking_level: ThinkingLevel,
    pub fallback_message: Option<String>,
}

// ============================================================================
// Helpers
// ============================================================================

/// Check if a model ID looks like an alias (no date suffix).
fn is_alias(id: &str) -> bool {
    if id.ends_with("-latest") {
        return true;
    }
    // Check if ID ends with a date pattern (-YYYYMMDD)
    let date_pattern = regex::Regex::new(r"-\d{8}$").unwrap();
    !date_pattern.is_match(id)
}

/// Check if a string is a valid thinking level.
fn is_valid_thinking_level(s: &str) -> bool {
    matches!(s, "off" | "minimal" | "low" | "medium" | "high")
}

/// Try to match a pattern to a model from the available models list.
fn try_match_model(model_pattern: &str, available_models: &[Model]) -> Option<Model> {
    // Try exact match first
    if let Some(exact) = find_exact_model_reference_match(model_pattern, available_models) {
        return Some(exact);
    }

    // No exact match - fall back to partial matching
    let lower_pattern = model_pattern.to_lowercase();
    let matches: Vec<&Model> = available_models
        .iter()
        .filter(|m| {
            m.id.to_lowercase().contains(&lower_pattern)
                || m.name.to_lowercase().contains(&lower_pattern)
        })
        .collect();

    if matches.is_empty() {
        return None;
    }

    // Separate into aliases and dated versions
    let aliases: Vec<&&Model> = matches.iter().filter(|m| is_alias(&m.id)).collect();
    let dated: Vec<&&Model> = matches.iter().filter(|m| !is_alias(&m.id)).collect();

    if !aliases.is_empty() {
        // Prefer alias - if multiple aliases, pick the one that sorts highest
        let mut sorted = aliases.clone();
        sorted.sort_by(|a, b| b.id.cmp(&a.id));
        Some((*sorted[0]).clone())
    } else if !dated.is_empty() {
        let mut sorted = dated.clone();
        sorted.sort_by(|a, b| b.id.cmp(&a.id));
        Some((*sorted[0]).clone())
    } else {
        None
    }
}

// ============================================================================
// findExactModelReferenceMatch
// ============================================================================

/// Find an exact model reference match.
/// Supports either a bare model id or a canonical provider/modelId reference.
pub fn find_exact_model_reference_match(
    model_reference: &str,
    available_models: &[Model],
) -> Option<Model> {
    let trimmed = model_reference.trim();
    if trimmed.is_empty() {
        return None;
    }

    let normalized = trimmed.to_lowercase();

    // Check canonical "provider/modelId" format
    let canonical_matches: Vec<&Model> = available_models
        .iter()
        .filter(|m| format!("{}/{}", m.provider, m.id).to_lowercase() == normalized)
        .collect();

    if canonical_matches.len() == 1 {
        return Some(canonical_matches[0].clone());
    }
    if canonical_matches.len() > 1 {
        return None;
    }

    // Check for "provider/modelId" with explicit slash
    if let Some(slash_idx) = trimmed.find('/') {
        let provider = trimmed[..slash_idx].trim();
        let model_id = trimmed[slash_idx + 1..].trim();
        if !provider.is_empty() && !model_id.is_empty() {
            let provider_matches: Vec<&Model> = available_models
                .iter()
                .filter(|m| {
                    m.provider.to_lowercase() == provider.to_lowercase()
                        && m.id.to_lowercase() == model_id.to_lowercase()
                })
                .collect();
            if provider_matches.len() == 1 {
                return Some(provider_matches[0].clone());
            }
            if provider_matches.len() > 1 {
                return None;
            }
        }
    }

    // Match by bare model ID
    let id_matches: Vec<&Model> = available_models
        .iter()
        .filter(|m| m.id.to_lowercase() == normalized)
        .collect();

    if id_matches.len() == 1 {
        Some(id_matches[0].clone())
    } else {
        None
    }
}

// ============================================================================
// parseModelPattern
// ============================================================================

/// Parse a pattern to extract model and thinking level.
/// Handles models with colons in their IDs.
pub fn parse_model_pattern(
    pattern: &str,
    available_models: &[Model],
    allow_invalid_thinking_level_fallback: bool,
) -> ParsedModelResult {
    // Try exact match first
    if let Some(exact) = try_match_model(pattern, available_models) {
        return ParsedModelResult {
            model: Some(exact),
            thinking_level: None,
            warning: None,
        };
    }

    // No match - try splitting on last colon if present
    let last_colon = pattern.rfind(':');
    let last_colon = match last_colon {
        Some(idx) => idx,
        None => {
            return ParsedModelResult {
                model: None,
                thinking_level: None,
                warning: None,
            };
        }
    };

    let prefix = &pattern[..last_colon];
    let suffix = &pattern[last_colon + 1..];

    if is_valid_thinking_level(suffix) {
        // Valid thinking level - recurse on prefix
        let result = parse_model_pattern(prefix, available_models, allow_invalid_thinking_level_fallback);
        if result.model.is_some() {
            return ParsedModelResult {
                thinking_level: if result.warning.is_some() { None } else { Some(suffix.to_string()) },
                ..result
            };
        }
        return result;
    } else if allow_invalid_thinking_level_fallback {
        // Invalid suffix - recurse on prefix and warn
        let result = parse_model_pattern(prefix, available_models, allow_invalid_thinking_level_fallback);
        if result.model.is_some() {
            return ParsedModelResult {
                model: result.model,
                thinking_level: None,
                warning: Some(format!(
                    "Invalid thinking level \"{}\" in pattern \"{}\". Using default instead.",
                    suffix, pattern
                )),
            };
        }
        return result;
    } else {
        // Strict mode: treat as part of model id and fail
        ParsedModelResult {
            model: None,
            thinking_level: None,
            warning: None,
        }
    }
}

// ============================================================================
// resolveModelScopeWithDiagnostics / resolveModelScope
// ============================================================================

/// Resolve model patterns to actual Model objects with optional thinking levels.
pub fn resolve_model_scope_with_diagnostics(
    patterns: &[String],
    available_models: &[Model],
) -> ResolveModelScopeResult {
    let mut scoped_models: Vec<ScopedModel> = Vec::new();
    let mut diagnostics: Vec<ModelScopeDiagnostic> = Vec::new();

    for pattern in patterns {
        // Check if pattern contains glob characters
        if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
            let colon_idx = pattern.rfind(':');
            let (glob_pattern, thinking_level) = if let Some(idx) = colon_idx {
                let suffix = &pattern[idx + 1..];
                if is_valid_thinking_level(suffix) {
                    (&pattern[..idx], Some(suffix.to_string()))
                } else {
                    (pattern.as_str(), None)
                }
            } else {
                (pattern.as_str(), None)
            };

            // Match against "provider/modelId" format OR just model ID
            let matching_models: Vec<Model> = available_models
                .iter()
                .filter(|m| {
                    let full_id = format!("{}/{}", m.provider, m.id);
                    glob_match(glob_pattern, &full_id) || glob_match(glob_pattern, &m.id)
                })
                .cloned()
                .collect();

            if matching_models.is_empty() {
                diagnostics.push(ModelScopeDiagnostic {
                    message: format!("No models match pattern \"{}\"", pattern),
                    pattern: pattern.clone(),
                });
                continue;
            }

            for model in matching_models {
                if !scoped_models.iter().any(|sm| sm.model.id == model.id && sm.model.provider == model.provider) {
                    scoped_models.push(ScopedModel {
                        thinking_level: thinking_level.clone(),
                        model,
                    });
                }
            }
            continue;
        }

        let result = parse_model_pattern(pattern, available_models, true);

        if let Some(ref warning) = result.warning {
            diagnostics.push(ModelScopeDiagnostic {
                message: warning.clone(),
                pattern: pattern.clone(),
            });
        }

        if let Some(model) = result.model {
            if !scoped_models.iter().any(|sm| sm.model.id == model.id && sm.model.provider == model.provider) {
                scoped_models.push(ScopedModel {
                    thinking_level: result.thinking_level,
                    model,
                });
            }
        } else {
            diagnostics.push(ModelScopeDiagnostic {
                message: format!("No models match pattern \"{}\"", pattern),
                pattern: pattern.clone(),
            });
        }
    }

    ResolveModelScopeResult {
        scoped_models,
        diagnostics,
    }
}

/// Simple glob matching (supports * and ?).
fn glob_match(pattern: &str, text: &str) -> bool {
    let regex_pattern = format!(
        "^{}$",
        regex::escape(pattern)
            .replace(r"\*", ".*")
            .replace(r"\?", ".")
    );
    regex::Regex::new(&regex_pattern)
        .map(|re| re.is_match(text))
        .unwrap_or(false)
}

/// Resolve model patterns, printing warnings to stderr.
pub fn resolve_model_scope(patterns: &[String], available_models: &[Model]) -> Vec<ScopedModel> {
    let result = resolve_model_scope_with_diagnostics(patterns, available_models);
    for diagnostic in &result.diagnostics {
        eprintln!("Warning: {}", diagnostic.message);
    }
    result.scoped_models
}

// ============================================================================
// resolveCliModel
// ============================================================================

/// Resolve a single model from CLI flags.
pub fn resolve_cli_model(
    cli_provider: Option<&str>,
    cli_model: Option<&str>,
    model_registry: &ModelRegistry,
) -> ResolveCliModelResult {
    let cli_model = match cli_model {
        Some(m) => m,
        None => {
            return ResolveCliModelResult {
                model: None,
                thinking_level: None,
                warning: None,
                error: None,
            };
        }
    };

    let available_models = model_registry.get_models();
    if available_models.is_empty() {
        return ResolveCliModelResult {
            model: None,
            thinking_level: None,
            warning: None,
            error: Some(
                "No models available. Check your installation or add models to models.json.".to_string(),
            ),
        };
    }

    // Build canonical provider lookup (case-insensitive)
    let provider_map: HashMap<String, String> = available_models
        .iter()
        .map(|m| (m.provider.to_lowercase(), m.provider.clone()))
        .collect();

    let mut provider = cli_provider
        .and_then(|p| provider_map.get(&p.to_lowercase()).cloned());

    // If no explicit --provider, try to interpret "provider/model" format
    let mut pattern = cli_model;
    let mut inferred_provider = false;

    if provider.is_none() {
        if let Some(slash_idx) = cli_model.find('/') {
            let maybe_provider = &cli_model[..slash_idx];
            if let Some(canonical) = provider_map.get(&maybe_provider.to_lowercase()) {
                provider = Some(canonical.clone());
                pattern = &cli_model[slash_idx + 1..];
                inferred_provider = true;
            }
        }
    }

    // If no provider was inferred, try exact matches
    if provider.is_none() {
        let lower = cli_model.to_lowercase();
        if let Some(exact) = available_models.iter().find(|m| {
            m.id.to_lowercase() == lower
                || format!("{}/{}", m.provider, m.id).to_lowercase() == lower
        }) {
            return ResolveCliModelResult {
                model: Some(exact.clone()),
                thinking_level: None,
                warning: None,
                error: None,
            };
        }
    }

    // If both provider and model were provided, strip provider prefix from pattern
    if cli_provider.is_some() && provider.is_some() {
        if let Some(ref prov) = provider {
            let prefix = format!("{}/", prov.to_lowercase());
            if cli_model.to_lowercase().starts_with(&prefix) {
                pattern = &cli_model[prefix.len()..];
            }
        }
    }

    let candidates: Vec<&Model> = if let Some(ref prov) = provider {
        available_models.iter().filter(|m| m.provider == *prov).collect()
    } else {
        available_models.iter().collect()
    };

    let candidates_owned: Vec<Model> = candidates.into_iter().cloned().collect();
    let result = parse_model_pattern(pattern, &candidates_owned, false);

    if let Some(model) = result.model {
        return ResolveCliModelResult {
            model: Some(model),
            thinking_level: result.thinking_level,
            warning: result.warning,
            error: None,
        };
    }

    // If we inferred a provider but found no match, fall back to full input
    if inferred_provider {
        let lower = cli_model.to_lowercase();
        if let Some(exact) = available_models.iter().find(|m| {
            m.id.to_lowercase() == lower
                || format!("{}/{}", m.provider, m.id).to_lowercase() == lower
        }) {
            return ResolveCliModelResult {
                model: Some(exact.clone()),
                thinking_level: None,
                warning: None,
                error: None,
            };
        }
        // Also try parseModelPattern on the full input
        let fallback = parse_model_pattern(cli_model, &available_models, false);
        if let Some(model) = fallback.model {
            return ResolveCliModelResult {
                model: Some(model),
                thinking_level: fallback.thinking_level,
                warning: fallback.warning,
                error: None,
            };
        }
    }

    ResolveCliModelResult {
        model: None,
        thinking_level: None,
        warning: None,
        error: Some(format!(
            "No model found matching \"{}\".",
            cli_model
        )),
    }
}

// ============================================================================
// findInitialModel / restoreModelFromSession
// ============================================================================

pub fn find_initial_model(
    cli_provider: Option<&str>,
    cli_model: Option<&str>,
    scoped_models: &[ScopedModel],
    is_continuing: bool,
    default_provider: Option<&str>,
    default_model_id: Option<&str>,
    default_thinking_level: Option<&str>,
    model_registry: &ModelRegistry,
) -> InitialModelResult {
    if let (Some(provider), Some(model_id)) = (cli_provider, cli_model) {
        if let Some(model) = model_registry.find(provider, model_id) {
            return InitialModelResult {
                model: Some(model),
                thinking_level: DEFAULT_THINKING_LEVEL.to_string(),
                fallback_message: None,
            };
        }
    }

    if !scoped_models.is_empty() && !is_continuing {
        return InitialModelResult {
            model: Some(scoped_models[0].model.clone()),
            thinking_level: scoped_models[0].thinking_level.clone().unwrap_or_else(|| {
                default_thinking_level
                    .unwrap_or(DEFAULT_THINKING_LEVEL)
                    .to_string()
            }),
            fallback_message: None,
        };
    }

    if let (Some(provider), Some(model_id)) = (default_provider, default_model_id) {
        if let Some(model) = model_registry.find(provider, model_id) {
            return InitialModelResult {
                model: Some(model),
                thinking_level: default_thinking_level
                    .unwrap_or(DEFAULT_THINKING_LEVEL)
                    .to_string(),
                fallback_message: None,
            };
        }
    }

    let available = model_registry.get_available();
    if !available.is_empty() {
        let default_models = [
            ("anthropic", "claude-sonnet-4-6"),
            ("openai", "gpt-4o"),
            ("google", "gemini-2.5-flash"),
            ("deepseek", "deepseek-chat"),
        ];
        for (provider, model_id) in &default_models {
            if let Some(model) = available
                .iter()
                .find(|m| m.provider == *provider && m.id == *model_id)
            {
                return InitialModelResult {
                    model: Some(model.clone()),
                    thinking_level: DEFAULT_THINKING_LEVEL.to_string(),
                    fallback_message: None,
                };
            }
        }
        return InitialModelResult {
            model: Some(available[0].clone()),
            thinking_level: DEFAULT_THINKING_LEVEL.to_string(),
            fallback_message: None,
        };
    }

    InitialModelResult {
        model: None,
        thinking_level: DEFAULT_THINKING_LEVEL.to_string(),
        fallback_message: None,
    }
}

pub fn restore_model_from_session(
    saved_provider: &str,
    saved_model_id: &str,
    current_model: Option<&Model>,
    model_registry: &ModelRegistry,
) -> InitialModelResult {
    let restored = model_registry.find(saved_provider, saved_model_id);
    let has_auth = restored
        .as_ref()
        .map(|m| model_registry.has_configured_auth(m))
        .unwrap_or(false);

    if let Some(ref model) = restored {
        if has_auth {
            return InitialModelResult {
                model: Some(model.clone()),
                thinking_level: DEFAULT_THINKING_LEVEL.to_string(),
                fallback_message: None,
            };
        }
    }

    let reason = if restored.is_none() {
        "model no longer exists"
    } else {
        "no auth configured"
    };

    if let Some(current) = current_model {
        return InitialModelResult {
            model: Some(current.clone()),
            thinking_level: DEFAULT_THINKING_LEVEL.to_string(),
            fallback_message: Some(format!(
                "Could not restore model {}/{} ({}). Using {}/{}.",
                saved_provider, saved_model_id, reason, current.provider, current.id
            )),
        };
    }

    let available = model_registry.get_available();
    if let Some(fallback) = available.first() {
        return InitialModelResult {
            model: Some(fallback.clone()),
            thinking_level: DEFAULT_THINKING_LEVEL.to_string(),
            fallback_message: Some(format!(
                "Could not restore model {}/{} ({}). Using {}/{}.",
                saved_provider, saved_model_id, reason, fallback.provider, fallback.id
            )),
        };
    }

    InitialModelResult {
        model: None,
        thinking_level: DEFAULT_THINKING_LEVEL.to_string(),
        fallback_message: None,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model_registry::ModelRegistry;

    fn test_registry() -> ModelRegistry {
        pi_agent_core::pi_ai::providers::register_builtins::register_built_in_api_providers();
        ModelRegistry::new(ModelRegistry::builtin_models_list())
    }

    fn test_models() -> Vec<Model> {
        pi_agent_core::pi_ai::providers::register_builtins::register_built_in_api_providers();
        ModelRegistry::builtin_models_list()
    }

    #[test]
    fn test_find_exact_model_reference_match_canonical() {
        let models = test_models();
        let result = find_exact_model_reference_match("anthropic/claude-sonnet-4-6", &models);
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "claude-sonnet-4-6");
    }

    #[test]
    fn test_find_exact_model_reference_match_bare_id() {
        let models = test_models();
        let result = find_exact_model_reference_match("gpt-4o", &models);
        assert!(result.is_some());
    }

    #[test]
    fn test_find_exact_model_reference_match_empty() {
        let models = test_models();
        assert!(find_exact_model_reference_match("", &models).is_none());
    }

    #[test]
    fn test_parse_model_pattern_exact() {
        let models = test_models();
        let result = parse_model_pattern("claude-sonnet-4-6", &models, true);
        assert!(result.model.is_some());
        assert_eq!(result.model.unwrap().id, "claude-sonnet-4-6");
    }

    #[test]
    fn test_parse_model_pattern_with_thinking_level() {
        let models = test_models();
        let result = parse_model_pattern("claude-sonnet-4-6:high", &models, true);
        assert!(result.model.is_some());
        assert_eq!(result.thinking_level, Some("high".to_string()));
    }

    #[test]
    fn test_parse_model_pattern_no_match() {
        let models = test_models();
        let result = parse_model_pattern("nonexistent-model", &models, true);
        assert!(result.model.is_none());
    }

    #[test]
    fn test_resolve_model_scope_empty() {
        let models = test_models();
        let result = resolve_model_scope_with_diagnostics(&[], &models);
        assert!(result.scoped_models.is_empty());
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_resolve_cli_model_with_provider() {
        let registry = test_registry();
        let result = resolve_cli_model(Some("anthropic"), Some("claude-sonnet-4-6"), &registry);
        assert!(result.model.is_some());
        assert_eq!(result.model.unwrap().id, "claude-sonnet-4-6");
    }

    #[test]
    fn test_resolve_cli_model_no_model() {
        let registry = test_registry();
        let result = resolve_cli_model(None, None, &registry);
        assert!(result.model.is_none());
        assert!(result.error.is_none());
    }

    #[test]
    fn test_find_initial_model_cli() {
        let registry = test_registry();
        let result = find_initial_model(
            Some("anthropic"),
            Some("claude-sonnet-4-6"),
            &[],
            false,
            None,
            None,
            None,
            &registry,
        );
        assert!(result.model.is_some());
        assert_eq!(result.model.unwrap().id, "claude-sonnet-4-6");
    }

    #[test]
    fn test_find_initial_model_scoped() {
        let registry = test_registry();
        let model = registry.find("openai", "gpt-4o").unwrap();
        let scoped = vec![ScopedModel {
            model,
            thinking_level: Some("high".to_string()),
        }];
        let result = find_initial_model(None, None, &scoped, false, None, None, None, &registry);
        assert!(result.model.is_some());
        assert_eq!(result.model.unwrap().id, "gpt-4o");
        assert_eq!(result.thinking_level, "high");
    }

    #[test]
    fn test_find_initial_model_default() {
        let registry = test_registry();
        let result = find_initial_model(
            None,
            None,
            &[],
            false,
            Some("openai"),
            Some("gpt-4o"),
            None,
            &registry,
        );
        assert!(result.model.is_some());
        assert_eq!(result.model.unwrap().id, "gpt-4o");
    }

    #[test]
    fn test_restore_model_from_session() {
        let registry = test_registry();
        let current = registry.find("openai", "gpt-4o").unwrap();
        let result =
            restore_model_from_session("anthropic", "claude-sonnet-4-6", Some(&current), &registry);
        assert!(result.model.is_some());
    }

    #[test]
    fn test_restore_model_not_found() {
        let registry = test_registry();
        let current = registry.find("openai", "gpt-4o").unwrap();
        let result = restore_model_from_session("nonexistent", "model", Some(&current), &registry);
        assert!(result.model.is_some());
        assert!(result.fallback_message.is_some());
    }

    #[test]
    fn test_is_alias() {
        assert!(is_alias("claude-sonnet-4-6"));
        assert!(is_alias("gpt-4o-latest"));
        assert!(!is_alias("claude-sonnet-4-6-20250929"));
    }

    #[test]
    fn test_default_model_per_provider() {
        let map = &*DEFAULT_MODEL_PER_PROVIDER;
        assert_eq!(map.get("anthropic"), Some(&"claude-opus-4-8"));
        assert_eq!(map.get("openai"), Some(&"gpt-5.5"));
        assert_eq!(map.get("deepseek"), Some(&"deepseek-v4-pro"));
    }
}
