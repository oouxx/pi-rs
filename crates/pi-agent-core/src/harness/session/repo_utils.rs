#![allow(
    clippy::module_name_repetitions,
    clippy::struct_field_names,
    clippy::too_many_lines,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::unnecessary_struct_initialization,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_closure,
    clippy::missing_const_for_fn,
    clippy::map_unwrap_or,
    clippy::option_if_let_else,
    clippy::manual_let_else,
    clippy::match_wildcard_for_single_variants,
    clippy::ref_option,
    clippy::redundant_clone,
    clippy::unnecessary_operation,
    clippy::unused_self,
    clippy::match_same_arms,
    clippy::bool_to_int_with_if,
    clippy::needless_continue,
    clippy::items_after_statements,
    clippy::unnecessary_to_owned,
    clippy::needless_pass_by_value,
    clippy::uninlined_format_args,
    clippy::derive_partial_eq_without_eq,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::string_lit_as_bytes,
    clippy::trivially_copy_pass_by_ref,
    clippy::single_char_pattern,
    clippy::format_push_string,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::needless_raw_string_hashes,
    clippy::unnecessary_fold,
    clippy::needless_pass_by_ref_mut,
    clippy::map_identity,
    clippy::needless_return_with_question_mark,
    clippy::needless_lifetimes,
    clippy::similar_names,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::large_enum_variant,
    clippy::enum_glob_use,
    clippy::future_not_send,
    clippy::should_implement_trait,
    clippy::new_without_default,
    clippy::return_self_not_must_use,
    clippy::use_self,












    clippy::significant_drop_tightening,

    clippy::default_trait_access,

    clippy::iter_with_drain,

    clippy::if_not_else,

    clippy::explicit_iter_loop,

    clippy::assigning_clones,

    clippy::implicit_hasher,

    clippy::ignored_unit_patterns,

    clippy::missing_fields_in_debug,

    clippy::or_fun_call,

    clippy::too_long_first_doc_paragraph,

    clippy::manual_string_new,

    clippy::single_match_else,

    clippy::significant_drop_in_scrutinee,

    clippy::needless_collect,

    clippy::duplicated_attributes,

)]
use crate::harness::types::{Session, SessionError, SessionStorage, SessionTreeEntry};

pub fn create_session_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

pub fn create_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn to_session<M: Clone + Send + Sync + 'static>(
    storage: Box<dyn SessionStorage<M>>,
) -> Session<M> {
    Session::new(storage)
}

pub async fn get_entries_to_fork<S: SessionStorage + ?Sized>(
    storage: &S,
    options: &crate::harness::types::ForkOptions,
) -> std::result::Result<Vec<SessionTreeEntry>, SessionError> {
    if options.entry_id.is_none() {
        return Ok(storage.get_entries(None).await);
    }

    let entry_id = options.entry_id.as_ref().unwrap();
    let target = storage
        .get_entry(entry_id)
        .await
        .ok_or_else(|| SessionError::NotFound(format!("Entry {entry_id} not found")))?;

    let effective_leaf_id = if options.position.as_deref() == Some("at") {
        Some(target.id().to_string())
    } else {
        match &target {
            SessionTreeEntry::Message { message, .. } => match message {
                crate::types::AgentMessage::User { .. } => {
                    target.parent_id().map(|s| s.to_string())
                }
                _ => {
                    return Err(SessionError::InvalidForkTarget(format!(
                        "Entry {} is not a user message",
                        entry_id
                    )));
                }
            },
            _ => {
                return Err(SessionError::InvalidForkTarget(format!(
                    "Entry {} is not a message",
                    entry_id
                )));
            }
        }
    };

    storage
        .get_path_to_root_or_compaction(effective_leaf_id.as_deref())
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_session_id_format() {
        let id = create_session_id();
        let parts: Vec<&str> = id.split('-').collect();
        assert!(
            parts.len() >= 4,
            "UUID v7 should have at least 4 parts, got: {}",
            id
        );
        assert!(!id.is_empty());
    }

    #[test]
    fn test_create_session_id_unique() {
        let id1 = create_session_id();
        let id2 = create_session_id();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_create_timestamp_format() {
        let ts = create_timestamp();
        assert!(ts.contains('T'));
        assert!(ts.contains('Z') || ts.contains('+'));
    }

    #[test]
    fn test_create_timestamp_not_empty() {
        let ts = create_timestamp();
        assert!(!ts.is_empty());
    }
}
