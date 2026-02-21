# Ensemble Methods for Multi-Model Weighting in Squall

**Research Date:** 2026-02-21
**Researcher:** ensemble-researcher (Claude Opus 4.6)
**Sources:** Web research + Squall multi-model review (Grok, Kimi, GLM)

---

## Executive Summary

Squall currently dispatches prompts to multiple AI models in parallel and concatenates outputs with attribution, letting the caller LLM synthesize. This research explores how ensemble ML methods could improve output combination through intelligent weighting, aggregation, and quality signaling -- all without requiring GPU resources.

**Key finding:** For code review, **diversity often beats raw accuracy**. A model with 70% accuracy that finds bugs others miss is worth more than a 90% model that finds the same bugs as everyone else. Weighting should prioritize **coverage of the error space** over benchmark scores.

---

## 1. Weighting Strategies

### 1.1 Condorcet Jury Theorem Analysis

The Condorcet Jury Theorem states: if each of _n_ independent voters has probability _p\_i_ > 0.5 of being correct, majority vote probability approaches 1 as _n_ grows. However, for LLM ensembles, **the independence assumption fails critically** -- models trained on similar corpora exhibit correlated errors.

The relevant extension is the **Correlated Jury Theorem**:

```
P(correct) = Phi( (sqrt(n) * (p_bar - 0.5)) / sqrt(p_bar*(1-p_bar) + (n-1)*rho*p_bar*(1-p_bar)) )
```

Where `p_bar` = average competence, `rho` = pairwise error correlation, `Phi` = standard normal CDF.

**When to add a weaker model:**
- Add model _k_ only if its **diversity gain** exceeds its accuracy penalty
- A model with 60% accuracy making *orthogonal* errors is more valuable than 75% accuracy with *correlated* errors
- Exclude models where estimated accuracy < 0.55 (buffered threshold for estimation error)

### 1.2 Static vs Adaptive Weights

| Approach | Pros | Cons | When to Use |
|----------|------|------|-------------|
| **Static** (benchmark-derived) | No cold-start, immediate | Doesn't adapt to domain drift | Baseline / cold-start |
| **Adaptive** (EMA from feedback) | Tracks real performance | Needs 50+ samples to stabilize | After sufficient feedback data |
| **Hybrid** (blend both) | Best of both worlds | Slightly more complex | Recommended approach |

**Recommended: Hybrid Static-Adaptive Weights**

```
effective_weight = 0.3 * base_weight + 0.7 * adaptive_component
                   * reliability_score
```

Where `adaptive_component` reverts to 0.5 (neutral) when fewer than 10 feedback samples exist.

**Adaptive Weight Update (EMA):**

```rust
fn update_weight(w: &mut f64, feedback_accuracy: f64, alpha: f64) {
    // alpha = 0.05-0.15 for slow adaptation
    *w = alpha * feedback_accuracy + (1.0 - alpha) * (*w);
}
```

**Model Exclusion via Wilson Score Interval:**

Use confidence interval lower bounds rather than point estimates to decide when to drop a model:

```rust
fn should_exclude(accuracy_ema: f64, sample_count: u64, threshold: f64) -> bool {
    if sample_count < 5 { return false; } // Insufficient data
    let n = sample_count as f64;
    let p = accuracy_ema;
    let z = 1.96; // 95% confidence
    let denom = 1.0 + z * z / n;
    let center = (p + z * z / (2.0 * n)) / denom;
    let margin = z * ((p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt()) / denom;
    (center - margin) < threshold // e.g., 0.4 for "worse than random"
}
```

### 1.3 Initial Static Weight Recommendations

Based on Squall's multi-model scorecard (4 review rounds):

| Model | Suggested Weight | Rationale |
|-------|-----------------|-----------|
| Codex | 0.30 | Highest precision (0 false positives), exact line refs |
| Gemini | 0.25 | Best at systems-level bugs, found all real bugs |
| Grok | 0.20 | Fast but 4+ FP/round; improved with system_prompt |
| Kimi | 0.15 | Contrarian, occasional edge cases, timeout-prone |
| GLM | 0.10 | Clear framing but zero real bugs found |

---

## 2. Aggregation Methods

### 2.1 Method Comparison

| Method | Complexity | GPU Required | Latency | Squall Fit |
|--------|-----------|--------------|---------|------------|
| **Majority Voting** | O(n) | No | <1ms | Good for binary (bug present?) |
| **Weighted Voting** | O(n) | No | <1ms | **Recommended starter** |
| **Bayesian Model Avg** | O(n*F) | No | <5ms | Target state |
| **Stacking (Linear)** | O(n*d) | No | <10ms | After 500+ labeled samples |
| **Stacking (Neural)** | O(n*d) | Yes | High | Avoid |
| **Mixture-of-Agents** | O(k*n) | No | High (multiplies API calls) | Avoid |

