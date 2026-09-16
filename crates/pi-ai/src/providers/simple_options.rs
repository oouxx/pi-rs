//! Shared helpers for simple (reasoning-aware) streaming options.
//!
//! Ported from `packages/ai/src/providers/simple-options.ts`.

use crate::types::{Context, Model, SimpleStreamOptions, StreamOptions, ThinkingBudgets};

const CONTEXT_SAFETY_TOKENS: i64 = 4096;
const MIN_MAX_TOKENS: u64 = 1;
/// Tokens always left for the answer when a thinking budget shares the response ceiling.
pub const MIN_ANSWER_TOKENS: u64 = 1024;

/// TS `clampMaxTokensToContext`: keep the requested output under the model's
/// context window (leaving room for the estimated prompt + a safety margin).
#[must_use]
pub fn clamp_max_tokens_to_context(model: &Model, context: &Context, max_tokens: u64) -> u64 {
    if model.context_window == 0 {
        return max_tokens.max(MIN_MAX_TOKENS);
    }
    let estimated = crate::utils::estimate::estimate_context_tokens(context).tokens;
    let available = model.context_window as i64 - estimated as i64 - CONTEXT_SAFETY_TOKENS;
    max_tokens.min(available.max(MIN_MAX_TOKENS as i64) as u64)
}

/// TS `thinkingBudgetForLevel`: per-level budget with custom overrides.
#[must_use]
pub fn thinking_budget_for_level(level: &str, custom_budgets: Option<&ThinkingBudgets>) -> u64 {
    let defaults = ThinkingBudgets {
        minimal: Some(1024),
        low: Some(2048),
        medium: Some(8192),
        high: Some(16_384),
    };
    let level = if level == "xhigh" || level == "max" { "high" } else { level };
    let budget = match custom_budgets {
        Some(cb) => match level {
            "minimal" => cb.minimal.or(defaults.minimal),
            "low" => cb.low.or(defaults.low),
            "medium" => cb.medium.or(defaults.medium),
            _ => cb.high.or(defaults.high),
        },
        None => match level {
            "minimal" => defaults.minimal,
            "low" => defaults.low,
            "medium" => defaults.medium,
            _ => defaults.high,
        },
    };
    budget.unwrap_or(16_384)
}

/// TS `clampThinkingBudgetToAnswerRoom`: cap the budget so at least
/// `MIN_ANSWER_TOKENS` remain under the shared response ceiling.
#[must_use]
pub fn clamp_thinking_budget_to_answer_room(thinking_budget: u64, ceiling: u64) -> u64 {
    thinking_budget.min(ceiling.saturating_sub(MIN_ANSWER_TOKENS))
}

/// Build a full `StreamOptions` from `SimpleStreamOptions` and an API key.
#[must_use]
pub fn build_base_options(
    model: &Model,
    context: &Context,
    options: Option<&SimpleStreamOptions>,
    api_key: Option<&str>,
) -> StreamOptions {
    let Some(opts) = options else {
        return StreamOptions {
            api_key: api_key.map(std::string::ToString::to_string),
            max_tokens: Some(clamp_max_tokens_to_context(
                model,
                context,
                model.max_tokens,
            )),
            sampling_params: model.sampling_params.clone(),
            ..Default::default()
        };
    };

    // TS `simple-options.ts`: per-request samplingParams override the
    // model-level defaults key by key.
    let sampling_params = match (&model.sampling_params, opts.base.sampling_params.as_ref()) {
        (Some(m), Some(o)) => {
            let mut merged = m.clone();
            for (k, v) in o {
                merged.insert(k.clone(), v.clone());
            }
            Some(merged)
        }
        (Some(m), None) => Some(m.clone()),
        (None, Some(o)) => Some(o.clone()),
        (None, None) => None,
    };

    StreamOptions {
        temperature: opts.base.temperature,
        // TS `buildBaseOptions` clamps the requested output to the context.
        max_tokens: Some(clamp_max_tokens_to_context(
            model,
            context,
            opts.base.max_tokens.unwrap_or(model.max_tokens),
        )),
        sampling_params,
        http_client: opts.base.http_client.clone(),
        signal: opts.base.signal.clone(),
        api_key: api_key
            .map(std::string::ToString::to_string)
            .or_else(|| opts.base.api_key.clone()),
        transport: opts.base.transport.clone(),
        cache_retention: opts.base.cache_retention.clone(),
        session_id: opts.base.session_id.clone(),
        headers: opts.base.headers.clone(),
        timeout_ms: opts.base.timeout_ms,
        websocket_connect_timeout_ms: opts.base.websocket_connect_timeout_ms,
        max_retries: opts.base.max_retries,
        max_retry_delay_ms: opts.base.max_retry_delay_ms,
        metadata: opts.base.metadata.clone(),
        tool_choice: opts.base.tool_choice.clone(),
        service_tier: opts.base.service_tier.clone(),
        reasoning_effort: opts.reasoning.clone(),
        thinking_budgets: opts.thinking_budgets.clone(),
        debug: opts.debug,
        // Hooks must survive the simple→full options conversion (match TS
        // `buildBaseOptions`, which spreads `onPayload`/`onResponse`).
        on_payload: opts.base.on_payload.clone(),
        on_headers: opts.base.on_headers.clone(),
        on_provider_response: opts.base.on_provider_response.clone(),
    }
}

