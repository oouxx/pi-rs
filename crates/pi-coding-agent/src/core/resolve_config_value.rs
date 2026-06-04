use std::collections::HashMap;
use std::sync::Mutex;

use regex::Regex;

static COMMAND_RESULT_CACHE: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);

enum TemplatePart {
    Literal(String),
    Env(String),
}

enum ConfigValueReference {
    Command(String),
    Template(Vec<TemplatePart>),
}

fn parse_template(config: &str) -> Vec<TemplatePart> {
    let mut parts = Vec::new();
    let mut literal = String::new();
    let chars: Vec<char> = config.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() {
            let next = chars[i + 1];
            if next == '$' || next == '!' {
                literal.push(next);
                i += 2;
                continue;
            }

            // Flush current literal buffer
            if !literal.is_empty() {
                parts.push(TemplatePart::Literal(literal.clone()));
                literal.clear();
            }

            if next == '{' {
                let end = config[i + 2..].find('}').map(|p| p + i + 2);
                if let Some(end_pos) = end {
                    let name = &config[i + 2..end_pos];
                    if is_valid_env_name(name) {
                        parts.push(TemplatePart::Env(name.to_string()));
                    } else {
                        literal.push_str(&config[i..=end_pos]);
                    }
                    i = end_pos + 1;
                    continue;
                }
            }

            // Try env name prefix
            let rest = &config[i + 1..];
            let env_match: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let env_len = env_match.len();
            if env_len > 0 {
                parts.push(TemplatePart::Env(env_match));
                i += 1 + env_len;
                continue;
            }

            literal.push('$');
            i += 1;
        } else {
            literal.push(chars[i]);
            i += 1;
        }
    }

    if !literal.is_empty() {
        parts.push(TemplatePart::Literal(literal));
    }

    parts
}

fn is_valid_env_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    name.chars()
        .all(|c| c.is_alphanumeric() || c == '_')
        && name.chars().next().map_or(false, |c| c.is_alphabetic() || c == '_')
}

fn parse_config_value_reference(config: &str) -> ConfigValueReference {
    if config.starts_with('!') {
        return ConfigValueReference::Command(config.to_string());
    }
    ConfigValueReference::Template(parse_template(config))
}

