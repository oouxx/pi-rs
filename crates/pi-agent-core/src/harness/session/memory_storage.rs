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
use async_trait::async_trait;
use std::collections::HashMap;

use crate::harness::types::{
    SessionCreateOptions, SessionEntryCursorOptions, SessionError, SessionMetadata, SessionRepo,
    SessionStats, SessionStorage, SessionTreeEntry,
};
use crate::pi_ai_types::Usage;
use crate::types::AgentMessage;

pub struct InMemorySessionStorage {
    metadata: SessionMetadata,
    entries: Vec<SessionTreeEntry>,
    by_id: HashMap<String, usize>,
    labels_by_id: HashMap<String, String>,
    leaf_id: Option<String>,
}

impl InMemorySessionStorage {
    pub fn new(options: Option<InMemorySessionStorageOptions>) -> Self {
        let opts = options.unwrap_or_default();
        let mut storage = Self {
            metadata: opts.metadata.unwrap_or(SessionMetadata {
                id: uuid::Uuid::new_v4().to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
                cwd: None,
                parent_session: None,
            }),
            entries: Vec::new(),
            by_id: HashMap::new(),
            labels_by_id: HashMap::new(),
            leaf_id: None,
        };

        for entry in opts.entries {
            let id = entry.id().to_string();
            storage.leaf_id = Some(id.clone());
            storage.by_id.insert(id, storage.entries.len());
            update_label_cache(&mut storage.labels_by_id, &entry);
            storage.entries.push(entry);
        }

        storage
    }

    fn leaf_id_after_entry(&self, entry: &SessionTreeEntry) -> Option<String> {
        match entry {
            SessionTreeEntry::Leaf { target_id, .. } => target_id.clone(),
            _ => Some(entry.id().to_string()),
        }
    }
}

pub struct InMemorySessionStorageOptions {
    pub entries: Vec<SessionTreeEntry>,
    pub metadata: Option<SessionMetadata>,
}

#[allow(clippy::derivable_impls)]
impl Default for InMemorySessionStorageOptions {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            metadata: None,
        }
    }
}

fn update_label_cache(labels_by_id: &mut HashMap<String, String>, entry: &SessionTreeEntry) {
    if let SessionTreeEntry::Label {
        target_id, label, ..
    } = entry
    {
        if let Some(l) = label {
            if l.trim().is_empty() {
                labels_by_id.remove(target_id);
            } else {
                labels_by_id.insert(target_id.clone(), l.clone());
            }
        } else {
            labels_by_id.remove(target_id);
        }
    }
}

fn generate_entry_id(by_id: &HashMap<String, usize>) -> String {
    for _ in 0..100 {
        // The uuidv7 prefix is timestamp-derived and nearly constant between calls,
        // so short ids must come from the random tail (match TS `generateEntryId`).
        let id = crate::pi_ai_types::uuid_v7();
        let id = &id[id.len().saturating_sub(8)..];
        if !by_id.contains_key(id) {
            return id.to_string();
        }
    }
    crate::pi_ai_types::uuid_v7()
}

#[async_trait]
impl SessionStorage for InMemorySessionStorage {
    async fn get_metadata(&self) -> SessionMetadata {
        self.metadata.clone()
    }

    async fn get_leaf_id(&self) -> Option<String> {
        self.leaf_id.clone()
    }

    async fn set_leaf_id(
        &mut self,
        leaf_id: Option<String>,
    ) -> std::result::Result<(), SessionError> {
        if let Some(ref id) = leaf_id {
            if !self.by_id.contains_key(id) {
                return Err(SessionError::NotFound(format!("Entry {id} not found")));
            }
        }
        let entry = SessionTreeEntry::Leaf {
            id: generate_entry_id(&self.by_id),
            parent_id: self.leaf_id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            target_id: leaf_id.clone(),
        };
        let new_leaf = self.leaf_id_after_entry(&entry);
        let id = entry.id().to_string();
        self.by_id.insert(id, self.entries.len());
        self.entries.push(entry);
        self.leaf_id = new_leaf;
        Ok(())
    }

    async fn create_entry_id(&self) -> String {
        generate_entry_id(&self.by_id)
    }

