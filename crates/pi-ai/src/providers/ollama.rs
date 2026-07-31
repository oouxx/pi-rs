//! Ollama 本地模型自动发现。
//!
//! Ollama 是一个本地 LLM 运行时，默认监听 `http://localhost:11434`，并暴露
//! OpenAI 兼容的 `/v1/chat/completions` 端点。模型列表（用户已 `ollama pull`
//! 下来的）通过 `/api/tags` 查询。本模块在运行时探测本机 Ollama，把已安装的
//! 模型注册为 `api = openai-completions`、`provider = "ollama"` 的 `Model`，
//! 让用户无需手写 `models.json` 即可直接选用本地模型。
//!
//! 设计要点：
//! - Ollama 可能未运行 → 任何错误（连接拒绝、超时、解析失败）一律返回空
//!   `Vec`，不报错、不阻塞上层启动。
//! - `/api/tags` 不返回上下文窗口；Ollama 实际支持长度因模型而异。这里用一
//!   个保守默认（8192），用户可在 `models.json` 里按模型覆盖。
//! - 走 `reqwest` 异步客户端，短超时（1.5s），避免本机不在线时拖慢启动。
//! - 端点可通过 `OLLAMA_BASE_URL` / `OLLAMA_HOST` 环境变量覆盖（Ollama 官方
//!   约定）。

use crate::types::Model;

/// Ollama `/api/tags` 响应里单个模型的字段（只取我们需要的）。
#[derive(serde::Deserialize)]
struct OllamaTagsModel {
    name: String,
}

#[derive(serde::Deserialize)]
struct OllamaTagsResponse {
    #[serde(default)]
    models: Vec<OllamaTagsModel>,
}

/// 解析 Ollama base URL：优先 `OLLAMA_BASE_URL`，其次 `OLLAMA_HOST`，最后默认。
fn ollama_base_url() -> String {
    if let Ok(v) = std::env::var("OLLAMA_BASE_URL") {
        if !v.trim().is_empty() {
            return trim_trailing_slash(&v);
        }
    }
    if let Ok(v) = std::env::var("OLLAMA_HOST") {
        if !v.trim().is_empty() {
            return trim_trailing_slash(&v);
        }
    }
    "http://localhost:11434".to_string()
}

fn trim_trailing_slash(s: &str) -> String {
    let t = s.trim_end_matches('/');
    // 若用户只给了 host:port（如 0.0.0.0:11434）补上 scheme
    if t.starts_with("http://") || t.starts_with("https://") {
        t.to_string()
    } else {
        format!("http://{t}")
    }
}

/// Ollama `/v1` 端点（OpenAI 兼容路径），作为模型的 `base_url`。
fn ollama_v1_base_url(base: &str) -> String {
    format!("{base}/v1")
}

/// 探测本机 Ollama，返回已安装模型对应的 `Model` 列表。
///
/// 任何失败（Ollama 未运行、超时、响应解析失败）都返回空 `Vec`，绝不向上
/// 传播错误——本机没有 Ollama 是合法的常见状态。
pub async fn discover_ollama_models() -> Vec<Model> {
    let base = ollama_base_url();
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(1500))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let resp = match client.get(format!("{base}/api/tags")).send().await {
        Ok(r) => r,
        Err(_) => return Vec::new(), // Ollama 未运行 / 不可达
    };
    if !resp.status().is_success() {
        return Vec::new();
    }
    let body: OllamaTagsResponse = match resp.json().await {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };

    let v1 = ollama_v1_base_url(&base);
    body.models
        .into_iter()
        .map(|m| ollama_model(&m.name, &v1))
        .collect()
}

/// 为单个 Ollama 模型构造 `Model`。
fn ollama_model(name: &str, v1_base_url: &str) -> Model {
    Model {
        id: name.to_string(),
        name: name.to_string(),
        api: "openai-completions".to_string(),
        provider: "ollama".to_string(),
        base_url: v1_base_url.to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".to_string()],
        cost: crate::types::ModelCost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
        context_window: 8192,
        max_tokens: 4096,
        headers: None,
        compat: None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn test_ollama_base_url_default() {
        // 确保未设环境变量时回退到默认本机端点。
        std::env::remove_var("OLLAMA_BASE_URL");
        std::env::remove_var("OLLAMA_HOST");
        assert_eq!(ollama_base_url(), "http://localhost:11434");
    }

    #[test]
    fn test_ollama_base_url_via_env() {
        std::env::set_var("OLLAMA_BASE_URL", "http://192.168.1.10:11434/");
        assert_eq!(ollama_base_url(), "http://192.168.1.10:11434");
        std::env::remove_var("OLLAMA_BASE_URL");
        std::env::set_var("OLLAMA_HOST", "0.0.0.0:11434");
        assert_eq!(ollama_base_url(), "http://0.0.0.0:11434");
        std::env::remove_var("OLLAMA_HOST");
    }

    #[test]
    fn test_ollama_model_shape() {
        let m = ollama_model("llama3.2", "http://localhost:11434/v1");
        assert_eq!(m.id, "llama3.2");
        assert_eq!(m.provider, "ollama");
        assert_eq!(m.api, "openai-completions");
        assert_eq!(m.base_url, "http://localhost:11434/v1");
        assert_eq!(m.cost.input, 0.0);
        assert!(!m.reasoning);
    }
}
