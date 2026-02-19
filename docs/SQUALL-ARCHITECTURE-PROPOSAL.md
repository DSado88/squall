# Squall: Rust MCP Server Architecture Proposal

**Date:** 2026-02-19
**Source:** Multi-model research session — Claude Opus 4.6 (lead), Codex GPT5.3, Gemini 3 Pro Preview, 3x Opus subagents
**Context:** Informed by CLAUDE-ORCHESTRATION-DEEP-DIVE.md, ZEN-USAGE-ANALYSIS.md, PAL-ARCHITECTURE-REVIEW.md

---

## Executive Summary

Replace the 80K-line Python PAL MCP server with a ~2.5K-line Rust binary (`squall`) that acts as a fast, honest async dispatcher for external AI model calls. Move all intelligence (system prompts, consensus synthesis, conversation memory, workflow orchestration) to Claude Code, which already handles it via slash commands, subagents, and agent teams. The server becomes a dumb-but-fast pipe. True parallelism via tokio replaces PAL's fake async, cutting consensus latency from 2-3 minutes to 10-20 seconds.

The key constraint driving the design: David pays for Codex and Gemini CLI subscriptions (usage included), so the server must support CLI subprocess dispatch alongside direct HTTP — not as a fallback, but as the primary path for 2 of 5 external models.

---

## Part 1: Why This Makes Sense

### What David Actually Uses

From the usage analysis (367 projects, 1,630+ conversation files), ~90% of zen invocations are three tools: `chat`, `clink`, and consensus via the `/consensus` slash command. The other 15 tools (codereview, secaudit, debug, analyze, etc.) are essentially chat with different system prompts — prompts that Claude is already better at applying in its own context window.

### What PAL Actually Does vs. What's Needed

| PAL capability | LOC (est.) | Needed in Squall? |
|---|---|---|
| 18 tool implementations with system prompts | ~15K | No — Claude applies prompts via slash commands/skills |
| Conversation memory (continuation_id, threads) | ~1,600 | No — Claude's context window handles this |
| Workflow orchestration (multi-step tools) | ~3K | No — Claude subagents/teams handle this |
| Provider routing + HTTP calls | ~4,400 | Yes — this is the core job |
| Model resolution + registries | ~650 | Yes — simplified to O(1) HashMap |
| Error handling + retry | ~200 | Yes — improved with Retry-After support |
| CLink CLI subprocess management | ~800 | Yes — first-class, not optional |
| Config, utils, security | ~2K | Partially — simplified |
| **Total** | **~80K** | **~2.5K Rust** |

### The Architectural Inversion

PAL tries to be smart in a place where Claude is already smarter. The slash command migration proved this — David replaced `mcp__zen__consensus` (a multi-step tool with internal orchestration) with a Claude-orchestrated slash command that dispatches `chat`/`clink` calls directly. The tool was doing work that Claude does better with full conversation context.

Squall completes this inversion: the MCP server handles transport (HTTP requests, subprocess spawning, response collection), and Claude handles intelligence (prompt engineering, synthesis, orchestration).

---

## Part 2: Architecture

### High-Level Design

```
Claude Code
    │ stdio (MCP JSON-RPC)
    ▼
squall (~2.5K LOC, Rust + tokio)
    ├── Dual dispatch backends:
    │   ├── CLI subprocess (tokio::process)
    │   │   ├── gemini-cli  ── paid subscription, usage included
    │   │   └── codex-cli   ── paid subscription, usage included
    │   └── HTTP (reqwest)
    │       ├── Grok        ── xAI API (api.x.ai)
    │       ├── Kimi        ── OpenRouter (moonshotai/kimi-k2.5)
    │       └── GLM         ── OpenRouter (z-ai/glm-5)
    │
    └── Claude Opus handled separately as Claude Code subagent (Task tool)
```

### Model Routing

