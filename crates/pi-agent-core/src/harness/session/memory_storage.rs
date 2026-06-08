use async_trait::async_trait;
use std::collections::HashMap;

use crate::harness::types::{
    SessionCreateOptions, SessionError, SessionMetadata, SessionRepo, SessionStorage,
    SessionTreeEntry,
};

pub struct InMemorySessionStorage {
    metadata: SessionMetadata,
    entries: Vec<SessionTreeEntry>,
    by_id: HashMap<String, usize>,
    labels_by_id: HashMap<String, String>,
    leaf_id: Option<String>,
}

impl InMemorySessionStorage {
    pub fn new(options: Option<InMemorySessionStorageOptions>) -> Self {
        let opts = options.unwrap_or(InMemorySessionStorageOptions::default());
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
        let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
        if !by_id.contains_key(&id) {
            return id;
        }
    }
    uuid::Uuid::new_v4().to_string()
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
                return Err(SessionError::NotFound(format!("Entry {} not found", id)));
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

pub struct InMemorySessionRepo {
    sessions: HashMap<String, crate::harness::types::Session>,
}

impl InMemorySessionRepo {
    pub fn new() -> Self {
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