/// Clamp reasoning effort — "xhigh" is treated as "high" for providers that
/// don't support it natively.
#[must_use] 
#[allow(clippy::single_option_map)]
pub fn clamp_reasoning(effort: Option<&str>) -> Option<String> {
    effort.map(|e| if e == "xhigh" { "high".to_string() } else { e.to_string() })
}

/// Adjust `max_tokens` to accommodate a thinking budget.
///
/// Returns the effective `max_tokens` and the `thinking_budget` in tokens.
#[must_use] 
pub fn adjust_max_tokens_for_thinking(
    base_max_tokens: Option<u64>,
    model_max_tokens: u64,
    reasoning_level: &str,
    custom_budgets: Option<&ThinkingBudgets>,
) -> AdjustedThinking {
    let default_budgets = ThinkingBudgets {
        minimal: Some(1024),
        low: Some(2048),
        medium: Some(8192),
        high: Some(16_384),
    };

    let budgets = match custom_budgets {
        Some(cb) => ThinkingBudgets {
            minimal: cb.minimal.or(default_budgets.minimal),
            low: cb.low.or(default_budgets.low),
            medium: cb.medium.or(default_budgets.medium),
            high: cb.high.or(default_budgets.high),
        },
        None => default_budgets,
    };

    let thinking_budget = match reasoning_level {
        "minimal" => budgets.minimal.unwrap_or(1024),
        "low" => budgets.low.unwrap_or(2048),
        "medium" => budgets.medium.unwrap_or(8192),
        "high" => budgets.high.unwrap_or(16_384),
        _ => budgets.high.unwrap_or(16_384),
    };

    let min_output_tokens = 1024;
    let max_tokens = base_max_tokens.map_or(model_max_tokens, |bmt| (bmt + thinking_budget).min(model_max_tokens));

    let thinking_budget = if max_tokens <= thinking_budget {
        max_tokens.saturating_sub(min_output_tokens)
    } else {
        thinking_budget
    };

    AdjustedThinking {
        max_tokens,
        thinking_budget,
    }
}

#[derive(Debug, Clone)]
pub struct AdjustedThinking {
    pub max_tokens: u64,
    pub thinking_budget: u64,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn test_model(context_window: u64, max_tokens: u64) -> Model {
        Model {
            id: "m".into(),
            name: "m".into(),
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            base_url: String::new(),
            reasoning: true,
            thinking_level_map: None,
            input: vec![],
            cost: crate::types::ModelCost::default(),
            context_window,
            max_tokens,
            sampling_params: None,
            headers: None,
            compat: None,
        }
    }

    fn empty_context() -> Context {
        Context {
            system_prompt: None,
            messages: vec![],
            tools: None,
        }
    }

    /// TS `clampMaxTokensToContext`: leave `contextWindow - estimate - 4096`.
    #[test]
    fn test_clamp_max_tokens_to_context() {
        let model = test_model(200_000, 64_000);
        assert_eq!(
            clamp_max_tokens_to_context(&model, &empty_context(), 300_000),
            195_904
        );
        assert_eq!(
            clamp_max_tokens_to_context(&model, &empty_context(), 1_000),
            1_000
        );
    }

    /// A model without a context window is not clamped (match TS).
    #[test]
    fn test_clamp_max_tokens_zero_context_uncapped() {
        let model = test_model(0, 64_000);
        assert_eq!(
            clamp_max_tokens_to_context(&model, &empty_context(), 12_345),
            12_345
        );
    }

    #[test]
    fn test_thinking_budget_for_level() {
        assert_eq!(thinking_budget_for_level("minimal", None), 1_024);
        assert_eq!(thinking_budget_for_level("low", None), 2_048);
        assert_eq!(thinking_budget_for_level("medium", None), 8_192);
        assert_eq!(thinking_budget_for_level("high", None), 16_384);
        // xhigh/max clamp to the high budget (match TS `clampReasoning`).
        assert_eq!(thinking_budget_for_level("xhigh", None), 16_384);
        assert_eq!(thinking_budget_for_level("max", None), 16_384);
    }

    /// TS `clampThinkingBudgetToAnswerRoom`: keep `MIN_ANSWER_TOKENS` free.
    #[test]
    fn test_clamp_thinking_budget_to_answer_room() {
        assert_eq!(clamp_thinking_budget_to_answer_room(16_000, 12_000), 10_976);
        assert_eq!(clamp_thinking_budget_to_answer_room(1_000, 12_000), 1_000);
    }

    #[test]
    fn test_clamp_reasoning_xhigh() {
        assert_eq!(clamp_reasoning(Some("xhigh")), Some("high".to_string()));
    }

    #[test]
    fn test_clamp_reasoning_high() {
        assert_eq!(clamp_reasoning(Some("high")), Some("high".to_string()));
    }

