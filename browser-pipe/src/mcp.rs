use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content, ServerCapabilities, ServerInfo};
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::info;

use crate::constants::WS_LISTEN_ADDR;
use crate::protocol::{DaemonRequest, DaemonResponse};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FetchParams {
    /// The URL to fetch
    pub url: String,

    /// HTTP method (GET, POST, PUT, DELETE, etc.). Defaults to "GET".
    pub method: Option<String>,

    /// HTTP headers as key-value pairs
    pub headers: Option<HashMap<String, String>>,

    /// Request body (for POST, PUT, etc.)
    pub body: Option<String>,

    /// Redirect behavior: "follow", "error", or "manual". Defaults to "follow".
    pub redirect: Option<String>,
}

/// Result returned by the fetch tool
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FetchResult {
    status: u16,
    status_text: String,
    body: Option<String>,
    body_base64: Option<String>,
    redirected: bool,
    url: String,
}

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct DaemonConnection {
    sink: SplitSink<WsStream, Message>,
    stream: SplitStream<WsStream>,
}

#[derive(Clone)]
pub struct BrowserPipeServer {
    conn: Arc<Mutex<Option<DaemonConnection>>>,
    tool_router: ToolRouter<Self>,
}

impl BrowserPipeServer {
    pub fn new() -> Self {
        Self {
            conn: Arc::new(Mutex::new(None)),
            tool_router: Self::tool_router(),
        }
    }

    async fn ensure_connection(&self) -> Result<(), String> {
        let mut conn = self.conn.lock().await;
        if conn.is_some() {
            return Ok(());
        }

        let url = format!("ws://{WS_LISTEN_ADDR}/mcp");

        // Try connecting to existing daemon
        match tokio_tungstenite::connect_async(&url).await {
            Ok((ws, _)) => {
                let (sink, stream) = ws.split();
                *conn = Some(DaemonConnection { sink, stream });
                return Ok(());
            }
            Err(_) => {
                // Daemon not running, spawn it
                info!("Daemon not running, spawning...");
                self.spawn_daemon()?;
            }
        }

        // Retry connecting with backoff
        for i in 0..30 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            match tokio_tungstenite::connect_async(&url).await {
                Ok((ws, _)) => {
                    info!("Connected to daemon after {} retries", i + 1);
                    let (sink, stream) = ws.split();
                    *conn = Some(DaemonConnection { sink, stream });
                    return Ok(());
                }
                Err(_) => continue,
            }
        }

        Err("Failed to connect to daemon after 3 seconds".to_string())
    }

    fn spawn_daemon(&self) -> Result<(), String> {
        crate::daemon::spawn_daemon().map_err(|err| format!("Failed to spawn daemon: {err}"))
    }
}

#[tool_router]
impl BrowserPipeServer {
    #[tool(
        name = "piped-fetch",
        description = "Fetch a URL using the local Chrome browser's cookies and session. The request is forwarded through a Chrome extension, which automatically injects browser cookies for authentication."
    )]
    async fn fetch(
        &self,
        Parameters(params): Parameters<FetchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Ensure daemon connection
        if let Err(err) = self.ensure_connection().await {
            return Ok(CallToolResult::error(vec![Content::text(err)]));
        }

        let request_id = uuid::Uuid::new_v4().to_string();

        let request = DaemonRequest {
            id: request_id.clone(),
            url: params.url,
            method: params.method.unwrap_or_else(|| "GET".to_string()),
            headers: params.headers,
            body: params.body,
            redirect: params.redirect.unwrap_or_else(|| "follow".to_string()),
        };

        // Send request and read response
        let response = {
            let mut conn = self.conn.lock().await;
            let daemon_conn = conn
                .as_mut()
                .ok_or_else(|| ErrorData::internal_error("Not connected to daemon", None))?;

            // Serialize and send via WebSocket
            let json = serde_json::to_string(&request).map_err(|err| {
                ErrorData::internal_error(format!("Serialize error: {err}"), None)
            })?;

            if let Err(err) = daemon_conn.sink.send(Message::Text(json.into())).await {
                // Connection broken, reset
                *conn = None;
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Failed to send request to daemon: {err}"
                ))]));
            }

            // Read response from WebSocket
            match daemon_conn.stream.next().await {
                Some(Ok(Message::Text(text))) => {
                    match serde_json::from_str::<DaemonResponse>(&text) {
                        Ok(resp) => resp,
                        Err(err) => {
                            return Ok(CallToolResult::error(vec![Content::text(format!(
                                "Failed to parse daemon response: {err}"
                            ))]));
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None => {
                    *conn = None;
                    return Ok(CallToolResult::error(vec![Content::text(
                        "Daemon connection closed unexpectedly",
                    )]));
                }
                Some(Ok(_)) => {
                    return Ok(CallToolResult::error(vec![Content::text(
                        "Unexpected message type from daemon",
                    )]));
                }
                Some(Err(err)) => {
                    *conn = None;
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "Failed to read daemon response: {err}"
                    ))]));
                }
            }
        };

        // Check for error response
        if let Some(error) = response.error {
            return Ok(CallToolResult::error(vec![Content::text(error)]));
        }

        // Build structured result
        let result = FetchResult {
            status: response.status.unwrap_or(0),
            status_text: response.status_text.unwrap_or_default(),
            body: response.body,
            body_base64: response.body_base64,
            redirected: response.redirected.unwrap_or(false),
            url: response.url.unwrap_or_default(),
        };

        let json = serde_json::to_string_pretty(&result).map_err(|err| {
            ErrorData::internal_error(format!("Serialize result error: {err}"), None)
        })?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }
}

#[tool_handler]
impl ServerHandler for BrowserPipeServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Browser Pipe: Fetch URLs using the local Chrome browser's cookies and session.",
        )
    }
}
