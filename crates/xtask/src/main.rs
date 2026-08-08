//! 维护者工具箱（对应原版 pi 的 `scripts/`）。
//!
//! 这个 crate `publish = false`，不进发布链、下游不依赖。用法：
//!
//! ```sh
//! cargo run -p xtask -- generate-models           # 抓取并写盘
//! cargo run -p xtask -- generate-models --check   # 只校验产物是否最新（CI 用）
//! cargo run -p xtask -- <subcommand> --help
//! ```

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("generate-models") => {
            let check_only = args.iter().any(|a| a == "--check");
            let out = pi_ai_data_dir()?.join("models_generated.json");
            generate_models::run(&out, check_only)
        }
        Some("fetch-bun") => {
            let check_only = args.iter().any(|a| a == "--check");
            fetch_bun::run(check_only)
        }
        Some("build-sdk") => build_sdk::run(),
        Some(other) => anyhow::bail!("unknown subcommand: {other}"),
        None => {
            eprintln!("usage: cargo run -p xtask -- <subcommand>");
            eprintln!();
            eprintln!("subcommands:");
            eprintln!("  generate-models [--check]  fetch providers/models and write crates/pi-ai/data/models_generated.json");
            eprintln!("  fetch-bun [--check]        download Bun runtime binary to assets/runtime/");
            eprintln!("  build-sdk                  bundle real TS SDK pure functions to assets/sdk/ (needs sibling `pi` repo + bun)");
            Ok(())
        }
    }
}

/// 仓库内的 `crates/pi-ai/data` 目录（产物落盘处，提交进 git）。
fn pi_ai_data_dir() -> anyhow::Result<PathBuf> {
    // CARGO_MANIFEST_DIR 指向 crates/xtask
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let data_dir = manifest
        .parent() // crates/
        .ok_or_else(|| anyhow::anyhow!("cannot locate crates/ parent of xtask"))?
        .join("pi-ai")
        .join("data");
    std::fs::create_dir_all(&data_dir)?;
    Ok(data_dir)
}

mod generate_models;
mod fetch_bun;
mod build_sdk;
