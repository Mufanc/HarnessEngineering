use crate::constants::{CHROME_CONNECT_TIMEOUT, LOG_PATH, REQUEST_TIMEOUT, WS_LISTEN_ADDR};
use crate::protocol::{ChromeMessage, ChromeRequest, DaemonRequest, DaemonResponse};
use anyhow::bail;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::collections::HashMap;
use std::convert::Infallible;
use std::env;
use std::fs::File;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::os::unix::prelude::CommandExt;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::signal;
use tokio::sync::{Mutex, Notify, oneshot};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

type PendingMap = HashMap<String, oneshot::Sender<DaemonResponse>>;
type WsSink = SplitSink<WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>, Message>;

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

pub fn spawn_daemon() -> anyhow::Result<()> {
    let exe = env::current_exe()?;
    let log_file = File::create(LOG_PATH)?;

    Command::new(exe)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_file))
        .process_group(0)
        .spawn()?;

    Ok(())
}

pub async fn ensure_daemon() -> anyhow::Result<()> {
    let url = format!("ws://{WS_LISTEN_ADDR}/mcp");

    // Check if daemon is already running
    if let Ok((mut ws, _)) = tokio_tungstenite::connect_async(&url).await {
        let _ = ws.close(None).await;
        println!("Daemon is already running");
        return Ok(());
    }

    // Daemon not running, spawn it
    spawn_daemon()?;

    // Wait briefly to confirm it started
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok((mut ws, _)) = tokio_tungstenite::connect_async(&url).await {
            let _ = ws.close(None).await;
            println!("Daemon started");
            return Ok(());
        }
    }

    bail!("Failed to confirm daemon started after 3 seconds")
}

pub async fn stop_daemon() -> anyhow::Result<()> {
    match TcpStream::connect(WS_LISTEN_ADDR).await {
        Ok(stream) => {
            let io = TokioIo::new(stream);
            let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
            tokio::spawn(conn);

            let req = Request::builder()
                .method("POST")
                .uri("/shutdown")
                .header("Connection", "close")
                .body(Empty::<Bytes>::new())?;

            let _ = sender.send_request(req).await?;
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
            bail!("Another daemon is already running at {}", WS_LISTEN_ADDR);
        }
        Err(err) => return Err(err.into()),
    };

    info!("Daemon listening at {}", WS_LISTEN_ADDR);
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

// ── HTTP server with path-based routing ──

async fn accept_connections(listener: TcpListener, state: Arc<DaemonState>) {
    loop {
        let (stream, addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(err) => {
                error!("Failed to accept TCP connection: {}", err);
                continue;
            }
        };

        let state = state.clone();
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let service = service_fn(move |req| {
                let state = state.clone();
                handle_connection(req, state, addr)
            });

            let conn = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, service)
                .with_upgrades();

            if let Err(err) = conn.await {
                debug!("Connection closed from {}: {}", addr, err);
            }
        });
    }
}

async fn handle_connection(
    req: Request<Incoming>,
    state: Arc<DaemonState>,
    addr: SocketAddr,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let path = req.uri().path().to_string();

    match path.as_str() {
        "/fetch" => handle_http_fetch(req, state).await,
        "/shutdown" => {
            info!("Received shutdown command from {}", addr);
            state.shutdown.notify_one();
            Ok(Response::builder()
                .status(StatusCode::OK)
                .body(Full::new(Bytes::from("OK")))
                .expect("wtf??"))
        }
        _ => handle_ws_upgrade(req, state, addr, path).await,
    }
}

// ── WebSocket upgrade ──

async fn handle_ws_upgrade(
    req: Request<Incoming>,
    state: Arc<DaemonState>,
    addr: SocketAddr,
    path: String,
) -> Result<Response<Full<Bytes>>, Infallible> {
    // Validate WebSocket upgrade request
    let key = match req.headers().get("sec-websocket-key") {
        Some(key) => match key.to_str() {
            Ok(s) => s.to_string(),
            Err(_) => {
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Full::new(Bytes::from("Invalid Sec-WebSocket-Key")))
                    .expect("wtf??"));
            }
        },
        None => {
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Full::new(Bytes::from("Not a WebSocket upgrade request")))
                .expect("wtf??"));
        }
    };

    let accept_key = derive_accept_key(key.as_bytes());

    match path.as_str() {
        "/mcp" => info!("MCP client connected from {}", addr),
        _ => info!("Chrome extension connected from {}", addr),
    }

    // Spawn task to handle the upgraded connection
    tokio::spawn(async move {
        match hyper::upgrade::on(req).await {
            Ok(upgraded) => {
                let ws =
                    WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None)
                        .await;

                match path.as_str() {
                    "/mcp" => handle_mcp_client(ws, state).await,
                    _ => handle_chrome(ws, state).await,
                }
            }
            Err(err) => {
                error!("WebSocket upgrade failed from {}: {}", addr, err);
            }
        }
    });

    // Return 101 Switching Protocols
    Ok(Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Accept", accept_key)
        .body(Full::new(Bytes::new()))
        .expect("wtf??"))
}

