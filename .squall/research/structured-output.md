# Structured Output Schemas Across Models

## Research Summary

Squall dispatches to 5 models across 3 backends. This document maps each provider's
structured output capabilities and recommends a common schema for code review findings
that enables automated consensus detection.

---

## 1. Provider Capability Matrix

| Provider | Model | Backend | `response_format` | `json_schema` | Structured Output | Notes |
|----------|-------|---------|-------------------|---------------|-------------------|-------|
| xAI | Grok-4 family | HTTP (direct) | Yes | Yes (`type: "json_schema"`) | Full | OpenAI-compatible. `strict: true` supported. All language models. |
| Google | Gemini 2.5/3.x | CLI (`gemini`) | Different params | `responseMimeType` + `responseSchema` | Full (API) / Limited (CLI) | CLI has `--output-format json` but no schema enforcement. API uses own param names, not OpenAI-compatible. |
| OpenAI | Codex | CLI (`codex`) | N/A (CLI) | `--output-schema file.json` | Yes (CLI flag) | `codex exec --output-schema schema.json` enforces JSON Schema on final output. Non-interactive mode only. |
| OpenRouter | Kimi K2.5 | HTTP | Yes | Yes (`type: "json_schema"`) | Yes | `response_format` and `structured_outputs` both listed as supported params. |
| OpenRouter | GLM-5 | HTTP | **No** | **No** | **No** | Supported params: reasoning, temperature, top_p, top_k, frequency_penalty, tools, tool_choice. No `response_format`. |
| Z.AI (direct) | GLM-4.5+ | HTTP | Yes | Yes (`json_object`) | Yes | Direct Z.AI API supports `response_format: {"type": "json_object"}`. OpenRouter wrapper does not expose this. |

### Key Insight: Lowest Common Denominator

GLM-5 on OpenRouter does **not** support `response_format`. This means Squall cannot
rely on provider-enforced JSON Schema for all models. The pragmatic approach:

1. **Use `response_format: json_schema` where available** (Grok, Kimi on OpenRouter)
2. **Use `--output-schema` for Codex CLI**
3. **Use prompt-based JSON enforcement for Gemini CLI and GLM-5** (system prompt instructs model to respond in JSON matching the schema)
4. **Validate all responses client-side** regardless of provider enforcement

This hybrid approach maximizes reliability where possible while gracefully degrading
to prompt-based enforcement for providers that lack native support.

---

## 2. Provider-Specific Details

### xAI (Grok)

- **Endpoint**: OpenAI-compatible chat completions
- **Structured output**: `response_format: { type: "json_schema", json_schema: { name: "...", schema: {...}, strict: true } }`
- **Supported types**: string, number, integer, boolean, object, array, enum, anyOf
- **Limitations**: `allOf` not supported. No `minItems`/`maxItems`/`minContains`/`maxContains` on arrays.
- **Guarantee**: "Response is guaranteed to match your input schema" when using structured outputs.

### Google Gemini

- **API**: Uses `response_mime_type: "application/json"` + `response_json_schema: {...}` (NOT `response_format`)
- **CLI**: `gemini --output-format json` provides JSON output but no schema enforcement
- **Supported types**: string, number, integer, boolean, object, array, null
- **Key properties**: description, title, enum, required, minItems/maxItems, minimum/maximum
- **Limitations**: Not all JSON Schema features supported; very large/deeply nested schemas may be rejected
- **Squall implication**: Since Squall uses Gemini via CLI, must rely on prompt-based JSON enforcement

### OpenAI Codex

- **CLI**: `codex exec "prompt" --output-schema schema.json` enforces JSON Schema on output
- **Schema file**: Standard JSON Schema format, placed in a `.json` file
- **Output**: `codex exec "prompt" --output-schema ./schema.json -o ./output.json`
- **Also available**: `--json` for JSONL event stream, `-o` for final message capture
- **Squall implication**: Can pass schema file to Codex CLI for enforced structured output

### Moonshot Kimi (via OpenRouter)

- **OpenRouter supported params**: `response_format`, `structured_outputs`, `tools`, `tool_choice`, temperature, top_p, top_k, frequency_penalty, reasoning, include_reasoning
- **Implementation**: Standard OpenRouter `response_format: { type: "json_schema", json_schema: {...} }`
- **Direct Moonshot API**: Limited/no native `response_format` support; OpenRouter adds this capability
- **Squall implication**: Full structured output support via OpenRouter passthrough

