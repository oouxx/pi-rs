//! `pi auth` subcommands — mirror of TS `cli/auth-command.ts`,
//! `cli/credential-print.ts` and `cli/auth-check.ts`.
//!
//! Commands:
//! - `pi auth check [--provider <p>] [--model <m>] [--json] [--credentials] [--no-refresh]`
//! - `pi auth print-api-key [--provider <p>] [--model <m>]`
//! - `pi auth print-bearer-token [--provider <p>] [--model <m>] [--min-expiry <duration>]`
//!
//! pi-rs has no OAuth refresh flow, so `print-bearer-token` reads stored OAuth
//! credentials without refreshing, and `--no-refresh` is accepted for
//! compatibility.

use pi_coding_agent::config::get_auth_path;
use pi_coding_agent::core::auth_storage::{AuthCredential, AuthStorage};
use pi_coding_agent::core::model_registry::ModelRegistry;
use pi_coding_agent::pi_agent_core::pi_ai::providers::register_builtins::register_built_in_api_providers;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthCommandKind {
    Check,
    ApiKey,
    BearerToken,
}

#[derive(Debug)]
pub struct AuthCommand {
    pub kind: AuthCommandKind,
    pub args: Vec<String>,
    pub json: bool,
    pub credentials: bool,
    pub no_refresh: bool,
    pub min_expiry_ms: Option<u64>,
}

pub struct AuthCommandError(pub String);

fn usage(kind: AuthCommandKind) -> String {
    match kind {
        AuthCommandKind::Check => {
            "pi-rs auth check --provider <provider> [--json] [--credentials] [--no-refresh]"
                .to_string()
        }
        AuthCommandKind::ApiKey => {
            "pi-rs auth print-api-key --provider <provider> [--model <model>]".to_string()
        }
        AuthCommandKind::BearerToken => {
            "pi-rs auth print-bearer-token --provider <provider> [--model <model>] [--min-expiry <duration>]"
                .to_string()
        }
    }
}

pub fn print_auth_command_help() {
    println!(
        "Usage:
  pi-rs auth print-api-key [--provider <provider>] [--model <model>]
  pi-rs auth print-bearer-token [--provider <provider>] [--model <model>] [--min-expiry <duration>]
  pi-rs auth check [--provider <provider>] [--model <model>] [--json] [--credentials] [--no-refresh]

Auth commands require at least one of --provider or --model. Checks refresh expired OAuth credentials by default; --no-refresh prevents this. --credentials emits the credential, or includes it in JSON output."
    );
}

pub fn is_auth_command_help(args: &[String]) -> bool {
    args.first().map(|s| s.as_str()) == Some("auth")
        && (args.get(1).is_none()
            || args.get(1).map(|s| s.as_str()) == Some("help")
            || args.iter().any(|a| a == "--help" || a == "-h"))
}

/// Parse `auth` subcommand arguments (TS `parseAuthCommand`). Returns
/// `Ok(None)` when `args[0]` is not "auth"; `Err` on parse errors.
pub fn parse_auth_command(args: &[String]) -> Result<Option<AuthCommand>, String> {
    if args.first().map(|s| s.as_str()) != Some("auth") {
        return Ok(None);
    }
    let kind = match args.get(1).map(|s| s.as_str()) {
        Some("check") => AuthCommandKind::Check,
        Some("print-api-key") => AuthCommandKind::ApiKey,
        Some("print-bearer-token") => AuthCommandKind::BearerToken,
        Some(other) => {
            return Err(format!(
                "Unknown auth command \"{other}\". Use \"pi-rs auth print-api-key\", \"pi-rs auth print-bearer-token\", or \"pi-rs auth check\"."
            ));
        }
        None => return Err("Unknown auth command \"\". Use \"pi-rs auth print-api-key\", \"pi-rs auth print-bearer-token\", or \"pi-rs auth check\".".to_string()),
    };

    let mut command_args: Vec<String> = Vec::new();
    let mut json = false;
    let mut credentials = false;
    let mut no_refresh = false;
    let mut min_expiry_ms: Option<u64> = None;
    let mut index = 2;
    while index < args.len() {
        let arg = args[index].as_str();
        match arg {
            "--min-expiry" => {
                if kind != AuthCommandKind::BearerToken {
                    return Err("--min-expiry is only supported by print-bearer-token".to_string());
                }
                let value = args.get(index + 1).cloned();
                index += 1;
                let Some(value) = value else {
                    return Err("--min-expiry must use a duration such as 30m or 1h".to_string());
                };
                let Some(cap) = parse_duration(&value) else {
                    return Err("--min-expiry must use a duration such as 30m or 1h".to_string());
                };
                min_expiry_ms = Some(cap);
            }
            "--json" | "--credentials" | "--no-refresh" => {
                if kind != AuthCommandKind::Check {
                    return Err(format!("{arg} is only supported by auth check"));
                }
                if arg == "--json" {
                    json = true;
                } else if arg == "--credentials" {
                    credentials = true;
                } else {
                    no_refresh = true;
                }
            }
            _ => command_args.push(arg.to_string()),
        }
        index += 1;
    }

    Ok(Some(AuthCommand {
        kind,
        args: command_args,
        json,
        credentials,
        no_refresh,
        min_expiry_ms,
    }))
}

