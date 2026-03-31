mod constants;
mod daemon;
mod mcp;
mod protocol;

use crate::mcp::BrowserPipeServer;
use anyhow::Result;
use clap::{Parser, Subcommand};
use rmcp::ServiceExt;
use std::io;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "browser-pipe",
    version,
    about = "MCP tool that fetches URLs through Chrome browser"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the daemon (bridge between Chrome extension and MCP clients)
    Daemon,
    /// Stop the running daemon gracefully
    StopDaemon,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Daemon) => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
                )
                .with_writer(io::stderr)
                .with_ansi(false)
                .init();

            daemon::run_daemon().await
        }
        Some(Commands::StopDaemon) => daemon::stop_daemon().await,
        None => {
            // MCP stdio server mode — logging MUST go to stderr
            tracing_subscriber::fmt()
                .with_env_filter(
                    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
                )
                .with_writer(io::stderr)
                .with_ansi(false)
                .init();

            let server = BrowserPipeServer::new();
            let service = server.serve(rmcp::transport::stdio()).await?;

            service.waiting().await?;

            Ok(())
        }
    }
}
