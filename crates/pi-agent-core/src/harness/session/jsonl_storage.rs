use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;

use crate::harness::types::{
    SessionCreateOptions, SessionError, SessionMetadata, SessionRepo, SessionStorage,
    SessionTreeEntry,
};

pub struct JsonlSessionStorage {
    file_path: PathBuf,
    metadata: SessionMetadata,
    entries: Vec<SessionTreeEntry>,
    by_id: HashMap<String, usize>,
    labels_by_id: HashMap<String, String>,
    leaf_id: Option<String>,
}

impl JsonlSessionStorage {
    pub async fn open(file_path: impl Into<PathBuf>) -> std::result::Result<Self, SessionError> {
        let file_path = file_path.into();
        let content = tokio::fs::read_to_string(&file_path).await.map_err(|e| {
            SessionError::Storage(format!(
                "Failed to read session {}: {}",
                file_path.display(),
                e
            ))
        })?;

        let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
        if lines.is_empty() {
            return Err(SessionError::InvalidSession(format!(
                "missing session header: {}",
                file_path.display()
            )));
        }

        let header: serde_json::Value = serde_json::from_str(lines[0])
            .map_err(|e| SessionError::InvalidSession(format!("Invalid header: {}", e)))?;

        let metadata = SessionMetadata {
            id: header
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            created_at: header
                .get("timestamp")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            cwd: header
                .get("cwd")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            parent_session: header
                .get("parentSession")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        };

        let mut entries = Vec::new();
        let mut by_id = HashMap::new();
        let mut labels_by_id = HashMap::new();
        let mut leaf_id = None;

        for (i, line) in lines.iter().enumerate().skip(1) {
            let entry: SessionTreeEntry = serde_json::from_str(line).map_err(|e| {
                SessionError::InvalidSession(format!("Invalid entry at line {}: {}", i + 1, e))
            })?;

            let id = entry.id().to_string();
            if let SessionTreeEntry::Leaf { target_id, .. } = &entry {
                leaf_id = target_id.clone();
            } else {
                leaf_id = Some(id.clone());
            }

            update_label_cache(&mut labels_by_id, &entry);
            by_id.insert(id, entries.len());
            entries.push(entry);
        }

        Ok(Self {
            file_path,
            metadata,
            entries,
            by_id,
            labels_by_id,
            leaf_id,
        })
    }

    pub async fn create(
        file_path: impl Into<PathBuf>,
        cwd: &str,
        session_id: &str,
        parent_session_path: Option<&str>,
    ) -> std::result::Result<Self, SessionError> {
        let file_path = file_path.into();

        let header = serde_json::json!({
            "type": "session",
            "version": 3,
            "id": session_id,
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "cwd": cwd,
            "parentSession": parent_session_path
        });

        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| SessionError::Storage(format!("Failed to create directory: {}", e)))?;
        }

        tokio::fs::write(&file_path, format!("{}\n", header))
            .await
            .map_err(|e| SessionError::Storage(format!("Failed to create session: {}", e)))?;

        let metadata = SessionMetadata {
            id: session_id.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            cwd: Some(cwd.to_string()),
            parent_session: parent_session_path.map(|s| s.to_string()),
        };

        Ok(Self {
            file_path,
            metadata,
            entries: Vec::new(),
            by_id: HashMap::new(),
            labels_by_id: HashMap::new(),
            leaf_id: None,
        })
    }

    async fn append_line(&self, line: &str) -> std::result::Result<(), SessionError> {
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&self.file_path)
            .await
            .map_err(|e| SessionError::Storage(format!("Failed to open session file: {}", e)))?;
        file.write_all(format!("{}\n", line).as_bytes())
            .await
            .map_err(|e| SessionError::Storage(format!("Failed to append to session: {}", e)))?;
        Ok(())
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
        let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
        if !by_id.contains_key(&id) {
            return id;
        }
    }
    uuid::Uuid::new_v4().to_string()
}

