//! Loopback HTTP server for the OAuth authorization callback.
//!
//! Port of `packages/mcp/src/oauth/callback.ts`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Mutex};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::oauth::errors::OAuthError;

/// Result delivered by the callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthCallback {
    /// Authorization code.
    pub code: String,
    /// `state` parameter.
    pub state: String,
    /// `iss` parameter (RFC 9207).
    pub iss: Option<String>,
}

/// Browser page outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthCallbackPage {
    /// Whether authorization succeeded.
    pub ok: bool,
    /// Message shown to the user.
    pub message: String,
    /// Optional details.
    pub details: Option<String>,
}

impl OAuthCallbackPage {
    fn success() -> Self {
        Self {
            ok: true,
            message: "Authorization complete. You may close this window.".to_string(),
            details: None,
        }
    }

    fn failure(message: impl Into<String>, details: Option<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
            details,
        }
    }

    fn plain_text(&self) -> String {
        if self.ok {
            return self.message.clone();
        }
        match &self.details {
            Some(details) => format!("{}\n\n{details}", self.message),
            None => self.message.clone(),
        }
    }
}

/// Renders the browser page after the redirect.
pub type PageRenderer = Arc<dyn Fn(&OAuthCallbackPage) -> String + Send + Sync>;

/// Options for [`OAuthCallbackServer::listen`].
#[derive(Clone, Default)]
pub struct OAuthCallbackServerOptions {
    /// Address to listen on. Default: `127.0.0.1`.
    pub host: Option<String>,
    /// Host name in the redirect URL. Default: `host`.
    pub redirect_host: Option<String>,
    /// Port; default: a free port.
    pub port: Option<u16>,
    /// Callback path. Default: `/callback`.
    pub path: Option<String>,
    /// More paths that receive the callback.
    pub extra_paths: Vec<String>,
    /// Callback timeout. Default: 5 minutes.
    pub timeout: Option<Duration>,
    /// Custom page renderer.
    pub render_page: Option<PageRenderer>,
}

struct Pending {
    sender: oneshot::Sender<std::result::Result<OAuthCallback, OAuthError>>,
    path: Option<String>,
}

/// Loopback HTTP server that receives the authorization response.
pub struct OAuthCallbackServer {
    /// Redirect URL clients should register.
    pub redirect_url: String,
    timeout: Duration,
    pending: Arc<Mutex<HashMap<String, Pending>>>,
    shutdown: CancellationToken,
    accept_task: tokio::task::JoinHandle<()>,
}