### 2.2 Recommended: Finding-Level Weighted Clustering

Code reviews produce **sets of findings**, not scalar predictions. The right aggregation operates at the finding level:

**Step 1: Parse findings** -- Extract structured issues from each model response:
```
Finding { file, line_range, severity, category, description, suggested_fix }
```

**Step 2: Cluster by proximity** -- Group findings from different models that reference the same code location (within 5 lines) and same category.

**Step 3: Weighted vote per cluster:**
```
aggregate_score(cluster) = sum(w_i * confidence_i) / sum(w_i)
include_finding = weighted_support > 0.5
```

**Step 4: Select best description** -- From the cluster, pick the description from the model with highest `weight * confidence`.

### 2.3 Location-Based Clustering Algorithm

```
for each file:
    sort findings by line number
    greedy merge: if line_distance <= 5 AND same category -> same cluster

line_distance(a, b) = max(0, b.start - a.end)  // 0 if overlapping
```

This is O(n log n) per file (dominated by sort) and requires no ML infrastructure.

### 2.4 Concatenation Remains Valuable

All three models agreed: for complex architectural reviews, **raw concatenation with attribution often outperforms voting** due to the complementary expertise effect. Different models catch different bug classes. The cluster-then-vote approach should be an **addition** to concatenation, not a replacement -- used to produce a prioritized summary while preserving full responses.

---

## 3. Quality Signals (Without Ground Truth)

### 3.1 Observable Proxy Signals

| Signal | Measurement | Weight | Rationale |
|--------|-------------|--------|-----------|
| **Specificity** | `line_references / total_issues` | 0.35 | Precise reviews reference exact lines |
| **Confidence** | `certainty_words - hedge_words` | 0.25 | Net confidence in findings |
| **Structure** | Finding count (penalize extremes) + suggestion ratio | 0.25 | Well-structured with actionable fixes |
| **Reliability** | Response time normalization | 0.15 | Very fast = possibly superficial |

**Composite Quality Score:**
```
q_i = 0.35 * specificity + 0.25 * confidence + 0.25 * structure + 0.15 * reliability
```

### 3.2 Signal Extraction (Rust-friendly regex)

```
LINE_REF:    r"(?i)(?:line|l)\s*[:#]?\s*(\d+)"
CODE_BLOCK:  r"```[\s\S]*?```"
HEDGING:     r"(?i)\b(might|could|possibly|perhaps|maybe)\b"
CERTAINTY:   r"(?i)\b(definitely|clearly|incorrect|wrong|must|should)\b"
```

### 3.3 Cross-Model Agreement as Signal

Findings corroborated by multiple models receive a **consensus bonus**:
```
agreement_bonus(finding) = ln(1.0 + sum(w_j for agreeing models j))
```

This uses diminishing returns (log scale) so a finding confirmed by 5 models isn't weighted dramatically more than one confirmed by 3.

### 3.4 Effective Weight Adjustment

```
w_i_effective = w_i * q_i * reliability_penalty_i
```

---

## 4. Straggler Cutoff Implications

### 4.1 Timeout-Aware Weighting

When a model times out (180s default cutoff):
- Set its contribution to 0 for that review
- Track timeout rate via EMA: `timeout_rate = alpha * timed_out + (1 - alpha) * prev_rate`
- Decay base weight: `w_adjusted = w * (1 - beta * timeout_rate)`, beta = 0.5

### 4.2 Reliability Scoring

```rust
struct ModelReliability {
    availability_ema: f64,       // EMA of success (1) / timeout (0)
    consecutive_timeouts: u8,
}

fn reliability_penalty(&self) -> f64 {
    if self.consecutive_timeouts >= 3 {
        0.0  // Hard cutoff after 3 consecutive timeouts
    } else {
        self.availability_ema.powi(2)  // Quadratic penalty
    }
}
```

### 4.3 Renormalization After Cutoff

After straggler cutoff, remaining model weights must be renormalized:
```
w_i_renormalized = w_i_effective / sum(w_j_effective for available j)
```

### 4.4 Adaptive Cutoff (Future)

Track per-model response time distributions. Set cutoff at p95 * safety_factor (2x), bounded by [min_cutoff, max_cutoff]. This avoids cutting off models that are consistently slow but valuable.

---

## 5. Practical Implementation Roadmap

### Phase 1: Static Weighted Concatenation (Week 1, ~50 LoC)

- Add `model_weights` to config (HashMap<ModelId, f64>)
- Sort responses by weight descending in output
- Display weight alongside model attribution
- **No behavioral change** -- just adds weight metadata for caller

### Phase 2: Quality Signal Extraction (Weeks 2-3, ~150 LoC)

