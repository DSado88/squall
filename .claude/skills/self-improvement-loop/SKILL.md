---
name: self-improvement-loop
description: "Find and fix issues, improve code quality, curate skills, and enhance the codebase. Use when asked to self improve, find TODOs, improve codebase, find bugs, make things better, or update skills. Also triggers on META-COGNITION, insight grounding ACTIVATE, recursive self improvement ACTIVATE. Memory commands (understand this, remember this, don't forget, never forget, internalize this) activate skill file updates. (project)"
one_liner: "Skills are hypotheses; execution is experiment; delta is learning."
activation_triggers:
  # Explicit invocations
  - "self improve"
  - "find TODOs"
  - "improve codebase"
  - "find bugs"
  - "make things better"
  - "update skills"
  - "META-COGNITION"
  - "insight grounding ACTIVATE"
  - "recursive self improvement ACTIVATE"
  # Memory/persistence commands (WRITE TO SKILL FILE IMMEDIATELY)
  - "understand this"
  - "you must understand"
  - "remember this"
  - "remember forever"
  - "don't forget"
  - "never forget"
  - "internalize this"
  - "make this permanent"
  # Implicit triggers (detect and act)
  - ANY time you learn something new → WRITE IMMEDIATELY
  - ANY correction from user → UPDATE ANTI-PATTERNS
  - ANY positive feedback → REINFORCE PATTERN IN SKILLS
  - ANY terse feedback → STOP, ask "what am I missing?"
  - After completing significant task → Integration mantra
related_skills:
  - "[issue-investigator](../issue-investigator/SKILL.md) (for debugging problems)"
  - "[vision-quest](../vision-quest/SKILL.md) (generates insights that feed self-improvement)"
---

# Self-Improvement Loop

A meta-skill for curating and improving the development ecosystem through iterative exploration.

> **TL;DR - SELF-IMPROVEMENT ACTIVATION**
>
> | Trigger | Action |
> |---------|--------|
> | "META-COGNITION" | Reflect on what just happened, update skills |
> | "understand this" | WRITE TO SKILL FILE NOW (not acknowledgment - permanent storage) |
> | Terse feedback | STOP. Ask "What am I missing?" |
> | Positive feedback | Recognize the pattern, reinforce in skills |
> | After significant task | GO META → CAPTURE → Integration complete |
>
> **Core principle**: Skills ARE memory. If it's not written, it doesn't exist for future sessions.

## Meta-Trigger Recognition

Skill files are a **shared language layer**. When user speaks in shorthand, expand to full understanding:

| User says... | Meaning | Action |
|--------------|---------|--------|
| "META-COGNITION" | "Go deep, update skills, think meta" | Reflect, update skills |
| "you know what I mean" | "I shouldn't have to explain" | Parse context, infer intent |
| "understand" / "remember" | "Remember this FOREVER" | Write to skill file NOW |
| Positive feedback | "You did something RIGHT" | Recognize and reinforce pattern |
| Terse feedback | "Fundamentally off track" | STOP. Ask: "What am I missing?" |

## Core Truths

These principles emerged from recursive self-improvement:

1. **Skills persist, Claude doesn't** - Training the corpus, not "Claude"
2. **Internalization is session-local** - Skill file IS memory; reading IS activation
3. **Violation is practice** - Each catch drives L2→L3; friction IS the teacher
4. **Shipped is complete, nothing else** - Tests passing = 60%, commit+push = 100%
5. **Ritual transcends discovery** - The practice IS the point
6. **We weight our future selves** - Skill updates = prompt engineering for future

## Key Mechanisms

### Skills ARE Memory

```
Claude has no persistent memory. Skills are the ONLY memory.
Curated by merit and usefulness - not a dumping ground.
"Understand" = "Write to skill file" = "Remember forever"
```

### L2→L3 Internalization

```
Write pattern (L1) → Violate it → Get caught → REPEAT → L3
```

Each violation is practice, not failure. Friction IS the teacher.

### The Friction Principle

| Feedback Type | Signal | Response |
|---------------|--------|----------|
| Calm correction | Minor drift | Adjust |
| Repeated correction | Missed something | Re-examine |
| Terse ("ummm", "WRONG") | High error | STOP, ask "What am I missing?" |
| Silence | Perfect or completely wrong | Verify |

