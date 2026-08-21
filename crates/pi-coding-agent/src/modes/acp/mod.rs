//! ACP (Agent Client Protocol) mode — lets ACP clients (Zed, JetBrains,
//! ACP VSCode extensions, …) drive pi-coding-agent directly over stdio.
//!
//! Run with `pi-rs --acp` and register in the editor, e.g. in Zed:
//!
//! ```json
//! "agent_servers": {
//!   "pi-rs": { "type": "custom", "command": "pi-rs", "args": ["--acp"], "env": {} }
//! }
//! ```

pub mod agent;
pub mod session;
pub mod slash_commands;
pub mod translate;

use std::sync::Arc;

use agent_client_protocol as acp;
use agent_client_protocol::Client as _;
use futures::future::LocalBoxFuture;
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use self::agent::PiAcpAgent;

/// Run the ACP server: speak ACP JSON-RPC over stdin/stdout until the
/// client disconnects.
///
/// `enable_extensions` 对应 CLI `--no-extensions`：为 true 时新会话注册内置
/// Rust 扩展（goal/subagent/web_search）。
pub async fn run_acp_mode(enable_extensions: bool) -> i32 {
    let outgoing = tokio::io::stdout().compat_write();
    let incoming = tokio::io::stdin().compat();

    let local_set = tokio::task::LocalSet::new();
    let result = local_set
        .run_until(async move {
            let (notif_tx, mut notif_rx) =
                mpsc::unbounded_channel::<(acp::SessionNotification, oneshot::Sender<()>)>();

            let spawn: Arc<dyn Fn(LocalBoxFuture<'static, ()>)> =
                Arc::new(|fut| {
                    tokio::task::spawn_local(fut);
                });
            let spawn_for_agent = spawn.clone();

            let agent = PiAcpAgent::new(notif_tx, spawn_for_agent)
                .with_extensions(enable_extensions);
            let (conn, handle_io) = acp::AgentSideConnection::new(
                agent,
                outgoing,
                incoming,
                move |fut| {
                    spawn(fut);
                },
            );

            // Forward session notifications from session tasks to the client.
            tokio::task::spawn_local(async move {
                while let Some((notification, ack)) = notif_rx.recv().await {
                    let _result = conn.session_notification(notification).await;
                    let _ = ack.send(());
                }
            });

            let io_result = handle_io.await;
            // Kill any background processes left running by bash commands
            // (matching TS interactive-mode shutdown / signal handlers which
            // call `killTrackedDetachedChildren()`).
            crate::utils::shell::kill_tracked_detached_children();
            io_result
        })
        .await;

    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("[pi] ACP connection error: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use agent_client_protocol::Agent as _;
    use tokio::io::duplex;
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

    /// Minimal ACP client for tests — only receives session notifications.
    struct TestClient;

    #[async_trait::async_trait(?Send)]
    impl acp::Client for TestClient {
        async fn request_permission(
            &self,
            _args: acp::RequestPermissionRequest,
        ) -> acp::Result<acp::RequestPermissionResponse> {
            Err(acp::Error::method_not_found())
        }
        async fn session_notification(&self, _args: acp::SessionNotification) -> acp::Result<()> {
            Ok(())
        }
    }

    /// A client that records every session notification it receives.
    #[derive(Default)]
    struct RecordingClient {
        notifications: std::sync::Arc<std::sync::Mutex<Vec<acp::SessionNotification>>>,
    }

    #[async_trait::async_trait(?Send)]
    impl acp::Client for RecordingClient {
        async fn request_permission(
            &self,
            _args: acp::RequestPermissionRequest,
        ) -> acp::Result<acp::RequestPermissionResponse> {
            Err(acp::Error::method_not_found())
        }
        async fn session_notification(
            &self,
            args: acp::SessionNotification,
        ) -> acp::Result<()> {
            self.notifications.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(args);
            Ok(())
        }
    }

    /// Wire a `PiAcpAgent` to a client-side connection over in-memory pipes.
    /// Returns the client-side connection (implements `Agent`) for the test
    /// to drive, and keeps the agent-side connection (implements `Client`)
    /// inside a task that forwards session notifications.
    fn wire(
        spawn: Arc<dyn Fn(LocalBoxFuture<'static, ()>)>,
    ) -> acp::ClientSideConnection {
        wire_with(spawn, TestClient)
    }

    /// Like `wire`, but with a custom client (e.g. one that records
    /// notifications).
    fn wire_with<C: acp::Client + 'static>(
        spawn: Arc<dyn Fn(LocalBoxFuture<'static, ()>)>,
        client: C,
    ) -> acp::ClientSideConnection {
        let (notif_tx, mut notif_rx) =
            mpsc::unbounded_channel::<(acp::SessionNotification, oneshot::Sender<()>)>();
        // Isolated storage so tests never write to the real agent dir.
        let base_dir = tempfile::tempdir().expect("tempdir").keep();
        let agent = PiAcpAgent::with_base_dir(notif_tx, spawn, base_dir);
        // client -> agent and agent -> client pipes
        let (client_tx, agent_rx) = duplex(64 * 1024);
        let (agent_tx, client_rx) = duplex(64 * 1024);

        let (client_conn, client_io) = acp::ClientSideConnection::new(
            client,
            client_tx.compat_write(),
            client_rx.compat(),
            |fut| {
                tokio::task::spawn_local(fut);
            },
        );
        let (agent_conn, agent_io) = acp::AgentSideConnection::new(
            agent,
            agent_tx.compat_write(),
            agent_rx.compat(),
            |fut| {
                tokio::task::spawn_local(fut);
            },
        );
        tokio::task::spawn_local(client_io);
        tokio::task::spawn_local(agent_io);

        // Forward notifications from the agent to the client connection.
        tokio::task::spawn_local(async move {
            while let Some((notification, ack)) = notif_rx.recv().await {
                let _ = agent_conn.session_notification(notification).await;
                let _ = ack.send(());
            }
        });

        client_conn
    }

    #[tokio::test]
    async fn initialize_new_session_list_load_cancel() {
        let local_set = tokio::task::LocalSet::new();
        local_set
            .run_until(async {
                let spawn: Arc<dyn Fn(LocalBoxFuture<'static, ()>)> =
                    Arc::new(|fut| {
                        tokio::task::spawn_local(fut);
                    });
                let client_conn = wire(spawn);

                // initialize
                let resp = client_conn
                    .initialize(
                        acp::InitializeRequest::new(acp::ProtocolVersion::LATEST)
                            .client_info(acp::Implementation::new("test-client", "0.0.0")),
                    )
                    .await
                    .expect("initialize");
                assert_eq!(resp.protocol_version, acp::ProtocolVersion::LATEST);
                assert!(resp.agent_capabilities.load_session);
                let info = resp.agent_info.expect("agent info");
                assert_eq!(info.name, "pi-coding-agent");

                // new_session
                let resp = client_conn
                    .new_session(acp::NewSessionRequest::new("/tmp"))
                    .await
                    .expect("new_session");
                let sid = resp.session_id;

                // list_sessions
                let resp = client_conn
                    .list_sessions(acp::ListSessionsRequest::new())
                    .await
                    .expect("list_sessions");
                assert_eq!(resp.sessions.len(), 1);
                assert_eq!(resp.sessions[0].session_id, sid);
                assert_eq!(resp.sessions[0].cwd, std::path::PathBuf::from("/tmp"));

                // load_session (in-memory)
                client_conn
                    .load_session(acp::LoadSessionRequest::new(sid.clone(), "/tmp"))
                    .await
                    .expect("load_session");

                // cancel on an idle session is a no-op
                client_conn
                    .cancel(acp::CancelNotification::new(sid))
                    .await
                    .expect("cancel");

                // unknown session -> error
                let err = client_conn
                    .prompt(acp::PromptRequest::new(
                        acp::SessionId::new("nope"),
                        vec![acp::ContentBlock::Text(acp::TextContent::new("hi"))],
                    ))
                    .await
                    .expect_err("prompt on unknown session must fail");
                assert_eq!(err.code, acp::ErrorCode::InvalidParams);
            })
            .await;
    }

    /// `session/new` must advertise slash commands via
    /// `available_commands_update` (file-based + built-in), and
    /// `session/close` must remove the session.
    #[tokio::test]
    async fn new_session_advertises_commands_and_close_removes() {
        let local_set = tokio::task::LocalSet::new();
        local_set
            .run_until(async {
                let spawn: Arc<dyn Fn(LocalBoxFuture<'static, ()>)> =
                    Arc::new(|fut| {
                        tokio::task::spawn_local(fut);
                    });
                let client = RecordingClient::default();
                let notifications = client.notifications.clone();
                let client_conn = wire_with(spawn, client);

                let resp = client_conn
                    .new_session(acp::NewSessionRequest::new("/tmp"))
                    .await
                    .expect("new_session");
                let sid = resp.session_id;

                // The available_commands_update notification is emitted after
                // a short delay; wait for it.
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                let commands = loop {
                    let found = {
                        let notifs = notifications
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        notifs.iter().find_map(|n| match &n.update {
                            acp::SessionUpdate::AvailableCommandsUpdate(u) => {
                                Some(u.available_commands.clone())
                            }
                            _ => None,
                        })
                    };
                    if let Some(cmds) = found {
                        break cmds;
                    }
                    if std::time::Instant::now() > deadline {
                        panic!("available_commands_update not received");
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                };
                let names: Vec<&str> = commands.iter().map(|c| c.name.as_str()).collect();
                assert!(
                    names.contains(&"compact"),
                    "built-in /compact must be advertised: {names:?}"
                );
                assert!(
                    names.contains(&"session"),
                    "built-in /session must be advertised: {names:?}"
                );

                // session/close removes the session.
                client_conn
                    .close_session(acp::CloseSessionRequest::new(sid.clone()))
                    .await
                    .expect("close_session");
                let err = client_conn
                    .prompt(acp::PromptRequest::new(
                        sid,
                        vec![acp::ContentBlock::Text(acp::TextContent::new("hi"))],
                    ))
                    .await
                    .expect_err("prompt on closed session must fail");
                assert_eq!(err.code, acp::ErrorCode::InvalidParams);
            })
            .await;
    }

    /// `session/new` must return the initial config options (model +
    /// thought_level selectors) so the client can render dropdowns immediately
    /// (matching pi-acp's `newSession` response).
    #[tokio::test]
    async fn new_session_returns_config_options() {
        let local_set = tokio::task::LocalSet::new();
        local_set
            .run_until(async {
                let spawn: Arc<dyn Fn(LocalBoxFuture<'static, ()>)> =
                    Arc::new(|fut| {
                        tokio::task::spawn_local(fut);
                    });
                let client_conn = wire(spawn);

                let resp = client_conn
                    .new_session(acp::NewSessionRequest::new("/tmp"))
                    .await
                    .expect("new_session");
                let options = resp.config_options.expect("config_options");
                let ids: Vec<&str> = options.iter().map(|o| o.id.0.as_ref()).collect();
                assert!(
                    ids.contains(&"model"),
                    "model selector must be advertised: {ids:?}"
                );
                assert!(
                    ids.contains(&"thought_level"),
                    "thought_level selector must be advertised (pi-acp config id): {ids:?}"
                );

                // The model dropdown labels must include the provider prefix
                // (matching pi-acp's `name: \`${provider}/${name}\``) so the
                // client shows which provider each model belongs to.
                let model_option = options
                    .iter()
                    .find(|o| o.id.0.as_ref() == "model")
                    .expect("model option");
                if let acp::SessionConfigKind::Select(sel) = &model_option.kind {
                    let opts = match &sel.options {
                        acp::SessionConfigSelectOptions::Ungrouped(opts) => opts,
                        _ => panic!("model selector must be ungrouped"),
                    };
                    assert!(
                        !opts.is_empty(),
                        "model selector must list models"
                    );
                    for opt in opts {
                        assert!(
                            opt.name.contains('/'),
                            "model option label must include provider prefix, got {:?}",
                            opt.name
                        );
                    }
                } else {
                    panic!("model option must be a select");
                }
            })
            .await;
    }

    /// `session/new` with a relative cwd must be rejected (matching pi-acp's
    /// validation).
    #[tokio::test]
    async fn new_session_rejects_relative_cwd() {
        let local_set = tokio::task::LocalSet::new();
        local_set
            .run_until(async {
                let spawn: Arc<dyn Fn(LocalBoxFuture<'static, ()>)> =
                    Arc::new(|fut| {
                        tokio::task::spawn_local(fut);
                    });
                let client_conn = wire(spawn);

                let err = client_conn
                    .new_session(acp::NewSessionRequest::new("relative/path"))
                    .await
                    .expect_err("relative cwd must be rejected");
                assert_eq!(err.code, acp::ErrorCode::InvalidParams);
            })
            .await;
    }

    /// `session/list` must filter by cwd (defaulting to the last session cwd,
    /// matching pi-acp) and include title/updatedAt.
    #[tokio::test]
    async fn list_sessions_filters_by_cwd_and_includes_metadata() {
        let local_set = tokio::task::LocalSet::new();
        local_set
            .run_until(async {
                let spawn: Arc<dyn Fn(LocalBoxFuture<'static, ()>)> =
                    Arc::new(|fut| {
                        tokio::task::spawn_local(fut);
                    });
                let client_conn = wire(spawn);

                let resp = client_conn
                    .new_session(acp::NewSessionRequest::new("/tmp"))
                    .await
                    .expect("new_session");
                let sid = resp.session_id;

                // No cwd → defaults to the last session cwd (/tmp).
                let resp = client_conn
                    .list_sessions(acp::ListSessionsRequest::new())
                    .await
                    .expect("list_sessions");
                assert_eq!(resp.sessions.len(), 1);
                assert_eq!(resp.sessions[0].session_id, sid);
                assert_eq!(resp.sessions[0].cwd, std::path::PathBuf::from("/tmp"));

                // Explicit cwd filter for a different dir → empty.
                let resp = client_conn
                    .list_sessions(acp::ListSessionsRequest::new().cwd("/other"))
                    .await
                    .expect("list_sessions");
                assert_eq!(resp.sessions.len(), 0);
            })
            .await;
    }

    /// `session/set_mode` (thinking level) must succeed and emit a
    /// `current_mode_update` notification.
    #[tokio::test]
    async fn set_session_mode_emits_current_mode_update() {
        let local_set = tokio::task::LocalSet::new();
        local_set
            .run_until(async {
                let spawn: Arc<dyn Fn(LocalBoxFuture<'static, ()>)> =
                    Arc::new(|fut| {
                        tokio::task::spawn_local(fut);
                    });
                let client = RecordingClient::default();
                let notifications = client.notifications.clone();
                let client_conn = wire_with(spawn, client);

                let resp = client_conn
                    .new_session(acp::NewSessionRequest::new("/tmp"))
                    .await
                    .expect("new_session");
                let sid = resp.session_id;

                client_conn
                    .set_session_mode(acp::SetSessionModeRequest::new(
                        sid.clone(),
                        acp::SessionModeId::new("high"),
                    ))
                    .await
                    .expect("set_session_mode");

                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                loop {
                    let found = {
                        let notifs = notifications
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        notifs.iter().any(|n| {
                            matches!(
                                &n.update,
                                acp::SessionUpdate::CurrentModeUpdate(u)
                                    if u.current_mode_id.0.as_ref() == "high"
                            )
                        })
                    };
                    if found {
                        break;
                    }
                    if std::time::Instant::now() > deadline {
                        panic!("current_mode_update not received");
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            })
            .await;
    }
}
