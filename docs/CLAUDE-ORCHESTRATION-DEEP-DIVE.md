# Claude Code Orchestration: Subagents, Teams, Swarms & Context

**Date:** 2026-02-19
**Source:** Claude Code .jsonl conversation logs, official docs, web research
**Scope:** 1,152 sessions with subagents, 3,522 Task tool invocations, 5.66 GB of conversation data

---

## Part 1: The Subagent System (Task Tool)

### How It Works

When Claude Code spawns a subagent via the Task tool, it creates a **fully isolated agent session** with its own context window. The subagent:

- Gets a fresh context containing **only** its prompt + system prompt + tool definitions (~16K tokens)
- Does **NOT** inherit the parent's conversation history
- Runs its own tool calls (Read, Grep, Bash, etc.) independently
- Returns only its **final text output** to the parent (typically 10-15K chars)
- Can be resumed later via its `agentId`

The parent receives the result as a `tool_result` message plus a small metadata footer with token usage and duration.

### Subagent Types

| Type | Tools Available | Purpose |
|------|----------------|---------|
| `general-purpose` | All tools (Read, Write, Edit, Bash, Glob, Grep, WebFetch, WebSearch, Task, etc.) | Broad research, multi-step tasks |
| `Bash` | Bash only | Shell command execution |
| `Plan` | All except Task, Edit, Write, NotebookEdit, ExitPlanMode | Architecture planning, implementation design |
| `claude-code-guide` | Glob, Grep, Read, WebFetch, WebSearch | Documentation/guidance lookup |
| `statusline-setup` | Read, Edit | Status line configuration |

### Model Tiers

| Model | Token | Use Case |
|-------|-------|----------|
| `opus` | Highest cost | P0 bug review, complex analysis, judgment calls |
| `sonnet` (default) | Medium cost | Exploration, file reading, general tasks |
| `haiku` | Lowest cost | Quick lookups, config finding, API research |

### Execution Modes

- **Foreground (default):** Parent blocks until subagent completes. Result returned inline.
- **Background (`run_in_background: true`):** Parent continues working. Result written to an output file. Parent polls via `TaskOutput` or `Read`.
- **Resumable:** Any subagent can be resumed via its `agentId`, continuing with full prior context preserved.

---

## Part 2: How David Uses Subagents (Empirical Data)

### Scale

- **3,522** Task tool invocations across 29 human projects + 267 ORI automated sessions
- **1,152** sessions contain subagents
- Average **4.9 subagents per session**

### Type Distribution

| Type | Count | % | Role |
|------|-------|---|------|
| `Explore` | 1,665 | 47% | Read-only codebase investigation — the workhorse |
| `general-purpose` | 1,542 | 43% | PR reviews, API research, doc auditing |
| `Bash` | 127 | 3% | Shell command delegation |
| `consensus-orchestrator` | 92 | 2% | Meta-subagent spawning model queries (early pattern, now deprecated) |
| `Plan` | 62 | 1% | Architecture/implementation planning |
| `claude-code-guide` | 22 | <1% | Documentation lookup |
| `code-reviewer` | 12 | <1% | Focused diff review |

### Model Selection

| Model | Count | % | Pattern |
|-------|-------|---|---------|
| default (Sonnet) | 2,701 | 76% | Exploration, file reading, general tasks |
| opus | 579 | 16% | P0 bug review, complex analysis |
| sonnet (explicit) | 150 | 4% | Mid-tier research |
| haiku | 92 | 2% | Quick lookups |

### Prompt Patterns

Analysis of 286 subagent prompts reveals:

| Pattern | Prevalence |
|---------|-----------|
| Numbered step lists (1. 2. 3.) | 95% |
| Absolute file paths included | 69% |
| "Be thorough" directive | 19% |
| "Do NOT use" constraints | 12% |
| Structured "Key context:" section | 9% |

**Prompt length:** Median 1,138 chars, mean 1,290 chars, max 5,629 chars. David never gives terse subagent instructions — every prompt has clear steps and context.

### The 10-15x Return Ratio

| Subagent Purpose | Prompt Size | Return Size | Internal Tokens Used |
|---|---|---|---|
| Issue investigation | ~800 chars | ~10-14K chars | ~45-60K tokens |
| Security review | ~876 chars | ~11K chars | ~52K tokens |
| Logical consistency check | ~832 chars | ~9K chars | ~52K tokens |

A ~800 char prompt produces ~10-14K chars of output after the subagent reads files, searches code, and synthesizes findings internally. The parent only sees the final result — all intermediate tool calls stay in the subagent's context.

### Parallelism Reality

