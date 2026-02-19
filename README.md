# Squall

Fast async MCP server for dispatching prompts to multiple AI models. ~1200 lines of Rust.

## What it does

Squall exposes three MCP tools:

- **chat** — Send a prompt to any HTTP model (Grok, Kimi, GLM via OpenRouter). Optionally attach file context read server-side.
- **clink** — Invoke a CLI agent (Gemini CLI, Codex CLI). Passes a file manifest and working directory so the agent can read code itself.
- **listmodels** — List all registered models with provider and backend info.

HTTP models can't see your filesystem. Squall reads files for them, injects content as XML into the prompt, and enforces a 512KB budget. CLI models get the working directory as their cwd and a manifest of relevant paths.

## Why

Claude Code can call external models through MCP, but HTTP APIs are blind to your codebase and CLI tools don't know where to look. Squall bridges that gap with minimal overhead — no Python runtime, no conversation state, no prompt framework. Just structured dispatch with safety guarantees.

## Setup

```bash
cargo build --release
```

### Environment variables

| Variable | Required | Models |
|----------|----------|--------|
| `XAI_API_KEY` | For Grok | grok-4-1-fast-reasoning |
| `OPENROUTER_API_KEY` | For OpenRouter | moonshotai/kimi-k2.5, z-ai/glm-5 |

CLI models (gemini, codex) are auto-detected from PATH. No API key needed for Gemini CLI (uses Google OAuth).

### Claude Code MCP config

Add to `~/.claude.json`:

```json
{
  "mcpServers": {
    "squall": {
      "command": "/path/to/squall/target/release/squall",
      "env": {
        "XAI_API_KEY": "...",
        "OPENROUTER_API_KEY": "..."
      }
    }
  }
}
```

## Safety

- **Path sandboxing** — Rejects absolute paths, `..` traversal, and symlink escapes (canonicalize + starts_with)
- **No shell** — CLI dispatch uses `Command::new` + discrete args, no shell interpolation
- **Process group kill** — Timeouts SIGKILL the entire process tree, not just the leader
- **Capped reads** — HTTP responses streamed with 2MB cap. CLI stdout/stderr capped via `take()`. File context pre-checked via metadata
- **Concurrency limits** — Semaphores cap CLI (4) and HTTP (8) concurrent requests
- **No cascade** — MCP results never set `is_error: true`, preventing Claude Code sibling tool call failures
- **Error sanitization** — User-facing messages never leak internal URLs or connection details

## Architecture

```
Claude Code
    │
    ├─► chat(prompt, model, file_paths?, working_directory?)
    │       │
    │       ├─► HTTP backend: file content injected as XML into prompt
    │       └─► Registry → HttpDispatch → OpenAI-compatible API
    │
    ├─► clink(prompt, cli_name, file_paths?, working_directory?)
    │       │
    │       ├─► CLI backend: path manifest prepended, working_directory as cwd
    │       └─► Registry → CliDispatch → subprocess (gemini/codex)
    │
    └─► listmodels()
```

## Tests

```bash
cargo test        # 83 tests
cargo clippy      # zero warnings
```
