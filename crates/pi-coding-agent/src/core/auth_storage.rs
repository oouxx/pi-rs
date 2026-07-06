use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::core::resolve_config_value;
use pi_ai::env_api_keys::get_env_api_key;
use pi_ai::env_api_keys::get_env_var_name;

// ---------------------------------------------------------------------------
// OAuth types – defined here until pi-ai exposes them upstream
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthCredentials {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires: u128,
    pub token_type: Option<String>,
    pub scope: Option<Vec<String>>,
}

pub trait OAuthLoginCallbacks: Send {
    fn on_url(
        &self,
        url: &str,
    ) -> Box<dyn std::future::Future<Output = Result<String, String>> + Send>;
}

pub fn find_env_keys(provider: &str) -> Vec<String> {
    if let Some(var_name) = get_env_var_name(provider) {
        if std::env::var(var_name).is_ok() {
            return vec![var_name.to_string()];
        }
    }
    Vec::new()
}

pub fn get_oauth_provider(_provider_id: &str) -> Option<OAuthProvider> {
    None
}

pub struct OAuthProvider;

impl OAuthProvider {
    pub fn get_api_key(&self, _creds: &OAuthCredentials) -> Option<String> {
        None
    }
}

// ---------------------------------------------------------------------------
// Auth credential types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AuthCredential {
    #[serde(rename = "api_key")]
    ApiKey { key: String },
    #[serde(rename = "oauth")]
    OAuth {
        #[serde(flatten)]
        credentials: OAuthCredentials,
    },
}

pub type AuthStorageData = HashMap<String, AuthCredential>;

#[derive(Debug, Clone)]
pub struct AuthStatus {
    pub configured: bool,
    pub source: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LockResult<T> {
    pub result: T,
    pub next: Option<String>,
}

// ---------------------------------------------------------------------------
// Backend abstraction
// ---------------------------------------------------------------------------

pub enum AuthStorageBackend {
    File(FileAuthStorageBackend),
    InMemory(InMemoryAuthStorageBackend),
}

impl AuthStorageBackend {
    fn with_lock<T>(&self, f: &mut dyn FnMut(Option<&str>) -> LockResult<T>) -> T {
        match self {
            AuthStorageBackend::File(b) => b.with_lock_impl(f),
            AuthStorageBackend::InMemory(b) => b.with_lock_impl(f),
        }
    }
}

pub struct FileAuthStorageBackend {
    auth_path: PathBuf,
    mutex: Mutex<()>,
}

impl FileAuthStorageBackend {
    pub fn new(auth_path: PathBuf) -> Self {
        Self {
            auth_path,
            mutex: Mutex::new(()),
        }
    }

    fn ensure_parent_dir(&self) {
        if let Some(parent) = self.auth_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
    }

    fn ensure_file_exists(&self) {
        if !self.auth_path.exists() {
            let _ = fs::write(&self.auth_path, "{}");
        }
    }

    fn read_content(&self) -> Option<String> {
        fs::read_to_string(&self.auth_path).ok()
    }

    fn write_content(&self, content: &str) {
        let _ = fs::write(&self.auth_path, content);
    }

    fn with_lock_impl<T>(&self, f: &mut dyn FnMut(Option<&str>) -> LockResult<T>) -> T {
        let _guard = self.mutex.lock().unwrap();
        self.ensure_parent_dir();
        self.ensure_file_exists();
        let current = self.read_content();
        let LockResult { result, next } = f(current.as_deref());
        if let Some(next_str) = next {
            self.write_content(&next_str);
        }
        result
    }
}

pub struct InMemoryAuthStorageBackend {
    value: Mutex<Option<String>>,
}

impl InMemoryAuthStorageBackend {
    pub fn new() -> Self {
        Self {
            value: Mutex::new(Some("{}".to_string())),
        }
    }