#[async_trait]
impl SessionStorage for JsonlSessionStorage {
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
                return Err(SessionError::NotFound(format!("Entry {} not found", id)));
            }
        }

        let entry = SessionTreeEntry::Leaf {
            id: generate_entry_id(&self.by_id),
            parent_id: self.leaf_id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            target_id: leaf_id.clone(),
        };

        let json = serde_json::to_string(&entry)
            .map_err(|e| SessionError::Storage(format!("Failed to serialize leaf entry: {}", e)))?;
        self.append_line(&json).await?;

        let id = entry.id().to_string();
        self.by_id.insert(id, self.entries.len());
        self.entries.push(entry);
        self.leaf_id = leaf_id;
        Ok(())
    }

    async fn create_entry_id(&self) -> String {
        generate_entry_id(&self.by_id)
    }

    async fn append_entry(
        &mut self,
        entry: SessionTreeEntry,
    ) -> std::result::Result<(), SessionError> {
        let json = serde_json::to_string(&entry)
            .map_err(|e| SessionError::Storage(format!("Failed to serialize entry: {}", e)))?;
        self.append_line(&json).await?;

        let id = entry.id().to_string();
        let new_leaf = match &entry {
            SessionTreeEntry::Leaf { target_id, .. } => target_id.clone(),
            _ => Some(id.clone()),
        };

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

    async fn get_path_to_root(
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

            match parent_id {
                Some(pid) => current_id = pid,
                None => break,
            }
        }

        path.reverse();
        Ok(path)
    }

    async fn get_entries(&self) -> Vec<SessionTreeEntry> {
        self.entries.clone()
    }
}

#[derive(Debug, Clone)]
pub struct JsonlSessionMetadata {
    pub metadata: SessionMetadata,
    pub path: PathBuf,
}

pub struct JsonlSessionRepo {
    sessions_root: PathBuf,
}

impl JsonlSessionRepo {
    pub fn new(sessions_root: impl Into<PathBuf>) -> Self {
        Self {
            sessions_root: sessions_root.into(),
        }
    }

    fn encode_cwd(cwd: &str) -> String {
        format!(
            "--{}--",
            cwd.trim_start_matches('/')
                .trim_start_matches('\\')
                .replace(|c| c == '/' || c == '\\' || c == ':', "-")
        )
    }

    fn session_dir(&self, cwd: &str) -> PathBuf {
        self.sessions_root.join(Self::encode_cwd(cwd))
    }
}

#[async_trait]
impl SessionRepo<JsonlSessionMetadata> for JsonlSessionRepo {
    async fn create(
        &mut self,
        options: SessionCreateOptions,
    ) -> std::result::Result<crate::harness::types::Session<JsonlSessionMetadata>, SessionError>
    {
        let id = options
            .id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let created_at = chrono::Utc::now().to_rfc3339().replace([':', '.'], "-");
        let session_dir = self.session_dir(&options.cwd);

        tokio::fs::create_dir_all(&session_dir).await.map_err(|e| {
            SessionError::Storage(format!("Failed to create session directory: {}", e))
        })?;

        let file_name = format!("{}_{}.jsonl", created_at, id);
        let file_path = session_dir.join(&file_name);

        let storage = JsonlSessionStorage::create(
            &file_path,
            &options.cwd,
            &id,
            options.parent_session_path.as_deref(),
        )
        .await?;

        let jsonl_metadata = JsonlSessionMetadata {
            metadata: storage.metadata.clone(),
            path: file_path,
        };

        let session_storage: Box<dyn SessionStorage<JsonlSessionMetadata>> =
            Box::new(JsonlSessionStorageAdapter {
                storage,
                jsonl_metadata: jsonl_metadata.clone(),
            });

        Ok(crate::harness::types::Session::new(session_storage))
    }

    async fn open(
        &self,
        metadata: &JsonlSessionMetadata,
    ) -> std::result::Result<crate::harness::types::Session<JsonlSessionMetadata>, SessionError>
    {
        let storage = JsonlSessionStorage::open(&metadata.path).await?;

        let jsonl_metadata = JsonlSessionMetadata {
            metadata: storage.metadata.clone(),
            path: metadata.path.clone(),
        };

        let session_storage: Box<dyn SessionStorage<JsonlSessionMetadata>> =
            Box::new(JsonlSessionStorageAdapter {
                storage,
                jsonl_metadata: jsonl_metadata.clone(),
            });

        Ok(crate::harness::types::Session::new(session_storage))
    }

