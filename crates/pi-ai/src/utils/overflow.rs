//! Context overflow detection for LLM provider responses.
//!
//! Ported from `packages/ai/src/utils/overflow.ts`.

use regex::Regex;

use crate::types::{AssistantMessage, StopReason};

/// Create a case-insensitive regex, matching the `/i` flag from the original TS patterns.
fn re(pattern: &str) -> Regex {
    Regex::new(&format!("(?i){}", pattern)).unwrap()
}

static OVERFLOW_PATTERNS: std::sync::LazyLock<Vec<Regex>> = std::sync::LazyLock::new(|| {
    vec![
        re(r"prompt is too long"),
        re(r"request_too_large"),
        re(r"input is too long for requested model"),
        re(r"exceeds the context window"),
        re(r"exceeds (?:the )?(?:model'?s )?maximum context length of [\d,]+ tokens?"),
        re(r"input token count.*exceeds the maximum"),
        re(r"maximum prompt length is \d+"),
        re(r"reduce the length of the messages"),
        re(r"maximum context length is \d+ tokens"),
        re(r"exceeds (?:the )?maximum allowed input length of [\d,]+ tokens?"),
        re(r"input \(\d+ tokens\) is longer than the model'?s context length \(\d+ tokens\)"),
        re(r"exceeds the limit of \d+"),
        re(r"exceeds the available context size"),
        re(r"greater than the context length"),
        re(r"context window exceeds limit"),
        re(r"exceeded model token limit"),
        re(r"too large for model with \d+ maximum context length"),
        re(r"model_context_window_exceeded"),
        re(r"prompt too long; exceeded (?:max )?context length"),
        re(r"context[_ ]length[_ ]exceeded"),
        re(r"too many tokens"),
        re(r"token limit exceeded"),
        re(r"^4(?:00|13)\s*(?:status code)?\s*\(no body\)"),
    ]
});

static NON_OVERFLOW_PATTERNS: std::sync::LazyLock<Vec<Regex>> = std::sync::LazyLock::new(|| {
    vec![
        re(r"^(Throttling error|Service unavailable):"),
        re(r"rate limit"),
        re(r"too many requests"),
    ]
});

/// Check if an assistant message represents a context overflow error.
///
/// Handles three cases:
/// 1. Error-based overflow: provider returns stopReason "error" with matching message
/// 2. Silent overflow: provider accepts overflow but usage exceeds contextWindow
/// 3. Length-stop overflow: provider truncates input, returns stopReason "length" with zero output
pub fn is_context_overflow(message: &AssistantMessage, context_window: Option<u64>) -> bool {
    // Case 1: Error message patterns
    if message.stop_reason == StopReason::Error {
        if let Some(ref error_msg) = message.error_message {
            let is_non_overflow = NON_OVERFLOW_PATTERNS.iter().any(|p| p.is_match(error_msg));
            if !is_non_overflow && OVERFLOW_PATTERNS.iter().any(|p| p.is_match(error_msg)) {
                return true;
            }
        }
    }

    // Case 2: Silent overflow (z.ai style) - successful but usage exceeds context
    if let Some(cw) = context_window {
        if message.stop_reason == StopReason::Stop {
            let input_tokens = message.usage.input + message.usage.cache_read;
            if input_tokens > cw {
                return true;
            }
        }
    }

    // Case 3: Length-stop overflow (Xiaomi MiMo style)
    if let Some(cw) = context_window {
        if message.stop_reason == StopReason::Length && message.usage.output == 0 {
            let input_tokens = message.usage.input + message.usage.cache_read;
            if input_tokens >= (cw as f64 * 0.99) as u64 {
                return true;
            }
        }
    }

    false
}