/// Parse `--min-expiry` durations like `30m`, `1h`, `15000ms` (TS regex
/// `^(\d+)(ms|s|m|h)$`).
fn parse_duration(value: &str) -> Option<u64> {
    let lower = value.to_lowercase();
    let (amount_str, unit) = if let Some(s) = lower.strip_suffix("ms") {
        (s, 1u64)
    } else if let Some(s) = lower.strip_suffix('s') {
        (s, 1_000)
    } else if let Some(s) = lower.strip_suffix('m') {
        (s, 60_000)
    } else if let Some(s) = lower.strip_suffix('h') {
        (s, 3_600_000)
    } else {
        return None;
    };
    if amount_str.is_empty() || !amount_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let amount: u64 = amount_str.parse().ok()?;
    Some(amount * unit)
}

/// Validate auth command args (TS `validateAuthCommandArgs`): only
/// `--provider` and `--model` are accepted; at least one is required.
pub fn validate_auth_command_args(
    command_args: &[String],
    kind: AuthCommandKind,
) -> Result<(Option<String>, Option<String>), String> {
    let mut provider: Option<String> = None;
    let mut model: Option<String> = None;
    let mut index = 0;
    while index < command_args.len() {
        let arg = command_args[index].as_str();
        match arg {
            "--provider" => {
                index += 1;
                let Some(value) = command_args.get(index) else {
                    return Err(format!("Missing value for --provider in \"{}\".", name(kind)));
                };
                provider = Some(value.trim().to_string()).filter(|s| !s.is_empty());
            }
            "--model" => {
                index += 1;
                let Some(value) = command_args.get(index) else {
                    return Err(format!("Missing value for --model in \"{}\".", name(kind)));
                };
                model = Some(value.trim().to_string()).filter(|s| !s.is_empty());
            }
            other if other.starts_with("--") => {
                return Err(format!(
                    "Unknown option --{} for \"{}\".\nUse \"{}\".",
                    other.trim_start_matches('-'),
                    name(kind),
                    usage(kind)
                ));
            }
            other => {
                return Err(format!(
                    "Unexpected argument \"{other}\" for \"{}\". Auth commands only accept --provider and --model.\nUse \"{}\".",
                    name(kind),
                    usage(kind)
                ));
            }
        }
        index += 1;
    }
    if provider.is_none() && model.is_none() {
        let (subject, suffix) = if kind == AuthCommandKind::Check {
            ("Auth checks", "")
        } else {
            ("Credential printing", "s")
        };
        return Err(format!(
            "{subject} require{suffix} --provider <provider> or --model <model>"
        ));
    }
    Ok((provider, model))
}

fn name(kind: AuthCommandKind) -> &'static str {
    match kind {
        AuthCommandKind::Check => "auth check",
        AuthCommandKind::ApiKey => "auth print-api-key",
        AuthCommandKind::BearerToken => "auth print-bearer-token",
    }
}

/// Build a model registry without registering `--api-key` (TS
/// `ModelRuntime.create({ allowModelNetwork: false })`): built-in models +
/// models.json provider configs, with the auth.json credential resolver
/// wired in.
fn build_auth_registry() -> Result<ModelRegistry, String> {
    register_built_in_api_providers();
    let mut registry = ModelRegistry::new(ModelRegistry::builtin_models_list());
    {
        let auth_path = get_auth_path();
        registry.set_api_key_resolver(std::sync::Arc::new(move |provider| {
            let storage = AuthStorage::create(auth_path.clone());
            storage.get(provider).and_then(|c| match c {
                AuthCredential::ApiKey { key, .. } => key.clone(),
                _ => None,
            })
        }));
    }
    Ok(registry)
}

