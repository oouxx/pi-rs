use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
    /// Raw JSON that couldn't be parsed as a SessionEntry (e.g. v1 format).
    /// Stored for migration purposes.
    RawJson(serde_json::Value),
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

        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if val.get("type").and_then(|v| v.as_str()) == Some("session") {
                if let Ok(header) = serde_json::from_value::<SessionHeader>(val.clone()) {
                    entries.push(FileEntry::Header(header));
                    continue;
                }
            }
            if let Ok(entry) = serde_json::from_value::<SessionEntry>(val.clone()) {
                entries.push(FileEntry::Entry(entry));
            } else {
                // Store as raw JSON for migration purposes (e.g. v1 format)
                entries.push(FileEntry::RawJson(val));
            }
        }
    }

    entries
}

/// Validate that a session file is valid by checking the first 512 bytes.
/// Returns true if the file starts with a valid session header.
pub fn is_valid_session_file(file_path: &Path) -> bool {
    if !file_path.exists() {
        return false;
    }

    // Read the first 512 bytes to validate the header
    let mut buf = [0u8; 512];
    let mut file = match fs::File::open(file_path) {
        Ok(f) => f,
        Err(_) => return false,
    };

    use std::io::Read;
    let n = match file.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return false,
    };

    if n == 0 {
        return false;
    }

    let head = &buf[..n];
    let head_str = match std::str::from_utf8(head) {
        Ok(s) => s,
        Err(_) => return false,
    };

    // Find the first line (header)
    let first_line = head_str.lines().next().unwrap_or("");
    if first_line.is_empty() {
        return false;
    }

    // Parse the first line as JSON and check for session type
    match serde_json::from_str::<serde_json::Value>(first_line) {
        Ok(val) => val.get("type").and_then(|v| v.as_str()) == Some("session"),
        Err(_) => false,
    }
}

// ============================================================================
// Version migration: v1 → v2 → v3
// ============================================================================

/// Migrate a session file from v1 to the current version (v3).
/// Returns the migrated entries, or the original entries if no migration was needed.
pub fn migrate_session_file(file_path: &Path) -> Result<Vec<FileEntry>, String> {
    let entries = load_entries_from_file(file_path);
    if entries.is_empty() {
        return Ok(entries);
    }

    let header = match entries.first() {
        Some(FileEntry::Header(h)) => h,
        _ => return Ok(entries), // No header, can't determine version
    };

    let version = header.version.unwrap_or(1);
    if version >= CURRENT_SESSION_VERSION {
        return Ok(entries); // Already at current version
    }

    let mut migrated = entries.clone();

    if version < 2 {
        migrated = migrate_v1_to_v2(migrated, file_path)?;
    }

    if version < 3 {
        migrated = migrate_v2_to_v3(migrated, file_path)?;
    }

    // Update the header version
    if let Some(FileEntry::Header(ref mut h)) = migrated.first_mut() {
        h.version = Some(CURRENT_SESSION_VERSION);
    }

    // Write the migrated entries back to the file
    let mut f = fs::File::create(file_path).map_err(|e| e.to_string())?;
    for entry in &migrated {
        let line = match entry {
            FileEntry::Header(h) => serde_json::to_string(h).unwrap_or_default(),
            FileEntry::Entry(e) => serde_json::to_string(e).unwrap_or_default(),
            FileEntry::RawJson(v) => serde_json::to_string(v).unwrap_or_default(),
        };
        writeln!(f, "{}", line).map_err(|e| e.to_string())?;
    }

    Ok(migrated)
}

