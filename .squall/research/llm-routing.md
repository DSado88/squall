# LLM Routing Strategies: Research Synthesis

> Research conducted 2026-02-21 for Squall MCP server.
> Sources: RouteLLM (UC Berkeley/LMSYS), Martian, Unify.ai, OpenRouter, semantic routing papers, and multi-model review via Squall (Grok, Kimi K2.5, GLM-5).

---

## 1. Landscape of LLM Routing Approaches

### 1.1 Commercial Platforms

| Platform | Approach | Key Differentiator |
|----------|----------|-------------------|
| **Martian** | Predicts model internals without running them | Claims 50-80% cost reduction; context-aware prompt analysis |
| **Not Diamond** | Learned classifier routing | Powers OpenRouter's auto-router |
| **Unify.ai** | Neural network on dynamic benchmarks | Three sliders (quality/cost/latency); custom router training on user data; benchmarks updated every 10 minutes |
| **OpenRouter** | Auto-router via Not Diamond + manual shortcuts | `:nitro` (fastest throughput), `:floor` (lowest price); model pool restrictions via plugins |
| **Neutrino AI** | Query-model matching | Enterprise-focused |
| **Requesty** | Configurable routing algorithms | Uptime + cost optimization |

### 1.2 Open-Source Frameworks

| Framework | Architecture | Training Signal |
|-----------|-------------|----------------|
| **RouteLLM** (LMSYS/Berkeley) | 4 router types (SW ranking, matrix factorization, BERT classifier, causal LLM) | Chatbot Arena preference data + augmentation |
| **Semantic Router** (Red Hat) | BERT embeddings + cosine similarity to task vectors | Predefined task categories |
| **UniRoute** | Cluster-based routing with model feature vectors | Performance on representative prompts |
| **RAGRouter** | Contrastive learning for RAG-specific routing | How retrieved docs impact model capabilities |
| **DiSRouter** | Distributed self-routing; each LLM has intrinsic self-awareness | Scalable, plug-and-play |
| **LLMRank** | Semantic-aware feature-driven design | Prompt semantics + multiple feature signals |

### 1.3 Research Papers of Note

- **RouteLLM** (ICLR 2025): Strong-vs-weak binary routing; 85% cost reduction on MT-Bench at 95% quality; generalizes across model pairs without retraining.
- **Universal Model Routing** (arXiv 2502.08773): Routes to "smallest feasible LLM" per prompt; validated on 30+ unseen models; theoretical excess risk bounds.
- **"Doing More with Less"** (arXiv 2502.00409): Extended survey on routing strategies in LLM-based systems.
- **Lookahead Routing** (arXiv 2510.19506): Forward-looking routing that anticipates downstream quality.
- **Lessons Learned from LLM Routing** (ACL Insights 2024): Practical pitfalls in production routing.

---

## 2. Signals That Predict Optimal Model Selection

### 2.1 High-Value Signals (consensus across all sources)

| Signal | Extraction Method | Predictive Power |
|--------|------------------|-----------------|
| **Task/intent type** | Keyword detection, user flags, zero-shot classifier | Very High |
| **Security-sensitive paths** | Pattern matching on file paths + code content (`auth`, `crypto`, `eval`, `exec`) | Very High |
| **Complexity density** | Cyclomatic complexity per changed function (via `tree-sitter`, `radon`) | High |
| **Diff size (tokenized)** | Token count via `tiktoken` or equivalent | High |
| **Cross-module scope** | Files spanning multiple directories/modules | High |
| **Language rarity** | Mainstream (Python/JS/Go) vs niche (Rust/Zig/COBOL) | Medium-High |
| **Unsafe code patterns** | `unsafe`, raw SQL, `eval()`, inline assembly | High |
| **Generated code detection** | Filename patterns, boilerplate markers | Medium |

### 2.2 Signals That Matter Less Than Expected

