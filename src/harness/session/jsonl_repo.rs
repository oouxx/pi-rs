use async_trait::async_trait;

use crate::harness::session::jsonl_storage::JsonlSessionStorage;
use crate::harness::session::repo_utils::{create_session_id, create_timestamp, get_entries_to_fork, to_session};
use crate::harness::types::{ForkOptions, Session, SessionCreateOptions, SessionError, SessionMetadata, SessionRepo, SessionStorage};

fn encode_cwd(cwd: &str) -> String {
    let trimmed = cwd
        .trim_start_matches(|c| c == '/' || c == '\\')
        .replace(|c: char| c == '/' || c == '\\' || c == ':', "-");
    format!("--{}--", trimmed)
}

pub struct JsonlSessionRepo {
    sessions_root: String,
}

impl JsonlSessionRepo {
    pub fn new(sessions_root: impl Into<String>) -> Self {
        Self {
            sessions_root: sessions_root.into(),
        }
    }

    fn session_dir(&self, cwd: &str) -> String {
        std::path::Path::new(&self.sessions_root)
            .join(encode_cwd(cwd))
            .to_string_lossy()
            .to_string()
    }

    fn create_session_file_path(&self, cwd: &str, session_id: &str, timestamp: &str) -> String {
        let file_name = format!("{}_{}.jsonl", timestamp.replace([':', '.'], "-"), session_id);
        std::path::Path::new(&self.session_dir(cwd))
            .join(&file_name)
            .to_string_lossy()
            .to_string()
    }
}

#[async_trait]
impl SessionRepo<SessionMetadata> for JsonlSessionRepo {
    async fn create(&mut self, options: SessionCreateOptions) -> std::result::Result<Session<SessionMetadata>, SessionError> {
        let id = options.id.unwrap_or_else(create_session_id);
        let created_at = create_timestamp();
        let session_dir = self.session_dir(&options.cwd);

        tokio::fs::create_dir_all(&session_dir)
            .await
            .map_err(|e| SessionError::Storage(format!("Failed to create session directory: {}", e)))?;

        let file_path = self.create_session_file_path(&options.cwd, &id, &created_at);
        let storage = JsonlSessionStorage::create(
            file_path,
            &options.cwd,
            &id,
            options.parent_session_path.as_deref(),
        )
        .await?;

        Ok(to_session(Box::new(storage)))
    }

    async fn open(&self, metadata: &SessionMetadata) -> std::result::Result<Session<SessionMetadata>, SessionError> {
        let path = metadata
            .cwd
            .as_ref()
            .ok_or_else(|| SessionError::NotFound("Session path not found in metadata".to_string()))?;

        if !tokio::fs::try_exists(path).await.unwrap_or(false) {
            return Err(SessionError::NotFound(format!("Session not found: {}", path)));
        }

        let storage = JsonlSessionStorage::open(path).await?;
        Ok(to_session(Box::new(storage)))
    }

    async fn list(&self) -> std::result::Result<Vec<SessionMetadata>, SessionError> {
        let sessions_root = &self.sessions_root;
        if !tokio::fs::try_exists(sessions_root).await.unwrap_or(false) {
            return Ok(Vec::new());
        }

        let mut entries = tokio::fs::read_dir(sessions_root)
            .await
            .map_err(|e| SessionError::Storage(format!("Failed to read sessions root: {}", e)))?;

        let mut sessions = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| SessionError::Storage(format!("Failed to read directory entry: {}", e)))?
        {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let mut dir_entries = tokio::fs::read_dir(&path)
                .await
                .map_err(|e| SessionError::Storage(format!("Failed to read session dir: {}", e)))?;

            while let Some(file_entry) = dir_entries
                .next_entry()
                .await
                .map_err(|e| SessionError::Storage(format!("Failed to read dir entry: {}", e)))?
            {
                let file_path = file_entry.path();
                if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }

                match JsonlSessionStorage::open(file_path.to_string_lossy().into_owned()).await {
                    Ok(storage) => {
                        let meta = SessionStorage::get_metadata(&storage).await;
                        sessions.push(meta);
                    }
                    Err(_) => continue,
                }
            }
        }

        sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(sessions)
    }

    async fn delete(&mut self, metadata: &SessionMetadata) -> std::result::Result<(), SessionError> {
        let path = metadata
            .cwd
            .as_ref()
            .ok_or_else(|| SessionError::NotFound("Session path not found".to_string()))?;

        tokio::fs::remove_file(path)
            .await
            .map_err(|e| SessionError::Storage(format!("Failed to delete session: {}", e)))?;

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
        let created_at = create_timestamp();
        let session_dir = self.session_dir(&options.cwd);

        tokio::fs::create_dir_all(&session_dir)
            .await
            .map_err(|e| SessionError::Storage(format!("Failed to create session directory: {}", e)))?;

        let file_path = self.create_session_file_path(&options.cwd, &id, &created_at);
        let parent_path = options
            .parent_session_path
            .or_else(|| source_metadata.cwd.clone());

        let mut storage = JsonlSessionStorage::create(
            file_path,
            &options.cwd,
            &id,
            parent_path.as_deref(),
        )
        .await?;

        for entry in &forked_entries {
            SessionStorage::append_entry(&mut storage, entry.clone()).await?;
        }

        Ok(to_session(Box::new(storage)))
    }
}