### Zhipu GLM (via OpenRouter)

- **OpenRouter supported params**: reasoning, include_reasoning, temperature, top_p, top_k, frequency_penalty, tools, tool_choice
- **Missing**: `response_format`, `structured_outputs` NOT in supported params list
- **Direct Z.AI API**: Supports `response_format: {"type": "json_object"}` and JSON Schema for GLM-4.5+
- **Squall implication**: Must use prompt-based JSON enforcement via OpenRouter. Alternative: switch to direct Z.AI API for structured output support.

---

## 3. Recommended Schema for Code Review Findings

Based on multi-model consensus (Grok, GLM-5, Kimi K2.5 all contributed), here is
the recommended schema. Design principles:

- **Flat array** of findings (not grouped by file) -- easier for consensus algorithms
- **Discrete confidence tiers** (not floats) -- models are poorly calibrated across providers
- **Line ranges** (not single line) -- models report different lines for the same issue
- **Code snippet** included -- ground truth for deduplication when line numbers drift
- **Optional suggested_fix** -- valuable but isolated from findings assessment
- **Typed references** -- CWE/CVE IDs enable exact-match consensus signals

### Schema (JSON Schema Draft 2020-12)

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "required": ["findings", "summary"],
  "additionalProperties": false,
  "properties": {
    "findings": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["severity", "category", "scope", "description", "confidence"],
        "additionalProperties": false,
        "properties": {
          "severity": {
            "enum": ["critical", "high", "medium", "low", "info"]
          },
          "category": {
            "enum": ["bug", "security", "performance", "style", "logic", "concurrency", "memory", "architecture"]
          },
          "scope": {
            "enum": ["line", "range", "function", "file", "module", "architecture"],
            "description": "Granularity of the finding location"
          },
          "file": {
            "type": ["string", "null"],
            "description": "File path, null for architectural/project-level findings"
          },
          "start_line": {
            "type": ["integer", "null"],
            "minimum": 1,
            "description": "Start line of the finding"
          },
          "end_line": {
            "type": ["integer", "null"],
            "description": "End line (same as start_line for single-line findings)"
          },
          "code_snippet": {
            "type": ["string", "null"],
            "description": "The actual code being flagged (2-5 lines). Critical for deduplication."
          },
          "description": {
            "type": "string",
            "description": "What the issue is"
          },
          "reasoning": {
            "type": ["string", "null"],
            "description": "Why this is an issue (separate from description for consensus analysis)"
          },
          "confidence": {
            "enum": ["high", "medium", "low"],
            "description": "Discrete tiers, not floats. Models are poorly calibrated across providers."
          },
          "rule_id": {
            "type": ["string", "null"],
            "description": "Canonical identifier (CWE-476, RUST-BORROW-001) for exact-match consensus"
          },
          "suggested_fix": {
            "type": ["object", "null"],
            "properties": {
              "description": { "type": "string" },
              "before": { "type": "string" },
              "after": { "type": "string" }
            },
            "additionalProperties": false
          },
          "references": {
            "type": ["array", "null"],
            "items": {
              "type": "object",
              "required": ["type"],
              "properties": {
                "type": { "enum": ["cwe", "cve", "owasp", "docs", "internal"] },
                "id": { "type": ["string", "null"] },
                "url": { "type": ["string", "null"] }
              },
              "additionalProperties": false
            }
          }
        }
      }
    },
    "summary": {
      "type": "string",
      "description": "Overall assessment of the code under review"
    }
  }
}
```

### Why These Design Choices

| Decision | Rationale |
|----------|-----------|
| Flat array | Consensus algorithms need to sort/filter across entire changeset. Grouped-by-file forces flattening anyway. |
| `scope` field | Discriminated location type. Architectural findings don't need file/line. |
| `start_line` + `end_line` | Models report different lines for the same issue. Ranges allow +/-3 line tolerance in consensus. |
| `code_snippet` | Ground truth. When line numbers drift, snippet text is the dedup anchor. |
| Discrete `confidence` | Float 0-1 gives false precision. GPT's 0.8 != Claude's 0.8. Discrete tiers are cross-model comparable. |
| `reasoning` separate from `description` | "What" vs "why". Similar reasoning is a stronger consensus signal than similar descriptions. |
| `rule_id` | CWE-476 from 3 models = strong consensus. Free-text matching is fuzzy. |
| `suggested_fix` optional | Fixes consume 30-50% of output tokens. Make optional; run heavy schema only for consensus findings. |
| `additionalProperties: false` | Catch model hallucinations. Pair with fallback parser for malformed JSON. |

---

## 4. Consensus Detection Strategy

The schema enables a multi-tier deduplication pipeline:

### Tier 1: Exact Match (cheapest)
- Match on `file` + `start_line` (within +/-3 tolerance) + `category`
- If `rule_id` present and matches across models, strong consensus

### Tier 2: Snippet Match
- Normalize `code_snippet` (strip whitespace, lowercase)
- Hash or compare snippets for same-code detection even when line numbers differ

### Tier 3: Semantic Match (most expensive)
- Embed `description` + `reasoning` with sentence-transformers
- Cosine similarity > 0.85 + shared category = likely same finding
- Use `rationale` overlap as additional signal

### Consensus Scoring
```
consensus_score = models_agreeing / total_models
severity_vote = mode(severities from agreeing models)
confidence_boost = "high" if consensus_score >= 0.6 else keep original
```

---

## 5. Implementation Path for Squall

### Phase 1: Prompt-Based (Works Today)
- Add the JSON Schema to `system_prompt` for review tool
- Instruct all models: "Respond ONLY with valid JSON matching this schema"
- Validate responses client-side with `serde_json` + custom validation
- Fallback: if JSON parsing fails, wrap raw text as single finding with `confidence: "low"`

### Phase 2: Provider-Enforced (Where Available)
- Pass `response_format: { type: "json_schema", ... }` to xAI and OpenRouter (Kimi)
- Pass `--output-schema schema.json` to Codex CLI
- Keep prompt-based enforcement for Gemini CLI and GLM-5 (OpenRouter)

### Phase 3: Consensus Layer
- Build post-processing in Rust: parse all model outputs, run Tier 1-3 dedup
- Produce `ConsensusReport` with grouped findings, agreement ratios, severity votes
- Write to `.squall/reviews/` as structured JSON alongside raw model outputs

### Schema Versioning
- Include `schema_version: "1.0"` in all outputs
- Squall can evolve the schema while remaining backwards-compatible

---

## 6. Tradeoffs and Risks

| Tradeoff | Impact | Mitigation |
|----------|--------|------------|
| Token overhead from structured schema | +15-30% output tokens | Use "light" schema for initial pass, "heavy" for consensus findings only |
| GLM-5 lacks `response_format` on OpenRouter | ~10-20% JSON parse failure rate with prompt-only enforcement | Consider direct Z.AI API, or accept graceful degradation |
| Models hallucinate confidence/severity | Miscalibrated consensus | Per-model calibration curves from historical accuracy data |
| Deeply nested schemas rejected by some models | Parse failures on Gemini | Keep max depth <= 3 levels |
| `additionalProperties: false` too strict | Rejects valid but extra fields | Use strict mode + fallback permissive parser |
| Code snippets consume tokens | Output bloat | Truncate to 3-5 lines, make optional in light mode |

---

## Sources

- [xAI Structured Outputs](https://docs.x.ai/developers/model-capabilities/text/structured-outputs)
- [Gemini API Structured Output](https://ai.google.dev/gemini-api/docs/structured-output)
- [OpenAI Structured Outputs](https://platform.openai.com/docs/guides/structured-outputs)
- [OpenRouter Structured Outputs](https://openrouter.ai/docs/guides/features/structured-outputs)
- [Codex CLI --output-schema](https://developers.openai.com/codex/noninteractive/)
- [Z.AI Structured Output](https://docs.z.ai/guides/capabilities/struct-output)
- [Kimi K2.5 on OpenRouter](https://openrouter.ai/moonshotai/kimi-k2.5) -- confirms response_format support
- [GLM-5 on OpenRouter](https://openrouter.ai/z-ai/glm-5) -- confirms NO response_format support
