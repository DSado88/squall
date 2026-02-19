# How David Uses Zen: A Cross-Project Usage Analysis

**Date:** 2026-02-19
**Source:** Claude Code conversation logs (.jsonl) across ~/.claude/projects/
**Scope:** 367 project directories, 1,630+ conversation files with zen tool usage

---

## Executive Summary

Zen (PAL MCP Server) is deeply embedded in David's entire development workflow. It's not a novelty — it's infrastructure. Across 367 project directories, **82% (302) have zen tool usage**. The three dominant tools — `chat`, `clink`, and `consensus` — account for ~90% of all invocations. Consensus specifically serves as a **quality gate** at every major decision point: before implementing a plan, after completing a feature, and before merging a PR.

The single largest consumer is **ori-v2**, an automated Slack bot that runs multi-model code reviews on every PR push — accounting for ~60-70% of all zen invocations system-wide.

---

## Part 1: Scale of Usage

| Metric | Count |
|--------|-------|
| Total project directories | 367 |
| Projects with any zen tool usage | 302 (82%) |
| Conversation files referencing zen tools | 1,630 |
| Conversation files referencing consensus | 1,465 |
| Conversations with `/consensus` slash command | 251 |

### Tool Frequency (All Projects Combined)

| Rank | Tool | Occurrences | Role |
|------|------|-------------|------|
| 1 | `mcp__zen__chat` | 9,428 | Direct model queries (Grok, Kimi, GLM) |
| 2 | `mcp__zen__clink` | 8,629 | CLI-agent routing (Gemini free OAuth, Codex) |
| 3 | `mcp__zen__consensus` | 4,431 | Built-in multi-model consensus (increasingly deprecated) |
| 4 | `mcp__zen__thinkdeep` | 917 | Extended reasoning |
| 5 | `mcp__zen__listmodels` | 759 | Model discovery |
| 6 | `mcp__zen__planner` | 529 | Planning workflows |
| 7 | `mcp__zen__codereview` | 414 | Dedicated code review |
| 8 | `mcp__zen__analyze` | 303 | Code analysis |
| 9 | `mcp__zen__secaudit` | 263 | Security audit |
| 10 | `mcp__zen__version` | 165 | Server version |
| 11 | `mcp__zen__debug` | 112 | Debugging |
| 12 | `mcp__zen__apilookup` | 75 | API docs |
| 13 | `mcp__zen__docgen` | 69 | Documentation |
| 14 | `mcp__zen__refactor` | 65 | Refactoring |
| 15 | `mcp__zen__tracer` | 63 | Execution tracing |
| 16 | `mcp__zen__testgen` | 58 | Test generation |
| 17 | `mcp__zen__challenge` | 57 | Challenge assumptions |
| 18 | `mcp__zen__precommit` | 53 | Pre-commit review |

**Key insight:** `chat` and `clink` are the workhorses — they are the dispatch mechanisms used within consensus workflows. Much of the consensus orchestration has migrated from the `mcp__zen__consensus` tool to the `/consensus` slash command, which calls `chat` and `clink` directly.

### Usage by Project (Top Consumers)

| Project | Files with zen tools | Nature |
|---------|---------------------|--------|
| ori-v2 | 1,081 | Automated PR code reviews (ORI bot) |
| TheGeneral | 103 | Unknown project |
| Orchid | 68 | iOS/macOS intelligence layer |
| cortex | 15 | Development tool |
| zen-mcp-server | 10 | This project (meta-analysis) |
| ori (v1) | 8 | ORI predecessor |
| iceland | 6 | Unknown project |
| ~300 ori streaming sessions | 3-5 each | Individual ORI review sessions |

**ori-v2 dominates overwhelmingly** because it IS the automated code review bot. Each PR review spawns a Claude Code session that dispatches reviews to 5-6 models.

---

## Part 2: The Two Consensus Eras

### Era 1: `mcp__zen__consensus` Tool (Dec 2025 - early Feb 2026)

The built-in consensus tool uses a sequential, step-based protocol:
1. Claude invokes `mcp__zen__consensus` with the query
2. The tool queries Model 1, records findings
3. Returns to Claude, Claude calls again for Model 2
4. Repeat for each model (3-5 sequential round-trips)
5. Final step synthesizes all responses

