use crate::harness::types::{Session, SessionError, SessionStorage, SessionTreeEntry};

pub fn create_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn create_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn to_session<M: Clone + Send + Sync + 'static>(storage: Box<dyn SessionStorage<M>>) -> Session<M> {
    Session::new(storage)
}

pub async fn get_entries_to_fork<S: SessionStorage + ?Sized>(
    storage: &S,
    options: &crate::harness::types::ForkOptions,
) -> std::result::Result<Vec<SessionTreeEntry>, SessionError> {
    if options.entry_id.is_none() {
        return Ok(storage.get_entries().await);
    }

    let entry_id = options.entry_id.as_ref().unwrap();
    let target = storage
        .get_entry(entry_id)
        .await
        .ok_or_else(|| SessionError::NotFound(format!("Entry {} not found", entry_id)))?;

    let effective_leaf_id = if options.position.as_deref() == Some("at") {
        Some(target.id().to_string())
    } else {
        match &target {
            SessionTreeEntry::Message { message, .. } => match message {
                crate::types::AgentMessage::User { .. } => target.parent_id().map(|s| s.to_string()),
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

    storage.get_path_to_root(effective_leaf_id.as_deref()).await
}