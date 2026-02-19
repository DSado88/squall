# Squall

Fast async MCP server for dispatching prompts to multiple AI models. ~1200 lines of Rust.

## Why multiple models

No single model finds everything. Different models have different strengths, different blind spots, and different failure modes. When you send the same question to Grok, Gemini, Kimi, and Codex, you get additive signal — each one catches things the others miss.

This showed up concretely during Squall's own development. A 6-model consensus review of the implementation plan caught three critical gaps: all 6 flagged the need for path sandboxing, Codex alone caught that 512KB of file context would hit the OS `ARG_MAX` limit on CLI subprocess spawn, and the group identified XML injection risks. No single model raised all three.

Later, we pointed Squall at its own source code. Gemini (reading files as a CLI subprocess) found three unbounded-buffering bugs — places where the server would read an entire HTTP response or subprocess output into memory before checking size limits. Grok and Kimi then found a symlink traversal vulnerability in the path sandboxing. Different models, different findings, all real bugs.

The pattern held across every review: Claude is good at architecture, Gemini at filesystem-level systems issues, Codex at OS-level constraints, Grok at fast logical analysis (with occasional false positives), Kimi at edge cases. The redundancy isn't waste — it's the point.

## What it does

Squall is an MCP server that Claude Code calls as a tool. It exposes three operations:

- **chat** — Send a prompt to an HTTP model (Grok, Kimi, GLM). Optionally attach source files that Squall reads server-side and injects into the prompt as XML.
- **clink** — Invoke a CLI agent (Gemini, Codex). Passes the working directory as subprocess cwd so the agent can read code itself, plus a manifest of relevant file paths.
- **listmodels** — List all registered models with provider and backend info.

The dispatch layer is intentionally simple. Claude Code is the orchestrator — it decides what to ask, which models to query, and how to synthesize the results. Squall just handles authenticated transport and file context injection.

### The HTTP blindness problem

HTTP models are stateless text-in/text-out endpoints. They can't see your filesystem. When you ask Grok to review a file, it only sees what you paste into the prompt. CLI models (Gemini, Codex) have filesystem access but need to know where to look.

Squall bridges both gaps. Pass `file_paths` and `working_directory`, and:
- HTTP models get file content injected as XML (budget-capped at 512KB)
- CLI models get the working directory as their subprocess cwd and a manifest of paths to examine

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
- **Capped reads** — HTTP responses streamed with 2MB cap. CLI stdout/stderr capped via `take()`. File context pre-checked via metadata before reading
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