**Problems experienced:**
- Sequential execution: 3-5 models at ~10s each = 30-50s total
- Each step requires a full MCP round-trip back to Claude
- The tool occasionally called itself recursively (discovered when "Opus 4.5 agent saw consensus tools available and called `mcp__zen__consensus` itself")

### Era 2: `/consensus` Slash Command (Feb 10, 2026 onward)

David created a custom slash command (`~/.claude/commands/consensus.md`, 103 lines) that:
- **Explicitly FORBIDS** the `mcp__zen__consensus` tool
- Dispatches to **6 models in parallel** in a single message with multiple tool calls
- Routes intelligently per model:

| Model | Tool | Why |
|-------|------|-----|
| Gemini 3 Pro Preview | `mcp__zen__clink` (cli_name="gemini") | Free OAuth tier (1000 req/day), bypasses paid API key |
| Grok 4.1 Fast Reasoning | `mcp__zen__chat` | Direct API |
| Claude Opus 4.6 | Task agent (subagent) | Explicitly told NOT to use consensus tools |
| Codex GPT5.3 | `mcp__zen__clink` (cli_name="codex") | Extended thinking |
| Kimi K2.5 | `mcp__zen__chat` (via OpenRouter) | model="moonshotai/kimi-k2.5" |
| GLM-5 | `mcp__zen__chat` (via OpenRouter) | model="z-ai/glm-5", often skipped for speed |

**Result:** Parallel dispatch replaced sequential. But latency is still high because PAL's blocking architecture means calls execute sequentially despite being dispatched together. This is the exact bottleneck identified in the architecture review.

### The ORI Bot (Automated Consensus)

ori-v2's `consensus.rs` module reads the model routing from `~/.claude/commands/consensus.md` at runtime, so model changes don't require Rust recompilation. The `zen_validator.rs` uses consensus for nightly validation runs. Findings are tagged:
- `[Consensus]` — 3+ models agree
- `[Disputed]` — models disagree
- `[Single]` — only 1 model found it

---

## Part 3: Project-by-Project Usage

### Orchid (iOS/macOS Intelligence Layer)

**Scale:** 122 conversation files, 518 consensus references, 187 tool invocations, ~111 explicit user requests.

**What consensus is used for (by frequency):**

1. **Code Quality Review (39 requests)** — After implementing a phase, evaluate code quality, SOLID adherence, TypeScript patterns, architecture alignment. Example: *"lets run it through consensus both for code quality and also against documentation"*

2. **Implementation Planning (25 requests)** — Before starting work, validate the plan. Example: *"before we go through with this plan, please run /consensus on the plan"*

3. **Bug Finding / Hidden Gotchas (20 requests)** — Proactive bug hunting. Example: *"on this implementation. what hidden gotchas arent we considering"*

4. **Architecture / Documentation Alignment** — Checking implementations against the Orchid Intelligence Layer architecture document (a ~50k-token spec).

5. **Feature Completeness / Merge Readiness** — Final validation. Example: *"lets get consensus that this is a clean and quality PR ready for merge"*

**Domain topics:** ActivePolicy, FastBrain/SlowBrain, policyStore, signal wiring, proactivity slider, shadow queue, AEC parameters, ProcessTap, Whisper/Qwen3-ASR, Silero VAD, breadcrumb navigation, dev panel.

**Workflow integration:**
> plan > consensus on plan > update the plan > TDD tests > implement > run tests > /deep-review > validate

### ori-v2 (Rust Slack Bot — Orchid Repository Intelligence)

**Scale:** 3,501 conversation files, 2,031 (58%) mention consensus, 1,081 (31%) reference zen tools directly. 1,964 are automated ORI bot sessions.

**Three invocation paths:**

1. **Human `/consensus`** (14 invocations) — Architecture validation, code quality review, edge case hunting, overengineering checks. Examples:
   - *"/consensus on this plan and what were missing / gotchas"*
   - *"/consensus on code quality and fixes. any bugs?"*
   - *"/consensus on this. what are we overengineering. how can we simplify. is this necessary."*

2. **Claude-initiated via Skill tool** (6 calls) — Claude itself invokes consensus during implementation for design review of specific modules (debounce buffers, SIGINT handling, trait abstractions).