/// Migrate from v1 to v2 format.
/// v1 → v2 changes:
/// - Add `version` field to header if missing
/// - Ensure all entries have `id` and `parent_id` fields
/// - Convert old-style message entries to the new format
fn migrate_v1_to_v2(entries: Vec<FileEntry>, _file_path: &Path) -> Result<Vec<FileEntry>, String> {
    let mut migrated: Vec<FileEntry> = Vec::new();
    let mut last_id: Option<String> = None;

    for entry in entries {
        match entry {
            FileEntry::Header(mut h) => {
                if h.version.is_none() {
                    h.version = Some(2);
                }
                migrated.push(FileEntry::Header(h));
            }
            FileEntry::Entry(e) => {
                // In v1, entries may not have id/parent_id. Generate them if missing.
                let id = if e.id().is_empty() {
                    generate_id_for_migration(&migrated)
                } else {
                    e.id().to_string()
                };

                let parent_id = e.parent_id().map(|s| s.to_string()).or_else(|| last_id.clone());

                // Reconstruct the entry with proper id/parent_id
                let migrated_entry = add_ids_to_entry(e, &id, parent_id);
                last_id = Some(id);
                migrated.push(FileEntry::Entry(migrated_entry));
            }
            FileEntry::RawJson(val) => {
                // Convert raw JSON to a proper SessionEntry with generated IDs
                let id = generate_id_for_migration(&migrated);
                let parent_id = last_id.clone();
                let timestamp = val.get("timestamp")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| {
                        // Use current time if no timestamp
                        Box::leak(Utc::now().to_rfc3339().into_boxed_str())
                    })
                    .to_string();

                // Determine the entry type from the raw JSON
                let entry_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("message");
                let migrated_entry = match entry_type {
                    "message" => {
                        let message = val.get("message").cloned().unwrap_or(serde_json::Value::Null);
                        SessionEntry::Message {
                            id: id.clone(),
                            parent_id,
                            timestamp,
                            message,
                        }
                    }
                    "thinking_level_change" => {
                        let thinking_level = val.get("thinking_level")
                            .and_then(|v| v.as_str())
                            .unwrap_or("off")
                            .to_string();
                        SessionEntry::ThinkingLevelChange {
                            id: id.clone(),
                            parent_id,
                            timestamp,
                            thinking_level,
                        }
                    }
                    "model_change" => {
                        let provider = val.get("provider")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let model_id = val.get("model_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        SessionEntry::ModelChange {
                            id: id.clone(),
                            parent_id,
                            timestamp,
                            provider,
                            model_id,
                        }
                    }
                    _ => {
                        // Default to custom entry for unknown types
                        let custom_type = entry_type.to_string();
                        SessionEntry::Custom {
                            id: id.clone(),
                            parent_id,
                            timestamp,
                            custom_type,
                            data: Some(val),
                        }
                    }
                };

                last_id = Some(id);
                migrated.push(FileEntry::Entry(migrated_entry));
            }
        }
    }

    Ok(migrated)
}

/// Migrate from v2 to v3 format.
/// v2 → v3 changes:
/// - Add `timestamp` field to entries that are missing it
/// - Normalize entry types to match the current SessionEntry enum
fn migrate_v2_to_v3(entries: Vec<FileEntry>, _file_path: &Path) -> Result<Vec<FileEntry>, String> {
    let mut migrated: Vec<FileEntry> = Vec::new();

    for entry in entries {
        match entry {
            FileEntry::Header(mut h) => {
                h.version = Some(3);
                migrated.push(FileEntry::Header(h));
            }
            FileEntry::Entry(e) => {
                // In v2, entries should already have id/parent_id.
                // Ensure timestamp is present.
                let timestamp = e.timestamp();
                if timestamp.is_empty() {
                    // Generate a timestamp if missing
                    let new_ts = Utc::now().to_rfc3339();
                    let migrated_entry = set_timestamp_on_entry(e, &new_ts);
                    migrated.push(FileEntry::Entry(migrated_entry));
                } else {
                    migrated.push(FileEntry::Entry(e));
                }
            }
            FileEntry::RawJson(val) => {
                // Pass through raw JSON entries; they'll be handled by v1→v2 migration
                migrated.push(FileEntry::RawJson(val));
            }
        }
    }

    Ok(migrated)
}

/// Generate a unique ID for migration purposes.
fn generate_id_for_migration(entries: &[FileEntry]) -> String {
    let existing: std::collections::HashSet<String> = entries
        .iter()
        .filter_map(|e| match e {
            FileEntry::Entry(entry) => Some(entry.id().to_string()),
            FileEntry::Header(_) | FileEntry::RawJson(_) => None,
        })
        .collect();

    let mut id = create_session_id();
    while existing.contains(&id) {
        id = create_session_id();
    }
    id
}

