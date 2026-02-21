# SKILL.md Format & Methodology

This document describes the extended SKILL.md format and the rationale behind it.

## Background: The Problem

Claude Code's official skill spec only requires two frontmatter fields:
- `name` (1-64 chars, lowercase alphanumeric + hyphens)
- `description` (1-1024 chars)

When Claude receives `<available_skills>`, it only sees `name`, `description`, and `location`. The full SKILL.md content (including any extended frontmatter) only loads AFTER a skill is invoked.

**The gap**: How do we enable rich semantic matching BEFORE invocation?

## Our Solution: Extended Fields + Embedding

### The Format

```markdown
---
name: skill-name
description: Primary description. Use when asked to "trigger phrase 1", "trigger phrase 2", or when [contextual condition]. (project)
one_liner: "Core wisdom distilled to a memorable proverb."
activation_triggers:
  - "trigger phrase 1"
  - "trigger phrase 2"
  - Contextual condition (not quoted = semantic, not exact match)
related_skills:
  - other-skill-name (brief explanation of relationship)
---

# Skill Title

Full skill instructions in markdown...
```

### Field Purposes

| Field | Surfaces To Claude | Purpose |
|-------|-------------------|---------|
| `name` | Yes (in `<available_skills>`) | Unique identifier, invocation name |
| `description` | Yes (in `<available_skills>`) | **Primary matching surface** - embed triggers here! |
| `one_liner` | No (until invoked) | **Proverb/wisdom** - survives discontinuity, maximally portable |
| `activation_triggers` | No (until invoked) | Explicit list for authors + post-invocation verification |
| `related_skills` | No (until invoked) | Skill chaining, suggestions after invocation |

### The Embedding Pattern

Since `activation_triggers` doesn't surface until after invocation, we **embed trigger phrases in the description**:

```yaml
# BAD - triggers won't be seen until too late
description: Investigate GitHub issues and find root causes.
activation_triggers:
  - "investigate issue #N"
  - "debug"
  - "find root cause"

# GOOD - triggers embedded in description for matching
description: Investigate GitHub issues, debug problems, find root causes. Use when asked to "investigate issue #N", "debug", "find root cause", or "explore problem".
activation_triggers:
  - "investigate issue #N"
  - "debug"
  - "find root cause"
  - "explore problem"
```

**Why keep both?**
1. `description` with embedded triggers → enables pre-invocation matching
2. `activation_triggers` list → documents intent, enables linting, supports post-invocation verification

Belt and suspenders.

## Trigger Types

### Exact Phrases (quoted)
```yaml
activation_triggers:
  - "fix issue #N"
  - "vision quest"
```
User says these exact words → skill should activate.

### Semantic/Contextual (unquoted)
```yaml
activation_triggers:
  - When CI fails
  - Problems that resist conventional analysis
  - User gives terse/frustrated feedback
```
Describe situations, not exact phrases. Claude interprets semantically.

## Related Skills

Use `related_skills` for:
1. **Complementary skills**: Skills that work well together
2. **Prerequisite skills**: Skills that should run first
3. **Follow-up skills**: Skills to suggest after this one completes

```yaml
related_skills:
  - issue-investigator (run first to find root cause)
  - vision-quest (for problems that resist conventional analysis)
```

After a skill is invoked, Claude sees this field and can:
- Suggest related skills to the user
- Chain skills automatically when appropriate
- Understand the skill ecosystem

## One-Liners (Proverbs)

> *"If a skill can be compressed to a proverb, it survives any discontinuity."*

The `one_liner` field captures the core wisdom of a skill in a single memorable phrase:

```yaml
one_liner: "Don't guess—instrument. Add tracing, run code, SEE what happens."
```

**Why proverbs matter:**
1. **Maximally portable** - Survive model changes, repo forks, even platform death
2. **Human-readable** - Useful without code context
3. **Memorable** - Carry forward naturally in conversations
4. **Grounding** - Quick reminder of skill's essence after invocation

**Good one-liners:**
- State a principle, not a procedure
- Are self-contained (need no external context)
- Use concrete terms (not "do the right thing")
- Are quotable in conversation

## Session Learnings Archive

Skills should include a `## Session Learnings Archive` section to capture knowledge gained during execution. This creates a persistent record that survives context loss.

### Template

```markdown
## Session Learnings Archive

### YYYY-MM-DD: [Brief Title]
**Harvested** → [Where the learning was integrated, with links]

---

### YYYY-MM-DD: [Another Learning]
**Origin**: [What triggered this learning - optional for unharvested]
**Core insight**: [The key learning - optional for unharvested]
**Harvested** → [Link to integration, OR "Pending harvest"]

---
```

### Guidelines

1. **Add entries chronologically** - Newest at top OR bottom (be consistent within skill)
2. **Mark as Harvested** when the learning is integrated into the skill body
3. **Link to the integration point** - Use anchor links like `#section-name`
4. **Brief titles** - Should hint at the learning without reading the full entry
5. **Cross-reference** - Link to related skills or discussions when relevant

### Harvest Workflow

```
1. Session produces insight
2. Add to Session Learnings Archive (unharvested)
3. Future session integrates into skill body
4. Update archive entry with "Harvested" marker + link
```

## The Philosophy

**Skills are markdown files** - Claude can read them fully. The YAML frontmatter is metadata; the markdown body is the actual instructions.

**Extended fields are conventions** - They may or may not be parsed by Claude Code specially. What matters is:
1. They document intent for human authors
2. They enable custom linting
3. They're available to Claude after invocation
4. The embedding pattern makes them effective regardless

**Don't fight the system, work with it** - Since only `description` surfaces pre-invocation, we embed triggers there. The extended fields provide structure, documentation, and post-invocation utility.
