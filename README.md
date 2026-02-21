# Squall

MCP server for [Claude Code](https://docs.anthropic.com/en/docs/claude-code) that dispatches prompts to multiple AI models in parallel. ~2000 lines of Rust.

## Why multiple models

No single model finds everything. Different models have different strengths, different blind spots, and different failure modes. When you send the same question to Grok, Gemini, Kimi, Codex, and GLM, you get additive signal — each one catches things the others miss.

This showed up concretely during Squall's own development. A multi-model review of the implementation plan caught three critical gaps: all models flagged the need for path sandboxing, Codex alone caught that 512KB of file context would hit the OS `ARG_MAX` limit on CLI subprocess spawn, and the group identified XML injection risks. No single model raised all three.

Later, we pointed Squall at its own source code. Gemini found three unbounded-buffering bugs — places where the server would read an entire HTTP response or subprocess output into memory before checking size limits. Grok and Kimi then found a symlink traversal vulnerability in the path sandboxing. Different models, different findings, all real bugs.

The pattern held across every review round:

| Model | Strength | Speed |
|-------|----------|-------|
| Gemini | Systems-level bugs, concurrency, resource leaks | 55–184s |
| Codex | Highest precision (0 false positives), exact line refs | 50–300s |
| Grok | Fast triage, obvious bugs | 20–65s |
| GLM | Architectural framing, API design | 75–93s |
| Kimi | Contrarian edge cases, adversarial scenarios | 60–300s |

The redundancy isn't waste — it's the point.

## What it does

Squall is an [MCP](https://modelcontextprotocol.io/) server built for [Claude Code](https://docs.anthropic.com/en/docs/claude-code). It exposes four tools:

- **chat** — Send a prompt to any HTTP model (OpenAI-compatible API). Optionally attach source files that Squall reads server-side and injects into the prompt as XML.
- **clink** — Invoke a CLI agent (Gemini, Codex). Passes the working directory as subprocess cwd so the agent can read code itself, plus a manifest of relevant file paths.
- **review** — Fan out a prompt to multiple models in parallel with a straggler cutoff. Supports per-model system prompts so each model gets a different expertise lens (security, architecture, correctness, etc.). Results persist to `.squall/reviews/` so they survive context compaction.
- **listmodels** — List all registered models with provider and backend info.

All tools support `system_prompt` and `temperature` parameters. The `review` tool additionally supports `per_model_system_prompts` — a map of model name to system prompt that overrides the shared `system_prompt` per model.

The dispatch layer is intentionally simple. Claude Code is the orchestrator — it decides what to ask, which models to query, and how to synthesize the results. Squall handles authenticated transport, file context injection, and parallel fan-out.

### The HTTP blindness problem

HTTP models are stateless text-in/text-out endpoints. They can't see your filesystem. When you ask Grok to review a file, it only sees what you paste into the prompt. CLI models (Gemini, Codex) have filesystem access but need to know where to look.

Squall bridges both gaps. Pass `file_paths` and `working_directory`, and:
- HTTP models get file content injected as XML (budget-capped at 512KB)
- CLI models get the working directory as their subprocess cwd and a manifest of paths to examine

## Models

Squall has two dispatch backends: **HTTP** (OpenAI-compatible chat completions) and **CLI** (subprocess). They use separate auth — CLI models use OAuth/consumer auth at zero cost, HTTP models use API keys.

| Model | Provider | Backend | Auth |
|-------|----------|---------|------|
| `grok-4-1-fast-reasoning` | xAI | HTTP | `XAI_API_KEY` |
| `moonshotai/kimi-k2.5` | OpenRouter | HTTP | `OPENROUTER_API_KEY` |
| `z-ai/glm-5` | OpenRouter | HTTP | `OPENROUTER_API_KEY` |
| `gemini` | Google | CLI | Google OAuth (free) |
| `codex` | OpenAI | CLI | OpenAI auth (free) |

CLI models are auto-detected from PATH. If a model name is misspelled, the error includes "Did you mean: ..." suggestions.

### Deep research (roadmap)

Both Gemini and Codex have deep research capabilities behind paid API keys. These are **not yet integrated** — today, `/squall-deep-research` works by sending research prompts through Codex CLI's normal web search via the existing `clink` tool.

| Capability | API | Key | What's needed |
|------------|-----|-----|---------------|
| Codex deep research | OpenAI chat completions (`o4-mini-deep-research`) | `OPENAI_API_KEY` | New HTTP model entry — fits existing dispatch |
| Gemini deep research | Gemini Interactions API (launch-then-poll) | `GEMINI_API_KEY` | New async-poll dispatch backend |

The architecture keeps these cleanly separated from CLI models. A future `codex-deep-research` HTTP model would use `OPENAI_API_KEY` while the existing `codex` CLI model continues using free consumer auth — different name, different backend, no cross-contamination.

## Setup

```bash
cargo build --release
```

### Environment variables

| Variable | Models |
|----------|--------|
| `XAI_API_KEY` | Grok |
| `OPENROUTER_API_KEY` | Kimi, GLM (any OpenRouter model) |

CLI models (gemini, codex) need their respective CLIs installed and authenticated.

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
- **Process group kill** — Timeouts SIGKILL the entire process tree via `libc::kill(-pgid, SIGKILL)`, not just the leader
- **Capped reads** — HTTP responses streamed with 2MB cap. CLI stdout/stderr capped via `take()`. File context pre-checked via metadata before reading
- **Concurrency limits** — Semaphores cap CLI (4) and HTTP (8) concurrent requests
- **No cascade** — MCP results never set `is_error: true`, preventing Claude Code sibling tool call failures
- **Error sanitization** — User-facing messages never leak internal URLs or connection details

## Architecture

```
Claude Code
    │
    ├─► chat(prompt, model, system_prompt?, temperature?, file_paths?)
    │       │
    │       ├─► HTTP backend: file content injected as XML into prompt
    │       └─► Registry → HttpDispatch → OpenAI-compatible API
    │
    ├─► clink(prompt, cli_name, system_prompt?, temperature?, file_paths?)
    │       │
    │       ├─► CLI backend: path manifest prepended, working_directory as cwd
    │       └─► Registry → CliDispatch → subprocess (gemini/codex)
    │
    ├─► review(prompt, models?, timeout_secs?, per_model_system_prompts?, ...)
    │       │
    │       ├─► Parallel fan-out to N models (HTTP + CLI mixed)
    │       ├─► Straggler cutoff: returns when all finish or timeout expires
    │       └─► Results persisted to .squall/reviews/ and returned inline
    │
    └─► listmodels()
```

## Skills

Squall ships with [Claude Code skills](https://docs.anthropic.com/en/docs/claude-code/skills) — prompt templates that teach Claude how to use the tools effectively:

| Skill | What it does |
|-------|-------------|
| `/squall-review` | Multi-model code review with per-model expertise lenses |
| `/squall-research` | Team-based research swarm — N agents × WebSearch × Squall review |
| `/squall-deep-research` | Deep sourced research via Codex web search |

Skills are markdown files in `.claude/skills/`. They don't change the Rust server — they teach the caller how to wire up tools that already exist.

### Team swarms

The `/squall-research` skill spawns a team of parallel agents, each investigating a different research vector. This requires Claude Code's experimental agent teams feature. To enable it, add to `~/.claude/settings.json`:

```json
{
  "env": {
    "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS": "1"
  }
}
```

Each team member gets full MCP tool access — they can call `listmodels`, `review`, `chat`, and `clink` directly via `ToolSearch`.

## Tests

```bash
cargo test        # 158 tests
cargo clippy      # zero warnings
```
