# Review Tool Design & Architecture

> Design document for Squall's `review` tool — multi-model dispatch with straggler cutoff and streaming partial capture.
> Validated by 5-model consensus + Gemini/Codex architectural review (2026-02-20).

## Motivation

MCP tools are atomic request/response. When Claude dispatches 5 parallel `chat`/`clink` calls, it must wait for ALL to complete before seeing any results. A single slow model (Kimi at 300s, Codex at 300s) blocks the entire review.

The `review` tool solves this by:
1. Dispatching to all models in parallel within a single MCP tool call
2. Enforcing a straggler cutoff (default 180s) — returning whatever arrived
3. Streaming partial capture — not discarding slow models, but snapshotting their progress
4. Writing results to disk for compaction resilience

## How Claude Code Dispatches Squall Today

```
You type "run squall on squall"
        │
        ▼
   Claude (LLM) ── decides to call 5 MCP tools in parallel
        │
        ▼
   Claude Code (host) ── sends 5 tool calls to squall MCP server
        │
        ├─► mcp__squall__chat(grok)     ──► HTTP POST to xai API
        ├─► mcp__squall__chat(kimi)     ──► HTTP POST to openrouter
        ├─► mcp__squall__chat(glm)      ──► HTTP POST to openrouter
        ├─► mcp__squall__clink(gemini)  ──► CLI subprocess
        └─► mcp__squall__clink(codex)   ──► CLI subprocess
        │
        │  (all 5 concurrent via tokio + semaphores)
        │  (Claude Code waits for ALL 5 to complete)
        │
        ▼
   All 5 results return as tool_result messages (in memory, not disk)
        │
        ▼
   Claude Code batches all 5 into one API call to Claude
        │
        ▼
   Claude reads all 5 essays (~30-60s inference) ← THIS is the bottleneck
        │
        ▼
   You see the synthesis
```

**Not sub-agents. Not swarming.** Just parallel MCP tool calls. Each response lives in memory until Claude Code writes the session JSONL transcript.

## Bottleneck Breakdown

| Phase | Time | Controllable? |
|-------|------|--------------|
| Slowest model inference | 20-300s | No (at their mercy) |
| Network/dispatch overhead | <1s | Already minimal |
| Claude reading 5 large responses | ~30-60s | **Yes — reduce what Claude reads** |
| Claude generating synthesis | ~20-30s | Somewhat |
| Squall in-memory buffering | ~0.001s | Irrelevant |

## 5-Model Consensus

All 5 models reviewed the review tool proposal. Results:

### Universal Agreement (5/5)

1. **Straggler cutoff is the core value** — MCP tools are atomic request/response. The caller cannot get partial results. Only squall can implement "return what you have after 180s."

2. **Don't merge/deduplicate findings** — Semantic merging is AI-complete. Concatenate with attribution headers. Let the caller LLM synthesize.

3. **Partial failure is the expected case** — Return MCP success with per-model status. Never fail the whole request for individual model failures. Consistent with existing anti-cascade behavior in `response.rs:48`.

4. **Keep chat/clink alongside review** — Review is additive, not a replacement. Not every use case needs multi-model dispatch.

### Key Disagreement

**Kimi (against):** Called it a "layering violation" and "god tool anti-pattern." Argued for a separate Squall-Ensemble service. Counterargument ("caller can do the same") is wrong because MCP tools are atomic — only squall can implement the cutoff.

### Unique Insights by Model

- **Codex** (zero false positives): Semaphore starvation risk — review tool could consume all permits. Suggests `not_started_before_cutoff` as distinct status for models that never got a permit.
- **Gemini**: `CLI_MAX_CONCURRENT=4` means 5 CLI models blocks the 5th. Need dedicated semaphore or higher limit.
- **GLM**: "The value is in latency control, not in merging" — cleanest architectural framing.
- **Grok**: Well-structured but generic, doesn't reference specific code paths.

## Request Schema

```rust
pub struct ReviewRequest {
    /// The prompt to send to all models
    pub prompt: String,
    /// Optional: specific models to query (defaults to all configured)
    pub models: Option<Vec<String>>,
    /// Straggler cutoff in seconds (default: 180)
    pub timeout_secs: Option<u64>,
    /// System prompt for all models
    pub system_prompt: Option<String>,
    /// Sampling temperature for HTTP models
    pub temperature: Option<f64>,
    /// File paths for context (same as chat/clink)
    pub file_paths: Option<Vec<String>>,
    /// Working directory for file_paths
    pub working_directory: Option<String>,
}
```

