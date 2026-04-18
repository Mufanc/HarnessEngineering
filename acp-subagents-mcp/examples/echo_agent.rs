//! Minimal ACP echo agent for integration testing.
//!
//! Reads from stdin/stdout using the ACP protocol.
//! On each prompt, sends all text blocks back as AgentMessageChunk notifications,
//! then responds with StopReason::EndTurn.
//!
//! Usage (configured in agents.toml):
//!
//!   [agents.echo]
//!   command = "/path/to/echo_agent"

use std::{cell::RefCell, rc::Rc};

use agent_client_protocol::{
    Agent, AgentCapabilities, AgentSideConnection, AuthenticateRequest, AuthenticateResponse,
    CancelNotification, Client, ContentBlock, ContentChunk, Implementation, InitializeRequest,
    InitializeResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    SessionNotification, SessionUpdate, StopReason, TextContent,
};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Minimal no-op client handler for the agent side (required by AgentSideConnection).
#[allow(dead_code)]
struct NoopClientHandler;

#[async_trait::async_trait(?Send)]
impl Client for NoopClientHandler {
    async fn request_permission(
        &self,
        _args: agent_client_protocol::RequestPermissionRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::RequestPermissionResponse> {
        Ok(agent_client_protocol::RequestPermissionResponse::new(
            agent_client_protocol::RequestPermissionOutcome::Cancelled,
        ))
    }

    async fn session_notification(
        &self,
        _args: SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        Ok(())
    }
}

/// Echo agent: reflects prompt text back as AgentMessageChunk notifications.
struct EchoAgent {
    /// Connection back to the client, injected after the connection is set up.
    conn: Rc<RefCell<Option<AgentSideConnection>>>,
}

#[async_trait::async_trait(?Send)]
impl Agent for EchoAgent {
    async fn initialize(
        &self,
        args: InitializeRequest,
    ) -> agent_client_protocol::Result<InitializeResponse> {
        Ok(InitializeResponse::new(args.protocol_version)
            .agent_info(Implementation::new("echo-agent", "0.1.0").title("Echo Agent"))
            .agent_capabilities(AgentCapabilities::new()))
    }

    async fn authenticate(
        &self,
        _args: AuthenticateRequest,
    ) -> agent_client_protocol::Result<AuthenticateResponse> {
        Ok(AuthenticateResponse::default())
    }

    async fn new_session(
        &self,
        args: NewSessionRequest,
    ) -> agent_client_protocol::Result<NewSessionResponse> {
        let _ = args;
        Ok(NewSessionResponse::new("echo-session-1"))
    }

    async fn prompt(&self, args: PromptRequest) -> agent_client_protocol::Result<PromptResponse> {
        let session_id = args.session_id.clone();

        // Collect all text from prompt blocks
        let text: String = args
            .prompt
            .into_iter()
            .filter_map(|block| {
                if let ContentBlock::Text(TextContent { text, .. }) = block {
                    Some(text)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Send text back as AgentMessageChunk via session_notification
        if let Some(conn) = self.conn.borrow().as_ref() {
            conn.session_notification(SessionNotification::new(
                session_id,
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                    TextContent::new(text),
                ))),
            ))
            .await?;
        }

        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(&self, _args: CancelNotification) -> agent_client_protocol::Result<()> {
        Ok(())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let conn_slot: Rc<RefCell<Option<AgentSideConnection>>> = Rc::new(RefCell::new(None));

    let agent = EchoAgent {
        conn: Rc::clone(&conn_slot),
    };

    let stdin = tokio::io::stdin().compat();
    let stdout = tokio::io::stdout().compat_write();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let (conn, io_future) = AgentSideConnection::new(agent, stdout, stdin, |fut| {
                tokio::task::spawn_local(fut);
            });

            // Inject the connection into the agent so it can send notifications
            *conn_slot.borrow_mut() = Some(conn);

            if let Err(e) = io_future.await {
                eprintln!("echo-agent I/O error: {e}");
            }
        })
        .await;
}
