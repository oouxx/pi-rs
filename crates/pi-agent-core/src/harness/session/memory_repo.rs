use std::collections::HashMap;

use async_trait::async_trait;

use crate::harness::session::memory_storage::{InMemorySessionStorage, InMemorySessionStorageOptions};
use crate::harness::session::repo_utils::{create_session_id, create_timestamp, get_entries_to_fork, to_session};
use crate::harness::types::{ForkOptions, Session, SessionCreateOptions, SessionError, SessionMetadata, SessionRepo};

pub struct InMemorySessionRepo {
    sessions: HashMap<String, Session<SessionMetadata>>,
}

impl InMemorySessionRepo {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }
}

impl Default for InMemorySessionRepo {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SessionRepo<SessionMetadata> for InMemorySessionRepo {
    async fn create(&mut self, options: SessionCreateOptions) -> std::result::Result<Session<SessionMetadata>, SessionError> {
        let id = options.id.unwrap_or_else(create_session_id);
        let metadata = SessionMetadata {
            id: id.clone(),
            created_at: create_timestamp(),
            cwd: Some(options.cwd),
            parent_session: options.parent_session_path,
        };
        let storage = Box::new(InMemorySessionStorage::new(Some(InMemorySessionStorageOptions {
            entries: Vec::new(),
            metadata: Some(metadata),
        })));
        let session = to_session(storage);
        self.sessions.insert(id, session.clone());
        Ok(session)
    }

    async fn open(&self, metadata: &SessionMetadata) -> std::result::Result<Session<SessionMetadata>, SessionError> {
        self.sessions
            .get(&metadata.id)
            .cloned()
            .ok_or_else(|| SessionError::NotFound(format!("Session not found: {}", metadata.id)))
    }

    async fn list(&self) -> std::result::Result<Vec<SessionMetadata>, SessionError> {
        let mut metas = Vec::new();
        for session in self.sessions.values() {
            metas.push(session.get_metadata().await);
        }
        Ok(metas)
    }

    async fn delete(&mut self, metadata: &SessionMetadata) -> std::result::Result<(), SessionError> {
        self.sessions.remove(&metadata.id);
        Ok(())
    }

    async fn fork(
        &mut self,
        source_metadata: &SessionMetadata,
        options: ForkOptions,
    ) -> std::result::Result<Session<SessionMetadata>, SessionError> {
        let source = self.open(source_metadata).await?;
        let storage = source.get_storage().await;
        let forked_entries = get_entries_to_fork(storage.as_ref(), &options).await?;
        drop(storage);

        let id = options.id.unwrap_or_else(create_session_id);
        let metadata = SessionMetadata {
            id: id.clone(),
            created_at: create_timestamp(),
            cwd: Some(options.cwd),
            parent_session: options.parent_session_path,
        };
        let storage = Box::new(InMemorySessionStorage::new(Some(InMemorySessionStorageOptions {
            entries: forked_entries,
            metadata: Some(metadata),
        })));
        let session = to_session(storage);
        self.sessions.insert(id, session.clone());
        Ok(session)
    }
}