/// Get a copy of the overflow patterns for testing.
pub fn get_overflow_patterns() -> Vec<Regex> {
    OVERFLOW_PATTERNS.clone()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Usage, UsageCost};

    fn make_error_msg(msg: &str) -> AssistantMessage {
        AssistantMessage {
            content: vec![],
            api: "test".into(),
            provider: "test".into(),
            model: "test".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Error,
            error_message: Some(msg.into()),
            timestamp: 0,
        }
    }

    fn make_success_msg(input: u64, output: u64, stop_reason: StopReason) -> AssistantMessage {
        AssistantMessage {
            content: vec![],
            api: "test".into(),
            provider: "test".into(),
            model: "test".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage {
                input,
                output,
                cache_read: 0,
                cache_write: 0,
                total_tokens: input + output,
                cost: UsageCost::default(),
            },
            stop_reason,
            error_message: None,
            timestamp: 0,
        }
    }

    #[test]
    fn test_anthropic_overflow() {
        let msg = make_error_msg("prompt is too long: 213462 tokens > 200000 maximum");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_openai_overflow() {
        let msg = make_error_msg("Your input exceeds the context window of this model");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_openai_litellm_overflow() {
        let msg = make_error_msg(
            "Requested token count exceeds the model's maximum context length of 200,000 tokens",
        );
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_google_overflow() {
        let msg = make_error_msg("The input token count (1196265) exceeds the maximum number of tokens allowed (1048575)");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_xai_overflow() {
        let msg = make_error_msg(
            "This model's maximum prompt length is 131072 but the request contains 537812 tokens",
        );
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_groq_overflow() {
        let msg = make_error_msg("Please reduce the length of the messages or completion");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_openrouter_overflow() {
        let msg = make_error_msg("This endpoint's maximum context length is 128000 tokens. However, you requested about 200000 tokens");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_openrouter_poolside_overflow() {
        let msg = make_error_msg(
            "Input length 150000 exceeds the maximum allowed input length of 100000 tokens.",
        );
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_together_overflow() {
        let msg = make_error_msg(
            "The input (200000 tokens) is longer than the model's context length (128000 tokens).",
        );
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_llamacpp_overflow() {
        let msg =
            make_error_msg("the request exceeds the available context size, try increasing it");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_lm_studio_overflow() {
        let msg = make_error_msg(
            "tokens to keep from the initial prompt is greater than the context length",
        );
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_kimi_overflow() {
        let msg = make_error_msg("Your request exceeded model token limit: 5000 (requested: 6000)");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_mistral_overflow() {
        let msg = make_error_msg(
            "Prompt contains 200000 tokens, too large for model with 128000 maximum context length",
        );
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_cerebras_overflow() {
        let msg = make_error_msg("400 status code (no body)");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_non_overflow_rate_limit() {
        let msg = make_error_msg("Throttling error: Too many tokens, please wait");
        assert!(!is_context_overflow(&msg, None));
    }

    #[test]
    fn test_non_overflow_too_many_requests() {
        let msg = make_error_msg("too many requests: rate limited");
        assert!(!is_context_overflow(&msg, None));
    }

    #[test]
    fn test_silent_overflow_zai_style() {
        // Silent overflow: stopReason=stop but input exceeds context window
        let msg = make_success_msg(150000, 100, StopReason::Stop);
        assert!(is_context_overflow(&msg, Some(128000)));
    }

    #[test]
    fn test_length_stop_overflow_xiaomi_style() {
        // Length-stop overflow: stopReason=length, output=0, input fills context window
        let msg = make_success_msg(127000, 0, StopReason::Length);
        assert!(is_context_overflow(&msg, Some(128000)));
    }

    #[test]
    fn test_normal_length_stop_not_overflow() {
        // Normal length stop: output > 0, so not overflow
        let msg = make_success_msg(50000, 500, StopReason::Length);
        assert!(!is_context_overflow(&msg, Some(128000)));
    }

    #[test]
    fn test_normal_stop_not_overflow() {
        let msg = make_success_msg(5000, 500, StopReason::Stop);
        assert!(!is_context_overflow(&msg, Some(128000)));
    }

    #[test]
    fn test_no_error_no_overflow() {
        let msg = make_success_msg(100, 50, StopReason::Stop);
        assert!(!is_context_overflow(&msg, None));
    }

    #[test]
    fn test_ollama_overflow() {
        let msg = make_error_msg("prompt too long; exceeded max context length by 5000 tokens");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_generic_context_length_exceeded() {
        let msg = make_error_msg("Context length exceeded");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_generic_too_many_tokens() {
        let msg = make_error_msg("too many tokens in the prompt");
        assert!(is_context_overflow(&msg, None));
    }

    #[test]
    fn test_get_overflow_patterns_returns_copy() {
        let patterns = get_overflow_patterns();
        assert!(!patterns.is_empty());
        assert!(patterns[0].is_match("prompt is too long"));
    }
}