impl OAuthCallbackServer {
    /// Bind to `port` (or a free port) and start accepting callbacks.
    pub async fn listen(
        options: OAuthCallbackServerOptions,
    ) -> std::result::Result<Self, OAuthError> {
        let host = options.host.unwrap_or_else(|| "127.0.0.1".to_string());
        let redirect_host = options.redirect_host.unwrap_or_else(|| host.clone());
        let path = options.path.unwrap_or_else(|| "/callback".to_string());
        let bind_host = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host
        };
        let listener = TcpListener::bind((bind_host.as_str(), options.port.unwrap_or(0)))
            .await
            .map_err(|error| OAuthError::Message(format!("OAuth callback server: {error}")))?;
        let address = listener
            .local_addr()
            .map_err(|error| OAuthError::Message(error.to_string()))?;
        let redirect_host = if redirect_host.contains(':') && !redirect_host.starts_with('[') {
            format!("[{redirect_host}]")
        } else {
            redirect_host
        };
        let redirect_url = format!("http://{redirect_host}:{}{path}", address.port());
        let paths = {
            let mut paths = vec![path];
            paths.extend(options.extra_paths);
            paths
        };
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let shutdown = CancellationToken::new();
        let task_redirect_url = redirect_url.clone();
        let accept_task = {
            let pending = Arc::clone(&pending);
            let shutdown = shutdown.clone();
            let paths = paths.clone();
            let render_page = options.render_page.clone();
            tokio::spawn(async move {
                let redirect_url = task_redirect_url;
                loop {
                    let accepted = tokio::select! {
                        () = shutdown.cancelled() => break,
                        accepted = listener.accept() => accepted,
                    };
                    let Ok((stream, _)) = accepted else {
                        if shutdown.is_cancelled() {
                            break;
                        }
                        continue;
                    };
                    let pending = Arc::clone(&pending);
                    let paths = paths.clone();
                    let render_page = render_page.clone();
                    let redirect_url = redirect_url.clone();
                    tokio::spawn(async move {
                        handle_connection(stream, &redirect_url, &paths, &pending, render_page.as_ref()).await;
                    });
                }
            })
        };
        Ok(Self {
            redirect_url,
            timeout: options.timeout.unwrap_or(Duration::from_secs(5 * 60)),
            pending,
            shutdown,
            accept_task,
        })
    }

    /// Wait for the authorization response with `state`.
    ///
    /// With `path`, a response on another path fails, so a server-specific
    /// redirect URI can tell authorization servers apart.
    pub async fn wait_for_callback(
        &self,
        state: &str,
        path: Option<&str>,
    ) -> std::result::Result<OAuthCallback, OAuthError> {
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if pending.contains_key(state) {
                return Err(OAuthError::Message("OAuth state is already pending".to_string()));
            }
            pending.insert(
                state.to_string(),
                Pending {
                    sender,
                    path: path.map(str::to_string),
                },
            );
        }
        match tokio::time::timeout(self.timeout, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(OAuthError::Message("OAuth callback channel closed".to_string())),
            Err(_) => {
                self.pending.lock().await.remove(state);
                Err(OAuthError::Message("OAuth callback timed out".to_string()))
            }
        }
    }

    /// Stop the server and fail all pending waits.
    pub async fn close(&self) {
        self.shutdown.cancel();
        self.accept_task.abort();
        let pending = {
            let mut pending = self.pending.lock().await;
            pending.drain().map(|(_, pending)| pending).collect::<Vec<_>>()
        };
        for pending in pending {
            let _ = pending
                .sender
                .send(Err(OAuthError::Message("OAuth callback server closed".to_string())));
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    redirect_url: &str,
    paths: &[String],
    pending: &Arc<Mutex<HashMap<String, Pending>>>,
    render_page: Option<&PageRenderer>,
) {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let read = match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buffer.len() > 16 * 1024 {
            break;
        }
    }
    let request = String::from_utf8_lossy(&buffer);
    let request_line = request.lines().next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or("/");
    if method != "GET" {
        respond(&mut stream, 405, &OAuthCallbackPage::failure("Method not allowed", None), render_page).await;
        return;
    }
    let url = match Url::parse(redirect_url).and_then(|base| base.join(target)) {
        Ok(url) => url,
        Err(_) => {
            respond(
                &mut stream,
                400,
                &OAuthCallbackPage::failure("Invalid callback URL", None),
                render_page,
            )
            .await;
            return;
        }
    };
    if !paths.iter().any(|path| path == url.path()) {
        respond(&mut stream, 404, &OAuthCallbackPage::failure("Not found", None), render_page).await;
        return;
    }
    let params: HashMap<String, String> = url.query_pairs().into_owned().collect();
    let Some(state) = params.get("state") else {
        respond(
            &mut stream,
            400,
            &OAuthCallbackPage::failure("Invalid or expired OAuth state", None),
            render_page,
        )
        .await;
        return;
    };
    let pending_entry = pending.lock().await.remove(state);
    let Some(pending_entry) = pending_entry else {
        respond(
            &mut stream,
            400,
            &OAuthCallbackPage::failure("Invalid or expired OAuth state", None),
            render_page,
        )
        .await;
        return;
    };
    if let Some(expected) = &pending_entry.path {
        if expected != url.path() {
            let _ = pending_entry.sender.send(Err(OAuthError::Message(
                "The authorization response arrived on another redirect URI".to_string(),
            )));
            respond(
                &mut stream,
                400,
                &OAuthCallbackPage::failure("Unexpected redirect URI", None),
                render_page,
            )
            .await;
            return;
        }
    }
    if let Some(error) = params.get("error") {
        let description = params
            .get("error_description")
            .cloned()
            .unwrap_or_else(|| error.clone());
        let _ = pending_entry
            .sender
            .send(Err(OAuthError::Message(description.clone())));
        respond(
            &mut stream,
            200,
            &OAuthCallbackPage::failure(
                "Authorization failed. You may close this window.",
                Some(description),
            ),
            render_page,
        )
        .await;
        return;
    }
    let Some(code) = params.get("code") else {
        let _ = pending_entry.sender.send(Err(OAuthError::Message(
            "OAuth callback did not include an authorization code".to_string(),
        )));
        respond(
            &mut stream,
            400,
            &OAuthCallbackPage::failure("Missing authorization code", None),
            render_page,
        )
        .await;
        return;
    };
    let _ = pending_entry.sender.send(Ok(OAuthCallback {
        code: code.clone(),
        state: state.clone(),
        iss: params.get("iss").cloned(),
    }));
    respond(&mut stream, 200, &OAuthCallbackPage::success(), render_page).await;
}

async fn respond(
    stream: &mut TcpStream,
    status: u16,
    page: &OAuthCallbackPage,
    render_page: Option<&PageRenderer>,
) {
    let (content_type, body) = match render_page {
        Some(render) => ("text/html; charset=utf-8", render(page)),
        None => ("text/plain; charset=utf-8", page.plain_text()),
    };
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[tokio::test]
    async fn receives_callback() {
        let server = OAuthCallbackServer::listen(OAuthCallbackServerOptions {
            port: Some(0),
            timeout: Some(Duration::from_secs(5)),
            ..Default::default()
        })
        .await
        .unwrap();
        let redirect_url = server.redirect_url.clone();
        let wait = tokio::spawn(async move { server.wait_for_callback("state-1", None).await });
        let response = reqwest::get(format!("{redirect_url}?code=abc&state=state-1"))
            .await
            .unwrap();
        assert!(response.status().is_success());
        let callback = wait.await.unwrap().unwrap();
        assert_eq!(callback.code, "abc");
        assert_eq!(callback.state, "state-1");
    }

    #[tokio::test]
    async fn rejects_unknown_state() {
        let server = OAuthCallbackServer::listen(OAuthCallbackServerOptions::default())
            .await
            .unwrap();
        let response = reqwest::get(format!("{}?code=abc&state=nope", server.redirect_url))
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 400);
        server.close().await;
    }
}