3. **ORI bot automated** (1,046 sessions) — Every PR push triggers multi-model code review. Uses `mcp__zen__chat` and `mcp__zen__clink` directly (not the consensus tool, to prevent recursion).

**Domain topics:** Bidi session management, SIGINT handling, resume-on-death, API trait abstractions, workspace isolation, auth consolidation.

**Key Rust integration:** The `mcp__zen__` prefix is hardcoded in 6 Rust source files across ori-v2. The `consensus.rs` reads model routing from the markdown slash command at runtime.

### Butler (Rust Application Framework)

**Scale:** Single long-running session (3,272 lines) building "Cart Blanche" — a grocery automation chatbot on Butler.

**Usage pattern:** Multi-model review at decision gates.

| Gate | What happened |
|------|--------------|
| Initial architecture | Asked Codex + Gemini via clink: which Butler crates to extract, copy vs git deps |
| Master plan approval | Full 6-model `/consensus`: 5/6 said **don't fork Butler** → plan changed to upstream pin |
| Phase 2 plan review | Gemini + Codex via clink: reviewed butler-claude integration plan |
| Phase 3 iMessage design | Gemini + Codex + Grok: 7 architecture questions about iMessage integration |
| Phase 3 final plan | Gemini + Codex: reviewed refined Phase 3 plan |

**Key finding:** Consensus directly changed the plan. The fork-vs-pin decision was reversed based on 5/6 model agreement. David treats consensus as a **design review board for solo development**.

**Invocation granularity:** David doesn't always use full 6-model consensus. Quick checks use 2-3 models ("lets ask codex and gemini via clink"). Full `/consensus` is reserved for the biggest decisions.

### Notion-Finder (Document Browser → "Potion")

**Scale:** Single long session (5,780 user messages), 5 distinct consensus requests.

**Consensus topics:**
1. Architecture review of initial plan — *"gotchas and AI automation over this server"*
2. Phase 3 code quality / bug review (2 attempts)
3. Branch code review — 18 files, ~1,685 lines
4. Potion evolution plan — backend-agnostic document browser with Plate.js + Git backend

**Also uses ad-hoc zen queries:** *"what are we missing. lets ask codex and gemini via clink"* — targeted 2-model check on Plate.js markdown editor gaps.

---

## Part 4: Invocation Patterns

### How David Invokes Consensus

1. **Bare `/consensus`** — Relies on conversation context. *"run /consensus"*, *"lets get /consensus on this branch"*
2. **Topic-prefixed** — *"/consensus on feature completeness"*, *"/consensus on this plan"*
3. **Explicit model selection** (Era 1) — *"lets get consensus from gemini 3 pro preview, grok 4, claude opus 4.5 and codex gpt5.2"*
4. **Inline with workflow** — *"before implementing, validate this plan with consensus"*
5. **Casual shorthand** — *"consensus? this sound like a good plan?"*
6. **With model exclusions** — *"skip GLM"*, *"just skip glm and grab consensus from everyone else"*
7. **Targeted subset** — *"lets ask codex and gemini via clink"* (2-model quick check, not full consensus)

### When David Uses Consensus (Decision Gates)

| Gate | Purpose | Frequency |
|------|---------|-----------|
| Before implementation | Validate plan, find gotchas | Very high |
| After feature completion | Code quality, bug hunting | Very high |
| Before merge | Merge readiness, final review | High |
| Architecture decisions | Evaluate approaches, break ties | High |
| Overengineering check | Simplification, necessity check | Medium |
| Fact-checking | Verify technical claims | Occasional |

### How Results Are Used

1. **Quality gate** — Consensus must pass before proceeding to next phase
2. **Plan modification** — Consensus findings directly change implementation plans (e.g., butler fork→pin)
3. **Bug backlog** — Consensus-identified issues become fix items
4. **Documentation updates** — Findings fed back into architecture docs
5. **Confidence building** — Independent confirmation of Claude's recommendations
6. **Tie-breaking** — When multiple approaches exist, majority model agreement decides

---

## Part 5: The Pain Points (Experienced Firsthand)

### Latency Is the Dominant Problem

David's own words during this analysis session: *"this is partially what im talking about — these things take forever"*