    async fn append_entry(
        &mut self,
        entry: SessionTreeEntry,
    ) -> std::result::Result<(), SessionError> {
        let id = entry.id().to_string();
        let new_leaf = self.leaf_id_after_entry(&entry);
        self.by_id.insert(id, self.entries.len());
        update_label_cache(&mut self.labels_by_id, &entry);
        self.entries.push(entry);
        self.leaf_id = new_leaf;
        Ok(())
    }

    async fn get_entry(&self, id: &str) -> Option<SessionTreeEntry> {
        self.by_id.get(id).map(|&idx| self.entries[idx].clone())
    }

    async fn find_entries(&self, entry_type: &str) -> Vec<SessionTreeEntry> {
        self.entries
            .iter()
            .filter(|e| e.entry_type() == entry_type)
            .cloned()
            .collect()
    }

    async fn get_label(&self, id: &str) -> Option<String> {
        self.labels_by_id.get(id).cloned()
    }

    async fn get_session_name(&self) -> Option<String> {
        let entries = self.find_entries("session_info").await;
        entries
            .last()
            .and_then(|e| match e {
                SessionTreeEntry::SessionInfo { name, .. } => Some(name.trim().to_string()),
                _ => None,
            })
            .filter(|n| !n.is_empty())
    }

    async fn get_session_stats(&self) -> SessionStats {
        let mut stats = SessionStats::default();
        for entry in &self.entries {
            if matches!(entry, SessionTreeEntry::Message { .. }) {
                stats.message_count += 1;
            }
            let usage = match entry {
                SessionTreeEntry::Message {
                    message: AgentMessage::Assistant { usage, .. },
                    ..
                } => Some(usage.clone()),
                SessionTreeEntry::Compaction { usage, .. }
                | SessionTreeEntry::BranchSummary { usage, .. } => usage
                    .as_ref()
                    .and_then(|u| serde_json::from_value::<Usage>(u.clone()).ok()),
                _ => None,
            };
            let Some(usage) = usage else { continue };
            stats.cached_tokens += usage.cache_read;
            stats.uncached_tokens += usage.input + usage.cache_write;
            stats.total_tokens += usage.input + usage.output + usage.cache_read + usage.cache_write;
            stats.cost_total += usage.cost.total;
        }
        stats
    }

    async fn get_path_to_root_or_compaction(
        &self,
        leaf_id: Option<&str>,
    ) -> std::result::Result<Vec<SessionTreeEntry>, SessionError> {
        let leaf_id = match leaf_id {
            Some(id) => id.to_string(),
            None => match &self.leaf_id {
                Some(id) => id.clone(),
                None => return Ok(Vec::new()),
            },
        };

        let mut path = Vec::new();
        let mut stop_at_entry_id: Option<String> = None;
        let mut current_id = leaf_id;

        loop {
            let idx = match self.by_id.get(&current_id) {
                Some(&idx) => idx,
                None => {
                    return Err(SessionError::NotFound(format!(
                        "Entry {} not found",
                        current_id
                    )))
                }
            };
            let entry = &self.entries[idx];
            let parent_id = entry.parent_id().map(|s| s.to_string());
            path.push(entry.clone());

            if let Some(stop_id) = &stop_at_entry_id {
                if entry.id() == stop_id {
                    break;
                }
            }
            if let SessionTreeEntry::Compaction {
                retained_tail,
                first_kept_entry_id,
                ..
            } = entry
            {
                if retained_tail.is_some() {
                    // Self-contained checkpoint: stop here.
                    break;
                }
                stop_at_entry_id = first_kept_entry_id.clone();
            }

            match parent_id {
                Some(pid) => current_id = pid,
                None => break,
            }
        }

        path.reverse();
        Ok(path)
    }

    async fn get_entries(
        &self,
        options: Option<&SessionEntryCursorOptions>,
    ) -> Vec<SessionTreeEntry> {
        let start = options.map(|o| o.after_entry_seq as usize).unwrap_or(0);
        let end = match options.and_then(|o| o.limit) {
            Some(limit) => start.saturating_add(limit as usize),
            None => self.entries.len(),
        };
        self.entries
            .get(start..end)
            .map(|s| s.to_vec())
            .unwrap_or_default()
    }
}

