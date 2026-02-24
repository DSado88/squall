---
name: squall-unified-review
description: "Unified code review with auto-depth detection and Opus agent. Use when asked to 'review', 'squall review', 'code review', 'deep review', 'check this code', or 'review this diff'. Auto-selects QUICK/STANDARD/DEEP based on code characteristics. (project)"
one_liner: "Auto-depth review — Claude picks the right depth, Opus adds the local perspective."
activation_triggers:
  - "squall review"
  - "review code"
  - "code review"
  - "check this code"
  - "review this diff"
  - "deep review"
  - "squall deep review"
  - "thorough review"
  - "quick review"
  - When user wants multi-model code review
  - When user wants AI review of a diff or PR
related_skills:
  - "[squall-research](../squall-research/SKILL.md) (for research, not code review)"
  - "[squall-deep-research](../squall-deep-research/SKILL.md) (for deep research questions)"
---

# Squall Unified Review

> **TL;DR - QUICK REFERENCE**
>
> | Concept | Translation |
> |---------|-------------|
> | Auto-depth | Claude scores the diff → QUICK / STANDARD / DEEP |
> | User override | "deep review" forces DEEP, "quick review" forces QUICK |
> | Opus agent | Background Task agent for STANDARD — local investigation (shell, git, tests) |
> | DEEP investigation | Main Claude does it sequentially — Opus is redundant |
> | Transparency | Always show depth reason: "Selected STANDARD (score 3): +2 auth, +1 size" |
>
> **Entry criteria:**
> - User wants code reviewed (any depth)
> - A diff, file set, or code snippet needs expert analysis

Unified review skill that auto-detects the right review depth and optionally spawns a Claude Code agent for local investigation. Replaces separate squall-review and squall-deep-review skills.

## Phase 0: Detect Depth

### User Override

Check the user's request first — explicit intent always wins:

| User says | Force |
|-----------|-------|
| "deep review", "thorough review", "investigate and review" | DEEP |
| "quick review", "quick check", "triage" | QUICK |
| "review" (no qualifier) | Auto-detect via score |

### Auto-Depth Scoring

Get the diff and compute a score. If no git diff is available (pasted snippet, file-only
review), estimate `lines_changed` from the snippet/file length and extract file paths
from context. If neither diff nor files are available, default to QUICK.

```
# Step 0: Gather inputs
changed_files = parse from git diff headers, user message, or file_paths argument
lines_changed = git diff --stat (non-test files only) or len(snippet.lines)
               → EXCLUDE lines from tests/ and *_test.* when counting

# Step 0b: Check memory patterns early (needed for scoring)
memory_patterns = call memory category "patterns"

SCORE = 0

# 1. Diff size (non-test lines only)
lines_changed > 500  → +3
lines_changed > 200  → +2
lines_changed > 50   → +1

# 2. File criticality (any changed file path matches)
auth/crypto/session/permission/middleware/token/jwt → +2
config files (*.toml, *.yml, schema)               → +1

# 3. Memory patterns (checked in Step 0b)
#    If any memory patterns match changed files → +2

# 4. Complexity markers (in diff text, excluding test files)
unsafe / Arc<Mutex / RwLock / select! / spawn → +1

HARD FLOOR: If any changed file matches security keywords (auth/crypto/session/
permission/middleware/token/jwt), minimum depth is STANDARD (never QUICK).

DECISION:
  SCORE >= 4  →  DEEP
  SCORE >= 2  →  STANDARD
  SCORE <  2  →  QUICK
```

### Transparency

Always display the depth decision and why:

```
Selected STANDARD review (score 3): +2 auth files, +1 diff size (82 lines)
```

```
Selected QUICK review (score 1): +1 diff size (55 lines), no critical files
```

```
Selected DEEP review (score 5): +3 diff size (620 lines), +2 security files
```

## Phase 1: Launch

Behavior depends on depth:

### QUICK
Skip directly to Phase 2 (Dispatch). No investigation, no Opus agent.

### STANDARD
1. Generate a `REVIEW_ID` (timestamp-based, e.g. `20260223-130500`)
2. Spawn Opus agent in background:
   ```
   Task(
     subagent_type: "general-purpose",
     run_in_background: true,
     prompt: <Opus prompt template below>,
     description: "Opus local review"
   )
   ```