// ── Shared request dispatch (used by MCP and HTTP handlers) ──

async fn dispatch_request(state: &DaemonState, request: DaemonRequest) -> DaemonResponse {
    let request_id = request.id.clone();

    // Wait for Chrome extension if not connected yet
    if !state.wait_for_chrome().await {
        return DaemonResponse::error(
            request_id,
            "Chrome extension is not connected. Please ensure the Browser Pipe extension is installed and the browser is open.".to_string(),
        );
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
        return DaemonResponse::error(request_id, err);
    }

    // Wait for response with timeout
    match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
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
    }
}

// ── HTTP /fetch handler ──

async fn handle_http_fetch(
    req: Request<Incoming>,
    state: Arc<DaemonState>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    // Extract URL from X-Forwarded-Url header
    let url = match req.headers().get("x-forwarded-url") {
        Some(value) => match value.to_str() {
            Ok(s) => s.to_string(),
            Err(_) => {
                return Ok(json_response(
                    StatusCode::BAD_REQUEST,
                    &DaemonResponse::error(
                        String::new(),
                        "Invalid X-Forwarded-Url header value".to_string(),
                    ),
                ));
            }
        },
        None => {
            return Ok(json_response(
                StatusCode::BAD_REQUEST,
                &DaemonResponse::error(String::new(), "Missing X-Forwarded-Url header".to_string()),
            ));
        }
    };

    let method = req.method().to_string();

    // Collect headers, filtering out hop-by-hop and special headers
    let headers: HashMap<String, String> = req
        .headers()
        .iter()
        .filter(|(name, _)| {
            !matches!(
                name.as_str(),
                "x-forwarded-url"
                    | "user-agent"
                    | "host"
                    | "content-length"
                    | "connection"
                    | "transfer-encoding"
                    | "accept-encoding"
            )
        })
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.to_string(), value.to_string()))
        })
        .collect();

    // Read body
    let body_bytes = match req.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(err) => {
            return Ok(json_response(
                StatusCode::BAD_REQUEST,
                &DaemonResponse::error(
                    String::new(),
                    format!("Failed to read request body: {err}"),
                ),
            ));
        }
    };

    let body = if body_bytes.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(&body_bytes).to_string())
    };

    let request = DaemonRequest {
        id: Uuid::new_v4().to_string(),
        url,
        method,
        headers: if headers.is_empty() {
            None
        } else {
            Some(headers)
        },
        body,
        redirect: "follow".to_string(),
    };

    let resp = dispatch_request(&state, request).await;
    Ok(json_response(StatusCode::OK, &resp))
}

fn json_response(status: StatusCode, resp: &DaemonResponse) -> Response<Full<Bytes>> {
    let json = serde_json::to_string(resp).unwrap_or_else(|_| "{}".to_string());
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Full::new(Bytes::from(json)))
        .unwrap()
}

// ── MCP client handler (WebSocket) ──

async fn reply_to_mcp(sink: &mut WsSink, resp: &DaemonResponse) -> bool {
    if let Ok(json) = serde_json::to_string(resp) {
        sink.send(Message::Text(json.into())).await.is_ok()
    } else {
        false
    }
}

async fn handle_mcp_client(
    ws_stream: WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>,
    state: Arc<DaemonState>,
) {
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
                if !reply_to_mcp(&mut sink, &err_resp).await {
                    break;
                }
                continue;
            }
        };

        let resp = dispatch_request(&state, request).await;

        if !reply_to_mcp(&mut sink, &resp).await {
            break;
        }
    }

    info!("MCP client disconnected");
}

// ── Chrome extension handler (WebSocket) ──

async fn handle_chrome(
    ws_stream: WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>,
    state: Arc<DaemonState>,
) {
    let (sink, mut stream) = ws_stream.split();
    {
        let mut chrome_tx = state.chrome_tx.lock().await;
        *chrome_tx = Some(sink);
    }
    state.chrome_ready.notify_waiters();

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
}
