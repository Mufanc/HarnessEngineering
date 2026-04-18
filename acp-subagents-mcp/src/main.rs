use std::sync::Arc;

use clap::Parser;
use rmcp::{ServiceExt, transport::stdio};

use acp_subagents_mcp::{config::Config, server::McpServer};

#[derive(Parser)]
#[command(
    name = "acp-subagents-mcp",
    about = "MCP server that provides subagent capabilities via ACP"
)]
struct Cli {
    /// Path to agents.toml config file.
    /// Defaults to ~/.config/acp-subagents-mcp/agents.toml
    #[arg(long)]
    config: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    let config = match cli.config {
        Some(path) => Config::load(&path)?,
        None => Config::load_default()?,
    };

    tracing::info!(agents = config.agents.len(), "acp-subagents-mcp starting");

    let server = McpServer::new(Arc::new(config));
    let running = server.serve(stdio()).await?;
    running.waiting().await?;

    Ok(())
}
