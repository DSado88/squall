# Squall

MCP server that dispatches prompts to multiple AI models in parallel. Built in Rust for [Claude Code](https://docs.anthropic.com/en/docs/claude-code).

## Why multiple models

No single model finds everything. Different models have different strengths, different blind spots, and different failure modes. When you send the same review to five models, you get additive signal — one catches concurrency bugs, another spots auth gaps, a third finds edge cases in error handling.

The overlap gives confidence. The divergence gives coverage. One model consistently finds resource leaks while another catches configuration gaps neither would find alone. The redundancy isn't waste — it's the point.

Squall tracks every model's performance — latency, success rate, failure modes — and Claude uses those metrics to pick the best ensemble for each review. A model that keeps timing out gets benched. A model that shines on security-sensitive code gets picked when auth files change. The selection adapts over time.

## Quick start

### Build

```bash
cargo build --release
```

### Configure Claude Code

Add to `~/.claude.json`:

```json
{
  "mcpServers": {
    "squall": {
      "command": "/path/to/squall/target/release/squall",
      "env": {
        "TOGETHER_API_KEY": "...",
        "XAI_API_KEY": "..."
      }
    }
  }
}
```

### API keys

| Variable | Unlocks |
|----------|---------|
| `TOGETHER_API_KEY` | Kimi K2.5, DeepSeek R1, DeepSeek V3, Qwen 3.5, Qwen3 Coder |
| `XAI_API_KEY` | Grok |
| `OPENROUTER_API_KEY` | GLM-5 |
| `MISTRAL_API_KEY` | Mistral Large |
| `OPENAI_API_KEY` | o3-deep-research, o4-mini-deep-research |
| `GOOGLE_API_KEY` | deep-research-pro |

CLI models (gemini, codex) use their own OAuth — no API key needed. Install and authenticate the [Gemini CLI](https://github.com/google-gemini/gemini-cli) and [Codex CLI](https://github.com/openai/codex) separately.

Models only appear in `listmodels` when their API key is set (HTTP) or their CLI is installed (CLI). Set what you have, skip what you don't.

### Verify

Ask Claude Code: *"list the available squall models"*. If Squall is connected, it will call `listmodels` and show what's available.

## Tools

Squall exposes seven tools to Claude Code.

### review

The flagship tool. Fan out a prompt to multiple models in parallel. Each model can get a different expertise lens via `per_model_system_prompts` — one focused on security, another on correctness, another on architecture.

Returns when all models finish or the straggler cutoff fires (default 180s). Models that don't finish in time return partial results. Results persist to `.squall/reviews/` so they survive context compaction — if Claude's context window resets, the `results_file` path still works.

Key parameters:
- `models` — which models to query (defaults to config if omitted)
- `per_model_system_prompts` — map of model name to expertise lens
- `deep: true` — raises timeout to 600s, reasoning effort to high, max tokens to 16384
- `diff` — unified diff text to include in the prompt
- `file_paths` + `working_directory` — source files injected as context

Models with less than 70% success rate (over 5+ reviews) are automatically excluded by a hard gate. This prevents known-broken models from wasting dispatch slots.

### chat

Query a single model via HTTP (OpenAI-compatible API). Pass `file_paths` and `working_directory` to inject source files as context. Good for one-off questions to a specific model.

### clink

Query a single CLI model (gemini, codex) as a subprocess. The model gets filesystem access via its native CLI — it can read your code directly. Useful when you need a model that can see the full project, not just the files you pass.

### listmodels

List all available models with metadata: provider, backend, speed tier, precision tier, strengths, and weaknesses. Call this before `review` to see what's available.

### memorize

Save a learning to persistent memory. Three categories:

- **pattern** — a recurring finding across reviews (e.g., "JoinError after abort silently drops panics")
- **tactic** — a prompt strategy that works (e.g., "Kimi needs a security lens to find real bugs")
- **recommend** — a model recommendation (e.g., "deepseek-v3.1 is fastest for Rust reviews")

Duplicate patterns auto-merge with evidence counting. Patterns reaching 5 occurrences get confirmed status. Scoped to branch or codebase, auto-detected from git context.

### memory

Read persistent memory. Returns model performance stats, recurring patterns, proven prompt tactics, or model recommendations with recency-weighted confidence scores. Call this before reviews to inform model selection and lens assignment.

### flush

Clean up branch-scoped memory after a PR merge. Graduates high-evidence patterns to codebase scope, archives the rest, and prunes model events older than 30 days.

## Models

Three dispatch backends: **HTTP** (OpenAI-compatible), **CLI** (subprocess, free), and **async-poll** (deep research, launch-then-poll).

| Model | Provider | Backend | Speed | Best for |
|-------|----------|---------|-------|----------|
| `grok` | xAI | HTTP | fast | Quick triage, broad coverage |
| `gemini` | Google | CLI (free) | medium | Systems-level bugs, concurrency |
| `codex` | OpenAI | CLI (free) | medium | Highest precision, zero false positives |
| `kimi-k2.5` | Together | HTTP | medium | Edge cases, adversarial scenarios |
| `deepseek-v3.1` | Together | HTTP | medium | Strong coder, finds real bugs |
| `deepseek-r1` | Together | HTTP | medium | Deep reasoning, logic-heavy analysis |
| `qwen-3.5` | Together | HTTP | medium | Pattern matching, multilingual |
| `qwen3-coder` | Together | HTTP | medium | Purpose-built for code review |
| `z-ai/glm-5` | OpenRouter | HTTP | medium | Architectural framing |
| `mistral-large` | Mistral | HTTP | fast | Efficient, multilingual |
| `o3-deep-research` | OpenAI | async-poll | minutes | Deep web research |
| `o4-mini-deep-research` | OpenAI | async-poll | minutes | Faster deep research |
| `deep-research-pro` | Google | async-poll | minutes | Google-powered deep research |

All models are configurable via TOML. Add your own models, swap providers, or override defaults.

## Configuration

Squall uses a three-layer TOML config system. Later layers override earlier ones:

1. **Built-in defaults** — 13 models, 5 providers, shipped with the binary
2. **User config** (`~/.config/squall/config.toml`) — personal overrides
3. **Project config** (`.squall/config.toml`) — project-specific settings

### Adding a custom model

```toml
[providers.custom]
base_url = "https://my-api.example.com/v1/chat/completions"
api_key_env = "CUSTOM_API_KEY"

[models.my-model]
provider = "custom"
backend = "http"
description = "My custom model"
speed_tier = "fast"
strengths = ["domain expertise"]
```

### Review defaults

When `models` is omitted from a `review` call, Squall dispatches to these defaults:

```toml
[review]
default_models = ["gemini", "codex", "grok"]
```

Override in your user or project config to change the default ensemble.

## Memory

Squall learns from every review and uses what it learns to make better decisions next time.

Three files in `.squall/memory/`:

- **models.md** — Per-model performance stats (latency, success rate, common failures). Updated automatically after every review. Claude reads this before each review to pick models, and Squall's hard gate uses it to auto-exclude models below 70% success rate.

- **patterns.md** — Recurring findings across reviews with evidence counting. Patterns found by multiple models in multiple reviews get confirmed status. Capped at 50 entries with automatic pruning.

- **tactics.md** — Proven system prompts and model+lens combinations. Claude reads this to assign the right expertise lens to each model — e.g., "Kimi performs best with a security-focused lens on Rust code."

### The learning loop

1. **Before review** — Claude calls `memory` to check which models are performing well, which lenses work, and what patterns keep recurring. This drives model selection and prompt assignment.
2. **After review** — Claude calls `memorize` to record what worked: which model found what, which lens was effective, which model missed obvious things.
3. **After PR merge** — call `flush` with the branch name. Graduates high-evidence patterns to codebase scope, archives the rest.

The result: reviews get better over time. Models that consistently fail get excluded. Lens assignments that produce good results get reused. The system adapts without manual tuning.

## Skills

Squall ships with [Claude Code skills](https://docs.anthropic.com/en/docs/claude-code/skills) — prompt templates that teach Claude how to orchestrate the tools. You trigger them with natural language or slash commands:

| You say | Skill | What happens |
|---------|-------|-------------|
| "review", "review this diff", "code review" | `squall-unified-review` | Auto-depth code review — Claude scores the diff and picks the right depth |
| "deep review", "thorough review" | `squall-unified-review` | Forces DEEP depth — full investigation + more models + longer timeouts |
| "quick review", "quick check" | `squall-unified-review` | Forces QUICK depth — single fast model, instant triage |
| "research [topic]" | `squall-research` | Team swarm — multiple agents investigating different vectors in parallel |
| "deep research [question]" | `squall-deep-research` | Web-sourced research via Codex and Gemini deep research |

### Auto-depth review

Claude automatically picks the right review intensity based on what changed:

| Depth | When | Models | What's different |
|-------|------|--------|-----------------|
| **QUICK** | Small non-critical changes | 1 (grok) | Fast triage, no parallel dispatch |
| **STANDARD** | Normal PRs | 5 (3 core + 2 picked by memory stats) | Per-model lenses, Opus agent for local investigation |
| **DEEP** | Security, auth, critical infra | 5+ models, deep mode | Claude investigates first, forms hypotheses, then models + Opus validate in parallel |

Claude reads `memory` before each review to check model success rates, proven tactics, and recurring patterns — then picks the best ensemble for this specific diff. You can always override: "deep review" forces DEEP, "quick review" forces QUICK.

Skills are markdown files in `.claude/skills/`. They teach Claude how to use the tools — they don't change the server.

Team swarms require Claude Code's experimental agent teams feature:

```json
{
  "env": {
    "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS": "1"
  }
}
```

## How it works

```
Claude Code (orchestrator)
    |
    +-- review -----> fan out to N models in parallel
    |                   |-- HTTP models get file content injected as context
    |                   |-- CLI models get filesystem access via subprocess
    |                   +-- straggler cutoff returns partial results for slow models
    |
    +-- memory/memorize/flush --> .squall/memory/ (learning loop)
    |
    +-- chat/clink --> single model query
    |
    +-- listmodels --> model discovery with metadata
```

Claude is the intelligence. Squall is transport + memory. Claude decides what to ask, which models to query, and how to synthesize results. Squall handles authenticated dispatch, file context injection, parallel fan-out, and persistent learning.

## Safety

- **Path sandboxing** — rejects absolute paths, `..` traversal, and symlink escapes
- **No shell** — CLI dispatch uses direct exec with discrete args, no shell interpolation
- **Process group kill** — timeouts kill the entire process tree, not just the leader
- **Capped reads** — HTTP responses: 2MB. CLI output: capped. File context: pre-checked via metadata
- **Concurrency limits** — semaphores: 8 HTTP, 4 CLI, 4 async-poll
- **No cascade errors** — MCP results never set `is_error: true`, preventing Claude Code sibling tool failures
- **Error sanitization** — user-facing messages never leak internal URLs or credentials

## Contributing

### Setup

```bash
cargo build
cargo test
cargo clippy --all-targets
```

All tests must pass. Zero clippy warnings.

### Adding a model

**To the built-in defaults** — add a `[models.name]` entry to `BUILTIN_DEFAULTS` in `src/config.rs`. HTTP models need a provider with `base_url` and `api_key_env`. CLI models need a parser in `src/dispatch/cli.rs`.

**For personal use** — add to `~/.config/squall/config.toml`. Same TOML format, no code changes needed.

### Pull requests

- One feature per PR
- Tests for new behavior
- `cargo test && cargo clippy --all-targets` clean before submitting
