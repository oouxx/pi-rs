//! Shared helpers for simple (reasoning-aware) streaming options.
//!
//! Ported from `packages/ai/src/providers/simple-options.ts`.

use crate::types::{Model, SimpleStreamOptions, StreamOptions, ThinkingBudgets};

/// Build a full `StreamOptions` from `SimpleStreamOptions` and an API key.
pub fn build_base_options(_model: &Model, options: Option<&SimpleStreamOptions>, api_key: Option<&str>) -> StreamOptions {
    let opts = match options {
        Some(o) => o,
        None => {
            return StreamOptions {
                api_key: api_key.map(|s| s.to_string()),
                ..Default::default()
            };
        }
    };

    StreamOptions {
        temperature: opts.base.temperature,
        max_tokens: opts.base.max_tokens,
        signal: opts.base.signal.clone(),
        api_key: api_key.map(|s| s.to_string()).or_else(|| opts.base.api_key.clone()),
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
    }
}

/// Clamp reasoning effort — "xhigh" is treated as "high" for providers that
/// don't support it natively.
pub fn clamp_reasoning(effort: Option<&str>) -> Option<String> {
    match effort {
        Some("xhigh") => Some("high".to_string()),
        other => other.map(|s| s.to_string()),
    }
}

/// Adjust max_tokens to accommodate a thinking budget.
///
/// Returns the effective `max_tokens` and the `thinking_budget` in tokens.
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
        high: Some(16384),
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
        "high" => budgets.high.unwrap_or(16384),
        _ => budgets.high.unwrap_or(16384),
    };

    let min_output_tokens = 1024;
    let max_tokens = match base_max_tokens {
        None => model_max_tokens,
        Some(bmt) => (bmt + thinking_budget).min(model_max_tokens),
    };

    let thinking_budget = if max_tokens <= thinking_budget {
        (max_tokens.saturating_sub(min_output_tokens)).max(0)
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
    use super::*;

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
        let result = adjust_max_tokens_for_thinking(None, 200000, "medium", None);
        assert_eq!(result.max_tokens, 200000);
        assert_eq!(result.thinking_budget, 8192);
    }

    #[test]
    fn test_adjust_max_tokens_with_base() {
        let result = adjust_max_tokens_for_thinking(Some(4096), 200000, "medium", None);
        // 4096 + 8192 = 12288, which is less than model max of 200000
        assert_eq!(result.max_tokens, 12288);
        assert_eq!(result.thinking_budget, 8192);
    }

    #[test]
    fn test_adjust_max_tokens_clamped_to_model_max() {
        let result = adjust_max_tokens_for_thinking(Some(199000), 200000, "high", None);
        // 199000 + 16384 = 215384, clamped to 200000
        assert_eq!(result.max_tokens, 200000);
    }

    #[test]
    fn test_adjust_max_tokens_small_budget() {
        // base=1000 + high_budget=16384 = 17384, which is > thinking_budget so no reduction
        let result = adjust_max_tokens_for_thinking(Some(1000), 200000, "high", None);
        assert_eq!(result.max_tokens, 17384);
        assert_eq!(result.thinking_budget, 16384);
    }

    #[test]
    fn test_adjust_max_tokens_tiny_budget_triggers_reduction() {
        // base=0 + high_budget=16384. max_tokens=16384, which is NOT > thinking_budget (16384 <= 16384)
        // so thinking_budget = max(0, 16384 - 1024) = 15360
        let result = adjust_max_tokens_for_thinking(Some(0), 200000, "high", None);
        assert_eq!(result.max_tokens, 16384);
        assert!(result.thinking_budget < 16384);
        assert!(result.thinking_budget >= 1024);
    }

    #[test]
    fn test_build_base_options_no_options() {
        let model = crate::types::Model {
            id: "test".into(),
            name: "test".into(),
            api: "test".into(),
            provider: "test".into(),
            base_url: "".into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            cost: Default::default(),
            context_window: 0,
            max_tokens: 0,
            headers: None,
            compat: None,
        };
        let opts = build_base_options(&model, None, Some("key123"));
        assert_eq!(opts.api_key, Some("key123".to_string()));
        assert!(opts.temperature.is_none());
    }
}
