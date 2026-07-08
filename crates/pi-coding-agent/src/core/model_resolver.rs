use std::collections::HashMap;
use std::sync::LazyLock;

use pi_agent_core::pi_ai_types::Model;

use super::model_registry::ModelRegistry;
use super::system_prompt::DEFAULT_THINKING_LEVEL;

pub type ThinkingLevel = String;

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

#[derive(Debug, Clone)]
pub struct ScopedModel {
    pub model: Model,
    pub thinking_level: Option<ThinkingLevel>,
}

#[derive(Debug, Clone)]
pub struct InitialModelResult {
    pub model: Option<Model>,
    pub thinking_level: ThinkingLevel,
    pub fallback_message: Option<String>,
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model_registry::builtin_models;

    fn test_registry() -> ModelRegistry {
        ModelRegistry::new(builtin_models())
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
}
