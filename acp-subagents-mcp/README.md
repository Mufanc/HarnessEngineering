# acp-subagents-mcp

An MCP (Model Context Protocol) server that provides subagent capabilities to MCP clients via the [ACP (Agent Client Protocol)](https://agentclientprotocol.com/get-started/introduction).

## Motivation

Many AI coding tools (such as Claude Code, Cursor, etc.) support spawning subagents, but only at the top level — a subagent cannot spawn further subagents of its own. This prevents building deeper agent hierarchies where a subagent needs to delegate subtasks just as the parent agent does.

This MCP server works around that limitation. Because it is exposed as a standard MCP tool, it is available to any agent that has MCP tool access — including subagents. Any agent in the hierarchy can call `subagent_run` to delegate work further down, regardless of whether the host application natively supports nested subagents.

A secondary benefit: subagents help preserve the parent agent's context window. The parent only receives the final result, not the subagent's intermediate reasoning and tool call history.

## Tools

| Tool | Description |
|------|-------------|
| `subagent_list` | List all configured subagent types with their descriptions |
| `subagent_run` | Run a subagent with a prompt and block until the result is ready |

### `subagent_run` parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `agent_type` | string | yes | Agent identifier as defined in `agents.toml` |
| `prompt` | string | yes | The task to send to the subagent |
| `timeout_secs` | integer | no | Timeout in seconds (default: 300) |

## Installation

```bash
cargo install --path .
```

## Configuration

Default config path: `~/.config/acp-subagents-mcp/agents.toml`

Use `--config <path>` to override.

### agents.toml format

```toml
[agents.<name>]
description = "What this agent does (shown to the LLM via subagent_list)."
command = "/path/to/acp-agent-binary"
args = ["--optional", "--flags"]
system_prompt = "Optional system prompt prepended to every request."

[agents.<name>.env]
# Optional: static environment variables passed to the subprocess
API_KEY = "..."
```

### Example

```toml
[agents.claude-code]
description = "Full-featured coding agent. Use for file edits, refactors, and multi-step coding tasks."
command = "claude"
args = ["--acp"]
system_prompt = "Return only the final result. Be concise."
```

## Usage with MCP clients

### Qoder

Add to your Qoder MCP settings:

```json
{
  "mcpServers": {
    "subagents": {
      "command": "acp-subagents-mcp",
      "args": []
    }
  }
}
```

### With a custom config path

```json
{
  "mcpServers": {
    "subagents": {
      "command": "acp-subagents-mcp",
      "args": ["--config", "/path/to/agents.toml"]
    }
  }
}
```

## Logging

Logs are written to stderr. Set `RUST_LOG` to control verbosity:

```bash
RUST_LOG=debug acp-subagents-mcp
```

For design decisions and future plans, see [DESIGN.md](./DESIGN.md).
