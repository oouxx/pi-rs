#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
#![allow(clippy::clone_on_copy)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::unreadable_literal)]
#![allow(clippy::wildcard_enum_match_arm)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::result_unit_err)]
#![allow(clippy::bind_instead_of_map)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::unnecessary_wraps)]
#![allow(clippy::disallowed_methods)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::items_after_statements)]
#![allow(clippy::similar_names)]
pub mod api_registry;
pub mod env_api_keys;
pub mod models;
pub mod providers;
pub mod stream;
pub mod types;
pub mod utils;
