---
name: squall-review
description: "Multi-model code review via Squall. Use when asked to 'review code', 'squall review', 'code review', 'check this code', or 'review this diff'. Dispatches to AI models with per-model expertise lenses. (project)"
one_liner: "The right models for the right code — Squall picks the ensemble, skills encode the wisdom."
activation_triggers:
  - "squall review"
  - "review code"
  - "code review"
  - "check this code"
  - "review this diff"
  - "squall-review"
  - When user wants multi-model code review
  - When user wants AI review of a diff or PR
related_skills:
  - "[squall-research](../squall-research/SKILL.md) (for research, not code review)"
  - "[squall-deep-research](../squall-deep-research/SKILL.md) (for deep research questions)"
---

# Squall Review

> **TL;DR - QUICK REFERENCE**
>
> | Concept | Translation |
> |---------|-------------|
> | `review` tool | Parallel fan-out to multiple models with straggler cutoff |
> | `per_model_system_prompts` | Different expertise lenses per model (security, architecture, etc.) |
> | `listmodels` first | Model names change — always fetch current names before calling review |
> | Results persist | `.squall/reviews/` files survive context compaction |
> | Caller synthesizes | Don't ask models to merge — read all responses yourself and synthesize |
>
> **Entry criteria:**
> - User wants code reviewed by multiple AI models
> - A diff, file set, or code snippet needs expert analysis
> - Security, architecture, or correctness review requested

Multi-model code review using Squall's `review` tool. Each model gets a tailored system prompt (lens) matched to its proven strengths. Results persist to disk so they survive context compaction.

## Workflow

1. **Consult memory** — call `memory` with category "recommend" for model recommendations, then "tactics" for proven per-model lenses, then "patterns" for known issues in the codebase
2. **Call `listmodels`** via Squall MCP to get exact current model names
3. **Select the ensemble** based on review intent (see Model Selection below)
4. **Build per-model system prompts** with expertise lenses (see Lenses below)
5. **Prepare the prompt** — include the diff/code and review instructions
6. **Call `review`** with models, prompt, per_model_system_prompts, file_paths, working_directory
7. **Read the results file** from `.squall/reviews/` and synthesize findings (see Synthesis Phase below)

```
┌────────────┐    ┌────────────┐    ┌────────────┐    ┌────────────┐    ┌────────────┐
│  Consult   │───>│ listmodels │───>│   Select   │───>│   Build    │───>│   Call     │
│  memory    │    │            │    │  ensemble  │    │  lenses +  │    │  review    │
│            │    │            │    │            │    │  prompt    │    │            │
└────────────┘    └────────────┘    └────────────┘    └────────────┘    └─────┬──────┘
                                                                              │
                                                                              v
                                                      ┌────────────┐  ┌────────────┐
                                                      │ Memorize   │<─│ Read result│
                                                      │ learnings  │  │ file and   │
                                                      │            │  │ synthesize │
                                                      └────────────┘  └────────────┘
```

## Synthesis Phase

After `review` returns, read the persisted results file and synthesize findings. This is the most important step — raw model outputs have overlap, contradictions, and varying quality. Your job is to extract signal.

### How to Synthesize

1. **Read the results file** from the `results_file` path in the review response
2. **Group findings by agreement:**
   - **Consensus findings** — flagged by 2+ models → high confidence, report these first
   - **Unique catches** — flagged by only 1 model → lower confidence but high coverage value (this is WHY we use multiple models)
   - **Contradictions** — models disagree → flag for human judgment
3. **Rank by severity:** critical > high > medium > low > info
4. **Filter by model reliability** (use the scorecard):
   - Codex findings at high confidence → almost certainly real (0 FP track record)
   - Gemini systems-level findings → likely real
   - Grok findings → check for known blind spots (XML escaping, edition 2024)
   - GLM findings → treat as architectural advice, not bug reports
   - Kimi findings → verify edge cases, may be contrarian but sometimes correct
5. **Output format:**

```
### Consensus Findings (2+ models agree)
- [critical] Description (models: gemini, codex, grok)
- [high] Description (models: gemini, kimi)

### Unique Catches (single model)
- [medium] Description (model: codex) — high confidence given Codex track record
- [low] Description (model: kimi) — edge case worth considering

### Possible False Positives
- [medium] Description (model: grok) — matches known Grok blind spot for X
```

### When to Skip Synthesis

For **quick triage** (single model, Grok only), synthesis is unnecessary — just relay the findings directly. Synthesis adds value when 2+ models are involved.

### After Synthesis: Memorize Learnings

After completing synthesis, save reusable insights to Squall's memory:

