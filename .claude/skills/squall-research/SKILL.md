---
name: squall-research
description: "Multi-agent research swarm via teams. Use when asked to 'research', 'squall research', 'investigate topic', 'deep dive on', or when broad multi-vector research is needed. Spawns parallel agents each using WebSearch + Squall review. (project)"
one_liner: "N vectors x M models -- research at the speed of parallel agents."
activation_triggers:
  - "squall research"
  - "research this"
  - "investigate topic"
  - "deep dive on"
  - "research swarm"
  - When user wants multi-vector research on a topic
  - When user wants parallel investigation of a question
related_skills:
  - "[squall-review](../squall-review/SKILL.md) (for code review, not research)"
  - "[squall-deep-research](../squall-deep-research/SKILL.md) (for single-agent deep research)"
---

# Squall Research

> **TL;DR - QUICK REFERENCE**
>
> | Concept | Translation |
> |---------|-------------|
> | Research swarm | Team of 3-5 agents each investigating one angle of a topic |
> | Vector | One independent research angle/facet of the topic |
> | Fan-out | Each agent uses WebSearch + Squall `review` (3 models) in parallel |
> | Synthesis | Team lead reads all reports and combines into coherent answer |
>
> **Entry criteria:**
> - User wants broad research on a topic (not code review)
> - Topic has 3+ independent angles worth investigating
> - Depth matters more than speed (this spawns real agents)

Multi-agent research swarm that decomposes a topic into independent vectors, assigns each to
a parallel agent, and combines their findings. Each agent amplifies its own research with
WebSearch and Squall's multi-model `review` tool, producing N vectors x M models worth of
perspectives.

Proven pattern from 2026-02-21: 5 agents researched ensemble methods, multi-agent debate,
LLM routing, structured output, and feedback loops -- all in parallel, all writing to disk,
all completed within minutes.

## Workflow

```
┌─────────────────────────────────────────────────────────────────┐
│                     SQUALL RESEARCH SWARM                       │
│                                                                 │
│  1. DECOMPOSE          Topic ──► 3-5 independent vectors        │
│                                                                 │
│  2. SPAWN              TeamCreate + TaskCreate + Teammates       │
│                              │                                  │
│                   ┌──────────┼──────────┐                       │
│                   ▼          ▼          ▼                        │
│  3. RESEARCH    Agent A    Agent B    Agent C    (parallel)      │
│                   │          │          │                        │
│              WebSearch   WebSearch  WebSearch                    │
│                 +            +          +                        │
│              Squall       Squall     Squall                     │
│              review       review     review                     │
│                 │            │          │                        │
│                 ▼            ▼          ▼                        │
│  4. WRITE    report-a.md report-b.md report-c.md                │
│              (.squall/research/)                                 │
│                   │          │          │                        │
│                   └──────────┼──────────┘                       │
│                              ▼                                  │
│  5. SYNTHESIZE     Team lead reads all reports                  │
│                              │                                  │
│  6. CLEANUP        Shutdown teammates + TeamDelete              │
│                              │                                  │
│  7. OUTPUT         .squall/research/ directory + synthesis       │
└─────────────────────────────────────────────────────────────────┘
```

### Step 1: Decompose the Topic