**Surprising finding:** Zero true parallel Task dispatches observed in the JSONL data (multiple Task tool_use blocks in one assistant message). All Task calls were dispatched one per message.

However, the `/consensus` slash command achieves **hybrid parallelism**: 1 Task call (for Claude Opus) + 5 `mcp__zen__chat`/`mcp__zen__clink` calls in a single message. The parallelism is between MCP tools and Task, not between multiple Tasks.

The `/deep-review` command requests "all 5 in parallel" but the data shows sequential dispatch. This may be a Claude Code limitation or a model behavior pattern.

---

## Part 3: Agent Teams (Swarms) — Feb 2026

### What It Is

Agent Teams transform Claude Code from a single-agent assistant into a **multi-agent orchestration system**. A lead agent coordinates multiple teammates, each running as a fully independent Claude Code instance with its own context window.

Enable with:
```json
// settings.json
{
  "env": {
    "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS": "1"
  }
}
```

### Architecture

| Component | Role |
|-----------|------|
| **Team Lead** | The main Claude Code session. Creates team, spawns teammates, coordinates, synthesizes |
| **Teammates** | Separate Claude Code instances, each with own context window |
| **Task List** | Shared work items stored as JSON files at `~/.claude/tasks/{team-name}/` |
| **Mailbox** | Inter-agent messaging for direct peer-to-peer communication |

### The Seven Coordination Primitives

1. **TeamCreate** — Initialize shared namespace
2. **TaskCreate** — Define work units as JSON files on disk
3. **TaskUpdate** — Claim/complete work (file-locking prevents races)
4. **TaskList** — Poll available tasks (self-scheduling)
5. **Task(team_name)** — Spawn a teammate with team awareness
6. **SendMessage** — Direct inter-agent communication (unicast or broadcast)
7. **TeamDelete** — Cleanup after shutdown

### Lifecycle

1. You describe a task and team structure in natural language
2. Lead creates team, spawns teammates, creates task list with dependencies
3. Teammates self-claim pending tasks (file-lock-based claiming prevents races)
4. Teammates work independently, communicate via messaging
5. Blocking task completions automatically unblock downstream tasks
6. Lead synthesizes findings and presents results
7. You ask lead to shut down teammates and clean up

### Display Modes

- **In-process** (default): All teammates in main terminal. `Shift+Down` cycles between them.
- **Split panes**: Each teammate gets its own tmux or iTerm2 pane.

### Critical Design: No Shared Memory

**There is no shared context between agents.** Each teammate gets:
- Its own isolated context window
- The same CLAUDE.md, MCP servers, and skills (loaded fresh)
- A spawn prompt from the lead describing its role
- Access to the shared task list on disk

**The lead's conversation history does NOT carry over to teammates.**

The only coordination channels are:
1. Task files on disk (`~/.claude/tasks/{team-name}/`)
2. SendMessage (the mailbox)
3. Automatic idle notifications

This isolation is deliberate: **LLMs perform worse as context expands**. Narrow scope + clean context = better reasoning per agent.

---

## Part 4: Subagents vs. Teams — When to Use What

| Dimension | Subagents (Task Tool) | Agent Teams |
|-----------|----------------------|-------------|
| **Communication** | Unidirectional — report back only | Bidirectional — peer messaging |
| **Coordination** | Parent orchestrates (hub-and-spoke) | Shared task list + self-claiming |
| **Context** | Own window; results summarized to parent | Fully independent; task files bridge state |
| **Nesting** | Cannot spawn other subagents | Cannot spawn nested teams |
| **Token Cost** | Lower — only final result flows back | Higher — each teammate is a full instance |
| **Best For** | Focused tasks where only the result matters | Complex work needing discussion/collaboration |
| **Lifecycle** | Created and destroyed within a session | Persistent across team's lifetime |
| **Inter-agent Talk** | Cannot talk to each other | Can message each other directly |

### Decision Guide

- **Single session**: Sequential tasks, same-file edits, many dependencies
- **Subagents**: Quick focused workers, parallel research, verbose output isolation, test running
- **Agent Teams**: Sustained parallelism exceeding a single context window, adversarial debate, cross-layer features requiring coordination

---

## Part 5: Custom Subagents (.claude/agents/)

Custom subagents (defined as `.claude/agents/*.md` files) gained significant capabilities in Feb 2026:

### Configuration Options (YAML Frontmatter)

```yaml
---
name: security-reviewer
model: opus
permission_mode: acceptEdits
tools:
  allowed: [Read, Grep, Glob, Bash]
  disallowed: [Write, Edit]
max_turns: 25
memory: project  # Persistent across sessions
skills:
  - security-checklist
hooks:
  PreToolUse: ./hooks/validate-tool.sh
  Stop: ./hooks/on-complete.sh
---

You are a security-focused code reviewer. Focus on OWASP Top 10...
```

