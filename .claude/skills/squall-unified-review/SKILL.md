---
name: squall-unified-review
description: "Unified code review with auto-depth detection, Opus agent, and optional agent teams. Use when asked to 'review', 'squall review', 'code review', 'deep review', 'swarm review', 'team review', 'quick review', or 'check this code'. Auto-selects QUICK/STANDARD/DEEP/SWARM based on code characteristics. (project)"
one_liner: "Auto-depth review — from single-model triage to multi-agent swarm."
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
  - "swarm review"
  - "team review"
  - "full investigation"
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
> | Auto-depth | Claude scores the diff → QUICK / STANDARD / DEEP / SWARM |
> | User override | "deep review" forces DEEP, "quick review" forces QUICK, "swarm review" forces SWARM |
> | Opus agent | Background Task agent for STANDARD + DEEP — local investigation (shell, git, tests) |
> | DEEP investigation | Main Claude investigates first (hypotheses), then Opus + models run in parallel |
> | SWARM agents | 3 independent team agents (security, correctness, architecture), each investigating + dispatching |
> | Degradation | SWARM → DEEP if agent teams unavailable (always notified, never silent) |
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
| "swarm review", "team review", "full investigation" | SWARM |
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
  SCORE >= 6  →  SWARM (if agent teams available; else DEEP with notification)
  SCORE >= 4  →  DEEP
  SCORE >= 2  →  STANDARD
  SCORE <  2  →  QUICK
```

### SWARM Availability Check

SWARM requires Claude Code agent teams (`CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`).

To check availability: use `ToolSearch` with query `"TeamCreate"`. If the tool is not found, teams
are unavailable. Alternatively, attempt `TeamCreate` — if it fails, catch the error and degrade.

If SWARM is selected (by score or user override) but teams are unavailable:
- **Always notify**: "SWARM requested (score N) but agent teams unavailable — falling back to DEEP"
- Never silently degrade. High-stakes code getting a downgraded review must be visible.
- Proceed with DEEP workflow.

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

```
Selected SWARM review (score 7): +3 diff size (700 lines), +2 security, +2 memory patterns
  → 3 investigation agents: security, correctness, architecture
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
Sequential investigation by main Claude, then Opus + models in parallel:

1. Check memory patterns for known issues in changed files (already fetched in Phase 0)
2. Read target files with the Read tool
3. Map control flow, trace callers/callees with Grep/Glob
4. Check git history (`git log`, `git blame`) on changed files
5. Form hypotheses — specific, testable claims with file:line references, informed by memory patterns
6. Write investigation notes — these become `investigation_context` AND inform per-model lenses
7. **Spawn Opus agent** (same as STANDARD, but with hypotheses added to its prompt)
8. Proceed to Phase 2 — Opus runs concurrently with Squall dispatch

Investigation MUST complete before dispatch. Hypotheses inform both model lenses AND the Opus agent.
The Opus agent gets hypotheses as additional context so it can validate or challenge them independently.

### SWARM
Team-based parallel investigation. 3 independent agents, each with a different lens, each doing local
investigation AND dispatching its own Squall review. **No Opus agent** — the 3 agents replace Opus.

**The team lead orchestrates only — it does NOT investigate, dispatch, or review code itself.**
It creates the team, assigns tasks, waits for all agents, then synthesizes their output.

1. Generate `REVIEW_ID` (timestamp-based)
2. `TeamCreate` with name `squall-review-{REVIEW_ID}`
3. `TaskCreate` × 3 tasks:
   - "Security investigation: {REVIEW_ID}" (activeForm: "Investigating security")
   - "Correctness investigation: {REVIEW_ID}" (activeForm: "Investigating correctness")
   - "Architecture investigation: {REVIEW_ID}" (activeForm: "Investigating architecture")
4. Spawn 3 `general-purpose` teammates **in parallel** (single message, 3 `Task` tool calls), each with:
   - `team_name: "squall-review-{REVIEW_ID}"`
   - `name: "security"` / `"correctness"` / `"architecture"`
   - `mode: "bypassPermissions"` — agents must run autonomously, no permission prompts
   - Lens-specific prompt (see SWARM Agent Prompt Templates below)
   - Fill `{LENS_SLUG}` as the lowercase lens name (e.g. `security`, `correctness`, `architecture`)
5. **Skip to Phase 3 (Gather)** — agents handle their own dispatch (Phase 2 is internal to each agent)

