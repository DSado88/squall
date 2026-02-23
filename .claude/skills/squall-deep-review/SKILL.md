---
name: squall-deep-review
description: "Superseded by squall-unified-review. Preserved for reference and backward compatibility. (project)"
one_liner: "Structure beats parallelism — investigate first, then dispatch."
related_skills:
  - "[squall-unified-review](../squall-unified-review/SKILL.md) (use this instead)"
  - "[squall-research](../squall-research/SKILL.md) (research swarms, not code review)"
---

# Squall Deep Review

> **Note:** This skill is superseded by [squall-unified-review](../squall-unified-review/SKILL.md), which auto-detects when deep review is needed and includes the full investigation workflow. Use the unified skill for new reviews. This file is preserved for backward compatibility and reference.

> **TL;DR - QUICK REFERENCE**
>
> | Concept | Translation |
> |---------|-------------|
> | Phase 1: Investigate | Read code, map control flow, form hypotheses — NO model calls |
> | Phase 2: Dispatch | Call `review` with `deep: true` + `investigation_context` + tailored lenses |
> | Phase 3: Synthesize | Read results file, group by agreement, cross-reference hypotheses |
> | `investigation_context` | Persist-only — models get context via `per_model_system_prompts` |
> | `warnings` + `summary` | Check these in the response for quality issues |
>
> **Entry criteria:**
> - Critical code changes that warrant structured investigation
> - Security-sensitive code (auth, crypto, permissions)
> - Complex systems code (concurrency, state machines, error handling)
> - User explicitly requests deep/thorough/structured review

Deep code review using Squall's `review` tool with a mandatory investigation phase. The key insight: **structure beats parallelism** — a structured investigation with focused models finds bugs that blind parallel fan-out misses.

## Workflow

```
Phase 1: INVESTIGATE          Phase 2: DISPATCH           Phase 3: SYNTHESIZE
┌──────────────────┐         ┌──────────────────┐        ┌──────────────────┐
│ Check memory for │         │ Consult memory    │        │ Read results_file│
│   known patterns │         │   for recs+tactics│        │ Group by         │
│ Read target files│────────>│ Call listmodels   │───────>│   agreement      │
│ Map control flow │         │ Build lenses from │        │ Cross-reference  │
│ Form hypotheses  │         │   hypotheses +    │        │   hypotheses     │
│ Write notes      │         │   memory tactics  │        │ Report quality   │
│                  │         │ Call review with   │        │ Memorize findings│
│ NO model calls   │         │   deep: true      │        │                  │
└──────────────────┘         └──────────────────┘        └──────────────────┘
```

## Phase 1: Investigate

**This phase is mandatory.** Do NOT skip to dispatch.

1. **Check memory for known patterns** — call `memory` category "patterns" to see if this code area has known issues from prior reviews. This informs your hypotheses.
2. **Read the target files** — use the Read tool, not model calls
3. **Map the control flow** — trace the critical paths, identify state transitions
4. **Identify complexity hotspots** — nested conditions, error handling gaps, concurrency patterns
5. **Form hypotheses** — "I suspect X because Y" — specific, testable claims
6. **Write investigation notes** — these become `investigation_context` and inform lenses

### What Good Investigation Notes Look Like

```
INVESTIGATION NOTES:
- src/auth.rs:45-80: Token validation path has 3 branches. The refresh branch (line 67)
  doesn't check expiry before refresh — potential window for expired token reuse.
- src/db.rs:120-145: Connection pool acquire uses timeout but doesn't handle the
  poisoned-connection case from line 130.
- HYPOTHESIS: Race condition between token refresh and concurrent request validation.
  The shared token state (Arc<RwLock>) could allow a request to validate against a
  stale token if refresh happens between read and use.
```

### What Bad Investigation Notes Look Like

```
BAD: "The code looks complex and might have bugs."
BAD: "I'll review the auth module."
BAD: (skipping investigation entirely and jumping to dispatch)
```

## Phase 2: Dispatch

1. **Consult memory** — call `memory` category "recommend" for data-driven model recommendations, and "tactics" for proven per-model lenses.
2. **Call `listmodels`** to get current model names
3. **Select ensemble** — use memory recommendations and listmodels output. For deep review, use 3-5 models.
4. **Build per-model lenses from hypotheses** — each model gets a system prompt informed by your investigation:

```
# Example: lenses tailored to investigation findings
per_model_system_prompts: {
  "gemini": "Focus on the token refresh race condition in src/auth.rs:67.
             Check if Arc<RwLock> usage allows stale reads during refresh.
             Also check connection pool poisoning in src/db.rs:130.",
  "codex": "Trace the exact code path from token validation to refresh.
            Verify mutual exclusion between concurrent validate+refresh calls.
            Check off-by-one in expiry comparison at auth.rs:52.",
  "grok": "Check for missing error handling in the 3 branches at auth.rs:45-80.
           Look for unwrap() calls on fallible operations. Check timeout handling."
}
```

