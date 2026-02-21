# Squall Next: Research Synthesis & Direction

> Session document from 2026-02-21. Five-agent research swarm validated team + MCP tool
> access pattern, produced 5 research reports, and converged on a lean path forward.

## What Squall Is Today

~1300 lines of Rust. MCP server dispatching prompts to multiple AI models via HTTP and CLI.

**Tools:** `chat` (HTTP), `clink` (CLI), `review` (parallel fan-out), `listmodels`

**Models:** Grok (xAI, HTTP), Gemini (CLI), Codex (CLI), Kimi (OpenRouter), GLM (OpenRouter)

**Recent additions (uncommitted, 158 tests passing):**
- Per-model system prompts on `review` (different lenses per model)
- Model name suggestions on `ModelNotFound` ("Did you mean: ...?")
- Better tool descriptions guiding callers to use `review` over multiple `chat`/`clink` calls

## The Core Insight

Squall solved dispatch. The fan-out works. The straggler cutoff works. Per-model system
prompts work.

**Everything interesting now lives outside the Rust server — in skills (prompt templates)
that teach callers how to use Squall effectively.**

The Rust stays lean. The intelligence is in the prompts.

---

## Session Arc

### 1. Deep Research API Exploration

**Elder's reference (Slack, Feb 11):**
> "FYI: deep research in gemini cli is neutered, but someone built a plugin to get around it"
> — Matthew Elder, linking to [allenhutchison/gemini-cli-deep-research](https://github.com/allenhutchison/gemini-cli-deep-research)

**What it actually is:** A Gemini CLI extension (MCP server) wrapping the Gemini Interactions
API. Despite being installable via `gemini extensions install`, it requires `GEMINI_API_KEY`
(paid tier). It's not piggybacking on CLI OAuth — it's just a convenience wrapper around the
API's launch-then-poll pattern.

**Codex deep research:** Blocked. `o4-mini-deep-research` model requires OpenAI API key, not
ChatGPT consumer auth. But Codex CLI's **normal web search** works well — ~93s, 20+ searches,
detailed sourced reports.

**Reachability summary:**

| Capability | Reachable? | Auth Required |
|---|---|---|
| Gemini deep research (Interactions API) | No | `GEMINI_API_KEY` (paid) |
| Codex deep research model | No | `OPENAI_API_KEY` |
| Codex web search (default model) | Yes | ChatGPT auth (existing) |
| Gemini CLI (standard) | Yes | OAuth (existing) |
| Grok/Kimi/GLM via Squall | Yes | `XAI_API_KEY` + `OPENROUTER_API_KEY` (existing) |

### 2. Team + MCP Tool Access Validation

Spawned a test team (`mcp-test`) with one teammate. Confirmed:
- Teammate successfully used `ToolSearch` to discover Squall MCP tools
- `listmodels` returned all 5 models
- `chat` with Grok worked (1.57s response)

**Conclusion:** Teammates inherit MCP tool access. No restrictions.

### 3. Vision Quest: What's Missing?

Dissolved assumptions about Squall's architecture. Cross-domain analysis (immune system,
orchestra, wave interference) revealed the same gap from multiple angles:

**The fan-out is solved. What happens AFTER responses come back is unsolved.**

Current architecture is a one-way pipe:
```
prompt → models → caller
```

Missing architecture is a feedback loop:
```
prompt → [router] → models → [synthesis] → caller → [feedback] → router
```

But most of that loop is overengineered for where we are today.

### 4. Five-Agent Research Swarm

Spun up `squall-research` team with 5 parallel researchers. Each used WebSearch + Squall
`review` (Grok + Kimi + GLM) to investigate one vector. All reports written to
`.squall/research/`.

| Researcher | Vector | Key Finding |
|---|---|---|
| ensemble-researcher | ML ensemble weighting | Diversity > raw accuracy (Condorcet). Static weights from scorecard as bootstrap. |
| debate-researcher | Multi-agent debate | **Don't build it.** Debate hurts discovery tasks. Add synthesis instead. |
| routing-researcher | LLM routing strategies | Reduce fan-out set, don't replace fan-out. Security = always frontier. |
| schema-researcher | Structured output schemas | GLM breaks `response_format` on OpenRouter. Prompt-based fallback needed. |
| feedback-researcher | Feedback loops | 1-bit accept/reject is enough. Dedup is highest-ROI first step. |

Full reports: `.squall/research/{ensemble-methods,multi-agent-debate,llm-routing,structured-output,feedback-loops}.md`

### 5. Overengineering Check

Most of the research roadmap is premature. We have 5 models and a handful of users.

**What's overengineered (skip for now):**
- Router with tree-sitter analysis (just pick models manually)
- Weighted ensemble scoring with Wilson score intervals
- Feedback loop infrastructure (haven't used review enough to know what feedback we'd want)
- Synthesis tool (the caller IS an LLM — it can read 5 responses)
- Consensus detection algorithm
- Multiple review slash commands for what's really "review with different models"

**What's lean (do now):**
- Skills (prompt templates teaching Claude how to use Squall)
- Structured JSON schema in system prompt (zero latency, makes results parseable)
- Commit the uncommitted 158-test codebase