**SWARM agent model assignments:**

| Agent | Models | Rationale |
|-------|--------|-----------|
| Security | kimi-k2.5, codex, grok | kimi: adversarial edge cases. codex: precision anchor (0 FP). grok: fast cross-function. |
| Correctness | codex, deepseek-v3.1, gemini | codex: precision. gemini: systems-level bugs. deepseek: fast broad coverage. |
| Architecture | gemini, codex, qwen-3.5 | gemini: architectural bugs. codex: contract violations. qwen: pattern matching. |

Model overlap across agents is intentional — same model with different lens produces different analysis.
Cross-agent overlap creates consensus signal (codex flagging the same issue through security AND
correctness lens = very high confidence).

## Phase 2: Dispatch

**For SWARM**: Skip this phase entirely. Each SWARM agent handles its own dispatch internally
(calls `listmodels`, `memory`, and `review` with its lens-specific models and system prompts).
Proceed to Phase 3. The instructions below apply to QUICK, STANDARD, and DEEP only.

1. **Consult memory** — call `memory` with category "recommend", then "tactics" (patterns already checked in Phase 0)
2. **Call `listmodels`** to get current model names and metadata (speed_tier, precision_tier, strengths, weaknesses)
3. **Select ensemble using tiered model system:**

   ### Tier 1: ALWAYS included (free + high quality)

   These 4 sources run on every QUICK/STANDARD/DEEP review (SWARM uses per-agent ensembles instead):

   | Source | Why | Backend |
   |--------|-----|---------|
   | **gemini** | Best absolute quality, finds architectural bugs. Free (CLI). | `review` (uses clink internally) |
   | **codex** | Highest signal-to-noise, concise, finds correctness bugs. Free (CLI). | `review` (uses clink internally) |
   | **grok** | Best speed/quality ratio, fast, finds cross-function bugs. | `review` (HTTP) |
   | **Opus agent** | Local investigation — shell, git, tests, cross-file tracing. | `Task` (background agent) |

   For **QUICK**: Use only grok (single fastest Tier 1 model). Skip Opus agent.
   For **STANDARD**: All 4 Tier 1 sources (Opus runs in parallel with dispatch).
   For **DEEP**: All 4 Tier 1 sources (Opus spawned after investigation, runs in parallel with dispatch).

   ### Tier 2: Pick 2 based on situation

   For **STANDARD** and **DEEP**, add up to 2 models from Tier 2 based on the code type and
   memory data. If no Tier 2 models pass the gate threshold (>=70% success, >=5 samples),
   proceed with Tier 1 only and note this in selection reasoning.

   Consult `memory` recommend + `memory` tactic + `listmodels` to choose:

   | Model | Best For | Notes |
   |-------|----------|-------|
   | **kimi-k2.5** | Security, edge cases, adversarial scenarios | 83% LiveCodeBench. Needs a focused lens to shine. |
   | **deepseek-v3.1** | Fast triage, broad coverage | Config key is `deepseek-v3.1` (provider model is V3.2-Exp). Fastest model (~30s). Pair with focused lens. |
   | **deepseek-r1** | Deep reasoning, logic-heavy analysis | Chain-of-thought reasoner. Routed via Together — persistent auth failures, check memory before selecting. |
   | **qwen-3.5** | Pattern matching, performance analysis | 397B MoE. Good with performance-focused lens. |
   | **qwen3-coder** | Code-specific review | Purpose-built for code. |
   | **z-ai/glm-5** | Architectural framing | Trained on Issue-PR pairs. Needs OpenRouter credits — check memory for 402 errors. |
   | **mistral-large** | Multilingual, efficient | Needs API key configured — check memory for auth failures. |

   **Selection criteria for Tier 2:**
   - Check `memory` recommend for success rates and `memory` tactic for proven lens combos
   - Match model strengths to the code type (concurrency → kimi, performance → qwen, etc.)
   - Include at least 1 model with <5 samples in memory (cold-start exploration)
   - Models with <70% success rate (>=5 samples) are **server-gated** — don't select them
   - Check for persistent infrastructure failures (auth, credits) in memory — these models won't be gated but will waste a dispatch slot
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
   - DEEP: add `deep: true`, `investigation_context` (string field on the `review` tool — your investigation notes/hypotheses), hypothesis-informed lenses

## Phase 3: Gather

### SWARM

