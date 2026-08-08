//! Remote model catalog refresh (bounded subset of TS `remote-catalog-provider.ts`).
//!
//! Fetches the latest per-provider model lists from a catalog service (default
//! `https://pi.dev/api/models/providers/{provider}`), merges them into the
//! `ModelRegistry` (replacing same-id models), and persists a
//! `models-store.json` cache so offline sessions reuse the last fetched
//! catalogs (match TS `models-store.json` entries).

use std::collections::HashMap;
use std::path::Path;

use pi_agent_core::pi_ai_types::Model;

use super::model_registry::ModelRegistry;

pub const DEFAULT_CATALOG_BASE_URL: &str = "https://pi.dev";
/// Freshness window for cached catalogs (match TS `REMOTE_CATALOG_REFRESH_INTERVAL_MS`).
pub const REMOTE_CATALOG_REFRESH_INTERVAL_MS: u64 = 4 * 60 * 60 * 1000;

/// Per-provider persisted catalog entry (match TS `ModelsStoreEntry`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsStoreEntry {
    pub models: Vec<Model>,
    #[serde(default)]
    pub checked_at: i64,
    #[serde(default)]
    pub last_modified: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

pub type ModelsStore = HashMap<String, ModelsStoreEntry>;

/// Result summary of a catalog refresh.
#[derive(Debug, Clone, Default)]
pub struct RefreshSummary {
    pub providers_checked: usize,
    pub providers_updated: usize,
    pub providers_failed: usize,
    pub models_added_or_updated: usize,
    pub errors: Vec<String>,
}

/// Load the persisted catalog cache. A missing or unparseable store is an
/// empty map (the store is a cache, not a source of truth).
pub fn load_models_store(path: &Path) -> ModelsStore {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return ModelsStore::new();
    };
    serde_json::from_str(&contents).unwrap_or_default()
}

/// Persist the catalog cache (best-effort; failures are recorded as errors).
pub fn save_models_store(path: &Path, store: &ModelsStore) -> Result<(), String> {
    let contents = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    std::fs::write(path, contents).map_err(|e| e.to_string())
}

/// Parse a catalog response body into models (match TS `parseCatalog`):
/// an array of models, `{ "models": [...] }`, or an object of models.
pub fn parse_catalog(provider_id: &str, value: &serde_json::Value) -> Result<Vec<Model>, String> {
    let entries: Vec<&serde_json::Value> = match value {
        serde_json::Value::Array(items) => items.iter().collect(),
        serde_json::Value::Object(map) => {
            if let Some(models) = map.get("models").and_then(serde_json::Value::as_array) {
                models.iter().collect()
            } else {
                map.values().collect()
            }
        }
        _ => {
            return Err(format!("Invalid model catalog for provider \"{provider_id}\""));
        }
    };
    let mut models = Vec::new();
    for entry in entries {
        let Some(id) = entry.get("id").and_then(serde_json::Value::as_str) else {
            continue;
        };
        // The catalog body may omit the provider; normalize it to ours
        // (match TS `{...model, provider}`).
        let mut model_value = entry.clone();
        if let Some(obj) = model_value.as_object_mut() {
            obj.insert("provider".to_string(), serde_json::Value::String(provider_id.to_string()));
        }
        let model: Model = match serde_json::from_value(model_value) {
            Ok(m) => m,
            Err(e) => {
                return Err(format!(
                    "Invalid model entry for provider \"{provider_id}\" (id {id:?}): {e}"
                ));
            }
        };
        models.push(model);
    }
    Ok(models)
}

/// Whether a cached entry is still fresh within the refresh interval
/// (match TS `REMOTE_CATALOG_REFRESH_INTERVAL_MS` check).
pub fn is_fresh(entry: Option<&ModelsStoreEntry>, now_ms: i64) -> bool {
    match entry {
        Some(e) => e.checked_at > 0 && now_ms.saturating_sub(e.checked_at) < REMOTE_CATALOG_REFRESH_INTERVAL_MS as i64,
        None => false,
    }
}

fn catalog_url(catalog_base_url: &str, provider_id: &str) -> String {
    format!(
        "{}/api/models/providers/{}",
        catalog_base_url.trim_end_matches('/'),
        urlencoding_encode(provider_id)
    )
}

