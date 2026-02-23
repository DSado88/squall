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
2. **Call `listmodels`** to get current model names
3. **Select ensemble by depth:**

| Depth | Models | deep flag | Typical Time |
|-------|--------|-----------|-------------|
| QUICK | grok-4-1-fast-reasoning | false | ~30s |
| STANDARD | grok + gemini + codex | false | ~120s |
| DEEP | gemini + codex + kimi-k2.5 | true | ~180s |

DEEP drops Grok (too many FPs for critical reviews) and uses 3 high-precision models.

4. **Build per-model lenses** using memory tactics + investigation hypotheses (if DEEP):

**Default lenses (override with memory tactics when available):**

| Model | Default Lens |
|-------|-------------|
| Gemini | "Focus on systems-level bugs, concurrency issues, resource leaks, and memory safety" |
| Codex | "Focus on logic errors, edge cases, off-by-one bugs, and step-by-step correctness reasoning" |
| Grok | "Focus on obvious bugs, null dereferences, missing validation, and common pitfalls" |
| Kimi | "Focus on edge cases, unusual inputs, race conditions, and adversarial scenarios" |

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
Relay Grok findings directly. No synthesis needed.

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

| Intent | Depth | Models | Opus? | Investigation? |
|--------|-------|--------|-------|----------------|
| Small non-critical change | QUICK | Grok | No | No |
| Normal PR, routine code | STANDARD | Grok + Gemini + Codex | Yes (parallel) | No |
| Security, critical infra | DEEP | Gemini + Codex + Kimi | No (main Claude) | Yes (sequential) |

**Always call `listmodels` first** — model names are examples and may change.

## Anti-Patterns

| Don't | Do Instead |
|-------|------------|
| Skip depth detection and always use 5 models | Let auto-depth match ensemble to intent |
| Use QUICK for security-sensitive files | Hard floor ensures minimum STANDARD |
| Skip `listmodels` before calling review | Model names change — always check |
| Omit `per_model_system_prompts` | Lenses are a huge quality improvement — always set them |
| Spawn Opus agent for DEEP reviews | Main Claude already investigates — Opus is redundant |
| Spawn Opus agent for QUICK reviews | Overhead isn't worth it for single-model triage |
| Assume Opus will always succeed | Graceful degradation — synthesize without it if needed |
| Start DEEP dispatch before investigation | Investigation MUST complete first — hypotheses inform lenses |
| Forget to show depth reasoning | Transparency: always display score breakdown |
| Skip memory before dispatch | Call `memory` recommend + tactics + patterns first |
| Skip memorize after synthesis | Close the learning loop — patterns, tactics, recommendations |
| Make separate chat/clink calls for multi-model | Use `review` — it does parallel fan-out with straggler cutoff |
| Ignore `results_file` in the response | Always read persisted file — it survives compaction |

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

---
<!-- SENTINEL:SESSION_LEARNINGS_END -->