### Key Features

| Feature | Description |
|---------|-------------|
| **Model selection** | `haiku`, `sonnet`, `opus`, or `inherit` from parent |
| **Permission modes** | `default`, `acceptEdits`, `dontAsk`, `bypassPermissions`, `plan` |
| **Tool restrictions** | Allow/disallow specific tools per agent |
| **Persistent memory** | `user` (global), `project` (repo-specific, committable), `local` (gitignored) |
| **Skills injection** | Preload domain knowledge |
| **MCP server access** | Same MCP servers as parent |
| **Max turns** | Limit agent runtime |
| **Lifecycle hooks** | `PreToolUse`, `PostToolUse`, `Stop` |

### Persistent Memory

Memory is stored as `MEMORY.md` — on startup, the first 200 lines are injected into the agent's system prompt. Scopes:
- `user`: All projects (`~/.claude/agent-memory/`)
- `project`: Repo-specific, checkable into git (`.claude/agent-memory/`)
- `local`: Repo-specific, gitignored (`.claude/agent-memory-local/`)

This means a code-reviewer subagent can accumulate knowledge about recurring patterns and conventions across sessions.

---

## Part 6: Context Passing — The Full Picture

### What Subagents Receive

From analysis of 1,152 sessions' JSONL files:

```
Subagent initial context:
  System prompt + tool definitions:  ~14K tokens (fixed overhead)
  User prompt (from Task tool):      ~1-2K tokens (variable)
  Total start:                       ~16K tokens

Parent at same point:                ~50-200K tokens
```

**The subagent starts at ~16K, not the parent's 50-200K.** This is the key efficiency gain.

### What Flows Back

Only the final text output + metadata:
```
Subagent return to parent:
  Final text result:     ~10-15K chars
  Metadata (agentId):    ~50 chars
  Token usage stats:     ~30 chars

NOT returned:
  - Intermediate tool calls
  - Files read by the subagent
  - Internal reasoning
  - Tool results (file contents, grep output, etc.)
```

### Context Growth During Subagent Execution

Observed token growth for a code review subagent:
```
Turn  1:  15,960 tokens  (initial prompt + system)
Turn 15:  91,757 tokens  (after reading diff files)
Turn 22:  95,409 tokens  (after reading source files)
Turn 40:  99,570 tokens  (accumulating findings)
Turn 58: 102,566 tokens  (final turn)
```

Starts lean, grows only as needed through its own tool use.

### Context Compaction

When conversations exceed ~175K tokens, automatic compaction kicks in:

1. A `compact_boundary` system message is written with `"preTokens": 175327`
2. A compact subagent (`agent-acompact-*`) is spawned
3. Prompt: "Create a detailed summary of the conversation so far, paying close attention to the user's explicit requests and your previous actions"
4. The summary (~15-25K chars) replaces the old conversation
5. New messages prefixed: "This session is being continued from a previous conversation that ran out of context..."

**529 compact agent files** found across the system. Compaction files are small: typically 2-3 lines, 15-60KB.

### Storage Analysis

| Metric | Value |
|--------|-------|
| Total subagent storage | 2.01 GB |
| Total parent storage (for sessions with subagents) | 3.65 GB |
| Combined | 5.66 GB |
| Average subagents per session | 4.9 |
| Typical size ratio (subagent/parent) | 0.1x to 0.5x |

### The bash_progress Bloat Problem

The single biggest storage issue: `bash_progress` entries in JSONL files.

The worst case: a 780MB subagent file where:
- **509 progress entries: 774.9 MB (99.5% of file)**
- 41 user entries: 0.2 MB
- 55 assistant entries: 0.1 MB

Each `bash_progress` entry stores both `output` (streaming partial) and `fullOutput` (cumulative). A single progress line can reach **10.2 MB** because `fullOutput` stores the entire accumulated bash output up to that point. With 495 entries, this is massive duplication.

The two largest subagent files alone (780MB + 467MB = 1.25GB) account for **62% of all subagent storage**, and are 99.5% bash_progress data.

For typical subagents (not doing large bash operations):
- user: 67.8%
- assistant: 27.3%
- progress: 4.9%

---

## Part 7: How David's Slash Commands Orchestrate Context

### /consensus — Hybrid Parallelism

The `/consensus` command achieves context efficiency by:
1. Each model gets the **same core query** — just the user's question + file paths
2. No parent history duplication — models start fresh
3. Claude Opus gets a Task agent (subagent); others get MCP tool calls
4. File paths are passed via `absolute_file_paths` parameter — models read files themselves
5. Results synthesized by the parent into Agreement/Divergence/Recommendation