/// Percent-encode a provider id for a URL path segment.
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Refresh the remote catalog for all providers in the registry.
///
/// - `store_path`: path to `models-store.json` (cache).
/// - `force`: bypass the freshness window and revalidate/download.
/// - When a provider is fresh and `force` is false, its cached models are
///   restored but no network request is made.
/// - Network failures keep the cached body (and its etag) so the next refresh
///   revalidates instead of clearing the overlay (match TS).
pub async fn refresh_remote_catalog(
    registry: &ModelRegistry,
    catalog_base_url: &str,
    store_path: &Path,
    force: bool,
) -> RefreshSummary {
    let mut summary = RefreshSummary::default();
    let mut store = load_models_store(store_path);
    let providers = registry.get_providers();
    let now = chrono::Utc::now().timestamp_millis();
    let client = reqwest::Client::new();

    for provider_id in &providers {
        summary.providers_checked += 1;
        let entry = store.get(provider_id).cloned();
        if !force && is_fresh(entry.as_ref(), now) {
            // Restore cached models without a network round-trip.
            if let Some(e) = entry {
                if !e.models.is_empty() {
                    registry.upsert_models(provider_id, &e.models);
                }
            }
            continue;
        }

        let url = catalog_url(catalog_base_url, provider_id);
        let mut req = client.get(&url).header("accept", "application/json");
        if let Some(etag) = entry.as_ref().and_then(|e| e.etag.as_deref()) {
            if !etag.is_empty() {
                req = req.header("if-none-match", etag);
            }
        }
        let response = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                summary.providers_failed += 1;
                summary.errors.push(format!("{provider_id}: {e}"));
                continue;
            }
        };
        let checked_at = chrono::Utc::now().timestamp_millis();

        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            if let Some(mut e) = entry {
                e.checked_at = checked_at;
                if !e.models.is_empty() {
                    registry.upsert_models(provider_id, &e.models);
                }
                store.insert(provider_id.clone(), e);
            }
            continue;
        }
        if response.status() == reqwest::StatusCode::NOT_FOUND
            || response.status() == reqwest::StatusCode::NOT_IMPLEMENTED
        {
            store.insert(
                provider_id.clone(),
                ModelsStoreEntry {
                    models: entry.as_ref().map(|e| e.models.clone()).unwrap_or_default(),
                    checked_at,
                    last_modified: 0,
                    etag: None,
                },
            );
            continue;
        }
        if !response.status().is_success() {
            summary.providers_failed += 1;
            summary.errors.push(format!(
                "Model catalog request failed for {provider_id}: {}",
                response.status()
            ));
            // Keep the cached body and validator for a later revalidation.
            if let Some(mut e) = entry {
                e.checked_at = checked_at;
                store.insert(provider_id.clone(), e);
            }
            continue;
        }

        // Capture headers before consuming the body.
        let last_modified = response
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .and_then(httpdate_parse)
            .unwrap_or(0);
        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body: serde_json::Value = match response.json().await {
            Ok(v) => v,
            Err(e) => {
                summary.providers_failed += 1;
                summary.errors.push(format!("{provider_id}: failed to parse catalog body: {e}"));
                continue;
            }
        };

        let refreshed = match parse_catalog(provider_id, &body) {
            Ok(m) => m,
            Err(e) => {
                summary.providers_failed += 1;
                summary.errors.push(format!("{provider_id}: {e}"));
                continue;
            }
        };
        let changed = registry.upsert_models(provider_id, &refreshed);
        summary.providers_updated += 1;
        summary.models_added_or_updated += changed;
        store.insert(
            provider_id.clone(),
            ModelsStoreEntry {
                models: refreshed,
                checked_at,
                last_modified,
                etag,
            },
        );
    }

    if let Err(e) = save_models_store(store_path, &store) {
        summary.errors.push(format!("failed to persist models store: {e}"));
    }
    summary
}