The consensus workflow, even with the parallel slash command, is bottlenecked by PAL's sequential blocking architecture. A 6-model consensus that should take ~10-15 seconds (limited by the slowest model) actually takes 2-3 minutes because:
1. PAL's `generate_content()` blocks the event loop
2. Even "parallel" dispatches execute sequentially server-side
3. Each model response must complete before the next can start
4. GLM-5 via OpenRouter is often the slowest, leading to *"skip GLM"* requests

### Model Reliability

- GLM-5 frequently times out or fails, leading to habitual exclusion
- The user has developed workarounds: *"just skip glm and grab consensus from everyone else"*
- When a model fails, the entire consensus still completes with remaining models

### The `mcp__zen__consensus` Tool Was Abandoned

David migrated from the built-in tool to a custom slash command because:
1. The tool's sequential step protocol was too slow
2. The tool had a recursion bug (agents calling consensus on themselves)
3. The slash command gives Claude full conversation context
4. Direct `chat`/`clink` dispatch is more controllable

### Context Loss Across Tools

The `mcp__zen__consensus` tool had to reconstruct context from arguments alone, losing the rich conversation history that Claude had. The slash command solved this by keeping orchestration in Claude's context window.

---

## Part 6: Implications for PAL's Architecture

### What David's Usage Reveals About PAL's Bottlenecks

1. **The async problem is real and user-visible.** David experiences the sequential blocking on every consensus call. His workaround (the slash command) attempts to parallelize at the Claude level, but PAL's internal blocking defeats this.

2. **Consensus is the killer feature, and it's the most bottlenecked.** With 1,465 conversation files referencing consensus across 302 projects, this is not a niche tool — it's the primary value proposition. Making it 3-5x faster would be the single most impactful improvement.

3. **The user has outgrown the built-in consensus tool.** The slash command is a user-built replacement that works around PAL's limitations. This suggests the tool's design didn't evolve with usage patterns.

4. **ori-v2's automated usage stresses the system at scale.** 1,046 automated sessions, each dispatching multi-model reviews, means PAL handles thousands of provider calls daily. The blocking architecture turns what should be a throughput problem into a latency problem.

5. **Model reliability matters.** GLM-5 timeouts cause cascading delays. A scatter-gather pattern with early return (return results as models complete, don't wait for stragglers) would dramatically improve perceived performance.

6. **The user wants granularity.** Sometimes 2 models (quick check), sometimes 6 (major decision). The slash command supports this via model exclusions, but PAL's consensus tool doesn't offer this flexibility.

### What Would Actually Help David

| Improvement | Impact on David's workflow |
|-------------|---------------------------|
| Async providers (AsyncOpenAI, genai.aio) | Parallel consensus actually runs in parallel. 2-3 min → 15-30s. |
| Scatter-gather with early return | Don't wait for slow/failing models (GLM). Return partial results. |
| Built-in parallel consensus mode | Eliminate need for the custom slash command workaround |
| Per-model timeout controls | Skip slow models automatically instead of manual "skip GLM" |
| Streaming partial results | Show model responses as they arrive, not all-at-once |
| First-class model subsets | "Quick consensus" (2-3 models) vs "full consensus" (6 models) as a parameter |

---

## Appendix: Global Configuration

### ~/.claude/commands/
- `consensus.md` — 6-model parallel consensus (103 lines)
- `deep-review.md` — 5-lens PR-CoT review
- `coderabbit.md` — CodeRabbit CLI review
- `draft-commit.md` — AI-assisted commit drafting
- `pull-template.md` — Skill template sync

### ~/.claude/skills/
- `compact-context/` — Context compression
- Various project-specific skills

### Model Routing (from consensus.md)
```
Gemini 3 Pro Preview → mcp__zen__clink (cli_name="gemini")  [Free OAuth]
Grok 4.1            → mcp__zen__chat  (model="grok-4-1-fast-reasoning")
Claude Opus 4.6     → Task agent      (subagent, model="opus")
Codex GPT5.3        → mcp__zen__clink (cli_name="codex")
Kimi K2.5           → mcp__zen__chat  (model="moonshotai/kimi-k2.5")  [OpenRouter]
GLM-5               → mcp__zen__chat  (model="z-ai/glm-5")  [OpenRouter, often skipped]
```
