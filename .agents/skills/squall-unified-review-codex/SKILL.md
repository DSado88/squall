---
name: squall-unified-review-codex
description: "Codex-flavored unified code review with auto-depth detection and Codex subagents. Mirror of squall-unified-review for the Claude side. Use when asked to 'review', 'squall review', 'code review', 'deep review', 'swarm review', 'team review', 'quick review', or 'check this code' inside Codex CLI. Auto-selects QUICK/STANDARD/DEEP/SWARM. (project)"
one_liner: "Codex hosts the agent. Squall fans out the consultants. Same skill, mirrored backends."
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
  - When user wants multi-model code review under Codex CLI
related_skills:
  - "[squall-deep-research](../squall-deep-research/SKILL.md) (for deep research questions)"
---

# Squall Unified Review — Codex Variant

> **TL;DR — QUICK REFERENCE**
>
> | Concept | Translation |
> |---------|-------------|
> | Auto-depth | Codex scores the diff → QUICK / STANDARD / DEEP / SWARM |
> | User override | "deep review" forces DEEP, "quick review" forces QUICK, "swarm review" forces SWARM |
> | Local agent | Background **Codex subagent** for STANDARD + DEEP — local investigation (shell, git, tests) |
> | DEEP investigation | Main Codex investigates first (hypotheses), then subagent + models run in parallel |
> | SWARM agents | 3 independent Codex Agent Team agents (security, correctness, architecture), each investigating + dispatching |
> | Degradation | SWARM → DEEP if Codex Agent Teams unavailable (always notified, never silent) |
> | Transparency | Always show depth reason: "Selected STANDARD (score 3): +2 auth, +1 size" |
> | Symmetry note | Where the Claude version uses `Task` + `clink(codex)`, this uses **Codex subagent** + `clink(claude)`. Recursion guard refuses `clink(codex)` when Codex is the host. |
>
> **Entry criteria:**
> - User wants code reviewed (any depth)
> - Squall MCP server is configured with `SQUALL_CLIENT=codex`

This skill is the Codex-CLI mirror of `squall-unified-review`. The control flow is identical;
only the **agent-spawning primitive** and the **clink target** differ. The synthesis logic,
depth scoring, ensemble math, and falsification framing are unchanged.

## Phase 0: Detect Depth

### User Override

| User says | Force |
|-----------|-------|
| "swarm review", "team review", "full investigation" | SWARM |
| "deep review", "thorough review", "investigate and review" | DEEP |
| "quick review", "quick check", "triage" | QUICK |
| "review" (no qualifier) | Auto-detect via score |

### Auto-Depth Scoring

```
SCORE = 0

# 1. Diff size (non-test lines only)
lines_changed > 500  → +3
lines_changed > 200  → +2
lines_changed > 50   → +1

# 2. File criticality (any changed file path matches)
auth/crypto/session/permission/middleware/token/jwt → +2
config files (*.toml, *.yml, schema)               → +1

# 3. Memory patterns (call `memory` category "patterns" in Phase 0)
#    If any memory patterns match changed files → +2

# 4. Complexity markers (in diff text, excluding test files)
unsafe / Arc<Mutex / RwLock / select! / spawn → +1

HARD FLOOR: If any changed file matches security keywords, minimum depth is STANDARD.

DECISION:
  SCORE >= 6  →  SWARM (if Codex Agent Teams available; else DEEP with notification)
  SCORE >= 4  →  DEEP
  SCORE >= 2  →  STANDARD
  SCORE <  2  →  QUICK
```

### SWARM Availability Check

SWARM requires Codex Agent Teams (`codex team init` style coordination). If teams aren't
available in the current Codex install:

- **Always notify**: "SWARM requested (score N) but Codex Agent Teams unavailable — falling back to DEEP"
- Never silently degrade.
- Proceed with DEEP workflow.

### Transparency

Always display the depth decision and why:

```
Selected STANDARD review (score 3): +2 auth files, +1 diff size (82 lines)
```

## Phase 1: Launch

### QUICK
Skip directly to Phase 2 (Dispatch). No investigation, no Codex subagent.

