use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::error::AppError;

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub agents: HashMap<String, AgentConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    /// Human-readable description of what this agent does (required).
    pub description: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub system_prompt: Option<String>,
}

impl Config {
    /// Defaults to ~/.config/acp-subagents-mcp/agents.toml
    pub fn load_default() -> Result<Self, AppError> {
        let path = default_config_path()?;
        Self::load(&path)
    }

    pub fn load(path: &Path) -> Result<Self, AppError> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| AppError::Config(format!("failed to read {}: {}", path.display(), e)))?;
        toml::from_str(&contents)
            .map_err(|e| AppError::Config(format!("failed to parse {}: {}", path.display(), e)))
    }
}

fn default_config_path() -> Result<PathBuf, AppError> {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("USERPROFILE").map(PathBuf::from))
        .map_err(|_| AppError::Config("cannot determine home directory".into()))?;

    Ok(home
        .join(".config")
        .join("acp-subagents-mcp")
        .join("agents.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config() {
        let toml = r#"
[agents.explore-agent]
description = "Fast agent specialized for exploring codebases."
command = "qodercli"
args = ["--acp", "--model", "bailian/minimax-m2.5-cp"]
system_prompt = "You are a helpful assistant."

[agents.dummy]
description = "Dummy minimal agent."
command = "dummycli"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.agents.len(), 2);

        let explore = &config.agents["explore-agent"];
        assert_eq!(explore.description, "Fast agent specialized for exploring codebases.");
        assert_eq!(explore.command, "qodercli");
        assert_eq!(explore.args, vec!["--acp", "--model", "bailian/minimax-m2.5-cp"]);
        assert_eq!(
            explore.system_prompt.as_deref(),
            Some("You are a helpful assistant.")
        );

        let dummy = &config.agents["dummy"];
        assert_eq!(dummy.command, "dummycli");
        assert!(dummy.args.is_empty());
        assert!(dummy.system_prompt.is_none());
    }

    #[test]
    fn test_default_config_path() {
        let path = default_config_path().unwrap();
        assert!(path.ends_with("acp-subagents-mcp/agents.toml"));
        assert!(path.to_string_lossy().contains(".config"));
    }
}