/// Parse an RFC 7231 `Last-Modified` header to epoch millis (best-effort).
fn httpdate_parse(s: &str) -> Option<i64> {
    use chrono::NaiveDate;
    // HTTP dates: "Sun, 06 Nov 1994 08:49:37 GMT"
    let parsed = NaiveDate::parse_from_str(s, "%a, %d %b %Y %H:%M:%S GMT").ok()?;
    Some(parsed.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn make_model(id: &str) -> Model {
        Model {
            id: id.to_string(),
            name: id.to_string(),
            api: "openai-completions".into(),
            provider: "test-provider".into(),
            base_url: "https://example.test/v1".into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec!["text".into()],
            cost: pi_agent_core::pi_ai_types::ModelCost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
                tiers: vec![],
            },
            context_window: 1000,
            max_tokens: 100,
            headers: None,
            compat: None,
        }
    }

    fn complete_model(id: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "name": id.to_uppercase(),
            "api": "openai-completions",
            "baseUrl": "https://example.test/v1",
            "reasoning": false,
            "input": ["text"],
            "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
            "contextWindow": 1000,
            "maxTokens": 100,
        })
    }

    #[test]
    fn test_parse_catalog_array() {
        let body = serde_json::json!([complete_model("m1")]);
        let models = parse_catalog("test-provider", &body).unwrap();
        assert_eq!(models.len(), 1);
        // Provider is normalized to ours.
        assert_eq!(models[0].provider, "test-provider");
    }

    #[test]
    fn test_parse_catalog_models_key() {
        let body = serde_json::json!({ "models": [complete_model("m1")] });
        let models = parse_catalog("test-provider", &body).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "m1");
    }

    #[test]
    fn test_parse_catalog_keyed_object() {
        // TS fixture: {"dynamic": { ...model } } — an object of models.
        let body = serde_json::json!({ "dynamic": complete_model("m1") });
        let models = parse_catalog("test-provider", &body).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "m1");
    }

    #[test]
    fn test_parse_catalog_invalid() {
        assert!(parse_catalog("p", &serde_json::json!("nope")).is_err());
    }

    #[test]
    fn test_store_roundtrip() {
        let dir = std::env::temp_dir().join(format!("pi-store-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("models-store.json");
        let mut store = ModelsStore::new();
        store.insert(
            "test-provider".into(),
            ModelsStoreEntry {
                models: vec![make_model("m1")],
                checked_at: 123,
                last_modified: 456,
                etag: Some("\"abc\"".into()),
            },
        );
        save_models_store(&path, &store).unwrap();
        let loaded = load_models_store(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded["test-provider"].models[0].id, "m1");
        assert_eq!(loaded["test-provider"].etag.as_deref(), Some("\"abc\""));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_is_fresh() {
        let now = chrono::Utc::now().timestamp_millis();
        let fresh = ModelsStoreEntry {
            models: vec![],
            checked_at: now - 60_000,
            last_modified: 0,
            etag: None,
        };
        assert!(is_fresh(Some(&fresh), now));
        let stale = ModelsStoreEntry {
            checked_at: now - REMOTE_CATALOG_REFRESH_INTERVAL_MS as i64 - 1000,
            ..fresh
        };
        assert!(!is_fresh(Some(&stale), now));
        assert!(!is_fresh(None, now));
    }

    #[test]
    fn test_urlencoding() {
        assert_eq!(urlencoding_encode("anthropic"), "anthropic");
        assert_eq!(urlencoding_encode("a b"), "a%20b");
    }

    #[tokio::test]
    async fn test_refresh_end_to_end() {
        // Mock catalog server: serves one complete model per provider.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
                let body = format!(
                    "{{\"models\": [{}]}}",
                    serde_json::json!({
                        "id": "m1",
                        "name": "M1",
                        "api": "openai-completions",
                        "baseUrl": "https://example.test/v1",
                        "reasoning": false,
                        "input": ["text"],
                        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
                        "contextWindow": 1000,
                        "maxTokens": 100,
                    })
                );
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, resp.as_bytes()).await;
            }
        });

        // Use an empty models.json so the test is hermetic (no user config).
        let config_dir = std::env::temp_dir().join(format!("pi-refresh-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&config_dir).unwrap();
        let empty_models_path = config_dir.join("models.json");
        std::fs::write(&empty_models_path, "{}").unwrap();
        let registry = ModelRegistry::new_with_models_path(vec![make_model("static")], &empty_models_path);
        let dir = std::env::temp_dir().join(format!("pi-refresh-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store_path = dir.join("models-store.json");
        let base = format!("http://{addr}");

        let summary = refresh_remote_catalog(&registry, &base, &store_path, true).await;
        assert!(summary.errors.is_empty(), "errors: {:?}", summary.errors);
        assert_eq!(summary.providers_updated, 1);

        // Remote model merged alongside the builtin model.
        let ids: Vec<String> = registry.get_models().iter().map(|m| m.id.clone()).collect();
        assert!(ids.contains(&"m1".to_string()));
        assert!(ids.contains(&"static".to_string()));

        // Cache persisted; a second (non-force) refresh restores from cache.
        let store = load_models_store(&store_path);
        assert!(store.contains_key("test-provider"));
        let summary2 = refresh_remote_catalog(&registry, &base, &store_path, false).await;
        assert_eq!(summary2.providers_checked, 1);
        assert_eq!(summary2.providers_updated, 0, "fresh cache should skip network");

        std::fs::remove_file(&store_path).ok();
    }
}