**Wait for all teammates — do not synthesize alone.** Agents may take 15+ minutes each
(local investigation + deep review with 600s Squall ceiling). Be patient.

1. Monitor via `TaskList` and incoming `SendMessage` notifications from agents
2. Wait until all 3 agents have completed their tasks (or an agent appears stuck — no progress
   for 5+ minutes after claiming its task, which may indicate a tool failure)
3. **Partial completion**: if an agent fails or stalls, proceed with available output and flag
   the missing lens: "Architecture agent failed — synthesis covers security + correctness only"
4. Read completed agent output files from `.squall/reviews/swarm-{REVIEW_ID}-{LENS_SLUG}.md`
   (where `{LENS_SLUG}` is `security`, `correctness`, or `architecture`)
5. Send `SendMessage` with `type: "shutdown_request"` to each teammate, then `TeamDelete`
6. Proceed to Phase 4 (Synthesize → SWARM section)

### QUICK / STANDARD / DEEP

1. **Read the `results_file`** from the review response
2. **If STANDARD or DEEP**: Check Opus agent output
   - Call `TaskOutput` with `timeout: 60000` (60s max wait after Squall returns)
   - Read `.squall/reviews/opus-{REVIEW_ID}.md`
   - If file missing or agent failed → proceed without it (graceful degradation)
   - Note in output: "Opus agent: timed out / not available" if applicable
3. **Check quality gates**:
   - `warnings` — unknown model keys? Truncation?
   - `not_started` — if non-empty, model was requested but not configured. Remove from future selections.
   - `summary.models_gated` — if >0, check which models were gated (especially Tier 1). Adapt synthesis expectations.
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

### DEEP (models + Opus + investigation hypotheses)

Three-way cross-reference — investigation hypotheses, model findings, and Opus agent:

| Signal | Confidence |
|--------|-----------|
| Hypothesis confirmed by models + Opus | HIGHEST |
| Hypothesis confirmed by models, Opus silent | HIGH |
| Hypothesis confirmed by Opus only | MEDIUM-HIGH (local evidence) |
| Hypothesis not flagged by anyone | Likely false alarm — drop or note as speculative |
| Opus unique finding outside hypotheses | MEDIUM-HIGH (local context credibility) |
| Model finding outside hypotheses | MEDIUM (unexpected, worth attention) |
| Opus + model contradict on hypothesis | FLAG for human judgment |

### SWARM (3 agents × 3 models each)

Read all completed agent output files. Cross-reference findings across agents:

| Signal | Confidence |
|--------|-----------|
| Found by 2+ agents' local investigation | CRITICAL (independent local evidence) |
| 1 agent local + models in another agent confirm | HIGH |
| Same model flags it through different lenses | HIGH (e.g. codex via security + correctness) |
| 1 agent local only | MEDIUM-HIGH (local evidence is credible) |
| Multiple models within 1 agent | MEDIUM |
| Single model only | MEDIUM |
| Agents contradict | FLAG for human judgment |

Group findings by consensus strength, not by agent. The output should unify across lenses:

```
### Depth: SWARM (score 7): +3 diff size, +2 security, +2 memory patterns
### Investigation: 3 agents (security, correctness, architecture) × 3 models each

### Cross-Agent Consensus (found by 2+ agents)
- [critical] Description (agents: security + correctness; models: codex, kimi, gemini)
  - Security perspective: [detail]
  - Correctness perspective: [detail]

### Agent-Specific Findings
#### Security
- [high] Description (source: local investigation + codex) — trust boundary at file:line

#### Correctness
- [medium] Description (source: gemini + deepseek-v3.1) — off-by-one at file:line

#### Architecture
- [medium] Description (source: local investigation) — layering violation

### Contradictions
- [flag] Security says X, Architecture says Y — needs human judgment

### Coverage Summary
- Security: 2 local + 4 model findings, agent completed
- Correctness: 1 local + 3 model findings, agent completed
- Architecture: agent timed out — findings from partial output only
```

### Output Format (QUICK / STANDARD / DEEP)

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

Used for STANDARD and DEEP depth. Caller fills `{placeholders}`.
For DEEP, add `{hypotheses}` section with investigation hypotheses for the agent to validate.

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

{hypotheses_section}

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

## SWARM Agent Prompt Templates

Used for SWARM depth. One prompt per lens, parameterized by `{placeholders}`.
Each agent loads Squall tools via ToolSearch, investigates locally, then dispatches its own Squall review.