---

## The Lean Path: Skills Over Infrastructure

Skills are markdown files in `.claude/skills/`. They don't change Squall's Rust code.
They teach the caller (Claude) how to wire up tools that already exist.

### `/review`

**What it does:** Code review via Squall's `review` tool.

**What the skill teaches Claude:**
- Use `listmodels` first to get exact model names
- Use `review` with per-model system prompts for different lenses
- For quick triage: just Grok
- For thorough review: all 5 models
- For security-sensitive code: always include Gemini + Codex
- Pass the diff as prompt, file context via `files` param

**Squall changes needed:** None. Uses existing `review` tool.

### `/research <topic>`

**What it does:** Spin up a team where each agent researches a different vector.

**What the skill teaches Claude:**
- TeamCreate with N researchers (3-5 depending on topic breadth)
- Each researcher gets: WebSearch + Squall `review` (Grok + Kimi + GLM)
- Each writes findings to `.squall/research/<topic>-<vector>.md`
- Team lead synthesizes when all report back
- Clean up: shutdown all teammates, TeamDelete

**Squall changes needed:** None. Uses existing tools + teams + WebSearch.

**Already proven:** This is exactly what the 5-agent swarm did in this session.

### `/deep-research <topic>`

**What it does:** Hit Codex CLI for web-search-powered research.

**What the skill teaches Claude:**
- Use Squall `clink` with model `codex` and a research-heavy prompt
- Set generous timeout (600s) — Codex does 20+ web searches
- Optionally run multiple `clink` calls with different research angles
- When `GEMINI_API_KEY` is available: also hit Gemini deep research extension

**Squall changes needed:** None. Uses existing `clink` tool.

**Limitation:** Gemini deep research blocked until API key is configured.

---

## Structured Output Schema (The One Rust-Adjacent Change)

Not a Rust change — a system prompt addition. When `review` is used for code review,
include a JSON schema in the system prompt so models return parseable findings:

```json
{
  "findings": [
    {
      "severity": "critical|high|medium|low|info",
      "category": "bug|security|performance|logic|concurrency|architecture",
      "file": "src/foo.rs",
      "start_line": 42,
      "end_line": 45,
      "description": "What the issue is",
      "reasoning": "Why this is an issue",
      "confidence": "high|medium|low",
      "code_snippet": "the actual code being flagged"
    }
  ],
  "summary": "Overall assessment"
}
```

**Why this matters:**
- Zero latency (it's just prompt text)
- Makes results grep-able and parseable
- Enables future consensus detection without building it now
- Works across all models (prompt-based enforcement)

**Why not provider-enforced schemas now:**
- GLM-5 on OpenRouter doesn't support `response_format`
- Gemini CLI has no schema enforcement
- Prompt-based works well enough, validate client-side later if needed

Full schema spec: `.squall/research/structured-output.md`

---

## Model Scorecard (Validated Across 4+ Rounds)

| Model | Speed | Precision | Unique Value | Weakness |
|---|---|---|---|---|
| Gemini | 55-184s | High | Systems-level bugs, finds real defects | Slower |
| Codex | 50-300s | Highest (0 FP) | Exact line references | Slowest |
| Grok | 20-65s | Medium (4+ FP/round) | Fast, good with system_prompt | XML escaping blind spot |
| GLM | 75-93s | Low | Architectural framing | Zero real bugs found |
| Kimi | 60-300s | Medium | Contrarian, edge cases | Frequent timeouts |

This scorecard IS the routing table. Skills can encode it as guidance:
"For quick checks use Grok. For security use Gemini+Codex. For architecture use GLM+Gemini."

No router infrastructure needed — the skill prompt tells Claude what we already know.

---

## Research Archive

All 5 detailed research reports are preserved at `.squall/research/`:

| File | Content |
|---|---|
| `ensemble-methods.md` | Condorcet jury theorem, static/adaptive weights, finding-level aggregation, quality signals |
| `multi-agent-debate.md` | 8 academic papers, why debate fails for code review, synthesis tool recommendation |
| `llm-routing.md` | RouteLLM, Martian, Unify.ai survey, rule-based router design, security hard gate |
| `structured-output.md` | Provider capability matrix, JSON schema design, consensus detection strategy |
| `feedback-loops.md` | CodeRabbit/Copilot/SonarQube/Devin survey, 1-bit feedback, JSONL storage design |

These are reference material for when we're ready to build infrastructure. Not now.

---

## What's Next (In Order)

1. **Commit the uncommitted codebase** — 158 tests, 0 clippy warnings, 820 lines of changes
2. **Write the skills** — `/review`, `/research`, `/deep-research` as prompt templates
3. **Add structured schema to review system prompt** — JSON in the prompt, not Rust changes
4. **Use it** — run reviews, run research swarms, learn what's actually needed
5. **Then** build infrastructure based on real usage patterns, not speculation

---

## Key Principle

> The Rust server stays lean. The intelligence lives in skills.
> Skills are cheap to write, cheap to iterate, and cheap to throw away.
> Infrastructure is expensive on all three counts.
>
> Build skills first. Build infrastructure only when skills hit a wall.