    fn with_lock_impl<T>(&self, f: &mut dyn FnMut(Option<&str>) -> LockResult<T>) -> T {
        let mut guard = self.value.lock().unwrap();
        let current: Option<&str> = match guard.as_ref() {
            Some(s) => Some(s.as_str()),
            None => None,
        };
        let LockResult { result, next } = f(current);
        if let Some(next_str) = next {
            *guard = Some(next_str);
        }
        result
    }
}

// ---------------------------------------------------------------------------
// AuthStorage
// ---------------------------------------------------------------------------

pub struct AuthStorage {
    data: AuthStorageData,
    runtime_overrides: HashMap<String, String>,
    fallback_resolver: Option<Box<dyn Fn(&str) -> Option<String> + Send>>,
    storage: AuthStorageBackend,
    load_error: Option<String>,
    errors: Vec<String>,
}

impl AuthStorage {
    pub fn new(storage: AuthStorageBackend) -> Self {
        let mut s = Self {
            data: AuthStorageData::new(),
            runtime_overrides: HashMap::new(),
            fallback_resolver: None,
            load_error: None,
            errors: Vec::new(),
            storage,
        };
        s.reload();
        s
    }

    pub fn create(auth_path: PathBuf) -> Self {
        Self::new(AuthStorageBackend::File(FileAuthStorageBackend::new(
            auth_path,
        )))
    }

    pub fn in_memory(data: AuthStorageData) -> Self {
        let backend = AuthStorageBackend::InMemory(InMemoryAuthStorageBackend::new());
        let mut s = Self::new(backend);
        s.data = data;
        s
    }

    pub fn set_runtime_api_key(&mut self, provider: &str, api_key: String) {
        self.runtime_overrides.insert(provider.to_string(), api_key);
    }

    pub fn remove_runtime_api_key(&mut self, provider: &str) {
        self.runtime_overrides.remove(provider);
    }

    pub fn set_fallback_resolver<F>(&mut self, resolver: F)
    where
        F: Fn(&str) -> Option<String> + 'static + Send,
    {
        self.fallback_resolver = Some(Box::new(resolver));
    }

    fn parse_storage_data(content: Option<&str>) -> AuthStorageData {
        match content {
            Some(s) if !s.is_empty() => serde_json::from_str(s).unwrap_or_default(),
            _ => AuthStorageData::new(),
        }
    }

    pub fn reload(&mut self) {
        self.data = AuthStorageData::new();
        self.storage.with_lock(&mut |current| {
            self.data = Self::parse_storage_data(current);
            self.load_error = None;
            LockResult {
                result: (),
                next: None,
            }
        });
    }

    fn persist_provider_change(&mut self, provider: &str, credential: Option<&AuthCredential>) {
        if self.load_error.is_some() {
            return;
        }
        let provider = provider.to_string();
        self.storage.with_lock(&mut |current| {
            let mut current_data = Self::parse_storage_data(current);
            match credential {
                Some(c) => {
                    current_data.insert(provider.clone(), c.clone());
                }
                None => {
                    current_data.remove(&provider);
                }
            }
            let next =
                serde_json::to_string_pretty(&current_data).unwrap_or_else(|_| "{}".to_string());
            LockResult {
                result: (),
                next: Some(next),
            }
        });
    }

    pub fn get(&self, provider: &str) -> Option<&AuthCredential> {
        self.data.get(provider)
    }

    pub fn set(&mut self, provider: &str, credential: AuthCredential) {
        self.data.insert(provider.to_string(), credential.clone());
        self.persist_provider_change(provider, Some(&credential));
    }

    pub fn remove(&mut self, provider: &str) {
        self.data.remove(provider);
        self.persist_provider_change(provider, None);
    }

    pub fn list(&self) -> Vec<String> {
        self.data.keys().cloned().collect()
    }

    pub fn has(&self, provider: &str) -> bool {
        self.data.contains_key(provider)
    }

    pub fn has_auth(&self, provider: &str) -> bool {
        if self.runtime_overrides.contains_key(provider) {
            return true;
        }
        if self.data.contains_key(provider) {
            return true;
        }
        if get_env_api_key(provider).is_some() {
            return true;
        }
        if let Some(ref resolver) = self.fallback_resolver {
            if resolver(provider).is_some() {
                return true;
            }
        }
        false
    }