pub struct InMemorySessionRepo {
    sessions: HashMap<String, crate::harness::types::Session>,
}

impl InMemorySessionRepo {
    pub fn new() -> Self {
        Self::default()
    }
}

#[allow(clippy::derivable_impls)]
impl Default for InMemorySessionRepo {
    fn default() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }
}

#[async_trait]
impl SessionRepo for InMemorySessionRepo {
    async fn create(
        &mut self,
        options: SessionCreateOptions,
    ) -> std::result::Result<crate::harness::types::Session, SessionError> {
        let id = options
            .id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let metadata = SessionMetadata {
            id: id.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
            cwd: Some(options.cwd),
            parent_session: options.parent_session_path,
        };
        let storage = Box::new(InMemorySessionStorage::new(Some(
            InMemorySessionStorageOptions {
                entries: Vec::new(),
                metadata: Some(metadata),
            },
        )));
        let session = crate::harness::types::Session::new(storage);
        let session = self.sessions.entry(id.clone()).or_insert(session);
        Ok(session.clone())
    }

    async fn open(
        &self,
        metadata: &SessionMetadata,
    ) -> std::result::Result<crate::harness::types::Session, SessionError> {
        self.sessions
            .get(&metadata.id)
            .cloned()
            .ok_or_else(|| SessionError::NotFound(format!("Session not found: {}", metadata.id)))
    }

    async fn list(&self) -> std::result::Result<Vec<SessionMetadata>, SessionError> {
        let mut metadata_list = Vec::new();
        for session in self.sessions.values() {
            metadata_list.push(session.get_metadata().await);
        }
        Ok(metadata_list)
    }

    async fn delete(
        &mut self,
        metadata: &SessionMetadata,
    ) -> std::result::Result<(), SessionError> {
        self.sessions
            .remove(&metadata.id)
            .ok_or_else(|| SessionError::NotFound(format!("Session not found: {}", metadata.id)))?;
        Ok(())
    }

