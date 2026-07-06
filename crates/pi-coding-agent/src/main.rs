//! pi-coding-agent — CLI binary entry point.
//!
//! Mirrors packages/coding-agent/src/cli.ts

use std::process;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = pi_coding_agent::cli::args::parse_args(&args);
    let exit_code = pi_coding_agent::cli::run::run(&parsed).await;
    process::exit(exit_code);
}