- **Total diff size alone** -- a 500-line schema migration needs less scrutiny than a 50-line crypto fix
- **Raw file count** -- spreading a simple refactor across 20 files is still simple
- **Language in isolation** -- frontier models handle mainstream languages comparably; language only matters at extremes (rare/niche)

### 2.3 Signal Extraction Architecture

All three Squall models converged on a lightweight feature vector approach:

```
ReviewSignals {
    security_sensitive: bool,      // path patterns + code patterns
    complexity_density: float,     // avg cyclomatic complexity per changed function
    cross_module: bool,            // changes span multiple directories
    unsafe_patterns: list[str],    // detected risky patterns
    languages: set[str],           // detected languages
    diff_tokens: int,              // tokenized size
    is_generated: bool,            // boilerplate/migration detection
    review_type: Option<str>,      // user-provided hint: security|architecture|bugs|style
    priority: str,                 // user-provided: quick|thorough|comprehensive
}
```

Extraction should be fast (<100ms) using AST tools like `tree-sitter`, not LLM inference.

---

## 3. Cost / Quality / Latency Tradeoffs

### 3.1 The Code Review Economics Insight

Code review has unique economics vs chatbots (consensus from Kimi and GLM):

- **False negatives are catastrophically expensive**: Missing a bug costs 100-1000x the API call. This inverts typical routing economics -- err on the side of stronger models for anything uncertain.
- **Latency is "soft real-time"**: Developers can context-switch, so 10-30s is acceptable. Time-to-first-token matters more than total time.
- **Variable stakes**: Style nits vs security vulns have orders-of-magnitude different impact.

### 3.2 Recommended Tiering Strategy

| Tier | Use Case | Model Class | Latency Target | Est. Cost/Review |
|------|----------|-------------|----------------|-----------------|
| **T1: Triage** | Style, generated code, trivial diffs (<20 lines) | Haiku, Flash, GPT-4o-mini, Llama-3.1-8B | <2s | $0.05-0.30 |
| **T2: Standard** | Bug detection, logic review, 1-10k token diffs | Sonnet, GPT-4o-mini, DeepSeek Coder | 3-15s | $0.50-2.00 |
| **T3: Deep Audit** | Security, architecture, complex concurrency, rare languages | Opus, o1, GPT-4o, extended thinking | 10-60s | $5-20 |

### 3.3 Utility Function

```
score = alpha * quality + beta * (1/cost) + gamma * (1/latency)
```

Recommended calibration for code review: alpha=0.6, beta=0.2, gamma=0.2. RouteLLM benchmarks show 2x cost savings with <1% quality drop using this balance.

### 3.4 Projected Savings

For a team doing 50 PRs/week (from GLM analysis):

| Strategy | Weekly Cost | Latency P50 | Quality Score |
|----------|-------------|-------------|---------------|
| All Frontier | ~$150 | 45s | 94% |
| All Medium | ~$30 | 12s | 87% |
| Smart Routing | ~$45 | 15s | 92% |

Smart routing captures ~85% of the quality gain at ~30% of the cost.

---

## 4. Routing Architecture for Squall

### 4.1 Design Principle: Reduce, Don't Replace

All three models agreed: **don't replace fan-out, reduce the model set before fan-out**. Given Squall already has parallel dispatch via the `review` tool, the router should prune models (5 -> 1-3) rather than trying to pick a single winner.

### 4.2 Recommended Architecture: Reduce -> Prioritize -> Execute

```
Input (diff + context)
        |
        v
  Signal Extractor          (<100ms, rule-based, tree-sitter/regex)
        |
        v
  Rule Engine / Router      (<10ms for rules, ~200ms if learned)
        |
        v
  Candidate Models + Priority Order
        |
        v
  Existing Squall fan-out   (parallel dispatch with straggler cutoff)
        |
        v
  Aggregated Results
```

### 4.3 Three Architecture Options (from Kimi analysis)