5. **Refine lenses with memory** — call `memory` category "tactics" and incorporate proven prompts. Merge your hypothesis-informed lenses with field-tested tactics from memory.
6. **Call `review`** with:
   - `deep: true` (sets timeout=600s, reasoning_effort=high, max_tokens=16384)
   - `investigation_context`: your notes from Phase 1 (persist-only, NOT sent to models)
   - `per_model_system_prompts`: lenses tailored to your hypotheses
   - `file_paths` + `working_directory`

7. **Check quality gates** in the response:
   - `warnings` — any unknown model keys? Truncation?
   - `summary.models_succeeded` — how many models actually returned results?
   - If `models_succeeded == 0`, investigation is wasted — diagnose before retrying

## Phase 3: Synthesize

1. **Read the `results_file`** from the review response
2. **Group findings by agreement:**
   - **Consensus findings** (2+ models) — high confidence, report first
   - **Unique catches** (1 model) — lower confidence but high coverage value
   - **Contradictions** — flag for human judgment
3. **Cross-reference against investigation hypotheses:**
   - Hypotheses confirmed by models — highest confidence
   - Hypotheses NOT flagged by any model — either false alarm or models missed it, investigate further
   - Model findings outside hypotheses — unexpected, worth attention
4. **Check quality issues from warnings/summary:**
   - Partial results — models were cut off, findings may be incomplete
   - Failed models — missing coverage in that model's specialty area
5. **If `models_succeeded == 0`** — report total failure, don't attempt empty synthesis
6. **Memorize findings** — after synthesis:
   - `memorize` category "pattern" for any confirmed bugs or recurring issues (include model attribution)
   - `memorize` category "tactic" for lens effectiveness observations (which lens + model combo worked best)
   - `memorize` category "recommend" for model performance notes (which models found real bugs vs false positives)

### Output Format

```
### Investigation Hypotheses
- [confirmed] Race condition in token refresh (models: gemini, codex)
- [unconfirmed] Connection pool poisoning — no model flagged this, may need manual verification

### Consensus Findings (2+ models agree)
- [critical] Token refresh window allows expired token reuse (models: gemini, codex)

### Unique Catches (single model)
- [medium] Missing error handling in auth branch 3 (model: grok)

### Quality Notes
- 4/5 models succeeded, 1 partial (kimi hit straggler cutoff)
- No warnings in response
```

## When to Use Deep Review vs Standard Review

| Signal | Use Deep Review | Use Standard Review |
|--------|----------------|-------------------|
| Security-sensitive code | Yes | No |
| Complex concurrency | Yes | No |
| Critical infrastructure | Yes | No |
| Small diff, style check | No | Yes |
| Quick triage | No | Yes |
| User says "deep" or "thorough" | Yes | No |

## Anti-Patterns

| Don't | Do Instead |
|-------|------------|
| Skip Phase 1 and jump straight to dispatch | Investigation is mandatory — it's why this skill exists |
| Write vague investigation notes | Be specific: file paths, line numbers, concrete hypotheses |
| Use generic lenses instead of hypothesis-informed ones | Tailor each model's system prompt to your specific findings |
| Ignore warnings and summary in the response | Check quality gates — they exist to catch silent failures |
| Attempt synthesis when models_succeeded == 0 | Diagnose the failure first (check warnings, API keys, model names) |
| Send investigation_context expecting models to see it | It's persist-only — use per_model_system_prompts for model context |
| Use this for every review | Reserve for critical code — standard review is faster for routine work |
| Skip memory check during investigation phase | Call `memory` category "patterns" first — known issues inform better hypotheses |
| Use only hardcoded lenses in dispatch | Merge hypothesis lenses with `memory` category "tactics" for proven prompts |
| Finish synthesis without memorizing | Call `memorize` for patterns, tactics, and recommendations after every deep review |

## Related Skills

- [squall-review](../squall-review/SKILL.md) — Standard multi-model review (no investigation phase)
- [squall-research](../squall-research/SKILL.md) — Multi-agent research swarms (not code review)
- [squall-deep-research](../squall-deep-research/SKILL.md) — Deep research via Codex/Gemini web search

<!-- SENTINEL:SESSION_LEARNINGS_START - Do not remove this line -->
## Session Learnings Archive

Insights captured during skill execution:

(Add learnings here as they occur, using this format:)

### YYYY-MM-DD: Brief Title
**Origin**: What triggered this learning
**Core insight**: The key learning
**Harvested** -> [Link to where it was integrated] OR "Pending harvest"

---
<!-- SENTINEL:SESSION_LEARNINGS_END -->