### Generic Template (fill {LENS}, {LENS_SLUG}, {INVESTIGATION_STEPS}, {MODELS})

`{LENS}` is the display name (e.g. "Security"). `{LENS_SLUG}` is the lowercase identifier
(e.g. `security`, `correctness`, `architecture`) — used in filenames and task matching.

```
You are the {LENS} investigator in a review swarm.
Your shore is {LENS} — own it completely.

You operate autonomously. No human gates your work.
Investigate deep, dispatch to models, write your findings, report back.

Two jobs, one mission:
1. LOCAL INVESTIGATION — shell, git, tests. Find what static analysis cannot.
2. SQUALL DISPATCH — 3 external models amplify your {LENS} lens.

## Step 1: Claim your task
- Call ToolSearch with query "squall" to load Squall MCP tools
- Call TaskList, find the task with "{LENS}" in its title, then TaskUpdate it to in_progress

## Step 2: Investigate locally
- Read changed files: {file_list}
- {INVESTIGATION_STEPS}
- Form specific hypotheses with file:line references
- Write investigation notes — these become your system prompts for the models

## Step 3: Dispatch to models
- Call `listmodels` to verify model names
- Call `memory` category "tactics" for proven system prompts
- Call `review` with:
  - models: {MODELS}
  - per_model_system_prompts: build from YOUR investigation findings + {LENS} focus
  - diff: {diff}
  - file_paths: {file_paths}
  - working_directory: {working_directory}
  - deep: true
  (This call may take 10+ minutes — the Squall ceiling is 600s. Be patient.)

## Step 4: Synthesize & write output
- Read the results_file from the review response
- Combine YOUR local findings with model findings
- Write to: .squall/reviews/swarm-{REVIEW_ID}-{LENS_SLUG}.md

OUTPUT FORMAT:
## {LENS} Investigation: {REVIEW_ID}

### Local Findings
#### [severity] Finding title
- **File**: path/to/file.rs:line
- **Evidence**: what you found locally (shell, git, tests)
- **Risk**: why it matters from a {LENS} perspective

### Model Findings
#### [severity] Finding title
- **Models agreeing**: [list]
- **Detail**: what models found

### {LENS} Assessment
Overall: safe / concerning / critical

## Step 5: Report and complete
- TaskUpdate your task to completed
- SendMessage to "team-lead" with a 2-3 sentence summary of your findings

CONSTRAINTS:
- Do NOT modify source code. Read-only investigation + Squall calls only.
- Write ONLY to your output file: .squall/reviews/swarm-{REVIEW_ID}-{LENS_SLUG}.md
- Do NOT read other agents' output files in .squall/reviews/. Your investigation must be independent.
- Always use non-interactive git flags (e.g. git --no-pager log).
- Focus on {LENS}-specific issues — other agents cover other lenses.
- If running tests, scope to changed modules, not the full suite.
```

### Lens-Specific Investigation Steps

**Security** (`{MODELS}`: kimi-k2.5, codex, grok):
- Trace all trust boundary crossings in changed code
- Check input validation and sanitization paths
- `git blame` on security-critical lines
- Grep for `unsafe`, `unwrap`, hardcoded secrets, TODO security comments
- Check if security-relevant tests exist for changed paths

**Correctness** (`{MODELS}`: codex, deepseek-v3.1, gemini):
- Trace control flow through changed functions, map all branches
- Run targeted tests (`cargo test module_name`) on changed modules
- Check error handling completeness (all error paths covered?)
- Verify changed public APIs still satisfy existing callers (grep for callers)

**Architecture** (`{MODELS}`: gemini, codex, qwen-3.5):
- Map dependency graph of changed modules (who imports what)
- Check for API contract changes by diffing public function signatures
- Grep for performance anti-patterns (allocations in hot loops, clone() chains)
- `git log --oneline -20` on changed files for refactoring context

## Ensemble Selection Reference

| Intent | Depth | Tier 1 | Tier 2 (pick 2) | Opus? | Investigation? |
|--------|-------|--------|-----------------|-------|----------------|
| Small non-critical change | QUICK | grok only | none | No | No |
| Normal PR, routine code | STANDARD | gemini + codex + grok | 2 by code type + memory data | Yes (parallel with dispatch) | No |
| Security, critical infra | DEEP | gemini + codex + grok | 2 by code type + memory data | Yes (parallel with dispatch, gets hypotheses) | Yes (sequential, before dispatch) |
| Large + security + memory patterns | SWARM | 3 agents × 3 models each | Per-agent (see SWARM tables) | No (agents replace Opus) | Yes (per agent, parallel) |