| Option | Description | Pros | Cons | Recommended? |
|--------|-------------|------|------|-------------|
| **A: Model Reduction** | Pre-filter to K=1-2 models | Cost linearity, predictable latency | No ensemble for ambiguous cases | Yes (MVP) |
| **B: Priority Streaming** | Fan out all, stream by confidence, circuit-break | Latency optimization | Complex cancellation in MCP | No (too complex) |
| **C: Mixture-of-Reviewers** | 3 specialized models + aggregator | Maximum coverage | 3x cost, conflict resolution | Later phase |

**Recommendation**: Option A with fallback. Router selects 1-2 models. If primary returns "uncertain" or empty findings on a high-risk file type, automatically trigger a secondary.

### 4.4 User Override Must Be Preserved

The `models` parameter on the `review` tool should always override automatic routing. Add a new `strategy` parameter:

```
strategy: "auto" | "fast" | "thorough" | "security"
  - auto: Use router (default when models not specified)
  - fast: T1 models only
  - thorough: T2-T3, fan out to 2-3
  - security: T3 models, no shortcuts
```

When `models` is explicitly provided, skip the router entirely.

---

## 5. Failure Modes of LLM Routing

### 5.1 Critical Failure Modes

| Mode | Description | Mitigation |
|------|-------------|-----------|
| **Misclassification cascade** | Router misidentifies security change as "minor refactor" -> cheap model misses vulnerability | Over-index on security signals; pattern allowlist bypasses routing |
| **Distribution shift** | Router trained on Python/JS PRs encounters Rust/COBOL | OOD detection via embedding distance; fallback to frontier for unseen patterns |
| **Cascade false negatives** | Cheap model confidently says "LGTM" on complex code | Confidence thresholds; safety gates for `eval()`, raw SQL, `unsafe` |
| **Stale router, fresh models** | Models update but router has old performance data | Version tracking; monthly re-evaluation |
| **Router latency overhead** | Classification takes 500ms but review on small model takes 300ms | Rule-based routing for 90% of cases; skip routing for trivial diffs |
| **Over-optimization on synthetic data** | Router trained on LeetCode-style problems fails on messy production code | Training data must include real, messy production diffs |

### 5.2 When Manual Selection Beats Automatic

- **Rare languages** (<1% of traffic) -- router has no signal
- **User-known preferences** ("always use Sonnet for this repo")
- **Novel vulnerability classes** not in training data
- **Mixed-intent PRs** that span security + style + architecture
- **High-stakes changes** where any routing error is unacceptable

### 5.3 Safety Principle

All three models independently recommended the same safety principle: **never route security-sensitive code to cheap models regardless of other signals**. Implement a hard gate:

```
SECURITY_PATTERNS = [auth, login, session, token, password, secret,
                     crypto, cipher, encrypt, hash, permission, privilege,
                     oauth, jwt, sql, query, execute, eval, unsafe]

if any pattern in file_paths or diff_content:
    route -> FRONTIER (bypass all other routing logic)
```

---

## 6. Minimum Viable Implementation

### 6.1 Phase 1: Rule-Based Router (Week 1-2)

Zero ML, ~200 LOC, immediate 40-60% cost savings on obvious cases.

**Decision tree:**
1. Security gate: security patterns detected -> Frontier (always)
2. User hint: `review_type` provided -> map directly to tier
3. Generated code: boilerplate patterns -> T1 Small
4. Trivial diff: <20 lines, no security signals -> T1 Small
5. Rare language: not in mainstream set -> Frontier
6. Complex logic: cyclomatic complexity > 10 or cross-module -> T2 Medium
7. Default: T2 Medium

**Signal extraction** uses `tree-sitter` for language detection and complexity, regex for security patterns, token counting for diff size. All under 100ms.

### 6.2 Phase 2: Feedback Collection (Week 2-4)

Log every routing decision with:
- Input signals (feature vector)
- Model(s) selected + reasoning
- Outcome: user rating, issues found, false positives, latency, cost
- Whether user overrode the routing

This creates the training set for Phase 3.

### 6.3 Phase 3: Learned Router (Month 2-3)