1. **Patterns**: If a finding recurs across reviews or matches a known pattern, call `memorize` with category "pattern"
2. **Tactics**: If a particular lens/prompt produced notably good or bad results, call `memorize` with category "tactic" with the model name
3. **Recommendations**: If a model consistently excels or fails at a task type, call `memorize` with category "recommend"

This closes the learning loop — future reviews benefit from today's findings.

## Model Selection

Choose the ensemble based on what kind of review is needed:

| Intent | Models | Typical Time | When to Use |
|--------|--------|-------------|-------------|
| **Quick triage** | `grok-4-1-fast-reasoning` | ~20s | Small diff, style check, obvious bugs |
| **Standard review** | `grok-4-1-fast-reasoning` + `gemini` | ~60s | Normal code review, feature PRs |
| **Thorough review** | `grok-4-1-fast-reasoning` + `moonshotai/kimi-k2.5` + `z-ai/glm-5` | ~120s | Large changes, unfamiliar code |
| **Security review** | `gemini` + `codex` + `grok-4-1-fast-reasoning` | ~120s | Auth, crypto, input handling, permissions |
| **Full consensus** | All 5 models | ~180s | High-stakes changes, critical infrastructure |

**Always call `listmodels` first** — model names above are examples and may have changed.

**Check memory before hardcoding lenses** — call `memory` category "tactics" for proven system prompts. The Per-Model Lenses table below is a starting point; tactics.md has field-tested refinements that supersede these defaults.

## Per-Model Lenses

Each model has a proven strength. Use `per_model_system_prompts` to assign each model its expertise lens:

| Model | Lens (system prompt) | Why |
|-------|---------------------|-----|
| **Gemini** | "Focus on systems-level bugs, concurrency issues, resource leaks, and memory safety" | Best at finding real systems bugs across 4 review rounds |
| **Codex** | "Focus on logic errors, edge cases, off-by-one bugs, and step-by-step correctness reasoning" | Highest precision — 0 false positives across all rounds, exact line refs |
| **Grok** | "Focus on obvious bugs, null dereferences, missing validation, and common pitfalls" | Fastest model, good first-pass filter |
| **GLM** | "Focus on architectural patterns, coupling, API design, and separation of concerns" | Strong architectural framing, design-level feedback |
| **Kimi** | "Focus on edge cases, unusual inputs, race conditions, and adversarial scenarios" | Contrarian perspective, finds what others miss |

For security reviews, override lenses:
- Gemini: "Focus on memory safety, use-after-free, buffer overflows, and unsafe code blocks"
- Codex: "Focus on authentication bypass, injection vulnerabilities, and privilege escalation"
- Grok: "Focus on input validation gaps, missing sanitization, and OWASP Top 10"

## Model Scorecard

| Model | Speed | Precision | Best For | Avoid For |
|-------|-------|-----------|----------|-----------|
| Gemini | 55-184s | High | Systems bugs, concurrency | Quick triage |
| Codex | 50-300s | Highest (0 FP) | High-stakes precision | Speed-sensitive |
| Grok | 20-65s | Medium (4+ FP/round) | Fast triage, obvious bugs | Security review (alone) |
| GLM | 75-93s | Low | Architecture framing | Bug hunting |
| Kimi | 60-300s | Medium | Contrarian edge cases | Time-sensitive |

## Key Principles

### Principle 1: Lenses Over Generics

Without `per_model_system_prompts`, all models get the same generic prompt and return overlapping, unfocused reviews. With lenses, each model focuses on what it's best at. The quality difference is dramatic — Grok's false positive rate dropped significantly when given a focused system prompt.

### Principle 2: The Caller Synthesizes

Squall concatenates responses with attribution — it does NOT merge or deduplicate. You (the caller) read all model responses and synthesize findings. This is by design: you're an LLM, you're good at this.

### Principle 4: Per-Model Prompts Override Shared

When both `system_prompt` and `per_model_system_prompts` are set, models in the per-model map get **only** their per-model prompt (it overrides, not concatenates). Models NOT in the map fall back to the shared `system_prompt`. If neither is set, the model gets no system prompt.

### Principle 3: Results Survive Compaction

Review results persist to `.squall/reviews/`. When context compaction destroys large tool results, the file path survives in the compaction summary. Read the file to recover full details.

## Review Tool Parameters

```
review({
  prompt: "Review this code for...",           // Required: what to review
  models: ["grok-4-1-fast-reasoning", "gemini"], // Optional: defaults to all
  timeout_secs: 180,                           // Optional: straggler cutoff (default 180)
  system_prompt: "You are an expert reviewer", // Optional: shared baseline
  temperature: 0,                              // Optional: 0 = deterministic
  file_paths: ["src/server.rs", "src/lib.rs"], // Optional: files to include as context
  working_directory: "/path/to/project",       // Required when file_paths is set
  diff: "--- a/file\n+++ b/file\n...",        // Optional: unified diff text
  per_model_system_prompts: {                  // Optional: per-model lens overrides
    "gemini": "Focus on systems-level bugs...",
    "grok-4-1-fast-reasoning": "Focus on obvious bugs..."
  }
})
```

