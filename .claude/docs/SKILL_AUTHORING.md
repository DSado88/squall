# Skill Authoring Guide

How to write effective skill files that actually improve agent behavior.

## Quick Start

Create a new skill:

```bash
mkdir -p .claude/skills/my-skill
cat > .claude/skills/my-skill/SKILL.md << 'EOF'
---
name: my-skill
description: Brief description. Use when asked to "trigger phrase". (project)
one_liner: "Core wisdom in one sentence."
activation_triggers:
  - "trigger phrase"
  - When some condition occurs
related_skills:
  - other-skill (relationship explanation)
---

# My Skill

> **TL;DR - QUICK REFERENCE**
>
> | Concept | Translation |
> |---------|-------------|
> | Key concept 1 | What it means |
> | Key concept 2 | What it means |

## Workflow

1. Step one
2. Step two
3. Step three

## Anti-Patterns

| Don't | Do Instead |
|-------|------------|
| Bad practice | Good practice |

## Session Learnings Archive

(Empty - learnings will accumulate here)
EOF
```

## Required Structure

Every skill MUST have:

### 1. Frontmatter (YAML)

```yaml
---
name: skill-name              # Required: lowercase, hyphens, 1-64 chars
description: "Description"    # Required: 1-1024 chars, embed triggers!
one_liner: "Proverb"          # Recommended: core wisdom
activation_triggers:          # Recommended: when to activate
  - "exact phrase"
  - Contextual condition
related_skills:               # Optional: skill connections
  - skill-name (relationship)
---
```

### 2. TL;DR Callout

Every skill should start with a quick reference:

```markdown
> **TL;DR - SKILL NAME**
>
> | Concept | Translation |
> |---------|-------------|
> | Key idea | Simple explanation |
>
> **Entry criteria:**
> - Condition 1
> - Condition 2
```

### 3. Main Content

The body of the skill with:
- Workflow/process steps
- Reference tables
- Examples
- Anti-patterns

### 4. Session Learnings Archive

A section at the end for capturing learnings:

```markdown
## Session Learnings Archive

(Learnings will be added here during skill execution)
```

## Writing Effective Descriptions

The `description` field is the **only thing Claude sees** before deciding to load your skill.

### Bad Description

```yaml
description: Handles debugging tasks.
```

Problems:
- Too vague
- No trigger phrases
- Won't match user intent

### Good Description

```yaml
description: Investigate bugs, debug issues, find root causes. Use when asked to "investigate issue", "debug", "find root cause", "why is this broken", or when something isn't working as expected. (project)
```

Strengths:
- Multiple synonyms for matching
- Embedded trigger phrases in quotes
- Contextual condition included
- `(project)` suffix marks it as project-specific

## Activation Triggers Best Practices

### Mix Exact and Semantic

```yaml
activation_triggers:
  # Exact phrases (quoted)
  - "investigate issue"
  - "debug"
  - "find root cause"

  # Semantic/contextual (unquoted)
  - When user reports something is broken
  - When a fix didn't work as expected
  - Problems that resist conventional debugging
```

### Cover Variations

Users say things differently:

```yaml
activation_triggers:
  - "investigate issue"
  - "investigate #"
  - "look into issue"
  - "check out issue"
  - "debug issue"
  - "analyze issue"
```

## Anti-Pattern Tables

Every skill should have an anti-patterns table:

```markdown
## Anti-Patterns

| Don't | Do Instead |
|-------|------------|
| Rush to fix without investigating | Investigate root cause first |
| Trust stale information | Verify information is current |
| Report done without testing | Always verify the fix works |
```

Anti-patterns are **high-value learning targets**. They capture what NOT to do, which is often more useful than what TO do.

## One-Liners (Proverbs)

The `one_liner` should be:

| Good | Bad |
|------|-----|
| "Don't guess—instrument." | "Debug carefully." |
| "Skills persist, Claude doesn't." | "Use skills well." |
| "Friction IS the teacher." | "Learn from mistakes." |

Test: Would you quote this in conversation? If not, make it more memorable.

## Workflow Sections

Structure workflows as numbered steps:

```markdown
## Workflow

1. **Gather context** - Read the issue, understand requirements
2. **Form hypothesis** - What do you think is wrong?
3. **Test hypothesis** - Run the code, check logs
4. **Verify fix** - Confirm the solution works
5. **Update skills** - Document what you learned
```

Each step should be actionable and verifiable.

## Callout Boxes

Use callout boxes for critical information:

```markdown
> **CRITICAL: Always do this**
>
> Explanation of why this is critical...

> **WARNING: Don't do this**
>
> What goes wrong if you do...
```

## Cross-Referencing

Link to related skills with relative paths:

```markdown
See [issue-investigator](../issue-investigator/SKILL.md) for investigation methodology.
```

Don't use prose references like "see the issue-investigator skill" - use actual links.

## Harvesting Learnings

The Session Learnings Archive workflow:

```
1. During task execution, something is learned
2. Add entry to archive (unharvested)
3. Later, integrate learning into skill body
4. Mark archive entry as "Harvested" with link

Example:
### 2024-01-15: Root cause was config, not code
**Harvested** → See [Anti-Patterns](#anti-patterns) ("Trust stale information")
```

**Important**: Don't just accumulate learnings. Actively harvest them into the skill body. The archive is history, not the brain.

## Skill Size Guidelines

| Metric | Healthy | Warning | Action Needed |
|--------|---------|---------|---------------|
| Word count | < 2000 | 2000-3000 | > 3000 (crystallize) |
| Read time | < 5 min | 5-8 min | > 8 min (split) |
| Archive entries | < 10 | 10-20 | > 20 (harvest) |

If a skill gets too large, consider:
1. Harvesting archive entries into the body
2. Splitting into multiple skills
3. Moving reference material to separate docs

## Linting Checklist

Before committing a skill:

- [ ] `name` is lowercase with hyphens
- [ ] `description` contains embedded trigger phrases
- [ ] `description` ends with `(project)`
- [ ] `activation_triggers` array is present
- [ ] TL;DR callout exists at the top
- [ ] Anti-patterns table exists
- [ ] Session Learnings Archive section exists
- [ ] All cross-references use markdown links, not prose

## Template

Use the template in `.claude/skills/_template/SKILL.md` as a starting point for new skills.