    #[test]
    fn test_clamp_reasoning_none() {
        assert_eq!(clamp_reasoning(None), None);
    }

    #[test]
    fn test_adjust_max_tokens_no_base() {
        let result = adjust_max_tokens_for_thinking(None, 200_000, "medium", None);
        assert_eq!(result.max_tokens, 200_000);
        assert_eq!(result.thinking_budget, 8192);
    }

    #[test]
    fn test_adjust_max_tokens_with_base() {
        let result = adjust_max_tokens_for_thinking(Some(4096), 200_000, "medium", None);
        // 4096 + 8192 = 12_288, which is less than model max of 200_000
        assert_eq!(result.max_tokens, 12_288);
        assert_eq!(result.thinking_budget, 8192);
    }

    #[test]
    fn test_adjust_max_tokens_clamped_to_model_max() {
        let result = adjust_max_tokens_for_thinking(Some(199_000), 200_000, "high", None);
        // 199_000 + 16_384 = 215_384, clamped to 200_000
        assert_eq!(result.max_tokens, 200_000);
    }

    #[test]
    fn test_adjust_max_tokens_small_budget() {
        // base=1000 + high_budget=16_384 = 17_384, which is > thinking_budget so no reduction
        let result = adjust_max_tokens_for_thinking(Some(1000), 200_000, "high", None);
        assert_eq!(result.max_tokens, 17_384);
        assert_eq!(result.thinking_budget, 16_384);
    }

    #[test]
    fn test_adjust_max_tokens_tiny_budget_triggers_reduction() {
        // base=0 + high_budget=16_384. max_tokens=16_384, which is NOT > thinking_budget (16_384 <= 16_384)
        // so thinking_budget = max(0, 16_384 - 1024) = 15_360
        let result = adjust_max_tokens_for_thinking(Some(0), 200_000, "high", None);
        assert_eq!(result.max_tokens, 16_384);
        assert!(result.thinking_budget < 16_384);
        assert!(result.thinking_budget >= 1024);
    }

    #[test]
    fn test_build_base_options_no_options() {
        let model = crate::types::Model {
            id: "test".into(),
            name: "test".into(),
            api: "test".into(),
            provider: "test".into(),
            base_url: String::new(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            cost: crate::types::ModelCost::default(),
            context_window: 0,
            max_tokens: 0,
            sampling_params: None,
            headers: None,
            compat: None,
        };
        let opts = build_base_options(&model, &Context { system_prompt: None, messages: vec![], tools: None }, None, Some("key123"));
        assert_eq!(opts.api_key, Some("key123".to_string()));
        assert!(opts.temperature.is_none());
    }

    /// TS 0.84: per-request samplingParams override the model-level defaults
    /// key by key.
    #[test]
    fn test_build_base_options_merges_sampling_params() {
        let model = crate::types::Model {
            id: "test".into(),
            name: "test".into(),
            api: "openai-completions".into(),
            provider: "test".into(),
            base_url: String::new(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            cost: crate::types::ModelCost::default(),
            context_window: 0,
            max_tokens: 0,
            sampling_params: Some(
                serde_json::json!({"temperature": 0.7, "repetition_penalty": 1.1})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            headers: None,
            compat: None,
        };
        let simple = crate::types::SimpleStreamOptions {
            base: crate::types::StreamOptions {
                sampling_params: Some(
                    serde_json::json!({"temperature": 0.2, "top_p": 0.9})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
                ..Default::default()
            },
            reasoning: None,
            thinking_budgets: None,
            debug: None,
        };
        let opts = build_base_options(&model, &Context { system_prompt: None, messages: vec![], tools: None }, Some(&simple), None);
        let sp = opts.sampling_params.unwrap();
        assert_eq!(sp.get("temperature").unwrap(), &serde_json::json!(0.2), "request overrides model");
        assert_eq!(sp.get("repetition_penalty").unwrap(), &serde_json::json!(1.1), "model default kept");
        assert_eq!(sp.get("top_p").unwrap(), &serde_json::json!(0.9));
    }

    /// TS 0.83 per-request `fetch` injection: a custom HTTP client supplied
    /// in the simple options flows through to the full stream options.
    #[test]
    fn test_build_base_options_http_client_injection() {
        let model = crate::types::Model {
            id: "test".into(),
            name: "test".into(),
            api: "openai-completions".into(),
            provider: "test".into(),
            base_url: String::new(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            cost: crate::types::ModelCost::default(),
            context_window: 0,
            max_tokens: 0,
            sampling_params: None,
            headers: None,
            compat: None,
        };
        let client = std::sync::Arc::new(reqwest::Client::new());
        let simple = crate::types::SimpleStreamOptions {
            base: crate::types::StreamOptions {
                http_client: Some(client.clone()),
                ..Default::default()
            },
            reasoning: None,
            thinking_budgets: None,
            debug: None,
        };
        let opts = build_base_options(&model, &Context { system_prompt: None, messages: vec![], tools: None }, Some(&simple), None);
        assert!(
            std::sync::Arc::ptr_eq(opts.http_client.as_ref().unwrap(), &client),
            "custom http client must be preserved"
        );
    }
}
