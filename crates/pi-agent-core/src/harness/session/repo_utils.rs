use crate::harness::types::{Session, SessionError, SessionStorage, SessionTreeEntry};

pub fn create_session_id() -> String {
    uuid::Uuid::now_v7().to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_session_id_format() {
        let id = create_session_id();
        let parts: Vec<&str> = id.split('-').collect();
        assert!(parts.len() >= 4, "UUID v7 should have at least 4 parts, got: {}", id);
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