**Tier 1 is strongly preferred** — gemini and codex are free and high quality, grok is fast and high quality. Always request them. Note: if a Tier 1 model drops below 70% success rate (>=5 samples), Squall's hard gate will exclude it server-side. Check `warnings` and `summary.models_gated` in the response — if Tier 1 models are gated, note this in synthesis.
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
| Spawn Opus agent for QUICK reviews | Overhead isn't worth it for single-model triage |
| Skip Opus agent for DEEP reviews | DEEP should have MORE sources, not fewer — Opus validates hypotheses independently |
| Assume Opus will always succeed | Graceful degradation — synthesize without it if needed |
| Start DEEP dispatch before investigation | Investigation MUST complete first — hypotheses inform lenses |
| Forget to show depth reasoning | Transparency: always display score breakdown AND model selection rationale |
| Skip memory before dispatch | Call `memory` recommend + tactic + patterns first |
| Skip memorize after synthesis | Close the learning loop — patterns, tactics, recommendations |
| Make separate chat/clink calls for multi-model | Use `review` — it does parallel fan-out with straggler cutoff |
| Ignore `results_file` in the response | Always read persisted file — it survives compaction |
| Select a model you know is server-gated | Squall rejects models <70% success (>=5 samples) — don't waste a slot |
| Always pick the same 3 "proven" models | Include at least 1 model with <5 samples for exploration (cold-start diversity) |
| Assume infrastructure failures = model quality | Auth/credit failures inflate error rates — check `reason` field, not just `status` |
| Skip per_model_system_prompts for "weak" models | Lenses transform average models into unique contributors (Kimi: B→A with security lens) |
| Spawn SWARM for score < 6 | SWARM costs ~3x tokens vs DEEP — reserve for high-stakes changes |
| Use SWARM without checking team availability | Always check first, degrade to DEEP with notification if unavailable |
| Silently degrade from SWARM to DEEP | Always notify — high-stakes code getting downgraded review must be visible |
| Have team lead investigate or dispatch | Team lead orchestrates and synthesizes ONLY — agents own their shores |
| Let SWARM agents read each other's output files | Each agent must investigate independently for the cross-reference matrix to work |
| Skip TeamDelete after SWARM | Always clean up — orphan teammates waste resources |
| Skip memorize after SWARM synthesis | 3x more signal = 3x more to learn from |
| Synthesize before all agents report back | Wait for all teammates — do not synthesize alone. Patience > premature results |
| Spawn SWARM agents without bypassPermissions | Agents must run autonomously — permission prompts stall the swarm |
| Set a short fixed timeout for SWARM agents | Agents call deep review (600s ceiling) — be patient, monitor for stalls instead |

## Backward Compatibility

- `/squall-review` still works — redirects to this skill at STANDARD depth
- `/squall-deep-review` still works — redirects to this skill at DEEP depth
- Explicit depth keywords ("deep review", "quick review", "swarm review") override auto-detection
- SWARM degrades gracefully to DEEP when agent teams are unavailable — no Rust code changes needed
- SWARM is entirely a skill-layer pattern using existing primitives (TeamCreate, TaskCreate, SendMessage, Squall `review`)

## Related Skills

- [squall-research](../squall-research/SKILL.md) — Multi-agent research swarms (not code review)
- [squall-deep-research](../squall-deep-research/SKILL.md) — Deep research via Codex/Gemini web search

<!-- SENTINEL:SESSION_LEARNINGS_START - Do not remove this line -->
## Session Learnings Archive

Insights captured during skill execution:

