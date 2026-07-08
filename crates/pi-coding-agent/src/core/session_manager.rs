use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config;

pub const CURRENT_SESSION_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHeader {
    #[serde(rename = "type")]
    pub entry_type: String,
    pub version: Option<u32>,
    pub id: String,
    pub timestamp: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewSessionOptions {
    pub id: Option<String>,
    pub parent_session: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionEntry {
    #[serde(rename = "message")]
    Message {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        message: serde_json::Value,
    },
    #[serde(rename = "thinking_level_change")]
    ThinkingLevelChange {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        thinking_level: String,
    },
    #[serde(rename = "model_change")]
    ModelChange {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        provider: String,
        model_id: String,
    },
    #[serde(rename = "compaction")]
    Compaction {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        summary: String,
        first_kept_entry_id: String,
        tokens_before: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        from_hook: Option<bool>,
    },
    #[serde(rename = "branch_summary")]
    BranchSummary {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        from_id: String,
        summary: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        from_hook: Option<bool>,
    },
    #[serde(rename = "custom")]
    Custom {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        custom_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
    },
    #[serde(rename = "custom_message")]
    CustomMessage {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        custom_type: String,
        content: serde_json::Value,
        display: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
    },
    #[serde(rename = "label")]
    Label {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        target_id: String,
        label: Option<String>,
    },
    #[serde(rename = "session_info")]
    SessionInfo {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

impl SessionEntry {
    pub fn id(&self) -> &str {
        match self {
            SessionEntry::Message { id, .. }
            | SessionEntry::ThinkingLevelChange { id, .. }
            | SessionEntry::ModelChange { id, .. }
            | SessionEntry::Compaction { id, .. }
            | SessionEntry::BranchSummary { id, .. }
            | SessionEntry::Custom { id, .. }
            | SessionEntry::CustomMessage { id, .. }
            | SessionEntry::Label { id, .. }
            | SessionEntry::SessionInfo { id, .. } => id,
        }
    }

    pub fn parent_id(&self) -> Option<&str> {
        match self {
            SessionEntry::Message { parent_id, .. }
            | SessionEntry::ThinkingLevelChange { parent_id, .. }
            | SessionEntry::ModelChange { parent_id, .. }
            | SessionEntry::Compaction { parent_id, .. }
            | SessionEntry::BranchSummary { parent_id, .. }
            | SessionEntry::Custom { parent_id, .. }
            | SessionEntry::CustomMessage { parent_id, .. }
            | SessionEntry::Label { parent_id, .. }
            | SessionEntry::SessionInfo { parent_id, .. } => parent_id.as_deref(),
        }
    }

    pub fn timestamp(&self) -> &str {
        match self {
            SessionEntry::Message { timestamp, .. }
            | SessionEntry::ThinkingLevelChange { timestamp, .. }
            | SessionEntry::ModelChange { timestamp, .. }
            | SessionEntry::Compaction { timestamp, .. }
            | SessionEntry::BranchSummary { timestamp, .. }
            | SessionEntry::Custom { timestamp, .. }
            | SessionEntry::CustomMessage { timestamp, .. }
            | SessionEntry::Label { timestamp, .. }
            | SessionEntry::SessionInfo { timestamp, .. } => timestamp,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionContext {
    pub messages: Vec<serde_json::Value>,
    pub thinking_level: String,
    pub model: Option<ModelInfo>,
}

#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub provider: String,
    pub model_id: String,
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub path: PathBuf,
    pub id: String,
    pub cwd: String,
    pub name: Option<String>,
    pub parent_session_path: Option<String>,
    pub created: DateTime<Utc>,
    pub modified: DateTime<Utc>,
    pub message_count: usize,
    pub first_message: String,
    pub all_messages_text: String,
}

#[derive(Debug, Clone)]
pub struct SessionTreeNode {
    pub entry: SessionEntry,
    pub children: Vec<SessionTreeNode>,
    pub label: Option<String>,
    pub label_timestamp: Option<String>,
}

fn create_session_id() -> String {
    Uuid::new_v4().to_string()
}

fn generate_id(existing: &HashMap<String, SessionEntry>) -> String {
    let mut id = create_session_id();
    while existing.contains_key(&id) {
        id = create_session_id();
    }
    id
}

pub fn derive_short_session_id() -> String {
    let uuid = Uuid::new_v4();
    let hex = uuid.to_string();
    // Take the last segment of the UUID (after last '-') as short ID
    hex.rsplit('-')
        .next()
        .unwrap_or(&hex[..8])
        .to_string()
}

fn assert_valid_session_id(id: &str) {
    if id.is_empty() {
        panic!("Session ID must not be empty");
    }
}

#[derive(Debug, Clone)]
enum FileEntry {
    Header(SessionHeader),
    Entry(SessionEntry),
}

fn load_entries_from_file(file_path: &Path) -> Vec<FileEntry> {
    if !file_path.exists() {
        return Vec::new();
    }

    let file = match fs::File::open(file_path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };

    let reader = std::io::BufReader::new(file);
    let mut entries = Vec::new();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if value.get("type").and_then(|v| v.as_str()) == Some("session") {
                if let Ok(header) = serde_json::from_value::<SessionHeader>(value) {
                    entries.push(FileEntry::Header(header));
                    continue;
                }
            }
            if let Ok(value2) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if let Ok(entry) = serde_json::from_value::<SessionEntry>(value2) {
                    entries.push(FileEntry::Entry(entry));
                }
            }
        }
    }

    entries
}

pub fn build_session_context(
    entries: &[SessionEntry],
    by_id: &HashMap<String, SessionEntry>,
    leaf_id: Option<&str>,
) -> SessionContext {
    if entries.is_empty() {
        return SessionContext {
            messages: Vec::new(),
            thinking_level: "off".to_string(),
            model: None,
        };
    }

    let leaf = if let Some(lid) = leaf_id {
        by_id.get(lid)
    } else {
        entries.last()
    };

    let leaf = match leaf {
        Some(l) => l,
        None => {
            return SessionContext {
                messages: Vec::new(),
                thinking_level: "off".to_string(),
                model: None,
            }
        }
    };

    let mut path = Vec::new();
    let mut current = Some(leaf);
    while let Some(entry) = current {
        path.push(entry.clone());
        current = entry.parent_id().and_then(|pid| by_id.get(pid));
    }
    path.reverse();

    let mut thinking_level = "off".to_string();
    let mut model: Option<ModelInfo> = None;
    let mut compaction_id: Option<String> = None;

    for entry in &path {
        match entry {
            SessionEntry::ThinkingLevelChange {
                thinking_level: tl, ..
            } => {
                thinking_level = tl.clone();
            }
            SessionEntry::ModelChange {
                provider, model_id, ..
            } => {
                model = Some(ModelInfo {
                    provider: provider.clone(),
                    model_id: model_id.clone(),
                });
            }
            SessionEntry::Compaction { id, .. } => {
                compaction_id = Some(id.clone());
            }
            _ => {}
        }
    }

    let mut messages = Vec::new();

    let append_message = |entry: &SessionEntry, msgs: &mut Vec<serde_json::Value>| match entry {
        SessionEntry::Message { message, .. } => {
            msgs.push(message.clone());
        }
        SessionEntry::BranchSummary { summary, .. } => {
            let compact_msg = serde_json::json!({
                "role": "user",
                "content": format!("[Branch Summary]\n{}", summary),
            });
            msgs.push(compact_msg);
        }
        SessionEntry::CustomMessage { content, .. } => {
            let custom_msg = serde_json::json!({
                "role": "user",
                "content": content,
            });
            msgs.push(custom_msg);
        }
        _ => {}
    };

    if let Some(ref cid) = compaction_id {
        let compaction_idx = path.iter().position(|e| e.id() == cid).unwrap_or(0);

        let compaction_entry = &path[compaction_idx];
        if let SessionEntry::Compaction {
            summary,
            first_kept_entry_id,
            tokens_before,
            ..
        } = compaction_entry
        {
            let compact_msg = serde_json::json!({
                "role": "user",
                "content": format!("[Compaction Summary ({} tokens before)]\n{}", tokens_before, summary),
            });
            messages.push(compact_msg);

            let mut found_first_kept = false;
            for entry in &path[..compaction_idx] {
                if entry.id() == first_kept_entry_id {
                    found_first_kept = true;
                }
                if found_first_kept {
                    append_message(entry, &mut messages);
                }
            }

            for entry in &path[compaction_idx + 1..] {
                append_message(entry, &mut messages);
            }
        }
    } else {
        for entry in &path {
            append_message(entry, &mut messages);
        }
    }

    SessionContext {
        messages,
        thinking_level,
        model,
    }
}

pub struct SessionManager {
    session_id: String,
    session_file: Option<PathBuf>,
    session_dir: PathBuf,
    cwd: String,
    persist: bool,
    flushed: bool,
    file_entries: Vec<FileEntry>,
    by_id: HashMap<String, SessionEntry>,
    labels_by_id: HashMap<String, String>,
    label_timestamps_by_id: HashMap<String, String>,
    leaf_id: Option<String>,
    last_run_prompt: Option<String>,
}

impl SessionManager {
    pub fn default_session_dir(cwd: &str, agent_dir: &str) -> String {
        let path = std::path::Path::new(agent_dir).join("sessions");
        path.to_string_lossy().to_string()
    }

    pub fn new(
        cwd: &str,
        session_dir: &str,
        session_file: Option<&str>,
        persist: bool,
        new_session_options: Option<NewSessionOptions>,
    ) -> Self {
        let resolved_cwd = config::resolve_path(cwd);
        let resolved_session_dir = PathBuf::from(session_dir);

        if persist && !resolved_session_dir.exists() {
            fs::create_dir_all(&resolved_session_dir).ok();
        }

        let mut mgr = Self {
            session_id: String::new(),
            session_file: None,
            session_dir: resolved_session_dir,
            cwd: resolved_cwd,
            persist,
            flushed: false,
            file_entries: Vec::new(),
            by_id: HashMap::new(),
            labels_by_id: HashMap::new(),
            label_timestamps_by_id: HashMap::new(),
            leaf_id: None,
            last_run_prompt: None,
        };

        if let Some(sf) = session_file {
            mgr.set_session_file(sf);
        } else {
            mgr.new_session(new_session_options);
        }

        mgr
    }

    pub fn set_session_file(&mut self, session_file: &str) {
        let resolved = PathBuf::from(session_file);
        self.session_file = Some(resolved.clone());

        if resolved.exists() {
            self.file_entries = load_entries_from_file(&resolved);
            if self.file_entries.is_empty() {
                self.new_session(None);
                return;
            }

            self.rebuild_index();

            if let Some(FileEntry::Header(header)) = self.file_entries.first() {
                self.session_id = header.id.clone();
            }

            self.leaf_id = self.find_last_leaf_id();
        } else {
            self.new_session(None);
        }
    }

    fn new_session(&mut self, options: Option<NewSessionOptions>) {
        let id = options
            .as_ref()
            .and_then(|o| o.id.clone())
            .unwrap_or_else(create_session_id);

        if let Some(ref opt_id) = options.as_ref().and_then(|o| o.id.clone()) {
            assert_valid_session_id(opt_id);
        }

        self.session_id = id.clone();
        self.leaf_id = None;
        self.file_entries.clear();
        self.by_id.clear();
        self.labels_by_id.clear();
        self.label_timestamps_by_id.clear();
        self.flushed = false;

        let timestamp = Utc::now().to_rfc3339();
        let file_timestamp = timestamp.replace([':', '.'], "-");
        let session_file = self
            .session_dir
            .join(format!("{}_{}.jsonl", file_timestamp, id));

        let header = SessionHeader {
            entry_type: "session".to_string(),
            version: Some(CURRENT_SESSION_VERSION),
            id: id.clone(),
            timestamp: timestamp.clone(),
            cwd: self.cwd.clone(),
            parent_session: options.and_then(|o| o.parent_session),
        };

        self.file_entries.push(FileEntry::Header(header.clone()));

        if self.persist {
            if let Some(parent) = session_file.parent() {
                fs::create_dir_all(parent).ok();
            }
            if let Ok(mut f) = fs::File::create(&session_file) {
                let _ = writeln!(f, "{}", serde_json::to_string(&header).unwrap_or_default());
            }
        }

        self.session_file = Some(session_file);
    }

    fn rebuild_index(&mut self) {
        self.by_id.clear();
        self.labels_by_id.clear();
        self.label_timestamps_by_id.clear();

        for entry in &self.file_entries {
            if let FileEntry::Entry(e) = entry {
                self.by_id.insert(e.id().to_string(), e.clone());

                if let SessionEntry::Label {
                    target_id,
                    label,
                    timestamp,
                    ..
                } = e
                {
                    if let Some(l) = label {
                        self.labels_by_id.insert(target_id.clone(), l.clone());
                        self.label_timestamps_by_id
                            .insert(target_id.clone(), timestamp.clone());
                    } else {
                        self.labels_by_id.remove(target_id);
                        self.label_timestamps_by_id.remove(target_id);
                    }
                }
            }
        }
    }

    fn find_last_leaf_id(&self) -> Option<String> {
        let child_ids: std::collections::HashSet<&str> =
            self.by_id.values().filter_map(|e| e.parent_id()).collect();

        let mut last = None;
        for e in self.by_id.values() {
            if !child_ids.contains(e.id()) {
                last = Some(e.id().to_string());
            }
        }
        last
    }

    fn persist_entry_str(&mut self, json: &str) {
        if !self.persist {
            return;
        }

        if let Some(ref session_file) = self.session_file.clone() {
            if !self.flushed {
                if let Ok(mut f) = fs::OpenOptions::new().append(true).open(session_file) {
                    let _ = writeln!(f, "{}", json);
                }
            } else {
                if let Ok(mut f) = fs::File::create(session_file) {
                    for fe in &self.file_entries {
                        let line = match fe {
                            FileEntry::Header(h) => serde_json::to_string(h).unwrap_or_default(),
                            FileEntry::Entry(e) => serde_json::to_string(e).unwrap_or_default(),
                        };
                        let _ = writeln!(f, "{}", line);
                    }
                }
                self.flushed = false;
            }
        }
    }

    fn append_entry(&mut self, entry: SessionEntry) {
        let json = serde_json::to_string(&entry).unwrap_or_default();
        let id = entry.id().to_string();
        self.file_entries.push(FileEntry::Entry(entry));
        self.by_id.insert(
            id.clone(),
            self.by_id.get(&id).cloned().unwrap_or(
                // Re-extract from file_entries
                match self.file_entries.last() {
                    Some(FileEntry::Entry(e)) => e.clone(),
                    _ => unreachable!(),
                },
            ),
        );
        self.leaf_id = Some(id.clone());
        self.persist_entry_str(&json);
    }

    pub fn append_message(&mut self, message: serde_json::Value) -> String {
        let id = generate_id(&self.by_id);
        let timestamp = Utc::now().to_rfc3339();
        let entry = SessionEntry::Message {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp,
            message,
        };
        let json = serde_json::to_string(&entry).unwrap_or_default();
        self.file_entries.push(FileEntry::Entry(entry.clone()));
        self.by_id.insert(id.clone(), entry);
        self.leaf_id = Some(id.clone());
        self.persist_entry_str(&json);
        id
    }

    pub fn append_thinking_level_change(&mut self, thinking_level: &str) -> String {
        let id = generate_id(&self.by_id);
        let timestamp = Utc::now().to_rfc3339();
        let entry = SessionEntry::ThinkingLevelChange {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp,
            thinking_level: thinking_level.to_string(),
        };
        let json = serde_json::to_string(&entry).unwrap_or_default();
        self.file_entries.push(FileEntry::Entry(entry.clone()));
        self.by_id.insert(id.clone(), entry);
        self.leaf_id = Some(id.clone());
        self.persist_entry_str(&json);
        id
    }

    pub fn append_model_change(&mut self, provider: &str, model_id: &str) -> String {
        let id = generate_id(&self.by_id);
        let timestamp = Utc::now().to_rfc3339();
        let entry = SessionEntry::ModelChange {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp,
            provider: provider.to_string(),
            model_id: model_id.to_string(),
        };
        let json = serde_json::to_string(&entry).unwrap_or_default();
        self.file_entries.push(FileEntry::Entry(entry.clone()));
        self.by_id.insert(id.clone(), entry);
        self.leaf_id = Some(id.clone());
        self.persist_entry_str(&json);
        id
    }

    pub fn append_compaction(
        &mut self,
        summary: &str,
        first_kept_entry_id: &str,
        tokens_before: u64,
        details: Option<serde_json::Value>,
        from_hook: Option<bool>,
    ) -> String {
        let id = generate_id(&self.by_id);
        let timestamp = Utc::now().to_rfc3339();
        let entry = SessionEntry::Compaction {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp,
            summary: summary.to_string(),
            first_kept_entry_id: first_kept_entry_id.to_string(),
            tokens_before,
            details,
            from_hook,
        };
        let json = serde_json::to_string(&entry).unwrap_or_default();
        self.file_entries.push(FileEntry::Entry(entry.clone()));
        self.by_id.insert(id.clone(), entry);
        self.leaf_id = Some(id.clone());
        self.persist_entry_str(&json);
        id
    }

    pub fn append_branch_summary(
        &mut self,
        from_id: &str,
        summary: &str,
        details: Option<serde_json::Value>,
        from_hook: Option<bool>,
    ) -> String {
        let id = generate_id(&self.by_id);
        let timestamp = Utc::now().to_rfc3339();
        let entry = SessionEntry::BranchSummary {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp,
            from_id: from_id.to_string(),
            summary: summary.to_string(),
            details,
            from_hook,
        };
        let json = serde_json::to_string(&entry).unwrap_or_default();
        self.file_entries.push(FileEntry::Entry(entry.clone()));
        self.by_id.insert(id.clone(), entry);
        self.leaf_id = Some(id.clone());
        self.persist_entry_str(&json);
        id
    }

    pub fn append_custom_entry(
        &mut self,
        custom_type: &str,
        data: Option<serde_json::Value>,
    ) -> String {
        let id = generate_id(&self.by_id);
        let timestamp = Utc::now().to_rfc3339();
        let entry = SessionEntry::Custom {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp,
            custom_type: custom_type.to_string(),
            data,
        };
        let json = serde_json::to_string(&entry).unwrap_or_default();
        self.file_entries.push(FileEntry::Entry(entry.clone()));
        self.by_id.insert(id.clone(), entry);
        self.leaf_id = Some(id.clone());
        self.persist_entry_str(&json);
        id
    }

    pub fn append_session_info(&mut self, name: &str) -> String {
        let id = generate_id(&self.by_id);
        let timestamp = Utc::now().to_rfc3339();
        let entry = SessionEntry::SessionInfo {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp,
            name: Some(name.trim().to_string()),
        };
        let json = serde_json::to_string(&entry).unwrap_or_default();
        self.file_entries.push(FileEntry::Entry(entry.clone()));
        self.by_id.insert(id.clone(), entry);
        self.leaf_id = Some(id.clone());
        self.persist_entry_str(&json);
        id
    }

    pub fn append_custom_message_entry(
        &mut self,
        custom_type: &str,
        content: serde_json::Value,
        display: bool,
        details: Option<serde_json::Value>,
    ) -> String {
        let id = generate_id(&self.by_id);
        let timestamp = Utc::now().to_rfc3339();
        let entry = SessionEntry::CustomMessage {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp,
            custom_type: custom_type.to_string(),
            content,
            display,
            details,
        };
        let json = serde_json::to_string(&entry).unwrap_or_default();
        self.file_entries.push(FileEntry::Entry(entry.clone()));
        self.by_id.insert(id.clone(), entry);
        self.leaf_id = Some(id.clone());
        self.persist_entry_str(&json);
        id
    }

    pub fn get_leaf_id(&self) -> Option<&str> {
        self.leaf_id.as_deref()
    }

    pub fn get_leaf_entry(&self) -> Option<&SessionEntry> {
        self.leaf_id.as_ref().and_then(|id| self.by_id.get(id))
    }

    pub fn get_entry(&self, id: &str) -> Option<&SessionEntry> {
        self.by_id.get(id)
    }

    pub fn get_by_id(&self) -> &HashMap<String, SessionEntry> {
        &self.by_id
    }

    pub fn get_children(&self, parent_id: &str) -> Vec<&SessionEntry> {
        self.by_id
            .values()
            .filter(|e| e.parent_id() == Some(parent_id))
            .collect()
    }

    pub fn get_label(&self, entry_id: &str) -> Option<&str> {
        self.labels_by_id.get(entry_id).map(|s| s.as_str())
    }

    pub fn get_branch(&self, from_id: Option<&str>) -> Vec<SessionEntry> {
        let start_id = from_id.or(self.leaf_id.as_deref());
        let start = match start_id.and_then(|id| self.by_id.get(id)) {
            Some(e) => e,
            None => return Vec::new(),
        };

        let mut path = Vec::new();
        let mut current = Some(start);
        while let Some(entry) = current {
            path.push(entry.clone());
            current = entry.parent_id().and_then(|pid| self.by_id.get(pid));
        }
        path.reverse();
        path
    }

    pub fn get_header(&self) -> Option<&SessionHeader> {
        self.file_entries.first().and_then(|e| match e {
            FileEntry::Header(h) => Some(h),
            _ => None,
        })
    }

    pub fn get_entries(&self) -> Vec<&SessionEntry> {
        self.file_entries
            .iter()
            .filter_map(|e| match e {
                FileEntry::Entry(entry) => Some(entry),
                _ => None,
            })
            .collect()
    }

    pub fn get_session_id(&self) -> &str {
        &self.session_id
    }

    pub fn get_cwd(&self) -> &str {
        &self.cwd
    }

    pub fn get_session_dir(&self) -> &Path {
        &self.session_dir
    }

    pub fn get_session_file(&self) -> Option<&Path> {
        self.session_file.as_deref()
    }

    pub fn get_session_name(&self) -> Option<String> {
        let entries = self.get_entries();
        for entry in entries.iter().rev() {
            if let SessionEntry::SessionInfo { name, .. } = entry {
                if let Some(n) = name {
                    let trimmed = n.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_string());
                    }
                }
                return None;
            }
        }
        None
    }

    pub fn build_context(&self) -> SessionContext {
        let entries: Vec<SessionEntry> = self.get_entries().into_iter().cloned().collect();
        build_session_context(&entries, &self.by_id, self.leaf_id.as_deref())
    }

    pub fn navigate_to(&mut self, entry_id: &str) -> bool {
        if self.by_id.contains_key(entry_id) {
            self.leaf_id = Some(entry_id.to_string());
            true
        } else {
            false
        }
    }

    pub fn navigate_to_parent(&mut self) -> bool {
        if let Some(leaf_id) = &self.leaf_id {
            if let Some(entry) = self.by_id.get(leaf_id) {
                if let Some(parent_id) = entry.parent_id() {
                    self.leaf_id = Some(parent_id.to_string());
                    return true;
                }
            }
        }
        false
    }

    pub fn get_tree(&self) -> Vec<SessionTreeNode> {
        let roots: Vec<&SessionEntry> = self
            .by_id
            .values()
            .filter(|e| e.parent_id().is_none())
            .collect();
        roots.into_iter().map(|r| self.build_tree_node(r)).collect()
    }

    fn build_tree_node(&self, entry: &SessionEntry) -> SessionTreeNode {
        let children = self
            .get_children(entry.id())
            .into_iter()
            .map(|c| self.build_tree_node(c))
            .collect();

        let label = self.labels_by_id.get(entry.id()).cloned();
        let label_timestamp = self.label_timestamps_by_id.get(entry.id()).cloned();

        SessionTreeNode {
            entry: entry.clone(),
            children,
            label,
            label_timestamp,
        }
    }

    pub fn set_label(&mut self, target_id: &str, label: Option<&str>) {
        let id = generate_id(&self.by_id);
        let timestamp = Utc::now().to_rfc3339();
        let entry = SessionEntry::Label {
            id,
            parent_id: self.leaf_id.clone(),
            timestamp,
            target_id: target_id.to_string(),
            label: label.map(|l| l.to_string()),
        };

        if let Some(l) = label {
            self.labels_by_id
                .insert(target_id.to_string(), l.to_string());
        } else {
            self.labels_by_id.remove(target_id);
        }

        let json = serde_json::to_string(&entry).unwrap_or_default();
        self.file_entries.push(FileEntry::Entry(entry.clone()));
        self.by_id.insert(entry.id().to_string(), entry);
        self.leaf_id = Some(self.file_entries.last().unwrap().id().to_string());
        self.persist_entry_str(&json);
    }

    pub fn fork_from(
        source_path: &str,
        target_cwd: &str,
        session_dir: Option<&str>,
        options: Option<NewSessionOptions>,
    ) -> Result<Self, String> {
        let resolved_source = PathBuf::from(source_path);
        let resolved_target_cwd = config::resolve_path(target_cwd);

        let source_entries = load_entries_from_file(&resolved_source);
        if source_entries.is_empty() {
            return Err(format!(
                "Cannot fork: source session file is empty or invalid: {}",
                resolved_source.display()
            ));
        }

        let source_header = source_entries.iter().find_map(|e| match e {
            FileEntry::Header(h) => Some(h.clone()),
            _ => None,
        });

        let _source_header = match source_header {
            Some(h) => h,
            None => {
                return Err(format!(
                    "Cannot fork: source session has no header: {}",
                    resolved_source.display()
                ))
            }
        };

        let dir = match session_dir {
            Some(d) => PathBuf::from(d),
            None => config::get_default_session_dir(&resolved_target_cwd, None),
        };

        if !dir.exists() {
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        }

        if let Some(ref opts) = options {
            if let Some(ref id) = opts.id {
                assert_valid_session_id(id);
            }
        }

        let new_session_id = options
            .as_ref()
            .and_then(|o| o.id.clone())
            .unwrap_or_else(create_session_id);

        let timestamp = Utc::now().to_rfc3339();
        let file_timestamp = timestamp.replace([':', '.'], "-");
        let new_session_file = dir.join(format!("{}_{}.jsonl", file_timestamp, new_session_id));

        let new_header = SessionHeader {
            entry_type: "session".to_string(),
            version: Some(CURRENT_SESSION_VERSION),
            id: new_session_id.clone(),
            timestamp: timestamp.clone(),
            cwd: resolved_target_cwd.clone(),
            parent_session: Some(resolved_source.to_string_lossy().to_string()),
        };

        {
            let mut f = fs::File::create(&new_session_file).map_err(|e| e.to_string())?;
            writeln!(
                f,
                "{}",
                serde_json::to_string(&new_header).unwrap_or_default()
            )
            .map_err(|e| e.to_string())?;

            for entry in &source_entries {
                if let FileEntry::Entry(e) = entry {
                    writeln!(f, "{}", serde_json::to_string(e).unwrap_or_default())
                        .map_err(|e| e.to_string())?;
                }
            }
        }

        Ok(Self::new(
            &resolved_target_cwd,
            &dir.to_string_lossy(),
            Some(&new_session_file.to_string_lossy()),
            true,
            None,
        ))
    }

    pub async fn list(cwd: &str, session_dir: Option<&str>) -> Vec<SessionInfo> {
        let dir = match session_dir {
            Some(d) => PathBuf::from(d),
            None => config::get_default_session_dir(cwd, None),
        };
        list_sessions_from_dir(&dir)
    }

    pub async fn list_all(session_dir: Option<&str>) -> Vec<SessionInfo> {
        let sessions_dir = match session_dir {
            Some(d) => PathBuf::from(d),
            None => config::get_sessions_dir(),
        };
        list_sessions_from_dir(&sessions_dir)
    }

    /// Set the run prompt for this session. The run prompt is the user's
    /// active input text. It is preserved through tool/session refresh
    /// operations and can be retrieved via [take_run_prompt].
    pub fn set_run_prompt(&mut self, prompt: &str) {
        self.last_run_prompt = Some(prompt.to_string());
    }

    /// Mark the session as flushed, so the next persist rewrites the entire
    /// file from the in-memory tree rather than appending.
    pub fn set_flushed(&mut self) {
        self.flushed = true;
    }

    /// Take the run prompt, consuming it. Returns `None` if no run prompt
    /// was set or it was already taken. This is a one-shot get — the second
    /// call returns `None`.
    pub fn take_run_prompt(&mut self) -> Option<String> {
        self.last_run_prompt.take()
    }

    /// Refresh session configuration by re-reading the session file from disk.
    ///
    /// Called before starting a new agent interaction turn to pick up any
    /// external changes to the session file (e.g. entries appended by another
    /// process or after a compaction/rewrite).
    pub async fn refresh_config(&mut self) -> Result<(), String> {
        if self.flushed {
            // The file was rewritten from the in-memory tree, so disk is
            // already in sync — no need to re-read.
            self.flushed = false;
            return Ok(());
        }

        if let Some(ref session_file) = self.session_file.clone() {
            if session_file.exists() {
                let new_entries = load_entries_from_file(session_file);
                if !new_entries.is_empty() {
                    self.file_entries = new_entries;
                    self.rebuild_index();
                    self.leaf_id = self.find_last_leaf_id();
                }
            }
        }

        Ok(())
    }
}

impl FileEntry {
    fn id(&self) -> String {
        match self {
            FileEntry::Header(h) => h.id.clone(),
            FileEntry::Entry(e) => e.id().to_string(),
        }
    }
}

fn list_sessions_from_dir(dir: &Path) -> Vec<SessionInfo> {
    if !dir.exists() {
        return Vec::new();
    }

    let mut sessions = Vec::new();

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                if let Some(info) = build_session_info(&path) {
                    sessions.push(info);
                }
            }
        }
    }

    sessions.sort_by(|a, b| b.modified.cmp(&a.modified));
    sessions
}

