use std::fmt;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct SessionCwdIssue {
    pub session_file: Option<String>,
    pub session_cwd: String,
    pub fallback_cwd: String,
}

impl fmt::Display for SessionCwdIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", format_missing_session_cwd_error(self))
    }
}

#[derive(Debug)]
pub struct MissingSessionCwdError {
    pub issue: SessionCwdIssue,
}

impl fmt::Display for MissingSessionCwdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", format_missing_session_cwd_error(&self.issue))
    }
}

impl std::error::Error for MissingSessionCwdError {}

pub trait SessionCwdSource {
    fn get_cwd(&self) -> &str;
    fn get_session_file(&self) -> Option<&str>;
}

pub fn get_missing_session_cwd_issue(
    session_manager: &impl SessionCwdSource,
    fallback_cwd: &str,
) -> Option<SessionCwdIssue> {
    let session_file = session_manager.get_session_file()?;
    let session_cwd = session_manager.get_cwd();
    if session_cwd.is_empty() || Path::new(session_cwd).exists() {
        return None;
    }
    Some(SessionCwdIssue {
        session_file: Some(session_file.to_string()),
        session_cwd: session_cwd.to_string(),
        fallback_cwd: fallback_cwd.to_string(),
    })
}

pub fn format_missing_session_cwd_error(issue: &SessionCwdIssue) -> String {
    let session_file = issue
        .session_file
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|f| format!("\nSession file: {}", f))
        .unwrap_or_default();
    format!(
        "Stored session working directory does not exist: {}{}\nCurrent working directory: {}",
        issue.session_cwd, session_file, issue.fallback_cwd
    )
}

pub fn format_missing_session_cwd_prompt(issue: &SessionCwdIssue) -> String {
    format!(
        "cwd from session file does not exist\n{}\n\ncontinue in current cwd\n{}",
        issue.session_cwd, issue.fallback_cwd
    )
}

pub fn assert_session_cwd_exists(
    session_manager: &impl SessionCwdSource,
    fallback_cwd: &str,
) -> Result<(), MissingSessionCwdError> {
    if let Some(issue) = get_missing_session_cwd_issue(session_manager, fallback_cwd) {
        return Err(MissingSessionCwdError { issue });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockSessionCwdSource {
        cwd: String,
        session_file: Option<String>,
    }

    impl SessionCwdSource for MockSessionCwdSource {
        fn get_cwd(&self) -> &str {
            &self.cwd
        }
        fn get_session_file(&self) -> Option<&str> {
            self.session_file.as_deref()
        }
    }

    #[test]
    fn test_no_issue_when_no_session_file() {
        let source = MockSessionCwdSource {
            cwd: "/tmp".into(),
            session_file: None,
        };
        assert!(get_missing_session_cwd_issue(&source, "/cwd").is_none());
    }

    #[test]
    fn test_issue_when_session_cwd_missing() {
        let source = MockSessionCwdSource {
            cwd: "/nonexistent_path_12345".into(),
            session_file: Some("/tmp/session.json".into()),
        };
        let issue = get_missing_session_cwd_issue(&source, "/fallback");
        assert!(issue.is_some());
        assert_eq!(issue.unwrap().session_cwd, "/nonexistent_path_12345");
    }

    #[test]
    fn test_no_issue_when_session_cwd_exists() {
        let source = MockSessionCwdSource {
            cwd: "/tmp".into(),
            session_file: Some("/tmp/session.json".into()),
        };
        assert!(get_missing_session_cwd_issue(&source, "/cwd").is_none());
    }

    #[test]
    fn test_assert_throws_on_missing() {
        let source = MockSessionCwdSource {
            cwd: "/nonexistent_path_67890".into(),
            session_file: Some("/tmp/session.json".into()),
        };
        let result = assert_session_cwd_exists(&source, "/fallback");
        assert!(result.is_err());
    }

    #[test]
    fn test_assert_ok_when_exists() {
        let source = MockSessionCwdSource {
            cwd: "/tmp".into(),
            session_file: Some("/tmp/session.json".into()),
        };
        assert!(assert_session_cwd_exists(&source, "/fallback").is_ok());
    }
}