**Failure mode observed:** Kimi K2.5 rejected a file set (138,579 tokens > 57,600 limit), showing that MCP tools have their own context limits independent of the parent.

### /deep-review — 5 Independent Lenses

Each of the 5 Task agents gets:
- A focused prompt (~800-1000 chars) describing its specific lens
- File paths to read independently
- No parent history — each agent discovers what it needs from the codebase

This is maximally context-efficient: 5 agents each starting at ~16K tokens instead of one agent trying to hold 5 perspectives in a single 200K context.

---

## Part 8: Efficiency Summary

### Where Context Passing Is Efficient

| Pattern | Why It Works |
|---------|-------------|
| Subagent isolation | Starts at ~16K tokens, not parent's 50-200K |
| Return compression | Only final text (~10-15K chars) flows back, not internal tool calls |
| Compaction | 175K tokens → 15-25K char summary enables continuation |
| Independent discovery | Consensus/deep-review agents read files themselves, no context duplication |
| Model tiering | Haiku for lookups, Sonnet for research, Opus for judgment |

### Where Context Gets Lost

| Problem | Impact |
|---------|--------|
| Compaction discards detail | 175K tokens → 15K chars. Nuance and intermediate reasoning lost |
| Subagents can't see parent history | Must explicitly include all needed context in prompt |
| No shared memory between subagents | Each re-discovers the same codebase independently |
| Cross-tool context gap | MCP tools (zen) can't access Claude's conversation context |

### Where Context Gets Bloated

| Problem | Impact |
|---------|--------|
| bash_progress entries | Single command can create 780MB of cumulative output duplication |
| Tool results stored verbatim | Large file reads inflate JSONL (entire file as tool_result) |
| 5.66 GB total storage | For 1,152 sessions with subagents |
| Two worst files = 62% of storage | Both are 99.5% bash_progress data |

---

## Part 9: Recommendations for David's Workflow

### What Would Help Most

1. **True parallel Task dispatch** — If Claude Code supported multiple Task tool calls in a single message (like it does for MCP tools), the /deep-review 5-lens pattern would run in genuine parallel instead of sequential. This appears to be a current limitation.

2. **Shared file cache between subagents** — When 5 deep-review agents all need to read the same files, they each independently read them. A shared file cache (even just a temp directory of pre-read files) would eliminate redundant I/O.

3. **Agent Teams for consensus** — Instead of the current slash command (which fights PAL's sequential blocking), a proper Agent Team could have 6 teammates each querying a different model truly in parallel, with the lead synthesizing.

4. **Persistent memory for reviewer agents** — A `security-reviewer` custom subagent with `memory: project` would accumulate knowledge about your codebase's patterns across sessions, making each review more context-aware.

5. **bash_progress cleanup** — A post-session hook that strips `fullOutput` from bash_progress entries (keeping only `output`) would recover ~1.25 GB immediately and prevent future bloat.

### What Already Works Well

- The 10-15x return ratio (800 char prompt → 10K char result) is excellent context efficiency
- Model tiering (haiku/sonnet/opus) optimizes cost vs quality appropriately
- The `/consensus` hybrid parallelism (1 Task + 5 MCP tools) is a clever workaround for current limitations
- Compaction at 175K tokens enables indefinitely long sessions
- Structured numbered prompts with file paths give subagents clear execution plans

---

## Appendix: Recent Claude Code Releases (Jan-Feb 2026)

### Agent Teams Fixes (v2.1.45-47, Feb 17-18)
- Fixed teammates failing on Bedrock, Vertex, and Foundry (env var propagation to tmux)
- Fixed Task tool (background subagents) crashing with `ReferenceError` on completion
- Fixed concurrent agent API errors ("thinking blocks cannot be modified")
- Fixed background agent results returning raw transcript instead of final answers
- Improved memory usage by releasing API stream buffers after use
- Fixed skills invoked by subagents leaking into main session after compaction

### Custom Subagent Enhancements (~v2.1.33, Feb 2026)
- Persistent memory (`MEMORY.md`) with user/project/local scopes
- Lifecycle hooks (PreToolUse, PostToolUse, Stop)
- Tool allow/disallow lists
- Max turns limits

### Agent SDK (Renamed from Claude Code SDK)
- Available as `pip install claude-agent-sdk` and `npm install @anthropic-ai/claude-agent-sdk`
- Same tools, agent loop, and context management as Claude Code, but programmatic
- `query()` API for single-turn, `session()` for multi-turn
- Deep research is now a first-class use case
