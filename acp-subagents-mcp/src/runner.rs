use std::time::Duration;

use agent_client_protocol::{
    Agent, Client, ClientSideConnection, ContentBlock, InitializeRequest, NewSessionRequest,
    PromptRequest, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SessionNotification, SessionUpdate, StopReason, TextContent,
};
use tokio::sync::mpsc;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tracing::{debug, warn};

use crate::{config::AgentConfig, error::AppError};

pub struct RunResult {
    pub text: String,
    pub stop_reason: StopReason,
}

pub struct SubagentRunner {
    pub config: AgentConfig,
}

impl SubagentRunner {
    /// Run the subagent. This method is `Send`-safe: the `!Send` ACP internals
    /// are confined to a dedicated `spawn_blocking` thread with its own runtime.
    pub async fn run(&self, prompt: &str, timeout: Duration) -> Result<RunResult, AppError> {
        let prompt = prompt.to_owned();
        let config = self.config.clone();

        // ACP uses `!Send` futures (Rc, LocalBoxFuture). Run everything on a
        // dedicated thread with its own single-threaded Tokio runtime so the
        // calling future remains Send.
        tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| AppError::Config(format!("failed to build runtime: {e}")))?;

            let local = tokio::task::LocalSet::new();
            rt.block_on(local.run_until(run_inner(&config, &prompt, timeout)))
        })
        .await
        .map_err(|e| AppError::Config(format!("spawn_blocking panicked: {e}")))?
    }
}

/// ACP client handler that collects AgentMessageChunk text via a channel.
struct CollectingClientHandler {
    tx: mpsc::UnboundedSender<String>,
}

#[async_trait::async_trait(?Send)]
impl Client for CollectingClientHandler {
    async fn request_permission(
        &self,
        args: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        warn!(
            session_id = ?args.session_id,
            "subagent requested permission — denying (no client capabilities configured)"
        );

        Ok(RequestPermissionResponse::new(
            RequestPermissionOutcome::Cancelled,
        ))
    }

    async fn session_notification(
        &self,
        args: SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        if let SessionUpdate::AgentMessageChunk(chunk) = args.update {
            if let ContentBlock::Text(text_content) = chunk.content {
                let _ = self.tx.send(text_content.text);
            }
        }
        Ok(())
    }
}

async fn run_inner(
    config: &AgentConfig,
    prompt: &str,
    timeout: Duration,
) -> Result<RunResult, AppError> {
    // 1. Spawn the agent subprocess
    let mut child = tokio::process::Command::new(&config.command)
        .args(&config.args)
        .envs(&config.env)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .map_err(AppError::SpawnFailed)?;

    let stdin = child.stdin.take().unwrap().compat_write();
    let stdout = child.stdout.take().unwrap().compat();

    // 2. Set up text collection channel
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    // 3. Establish ACP connection (runs in LocalSet via spawn_local)
    let (conn, io_future) =
        ClientSideConnection::new(CollectingClientHandler { tx }, stdin, stdout, |fut| {
            tokio::task::spawn_local(fut);
        });
    tokio::task::spawn_local(async move {
        if let Err(e) = io_future.await {
            debug!("ACP I/O task ended: {e}");
        }
    });

    // 4. Initialize
    conn.initialize(InitializeRequest::new(
        agent_client_protocol::ProtocolVersion::LATEST,
    ))
    .await
    .map_err(|e| AppError::HandshakeFailed(format!("initialize: {e}")))?;

    // 5. New session
    let session_resp = conn
        .new_session(NewSessionRequest::new(
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/")),
        ))
        .await
        .map_err(|e| AppError::HandshakeFailed(format!("new_session: {e}")))?;
    let session_id = session_resp.session_id;

    // 6. Build prompt content, prepend system_prompt if configured
    let mut prompt_blocks: Vec<ContentBlock> = Vec::new();
    if let Some(sys) = &config.system_prompt {
        prompt_blocks.push(ContentBlock::Text(TextContent::new(sys.clone())));
    }
    prompt_blocks.push(ContentBlock::Text(TextContent::new(prompt)));

    // 7. Send prompt with timeout
    let prompt_result = tokio::time::timeout(
        timeout,
        conn.prompt(PromptRequest::new(session_id, prompt_blocks)),
    )
    .await;

    // 8. Drain the text channel
    rx.close();
    let mut chunks: Vec<String> = Vec::new();
    while let Ok(chunk) = rx.try_recv() {
        chunks.push(chunk);
    }
    let text = chunks.join("");

    // 9. Kill the child process regardless of outcome
    if let Err(e) = child.kill().await {
        warn!("failed to kill agent process: {e}");
    }

    // 10. Handle prompt result
    let prompt_resp = match prompt_result {
        Ok(Ok(resp)) => resp,
        Ok(Err(e)) => return Err(AppError::AcpError(e)),
        Err(_) => return Err(AppError::Timeout),
    };

    match &prompt_resp.stop_reason {
        StopReason::Refusal => {
            return Err(AppError::AgentStopped("agent refused the request".into()));
        }
        StopReason::Cancelled => {
            return Err(AppError::AgentStopped("agent was cancelled".into()));
        }
        _ => {}
    }

    Ok(RunResult {
        text,
        stop_reason: prompt_resp.stop_reason,
    })
}