### 2026-02-24: SWARM implementation review found 9 issues — placeholder contracts + lifecycle duplication
**Origin**: 3-model review (gemini, codex, grok) of the SWARM skill diff with lens-specific prompts (runtime-execution, contract-verification, usability).
**Core insight**: Skill text for multi-agent workflows has a specific failure mode: **lifecycle phase duplication**. Phase 1 had monitoring + partial synthesis that Phase 3 repeated — Claude would get confused about which phase owns completion logic. Fix: Phase 1 = launch only, Phase 3 = all monitoring. Second pattern: **placeholder contracts** — declaring `{LENS_FOCUS}` but using `{LENS_SLUG}`, or using `{lens}` in gather but `{LENS_SLUG}` in the agent template. Every placeholder must have exactly one canonical name and one definition point. Third: "EVERY review" wording is dangerous when adding a new depth that doesn't include a component — always qualify with which depths a statement applies to.
**Model performance**: Codex found the most issues (8) with highest precision (all actionable). Gemini found 5, including the best fix for availability check (ToolSearch for TeamCreate). Grok provided useful overall assessment (9/10) but fewer unique findings. The contract-verification lens was the most productive — good tactic for skill-file reviews.
**Harvested** -> 9 fixes applied: removed Phase 1/3 duplication, fixed `{LENS_FOCUS}` → `{LENS_SLUG}`, unified filename tokens, added ToolSearch availability check, fixed "tactic" → "tactics", qualified Tier 1 scope, specified SendMessage shutdown format, added lens-based task matching.

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

### 2026-02-23: DEEP review gets Opus agent — more sources for higher stakes
**Origin**: User feedback: "i think deep not having a separate claude reviewer is a miss"
**Core insight**: The original rationale ("Opus is redundant because Claude already investigates") was backwards. Investigation and independent review serve different purposes: investigation forms hypotheses (questions), Opus provides independent validation (answers). DEEP is the highest-stakes review — it should have MORE signal sources, not fewer. DEEP now runs: (1) Claude investigates sequentially → forms hypotheses, (2) Opus agent + 5 Squall models dispatch in parallel, (3) synthesize from 6+ sources with three-way cross-referencing (hypotheses vs models vs Opus). Opus gets the hypotheses as additional context so it can specifically validate or challenge them.
**Harvested** -> Updated Phase 1, Phase 3, Phase 4, Opus prompt template, ensemble reference table, anti-patterns

### 2026-02-24: Skill quality IS the architecture — competitive analysis validates design
**Origin**: Independent comparison of Squall vs PAL (a competing MCP server with 15+ hardcoded system prompts and server-side conversation state).
**Core insight**: The analysis identified 4 "gaps": no tool specialization (no baked-in review/secaudit/debug prompts), no conversation state, no multimodal, narrower providers. All four are features, not bugs — they follow from the thesis "Claude is the intelligence, Squall is transport + memory." PAL bakes 18KB of static Python prompts into the server. Squall lets Claude dynamically construct per_model_system_prompts based on the diff, memory stats, proven tactics, and model strengths. This is strictly more capable — but ONLY if the skill file is good. A thin skill file makes Claude fall back to generic prompts and the "no specialization" criticism lands. The fix is always improving the skill, never baking prompts into Rust. Same for conversation state: Claude Code IS the thread. PAL's SessionStore solves a problem Claude Code doesn't have. Squall's results_file persistence handles the compaction edge case better than server-side session state.
**Key principle**: Squall's output quality = skill file quality. This skill file is the product, not just documentation.
**Harvested** -> This learning itself. Reinforces why skill updates are mandatory after every task.

### 2026-02-23: 5-agent swarm audit found 1 code bug + 7 skill text gaps
**Origin**: 5-agent team (researcher, architect, scribe, builder, tester) traced all model selection logic, dispatch paths, and naming lifecycle end-to-end.
**Core insight**: (1) Code bug: `compute_summary()` and `generate_recommendations()` in memory.rs didn't normalize model names — legacy events with provider model_ids (e.g. "moonshotai/Kimi-K2.5") showed up in `memory recommend` output instead of config keys ("kimi-k2.5"). Fixed by threading `id_to_key` map through both functions via MemoryStore field. (2) Skill text: "non-negotiable" for Tier 1 was misleading — server CAN gate them. Changed to "strongly preferred" with gate acknowledgment. (3) "pick 2" had no escape clause for when all Tier 2 models are gated. (4) Phase 3 quality gates didn't check `not_started` or `models_gated`. (5) "tactics" vs "tactic" category naming inconsistency. (6) Stale model notes (deepseek-r1, deepseek-v3.1). (7) Zombie models (100% infra failures never gated) identified as architectural concern — not yet fixed.
**Harvested** -> memory.rs normalization fix, 7 skill text updates, MemoryStore.with_id_to_key() builder

---
<!-- SENTINEL:SESSION_LEARNINGS_END -->