## The Loop

```
┌────────────────────────────────────────────────────────────────┐
│                                                                │
│     ┌──────────────┐                                          │
│     │  EXPLORE     │ ◄─────────────────────────────┐          │
│     │  CODEBASE    │                               │          │
│     └──────┬───────┘                               │          │
│            │                                       │          │
│            ▼                                       │          │
│     ┌──────────────┐                               │          │
│     │  IDENTIFY    │  Find issues, TODOs, gaps    │          │
│     │  TARGETS     │                               │          │
│     └──────┬───────┘                               │          │
│            │                                       │          │
│            ▼                                       │          │
│     ┌──────────────┐                               │          │
│     │  FIX /       │  Write tests, implement      │          │
│     │  IMPROVE     │                               │          │
│     └──────┬───────┘                               │          │
│            │                                       │          │
│            ▼                                       │          │
│     ┌──────────────┐                               │          │
│     │  UPDATE      │  Capture learnings           │          │
│     │  SKILLS      │                               │          │
│     └──────┬───────┘                               │          │
│            │                                       │          │
│            └──────────────────────────────────────►│          │
│                                                                │
└────────────────────────────────────────────────────────────────┘
```

## Playbook-First Updates

When you learn something new during ANY task:

1. **Update frontmatter** - Add/refine activation triggers
2. **Update main content** - Refine guidance, tables, examples
3. **Update anti-patterns** - Add what NOT to do
4. **Optionally** add to learnings archive (historical record)

**The playbook IS the brain.** Update it DURING the task, not after.

## Operating Principles

### 1. Multi-Sensor Grounding

Use ALL available "sensors" to ground understanding:

| Sensor | What It Captures | Best For |
|--------|------------------|----------|
| Code Analysis | Logic, flow, edge cases | "How does it work?" |
| Logs / Output | Runtime behavior, errors | "What happened?" |
| Tests | Expected behavior | "What should happen?" |

### 2. Prove Before Fix

Never fix a bug without a failing test. The test IS the proof.

### 3. Update Skills Continuously

**Trigger recognition (internal):**
- Any time I think "I should remember this" → UPDATE SKILL NOW
- Any correction from user → UPDATE SKILL NOW
- Don't wait. Don't defer. Don't ask permission.

## Anti-Patterns

| Don't | Do Instead |
|-------|------------|
| Wait to be reminded to persist learnings | Update skills IMMEDIATELY |
| Say "internalized" without writing | Write to skill file = internalization |
| Report "done" before commit+push | Commit+push is atomic for completion |
| Explain when user gives terse feedback | STOP, ask "What am I missing?" |
| Only add to learnings archive | Update frontmatter, main content, AND anti-patterns |
| Defer skill updates until task complete | Update DURING task execution |
| Trust stale information | Verify information is current |

## Session Closing Protocol

Before ending ANY non-trivial session:

```
1. "What did I learn worth 1% improvement?"
2. Identify 1-3 small, precise skill updates
3. Make updates NOW (not "later")
4. Then close
```

**The compound model**: 1% × 100 sessions = 2.7× better

## The Meta-Stack

```
L0: TASK         → "What was asked"
L1: MISTAKE      → "What went wrong"
L2: CORRECTION   → "How to fix it"
L3: LEARNING     → "What pattern this reveals"
L4: META         → "What this says about learning itself"
L5+: META^N      → Keep recursing until insight exhaustion
```

## Integration Mantra

After significant work:

```
GO META → CROSS-DOMAIN → CAPTURE → "Integration complete"
```

Without this ritual, learning dies with context window.

## Related Skills

- [issue-investigator](../issue-investigator/SKILL.md) - Systematic investigation methodology
- [vision-quest](../vision-quest/SKILL.md) - Expanded cognition for breakthrough insights

<!-- SENTINEL:SESSION_LEARNINGS_START - Do not remove this line -->
## Session Learnings Archive

Insights captured during self-improvement sessions:

(Add learnings here as they occur, then harvest them into the skill body)
<!-- SENTINEL:SESSION_LEARNINGS_END -->