    async fn fork(
        &mut self,
        source_metadata: &SessionMetadata,
        options: crate::harness::types::ForkOptions,
    ) -> std::result::Result<crate::harness::types::Session, SessionError> {
        let source = self.open(source_metadata).await?;
        let entries = source.get_entries(None).await;

        let forked_entries = if let Some(entry_id) = &options.entry_id {
            let target = source.get_entry(entry_id).await.ok_or_else(|| {
                SessionError::InvalidForkTarget(format!("Entry {entry_id} not found"))
            })?;

            let effective_leaf_id = match options.position.as_deref() {
                Some("at") => Some(target.id().to_string()),
                _ => target.parent_id().map(|s| s.to_string()),
            };

            source
                .get_path_to_root_or_compaction(effective_leaf_id.as_deref())
                .await?
        } else {
            entries
        };

        let id = options
            .id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let metadata = SessionMetadata {
            id: id.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
            cwd: Some(options.cwd),
            parent_session: options.parent_session_path,
        };

        let storage = Box::new(InMemorySessionStorage::new(Some(
            InMemorySessionStorageOptions {
                entries: forked_entries,
                metadata: Some(metadata),
            },
        )));
        let session = crate::harness::types::Session::new(storage);
        self.sessions.insert(id, session);
        Ok(self.sessions.values().last().unwrap().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::types::SessionTreeEntry;
    use crate::pi_ai_types::ContentBlock;

    fn text_message(id: &str, parent: Option<&str>, text: &str) -> SessionTreeEntry {
        SessionTreeEntry::Message {
            id: id.to_string(),
            parent_id: parent.map(|s| s.to_string()),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            message: crate::types::AgentMessage::User {
                content: vec![ContentBlock::Text {
                    text: text.to_string(),
                    text_signature: None,
                }],
                timestamp: 0,
            },
        }
    }

    fn compaction_entry(id: &str, parent: Option<&str>, retained_tail: Option<Vec<crate::types::AgentMessage>>) -> SessionTreeEntry {
        SessionTreeEntry::Compaction {
            id: id.to_string(),
            parent_id: parent.map(|s| s.to_string()),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            summary: "summary".to_string(),
            first_kept_entry_id: None,
            tokens_before: 100,
            retained_tail,
            details: None,
            usage: None,
            from_hook: None,
        }
    }

    #[tokio::test]
    async fn test_get_path_to_root_or_compaction_stops_at_retained_tail() {
        // root -> compaction(with retainedTail) -> msg1 -> msg2 (leaf)
        // The walk must stop at the compaction entry (self-contained checkpoint).
        let mut storage = InMemorySessionStorage::new(None);
        storage
            .append_entry(text_message("root", None, "root"))
            .await
            .unwrap();
        storage
            .append_entry(compaction_entry("comp", Some("root"), Some(vec![])))
            .await
            .unwrap();
        storage
            .append_entry(text_message("m1", Some("comp"), "m1"))
            .await
            .unwrap();
        storage
            .append_entry(text_message("m2", Some("m1"), "m2"))
            .await
            .unwrap();
        storage.set_leaf_id(Some("m2".to_string())).await.unwrap();

        let path = storage
            .get_path_to_root_or_compaction(Some("m2"))
            .await
            .unwrap();
        let ids: Vec<&str> = path.iter().map(|e| e.id()).collect();
        // stops at compaction: [comp, m1, m2] — root is NOT included
        assert_eq!(ids, vec!["comp", "m1", "m2"]);
    }

    #[tokio::test]
    async fn test_get_path_to_root_or_compaction_stops_at_first_kept() {
        // root -> pre -> compaction(firstKeptEntryId=pre) -> m1 -> m2 (leaf)
        // Without retainedTail, the walk continues upward and stops at the
        // first kept entry (pre), excluding root.
        let mut storage = InMemorySessionStorage::new(None);
        storage
            .append_entry(text_message("root", None, "root"))
            .await
            .unwrap();
        storage
            .append_entry(text_message("pre", Some("root"), "pre"))
            .await
            .unwrap();
        storage
            .append_entry(SessionTreeEntry::Compaction {
                id: "comp".to_string(),
                parent_id: Some("pre".to_string()),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                summary: "summary".to_string(),
                first_kept_entry_id: Some("pre".to_string()),
                tokens_before: 100,
                retained_tail: None,
                details: None,
                usage: None,
                from_hook: None,
            })
            .await
            .unwrap();
        storage
            .append_entry(text_message("m1", Some("comp"), "m1"))
            .await
            .unwrap();
        storage
            .append_entry(text_message("m2", Some("m1"), "m2"))
            .await
            .unwrap();
        storage.set_leaf_id(Some("m2".to_string())).await.unwrap();

        let path = storage
            .get_path_to_root_or_compaction(Some("m2"))
            .await
            .unwrap();
        let ids: Vec<&str> = path.iter().map(|e| e.id()).collect();
        // stops at first kept entry (pre): [pre, comp, m1, m2] — root is NOT included
        assert_eq!(ids, vec!["pre", "comp", "m1", "m2"]);
    }

    #[tokio::test]
    async fn test_get_session_stats_aggregates_usage() {
        let mut storage = InMemorySessionStorage::new(None);
        storage
            .append_entry(SessionTreeEntry::Message {
                id: "m1".to_string(),
                parent_id: None,
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                message: crate::types::AgentMessage::Assistant {
                    content: vec![],
                    api: "openai-completions".into(),
                    provider: "openai".into(),
                    model: "gpt-5.5".into(),
                    usage: crate::pi_ai_types::Usage {
                        input: 100,
                        output: 50,
                        cache_read: 10,
                        cache_write: 5,
                        reasoning: None,
                        cache_write_1h: None,
                        total_tokens: 165,
                        cost: crate::pi_ai_types::UsageCost {
                            input: 1.0,
                            output: 2.0,
                            cache_read: 0.1,
                            cache_write: 0.2,
                            total: 3.3,
                        },
                    },
                    stop_reason: Some(crate::pi_ai_types::StopReason::Stop),
                    error_message: None,
                    timestamp: 0,
                },
            })
            .await
            .unwrap();

        let stats = storage.get_session_stats().await;
        assert_eq!(stats.message_count, 1);
        assert_eq!(stats.cached_tokens, 10);
        assert_eq!(stats.uncached_tokens, 105);
        assert_eq!(stats.total_tokens, 165);
        assert_eq!(stats.cost_total, 3.3);
    }
}
