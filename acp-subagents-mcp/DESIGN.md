# Design Notes

Technical decisions and architectural findings recorded during development.

---

## v1: Current Capabilities

- `subagent_run` — spawn a subagent, send a prompt, block until result is returned, kill the process
- `subagent_list` — list all configured agent types with their descriptions
- Config via `agents.toml` with fields: `description`, `command`, `args`, `env`, `system_prompt`
- Per-call `timeout_secs` parameter
- System prompt injection: prepended as the first content block before the user prompt

---

## Recursion Depth Limiting

To prevent subagents from infinitely spawning further subagents, we use a CLI flag `--max-depth` combined with an environment variable `AGENT_DEPTH` to propagate the remaining depth across process boundaries.

**How it works:**

- The root `acp-subagents-mcp` process is started with `--max-depth N`
- When spawning a subagent subprocess, it injects `AGENT_DEPTH=N-1` into the child's environment
- Each layer reads `AGENT_DEPTH` from its environment to know its remaining budget
- When `AGENT_DEPTH=0`, `subagent_run` refuses to execute and returns an error immediately
- If `--max-depth` is not specified, `AGENT_DEPTH` is never injected and no limit is enforced

This approach requires no changes to agent configs or prompts, and works transparently regardless of how deeply nested the call chain is.

---

## Session Isolation for Multi-Agent Scenarios

When a future `subagent_list` / `subagent_status` tool is added, each agent should only be able to see sessions it created — not sessions belonging to other agents running concurrently.

**Finding:** rmcp's behavior depends on the transport:

- **Stdio transport** (current): one process = one connection = one handler instance. Isolation is trivially guaranteed — there is no other client to leak to.

- **Streamable HTTP transport**: rmcp calls a `service_factory` closure to create a **fresh handler instance per client connection**. As long as the session registry lives inside the `McpServer` instance (not in a global `Arc<Mutex<...>>`), each client connection gets its own isolated registry with zero extra effort.

**Conclusion:** Keep session state as instance fields on `McpServer`, not global state. Both transport modes then provide natural isolation for free.

---

## Inactivity Timeout (`--timeout`)

The existing per-call `timeout_secs` parameter measures total wall-clock time. This is a blunt instrument: a legitimate long-running task and a completely stuck agent look identical.

A separate `--timeout N` CLI flag introduces an **inactivity timeout**: if no meaningful activity signal is received from the subagent within N seconds, the call is aborted and the agent process is killed.

**What counts as activity:**

Any `SessionNotification` carrying a substantive `SessionUpdate` — such as a text chunk, a tool call event, or an MCP call — resets the timer. Protocol-level heartbeats do not count, because a network-layer hang can still allow heartbeats to pass while the agent has effectively stopped making progress.

**Rationale:**

With this in place, the MCP client-side timeout can be set to infinity. `acp-subagents-mcp` itself becomes responsible for detecting stuck agents and terminating them, cleanly separating "task is slow but alive" from "task is hung".

### v2: Batch Synchronous Execution

Add a `subagent_run_batch` tool that accepts a list of tasks, runs all subagents concurrently, and waits for all of them to complete before returning. This keeps the interface simple — one call in, one structured result out — while achieving parallelism internally.

A task that fails does not cancel the others. All results (including errors) are collected and returned together once every task has either completed or timed out.

This combined with the existing `subagent_run` covers the majority of use cases without introducing async state management.

### v3: Async Session Management

For long-running tasks where batch-synchronous is too slow or too expensive, a more capable tool set:

| Tool | Description |
|------|-------------|
| `subagent_start` | Launch a subagent without waiting, returns a session ID |
| `subagent_wait` | Wait on a list of session IDs; returns as soon as any completes, or on timeout with a lightweight status summary |
| `subagent_result` | Retrieve the result of a completed session by ID |
| `subagent_list` | List currently running sessions (scoped to the current connection) |

Completed results are buffered in memory so that a session finishing while no one is polling does not lose its output.

To avoid the parent agent wasting context on repeated poll→timeout→poll cycles, `subagent_wait` accepts multiple session IDs and uses `select!` internally — the parent makes one call per "round" regardless of how many subagents are running.

### v4: Automatic Progress Summarization

When `subagent_wait` times out, automatically invoke a lightweight summarizer agent (no tools, minimal context) to condense the partial output collected so far. The summary is returned directly as the timeout response, so the parent agent never sees raw intermediate output and token consumption stays bounded.
