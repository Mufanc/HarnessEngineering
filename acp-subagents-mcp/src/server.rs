use std::{sync::Arc, time::Duration};

use rmcp::{handler::server::wrapper::Parameters, schemars, tool, tool_router};
use serde::Deserialize;

use crate::{config::Config, runner::SubagentRunner};

const DEFAULT_TIMEOUT_SECS: u64 = 300;

#[derive(Clone)]
pub struct McpServer {
    config: Arc<Config>,
}

impl McpServer {
    pub fn new(config: Arc<Config>) -> Self {
        Self { config }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SubagentRunParams {
    /// Agent type identifier as defined in agents.toml (e.g. "claude-code")
    pub agent_type: String,
    /// The prompt to send to the subagent
    pub prompt: String,
    /// Timeout in seconds (default: 300)
    pub timeout_secs: Option<u64>,
}

#[tool_router(server_handler)]
impl McpServer {
    #[tool(description = "List all configured subagent types with their descriptions.")]
    async fn subagent_list(&self) -> Result<String, rmcp::ErrorData> {
        if self.config.agents.is_empty() {
            return Ok("No agents configured.".to_string());
        }

        let mut lines: Vec<String> = self
            .config
            .agents
            .iter()
            .map(|(name, cfg)| format!("- **{}**: {}", name, cfg.description))
            .collect();
        lines.sort();
        Ok(lines.join("\n"))
    }

    #[tool(
        description = "Run an ACP subagent with a prompt and block until the result is ready. Returns the agent's text response."
    )]
    async fn subagent_run(
        &self,
        Parameters(SubagentRunParams {
            agent_type,
            prompt,
            timeout_secs,
        }): Parameters<SubagentRunParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let agent_config = self
            .config
            .agents
            .get(&agent_type)
            .ok_or_else(|| {
                rmcp::ErrorData::invalid_params(
                    format!("agent type '{agent_type}' not found in config"),
                    None,
                )
            })?
            .clone();

        let timeout = Duration::from_secs(timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
        let runner = SubagentRunner {
            config: agent_config,
        };

        let result = runner
            .run(&prompt, timeout)
            .await
            .map_err(rmcp::ErrorData::from)?;

        let stop_reason = format!("{:?}", result.stop_reason);
        let output = if result.text.is_empty() {
            format!("[stop_reason: {stop_reason}]")
        } else {
            format!("{}\n\n[stop_reason: {stop_reason}]", result.text)
        };

        Ok(output)
    }
}
