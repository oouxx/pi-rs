#![allow(
    clippy::module_name_repetitions,
    clippy::too_many_lines,
    clippy::large_futures,
)]
#![cfg_attr(test, allow(
    clippy::unwrap_used,
    clippy::expect_used,
))]

pub mod args;
pub mod file_processor;
pub mod initial_message;
pub mod list_models;
pub mod package_manager_cli;
pub mod run;
