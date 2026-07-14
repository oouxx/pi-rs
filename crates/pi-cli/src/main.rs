//! pi-cli — CLI binary entry point for the pi coding agent.
//!
//! Mirrors packages/coding-agent/src/cli.ts

use std::process;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = pi_cli::args::parse_args(&args);
    let exit_code = pi_cli::run::run(&parsed).await;
    process::exit(exit_code);
}