    async fn list(&self) -> std::result::Result<Vec<JsonlSessionMetadata>, SessionError> {
        let mut sessions = Vec::new();

        if !self.sessions_root.exists() {
            return Ok(sessions);
        }

        let mut entries = tokio::fs::read_dir(&self.sessions_root)
            .await
            .map_err(|e| SessionError::Storage(format!("Failed to read sessions root: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| SessionError::Storage(format!("Failed to read directory entry: {}", e)))?
        {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let mut file_entries = tokio::fs::read_dir(&path)
                .await
                .map_err(|e| SessionError::Storage(format!("Failed to read session dir: {}", e)))?;

            while let Some(file_entry) = file_entries
                .next_entry()
                .await
                .map_err(|e| SessionError::Storage(format!("Failed to read file entry: {}", e)))?
            {
                let file_path = file_entry.path();
                if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }

                let content = match tokio::fs::read_to_string(&file_path).await {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                let first_line = match content.lines().next() {
                    Some(l) => l,
                    None => continue,
                };

                let header: serde_json::Value = match serde_json::from_str(first_line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                let metadata = SessionMetadata {
                    id: header
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    created_at: header
                        .get("timestamp")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    cwd: header
                        .get("cwd")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    parent_session: header
                        .get("parentSession")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                };

                sessions.push(JsonlSessionMetadata {
                    metadata,
                    path: file_path,
                });
            }
        }

        sessions.sort_by(|a, b| b.metadata.created_at.cmp(&a.metadata.created_at));
        Ok(sessions)
    }

    async fn delete(
        &mut self,
        metadata: &JsonlSessionMetadata,
    ) -> std::result::Result<(), SessionError> {
        tokio::fs::remove_file(&metadata.path)
            .await
            .map_err(|e| SessionError::Storage(format!("Failed to delete session: {}", e)))?;
        Ok(())
    }

    async fn fork(
        &mut self,
        source_metadata: &JsonlSessionMetadata,
        options: crate::harness::types::ForkOptions,
    ) -> std::result::Result<crate::harness::types::Session<JsonlSessionMetadata>, SessionError>
    {
        let source = self.open(source_metadata).await?;
        let entries = source.get_entries().await;

        let forked_entries = if let Some(entry_id) = &options.entry_id {
            let target = source.get_entry(entry_id).await.ok_or_else(|| {
                SessionError::InvalidForkTarget(format!("Entry {} not found", entry_id))
            })?;

            let effective_leaf_id = match options.position.as_deref() {
                Some("at") => Some(target.id().to_string()),
                _ => target.parent_id().map(|s| s.to_string()),
            };

            source
                .get_path_to_root(effective_leaf_id.as_deref())
                .await?
        } else {
            entries
        };

        let id = options
            .id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let created_at = chrono::Utc::now().to_rfc3339().replace([':', '.'], "-");
        let session_dir = self.session_dir(&options.cwd);

        tokio::fs::create_dir_all(&session_dir).await.map_err(|e| {
            SessionError::Storage(format!("Failed to create session directory: {}", e))
        })?;

        let file_name = format!("{}_{}.jsonl", created_at, id);
        let file_path = session_dir.join(&file_name);

        let mut storage = JsonlSessionStorage::create(
            &file_path,
            &options.cwd,
            &id,
            options
                .parent_session_path
                .as_deref()
                .or(Some(source_metadata.path.to_str().unwrap_or(""))),
        )
        .await?;

        for entry in forked_entries {
            storage.append_entry(entry).await?;
        }

        let jsonl_metadata = JsonlSessionMetadata {
            metadata: storage.metadata.clone(),
            path: file_path,
        };

        let session_storage: Box<dyn SessionStorage<JsonlSessionMetadata>> =
            Box::new(JsonlSessionStorageAdapter {
                storage,
                jsonl_metadata: jsonl_metadata.clone(),
            });

        Ok(crate::harness::types::Session::new(session_storage))
    }
}

struct JsonlSessionStorageAdapter {
    storage: JsonlSessionStorage,
    jsonl_metadata: JsonlSessionMetadata,
}

#[async_trait]
impl SessionStorage<JsonlSessionMetadata> for JsonlSessionStorageAdapter {
    async fn get_metadata(&self) -> JsonlSessionMetadata {
        self.jsonl_metadata.clone()
    }

    async fn get_leaf_id(&self) -> Option<String> {
        self.storage.get_leaf_id().await
    }

    async fn set_leaf_id(
        &mut self,
        leaf_id: Option<String>,
    ) -> std::result::Result<(), SessionError> {
        self.storage.set_leaf_id(leaf_id).await
    }

    async fn create_entry_id(&self) -> String {
        self.storage.create_entry_id().await
    }

    async fn append_entry(
        &mut self,
        entry: SessionTreeEntry,
    ) -> std::result::Result<(), SessionError> {
        self.storage.append_entry(entry).await
    }

    async fn get_entry(&self, id: &str) -> Option<SessionTreeEntry> {
        self.storage.get_entry(id).await
    }

    async fn find_entries(&self, entry_type: &str) -> Vec<SessionTreeEntry> {
        self.storage.find_entries(entry_type).await
    }

    async fn get_label(&self, id: &str) -> Option<String> {
        self.storage.get_label(id).await
    }

    async fn get_path_to_root(
        &self,
        leaf_id: Option<&str>,
    ) -> std::result::Result<Vec<SessionTreeEntry>, SessionError> {
        self.storage.get_path_to_root(leaf_id).await
    }

    async fn get_entries(&self) -> Vec<SessionTreeEntry> {
        self.storage.get_entries().await
    }
}