    pub fn get_auth_status(&self, provider: &str) -> AuthStatus {
        if self.data.contains_key(provider) {
            return AuthStatus {
                configured: true,
                source: Some("stored".to_string()),
                label: None,
            };
        }

        if self.runtime_overrides.contains_key(provider) {
            return AuthStatus {
                configured: false,
                source: Some("runtime".to_string()),
                label: Some("--api-key".to_string()),
            };
        }

        let env_keys = find_env_keys(provider);
        if let Some(first_key) = env_keys.first() {
            return AuthStatus {
                configured: false,
                source: Some("environment".to_string()),
                label: Some(first_key.clone()),
            };
        }

        if let Some(ref resolver) = self.fallback_resolver {
            if resolver(provider).is_some() {
                return AuthStatus {
                    configured: false,
                    source: Some("fallback".to_string()),
                    label: Some("custom provider config".to_string()),
                };
            }
        }

        AuthStatus {
            configured: false,
            source: None,
            label: None,
        }
    }

    pub fn get_all(&self) -> AuthStorageData {
        self.data.clone()
    }

    pub fn drain_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.errors)
    }

    pub async fn login(
        &mut self,
        _provider_id: &str,
        _callbacks: &dyn OAuthLoginCallbacks,
    ) -> Result<(), String> {
        Err("OAuth login not yet implemented in Rust port".to_string())
    }

    pub fn logout(&mut self, provider: &str) {
        self.remove(provider);
    }

    pub async fn get_api_key(
        &mut self,
        provider_id: &str,
        include_fallback: bool,
    ) -> Option<String> {
        if let Some(key) = self.runtime_overrides.get(provider_id) {
            return Some(key.clone());
        }

        let cred = self.data.get(provider_id);

        if let Some(AuthCredential::ApiKey { key }) = cred {
            return resolve_config_value::resolve_config_value(key);
        }

        if let Some(AuthCredential::OAuth { credentials }) = cred {
            if let Some(provider) = get_oauth_provider(provider_id) {
                if let Some(api_key) = provider.get_api_key(credentials) {
                    return Some(api_key);
                }
            }
            return None;
        }

        if let Some(env_key) = get_env_api_key(provider_id) {
            return Some(env_key);
        }

        if include_fallback {
            if let Some(ref resolver) = self.fallback_resolver {
                return resolver(provider_id);
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_and_get_api_key() {
        let mut storage = AuthStorage::in_memory(AuthStorageData::new());
        storage.set(
            "anthropic",
            AuthCredential::ApiKey {
                key: "sk-test".into(),
            },
        );
        let cred = storage.get("anthropic");
        assert!(cred.is_some());
        assert!(matches!(cred, Some(AuthCredential::ApiKey { .. })));
    }

    #[test]
    fn test_remove() {
        let mut storage = AuthStorage::in_memory(AuthStorageData::new());
        storage.set(
            "openai",
            AuthCredential::ApiKey {
                key: "sk-test".into(),
            },
        );
        assert!(storage.has("openai"));
        storage.remove("openai");
        assert!(!storage.has("openai"));
    }

    #[test]
    fn test_list() {
        let mut storage = AuthStorage::in_memory(AuthStorageData::new());
        storage.set("a", AuthCredential::ApiKey { key: "k1".into() });
        storage.set("b", AuthCredential::ApiKey { key: "k2".into() });
        let providers = storage.list();
        assert_eq!(providers.len(), 2);
        assert!(providers.contains(&"a".to_string()));
        assert!(providers.contains(&"b".to_string()));
    }

    #[test]
    fn test_runtime_override() {
        let mut storage = AuthStorage::in_memory(AuthStorageData::new());
        storage.set_runtime_api_key("anthropic", "sk-override".into());
        assert!(storage.has_auth("anthropic"));
    }

    #[test]
    fn test_get_auth_status() {
        let mut storage = AuthStorage::in_memory(AuthStorageData::new());
        storage.set(
            "anthropic",
            AuthCredential::ApiKey {
                key: "sk-abc".into(),
            },
        );
        let status = storage.get_auth_status("anthropic");
        assert!(status.configured);
        assert_eq!(status.source.unwrap(), "stored");
    }

    #[test]
    fn test_has_auth_no_credential() {
        let storage = AuthStorage::in_memory(AuthStorageData::new());
        assert!(!storage.has_auth("nonexistent"));
    }

    #[test]
    fn test_drain_errors() {
        let mut storage = AuthStorage::in_memory(AuthStorageData::new());
        assert!(storage.drain_errors().is_empty());
    }
}
