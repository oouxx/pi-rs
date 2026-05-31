use pi_agent_core::pi_ai_types::Model;

#[derive(Debug, Clone, Default)]
pub struct ContextUsage {
    pub total_tokens: u64,
    pub context_window: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub messages_count: usize,
}

impl ContextUsage {
    pub fn usage_percentage(&self) -> f64 {
        if self.context_window == 0 {
            return 0.0;
        }
        (self.total_tokens as f64 / self.context_window as f64) * 100.0
    }

    pub fn remaining_tokens(&self) -> u64 {
        self.context_window.saturating_sub(self.total_tokens)
    }

    pub fn is_near_limit(&self, threshold: f64) -> bool {
        self.usage_percentage() >= threshold
    }

    pub fn format_usage(&self) -> String {
        let pct = self.usage_percentage();
        let remaining = self.remaining_tokens();
        let mut parts = vec![format!(
            "Context: {:.1}% ({}/{} tokens, {} remaining)",
            pct,
            format_tokens(self.total_tokens),
            format_tokens(self.context_window),
            format_tokens(remaining),
        )];

        if self.input_tokens > 0 || self.output_tokens > 0 {
            parts.push(format!(
                "Session: {} in / {} out",
                format_tokens(self.input_tokens),
                format_tokens(self.output_tokens),
            ));
        }

        if let Some(cache_read) = self.cache_read_tokens {
            if cache_read > 0 {
                parts.push(format!("Cache: {} read", format_tokens(cache_read)));
            }
        }

        parts.join(" | ")
    }
}

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        format!("{}", n)
    }
}

pub fn estimate_tokens(text: &str) -> u64 {
    let char_count = text.len() as u64;
    char_count / 4
}

pub fn compute_context_usage(
    model: &Model,
    messages_count: usize,
    total_input_tokens: u64,
    total_output_tokens: u64,
    cache_read_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
) -> ContextUsage {
    let total_tokens = total_input_tokens + total_output_tokens;
    ContextUsage {
        total_tokens,
        context_window: model.context_window,
        input_tokens: total_input_tokens,
        output_tokens: total_output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        messages_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_usage_percentage() {
        let usage = ContextUsage {
            total_tokens: 50000,
            context_window: 100000,
            input_tokens: 40000,
            output_tokens: 10000,
            cache_read_tokens: None,
            cache_write_tokens: None,
            messages_count: 10,
        };
        assert!((usage.usage_percentage() - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_context_usage_remaining() {
        let usage = ContextUsage {
            total_tokens: 30000,
            context_window: 100000,
            input_tokens: 25000,
            output_tokens: 5000,
            cache_read_tokens: None,
            cache_write_tokens: None,
            messages_count: 5,
        };
        assert_eq!(usage.remaining_tokens(), 70000);
    }

    #[test]
    fn test_context_usage_near_limit() {
        let usage = ContextUsage {
            total_tokens: 85000,
            context_window: 100000,
            input_tokens: 70000,
            output_tokens: 15000,
            cache_read_tokens: None,
            cache_write_tokens: None,
            messages_count: 20,
        };
        assert!(usage.is_near_limit(80.0));
        assert!(!usage.is_near_limit(90.0));
    }

    #[test]
    fn test_format_tokens() {
        assert_eq!(format_tokens(500), "500");
        assert_eq!(format_tokens(1500), "1.5K");
        assert_eq!(format_tokens(1500000), "1.5M");
    }

    #[test]
    fn test_estimate_tokens() {
        assert!(estimate_tokens("hello world") > 0);
    }
}