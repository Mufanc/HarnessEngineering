use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::Arc;

use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::signal;
use tokio::sync::{Mutex, Notify, oneshot};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tracing::{error, info, warn};

use crate::constants::{CHROME_CONNECT_TIMEOUT, REQUEST_TIMEOUT, WS_LISTEN_ADDR};
use crate::protocol::{ChromeMessage, ChromeRequest, DaemonRequest, DaemonResponse};

type PendingMap = HashMap<String, oneshot::Sender<DaemonResponse>>;
type WsSink = SplitSink<WebSocketStream<TcpStream>, Message>;

struct DaemonState {
    chrome_tx: Mutex<Option<WsSink>>,
    pending: Mutex<PendingMap>,
    chrome_ready: Notify,
    shutdown: Notify,
}

impl DaemonState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            chrome_tx: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            chrome_ready: Notify::new(),
            shutdown: Notify::new(),
        })
    }

    async fn drain_pending(&self, reason: &str) {
        let mut pending = self.pending.lock().await;

        for (id, tx) in pending.drain() {
            let _ = tx.send(DaemonResponse::error(id, reason.to_string()));
        }
    }

    async fn is_chrome_ready(&self) -> bool {
        self.chrome_tx.lock().await.is_some()
    }

    async fn wait_for_chrome(&self) -> bool {
        use std::pin::pin;

        // Register interest BEFORE checking state to avoid race condition
        let mut notified = pin!(self.chrome_ready.notified());
        notified.as_mut().enable();

        if self.is_chrome_ready().await {
            return true;
        }

        info!("Waiting for Chrome extension to connect...");

        match tokio::time::timeout(CHROME_CONNECT_TIMEOUT, notified).await {
            Ok(_) => {
                info!("Chrome extension connected, proceeding with request");
                true
            }
            Err(_) => {
                warn!(
                    "Timed out waiting for Chrome extension ({}s)",
                    CHROME_CONNECT_TIMEOUT.as_secs()
                );
                false
            }
        }
    }
}

pub async fn stop_daemon() -> anyhow::Result<()> {
    let url = format!("ws://{WS_LISTEN_ADDR}/shutdown");

    match tokio_tungstenite::connect_async(&url).await {
        Ok((mut ws, _)) => {
            let _ = ws.close(None).await;
            println!("Shutdown signal sent to daemon");
        }
        Err(_) => {
            println!("No daemon running (could not connect)");
        }
    }

    Ok(())
}

pub async fn run_daemon() -> anyhow::Result<()> {
    let state = DaemonState::new();
    let tcp_listener = match TcpListener::bind(WS_LISTEN_ADDR).await {
        Ok(listener) => listener,
        Err(err) if err.kind() == ErrorKind::AddrInUse => {
            anyhow::bail!("Another daemon is already running at {}", WS_LISTEN_ADDR);
        }
        Err(err) => return Err(err.into()),
    };

    info!("WebSocket server listening at {}", WS_LISTEN_ADDR);
    tokio::select! {
        _ = accept_connections(tcp_listener, state.clone()) => {}
        _ = state.shutdown.notified() => {
            info!("Received shutdown command");
        }
        _ = signal::ctrl_c() => {
            info!("Received shutdown signal");
        }
    }

    state.drain_pending("Daemon shutting down").await;
    info!("Daemon stopped");

    Ok(())
}

// ── WebSocket server with path-based routing ──