/// Resolve the credential for a provider (TS `getAuthCredential`): the API
/// key, or the `Authorization: Bearer ...` header value.
async fn credential_for_provider(
    registry: &ModelRegistry,
    provider: &str,
) -> Result<Option<String>, String> {
    // Direct API-key resolution (env → auth.json → registered → models.json).
    if let Some(key) = registry.get_api_key_for_provider(provider) {
        return Ok(Some(key));
    }
    // Header-based auth (models.json `authHeader` / registered headers).
    let models = registry.get_models_for_provider(provider);
    if let Some(model) = models.first() {
        if let Ok(result) = registry.get_api_key_and_headers(model).await {
            if result.ok && !result.api_key.is_empty() {
                return Ok(Some(result.api_key));
            }
            if let Some(headers) = result.headers {
                for (name, value) in &headers {
                    if name.eq_ignore_ascii_case("authorization") {
                        if let Some(token) = value
                            .strip_prefix("Bearer ")
                            .or_else(|| value.strip_prefix("bearer "))
                        {
                            return Ok(Some(token.to_string()));
                        }
                    }
                }
            }
        }
    }
    Ok(None)
}

/// Resolve which provider an auth command targets (TS `resolveCliModel`
/// flow): explicit `--provider`, else the provider of `--model`, else error.
fn resolve_provider(
    registry: &ModelRegistry,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<String, String> {
    if let Some(p) = provider {
        return Ok(p.to_string());
    }
    let m = model.ok_or_else(|| "Auth commands require --provider <provider> or --model <model>".to_string())?;
    let found = registry
        .get_models()
        .iter()
        .find(|m2| m2.id == m)
        .map(|m2| m2.provider.clone())
        .ok_or_else(|| format!("Model \"{m}\" not found. Use --list-models to see available models."))?;
    Ok(found)
}

/// Run an auth subcommand. Returns the process exit code (TS semantics:
/// check → 0 ready / 1 not_ready / 2 invalid; print commands → 0 / 1).
pub async fn run_auth_command(args: &[String]) -> i32 {
    if is_auth_command_help(args) {
        print_auth_command_help();
        return 0;
    }
    let command = match parse_auth_command(args) {
        Ok(Some(c)) => c,
        Ok(None) => return -1, // not an auth command
        Err(e) => {
            eprintln!("Error: {e}");
            return 1;
        }
    };

    let (provider, model) = match validate_auth_command_args(&command.args, command.kind) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {e}");
            return if command.kind == AuthCommandKind::Check { 2 } else { 1 };
        }
    };

    let registry = match build_auth_registry() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: Failed to initialize model registry: {e}");
            return if command.kind == AuthCommandKind::Check { 2 } else { 1 };
        }
    };

    match command.kind {
        AuthCommandKind::ApiKey | AuthCommandKind::BearerToken => {
            match print_credential(&registry, &provider, &model, command.kind, command.min_expiry_ms).await {
                Ok(value) => {
                    println!("{value}");
                    0
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    1
                }
            }
        }
        AuthCommandKind::Check => run_check(&registry, &provider, &model, &command).await,
    }
}

async fn print_credential(
    registry: &ModelRegistry,
    provider: &Option<String>,
    model: &Option<String>,
    kind: AuthCommandKind,
    min_expiry_ms: Option<u64>,
) -> Result<String, String> {
    let provider_id = resolve_provider(registry, provider.as_deref(), model.as_deref())?;
    match kind {
        AuthCommandKind::ApiKey => {
            let value = credential_for_provider(registry, &provider_id).await?;
            match value {
                Some(v) => Ok(v),
                None => {
                    let is_oauth = is_oauth_provider(&provider_id);
                    if is_oauth {
                        Err(format!(
                            "Provider \"{provider_id}\" is configured with OAuth, not an API key"
                        ))
                    } else {
                        Err(format!("No usable API key is configured for \"{provider_id}\""))
                    }
                }
            }
        }
        AuthCommandKind::BearerToken => {
            let auth_path = get_auth_path();
            let storage = AuthStorage::create(auth_path);
            match storage.get(&provider_id) {
                Some(AuthCredential::OAuth { credentials }) => {
                    // pi-rs has no OAuth refresh; honor --min-expiry as a
                    // remaining-validity check.
                    if let Some(min_expiry) = min_expiry_ms {
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u128;
                        if credentials.expires <= now_ms + min_expiry as u128 {
                            return Err(format!(
                                "OAuth token for \"{provider_id}\" expires sooner than the requested --min-expiry; refresh is not supported"
                            ));
                        }
                    }
                    Ok(credentials.access_token.clone())
                }
                _ => Err(format!(
                    "Provider \"{provider_id}\" is not configured with an OAuth bearer token"
                )),
            }
        }
        AuthCommandKind::Check => unreachable!(),
    }
}

fn is_oauth_provider(provider: &str) -> bool {
    let storage = AuthStorage::create(get_auth_path());
    matches!(storage.get(provider), Some(AuthCredential::OAuth { .. }))
}