| Model | Backend | Route | Auth |
|---|---|---|---|
| Gemini 3 Pro Preview | CLI subprocess | `gemini -p ... --output-format json` | Paid subscription (OAuth) |
| Codex GPT5.3 | CLI subprocess | `codex exec --json` | Paid subscription |
| Grok 4.1 Fast Reasoning | HTTP (reqwest) | `api.x.ai/v1/chat/completions` | Bearer token (`XAI_API_KEY`) |
| Kimi K2.5 | HTTP (reqwest) | OpenRouter `/chat/completions` | Bearer token (`OPENROUTER_API_KEY`) |
| GLM-5 | HTTP (reqwest) | OpenRouter `/chat/completions` | Bearer token (`OPENROUTER_API_KEY`) |
| Claude Opus 4.6 | Not in squall | Claude Code Task agent (subagent) | Native |

### Crate Structure

```
squall/
├── Cargo.toml
├── src/
│   ├── main.rs              ── MCP server setup via rmcp 0.16, stdio transport, signal handling
│   ├── tools/
│   │   ├── mod.rs
│   │   ├── chat.rs           ── query single model (compat tool name: "chat")
│   │   ├── parallel.rs       ── query N models concurrently (new: "query_parallel")
│   │   ├── clink.rs          ── CLI subprocess bridge (compat tool name: "clink")
│   │   └── listmodels.rs     ── model catalog (compat tool name: "listmodels")
│   ├── dispatch/
│   │   ├── mod.rs
│   │   ├── contract.rs       ── ProviderRequest / ProviderResult types
│   │   ├── http.rs           ── reqwest backend (OpenAI-compatible: Grok, OpenRouter)
│   │   ├── cli.rs            ── subprocess backend (Gemini CLI, Codex CLI)
│   │   └── registry.rs       ── HashMap<String, ProviderConfig>, O(1) lookup
│   ├── parsers/
│   │   ├── mod.rs
│   │   ├── gemini.rs         ── parse gemini --output-format json
│   │   └── codex.rs          ── parse codex --json JSONL events
│   ├── error.rs              ── ProviderError enum, retryable classification
│   ├── retry.rs              ── per-provider backoff with Retry-After header support
│   └── config.rs             ── env vars, model catalog, provider configs
└── tests/
    ├── fixtures/             ── captured CLI output samples, HTTP responses
    ├── cli_contract_tests.rs ── parse real Gemini/Codex output fixtures
    ├── http_mock_tests.rs    ── wiremock: retry, 429, 5xx, malformed JSON
    └── mcp_e2e_tests.rs      ── tools/list + tools/call snapshots
```

### Internal Contract (Prevents Backend Leakage)

Both subprocess and HTTP backends return the same types. Tool handlers never see which backend was used.

```rust
struct ProviderRequest {
    prompt: String,
    model: String,
    deadline: Instant,
    max_chars: usize,
    trace_id: String,
}

struct ProviderResult {
    provider: String,
    status: ResultStatus,       // Success | Error
    text: Option<String>,
    latency_ms: u64,
    retry_count: u32,
    error_kind: Option<ErrorKind>,
}

enum ErrorKind {
    Timeout,
    RateLimited,
    SchemaParse,      // CLI output format changed
    ProcessExit(i32), // CLI non-zero exit
    Upstream5xx,
    ContentFiltered,
    ContextLengthExceeded,
    AuthFailed,
    Unknown(String),
}
```

---

## Part 3: Tool Compatibility

### The Naming Constraint

The ecosystem is hardwired to `mcp__zen__*` tool names with `prompt=` parameter semantics. This is baked into:

- `~/.claude/commands/consensus.md` — routes to `mcp__zen__chat` and `mcp__zen__clink`
- ori-v2's `metadata.rs`, `mcp_permissions.rs`, `zen_validator.rs` (6 Rust source files)
- ori-v2's `consensus.rs` reads model routing from the slash command at runtime
- Every slash command and skill that references zen tools

**Decision:** Ship with compatible tool names. The MCP server name stays `zen`. Tools are `chat`, `clink`, `listmodels` (appearing as `mcp__zen__chat`, etc.). Add `query_parallel` as a new tool alongside the existing names.

### Tool Specifications

**`chat`** — Query a single model
```
Parameters: { prompt: String, model?: String }
Returns: { status, content, provider, model, latency_ms }
```

