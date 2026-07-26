//! pi-cli — CLI binary entry point for the pi coding agent.
//!
//! Mirrors packages/coding-agent/src/cli.ts

use std::path::Path;
use std::process;

/// Load a .env file if it exists. Returns (true, errors) where true means
/// at least one variable was loaded.
fn load_env_file(path: &Path) -> (bool, Vec<String>) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return (false, vec![]),
    };

    let mut loaded = false;
    let mut errors = vec![];

    for (lineno, line) in content.lines().enumerate() {
        let line = line.trim();
        // Skip blank lines and comments
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Split on first '='
        if let Some(eq_pos) = line.find('=') {
            let key = line[..eq_pos].trim().to_string();
            let value = line[eq_pos + 1..].trim().to_string();
            if key.is_empty() {
                errors.push(format!("line {}: empty key", lineno + 1));
                continue;
            }
            // Only set if not already set in environment (don't override explicit env vars)
            if std::env::var(&key).is_err() {
                std::env::set_var(&key, &value);
                loaded = true;
            }
        } else {
            errors.push(format!("line {}: no '=' found", lineno + 1));
        }
    }

    (loaded, errors)
}

#[tokio::main]
async fn main() {
    // Load .env files: first .atrading/.env relative to cwd, then .env
    if let Ok(cwd) = std::env::current_dir() {
        // Prefer .atrading/.env (for the trading-agent project layout)
        let atrading_env = cwd.join(".atrading").join(".env");
        load_env_file(&atrading_env);

        // Fallback to .env
        let dot_env = cwd.join(".env");
        load_env_file(&dot_env);
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = pi_cli::args::parse_args(&args);
    let exit_code = pi_cli::run::run(&parsed).await;
    process::exit(exit_code);
}
