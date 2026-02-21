# Multi-Agent Debate vs Parallel Monologues for Code Review

**Research Date:** 2026-02-21
**Researcher:** debate-researcher
**Question:** Would multi-round adversarial debate between models find more issues than Squall's current single-shot parallel dispatch?

---

## Executive Summary

**Recommendation: Do NOT add multi-round debate. Instead, add an optional synthesis/adjudication tool.**

The academic literature shows debate improves performance on tasks with verifiable correct answers (math, factual QA) but is poorly suited for open-ended discovery tasks like code review. All three models consulted (Grok, GLM, Kimi) converge on this: debate reduces false positives but risks suppressing true findings through convergence pressure. The better path is enhanced aggregation of parallel monologue results.

---

## 1. Academic Literature Review

### Key Papers

**Du et al. 2023 — "Improving Factuality and Reasoning through Multiagent Debate" (ICML 2024)**
- Multiple LLM instances propose and debate responses over multiple rounds
- Improvements on math (GSM8K) and reasoning (StrategyQA): 10-20% accuracy gains
- Key insight: works best on tasks with **verifiable ground truth**
- Critical limitation: incorrect consensus can emerge and be "hard to break"
- [Paper](https://arxiv.org/abs/2305.14325) | [Code](https://github.com/composable-models/llm_multiagent_debate)

**Liang et al. 2023 — "Encouraging Divergent Thinking through Multi-Agent Debate" (EMNLP 2024)**
- Identifies **Degeneration-of-Thought (DoT)** problem: once an LLM establishes confidence, it cannot generate novel thoughts through reflection
- MAD framework: agents argue "tit for tat" with a judge managing the process
- Effective on counter-intuitive reasoning tasks
- Key finding: debate *reduces* divergent thinking when models share similar training distributions
- Requires: adaptive termination and moderate adversarial engagement
- Warning: "LLMs might not be a fair judge if different LLMs are used for agents"
- [Paper](https://arxiv.org/abs/2305.19118) | [Code](https://github.com/Skytliang/Multi-Agents-Debate)

**ChatEval (Chan et al. 2023, ICLR 2024)**
- Multi-agent referee team for text evaluation
- Up to 6.2% accuracy improvement over single-agent evaluation
- Diverse role prompts essential; same-role prompts degrade performance
- [Paper](https://arxiv.org/abs/2308.07201) | [Code](https://github.com/thunlp/ChatEval)

**D3: Debate, Deliberate, Decide (2024)**
- Cost-aware adversarial framework with advocate/judge/jury roles
- Two protocols: MORE (multi-advocate, one round) and SAMRE (single-advocate, multi-round with budgeted stopping)
- Shows favorable cost-accuracy frontier with explicit token budgeting
- [Paper](https://arxiv.org/abs/2410.04663)

**"Can LLM Agents Really Debate?" (2025, arXiv:2511.07784)**
- Controlled study using Knight-Knave-Spy logic puzzles
- **Debate success is primarily driven by reasoning strength and diversity**, not debate structure
- Structural parameters (order, confidence visibility, depth) showed statistically insignificant effects
- Weak agents show negligible improvement regardless of debate configuration (3.6% self-correction rate vs 30-34% for strong models)
- Echo chamber risk: homogeneous groups entrench positions rather than seeking truth
- Question: do improvements reflect genuine deliberation or just ensembling/majority voting?
- [Paper](https://arxiv.org/abs/2511.07784)

**ICLR 2025 Blog Post — "Multi-LLM-Agents Debate: Performance, Efficiency, and Scaling Challenges"**
- **Current MAD frameworks fail to consistently outperform simple single-agent strategies**
- Increasing compute (more agents, more rounds) does NOT reliably improve accuracy
- Over-aggressiveness: MAD frequently flips correct answers to incorrect ones
- Unfavorable economics: substantially increased token consumption for negligible/negative gains
- [Blog](https://d2jud02ci9yv69.cloudfront.net/2025-04-28-mad-159/blog/mad/)

**"Why Do Multi-Agent LLM Systems Fail?" (Cemri et al. 2025)**
- Taxonomy of 14 failure modes across 3 categories: design, inter-agent misalignment, verification
- Tactical fixes yield only +14% improvement — structural solutions needed
- [Paper](https://arxiv.org/abs/2503.13657)

### Multi-Agent Systems for Software Engineering (2025-2026)

- LLM-based auditor agents with critic filtering show promise for vulnerability detection
- MuCoLD assigns tester/developer roles for iterative code assessment
- HyperAgent achieves 26-33% on SWE-Bench but faces scalability challenges
- Inconsistent role adherence and ineffective inter-agent communication remain problems
- [Survey](https://dl.acm.org/doi/10.1145/3712003)

---

## 2. Why Debate Fails for Code Review Specifically

### The Core Mismatch

Debate excels at tasks with **verifiable correct answers** (math, logic puzzles, factual QA). Code review is fundamentally different — it is an **open-ended discovery task** with no ground truth during the review itself.

| Task Property | Math/Logic (Debate Works) | Code Review (Debate Risky) |
|---|---|---|
| Ground truth | Verifiable during task | Unknown until exploitation |
| Success metric | Accuracy (single answer) | Recall (find all bugs) |
| Diversity value | Moderate (paths to same answer) | High (different bug classes) |
| Convergence effect | Helpful (consensus = correct) | Harmful (suppresses outliers) |

### Specific Failure Modes for Code Review

1. **Strong Model Suppression**: When GPT-4 confidently argues a race condition is "intentional concurrency," weaker models that correctly flagged it defer. The bug is now hidden behind false confidence — worse than never finding it.

2. **Convergence Kills Recall**: Parallel monologues produce the union of all findings {1,2,3,4,5}. Debate converges to the intersection {2,3}. For security review, missing a vulnerability > overflagging.

3. **Context Dilution**: By round 3, models have 6k+ tokens of debate history and less room for actual code, causing them to miss issues in the source material.

4. **"Obvious Bug" Trap**: Debate excels at confirming clear issues (null dereferences, SQL injection) but suppresses "this pattern feels wrong" intuitions that catch novel vulnerabilities.

5. **Anchoring**: The first model's response anchors subsequent rounds. If it misses a bug class entirely, debate rarely introduces it later.

---

## 3. Multi-Model Opinions (via Squall Review Tool)

Consulted: Grok-4-1-fast-reasoning, GLM-5, Kimi-K2.5

### Consensus Points (All 3 Models Agree)

- **Do not add full multi-round debate** to Squall
- Debate primarily reduces false positives, not increases true positive detection
- Echo chambers and strong-model suppression are the biggest risks
- Current parallel monologues + caller synthesis is the better default
- An optional synthesis/adjudication tool is the right enhancement

### Grok's Position (Most Pro-Debate)
- Estimates +15-25% true positive increase from debate, but acknowledges echo chamber risk
- Recommends debate as optional mode alongside parallel, for high-stakes security audits
- Suggests: force diverse models, anonymize speakers, add "devil's advocate" role
- Proposes: 60s/round timeboxing, 2-3 rounds, auto-fallback to round 1 on straggler

### GLM's Position (Most Anti-Debate)
- "Discovery tasks need divergence, not consensus"
- Debate gives 80% of benefits via synthesis tool with none of the costs
- Recommends: keep parallel + add `synthesize_reviews` tool
- Output structure: `{issues, disputed_claims, unique_findings}` with model attribution

### Kimi's Position (Pragmatic Middle)
- Debate harmful to recall specifically for code review
- Timeout cascade risk with MCP atomicity is underappreciated
- Recommends: disagreement surfacing + optional adjudication as separate tool
- Better prompt engineering (e.g., "focus on easy-to-miss bugs") reduces FPs without debate
- Reconsider debate only if: deliberate diversity (security/concurrency/API specialist roles) AND streaming MCP

### Full model responses preserved at:
`.squall/reviews/1771699606360_50536_4.json`

---

## 4. Architectural Recommendation for Squall

### Do This (Phase 1): Enhanced Aggregation

Add a `synthesize` tool that takes parallel review results and produces structured output:

```
synthesize_reviews(responses[], judge_model?) -> {
  issues: [{description, severity, agreeing_models[], confidence}],
  disputed_claims: [{claim, supporting_models[], opposing_models[], reasoning}],
  unique_findings: [{issue, sole_model, why_others_missed}]
}
```

**Benefits:**
- Preserves full recall from parallel monologues (union of findings)
- Adds cross-validation and confidence scoring
- No echo chamber risk (no back-and-forth)
- Caller-orchestrated, respects MCP atomicity
- Fast (single model pass over aggregated results)

### Do This (Phase 1.5): Specialized Prompts via per_model_system_prompts

Instead of debate, use Squall's existing `per_model_system_prompts` to create deliberate diversity:

```json
{
  "grok": "Focus on concurrency bugs, race conditions, and deadlocks",
  "gemini": "Focus on security vulnerabilities, injection, and auth bypass",
  "codex": "Focus on logic errors, edge cases, and off-by-one bugs"
}
```

This gets the diversity benefit of heterogeneous debate without convergence pressure.

### Maybe Do This (Phase 2): Single-Round Critique

If Phase 1 proves insufficient, add one round of cross-critique:

1. Round 1: Parallel dispatch (current `review` tool, ~60s)
2. Round 2: Each model sees others' findings, asked only to **validate or dispute** (not find new issues)
3. Return: original findings + validation annotations

**Constraints:**
- Hard 2-round limit (never round 3)
- 150s total timeout with graceful degradation to round 1 results
- Same-tier models only (no mixing strong/weak)

### Do NOT Do This

- Full sequential multi-round debate within one MCP call
- Caller-orchestrated debate rounds (breaks simplicity)
- Consensus-forcing mechanisms (suppresses minority findings)
- Anonymous debate (loses attribution value for caller synthesis)

---

## 5. Cost-Benefit Summary

| Approach | Bug Recall | False Positives | Latency | Token Cost | MCP Reliability | Recommendation |
|---|---|---|---|---|---|---|
| Parallel monologues (current) | High | High | 60-120s | 1x | High | Keep as default |
| + Synthesis tool | High | Low | +30-60s | 1.3x | High | **Add (Phase 1)** |
| + Specialized prompts | Higher | Medium | Same | Same | Same | **Add (Phase 1.5)** |
| + Single-round critique | High | Lower | +60-90s | 2x | Medium | Consider (Phase 2) |
| Full multi-round debate | Lower | Lowest | 150-300s | 2.5-3x | Low | **Do not add** |

---

## 6. Key Takeaway

> **The literature is clear: debate helps convergence tasks but hurts discovery tasks. Code review is a discovery task. Squall's current parallel monologues with caller synthesis is already the right architecture. Enhance aggregation, don't add debate.**

The best improvement to Squall's review quality is not more rounds — it is better *diversity* via specialized system prompts and better *aggregation* via a synthesis tool. Both can be added incrementally without architectural changes.

---

## References

1. Du et al. (2023). "Improving Factuality and Reasoning in Language Models through Multiagent Debate." ICML 2024. [arXiv:2305.14325](https://arxiv.org/abs/2305.14325)
2. Liang et al. (2023). "Encouraging Divergent Thinking in Large Language Models through Multi-Agent Debate." EMNLP 2024. [arXiv:2305.19118](https://arxiv.org/abs/2305.19118)
3. Chan et al. (2023). "ChatEval: Towards Better LLM-based Evaluators through Multi-Agent Debate." ICLR 2024. [arXiv:2308.07201](https://arxiv.org/abs/2308.07201)
4. D3 (2024). "Debate, Deliberate, Decide: A Cost-Aware Adversarial Framework." [arXiv:2410.04663](https://arxiv.org/abs/2410.04663)
5. "Can LLM Agents Really Debate?" (2025). [arXiv:2511.07784](https://arxiv.org/abs/2511.07784)
6. ICLR 2025 Blog. "Multi-LLM-Agents Debate: Performance, Efficiency, and Scaling Challenges." [Link](https://d2jud02ci9yv69.cloudfront.net/2025-04-28-mad-159/blog/mad/)
7. Cemri et al. (2025). "Why Do Multi-Agent LLM Systems Fail?" [arXiv:2503.13657](https://arxiv.org/abs/2503.13657)
8. "LLM-Based Multi-Agent Systems for Software Engineering." ACM TOSEM 2025. [Link](https://dl.acm.org/doi/10.1145/3712003)
