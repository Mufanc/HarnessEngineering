use agent_client_protocol::{
    Client, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SessionNotification,
};
use tracing::warn;

/// Minimal ACP client handler.
///
/// Denies all permission requests from the agent and silently accepts session
/// notifications. This satisfies the `Client` trait requirements without granting
/// any file system or terminal access to the subagent.
pub struct DefaultClientHandler;

#[async_trait::async_trait(?Send)]
impl Client for DefaultClientHandler {
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
        _args: SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        Ok(())
    }
}
