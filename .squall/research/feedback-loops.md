# Feedback Loops for AI Code Review Tools

> Research synthesis for Squall's review tool feedback mechanism.
> Generated 2026-02-21 by feedback-researcher.

## Problem Statement

Squall dispatches code review prompts to multiple AI models in parallel and persists
each model's response to `.squall/reviews/` as markdown files. Currently there is no
feedback loop -- the tool never reads back previous results to learn which findings
were real bugs vs false positives. Over 150+ review files have accumulated with no
signal about their quality.

---

## 1. Industry Survey: How Existing Tools Handle Feedback

### CodeRabbit (Best-in-Class Feedback Loop)

**Mechanism:** Learnings captured from natural-language PR chat. When a developer
disagrees with a suggestion, they reply with context (e.g., "We prefer early returns
over try-catch here"). CodeRabbit evaluates the comment and creates a Learning,
acknowledged with a collapsible "Learnings Added" section.

**Storage:** Organization-scoped vector database. Learnings include metadata: PR number,
filename, GitHub user, learning text. Exportable as CSV.

**Scope control:** `auto` (repo-specific for public, org-wide for private), `global`
(all org learnings everywhere), `local` (repo-specific only).

**Application:** Before adding any comment, CodeRabbit loads applicable learnings as
additional context/instructions. Pattern matching via vector similarity.

**Key insight:** "Don't just tell it what to do -- explain the reasoning. The 'why'
helps apply the learning in similar-but-not-identical situations."

Sources: [CodeRabbit Learnings Guide](https://docs.coderabbit.ai/guides/learnings),
[CodeRabbit Review Instructions](https://docs.coderabbit.ai/guides/review-instructions)

### GitHub Copilot Code Review

**Mechanism:** Thumbs up/down on individual comments. Optional reason and text feedback
on downvote. Confidence scores shown per suggestion (added 2025).

**Storage:** GitHub internal telemetry. No user-visible feedback history.

**Custom rules:** `.github/copilot-instructions.md` for repo-wide guidelines,
`.github/instructions/**/*.instructions.md` for path-specific rules.

**Key limitation:** Always leaves "Comment" reviews (never Approve/Request Changes).
No visible learning loop -- feedback goes to GitHub's global model training, not
per-repo adaptation.

Sources: [GitHub Copilot Code Review Docs](https://docs.github.com/copilot/using-github-copilot/code-review/using-copilot-code-review)

### SonarQube

**Mechanism:** "Resolve as false positive" or "Won't fix" statuses. Manual, per-issue.
Status persists across PR merges (Developer Edition).

**Scale:** Analyzes 750B LOC/day. Overall false positive rate: 3.2% (as of 2025) --
achieved via compiler-level code understanding, not user feedback.

**Key insight:** Binary resolution (FP / Won't Fix) with persistence across merges is
the minimum viable signal for static analysis tools. No learning loop per se -- the
status just suppresses re-reporting.

Sources: [SonarQube False Positive Handling](https://securityboulevard.com/2026/02/how-sonarqube-minimizes-false-positives-in-code-analysis-below-5/)

### Devin

**Mechanism:** Knowledge base -- a personalized repository of project-specific facts.
Devin may suggest updates to it. Learns from task execution history, not just review
feedback.

**Key insight:** The compounding advantage -- errors decrease over weeks as Devin gains
familiarity with the project's patterns and edge cases.

Sources: [Devin Review Docs](https://docs.devin.ai/work-with-devin/devin-review)

### Cursor / Elementor Self-Learning Pattern

**Mechanism:** CI-based feedback loops. When a PR merges, the feedback cycle closes.
Human reviewer comments on AI suggestions become training signal.

**Key problem found:** Suggestion tracking bugs -- dismissed completions sometimes
silently reapplied alongside accepted ones, causing hard-to-track regressions.

**Key insight:** Merged PRs are the strongest implicit signal. If code changed in
response to a finding, the finding was real. If it didn't, it was noise.

Sources: [Cursor Feedback Loops](https://www.se-trends.de/en/cursor-ai-feedback-loops/),
[Elementor Self-Learning Code Review](https://medium.com/elementor-engineers/the-self-learning-code-review-teaching-ai-cursor-to-learn-from-human-feedback-454df64c98cc)

---

## 2. Minimum Viable Feedback Signal

### Consensus Across Tools and Models

All three Squall-consulted models (Grok, GLM, Kimi) and the industry survey converge
on the same answer:

> **1-bit per finding: accepted or rejected.**

This is the highest-entropy, lowest-burden signal. It decomposes into:

| Signal | Bits | Source | Value |
|--------|------|--------|-------|
| Per-finding accept/reject | 1 bit | Explicit caller action | Eliminates FP classes via dedup |
| Per-model accuracy (derived) | ~4 bits | Aggregated from above | Model selection/ranking |
| Implicit rejection | 0 bits (free) | File deletion or no code change | Captures signal from natural behavior |
| Per-review usefulness score | ~3 bits | Optional caller rating | Cross-model comparison |

**What NOT to collect initially:**
- Free-text explanations (high burden, low signal density)
- Continuous confidence scores (require calibration)
- Finding categories/taxonomies (premature abstraction)

### The "Merged PR" Proxy

The strongest implicit signal is whether code actually changed in response to a finding.
If the caller modifies the code a finding pointed to, that's an implicit accept. If a
review file is deleted or all its findings are ignored, that's an implicit reject.

---

## 3. Storage Design: Three Competing Approaches

### Option A: YAML Frontmatter in Existing Review Files (Kimi's Recommendation)

```markdown
---
model: claude-sonnet-4
timestamp: 2024-01-15T10:30:00Z
finding_id: "sha256:a3f4..."
feedback:
  status: rejected
  resolved_at: 2024-01-15T10:35:00Z
  source: explicit
---
## SQL Injection Risk
...
```

**Pros:** Zero new files, git-versioned, atomic with review content.
**Cons:** Requires parsing all review files to aggregate; existing 150+ files lack
frontmatter; modifying review files after creation breaks append-only semantics.

### Option B: Centralized `.squall/feedback.jsonl` (Grok's Recommendation)

```json
{"sha":"abc123","ts":"2024-01-15T10:32:00Z","model":"opus","scores":{"opus":0.9,"gpt4":0.7},"winner":"opus","merged":true}
```

**Pros:** Append-only, easy to aggregate, no dependency on review file format.
**Cons:** Requires cross-referencing with review files; separate data from findings.

### Option C: Hybrid -- Sidecar Files + Aggregate Cache (Pragmatic Middle Ground)

```
.squall/
  reviews/
    2024-01-15-auth-opus.md          # review (immutable)
    2024-01-15-auth-opus.feedback     # sidecar (mutable)
  feedback.jsonl                      # append-only event log
  model_accuracy.json                 # derived aggregate (overwritten)
```

**Pros:** Reviews stay immutable, feedback is colocated, aggregates are precomputed.
**Cons:** More files to manage.

### Recommendation

**Start with Option B (centralized JSONL)** for simplicity. It requires no changes to
existing review file format, is trivially appendable, and aggregation is a simple
streaming read. Migrate to Option C only if per-finding granularity proves necessary.

---

## 4. How Feedback Should Influence Future Reviews

### Tier 1: Finding Deduplication (Immediate Value, Day 1)

Hash findings by `(file_path, line, rule_pattern)`. Before surfacing a finding, check
if the same hash was previously rejected. Suppress or downgrade severity.

This is the SonarQube pattern -- simple, high-impact, zero ML.

### Tier 2: Model Selection/Ranking (Compound Value, Week 1)

Track per-model precision: `P(accepted | model, file_extension)`. Use this to:
- Rank models in review output (most precise first)
- Optionally skip low-precision models for certain file types
- Weight model findings differently in consensus

This is derived data -- no extra feedback collection needed, just aggregation.

### Tier 3: System Prompt Injection (Advanced, Week 2+)

Inject learned patterns into model system prompts:

```
[LEARNED PATTERNS FROM 847 PAST FINDINGS]

High-confidence false positives (avoid these patterns):
- "Unused variable" in test files: 94% rejected
- "Missing error handling" in generated proto code: 87% rejected

High-confidence real bugs (prioritize these):
- "Null dereference after map lookup": 78% accepted
- "Missing transaction rollback on error": 82% accepted
```

Requires pattern normalization and sufficient sample size (10+ per pattern).

### Tier 4: Per-Model Calibration (Month 2+)

Different models have different FP profiles (from Squall's scorecard: Grok has 4+ FP
per round on XML escaping and edition 2024; Codex has 0 FP). Inject model-specific
suppression rules.

---

## 5. Architecture Decision: Who Consumes Feedback?

### Consensus: Squall Should Consume Feedback

All three models agree: Squall should own the learning loop, not just persist data.

| Layer | Responsibility |
|-------|----------------|
| **Caller (Claude Code)** | Decides what to do with findings, provides outcome signal |
| **Squall** | Aggregates signal, derives patterns, injects into prompts, ranks models |

**Why not caller-only:**
- Callers are non-deterministic and context-limited (compaction destroys history)
- Every caller would need to reimplement learning
- Squall already owns the dispatch -- it should also own the feedback loop

**MCP tool interface:**

```
submit_feedback(review_file, finding_id, verdict: accept|reject)
get_model_stats(file_extension?) -> model rankings
```

The `review` tool automatically reads feedback.jsonl before dispatch and:
1. Filters known false positives (dedup)
2. Ranks models by historical precision
3. Injects learned patterns into system prompts

---

## 6. Proposed Minimal Schema

```json
{
  "ts": "2024-01-15T10:32:00Z",
  "review_file": "2024-01-15-auth-opus.md",
  "model": "grok-4-1-fast-reasoning",
  "finding_sig": "src/auth.rs:47:null-deref",
  "verdict": "reject",
  "source": "explicit"
}
```

Fields:
- `ts`: ISO 8601 timestamp
- `review_file`: Reference to the review markdown file
- `model`: Model that produced the finding
- `finding_sig`: Stable signature: `file:line:rule-pattern`
- `verdict`: `accept` | `reject` | `ignored`
- `source`: `explicit` (caller told us) | `implicit` (inferred from behavior)

Derived aggregate (`.squall/model_accuracy.json`, regenerated on read):

```json
{
  "grok-4-1-fast-reasoning": {
    ".rs": {"accepted": 8, "rejected": 12, "precision": 0.40},
    ".ts": {"accepted": 5, "rejected": 2, "precision": 0.71}
  },
  "codex": {
    ".rs": {"accepted": 15, "rejected": 0, "precision": 1.0}
  }
}
```

---

## 7. Implementation Roadmap

### Phase 1: Capture (1 day)
- Add `submit_feedback` MCP tool
- Append to `.squall/feedback.jsonl`
- Nothing reads it yet

### Phase 2: Dedup (1 day)
- On `review` dispatch, load rejected finding signatures
- Skip findings that match rejected signatures
- Log suppressions for transparency

### Phase 3: Model Ranking (1 day)
- Aggregate feedback.jsonl into per-model precision stats
- Surface in `listmodels` output or new `get_model_stats` tool
- Optionally auto-select top-N models per file type

### Phase 4: Prompt Injection (1 week)
- Derive pattern-level accept/reject rates (needs 10+ samples per pattern)
- Inject condensed patterns into review system prompts
- Monitor whether models actually follow the guidance

### Phase 5: Implicit Feedback (Week 2+)
- Track whether code at finding locations changed after review
- Treat unchanged code + merged PR as implicit rejection
- Treat code changes at finding locations as implicit acceptance

---

## 8. Key Takeaways

1. **1-bit per finding is enough.** Accept/reject is the minimum viable signal that
   compounds into model ranking, pattern learning, and deduplication.

2. **Start with JSONL, not a database.** Append-only, git-friendly, zero dependencies.
   SQLite and vector DBs are premature optimization.

3. **Squall should own the loop.** The caller provides the signal; Squall aggregates,
   derives patterns, and injects them into future dispatches.

4. **Deduplication is the highest-ROI first step.** Suppressing known false positives
   requires zero ML and immediately reduces noise.

5. **Implicit feedback is free.** File deletion, code changes, and PR merges are
   observable outcomes that require no explicit user action.

6. **CodeRabbit's "explain the why" principle applies.** When Squall eventually
   supports richer feedback, capturing the reasoning (not just the verdict) will
   enable better pattern generalization.

---

## Sources

### Industry Documentation
- [CodeRabbit Learnings Guide](https://docs.coderabbit.ai/guides/learnings)
- [CodeRabbit Review Instructions](https://docs.coderabbit.ai/guides/review-instructions)
- [GitHub Copilot Code Review Docs](https://docs.github.com/copilot/using-github-copilot/code-review/using-copilot-code-review)
- [Devin Review Docs](https://docs.devin.ai/work-with-devin/devin-review)
- [SonarQube FP Handling](https://securityboulevard.com/2026/02/how-sonarqube-minimizes-false-positives-in-code-analysis-below-5/)

### Design Patterns
- [Cursor AI Feedback Loops](https://www.se-trends.de/en/cursor-ai-feedback-loops/)
- [Elementor Self-Learning Code Review](https://medium.com/elementor-engineers/the-self-learning-code-review-teaching-ai-cursor-to-learn-from-human-feedback-454df64c98cc)
- [State of AI Code Review Tools 2025](https://www.devtoolsacademy.com/blog/state-of-ai-code-review-tools-2025/)
- [Best AI Code Review Tools 2026](https://www.qodo.ai/blog/best-ai-code-review-tools-2026/)
- [AI Code Review Implementation Best Practices](https://graphite.com/guides/ai-code-review-implementation-best-practices)

### Multi-Model Perspectives (via Squall review tool)
- Grok 4.1 Fast: Per-review usefulness score + winner flag, centralized JSONL, three-tier injection
- GLM-5: Per-finding binary verdict, YAML frontmatter in review files, system prompt injection with pattern aggregation
- Kimi K2.5: Per-finding accept/reject, YAML frontmatter + aggregate cache, dedup-first approach with implicit git signals
- Full model responses persisted at `.squall/reviews/1771699489536_50536_1.json`