/// Add id and parent_id to an entry, preserving all other fields.
fn add_ids_to_entry(entry: SessionEntry, id: &str, parent_id: Option<String>) -> SessionEntry {
    let timestamp = if entry.timestamp().is_empty() {
        Utc::now().to_rfc3339()
    } else {
        entry.timestamp().to_string()
    };

    match entry {
        SessionEntry::Message { message, .. } => SessionEntry::Message {
            id: id.to_string(),
            parent_id,
            timestamp,
            message,
        },
        SessionEntry::ThinkingLevelChange { thinking_level, .. } => {
            SessionEntry::ThinkingLevelChange {
                id: id.to_string(),
                parent_id,
                timestamp,
                thinking_level,
            }
        }
        SessionEntry::ModelChange {
            provider, model_id, ..
        } => SessionEntry::ModelChange {
            id: id.to_string(),
            parent_id,
            timestamp,
            provider,
            model_id,
        },
        SessionEntry::Compaction {
            summary,
            first_kept_entry_id,
            tokens_before,
            details,
            from_hook,
            ..
        } => SessionEntry::Compaction {
            id: id.to_string(),
            parent_id,
            timestamp,
            summary,
            first_kept_entry_id,
            tokens_before,
            details,
            from_hook,
        },
        SessionEntry::BranchSummary {
            from_id,
            summary,
            details,
            from_hook,
            ..
        } => SessionEntry::BranchSummary {
            id: id.to_string(),
            parent_id,
            timestamp,
            from_id,
            summary,
            details,
            from_hook,
        },
        SessionEntry::Custom {
            custom_type, data, ..
        } => SessionEntry::Custom {
            id: id.to_string(),
            parent_id,
            timestamp,
            custom_type,
            data,
        },
        SessionEntry::CustomMessage {
            custom_type,
            content,
            display,
            details,
            ..
        } => SessionEntry::CustomMessage {
            id: id.to_string(),
            parent_id,
            timestamp,
            custom_type,
            content,
            display,
            details,
        },
        SessionEntry::Label {
            target_id,
            label,
            ..
        } => SessionEntry::Label {
            id: id.to_string(),
            parent_id,
            timestamp,
            target_id,
            label,
        },
        SessionEntry::SessionInfo { name, .. } => SessionEntry::SessionInfo {
            id: id.to_string(),
            parent_id,
            timestamp,
            name,
        },
    }
}