3. Proceed immediately to Phase 2 — Opus runs concurrently with Squall dispatch

### DEEP
Sequential investigation by main Claude (no Opus — it would be redundant):

1. Check memory patterns for known issues in changed files (already fetched in Phase 0)
2. Read target files with the Read tool
3. Map control flow, trace callers/callees with Grep/Glob
4. Check git history (`git log`, `git blame`) on changed files
5. Form hypotheses — specific, testable claims with file:line references, informed by memory patterns
6. Write investigation notes — these become `investigation_context` AND inform per-model lenses

Investigation MUST complete before dispatch. Hypotheses inform lenses.

## Phase 2: Dispatch

1. **Consult memory** — call `memory` with category "recommend", then "tactics" (patterns already checked in Phase 0)
2. **Call `listmodels`** to get current model names and metadata (speed_tier, precision_tier, strengths, weaknesses)
3. **Select ensemble using tiered model system:**

   ### Tier 1: ALWAYS included (free + high quality)

   These 4 sources run on EVERY review regardless of depth:

   | Source | Why | Backend |
   |--------|-----|---------|
   | **gemini** | Best absolute quality, finds architectural bugs. Free (CLI). | `review` (uses clink internally) |
   | **codex** | Highest signal-to-noise, concise, finds correctness bugs. Free (CLI). | `review` (uses clink internally) |
   | **grok** | Best speed/quality ratio, fast, finds cross-function bugs. | `review` (HTTP) |
   | **Opus agent** | Local investigation — shell, git, tests, cross-file tracing. | `Task` (background agent) |

   For **QUICK**: Use only grok (single fastest Tier 1 model). Skip Opus agent.
   For **STANDARD** and **DEEP**: All 4 Tier 1 sources.

   ### Tier 2: Pick 2 based on situation

   For **STANDARD** and **DEEP**, add 2 models from Tier 2 based on the code type and
   memory data. Consult `memory` recommend + tactics + `listmodels` to choose:

   | Model | Best For | Notes |
   |-------|----------|-------|
   | **kimi-k2.5** | Security, edge cases, adversarial scenarios | 83% LiveCodeBench. Needs a focused lens to shine. |
   | **deepseek-v3.1** | Fast triage, broad coverage | Fastest model (~30s). Higher false positive rate — pair with focused lens. |
   | **deepseek-r1** | Deep reasoning, logic-heavy analysis | Chain-of-thought reasoner. Currently needs auth fix or Together reroute. |
   | **qwen-3.5** | Pattern matching, performance analysis | 397B MoE. Good with performance-focused lens. |
   | **qwen3-coder** | Code-specific review | Purpose-built for code. Add to config when available. |
   | **z-ai/glm-5** | Architectural framing | Trained on Issue-PR pairs. Needs OpenRouter credits. |
   | **mistral-large** | Multilingual, efficient | Needs API key configured. |

   **Selection criteria for Tier 2:**
   - Check `memory` recommend for success rates and tactics for proven lens combos
   - Match model strengths to the code type (concurrency → kimi, performance → qwen, etc.)
   - Include at least 1 model with <5 samples in memory (cold-start exploration)
   - Models with <70% success rate (>=5 samples) are **server-gated** — don't select them
   - Prefer diversity of strengths over raw success rate

   **ALWAYS show selection reasoning:**
   ```
   Tier 1: gemini, codex, grok, Opus agent (always)
   Tier 2: kimi-k2.5 (security lens, 100% success), deepseek-v3.1 (fast triage, 100% success)
   Rationale: Rust concurrency code — Kimi strong on edge cases, DV3.1 for broad coverage
   ```

4. **Build per-model lenses** using memory tactics + investigation hypotheses (if DEEP):

   Check `memory` category "tactics" for proven lens + model combos. Fall back to these defaults:

   | Strength Area | Default Lens |
   |---------------|-------------|
   | Systems/concurrency | "Focus on concurrency issues, resource leaks, memory safety, and deadlock potential" |
   | Correctness/logic | "Focus on logic errors, edge cases, off-by-one bugs, and step-by-step correctness" |
   | Surface bugs | "Focus on obvious bugs, null dereferences, missing validation, and common pitfalls" |
   | Adversarial | "Focus on edge cases, unusual inputs, race conditions, and adversarial scenarios" |

   Assign lenses based on each model's strengths (from listmodels or tactics), not by model name.
   For DEEP, tailor lenses to investigation hypotheses (see squall-deep-review examples).

