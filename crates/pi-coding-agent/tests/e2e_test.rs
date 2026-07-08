//! End-to-end tests for pi-coding-agent.
//!
//! Run with: cargo test -p pi-coding-agent --test e2e_test -- --nocapture
//!
//! These tests require:
//! - `OPENROUTER_API_KEY` environment variable set (or other provider key)
//! - Network access
//! - `bun` on PATH (for extension support)
//!
//! Run with: OPENROUTER_API_KEY=sk-or-v1-... cargo test -p pi-coding-agent --test e2e_test -- --nocapture

use std::process::Command;

/// Path to the built binary, relative to workspace root.
fn pi_binary() -> String {
    let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("../../target/debug/pi-coding-agent");
    path.to_string_lossy().to_string()
}

/// Check if a provider API key is available.
fn has_api_key() -> bool {
    std::env::var("OPENROUTER_API_KEY").is_ok()
        || std::env::var("DEEPSEEK_API_KEY").is_ok()
        || std::env::var("ANTHROPIC_API_KEY").is_ok()
        || std::env::var("OPENAI_API_KEY").is_ok()
}

/// Check if bun is available.
fn has_bun() -> bool {
    Command::new("bun").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

// ============================================================================
// Basic CLI tests (no API key required)
// ============================================================================

#[test]
fn test_cli_version() {
    let output = Command::new(pi_binary())
        .arg("--version")
        .output()
        .expect("failed to run pi --version");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("v1."));
}

#[test]
fn test_cli_help() {
    let output = Command::new(pi_binary())
        .arg("--help")
        .output()
        .expect("failed to run pi --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("USAGE:"));
    assert!(stdout.contains("--model"));
}

#[test]
fn test_cli_list_models() {
    let output = Command::new(pi_binary())
        .arg("--list-models")
        .output()
        .expect("failed to run pi --list-models");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("deepseek"));
}

#[test]
fn test_cli_install_missing_source() {
    let output = Command::new(pi_binary())
        .arg("install")
        .output()
        .expect("failed to run pi install");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Usage:"));
}

#[test]
fn test_cli_list_packages() {
    let output = Command::new(pi_binary())
        .arg("list")
        .output()
        .expect("failed to run pi list");
    assert!(output.status.success());
}

// ============================================================================
// RPC mode tests (no API key required)
// ============================================================================

#[test]
fn test_rpc_get_state() {
    let mut child = Command::new(pi_binary())
        .arg("--mode")
        .arg("rpc")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to start pi in RPC mode");

    // Send get_state + shutdown
    let stdin = child.stdin.as_mut().unwrap();
    use std::io::Write;
    writeln!(stdin, r#"{{"type":"get_state"}}"#).unwrap();
    writeln!(stdin, r#"{{"type":"shutdown"}}"#).unwrap();
    let _ = stdin;

    let output = child.wait_with_output().expect("failed to read RPC output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("get_state"));
    assert!(stdout.contains("session_id"));
    assert!(stdout.contains("shutdown"));
}

// ============================================================================
// Real model tests (require API key)
// ============================================================================

#[test]
#[ignore = "Requires API key. Run with OPENROUTER_API_KEY=... cargo test -- --include-ignored"]
fn test_print_mode_with_real_model() {
    if !has_api_key() {
        eprintln!("Skipping: no API key configured");
        return;
    }

    let output = Command::new(pi_binary())
        .args(["-p", "say hello in one word"])
        .env("OPENROUTER_API_KEY", std::env::var("OPENROUTER_API_KEY").unwrap_or_default())
        .output()
        .expect("failed to run pi -p");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("CLI error: {stderr}");
        // In CI/keyless environment, skip instead of failing
        if stderr.contains("No models available") || stderr.contains("No API key") {
            eprintln!("Skipping: model not configured");
            return;
        }
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should have produced some response
    assert!(!stdout.trim().is_empty(), "Expected non-empty response, got: '{stdout}'");
    eprintln!("Response: {stdout}");
}

#[test]
#[ignore = "Requires API key and bun. Run with OPENROUTER_API_KEY=..."]
fn test_extension_loading_via_rpc() {
    if !has_bun() {
        eprintln!("Skipping: bun not available");
        return;
    }

    let mut child = Command::new(pi_binary())
        .arg("--mode")
        .arg("rpc")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to start pi in RPC mode");

    // Load extensions from the test extensions dir
    let stdin = child.stdin.as_mut().unwrap();
    use std::io::Write;
    writeln!(
        stdin,
        r#"{{"type":"load_extensions","params":{{"extensionPaths":["/tmp/test-extensions/test-hello/index.ts"]}}}}"#
    ).unwrap();
    writeln!(stdin, r#"{{"type":"get_available_models"}}"#).unwrap();
    writeln!(stdin, r#"{{"type":"shutdown"}}"#).unwrap();
    let _ = stdin;

    let output = child.wait_with_output().expect("failed to read RPC output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should show available models
    assert!(stdout.contains("deepseek") || stdout.contains("anthropic") || stdout.contains("openai"),
        "Expected model list, got: {stdout}");
}