async fn accept_connections(listener: TcpListener, state: Arc<DaemonState>) {
    loop {
        let (stream, addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(err) => {
                error!("Failed to accept TCP connection: {}", err);
                continue;
            }
        };

        // Extract path from HTTP upgrade request during handshake
        let path = parking_lot::Mutex::new(String::new());
        let ws_stream = match tokio_tungstenite::accept_hdr_async(
            stream,
            |req: &tokio_tungstenite::tungstenite::http::Request<()>, resp| {
                *path.lock() = req.uri().path().to_string();
                Ok(resp)
            },
        )
        .await
        {
            Ok(ws) => ws,
            Err(err) => {
                error!("WebSocket handshake failed from {}: {}", addr, err);
                continue;
            }
        };

        let path = path.into_inner();

        match path.as_str() {
            "/mcp" => {
                info!("MCP client connected from {}", addr);
                let state = state.clone();
                tokio::spawn(handle_mcp_client(ws_stream, state));
            }
            "/shutdown" => {
                info!("Received shutdown command from {}", addr);
                state.shutdown.notify_one();
            }
            _ => {
                // Default: Chrome extension
                info!("Chrome extension connected from {}", addr);

                let (sink, mut stream) = ws_stream.split();
                {
                    let mut chrome_tx = state.chrome_tx.lock().await;
                    *chrome_tx = Some(sink);
                }
                state.chrome_ready.notify_waiters();

                let state = state.clone();
                tokio::spawn(async move {
                    while let Some(msg) = stream.next().await {
                        let msg = match msg {
                            Ok(Message::Text(text)) => text,
                            Ok(Message::Close(_)) => break,
                            Ok(_) => continue,
                            Err(err) => {
                                warn!("WebSocket read error: {}", err);
                                break;
                            }
                        };

                        let chrome_msg: ChromeMessage = match serde_json::from_str(&msg) {
                            Ok(m) => m,
                            Err(err) => {
                                warn!("Failed to parse Chrome message: {}", err);
                                continue;
                            }
                        };

                        let resp = chrome_msg.into_daemon_resp();
                        let id = resp.id.clone();

                        let sender = {
                            let mut pending = state.pending.lock().await;
                            pending.remove(&id)
                        };

                        match sender {
                            Some(tx) => {
                                let _ = tx.send(resp);
                            }
                            None => {
                                warn!("No pending request for id {}", id);
                            }
                        }
                    }

                    info!("Chrome extension disconnected");

                    {
                        let mut chrome_tx = state.chrome_tx.lock().await;
                        *chrome_tx = None;
                    }
                    state.drain_pending("Chrome extension disconnected").await;
                });
            }
        }
    }
}

// ── MCP client handler (WebSocket) ──

async fn reply_to_mcp(sink: &mut WsSink, resp: &DaemonResponse) -> Result<(), ()> {
    match serde_json::to_string(resp) {
        Ok(json) => sink.send(Message::Text(json.into())).await.map_err(|_| ()),
        Err(_) => Err(()),
    }
}

async fn handle_mcp_client(ws_stream: WebSocketStream<TcpStream>, state: Arc<DaemonState>) {
    let (mut sink, mut stream) = ws_stream.split();

    while let Some(msg) = stream.next().await {
        let text = match msg {
            Ok(Message::Text(text)) => text,
            Ok(Message::Close(_)) => break,
            Ok(_) => continue,
            Err(err) => {
                warn!("MCP WebSocket read error: {}", err);
                break;
            }
        };

        // Check for shutdown command
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
            && value.get("shutdown").and_then(|value| value.as_bool()) == Some(true)
        {
            info!("Received shutdown command from MCP client");
            state.shutdown.notify_one();
            return;
        }

        let request: DaemonRequest = match serde_json::from_str(&text) {
            Ok(r) => r,
            Err(err) => {
                warn!("Failed to parse daemon request: {}", err);
                let err_resp =
                    DaemonResponse::error(String::new(), format!("Invalid request: {err}"));
                if reply_to_mcp(&mut sink, &err_resp).await.is_err() {
                    break;
                }
                continue;
            }
        };

        let request_id = request.id.clone();

        // Wait for Chrome extension if not connected yet
        if !state.wait_for_chrome().await {
            let resp = DaemonResponse::error(
                request_id,
                "Chrome extension is not connected. Please ensure the Browser Pipe extension is installed and the browser is open.".to_string(),
            );
            if reply_to_mcp(&mut sink, &resp).await.is_err() {
                break;
            }
            continue;
        }

        // Create oneshot channel for response
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = state.pending.lock().await;
            pending.insert(request_id.clone(), tx);
        }

        // Send request to Chrome
        let send_result = {
            let mut chrome_tx = state.chrome_tx.lock().await;

            match chrome_tx.as_mut() {
                Some(chrome_sink) => {
                    let chrome_req = ChromeRequest::from(request);

                    match serde_json::to_string(&chrome_req) {
                        Ok(json) => chrome_sink
                            .send(Message::Text(json.into()))
                            .await
                            .map_err(|err| err.to_string()),
                        Err(err) => Err(err.to_string()),
                    }
                }
                None => Err("Chrome extension disconnected while preparing request".to_string()),
            }
        };

        if let Err(err) = send_result {
            {
                let mut pending = state.pending.lock().await;
                pending.remove(&request_id);
            }
            let resp = DaemonResponse::error(request_id, err);

            if reply_to_mcp(&mut sink, &resp).await.is_err() {
                break;
            }

            continue;
        }

        // Wait for response with timeout
        let resp = match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(_)) => DaemonResponse::error(
                request_id.clone(),
                "Internal error: response channel closed".to_string(),
            ),
            Err(_) => {
                {
                    let mut pending = state.pending.lock().await;
                    pending.remove(&request_id);
                }
                DaemonResponse::error(request_id, "Request timed out (30s)".to_string())
            }
        };

        if reply_to_mcp(&mut sink, &resp).await.is_err() {
            break;
        }
    }

    info!("MCP client disconnected");
}