- Implement regex-based signal extraction (line refs, code blocks, hedging/certainty)
- Compute per-response quality score
- Log quality signals (even if not used for weighting yet -- data collection for Phase 4)
- Add reliability tracking (timeout EMA, consecutive timeout counter)

### Phase 3: Finding-Level Aggregation (Month 2, ~200 LoC)

- Parse model outputs into structured findings (file, line, severity, description)
- Implement location-based clustering
- Weighted voting per cluster
- Produce prioritized summary alongside full concatenated output

### Phase 4: Adaptive Weighting (Month 3+, ~100 LoC)

- Add feedback API hook (caller sends thumbs-up/down with model attribution)
- Implement EMA weight updates
- Persist weights to JSON or SQLite
- Wilson score interval for model exclusion decisions

### What to Avoid

- **Neural meta-learners** -- require training data and infrastructure Squall doesn't have
- **Mixture-of-Agents iterative synthesis** -- multiplies API costs and latency
- **Complex stacking** -- diminishing returns for code review vs simpler methods
- **Token-level fusion (FusionRoute)** -- requires access to model logits, impossible with API access

---

## 6. Key Insights from Multi-Model Review

### Model Agreement

All three models (Grok, Kimi, GLM) converged on these recommendations:
1. Start with static weighted voting, evolve to adaptive
2. Code review is a **structured prediction task** -- extract findings, don't just average text
3. Quality signals are achievable with regex alone, no NLP models needed
4. Concatenation should be preserved alongside any aggregation

### Model Disagreements

- **Grok** favored aggressive model exclusion (drop if p < 0.55)
- **Kimi** emphasized the diversity-accuracy tradeoff (keep weak-but-diverse models)
- **GLM** recommended conservative exclusion via Wilson score intervals (need statistical confidence)

**Resolution:** GLM's approach is most principled -- use Wilson score lower bounds for exclusion decisions, which naturally requires sufficient data before dropping models.

### Unique Contributions by Model

- **Grok** (20s): Provided the clearest EMA formulas and the Exp3 bandit algorithm suggestion
- **Kimi** (140s): Best analysis of Condorcet limitations (correlation breaks independence); introduced inverse probability weighting for missing responses
- **GLM** (178s): Most comprehensive Rust pseudocode; introduced finding-level clustering with cross-model agreement bonus

---

## 7. Relevant Literature

- [Ensemble Large Language Models: A Survey](https://www.mdpi.com/2078-2489/16/8/688) -- Comprehensive survey of LLM ensemble approaches
- [Mixture-of-Agents Enhances LLM Capabilities](https://arxiv.org/abs/2406.04692) -- MoA architecture (ICLR 2025)
- [Rethinking Mixture-of-Agents](https://arxiv.org/abs/2502.00674) -- Self-MoA outperforms standard MoA by 6.6%
- [PoLL: Panel of LLM Evaluators](https://medium.com/@techsachin/replacing-judges-with-juries-llm-generation-evaluations-with-panel-of-llm-evaluators-d1e77dfb521e) -- Smaller diverse panels outperform single large judges at 1/7 cost
- [Condorcet's Jury Theorem](https://en.wikipedia.org/wiki/Condorcet%27s_jury_theorem) -- Foundational theorem on majority voting accuracy
- [Ensemble of DNNs based on Condorcet's Jury Theorem](https://pmc.ncbi.nlm.nih.gov/articles/PMC9404085/) -- Applied Condorcet to neural ensemble voting
- [Token-Level LLM Collaboration via FusionRoute](https://arxiv.org/html/2601.05106) -- Token-level fusion (not practical for API-only access)
- [LLMs-as-Judges Survey](https://arxiv.org/html/2412.05579v2) -- Comprehensive survey on LLM evaluation methods
- [Ensemble Learning for LLMs in Text Classification](https://arxiv.org/pdf/2503.13505) -- Output ensemble techniques without training

---

## 8. Connection to Squall Architecture

Squall's current design (concatenate with attribution, straggler cutoff) is already a solid foundation. The ensemble methods described here are **additive enhancements**, not replacements:

| Current | Enhancement | Preserved? |
|---------|-------------|------------|
| Parallel dispatch | No change | Yes |
| Straggler cutoff (180s) | Add reliability tracking + adaptive cutoff | Yes |
| Concatenation with attribution | Add weighted priority ordering + finding summary | Yes |
| `.squall/reviews/` persistence | Add quality signal logs alongside | Yes |
| `system_prompt` per model | Use for review-angle specialization | Yes |

The review tool's `per_model_system_prompts` feature is particularly valuable -- it enables **specialized review angles** (security expert, performance expert, etc.) which directly increases the diversity term in the Condorcet formula, making the ensemble more effective even with correlated base models.