## Response Schema

Three per-model statuses: `success`, `partial` (streaming cutoff), `error`.

```json
{
  "results": [
    {
      "model": "grok-4-1-fast-reasoning",
      "provider": "xai",
      "status": "success",
      "response": "## Findings\nFull analysis...",
      "latency_ms": 24000
    },
    {
      "model": "moonshotai/kimi-k2.5",
      "provider": "openrouter",
      "status": "partial",
      "reason": "cutoff",
      "response": "## Finding 1\nThe code at line 42...",
      "latency_ms": 180000
    },
    {
      "model": "z-ai/glm-5",
      "provider": "openrouter",
      "status": "error",
      "reason": "rate_limited",
      "response": null,
      "error": "rate limited by openrouter",
      "latency_ms": 3000
    }
  ],
  "not_started": [],
  "cutoff_seconds": 180,
  "elapsed_ms": 180023,
  "results_file": ".squall/reviews/2026-02-20T12-23-45.json"
}
```

## Streaming Straggler Capture

Instead of hard cutoff (timeout -> discard), use **streaming** to capture whatever arrived before the deadline. Every token that made it through the wire is preserved.

### HTTP Backends

Set `"stream": true` in API request. Accumulate SSE `content` deltas in a buffer. On cutoff, snapshot the buffer -> `status: "partial"`.

```
SSE events arriving over time:
  t=0s    data: {"choices":[{"delta":{"content":"## Finding 1\n"}}]}
  t=5s    data: {"choices":[{"delta":{"content":"The code at line 42..."}}]}
  ...accumulating...
  t=180s  -- cutoff fires -- snapshot buffer -- return "partial"
```

### CLI Backends

Read from pipe into shared buffer (`Arc<Mutex<Vec<u8>>>`). On cutoff, kill process group and snapshot buffer contents. Skip JSON parsing for partial results (return raw text — complete findings are still useful without parser structure).

### Three States

- `success`: Model completed before cutoff. Full response, parsed normally.
- `partial`: Model was still streaming when cutoff fired. Whatever arrived is returned as-is.
- `error`: Model failed (rate limit, auth, spawn failure). No response.

### Reason Field

Each non-success result includes a `reason`: `cutoff`, `semaphore_timeout`, `killed`, `parse_error`, `rate_limited`, `auth_failed`, `spawn_failed`.

## Compaction Resilience: Spill-to-Disk

### The Problem

When Claude Code compacts (approaches context limit), tool results are summarized lossy. 20K tokens of model responses become ~500 tokens of summary. Nuance, exact quotes, line references, and minority opinions are destroyed.

### Three-Tier Persistence Model

| Tier | Where | Survives compaction? | Survives session? | Practical to access? |
|------|-------|---------------------|-------------------|---------------------|
| Context window | RAM | No | No | Yes (immediate) |
| **Review files** | `.squall/reviews/` | **Yes (path in summary)** | **Yes** | **Yes (Read tool)** |
| Session JSONL | `.claude/projects/.../` | Yes | Yes | No (raw log, huge) |
| Auto-memory | `memory/` | Yes | Yes | Yes (always loaded) |

### Solution

```
Review tool executes:
    ├── Dispatch to N models, collect with straggler cutoff
    ├── Write full results to disk:
    │   └── .squall/reviews/2026-02-20T12-23-45.json
    ├── Return to Claude:
    │   ├── Structured summary (small, context-friendly)
    │   └── results_file: ".squall/reviews/2026-02-20T12-23-45.json"
    │
    ▼ Compaction happens
    │
    Summary preserves: "Round 4 review saved to .squall/reviews/..."
    │
    ▼ Claude needs a detail post-compaction
    │
    Read(".squall/reviews/2026-02-20T12-23-45.json") ← full results recovered
```

The file path survives compaction because it's a short string in the summary.

## Architecture: ReviewExecutor

**Don't reuse `Registry::query` for review.** Build a dedicated `ReviewExecutor` that:

- Owns all futures and abort handles (prevents leaked reader tasks)
- Has shared buffers (`Arc<Mutex<Vec<u8>>>`) readable by cutoff handler
- Returns `Vec<ReviewModelResult>` with per-model status
- Handles persistence (writes to disk, returns file path)
- Reports `not_started` for models that never got semaphore permits

### Why Not Reuse Registry::query