Break the research topic into 3-5 **independent** vectors. Each vector should be:
- Researchable on its own (no dependency on another vector's findings)
- Distinct enough that two agents won't return the same results
- Specific enough to guide WebSearch queries

Example for "How do production systems aggregate multi-model LLM output?":
1. ML ensemble weighting methods (Condorcet, stacking, mixture of experts)
2. Multi-agent debate protocols (academic papers, when debate helps vs hurts)
3. LLM routing strategies (RouteLLM, Martian, rule-based approaches)
4. Structured output schemas (JSON enforcement, provider support matrix)
5. Feedback loop architectures (CodeRabbit, Copilot, SonarQube patterns)

### Step 2: Create the Team

```
TeamCreate:
  name: "squall-research-<topic-slug>"
  description: "Research swarm for <topic>"
```

Then create one task per vector:

```
TaskCreate:
  subject: "Research: <vector name>"
  description: |
    Investigate <specific angle>. Focus on:
    - <key question 1>
    - <key question 2>
    Write findings to: .squall/research/<vector-slug>.md
  activeForm: "Researching <vector name>"
```

### Step 3: Spawn Teammates

Spawn one `general-purpose` agent per vector, **all in parallel**. Each agent's prompt must include:

1. **What to research** (their specific vector)
2. **ToolSearch instructions** for loading Squall MCP tools
3. **Exact model names** for the Squall review call
4. **Output file path** where they must write their report

Agent prompt template:

```
You are a research agent investigating: <VECTOR DESCRIPTION>

Your workflow:
1. TaskList to find your task, then TaskUpdate it to in_progress
2. Use WebSearch to find papers, docs, blog posts, implementations
3. Use ToolSearch with query "squall" to load Squall MCP tools
3a. Call `listmodels` to verify exact current model names
4. Use the Squall `review` tool to get multi-model perspectives:
   - models: ["grok-4-1-fast-reasoning", "moonshotai/kimi-k2.5", "z-ai/glm-5"]
   - system_prompt: "You are a research advisor specializing in <VECTOR TOPIC>.
     Analyze the research findings provided and add your own knowledge.
     Cite sources where possible."
   - prompt: <paste your WebSearch findings as the prompt>
5. Write your complete findings to:
   .squall/research/<FILENAME>.md  (relative to working directory)
   Format: markdown with sources cited, key findings highlighted
6. TaskUpdate your task to completed
7. SendMessage to "team-lead" with a 2-3 sentence summary
```

### Step 4: Wait and Monitor

The team lead waits for all agents to complete. Monitor via:
- TaskList to check task statuses
- Incoming SendMessage notifications from agents

### Step 5: Synthesize

Once all agents report back:
1. Read all output files from `.squall/research/`
2. Identify themes that span multiple vectors
3. Note contradictions between agents/models
4. Produce a unified summary for the user

### Step 6: Cleanup

```
- SendMessage type: shutdown_request to each teammate
- TeamDelete the team
```

The `.squall/research/` files persist for future reference.

## Key Principles

### Principle 1: Vectors Must Be Independent

If vector B depends on vector A's results, they aren't independent -- merge them or
sequence them. Independence enables parallelism, which is the whole point.

### Principle 2: Teammates Have Full MCP Access

Confirmed 2026-02-21: agents spawned with `general-purpose` subagent_type inherit all MCP
tool access. They can use ToolSearch to discover and load Squall tools, then call `review`,
`chat`, `clink`, and `listmodels` directly.

### Principle 3: Disk Is the Integration Layer

Agents write to `.squall/research/`. This survives context compaction, agent shutdown, and
session restarts. The team lead reads files, not message history.

### Principle 4: 3 Models Is the Sweet Spot for Research

The review call uses Grok (fast, broad), Kimi (contrarian), and GLM (architectural framing).
This gives diverse perspectives without the latency cost of Gemini/Codex (which are better
for code review than research advising).

## Reference Tables

### Model Selection for Research Review

| Model | Strength | Speed | Use For |
|-------|----------|-------|---------|
| `grok-4-1-fast-reasoning` | Fast, broad knowledge | 20-65s | Always include |
| `moonshotai/kimi-k2.5` | Contrarian, edge cases | 60-300s | Alternative perspectives |
| `z-ai/glm-5` | Architectural framing | 75-93s | Big-picture structure |

### Agent Count Guidelines

| Topic Breadth | Agents | Example |
|---------------|--------|---------|
| Narrow (1-2 angles) | Use `/deep-research` instead | "How does RouteLLM work?" |
| Medium (3 angles) | 3 | "Compare 3 specific approaches" |
| Broad (4-5 angles) | 4-5 | "Survey an entire problem space" |
| Very broad | 5 max, decompose topic first | "State of AI in 2026" |

## Anti-Patterns

| Don't | Do Instead |
|-------|------------|
| Spawn more than 5 agents | Cap at 5; diminishing returns, resource heavy |
| Research overlapping vectors | Ensure each vector is clearly distinct |
| Forget to TeamDelete after | Always clean up; orphan processes waste resources |
| Skip the Squall review step | 3 model perspectives >> WebSearch alone |
| Have team lead duplicate agent work | Team lead synthesizes, agents research |
| Use Gemini/Codex for research review | Use Grok/Kimi/GLM (faster, better for advising) |
| Put all findings in messages only | Write to disk; messages don't survive compaction |
| Use this for narrow single-angle questions | Use `/deep-research` for single-vector depth |

## Examples

### Example 1: Researching Multi-Model Aggregation Strategies

```
User: "squall research how production systems aggregate multi-model output"

Decomposition:
  Vector 1: ML ensemble weighting (Condorcet, stacking, MoE)
  Vector 2: Multi-agent debate (academic papers, effectiveness)
  Vector 3: LLM routing (RouteLLM, Martian, rule-based)
  Vector 4: Structured output schemas (JSON enforcement)
  Vector 5: Feedback loops (CodeRabbit, Copilot patterns)

Team: "squall-research-aggregation" with 5 agents
Output: .squall/research/{ensemble-methods,multi-agent-debate,llm-routing,
        structured-output,feedback-loops}.md
```

This was the actual first run of this pattern (2026-02-21). All 5 agents completed
successfully, producing detailed reports with cited sources and multi-model perspectives.

### Example 2: Researching a Narrower Topic

```
User: "research the state of WebAssembly for server-side AI inference"

Decomposition:
  Vector 1: WASM runtime performance for ML (wasmtime, wasmer benchmarks)
  Vector 2: WASM-compatible ML frameworks (ONNX, TFLite, custom)
  Vector 3: Production deployments (Fastly, Cloudflare, Fermyon case studies)

Team: "squall-research-wasm-ai" with 3 agents
Output: .squall/research/{wasm-runtimes,wasm-ml-frameworks,wasm-deployments}.md
```

Three vectors is enough for a focused topic. No need to force 5 agents.

## Related Skills

- [squall-review](../squall-review/SKILL.md) - Code review via Squall (different use case: code, not research)
- [squall-deep-research](../squall-deep-research/SKILL.md) - Single-agent deep research via Codex CLI (for narrow topics, no team needed)

<!-- SENTINEL:SESSION_LEARNINGS_START - Do not remove this line -->
## Session Learnings Archive

Insights captured during skill execution:

### 2026-02-21: First Research Swarm Validation
**Origin**: Five-agent research swarm run on multi-model aggregation topic
**Core insight**: Teammates inherit MCP tool access via ToolSearch. The pattern works end-to-end: TeamCreate, parallel agents with WebSearch + Squall review, disk-based output, team lead synthesis.
**Harvested** -> Encoded as the core workflow in this skill

---
<!-- SENTINEL:SESSION_LEARNINGS_END -->
