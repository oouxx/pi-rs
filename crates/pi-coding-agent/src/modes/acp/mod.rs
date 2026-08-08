//! ACP (Agent Client Protocol) mode — lets ACP clients (Zed, JetBrains,
//! ACP VSCode extensions, …) drive pi-coding-agent directly over stdio.
//!
//! Run with `pi-cli --acp` and register in the editor, e.g. in Zed:
//!
//! ```json
//! "agent_servers": {
//!   "pi-rs": { "type": "custom", "command": "pi-cli", "args": ["--acp"], "env": {} }
//! }
//! ```

pub mod agent;
pub mod session;
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
pub async fn run_acp_mode() -> i32 {
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

            let agent = PiAcpAgent::new(notif_tx, spawn_for_agent);
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

            handle_io.await
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

    /// Wire a `PiAcpAgent` to a client-side connection over in-memory pipes.
    /// Returns the client-side connection (implements `Agent`) for the test
    /// to drive, and keeps the agent-side connection (implements `Client`)
    /// inside a task that forwards session notifications.
    fn wire(
        spawn: Arc<dyn Fn(LocalBoxFuture<'static, ()>)>,
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
            TestClient,
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
}