`Registry::query` is designed for single-model, single-response dispatch. The review tool needs:
- Shared ownership of in-flight buffers (for partial capture on cutoff)
- Abort handles for all spawned tasks (cleanup on cutoff)
- Per-model status tracking (success/partial/error with reasons)
- A global cutoff timer that snapshots all in-flight work

These are fundamentally different ownership and lifecycle requirements.

## Implementation Plan

### New Files

| File | Purpose |
|------|---------|
| `src/tools/review.rs` | ReviewRequest struct, ReviewModelResult |
| `src/review.rs` | ReviewExecutor — dispatch, cutoff, persistence |

### Modified Files

| File | Change |
|------|--------|
| `src/dispatch/http.rs` | Add streaming mode (SSE accumulation) |
| `src/dispatch/mod.rs` | Add `ProviderStatus` enum (Complete/Partial) |
| `src/server.rs` | New `#[tool(name = "review")]` handler |
| `src/lib.rs` | Add `pub mod review;` |

### Implementation Order

1. `ProviderStatus` enum in `dispatch/mod.rs`
2. `ReviewRequest` + `ReviewModelResult` in `tools/review.rs`
3. SSE streaming mode in `dispatch/http.rs`
4. Shared-buffer CLI dispatch (Arc<Mutex<Vec<u8>>>)
5. `ReviewExecutor` in `src/review.rs`
6. Disk persistence (.squall/reviews/)
7. Server handler in `server.rs`
8. Tests (unit + integration)

### Key Constraints

- Response size: 5 models x ~4K tokens each = ~80KB. Within MCP limits but significant.
- Cutoff must be shorter than Claude Code's MCP tool timeout (~600s).
- Always return MCP success (anti-cascade pattern from response.rs).
- Per-model error detail in the JSON payload, not at MCP transport level.
- Partial responses skip parser (GeminiParser/CodexParser expect complete JSON). Return raw text.
- Persistence failure must never erase in-memory results — return results + persistence error metadata.
- Use UUID filenames for concurrent safety, temp-file + atomic rename.
- Anchor disk path to absolute directory, not relative (depends on launcher cwd).

## Critical Issues from Validation Review

Gemini + Codex reviewed this design (2026-02-20). Must address before implementation:

1. **CLI buffer ownership prevents snapshot** — `Vec<u8>` owned by spawned tasks inside `read_future`. When timeout drops the future, data is lost. Fix: shared `Arc<Mutex<Vec<u8>>>` or channel so cutoff handler can read partial data.

2. **HTTP SSE requires separate code path** — Current `http.rs` parses one JSON `ChatCompletion`. SSE is `data: {...}` line-delimited events. Cannot reuse existing dispatch — need `HttpStreamDispatch` or streaming mode in `HttpDispatch`.

3. **Dropped futures leak spawned reader tasks** — `tokio::spawn` tasks in `cli.rs` outlive cancelled parent future. Detached readers hold pipe handles -> resource leaks. Review executor must own abort handles.

4. **UTF-8 split at cutoff** — Partial buffer may end mid-codepoint. Must use `String::from_utf8_lossy` or trim incomplete trailing bytes.

5. **Semaphore starvation expected** — 5 CLI + `CLI_MAX_CONCURRENT=4` = one waits. Map to `not_started` (distinct from error/timeout).

6. **ProviderResult needs status field** — Without `Complete` vs `Partial`, partial responses look like successes to consumer. Add `ProviderStatus` enum.

## Multi-Model Review Scorecard

From 4+ rounds of review across this session:

| Model | Precision | Speed | Strengths | Weaknesses |
|-------|-----------|-------|-----------|------------|
| **Gemini** | High | 55-184s | Systems-level bugs (pipes, streams, process lifecycle). Found all real bugs. | Occasional FP (missing import, let_chains) |
| **Codex** | Highest (0 FP) | 50-300s | Exact line references, OS-level constraints (ARG_MAX, semaphores) | Can timeout at 300s |
| **Grok** | Low (4+ FP/round) | 20-65s | Fast first pass, well-structured output | Persistent blind spots: XML escaping, edition 2024 |
| **GLM** | Medium | 75-93s | Clear architectural framing | Zero real bugs across all rounds |
| **Kimi** | Low-Medium | 113-300s (often timeout) | Edge cases when it responds (symlink traversal) | Consistently timeouts, contrarian arguments |

### Recommendations for Review Tool Defaults

- **Always include:** Gemini, Codex (highest signal)
- **Include if fast matters:** Grok (fast but noisy)
- **Consider dropping:** Kimi (300s timeout = straggler), GLM (zero bugs found)