/// `pi auth check` (TS `checkProviderAuth`): ready / not_ready / invalid.
async fn run_check(
    registry: &ModelRegistry,
    provider: &Option<String>,
    model: &Option<String>,
    command: &AuthCommand,
) -> i32 {
    struct CheckResult {
        status: &'static str,
        provider: String,
        reason: Option<&'static str>,
        auth_type: Option<&'static str>,
    }

    let provider_id = match resolve_provider(registry, provider.as_deref(), model.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {e}");
            return 2;
        }
    };

    // TS `modelRuntime.getProvider` — is the provider known at all?
    let known = registry.get_provider_config(&provider_id).is_some()
        || !registry.get_models_for_provider(&provider_id).is_empty();
    let result = if !known {
        CheckResult {
            status: "not_ready",
            provider: provider_id.clone(),
            reason: Some("provider_not_found"),
            auth_type: None,
        }
    } else if registry.get_api_key_for_provider(&provider_id).is_some() {
        CheckResult {
            status: "ready",
            provider: provider_id.clone(),
            reason: None,
            auth_type: Some("api_key"),
        }
    } else {
        CheckResult {
            status: "not_ready",
            provider: provider_id.clone(),
            reason: Some("credentials_not_configured"),
            auth_type: None,
        }
    };

    // --credentials: emit the credential when ready (TS
    // `getProviderCredential`).
    let mut credential: Option<String> = None;
    if command.credentials && result.status == "ready" {
        credential = credential_for_provider(registry, &provider_id)
            .await
            .ok()
            .flatten();
        if credential.is_none() {
            // fall through with not_ready below
        }
    }

    let status = if command.credentials && result.status == "ready" && credential.is_none() {
        "not_ready"
    } else {
        result.status
    };

    let output = if command.json {
        let mut obj = serde_json::Map::new();
        obj.insert("status".into(), serde_json::json!(status));
        obj.insert("provider".into(), serde_json::json!(result.provider));
        if let Some(reason) = result.reason {
            obj.insert("reason".into(), serde_json::json!(reason));
        }
        if let Some(at) = result.auth_type {
            obj.insert("authType".into(), serde_json::json!(at));
        }
        if let Some(c) = &credential {
            obj.insert("credentials".into(), serde_json::json!(c));
        }
        serde_json::to_string(&obj).unwrap_or_default()
    } else {
        credential.clone().unwrap_or_else(|| status.to_string())
    };
    println!("{output}");
    match status {
        "ready" => 0,
        "not_ready" => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_auth_kinds() {
        assert!(parse_auth_command(&v(&["run", "code"])).unwrap().is_none());
        let c = parse_auth_command(&v(&["auth", "check", "--provider", "ollama"])).unwrap().unwrap();
        assert_eq!(c.kind, AuthCommandKind::Check);
        assert_eq!(c.args, v(&["--provider", "ollama"]));
        let c = parse_auth_command(&v(&["auth", "print-api-key", "--provider", "x"])).unwrap().unwrap();
        assert_eq!(c.kind, AuthCommandKind::ApiKey);
        let c = parse_auth_command(&v(&["auth", "print-bearer-token", "--provider", "x", "--min-expiry", "30m"])).unwrap().unwrap();
        assert_eq!(c.kind, AuthCommandKind::BearerToken);
        assert_eq!(c.min_expiry_ms, Some(30 * 60_000));
    }

    #[test]
    fn parse_auth_command_flags() {
        let c = parse_auth_command(&v(&["auth", "check", "--provider", "x", "--json", "--credentials", "--no-refresh"]))
            .unwrap().unwrap();
        assert!(c.json && c.credentials && c.no_refresh);
        // Flags restricted to check.
        assert!(parse_auth_command(&v(&["auth", "print-api-key", "--json"])).is_err());
        // --min-expiry restricted to bearer-token.
        assert!(parse_auth_command(&v(&["auth", "check", "--min-expiry", "1h"])).is_err());
        // Bad duration.
        assert!(parse_auth_command(&v(&["auth", "print-bearer-token", "--min-expiry", "soon"])).is_err());
    }

    #[test]
    fn parse_duration_units() {
        assert_eq!(parse_duration("500ms"), Some(500));
        assert_eq!(parse_duration("10s"), Some(10_000));
        assert_eq!(parse_duration("30m"), Some(30 * 60_000));
        assert_eq!(parse_duration("1h"), Some(3_600_000));
        assert_eq!(parse_duration("1H"), Some(3_600_000));
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration(""), None);
    }

    #[test]
    fn validate_args_requires_provider_or_model() {
        assert!(validate_auth_command_args(&v(&["--provider", "ollama"]), AuthCommandKind::Check).is_ok());
        assert!(validate_auth_command_args(&v(&["--model", "gpt-5"]), AuthCommandKind::ApiKey).is_ok());
        assert!(validate_auth_command_args(&v(&[]), AuthCommandKind::Check).is_err());
        assert!(validate_auth_command_args(&v(&["--bogus"]), AuthCommandKind::Check).is_err());
    }
}