**`clink`** — Invoke a CLI agent
```
Parameters: { prompt: String, cli_name: "gemini" | "codex", role?: String }
Returns: { status, content, provider, cli_name, latency_ms }
```

**`listmodels`** — List available models
```
Parameters: {}
Returns: { models: [{ name, provider, backend, context_window }] }
```

**`query_parallel`** (new) — Query N models concurrently
```
Parameters: {
    prompt: String,
    models: String[],
    max_chars_per_response?: u32,  // default 3000
    min_successes?: u32,           // default 1
    deadline_ms?: u32              // default 30000
}
Returns: {
    overall_status: "success" | "partial" | "failed",
    succeeded: u32,
    failed: u32,
    results: { [model]: ProviderResult }
}
```

### MCP Tool Annotations

All tools annotated with `readOnlyHint: true` to enable parallel dispatch from Claude Code. In rmcp 0.16, this is `annotations(read_only_hint = true)` (snake_case in the macro, mapped to protocol's `readOnlyHint`).

---

## Part 4: Concurrency Model

### How Parallel Dispatch Works

When Claude Code emits multiple `tool_use` blocks in one message (e.g., 6 calls from the `/consensus` slash command), and tools have `readOnlyHint: true`:

1. Claude Code sends each as a separate JSON-RPC request over stdin (MCP removed JSON-RPC batch support in revision 2025-03-26)
2. For `readOnlyHint: true` tools, Claude Code does NOT wait for one response before sending the next
3. Squall reads JSON-RPC messages from stdin in a loop, dispatches each to a separate tokio task
4. Responses are written back to stdout (serialized via mutex — one write at a time)
5. IPC overhead is ~5ms per call, negligible vs 5-30s API latency

### Inside `query_parallel`

```rust
// Pseudocode
async fn query_parallel(models: Vec<String>, prompt: &str, deadline: Duration) -> ParallelResult {
    let futures: Vec<_> = models.iter().map(|model| {
        let backend = registry.get(model);
        let req = ProviderRequest { prompt, model, deadline, ... };
        async move {
            match timeout(deadline, backend.query(req)).await {
                Ok(Ok(result)) => result,
                Ok(Err(e)) => ProviderResult::error(model, e),
                Err(_) => ProviderResult::error(model, ErrorKind::Timeout),
            }
        }
    }).collect();

    // join_all, NOT try_join! — try_join cancels remaining futures on first Err
    let results = futures::future::join_all(futures).await;
    ParallelResult::from_results(results)
}
```

Critical: uses `join_all` (waits for all) not `try_join!` (cancels on first error). One slow model timeout must not kill responses already received.

### Latency Comparison

| Scenario | PAL (current) | Squall |
|---|---|---|
| Single model query | 5-30s (model) + ~0ms (overhead) | 5-30s (model) + ~0ms (overhead) |
| 5-model consensus | 5 x 5-30s sequential = **25-150s** | max(5-30s) parallel = **5-30s** |
| 5-model consensus (one slow) | Blocked by slowest, sequentially | Bounded by deadline, partial return |

The improvement is 3-5x for consensus — the single most-used and most-bottlenecked workflow.

---

## Part 5: CLI Subprocess Management

### Why Subprocess Dispatch Is Non-Optional

David pays for Codex and Gemini subscriptions. CLI usage is included in the subscription. Direct API calls to Gemini or OpenAI would be paying twice — subscription fee plus per-token API charges. 2 of 5 external models go through subprocess spawning. This is day-1 core infrastructure, not a nice-to-have.

### Structured Output Modes

Don't parse freeform text. Both CLIs have structured output:

- **Gemini CLI**: `gemini -p "prompt" --output-format json` — headless JSON mode
- **Codex CLI**: `codex exec --json --dangerously-bypass-approvals-and-sandbox` — JSONL events

Pin minimum CLI versions and run a startup self-check probe (`--version` + tiny no-op prompt) to catch breaking format changes before they bite mid-consensus.

### Known CLI Output Drift Risks

Codex CLI has documented output format instability:
- [openai/codex#4776](https://github.com/openai/codex/issues/4776) — JSON output drift
- [openai/codex#5773](https://github.com/openai/codex/issues/5773) — exec-mode bugs
- [openai/codex#6717](https://github.com/openai/codex/issues/6717) — event format changes

Version-gate parsers and fail with explicit "unsupported CLI version/output schema" errors rather than silently misparse.

### Process Management

```rust
async fn invoke_cli(binary: &str, args: &[String], prompt: &str, timeout: Duration)
    -> Result<String, ProviderError>
{
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0)   // new process group — gemini-cli spawns Node.js children
        .spawn()?;

    // Write prompt to stdin, close to signal EOF
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(prompt.as_bytes()).await?;
    drop(stdin);

    // Read stdout + stderr in parallel tasks (avoids pipe buffer deadlock)
    let stdout_handle = child.stdout.take().unwrap();
    let stderr_handle = child.stderr.take().unwrap();
    let stdout_task = tokio::spawn(read_to_string(stdout_handle));
    let stderr_task = tokio::spawn(read_to_string(stderr_handle));

    // Wait with timeout
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => {
            let stdout = stdout_task.await??;
            let stderr = stderr_task.await??;
            if status.success() {
                parse_cli_output(binary, &stdout)
            } else {
                Err(ProviderError::ProcessExit(status.code().unwrap_or(-1)))
            }
        }
        Ok(Err(e)) => Err(ProviderError::from(e)),
        Err(_) => {
            // Timeout: SIGTERM first, grace period, then SIGKILL the process group
            graceful_kill(&mut child, Duration::from_secs(3)).await;
            Err(ProviderError::Timeout)
        }
    }
}

async fn graceful_kill(child: &mut Child, grace: Duration) {
    if let Some(pid) = child.id() {
        unsafe { libc::killpg(pid as i32, libc::SIGTERM); }
    }
    match tokio::time::timeout(grace, child.wait()).await {
        Ok(_) => {}
        Err(_) => { let _ = child.kill().await; }
    }
}
```

Sharp edges handled:
- `process_group(0)` — kills entire process tree, not just top-level binary
- `kill_on_drop(true)` — safety net if the Child is dropped without explicit cleanup
- stdout/stderr read in separate tokio tasks — avoids deadlock on pipe buffers
- SIGTERM → grace period → SIGKILL escalation — allows graceful CLI shutdown

---

## Part 6: Response Size Management

### The Constraint

Claude Code enforces a ~25K token soft limit on MCP tool responses (`MAX_MCP_OUTPUT_TOKENS`). A `query_parallel` returning 6 model responses at 4-8K tokens each = 24-48K tokens. Right at or over the limit.

### The Strategy

- Per-response character limit: ~3,000 chars each (6 responses x 3K = 18K chars, safely under 25K tokens)
- If a model response exceeds the limit, extract `<SUMMARY>` tag if present, otherwise truncate with head + tail excerpt
- Return structured JSON so Claude Code renders it more fully (JSON formatting workaround from Claude Code issue #2638)
- The `max_chars_per_response` parameter on `query_parallel` allows Claude to adjust per use case

### Progress Notifications

Claude Code does **NOT** display `notifications/progress` from MCP servers (confirmed closed as "NOT PLANNED" in GitHub issues #3174, #4157). Claude Code also does not send `progressToken` in requests. Do not invest in streaming partial results. Instead:

- Log detailed progress to **stderr** (visible via `claude --debug` or in logs)
- Keep total execution under 60s via true parallel dispatch
- Return partial results from completed models rather than failing the entire call

---

## Part 7: Server Lifecycle & Resilience

### Claude Code Provides Almost No Lifecycle Management

- **No timeout**: Claude Code has zero timeout for MCP tool calls. Documented case of a 16+ hour hang.
- **No health checks**: No ping, no heartbeat, no watchdog.
- **No auto-restart**: If the MCP server exits, tools become unavailable until Claude Code restarts.
- **Unreliable cleanup**: Orphaned MCP server processes documented after Claude Code exits.

### Self-Defensive Design

```rust
#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing (stderr only — stdout is MCP JSON-RPC)
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();

    // Build MCP server
    let server = SquallServer::new(Config::from_env()?)?;
    let service = server.serve_stdio().await?;

    // Graceful shutdown on signals or stdin EOF
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    tokio::select! {
        _ = service.waiting() => { info!("stdin closed, shutting down"); }
        _ = sigterm.recv() => { info!("SIGTERM received"); }
        _ = sigint.recv() => { info!("SIGINT received"); }
    }

    // Cancel all in-flight requests, drop HTTP clients, reap children
    server.shutdown().await;
    Ok(())
}
```

Every external call (HTTP and subprocess) has a per-provider timeout. No unbounded waits. Panic safety via `tokio::task::JoinHandle` error handling — a panic in one tool handler returns an error response, not a server crash.

### Minimal Volatile State

The server is mostly stateless but retains short-lived runtime state:

| State | Purpose | TTL |
|---|---|---|
| CLI capability cache | Version + feature detection from startup probe | Session lifetime |
| Provider health counters | Circuit breaker — if Grok fails 3x in 60s, skip it | 60s rolling window |
| In-flight request registry | For cancellation on shutdown — know what to kill | Per-request |
| Shared reqwest::Client | Connection pooling (TLS reuse, Keep-Alive) | Session lifetime |

No durable conversation state. No disk persistence. Resets on restart.

---

## Part 8: Agent Teams Integration

### When to Use What (Decision Framework from Opus Subagent Analysis)

```
Task arrives
    │
    ├── 1-2 model calls, light analysis
    │   └── Direct MCP call: chat or clink
    │
    ├── 3-6 model calls, same prompt, cross-reference responses
    │   └── query_parallel (single MCP call, tokio fan-out)
    │
    ├── 3-6 independent tasks, each needs 2-4 tool calls
    │   └── Subagents (Task tool, one per task)
    │
    └── Sustained parallel work, 5+ tool calls each, benefits from context isolation
        └── Agent Teams (teammates)
```

### Token Efficiency Math (6-Model Consensus)

| Approach | Total Tokens | Relative Cost | Wall Clock |
|---|---|---|---|
| One agent + `query_parallel` | **~38K** | **1.0x** | 15-30s |
| 6 subagents + `query` each | ~167K | 4.4x | 20-45s |
| 6 teammates + `query` each | ~185K | 4.9x | 25-50s |

**For simple consensus, `query_parallel` wins decisively.** The 16K token startup overhead per subagent/teammate is wasted when the Rust server fans out 6 HTTP/subprocess calls in microseconds.

### The 30% Overhead Rule

If startup cost (16K tokens) exceeds 30% of the useful work a subagent/teammate will do, drop to a lighter approach. Below ~50K tokens of useful work per agent, use `query_parallel` directly.

### Where Teams Earn Their Cost

**Multi-lens deep review (5 teammates, one per lens):**
- Each teammate gets a clean context window with only its specialized prompt + its model responses
- `query_parallel` inside each teammate handles the model fan-out
- 80K startup cost justified because each teammate does substantial analytical work
- Lead receives 5 focused reports instead of 15 raw model outputs

**Large PR review (50+ files):**
- Split by dependency clusters, not alphabetically
- Pre-compute interface summaries (~2-5K tokens) for cross-cluster awareness
- Duplicate "bridge files" (imported by 3+ clusters) into multiple teammates' contexts
- Total: ~345K tokens — expensive, but the only viable option at scale

---

## Part 9: Migration Strategy

### Phase 1: Scaffold + Single Model (Week 1)

1. `cargo init squall`, add rmcp `=0.16.0`, reqwest, tokio, serde
2. Implement MCP server with `listmodels` tool (static model catalog)
3. Implement `chat` tool with HTTP backend (Grok via xAI API)
4. Wire into Claude Code as `squall` alongside PAL for side-by-side testing
5. Validate: `mcp__squall__chat` works, response format matches expectations

### Phase 2: Full Provider Coverage (Week 1-2)

1. Add OpenRouter HTTP backend (Kimi, GLM)
2. Add CLI subprocess backend (Gemini CLI, Codex CLI)
3. Implement `clink` tool with structured output parsing
4. Startup self-check probes for CLI versions
5. Validate: all 5 models reachable, error handling for each failure mode

### Phase 3: Parallel Consensus (Week 2)

1. Implement `query_parallel` tool with `futures::future::join_all`
2. Partial failure semantics (overall_status, per-provider results)
3. Per-response character limits and truncation
4. Per-provider timeout + circuit breaker
5. Validate: parallel consensus completes in 10-20s, partial failures handled gracefully

### Phase 4: Compatibility Cutover (Week 2-3)

1. Rename MCP server from `squall` to `zen` (replacing PAL)
2. Verify tool names match: `chat`, `clink`, `listmodels` → `mcp__zen__chat`, etc.
3. Update `/consensus` slash command to use `query_parallel` instead of individual calls
4. Run replay validation with captured real consensus prompts
5. Update ori-v2 if any tool signatures changed (audit 6 Rust source files)
6. Decommission PAL Python server

### Rollback Plan

Keep PAL installed at `~/.local/bin/pal-mcp-server` during transition. MCP config can switch between Squall and PAL by changing the server command. No data migration needed — Squall is stateless. The repo lives at `/Users/david/Documents/Programs/squall`.

---

## Part 10: Testing Strategy

### Contract Tests (CLI Output Parsing)

Capture real Gemini JSON and Codex JSONL output as fixture files. Parse with the same code paths used in production. Validate truncation to character budget and error normalization. Use `insta` for snapshot testing.

### Fake CLI Binaries

Shell scripts or tiny Rust binaries that simulate:
- Normal structured output
- Schema drift (missing fields, extra fields, changed format)
- Partial output (process killed mid-response)
- Hangs (never exit — tests timeout handling)
- Non-zero exit codes

### HTTP Mocks

Use `wiremock` to simulate:
- Successful responses from Grok, OpenRouter
- 429 rate limits with and without Retry-After headers
- 5xx server errors (retry behavior)
- Malformed JSON responses
- Connection timeouts
- Content filtering (empty choices, null content)

### MCP End-to-End

- `tools/list` stability snapshots (tool names, parameter schemas don't drift)
- `chat`, `clink`, `listmodels`, `query_parallel` with deterministic inputs
- Partial failure assertions for `query_parallel`
- Response size validation (under 25K token limit)

### Record/Replay (Nightly, Optional)

Run against real providers with redacted fixtures. Not in the unit test path. Catches provider API changes that mocks can't simulate.

---

## Part 11: LOC Reality Check

### Estimated Line Counts

| Component | Lines |
|---|---|
| MCP server setup, signal handling, main | ~150 |
| Tool handlers (chat, clink, listmodels, query_parallel) | ~400 |
| Internal contract types (ProviderRequest, ProviderResult) | ~80 |
| HTTP backend (reqwest, OpenAI-compatible) | ~300 |
| CLI subprocess backend | ~200 |
| CLI output parsers (Gemini JSON, Codex JSONL) | ~120 |
| Model registry (HashMap + config loading) | ~150 |
| Error types + retry logic | ~180 |
| Config (env vars, provider configs) | ~100 |
| Circuit breaker + health counters | ~80 |
| **Total** | **~1,760 - 2,360** |

Budget for ~400-700 lines of slack for edge cases discovered during implementation. Final estimate: **2,000-2,500 LOC** of Rust, replacing ~80K LOC of Python.

### Dependency Budget

```toml
[dependencies]
rmcp = { version = "=0.16.0", features = ["server", "transport-io"] }
tokio = { version = "1", features = ["full"] }
reqwest = { version = "0.12", features = ["json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
futures = "0.3"
tracing = "0.1"
tracing-subscriber = "0.3"
thiserror = "2"

[dev-dependencies]
wiremock = "0.6"
insta = { version = "1", features = ["json"] }
tokio-test = "0.4"
```

---

## Part 12: What We're NOT Building

Explicit scope exclusions to prevent creep:

- **No system prompts in the server** — Claude applies these via slash commands and skills
- **No conversation memory** — Claude's context window handles this
- **No workflow orchestration** — Claude subagents and teams handle this
- **No file reading** — Claude reads files directly, passes content in prompts
- **No consensus synthesis** — Claude synthesizes in its own context
- **No streaming to Claude Code** — MCP doesn't support it; Claude Code ignores progress notifications
- **No web UI or HTTP transport** — stdio only, matching Claude Code's MCP architecture
- **No Azure or DIAL providers** — not in David's active model rotation
- **No Gemini direct API** — Gemini routes through CLI subscription; API path omitted unless needed as fallback
- **No OpenAI direct API** — Codex routes through CLI subscription; same reasoning

---

## Part 13: Risk Register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| CLI output format drift (Gemini/Codex updates) | High | Medium | Version-gate parsers, startup probe, explicit "unsupported version" errors |
| rmcp 0.16 instability (released 2 days ago) | Medium | High | Pin exact version, audit task_manager memory, avoid task system |
| Rate limiting from parallel burst | Medium | Medium | Per-provider circuit breaker, Retry-After header support |
| MCP response exceeds 25K token limit | Medium | Low | Per-response char limits, structured JSON, configurable budget |
| ori-v2 tool name incompatibility | Low | High | Ship with identical tool names, validate with ori-v2 test suite |
| Claude Code orphans Squall process on exit | Low | Low | Signal handlers, stdin EOF detection, kill_on_drop |
| Provider API breaking changes | Low | Medium | Nightly record/replay tests against real providers |

---

## Appendix A: Consensus Latency Model

### Current (PAL Python)

```
Sequential: model_1 (10s) → model_2 (8s) → model_3 (15s) → model_4 (5s) → model_5 (12s)
Total: 50s + MCP round-trip overhead (~5s) = ~55s
With event loop blocking and retry delays: 2-3 minutes typical
```

### Squall with query_parallel

```
Parallel: model_1 (10s) ─┐
          model_2 (8s)  ─┤
          model_3 (15s) ─┤ tokio::join_all → max = 15s
          model_4 (5s)  ─┤
          model_5 (12s) ─┘
Total: 15s + MCP overhead (~5ms) = ~15s
```

### Squall with query_parallel + deadline

```
Parallel with 20s deadline:
  model_1 (10s) ── success
  model_2 (8s)  ── success
  model_3 (25s) ── timeout at 20s, partial return
  model_4 (5s)  ── success
  model_5 (12s) ── success
Total: 20s, 4/5 succeeded, overall_status: "partial"
```

---

## Appendix B: Sources

### Research Documents (This Project)
- `CLAUDE-ORCHESTRATION-DEEP-DIVE.md` — Subagent/team mechanics, context passing, 5.66 GB empirical data
- `ZEN-USAGE-ANALYSIS.md` — Cross-project zen usage, 367 projects, consensus workflow evolution
- `PAL-ARCHITECTURE-REVIEW.md` — 5-lens deep review of PAL v9.8.2, P0-P2 findings

### External Model Inputs (This Session)
- **Codex GPT5.3** (via clink, 2 rounds) — Tool name compatibility risk, CLI output drift, partial failure semantics, rmcp macro annotations, testing strategy
- **Gemini 3 Pro Preview** (via clink, round 1) — Rate limit burst risk, connection pooling, list_models caching, auth/env variable management
- **Claude Opus 4.6 subagent** (team swarm design) — Token efficiency math, 30% overhead rule, dependency-clustered PR review, hybrid decision framework
- **Claude Opus 4.6 subagent** (MCP transport) — 25K token limit, readOnlyHint parallelism, rmcp 0.16 assessment, progress notification dead end, process lifecycle
- **Claude Opus 4.6 subagent** (provider complexity) — LOC reality check, OpenAI-compatible quirks, Gemini API differences, error taxonomy, auth abstraction

### Upstream References
- MCP specification (2025-06-18): https://modelcontextprotocol.io/specification/
- rmcp crate (v0.16.0): https://crates.io/crates/rmcp
- Gemini CLI headless mode: https://google-gemini.github.io/gemini-cli/docs/cli/headless.html
- Codex non-interactive mode: https://developers.openai.com/codex/noninteractive
- Claude Code MCP response limits: GitHub issues #2638, #12054
- Claude Code progress notification status: GitHub issues #3174, #4157
- Claude Code MCP timeout behavior: GitHub issue #15945
