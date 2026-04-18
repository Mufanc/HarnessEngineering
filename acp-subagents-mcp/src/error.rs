use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("agent type '{0}' not found in config")]
    AgentNotFound(String),

    #[error("failed to spawn agent process: {0}")]
    SpawnFailed(#[source] std::io::Error),

    #[error("ACP handshake failed: {0}")]
    HandshakeFailed(String),

    #[error("ACP error: {0}")]
    AcpError(#[from] agent_client_protocol::Error),

    #[error("operation timed out")]
    Timeout,

    #[error("agent stopped unexpectedly: {0}")]
    AgentStopped(String),

    #[error("config error: {0}")]
    Config(String),
}

impl From<AppError> for rmcp::ErrorData {
    fn from(err: AppError) -> Self {
        rmcp::ErrorData::internal_error(err.to_string(), None)
    }
}