## Anti-Patterns

| Don't | Do Instead |
|-------|------------|
| Use all 5 models for every review | Match ensemble to intent (quick triage = Grok only) |
| Use Grok alone for security-sensitive code | Always include Gemini + Codex for security |
| Skip `listmodels` before calling review | Model names change — always check first |
| Omit `per_model_system_prompts` | Lenses are a huge quality improvement — always set them |
| Make separate chat/clink calls for multi-model review | Use `review` — it does parallel fan-out with straggler cutoff |
| Try to parse/merge results yourself | Read the results file, synthesize findings in natural language |
| Set very short timeouts for thorough reviews | Codex and Kimi can take 300s — use 180s minimum for full consensus |
| Ignore the `results_file` path in the response | Always read the persisted file — it survives compaction |
| Skip checking memory before selecting models/lenses | Call `memory` category "recommend" + "tactics" + "patterns" first — proven lenses beat defaults |

## Examples

### Example 1: Quick Triage of a Small Diff

```
# 1. Get current model names
listmodels()

# 2. Quick review with just Grok
review({
  prompt: "Review this diff for bugs and style issues",
  models: ["grok-4-1-fast-reasoning"],
  diff: "<paste unified diff here>",
  system_prompt: "You are an expert code reviewer. Be concise.",
  temperature: 0
})
```

Fast feedback in ~20 seconds. Good for small PRs and style checks.

### Example 2: Security Review

```
# 1. Get current model names
listmodels()

# 2. Security-focused review with frontier models
review({
  prompt: "Security review: check for auth bypass, injection, privilege escalation, and unsafe operations",
  models: ["gemini", "codex", "grok-4-1-fast-reasoning"],
  file_paths: ["src/auth.rs", "src/api/handlers.rs"],
  working_directory: "/Users/david/project",
  timeout_secs: 300,
  temperature: 0,
  per_model_system_prompts: {
    "gemini": "Focus on memory safety, use-after-free, buffer overflows, and unsafe code blocks",
    "codex": "Focus on authentication bypass, injection vulnerabilities, and privilege escalation",
    "grok-4-1-fast-reasoning": "Focus on input validation gaps, missing sanitization, and OWASP Top 10"
  }
})
```

Three frontier models with security-specific lenses. ~120 seconds.

### Example 3: Full Consensus Review

```
# 1. Get current model names
listmodels()

# 2. All models, all lenses
review({
  prompt: "Thorough review of this critical change. Flag any bugs, security issues, or architectural concerns.",
  models: ["gemini", "codex", "grok-4-1-fast-reasoning", "moonshotai/kimi-k2.5", "z-ai/glm-5"],
  file_paths: ["src/server.rs", "src/dispatch/http.rs"],
  working_directory: "/Users/david/project",
  diff: "<unified diff>",
  timeout_secs: 180,
  temperature: 0,
  per_model_system_prompts: {
    "gemini": "Focus on systems-level bugs, concurrency issues, resource leaks, and memory safety",
    "codex": "Focus on logic errors, edge cases, off-by-one bugs, and correctness proofs",
    "grok-4-1-fast-reasoning": "Focus on obvious bugs, null dereferences, missing validation, and common pitfalls",
    "z-ai/glm-5": "Focus on architectural patterns, coupling, API design, and separation of concerns",
    "moonshotai/kimi-k2.5": "Focus on edge cases, unusual inputs, race conditions, and adversarial scenarios"
  }
})

# 3. Read the persisted results file
# The response includes a `results_file` path — read it for full details
```

Five models, five lenses, 180s straggler cutoff. For high-stakes changes only.

## Structured Output Schema

For parseable review findings, a JSON schema is available at `.squall/review-schema.json`. To use it, include the schema in the system prompt and instruct models to respond in JSON. This is optional — free-text reviews work fine for most cases. Use structured output when you need to programmatically compare findings across models.

## Related Skills

- [squall-research](../squall-research/SKILL.md) - Multi-agent research swarms (not code review)
- [squall-deep-research](../squall-deep-research/SKILL.md) - Deep research via Codex web search

<!-- SENTINEL:SESSION_LEARNINGS_START - Do not remove this line -->
## Session Learnings Archive

Insights captured during skill execution:

(Add learnings here as they occur, using this format:)

### YYYY-MM-DD: Brief Title
**Origin**: What triggered this learning
**Core insight**: The key learning
**Harvested** → [Link to where it was integrated] OR "Pending harvest"

---
<!-- SENTINEL:SESSION_LEARNINGS_END -->