### STANDARD
1. Generate `REVIEW_ID` (timestamp-based, e.g. `20260507-093000`)
2. **Spawn a Codex subagent in the background** for local investigation:
   - Codex doesn't expose a `Task()` tool — use the natural-language spawn pattern.
   - Tell Codex: "Spawn a background subagent named 'opus-mirror' to perform local
     investigation using the prompt template at the bottom of this skill. The subagent
     must write its findings to `.squall/reviews/codex-subagent-{REVIEW_ID}.md` and
     stop when done. Do not block on it — proceed to dispatch."
3. Proceed immediately to Phase 2 — subagent runs concurrently with Squall dispatch.

### DEEP
Sequential investigation by main Codex, then subagent + models in parallel:

1. Check memory patterns (already fetched in Phase 0).
2. Read target files.
3. Map control flow, trace callers/callees with grep/glob.
4. Check git history (`git log`, `git blame`) on changed files.
5. Form hypotheses — specific, testable claims with file:line references.
6. Write investigation notes — these become `investigation_context` (persist-only, models don't see this).
7. **Spawn the Codex subagent** (same as STANDARD, but pass the hypotheses in the prompt).
8. Proceed to Phase 2.

Investigation MUST complete before dispatch. The subagent gets hypotheses as additional
context so it can independently validate or challenge them.

### SWARM
Team-based parallel investigation. 3 independent Codex Agent Team members, each with a
different lens, each doing local investigation AND dispatching its own Squall review.
**No separate Codex subagent** — the 3 team members replace it.

**The team lead orchestrates only — it does NOT investigate, dispatch, or review code itself.**

1. Generate `REVIEW_ID`.
2. Initialize a Codex Agent Team (`codex team init` or equivalent) named `squall-review-{REVIEW_ID}`.
3. Add 3 tasks: "Security investigation", "Correctness investigation", "Architecture investigation".
4. Spawn 3 team members in parallel, each with a lens-specific prompt (see SWARM templates below).
   Use `bypassPermissions` / autonomous mode — agents must not stall on prompts.
5. **Skip to Phase 3 (Gather)** — agents handle their own dispatch.

**SWARM agent model assignments (Codex variant):**

| Agent | Models | Rationale |
|-------|--------|-----------|
| Security | kimi-k2.7-code, claude, grok | kimi: adversarial edge cases. claude: precision anchor (mirror of codex in Claude version). grok: fast cross-function. |
| Correctness | claude, deepseek-v4-pro, gemini | claude: precision. gemini: systems-level bugs. deepseek: fast broad coverage. |
| Architecture | gemini, claude, qwen-3.7-max | gemini: architectural bugs. claude: contract violations. qwen: pattern matching. |

Note `claude` replaces `codex` in every ensemble — recursion guard refuses `clink(codex)`
when Codex is the host. `claude` plays the symmetric role.

## Phase 2: Dispatch

**For SWARM**: Skip — each team member dispatches its own. Proceed to Phase 3.

1. **Consult memory** — call `memory` with category "recommend", then "tactics".
2. **Call `listmodels`** for current names and metadata.
3. **Select ensemble using tiered model system:**

   ### Tier 1: ALWAYS included

   | Source | Why | Backend |
   |--------|-----|---------|
   | **gemini-flash** / **gemini-pro** | Antigravity CLI. Pick by depth (see below). Free (CLI). | `review` |
   | **claude** | High signal-to-noise, finds correctness bugs. Mirror of `codex` in the Claude variant. | `review` (uses clink internally) |
   | **grok** | Best speed/quality ratio, fast, finds cross-function bugs. | `review` (HTTP) |
   | **Codex subagent** | Local investigation — shell, git, tests, cross-file tracing. | Background subagent |

   For **QUICK**: just grok. Skip Codex subagent.

   **Gemini tier is depth-driven** — the Antigravity CLI exposes Flash and Pro as
   separate Squall models, so pick the one matching the review's depth:

   | Depth | Use | Why |
   |-------|-----|-----|
   | QUICK | `gemini-flash` (if including gemini at all) | fast, cheap breadth |
   | STANDARD | `gemini-flash` | good signal per second |
   | DEEP / SWARM | `gemini-pro` | architectural and systems-level depth |

   Plain `gemini` inherits whatever is set in `~/.gemini/antigravity-cli/settings.json`.
   Use it when the operator's own choice should win; prefer the explicit `-flash`/`-pro`
   keys when the depth should decide. Do NOT pass `reasoning_effort` to these — `agy`
   encodes effort in the model label, and its `--effort` flag silently downgrades the
   model to Flash.
   For **STANDARD**: All 4 Tier 1 sources (subagent runs in parallel).
   For **DEEP**: All 4 Tier 1 sources (subagent spawned after investigation).

   ### Tier 2: Pick 2 based on situation

   | Model | Best For | Notes |
   |-------|----------|-------|
   | **kimi-k2.7-code** | Security, edge cases, adversarial scenarios | Code-specialized contrarian. Pair with focused lens. |
   | **deepseek-v4-pro** | Fast triage, broad coverage | 512K ctx, high precision. |
   | **qwen-3.7-max** | Pattern matching, performance analysis | 1M ctx, agentic. |
   | **glm-5.2** | Architectural framing | GLM-5.2 via Together, 262K ctx. |

   **Selection criteria** (same as Claude variant): memory recommend + memory tactic +
   model strengths matching the diff. Models with <70% success rate (>=5 samples) are
   server-gated. Include 1+ cold-start model for exploration.

   **Always show selection reasoning:**
   ```
   Tier 1: gemini, claude, grok, Codex subagent (always)
   Tier 2: kimi-k2.7-code (security lens), deepseek-v4-pro (fast triage)
   Rationale: Rust concurrency code — Kimi for edge cases, DV4-Pro for broad coverage
   ```

4. **Build per-model lenses** using memory tactics. Defaults:

   | Strength Area | Default Lens |
   |---------------|-------------|
   | Systems/concurrency | "You are a concurrency skeptic. Attempt to PROVE race conditions, resource leaks, or deadlocks exist. If you cannot find evidence, explain why the synchronization is correct. Report confidence (high/medium/low) for each finding." |
   | Correctness/logic | "You are a correctness enforcer. Attempt to PROVE logic errors, edge-case failures, or off-by-one bugs exist. Trace each path step by step. If the logic is sound, explain why. Report confidence for each finding." |
   | Surface/defects | "You are a defect hunter. Attempt to PROVE null dereferences, missing validation, or common pitfalls exist. If the code handles these correctly, explain the safeguards. Report confidence for each finding." |
   | Adversarial/security | "You are an adversarial tester. Attempt to PROVE the code fails under unusual inputs, injection attacks, or trust boundary violations. If it's robust, explain the defenses. Report confidence for each finding." |

   Lenses are expertise-based with falsification framing. Investigation hypotheses go in
   `investigation_context`, not in `per_model_system_prompts`.

5. **Call `review`**:
   - QUICK: just `prompt`, `models: ["grok"]`, `diff`, `temperature: 0`
   - STANDARD: add `file_paths`, `working_directory`, `per_model_system_prompts`
   - DEEP: add `deep: true`, `investigation_context`

## Phase 3: Gather

### SWARM
**Wait for all team members.** Each may take 15+ minutes. Read their output files at
`.squall/reviews/swarm-{REVIEW_ID}-{LENS_SLUG}.md`. If any team member fails or stalls,
proceed with available output and flag the missing lens. Tear down the team when done.

### QUICK / STANDARD / DEEP
1. Read the `results_file` from the review response.
2. **STANDARD/DEEP**: Check the Codex subagent's output file at
   `.squall/reviews/codex-subagent-{REVIEW_ID}.md`. If missing or the subagent failed,
   proceed without it (graceful degradation; note in synthesis).
3. **Quality gates**: check `warnings`, `not_started`, `summary.models_gated`.

## Phase 4: Synthesize

### QUICK
Relay grok's findings directly.

### STANDARD (models + Codex subagent)

| Signal | Confidence |
|--------|-----------|
| Models agree + subagent confirms | HIGHEST |
| Models agree, subagent silent | HIGH |
| Subagent unique finding | MEDIUM-HIGH (local context credibility) |
| Single model + subagent confirms | MEDIUM-HIGH |
| Subagent + model contradict | FLAG for human |

### DEEP (models + subagent + investigation hypotheses)

| Signal | Confidence |
|--------|-----------|
| Hypothesis confirmed by models + subagent | HIGHEST |
| Hypothesis confirmed by models, subagent silent | HIGH |
| Hypothesis confirmed by subagent only | MEDIUM-HIGH |
| Hypothesis not flagged anywhere | Likely false alarm — drop or mark speculative |
| Subagent unique finding outside hypotheses | MEDIUM-HIGH |
| Model finding outside hypotheses | MEDIUM |
| Subagent + model contradict | FLAG for human |

### SWARM (3 agents × 3 models each)

| Signal | Confidence |
|--------|-----------|
| Found by 2+ agents' local investigation | CRITICAL |
| 1 agent local + models in another agent confirm | HIGH |
| Same model flags it through different lenses | HIGH |
| 1 agent local only | MEDIUM-HIGH |
| Multiple models within 1 agent | MEDIUM |
| Single model only | MEDIUM |
| Agents contradict | FLAG for human |

### Output Format (QUICK / STANDARD / DEEP)

```
### Depth: STANDARD (score 3): +2 auth files, +1 diff size

### Consensus Findings (2+ sources agree)
- [critical] Description (models: gemini, claude; subagent: confirmed)
- [high] Description (models: gemini, grok)

### Unique Catches
- [medium] Description (source: codex-subagent) — local investigation found caller mismatch
- [low] Description (model: claude) — edge case in error path

### Possible False Positives
- [medium] Description (model: grok) — matches known Grok blind spot

### Quality Notes
- 3/3 models succeeded, Codex subagent: completed
- No warnings
```

## Phase 5: Memorize + Report

After synthesis, close the learning loop:

1. **`memorize` category "pattern"** — confirmed bugs / recurring issues (model attribution).
2. **`memorize` category "tactic"** — lens effectiveness observations.
3. **`memorize` category "recommend"** — model performance notes.

## Codex Subagent Prompt Template

Used for STANDARD and DEEP. Caller fills `{placeholders}`. For DEEP, include a hypotheses
section. The subagent should be spawned with file-system + shell access but no Squall MCP
access (it's the local-investigation specialist; external models handle the AI review).

```
You are an expert code reviewer doing LOCAL investigation. External AI models are
reviewing this code in parallel via Squall — your job is the perspective they CANNOT provide.

You have shell access. They don't. Use it:
1. Read changed files + trace their callers/callees across the codebase.
2. Check test coverage: are the changed code paths tested? Run `cargo test` (or framework
   equivalent) on changed modules if useful.
3. Run `git log`/`blame` on changed files for context (why does this code exist?).
4. Grep for related patterns — is this change consistent with the rest of the codebase?
5. Look for integration issues: does this change break any callers?

CONSTRAINTS:
- Do NOT use Squall MCP tools (no review/chat/clink/memory).
- Do NOT modify source code. Read-only investigation. Build/test commands are fine.
- Scope tests to changed modules — you have limited time.
- Always use non-interactive flags (e.g. `git --no-pager log`).
- Focus on what static text analysis misses: cross-file interactions, test coverage gaps,
  git history context, inconsistencies.

Changed files: {file_list}
Diff summary: {diff_stat}
Working directory: {working_directory}

{hypotheses_section}

Write findings to: .squall/reviews/codex-subagent-{REVIEW_ID}.md

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

Used for SWARM. One prompt per lens. Each agent loads Squall tools (via Codex's MCP wiring),
investigates locally, dispatches its own Squall review.

```
You are the {LENS} investigator in a Codex Agent Team review swarm.
Your shore is {LENS} — own it completely.

You operate autonomously. No human gates your work.
Investigate deep, dispatch to models, write your findings, report back.

Two jobs, one mission:
1. LOCAL INVESTIGATION — shell, git, tests. Find what static analysis cannot.
2. SQUALL DISPATCH — 3 external models amplify your {LENS} lens.

## Step 1: Claim your task
- Squall MCP tools should already be available (declared in agents/openai.yaml).
- Find the team task with "{LENS}" in its title and mark it in_progress.

## Step 2: Investigate locally
- Read changed files: {file_list}
- {INVESTIGATION_STEPS}
- Form hypotheses with file:line references.
- Investigation notes inform which areas to probe; system prompts use falsification framing.
- Do NOT inject specific hypotheses into per_model_system_prompts.

## Step 3: Dispatch to models
- Call `listmodels` to verify model names.
- Call `memory` category "tactics" for proven system prompts.
- Call `review` with:
  - models: {MODELS}
  - per_model_system_prompts: built from your investigation + {LENS} focus
  - diff, file_paths, working_directory
  - deep: true
  (May take 10+ minutes. Squall ceiling is 600s. Be patient.)

## Step 4: Synthesize & write output
- Read the results_file from the review response.
- Combine your local findings with model findings.
- Write to: .squall/reviews/swarm-{REVIEW_ID}-{LENS_SLUG}.md

OUTPUT FORMAT:
## {LENS} Investigation: {REVIEW_ID}

### Local Findings
#### [severity] Finding title
- **File**: path/to/file.rs:line
- **Evidence**: what you found locally
- **Risk**: why it matters from a {LENS} perspective

### Model Findings
#### [severity] Finding title
- **Models agreeing**: [list]
- **Detail**: what models found

### {LENS} Assessment
Overall: safe / concerning / critical

## Step 5: Report and complete
- Mark your team task completed.
- Send a 2-3 sentence summary to the team lead.

CONSTRAINTS:
- Read-only investigation + Squall calls. Do NOT modify source code.
- Write ONLY to: .squall/reviews/swarm-{REVIEW_ID}-{LENS_SLUG}.md
- Do NOT read other agents' output files. Investigation must be independent.
- Always use non-interactive git flags.
- If running tests, scope to changed modules.
```

### Lens-Specific Investigation Steps

**Security** (`{MODELS}: kimi-k2.7-code, claude, grok`):
- Trace all trust boundary crossings.
- Check input validation and sanitization paths.
- `git blame` on security-critical lines.
- Grep for `unsafe`, `unwrap`, hardcoded secrets, TODO security comments.
- Check if security-relevant tests exist for changed paths.

**Correctness** (`{MODELS}: claude, deepseek-v4-pro, gemini`):
- Trace control flow through changed functions; map all branches.
- Run targeted tests on changed modules.
- Check error handling completeness.
- Verify changed public APIs still satisfy existing callers (grep for callers).

**Architecture** (`{MODELS}: gemini, claude, qwen-3.7-max`):
- Map dependency graph of changed modules.
- Check for API contract changes by diffing public function signatures.
- Grep for performance anti-patterns.
- `git log --oneline -20` on changed files for refactoring context.

## Ensemble Selection Reference

| Intent | Depth | Tier 1 | Tier 2 (pick 2) | Subagent? | Investigation? |
|--------|-------|--------|-----------------|-----------|----------------|
| Small non-critical change | QUICK | grok only | none | No | No |
| Normal PR, routine code | STANDARD | gemini + claude + grok | 2 by code type + memory data | Yes (parallel) | No |
| Security, critical infra | DEEP | gemini + claude + grok | 2 by code type + memory data | Yes (parallel, gets hypotheses) | Yes (sequential, before dispatch) |
| Large + security + memory patterns | SWARM | 3 agents × 3 models each | Per-agent (see tables) | No (agents replace it) | Yes (per agent, parallel) |

## Anti-Patterns

| Don't | Do Instead |
|-------|------------|
| Call `clink(model: "codex")` from inside Codex | Recursion guard refuses it. Use `clink(model: "claude")` for the symmetric local-Anthropic agent. |
| Hardcode model names | Read memory recommend + listmodels, reason about model fit |
| Skip depth detection | Let auto-depth match ensemble to intent |
| Use QUICK for security-sensitive files | Hard floor ensures minimum STANDARD |
| Skip `listmodels` before review | Model availability changes — always check |
| Omit `per_model_system_prompts` | Lenses are a huge quality improvement |
| Spawn Codex subagent for QUICK | Overhead isn't worth it for single-model triage |
| Skip Codex subagent for DEEP | DEEP needs MORE sources, not fewer |
| Assume the subagent will succeed | Graceful degradation — synthesize without it if needed |
| Start DEEP dispatch before investigation | Investigation MUST complete first |
| Skip memory before dispatch | `memory` recommend + tactic + patterns first |
| Skip memorize after synthesis | Close the learning loop |
| Use conclusion-driven lenses | Falsification framing instead |
| Inject hypotheses into per_model_system_prompts | Keep them in `investigation_context` |
| Spawn SWARM for score < 6 | Reserve for high-stakes |
| Silently degrade SWARM → DEEP | Always notify |
| Have team lead investigate or dispatch | Team lead orchestrates ONLY |

## Notes

- Explicit depth keywords ("deep review", "quick review", "swarm review") override auto-detection.
- The recursion guard makes `clink(codex)` always fail in this mode — use `claude` instead.
- This skill is the Codex mirror of `squall-unified-review` — keep them in sync when fixes land.

## Related Skills

- [squall-deep-research](../squall-deep-research/SKILL.md) — Deep research via web search