5. **Call `review`** with appropriate parameters:
   - QUICK: just `prompt`, `models`, `diff`, `temperature: 0`
   - STANDARD: add `file_paths`, `working_directory`, `per_model_system_prompts`
   - DEEP: add `deep: true`, `investigation_context`, hypothesis-informed lenses

## Phase 3: Gather

1. **Read the `results_file`** from the review response
2. **If STANDARD**: Check Opus agent output
   - Call `TaskOutput` with `timeout: 60000` (60s max wait after Squall returns)
   - Read `.squall/reviews/opus-{REVIEW_ID}.md`
   - If file missing or agent failed → proceed without it (graceful degradation)
   - Note in output: "Opus agent: timed out / not available" if applicable
3. **If DEEP**: No Opus to gather — investigation was done in Phase 1
4. **Check quality gates**:
   - `warnings` — unknown model keys? Truncation?
   - `summary.models_succeeded` — if 0, diagnose before synthesizing

## Phase 4: Synthesize

### QUICK (single model)
Relay Grok findings directly. No multi-source synthesis needed.

### STANDARD (models + Opus)

Three-source triangulation when Opus is available:

| Signal | Confidence |
|--------|-----------|
| Models agree + Opus confirms | HIGHEST |
| Models agree, Opus silent | HIGH |
| Opus unique finding | MEDIUM (local context adds credibility) |
| Single model + Opus confirms | MEDIUM-HIGH |
| Opus + model contradict | FLAG for human judgment |

Without Opus, fall back to standard model-agreement synthesis:
- Consensus (2+ models) → HIGH confidence
- Unique catches (1 model) → MEDIUM confidence (this is WHY we use multiple models)
- Contradictions → FLAG for human

### DEEP (models + investigation hypotheses)

Cross-reference investigation with model findings:
- Hypotheses confirmed by models → HIGHEST confidence
- Hypotheses not flagged by any model → false alarm or models missed it, investigate further
- Model findings outside hypotheses → unexpected, worth attention

### Output Format

```
### Depth: STANDARD (score 3): +2 auth files, +1 diff size

### Consensus Findings (2+ sources agree)
- [critical] Description (models: gemini, codex; opus: confirmed)
- [high] Description (models: gemini, grok)

### Unique Catches
- [medium] Description (source: opus) — local investigation found caller mismatch
- [low] Description (model: codex) — edge case in error path

### Possible False Positives
- [medium] Description (model: grok) — matches known Grok blind spot

### Quality Notes
- 3/3 models succeeded, Opus agent: completed
- No warnings
```

## Phase 5: Memorize + Report

After synthesis, close the learning loop:

1. **`memorize` category "pattern"** — confirmed bugs or recurring issues (include model attribution)
2. **`memorize` category "tactic"** — lens effectiveness observations (which lens + model combo worked)
3. **`memorize` category "recommend"** — model performance notes (precision, false positives)

## Opus Agent Prompt Template

Used for STANDARD depth only. Caller fills `{placeholders}`.