With 1000+ rated reviews:
- Train a small classifier (DistilBERT or gradient boosting on hand-crafted features)
- A/B test: 50% heuristic, 50% learned
- Measure: human satisfaction, bug catch rate, cost per bug found

### 6.4 Phase 4: Uncertainty-Based Fan-Out (Month 4+)

- If classifier confidence < 0.7, fan out to 2 models instead of 1
- Add cost-aware scheduling (respect user budget limits)
- Dynamic ensemble based on accumulated model scorecards

### 6.5 Key Metrics to Monitor

| Metric | What It Measures | Target |
|--------|-----------------|--------|
| **Routing accuracy** | % of times user "upgrades" selected model | <10% upgrade rate |
| **Cost per bug found** | Efficiency, normalized by severity | Lower than all-frontier |
| **Escape rate** | Critical bugs found by manual review that routing sent to cheap models | <1% |
| **Router latency** | Overhead of routing decision | <100ms for rules, <300ms for learned |
| **Quality retention** | Review quality vs all-frontier baseline | >90% |

---

## 7. Relevance to Squall's Current Architecture

### 7.1 What Squall Already Has

- Parallel fan-out via `review` tool with straggler cutoff
- Model registry with HTTP and CLI backends
- `system_prompt` and `temperature` knobs
- File context inclusion
- Results persistence to `.squall/reviews/`

### 7.2 What Would Need to Be Added

1. **Signal extractor module** -- analyzes diff/files before dispatch, produces `ReviewSignals`
2. **Router module** -- maps signals to model set (initially rule-based)
3. **`strategy` parameter** on `review` tool -- `auto|fast|thorough|security`
4. **Routing decision logging** -- for feedback collection and later ML training
5. **Security pattern allowlist** -- hard-coded bypass for security-sensitive code

### 7.3 Integration Points

The router sits **between** the `review` tool's input parsing and its model dispatch. Minimal changes to existing fan-out logic:

```
Current:  review(prompt, models) -> fan_out(models)
Proposed: review(prompt, models?, strategy?) -> route(signals) -> fan_out(selected_models)
```

When `models` is explicitly provided, routing is bypassed entirely (backward compatible).

### 7.4 Alignment with Multi-Model Scorecard

Squall's existing scorecard data (from MEMORY.md) directly informs initial routing rules:

| Model | Strength | Weakness | Routing Implication |
|-------|----------|----------|-------------------|
| Gemini | Systems-level bugs | Slower | Route for complex systems code |
| Codex | Highest precision, 0 false positives | Slow (50-300s) | Route for high-stakes where precision matters |
| Grok | Fast (20-65s) | 4+ FP/round, blind spots | Route for quick triage, not security |
| GLM | Architectural framing | Zero real bugs found | Route for architecture reviews only |
| Kimi | Contrarian edge cases | Timeouts at 300s | Route as diversity pick on thorough reviews |

---

## 8. Key Takeaways

1. **Start with rules, not ML.** Rule-based routing is debuggable, predictable, and captures 80% of the value. ML routing adds ~10-15% over good heuristics at 10x the complexity.

2. **Security is a hard gate, not a signal.** Never route security-sensitive code to cheap models. Over-detection is far cheaper than under-detection.

3. **Reduce the fan-out set, don't replace fan-out.** Squall's existing parallel dispatch is the right architecture. The router just prunes 5 models to 1-3.

4. **False negatives dominate code review economics.** Unlike chatbots where routing errors mean slightly worse responses, in code review a routing error means a missed vulnerability. Err toward stronger models.

5. **User override is sacred.** Explicit `models` selection always bypasses routing. The router is a smart default, not a gatekeeper.

6. **RouteLLM demonstrates strong generalization.** Routers trained on one model pair (GPT-4/Mixtral) transfer to other pairs (Claude/Llama) without retraining -- suggesting that routers learn task difficulty, not model-specific features.

7. **The existing Squall scorecard is your initial routing table.** Four rounds of multi-model review have already revealed which models excel at what. Encode this directly as Phase 1 rules.