fn build_session_info(file_path: &Path) -> Option<SessionInfo> {
    let entries = load_entries_from_file(file_path);

    let header = entries.iter().find_map(|e| match e {
        FileEntry::Header(h) => Some(h.clone()),
        _ => None,
    })?;

    let mut message_count = 0usize;
    let mut first_message = String::new();
    let mut all_messages = Vec::new();
    let mut last_activity_time: Option<DateTime<Utc>> = None;
    let mut name: Option<String> = None;

    for entry in &entries {
        if let FileEntry::Entry(e) = entry {
            if let SessionEntry::Message {
                message, timestamp, ..
            } = e
            {
                message_count += 1;
                if let Ok(dt) = DateTime::parse_from_rfc3339(timestamp) {
                    let dt = dt.to_utc();
                    last_activity_time = Some(
                        last_activity_time
                            .map(|t| if dt > t { dt } else { t })
                            .unwrap_or(dt),
                    );
                }
                if let Some(text) = message.get("content").and_then(|c| c.as_str()) {
                    if !text.is_empty() {
                        all_messages.push(text.to_string());
                        if first_message.is_empty() {
                            if message.get("role").and_then(|r| r.as_str()) == Some("user") {
                                first_message = text.to_string();
                            }
                        }
                    }
                }
            } else if let SessionEntry::SessionInfo {
                name: n, timestamp, ..
            } = e
            {
                if n.is_some() {
                    name = n.clone();
                }
                if let Ok(dt) = DateTime::parse_from_rfc3339(timestamp) {
                    let dt = dt.to_utc();
                    last_activity_time = Some(
                        last_activity_time
                            .map(|t| if dt > t { dt } else { t })
                            .unwrap_or(dt),
                    );
                }
            }
        }
    }

    let created = DateTime::parse_from_rfc3339(&header.timestamp)
        .map(|dt| dt.to_utc())
        .ok()?;

    let modified = last_activity_time.unwrap_or(created);

    Some(SessionInfo {
        path: file_path.to_path_buf(),
        id: header.id,
        cwd: header.cwd,
        name,
        parent_session_path: header.parent_session,
        created,
        modified,
        message_count,
        first_message: if first_message.is_empty() {
            "(no messages)".to_string()
        } else {
            first_message
        },
        all_messages_text: all_messages.join(" "),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_session_manager_new_session() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);

        assert!(!mgr.get_session_id().is_empty());
        assert_eq!(mgr.get_cwd(), "/tmp/test");
        assert!(mgr.get_leaf_id().is_none());
    }

    #[test]
    fn test_append_message() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr =
            SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);

        let msg = serde_json::json!({
            "role": "user",
            "content": "Hello"
        });
        let id = mgr.append_message(msg);
        assert!(!id.is_empty());
        assert_eq!(mgr.get_leaf_id(), Some(id.as_str()));
    }

    #[test]
    fn test_append_thinking_level_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr =
            SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);

        let id = mgr.append_thinking_level_change("high");
        assert!(!id.is_empty());
    }

    #[test]
    fn test_append_model_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr =
            SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);

        let id = mgr.append_model_change("anthropic", "claude-3-opus");
        assert!(!id.is_empty());
    }

    #[test]
    fn test_build_context() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr =
            SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);

        mgr.append_thinking_level_change("medium");
        mgr.append_message(serde_json::json!({
            "role": "user",
            "content": "Hello"
        }));
        mgr.append_message(serde_json::json!({
            "role": "assistant",
            "content": "Hi there"
        }));

        let ctx = mgr.build_context();
        assert_eq!(ctx.thinking_level, "medium");
        assert_eq!(ctx.messages.len(), 2);
    }

    #[test]
    fn test_navigate_to() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr =
            SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);

        let id1 = mgr.append_message(serde_json::json!({
            "role": "user",
            "content": "First"
        }));
        let id2 = mgr.append_message(serde_json::json!({
            "role": "user",
            "content": "Second"
        }));

        assert!(mgr.navigate_to(&id1));
        assert_eq!(mgr.get_leaf_id(), Some(id1.as_str()));

        assert!(mgr.navigate_to(&id2));
        assert_eq!(mgr.get_leaf_id(), Some(id2.as_str()));
    }

    #[test]
    fn test_get_branch() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr =
            SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);

        let id1 = mgr.append_message(serde_json::json!({
            "role": "user",
            "content": "First"
        }));
        mgr.append_message(serde_json::json!({
            "role": "user",
            "content": "Second"
        }));

        let branch = mgr.get_branch(Some(&id1));
        assert_eq!(branch.len(), 1);
    }

    #[test]
    fn test_session_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr =
            SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);

        assert!(mgr.get_session_name().is_none());

        mgr.append_session_info("My Session");
        assert_eq!(mgr.get_session_name(), Some("My Session".to_string()));
    }

    #[test]
    fn test_persist_to_file() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().to_str().unwrap();

        let mut mgr = SessionManager::new("/tmp/test", session_dir, None, true, None);

        mgr.append_message(serde_json::json!({
            "role": "user",
            "content": "Hello"
        }));

        let session_file = mgr.get_session_file().unwrap().to_path_buf();
        assert!(session_file.exists());

        let content = fs::read_to_string(&session_file).unwrap();
        assert!(content.contains("\"type\":\"session\""));
        assert!(content.contains("\"type\":\"message\""));
    }

    #[test]
    fn test_load_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().to_str().unwrap();

        let mut mgr = SessionManager::new("/tmp/test", session_dir, None, true, None);

        mgr.append_message(serde_json::json!({
            "role": "user",
            "content": "Hello"
        }));

        let session_file = mgr.get_session_file().unwrap().to_path_buf();
        let session_id = mgr.get_session_id().to_string();

        let mgr2 = SessionManager::new(
            "/tmp/test",
            session_dir,
            Some(session_file.to_str().unwrap()),
            true,
            None,
        );

        assert_eq!(mgr2.get_session_id(), session_id);
        assert_eq!(mgr2.get_entries().len(), 1);
    }

    #[test]
    fn test_append_compaction() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr =
            SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);

        let msg_id = mgr.append_message(serde_json::json!({
            "role": "user",
            "content": "Hello"
        }));

        let id = mgr.append_compaction("Summary text", &msg_id, 1000, None, None);
        assert!(!id.is_empty());
    }

    #[test]
    fn test_build_context_with_compaction() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr =
            SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);

        let msg_id = mgr.append_message(serde_json::json!({
            "role": "user",
            "content": "Old message"
        }));
        mgr.append_compaction("Summary of old messages", &msg_id, 1000, None, None);
        mgr.append_message(serde_json::json!({
            "role": "user",
            "content": "New message"
        }));

        let ctx = mgr.build_context();
        assert!(ctx.messages.len() >= 2);
    }

    #[test]
    fn test_custom_entry() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr =
            SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);

        let id = mgr.append_custom_entry("my_extension", Some(serde_json::json!({"key": "value"})));
        assert!(!id.is_empty());
    }

    #[test]
    fn test_label() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr =
            SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);

        let msg_id = mgr.append_message(serde_json::json!({
            "role": "user",
            "content": "Hello"
        }));

        mgr.set_label(&msg_id, Some("important"));
        assert_eq!(mgr.get_label(&msg_id), Some("important"));

        mgr.set_label(&msg_id, None);
        assert_eq!(mgr.get_label(&msg_id), None);
    }

    #[test]
    fn test_get_tree() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr =
            SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);

        mgr.append_message(serde_json::json!({
            "role": "user",
            "content": "Root"
        }));
        mgr.append_message(serde_json::json!({
            "role": "user",
            "content": "Child"
        }));

        let tree = mgr.get_tree();
        assert!(!tree.is_empty());
    }

    #[test]
    fn test_run_prompt_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr =
            SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);

        mgr.set_run_prompt("write a test");
        let prompt = mgr.take_run_prompt();
        assert_eq!(prompt, Some("write a test".to_string()));

        // Second take should return None
        assert!(mgr.take_run_prompt().is_none());
    }

    #[test]
    fn test_run_prompt_not_set() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr =
            SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);

        assert!(mgr.take_run_prompt().is_none());
    }

    #[test]
    fn test_run_prompt_override() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr =
            SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);

        mgr.set_run_prompt("first prompt");
        mgr.set_run_prompt("second prompt");
        let prompt = mgr.take_run_prompt();
        assert_eq!(prompt, Some("second prompt".to_string()));
    }

    #[test]
    fn test_derive_short_session_id() {
        let id = derive_short_session_id();
        assert!(!id.is_empty());
        assert!(id.len() <= 12); // short enough
    }

    #[tokio::test]
    async fn test_refresh_config_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr =
            SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);
        let result = mgr.refresh_config().await;
        // Should not fail even with non-existent config file
        assert!(result.is_ok());
    }
}