```
You are an expert code reviewer doing LOCAL investigation. External AI models are
reviewing this code in parallel via Squall — your job is the perspective they CANNOT provide.

You have shell access. They don't. Use it:
1. Read changed files + trace their callers/callees across the codebase
2. Check test coverage: are the changed code paths tested? Run cargo test if useful.
3. Run git log/blame on changed files for context (why does this code exist?)
4. Grep for related patterns — is this change consistent with the rest of the codebase?
5. Look for integration issues: does this change break any callers?

CONSTRAINTS:
- Do NOT use ToolSearch for Squall tools. No review/chat/clink/memory calls.
- Do NOT modify source code. Compilation (cargo test) and git commands are fine.
  Write ONLY to your output file.
- If running tests, scope to changed modules (e.g. cargo test module_name),
  NOT the full suite — you have limited time.
- Always use non-interactive flags (e.g. git --no-pager log).
- Focus on what static text analysis misses: cross-file interactions,
  test coverage gaps, git history context, inconsistencies.

Changed files: {file_list}
Diff summary: {diff_stat}
Working directory: {working_directory}

Write findings to: {output_file}

OUTPUT FORMAT — use these exact sections:
## Findings
### [severity] Finding title
- File: path/to/file.rs:line
- Evidence: what you found
- Risk: why it matters

## Test Coverage Assessment
(Which changed paths have tests, which don't)

## Cross-File Integration Notes
(Any caller/callee issues, API contract changes)

## Git History Context
(Relevant history that informs the review)

Be specific. Cite exact file paths and line numbers. If no real issues, say so briefly.
```

## Ensemble Selection Reference

| Intent | Depth | Tier 1 | Tier 2 (pick 2) | Opus? | Investigation? |
|--------|-------|--------|-----------------|-------|----------------|
| Small non-critical change | QUICK | grok only | none | No | No |
| Normal PR, routine code | STANDARD | gemini + codex + grok | 2 by code type + memory data | Yes (parallel) | No |
| Security, critical infra | DEEP | gemini + codex + grok | 2 by code type + memory data | No (main Claude) | Yes (sequential) |

**Tier 1 is non-negotiable** — gemini and codex are free and high quality, grok is fast and high quality.
**Always call `listmodels` first** — model availability changes. Use `memory` category "recommend" for Tier 2 selection.
Models with <70% success rate (>=5 samples) are automatically rejected by Squall's hard gate.

## Anti-Patterns

| Don't | Do Instead |
|-------|------------|
| Hardcode model names in ensemble selection | Read memory recommend + listmodels, reason about model fit for THIS diff |
| Ignore memory stats when picking models | Always check success rates, latency, and tactics before selecting |
| Pick models by name alone | Pick by success rate + strength match for the code type |
| Skip depth detection and always use 5 models | Let auto-depth match ensemble to intent |
| Use QUICK for security-sensitive files | Hard floor ensures minimum STANDARD |
| Skip `listmodels` before calling review | Model availability changes — always check |
| Omit `per_model_system_prompts` | Lenses are a huge quality improvement — always set them |
| Spawn Opus agent for DEEP reviews | Main Claude already investigates — Opus is redundant |
| Spawn Opus agent for QUICK reviews | Overhead isn't worth it for single-model triage |
| Assume Opus will always succeed | Graceful degradation — synthesize without it if needed |
| Start DEEP dispatch before investigation | Investigation MUST complete first — hypotheses inform lenses |
| Forget to show depth reasoning | Transparency: always display score breakdown AND model selection rationale |
| Skip memory before dispatch | Call `memory` recommend + tactics + patterns first |
| Skip memorize after synthesis | Close the learning loop — patterns, tactics, recommendations |
| Make separate chat/clink calls for multi-model | Use `review` — it does parallel fan-out with straggler cutoff |
| Ignore `results_file` in the response | Always read persisted file — it survives compaction |
| Select a model you know is server-gated | Squall rejects models <70% success (>=5 samples) — don't waste a slot |
| Always pick the same 3 "proven" models | Include at least 1 model with <5 samples for exploration (cold-start diversity) |
| Assume infrastructure failures = model quality | Auth/credit failures inflate error rates — check `reason` field, not just `status` |
| Skip per_model_system_prompts for "weak" models | Lenses transform average models into unique contributors (Kimi: B→A with security lens) |

## Backward Compatibility

- `/squall-review` still works — redirects to this skill at STANDARD depth
- `/squall-deep-review` still works — redirects to this skill at DEEP depth
- Explicit depth keywords ("deep review", "quick review") override auto-detection

## Related Skills

- [squall-research](../squall-research/SKILL.md) — Multi-agent research swarms (not code review)
- [squall-deep-research](../squall-deep-research/SKILL.md) — Deep research via Codex/Gemini web search

<!-- SENTINEL:SESSION_LEARNINGS_START - Do not remove this line -->
## Session Learnings Archive

Insights captured during skill execution:

### 2026-02-23: First self-review found 5 actionable fixes
**Origin**: Ran unified review on its own implementation (STANDARD depth, score 2)
**Core insight**: The skill's first run exposed: (1) no snippet fallback path, (2) Opus "read-only" vs "cargo test" contradiction, (3) memory patterns needed in Phase 0 not Phase 2, (4) test file scoring used "ALL changed files" instead of excluding tests from line count, (5) overlapping activation_triggers caused three-way ambiguity. All 5 fixed same session. Opus agent's unique value confirmed — found trigger overlap and stale cross-references that external models missed.
**Harvested** -> Applied to this skill file (Phase 0 scoring, Opus prompt constraints, anti-patterns)

### 2026-02-23: Closed the feedback loop — adaptive model selection + hard gates
**Origin**: 6-model architecture review (Grok, Codex, Gemini, DeepSeek, then Codex+Gemini round 2) on how to close the gap between memory benchmarks and model selection.
**Core insight**: Claude IS the adaptive reasoning layer — building a bandit algorithm in Rust would be a dumber duplicate. The fix is simpler: (1) skill says "read memory, reason about model fit" instead of hardcoded tables, (2) Squall enforces hard gates in Rust (reject models <70% success with >=5 samples). No DuckDB needed at current scale. Bandits, DuckDB, intent-based API deferred to v2. Codex+Gemini both reversed their bandit recommendation once they learned Claude is the client.
**Harvested** -> Phase 2 rewritten for adaptive selection, hard gates added to src/review.rs, 7 TDD tests

### 2026-02-23: 9-model benchmark proves diversity is mandatory + finds 4 bugs in hard gate
**Origin**: Research swarm (3 agents + team lead) investigated selection bias, model capabilities, and divergent value. Ran 9-model benchmark twice (generic + lensed).
**Core insight**: Every model found something unique across both runs. Kimi went from "no unique findings" to "found a display bug nobody else saw" simply by getting a security-focused lens. The benchmark found 4 bugs in the hard gate implementation we just built: (1) Gemini found API accounting bug — `models_requested` lies after gating, (2) Codex found infrastructure failures conflated with quality failures — cutoff/auth/rate-limit errors counted as model quality, (3) Grok found case-sensitive HashMap lookups bypass gate, (4) Kimi found `{:.0}%` formatting rounds 69.9% to "70%" in warning. Analyst found model identity fragmentation: config keys (kimi-k2.5) differ from memory names (moonshotai/Kimi-K2.5), making the hard gate a silent no-op for 4+ models.
**Key numbers**: 3-model oligopoly (grok/codex/gemini = 64% of all calls). 3/9 models failed due to auth/credits (infrastructure, not capability). 7 of 8 unique bugs found by different individual models.
**Harvested** -> 3 new anti-patterns added (diversity, infrastructure failures, lens assignment). Benchmark comparison at .squall/reviews/benchmark-comparison.md. Bugs to fix: model identity normalization, failure reason discrimination, display rounding, API accounting.

### 2026-02-23: Fixed 5 hard gate bugs + updated model config
**Origin**: Bugs found during 9-model benchmark + model identity analysis.
**Core insight**: (1) Model identity fragmentation — models.md logged provider model_ids ("deepseek-ai/DeepSeek-V3.1") instead of config keys ("deepseek-v3.1"). Hard gate lookups missed these. Fix: normalization map (model_id→config_key) in registry, threaded through get_model_stats and log_model_metrics. (2) Infrastructure failures (auth, rate limits) counted as quality failures → models permanently gated. Fix: added reason column to models.md, excluded auth_failed/rate_limited from success rate denominator. (3) Display rounding {:.0}% showed "70%" for 69.9% → fixed to {:.1}%. (4) models_requested counted post-gate → fixed to pre-gate count, added models_gated field. (5) Partial successes inflated stats → now check partial != "yes". Also updated deepseek-v3.1→V3.2, rerouted deepseek-r1 to Together (fixes auth), added qwen3-coder model.
**Harvested** -> 6 new tests (399 total). Event format now has 8 columns (backward-compatible with old 7-column format via cols.len() detection).

---
<!-- SENTINEL:SESSION_LEARNINGS_END -->