fn resolve_env_value(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn get_template_env_var_names(parts: &[TemplatePart]) -> Vec<String> {
    let mut names = Vec::new();
    for part in parts {
        if let TemplatePart::Env(name) = part {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
    }
    names
}

fn resolve_template(parts: &[TemplatePart]) -> Option<String> {
    let mut resolved = String::new();
    for part in parts {
        match part {
            TemplatePart::Literal(s) => resolved.push_str(s),
            TemplatePart::Env(name) => {
                let val = resolve_env_value(name)?;
                resolved.push_str(&val);
            }
        }
    }
    Some(resolved)
}

pub fn get_config_value_env_var_name(config: &str) -> Option<String> {
    let reference = parse_config_value_reference(config);
    match reference {
        ConfigValueReference::Command(_) => None,
        ConfigValueReference::Template(ref parts) => {
            if parts.len() == 1 {
                if let TemplatePart::Env(name) = &parts[0] {
                    return Some(name.clone());
                }
            }
            None
        }
    }
}

pub fn get_config_value_env_var_names(config: &str) -> Vec<String> {
    let reference = parse_config_value_reference(config);
    match reference {
        ConfigValueReference::Command(_) => vec![],
        ConfigValueReference::Template(ref parts) => get_template_env_var_names(parts),
    }
}

pub fn get_missing_config_value_env_var_names(config: &str) -> Vec<String> {
    get_config_value_env_var_names(config)
        .into_iter()
        .filter(|name| resolve_env_value(name).is_none())
        .collect()
}

pub fn is_command_config_value(config: &str) -> bool {
    matches!(parse_config_value_reference(config), ConfigValueReference::Command(_))
}

pub fn is_config_value_configured(config: &str) -> bool {
    get_missing_config_value_env_var_names(config).is_empty()
}

pub fn is_legacy_env_var_name_config_value(config: &str) -> bool {
    let legacy_re = Regex::new(r"^[A-Z_][A-Z0-9_]*$").unwrap();
    legacy_re.is_match(config)
}

pub fn resolve_config_value(config: &str) -> Option<String> {
    let reference = parse_config_value_reference(config);
    match reference {
        ConfigValueReference::Command(cmd) => execute_command(&cmd),
        ConfigValueReference::Template(parts) => resolve_template(&parts),
    }
}

fn execute_command(command_config: &str) -> Option<String> {
    {
        let cache_guard = COMMAND_RESULT_CACHE.lock().unwrap();
        if let Some(ref cache) = *cache_guard {
            if let Some(cached) = cache.get(command_config) {
                return cached.clone();
            }
        }
    }

    let result = execute_command_uncached(&command_config[1..]);

    let mut cache_guard = COMMAND_RESULT_CACHE.lock().unwrap();
    let cache = cache_guard.get_or_insert_with(HashMap::new);
    cache.insert(command_config.to_string(), result.clone());
    result
}

fn execute_command_uncached(command: &str) -> Option<String> {
    let child = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    let output = child.wait_with_output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() { None } else { Some(stdout) }
}

pub fn resolve_config_value_uncached(config: &str) -> Option<String> {
    let reference = parse_config_value_reference(config);
    match reference {
        ConfigValueReference::Command(cmd) => {
            let command = &cmd[1..];
            execute_command_uncached(command)
        }
        ConfigValueReference::Template(parts) => resolve_template(&parts),
    }
}

pub fn resolve_config_value_or_throw(config: &str, description: &str) -> Result<String, String> {
    let value = resolve_config_value_uncached(config);
    if let Some(v) = value {
        return Ok(v);
    }

    let reference = parse_config_value_reference(config);
    match reference {
        ConfigValueReference::Command(cmd) => {
            Err(format!(
                "Failed to resolve {} from shell command: {}",
                description,
                &cmd[1..]
            ))
        }
        ConfigValueReference::Template(_) => {
            let missing = get_missing_config_value_env_var_names(config);
            if missing.len() == 1 {
                Err(format!(
                    "Failed to resolve {} from environment variable: {}",
                    description, missing[0]
                ))
            } else if missing.len() > 1 {
                Err(format!(
                    "Failed to resolve {} from environment variables: {}",
                    description,
                    missing.join(", ")
                ))
            } else {
                Err(format!("Failed to resolve {}", description))
            }
        }
    }
}

pub fn resolve_headers(
    headers: Option<&HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    let headers = headers?;
    let mut resolved = HashMap::new();
    for (key, value) in headers {
        if let Some(resolved_value) = resolve_config_value(value) {
            resolved.insert(key.clone(), resolved_value);
        }
    }
    if resolved.is_empty() { None } else { Some(resolved) }
}

pub fn resolve_headers_or_throw(
    headers: Option<&HashMap<String, String>>,
    description: &str,
) -> Result<Option<HashMap<String, String>>, String> {
    let headers = match headers {
        Some(h) => h,
        None => return Ok(None),
    };
    let mut resolved = HashMap::new();
    for (key, value) in headers {
        let resolved_value =
            resolve_config_value_or_throw(value, &format!("{} header \"{}\"", description, key))?;
        resolved.insert(key.clone(), resolved_value);
    }
    Ok(Some(resolved))
}

pub fn clear_config_value_cache() {
    let mut cache_guard = COMMAND_RESULT_CACHE.lock().unwrap();
    *cache_guard = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_env_var() {
        let result = parse_template("$HOME");
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], TemplatePart::Env(n) if n == "HOME"));
    }

    #[test]
    fn test_braced_env_var() {
        let result = parse_template("${PATH}");
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], TemplatePart::Env(n) if n == "PATH"));
    }

    #[test]
    fn test_literal_dollar_escape() {
        let result = parse_template("$$literal");
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], TemplatePart::Literal(s) if s == "$literal"));
    }

    #[test]
    fn test_mixed_literal_and_env() {
        let result = parse_template("hello_${USER}_world");
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_is_command() {
        assert!(is_command_config_value("!echo hello"));
        assert!(!is_command_config_value("$HOME"));
    }

    #[test]
    fn test_get_env_var_name() {
        assert_eq!(
            get_config_value_env_var_name("$API_KEY"),
            Some("API_KEY".into())
        );
        assert_eq!(
            get_config_value_env_var_name("${API_KEY}"),
            Some("API_KEY".into())
        );
        assert_eq!(get_config_value_env_var_name("prefix_${API_KEY}"), None);
        assert_eq!(get_config_value_env_var_name("!command"), None);
    }

    #[test]
    fn test_get_env_var_names() {
        let names = get_config_value_env_var_names("${KEY1}_${KEY2}");
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"KEY1".into()));
        assert!(names.contains(&"KEY2".into()));
    }

    #[test]
    fn test_is_legacy_env_var() {
        assert!(is_legacy_env_var_name_config_value("OPENAI_API_KEY"));
        assert!(!is_legacy_env_var_name_config_value("not_legacy"));
        assert!(!is_legacy_env_var_name_config_value(""));
    }

    #[test]
    fn test_resolve_env_var() {
        // Set a known env var for testing
        unsafe { std::env::set_var("_PI_TEST_VAR", "test_value"); }
        assert_eq!(resolve_config_value("$_PI_TEST_VAR"), Some("test_value".into()));
        assert_eq!(resolve_config_value("${_PI_TEST_VAR}"), Some("test_value".into()));
        assert_eq!(
            resolve_config_value("prefix_${_PI_TEST_VAR}_suffix"),
            Some("prefix_test_value_suffix".into())
        );
        unsafe { std::env::remove_var("_PI_TEST_VAR"); }
    }

    #[test]
    fn test_resolve_missing_env_var() {
        assert_eq!(resolve_config_value("$NONEXISTENT_VAR_12345"), None);
    }

    #[test]
    fn test_resolve_headers() {
        unsafe { std::env::set_var("_PI_TEST_KEY", "secret123"); }

        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), "Bearer $_PI_TEST_KEY".into());
        headers.insert("Static".into(), "value".into());

        let resolved = resolve_headers(Some(&headers)).unwrap();
        assert_eq!(resolved.get("Authorization").unwrap(), "Bearer secret123");
        assert_eq!(resolved.get("Static").unwrap(), "value");

        unsafe { std::env::remove_var("_PI_TEST_KEY"); }
    }

    #[test]
    fn test_resolve_config_value_or_throw_ok() {
        unsafe { std::env::set_var("_PI_TEST_OK", "hello"); }
        let result = resolve_config_value_or_throw("$_PI_TEST_OK", "test");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "hello");
        unsafe { std::env::remove_var("_PI_TEST_OK"); }
    }

    #[test]
    fn test_clear_cache() {
        clear_config_value_cache();
        let cache_guard = COMMAND_RESULT_CACHE.lock().unwrap();
        assert!(cache_guard.is_none());
    }
}