/// Set the timestamp on an entry, preserving all other fields.
fn set_timestamp_on_entry(entry: SessionEntry, timestamp: &str) -> SessionEntry {
    let ts = timestamp.to_string();
    match entry {
        SessionEntry::Message {
            id,
            parent_id,
            message,
            ..
        } => SessionEntry::Message {
            id,
            parent_id,
            timestamp: ts,
            message,
        },
        SessionEntry::ThinkingLevelChange {
            id,
            parent_id,
            thinking_level,
            ..
        } => SessionEntry::ThinkingLevelChange {
            id,
            parent_id,
            timestamp: ts,
            thinking_level,
        },
        SessionEntry::ModelChange {
            id,
            parent_id,
            provider,
            model_id,
            ..
        } => SessionEntry::ModelChange {
            id,
            parent_id,
            timestamp: ts,
            provider,
            model_id,
        },
        SessionEntry::Compaction {
            id,
            parent_id,
            summary,
            first_kept_entry_id,
            tokens_before,
            details,
            from_hook,
            ..
        } => SessionEntry::Compaction {
            id,
            parent_id,
            timestamp: ts,
            summary,
            first_kept_entry_id,
            tokens_before,
            details,
            from_hook,
        },
        SessionEntry::BranchSummary {
            id,
            parent_id,
            from_id,
            summary,
            details,
            from_hook,
            ..
        } => SessionEntry::BranchSummary {
            id,
            parent_id,
            timestamp: ts,
            from_id,
            summary,
            details,
            from_hook,
        },
        SessionEntry::Custom {
            id,
            parent_id,
            custom_type,
            data,
            ..
        } => SessionEntry::Custom {
            id,
            parent_id,
            timestamp: ts,
            custom_type,
            data,
        },
        SessionEntry::CustomMessage {
            id,
            parent_id,
            custom_type,
            content,
            display,
            details,
            ..
        } => SessionEntry::CustomMessage {
            id,
            parent_id,
            timestamp: ts,
            custom_type,
            content,
            display,
            details,
        },
        SessionEntry::Label {
            id,
            parent_id,
            target_id,
            label,
            ..
        } => SessionEntry::Label {
            id,
            parent_id,
            timestamp: ts,
            target_id,
            label,
        },
        SessionEntry::SessionInfo {
            id,
            parent_id,
            name,
            ..
        } => SessionEntry::SessionInfo {
            id,
            parent_id,
            timestamp: ts,
            name,
        },
    }
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

/// Read-only interface for session data access.
/// Matches the TS `ReadonlySessionManager` type.
pub trait ReadonlySessionManager {
    fn get_session_id(&self) -> &str;
    fn get_cwd(&self) -> &str;
    fn get_session_dir(&self) -> &Path;
    fn get_session_file(&self) -> Option<&Path>;
    fn get_session_name(&self) -> Option<String>;
    fn get_leaf_id(&self) -> Option<&str>;
    fn get_leaf_entry(&self) -> Option<&SessionEntry>;
    fn get_entry(&self, id: &str) -> Option<&SessionEntry>;
    fn get_children(&self, parent_id: &str) -> Vec<&SessionEntry>;
    fn get_label(&self, entry_id: &str) -> Option<&str>;
    fn get_branch(&self, from_id: Option<&str>) -> Vec<SessionEntry>;
    fn get_header(&self) -> Option<&SessionHeader>;
    fn get_entries(&self) -> Vec<&SessionEntry>;
    fn get_tree(&self) -> Vec<SessionTreeNode>;
    fn build_context(&self) -> SessionContext;
    fn get_by_id(&self) -> &HashMap<String, SessionEntry>;
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
                            FileEntry::RawJson(v) => serde_json::to_string(v).unwrap_or_default(),
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
}

impl ReadonlySessionManager for SessionManager {
    fn get_session_id(&self) -> &str { self.get_session_id() }
    fn get_cwd(&self) -> &str { self.get_cwd() }
    fn get_session_dir(&self) -> &Path { self.get_session_dir() }
    fn get_session_file(&self) -> Option<&Path> { self.get_session_file() }
    fn get_session_name(&self) -> Option<String> { self.get_session_name() }
    fn get_leaf_id(&self) -> Option<&str> { self.get_leaf_id() }
    fn get_leaf_entry(&self) -> Option<&SessionEntry> { self.get_leaf_entry() }
    fn get_entry(&self, id: &str) -> Option<&SessionEntry> { self.get_entry(id) }
    fn get_children(&self, parent_id: &str) -> Vec<&SessionEntry> { self.get_children(parent_id) }
    fn get_label(&self, entry_id: &str) -> Option<&str> { self.get_label(entry_id) }
    fn get_branch(&self, from_id: Option<&str>) -> Vec<SessionEntry> { self.get_branch(from_id) }
    fn get_header(&self) -> Option<&SessionHeader> { self.get_header() }
    fn get_entries(&self) -> Vec<&SessionEntry> { self.get_entries() }
    fn get_tree(&self) -> Vec<SessionTreeNode> { self.get_tree() }
    fn build_context(&self) -> SessionContext { self.build_context() }
    fn get_by_id(&self) -> &HashMap<String, SessionEntry> { self.get_by_id() }
}

impl SessionManager {
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

    /// Create a branch from the current leaf, returning the new leaf ID.
    /// The branch is created by appending a BranchSummary entry that marks
    /// the fork point, then resetting the leaf to the specified entry.
    /// Returns the entry ID that was branched from.
    pub fn branch(&mut self, from_id: Option<&str>) -> Option<String> {
        let target_id = from_id
            .map(|id| id.to_string())
            .or_else(|| self.leaf_id.clone())?;

        if !self.by_id.contains_key(&target_id) {
            return None;
        }

        // Reset leaf to the target entry, effectively creating a branch
        self.leaf_id = Some(target_id.clone());
        Some(target_id)
    }

    /// Reset the leaf to a specific entry, discarding any entries after it.
    /// Returns true if the entry was found and the leaf was reset.
    pub fn reset_leaf(&mut self, entry_id: &str) -> bool {
        if !self.by_id.contains_key(entry_id) {
            return false;
        }
        self.leaf_id = Some(entry_id.to_string());
        true
    }

    /// Create a branch with a summary. Appends a BranchSummary entry at the
    /// current leaf, then resets the leaf to the specified from_id.
    /// Returns the ID of the BranchSummary entry, or None if from_id is invalid.
    pub fn branch_with_summary(
        &mut self,
        from_id: &str,
        summary: &str,
        details: Option<serde_json::Value>,
    ) -> Option<String> {
        if !self.by_id.contains_key(from_id) {
            return None;
        }

        // Append a BranchSummary entry at the current leaf
        let summary_id = self.append_branch_summary(from_id, summary, details, None);

        // Reset leaf to the from_id, creating the branch
        self.leaf_id = Some(from_id.to_string());

        Some(summary_id)
    }

    /// Create a new branched session file from the current session.
    /// Copies all entries up to (and including) the specified entry_id into
    /// a new session file. Returns the path to the new session file.
    pub fn create_branched_session(
        &mut self,
        entry_id: &str,
        target_cwd: Option<&str>,
    ) -> Result<String, String> {
        if !self.by_id.contains_key(entry_id) {
            return Err(format!("Entry not found: {}", entry_id));
        }

        let cwd = target_cwd.unwrap_or_else(|| self.cwd.as_str());
        let timestamp = Utc::now().to_rfc3339();
        let file_timestamp = timestamp.replace([':', '.'], "-");
        let new_id = create_session_id();
        let new_file = self
            .session_dir
            .join(format!("{}_{}.jsonl", file_timestamp, new_id));

        // Build the path from root to the specified entry
        let mut path: Vec<&SessionEntry> = Vec::new();
        let mut current = self.by_id.get(entry_id);
        while let Some(entry) = current {
            path.push(entry);
            current = entry.parent_id().and_then(|pid| self.by_id.get(pid));
        }
        path.reverse();

        let new_header = SessionHeader {
            entry_type: "session".to_string(),
            version: Some(CURRENT_SESSION_VERSION),
            id: new_id,
            timestamp: timestamp.clone(),
            cwd: cwd.to_string(),
            parent_session: self
                .session_file
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
        };

        // Write the new session file
        if let Some(parent) = new_file.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let mut f = fs::File::create(&new_file).map_err(|e| e.to_string())?;
        writeln!(f, "{}", serde_json::to_string(&new_header).unwrap_or_default())
            .map_err(|e| e.to_string())?;

        for entry in &path {
            writeln!(f, "{}", serde_json::to_string(entry).unwrap_or_default())
                .map_err(|e| e.to_string())?;
        }

        Ok(new_file.to_string_lossy().to_string())
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
            FileEntry::RawJson(v) => v.get("id")
                .and_then(|id| id.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default(),
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

/// Progress callback for session list loading.
pub type SessionListProgressCallback = Arc<dyn Fn(usize, usize) + Send + Sync>;

/// List sessions from a directory with concurrent loading and progress reporting.
/// Uses a semaphore to limit concurrency (default 10).
pub async fn list_sessions_concurrent(
    dir: &Path,
    progress: Option<SessionListProgressCallback>,
    concurrency: Option<usize>,
) -> Vec<SessionInfo> {
    if !dir.exists() {
        return Vec::new();
    }

    let max_concurrency = concurrency.unwrap_or(10);
    let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrency));

    let mut paths: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                paths.push(path);
            }
        }
    }

    let total = paths.len();
    let completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut handles = Vec::new();

    for path in paths {
        let sem = Arc::clone(&semaphore);
        let comp = Arc::clone(&completed);
        let prog = progress.clone();

        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await;
            let info = tokio::task::spawn_blocking(move || {
                build_session_info(&path)
            }).await.ok().flatten();
            let done = comp.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if let Some(ref cb) = prog {
                cb(done, total);
            }
            info
        }));
    }

    let mut sessions: Vec<SessionInfo> = Vec::new();
    for handle in handles {
        if let Some(info) = handle.await.unwrap_or(None) {
            sessions.push(info);
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

    // ============================================================
    // Branch operations
    // ============================================================

    #[test]
    fn test_branch_creates_branch_at_leaf() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);
        let id1 = mgr.append_message(serde_json::json!({"role": "user", "content": "hello"}));
        let id2 = mgr.append_message(serde_json::json!({"role": "assistant", "content": "hi"}));

        // Branch from the first message
        let branch_point = mgr.branch(Some(&id1));
        assert_eq!(branch_point, Some(id1.clone()));
        assert_eq!(mgr.get_leaf_id(), Some(id1.as_str()));
    }

    #[test]
    fn test_branch_from_nonexistent_entry() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);
        let result = mgr.branch(Some("nonexistent"));
        assert!(result.is_none());
    }

    #[test]
    fn test_branch_without_from_id_uses_leaf() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);
        let id1 = mgr.append_message(serde_json::json!({"role": "user", "content": "hello"}));
        let id2 = mgr.append_message(serde_json::json!({"role": "assistant", "content": "hi"}));

        // Branch from current leaf (id2)
        let branch_point = mgr.branch(None);
        assert_eq!(branch_point, Some(id2.clone()));
        assert_eq!(mgr.get_leaf_id(), Some(id2.as_str()));
    }

    #[test]
    fn test_reset_leaf_to_valid_entry() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);
        let id1 = mgr.append_message(serde_json::json!({"role": "user", "content": "first"}));
        let id2 = mgr.append_message(serde_json::json!({"role": "assistant", "content": "second"}));

        assert!(mgr.reset_leaf(&id1));
        assert_eq!(mgr.get_leaf_id(), Some(id1.as_str()));
    }

    #[test]
    fn test_reset_leaf_to_invalid_entry() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);
        assert!(!mgr.reset_leaf("nonexistent"));
    }

    #[test]
    fn test_branch_with_summary_creates_summary_and_resets_leaf() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);
        let id1 = mgr.append_message(serde_json::json!({"role": "user", "content": "hello"}));
        let _id2 = mgr.append_message(serde_json::json!({"role": "assistant", "content": "world"}));

        // Branch with summary from id1
        let summary_id = mgr.branch_with_summary(&id1, "Test summary", None);
        assert!(summary_id.is_some());

        // Leaf should be reset to id1
        assert_eq!(mgr.get_leaf_id(), Some(id1.as_str()));

        // The summary entry should exist
        let summary_entry = mgr.get_entry(&summary_id.unwrap());
        assert!(summary_entry.is_some());
        if let Some(SessionEntry::BranchSummary { summary, from_id, .. }) = summary_entry {
            assert_eq!(summary, "Test summary");
            assert_eq!(from_id, &id1);
        } else {
            panic!("Expected BranchSummary entry");
        }
    }

    #[test]
    fn test_branch_with_summary_invalid_from_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);
        let result = mgr.branch_with_summary("nonexistent", "summary", None);
        assert!(result.is_none());
    }

    #[test]
    fn test_create_branched_session_creates_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, true, None);
        let id1 = mgr.append_message(serde_json::json!({"role": "user", "content": "hello"}));
        let _id2 = mgr.append_message(serde_json::json!({"role": "assistant", "content": "world"}));

        let result = mgr.create_branched_session(&id1, None);
        assert!(result.is_ok());

        let new_path = result.unwrap();
        assert!(std::path::Path::new(&new_path).exists());

        // Clean up
        std::fs::remove_file(&new_path).ok();
    }

    #[test]
    fn test_create_branched_session_invalid_entry() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = SessionManager::new("/tmp/test", dir.path().to_str().unwrap(), None, false, None);
        let result = mgr.create_branched_session("nonexistent", None);
        assert!(result.is_err());
    }

    // ============================================================
    // Version migration
    // ============================================================

    #[test]
    fn test_is_valid_session_file_valid() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.jsonl");
        let header = SessionHeader {
            entry_type: "session".to_string(),
            version: Some(3),
            id: "test-id".to_string(),
            timestamp: Utc::now().to_rfc3339(),
            cwd: "/tmp".to_string(),
            parent_session: None,
        };
        let json = serde_json::to_string(&header).unwrap();
        std::fs::write(&file_path, &json).unwrap();
        assert!(is_valid_session_file(&file_path));
    }

    #[test]
    fn test_is_valid_session_file_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("invalid.jsonl");
        std::fs::write(&file_path, "not json content").unwrap();
        assert!(!is_valid_session_file(&file_path));
    }

    #[test]
    fn test_is_valid_session_file_nonexistent() {
        assert!(!is_valid_session_file(Path::new("/nonexistent/file.jsonl")));
    }

    #[test]
    fn test_migrate_v1_to_v2_adds_ids() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("v1_session.jsonl");

        // Create a v1 session file (no version field, minimal entries)
        let v1_header = serde_json::json!({
            "type": "session",
            "id": "v1-session",
            "timestamp": "2024-01-01T00:00:00Z",
            "cwd": "/tmp"
        });
        let v1_message = serde_json::json!({
            "type": "message",
            "message": {"role": "user", "content": "hello"}
        });

        let mut f = fs::File::create(&file_path).unwrap();
        writeln!(f, "{}", serde_json::to_string(&v1_header).unwrap()).unwrap();
        writeln!(f, "{}", serde_json::to_string(&v1_message).unwrap()).unwrap();
        drop(f);

        let result = migrate_session_file(&file_path);
        assert!(result.is_ok());
        let entries = result.unwrap();
        assert_eq!(entries.len(), 2);

        // Header should now have version = 3
        if let FileEntry::Header(h) = &entries[0] {
            assert_eq!(h.version, Some(3));
        } else {
            panic!("Expected header");
        }

        // Entry should have id (parent_id may be None for the first entry)
        if let FileEntry::Entry(e) = &entries[1] {
            assert!(!e.id().is_empty());
        } else {
            panic!("Expected entry");
        }

        // Clean up
        std::fs::remove_file(&file_path).ok();
    }

    #[test]
    fn test_migrate_v2_to_v3_adds_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("v2_session.jsonl");

        // Create a v2 session file
        let v2_header = serde_json::json!({
            "type": "session",
            "version": 2,
            "id": "v2-session",
            "timestamp": "2024-01-01T00:00:00Z",
            "cwd": "/tmp"
        });
        let v2_message = serde_json::json!({
            "type": "message",
            "id": "msg-1",
            "parent_id": null,
            "message": {"role": "user", "content": "hello"}
        });

        let mut f = fs::File::create(&file_path).unwrap();
        writeln!(f, "{}", serde_json::to_string(&v2_header).unwrap()).unwrap();
        writeln!(f, "{}", serde_json::to_string(&v2_message).unwrap()).unwrap();
        drop(f);

        let result = migrate_session_file(&file_path);
        assert!(result.is_ok());
        let entries = result.unwrap();
        assert_eq!(entries.len(), 2);

        // Header should now have version = 3
        if let FileEntry::Header(h) = &entries[0] {
            assert_eq!(h.version, Some(3));
        } else {
            panic!("Expected header");
        }

        // Clean up
        std::fs::remove_file(&file_path).ok();
    }

    #[test]
    fn test_migrate_already_current_version() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("v3_session.jsonl");

        let v3_header = serde_json::json!({
            "type": "session",
            "version": 3,
            "id": "v3-session",
            "timestamp": "2024-01-01T00:00:00Z",
            "cwd": "/tmp"
        });

        let mut f = fs::File::create(&file_path).unwrap();
        writeln!(f, "{}", serde_json::to_string(&v3_header).unwrap()).unwrap();
        drop(f);

        let result = migrate_session_file(&file_path);
        assert!(result.is_ok());
        let entries = result.unwrap();
        assert_eq!(entries.len(), 1);

        // Version should still be 3
        if let FileEntry::Header(h) = &entries[0] {
            assert_eq!(h.version, Some(3));
        } else {
            panic!("Expected header");
        }

        // Clean up
        std::fs::remove_file(&file_path).ok();
    }
}
