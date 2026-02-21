---
name: pull-template
description: "Pull skill updates from skill-based-learning template WITHOUT overwriting local learnings. Use when asked to pull skills, update skills from template, sync skills, get latest skills, or refresh skills from upstream. Preserves Session Learnings Archive. (project)"
one_liner: "Structure flows down, learnings stay local."
activation_triggers:
  - "pull skills"
  - "pull template"
  - "update skills from template"
  - "sync skills"
  - "get latest skills"
  - "refresh skills"
  - "/pull-template"
related_skills:
  - "[self-improvement-loop](../self-improvement-loop/SKILL.md) (capture learnings after pull)"
---

# Pull Template Skills

> **TL;DR - SAFE SKILL UPDATES**
>
> | What | Behavior |
> |------|----------|
> | Methodology docs | Overwrite (safe) |
> | Skill body/structure | Update from template |
> | Session Learnings Archive | **PRESERVE** (never overwrite) |
> | CLAUDE.md | Don't overwrite if exists |
>
> **Core principle**: Structure flows downstream, learnings stay local.

## Workflow

When this skill is invoked, execute these steps:

### Step 1: Fetch Template Files

```bash
# Create temp directory and fetch template
TEMP_DIR=$(mktemp -d)
gh repo clone DSado88/skill-based-learning "$TEMP_DIR/template" -- --depth 1
```

### Step 2: Update Methodology Docs (Safe to Overwrite)

These files contain no project-specific learnings:

```
.claude/docs/SKILL_FORMAT.md
.claude/docs/SKILL_AUTHORING.md
.claude/docs/META_COGNITIVE_ARCHITECTURE.md
.claude/TDD.md
.claude/SOLID.md
.claude/THREE_VIRTUES.md
.claude/LET_IT_CRASH.md
```

**Action**: Copy from template, overwriting local versions.

### Step 3: Smart Merge Skills (PRESERVE LEARNINGS!)

For each skill (`self-improvement-loop`, `issue-investigator`, `vision-quest`, `pull-template`):

1. **Extract local Session Learnings Archive** (everything between sentinels):
   ```
   <!-- SENTINEL:SESSION_LEARNINGS_START - Do not remove this line -->
   ... preserve this content ...
   <!-- SENTINEL:SESSION_LEARNINGS_END -->
   ```
2. **Get template skill body** (everything before `SENTINEL:SESSION_LEARNINGS_START`)
3. **Combine**: Template body + Local learnings (within sentinels)

```
MERGED SKILL = Template Body + Local Session Learnings (between sentinels)
```

**CRITICAL**: Content between sentinels must NEVER be overwritten. The sentinels themselves stay.

### Step 4: Handle CLAUDE.md

- If CLAUDE.md **doesn't exist** → Copy from template
- If CLAUDE.md **exists** → Don't overwrite (project may have customizations)

### Step 5: Cleanup

```bash
rm -rf "$TEMP_DIR"
```

## Merge Visualization

```
Template SKILL.md                    Local SKILL.md
┌───────────────────────┐            ┌───────────────────────┐
│ ---                   │            │ ---                   │
│ frontmatter           │            │ frontmatter           │
│ ---                   │            │ ---                   │
│                       │            │                       │
│ # Body                │──┐         │ # Body                │
│ ## Workflow           │  │         │ ## Workflow           │
│ ## Anti-Patterns      │  │         │ ## Anti-Patterns      │
│                       │  │         │                       │
│ <!-- SENTINEL:START --│  │         │ <!-- SENTINEL:START --│──┐
│ ## Session Learnings  │  │         │ ## Session Learnings  │  │
│ (empty)               │  │         │ ### Learning 1        │  │
│ <!-- SENTINEL:END --> │  │         │ ### Learning 2        │  │
└───────────────────────┘  │         │ <!-- SENTINEL:END --> │  │
                           │         └───────────────────────┘  │
                           │                                    │
                           ▼          RESULT                    ▼
                    ┌─────────────────────────────────────────────┐
                    │ ---                                         │
                    │ frontmatter (from TEMPLATE)                │
                    │ ---                                         │
                    │                                             │
                    │ # Body (from TEMPLATE)                      │
                    │ ## Workflow (from TEMPLATE)                │
                    │ ## Anti-Patterns (from TEMPLATE)           │
                    │                                             │
                    │ <!-- SENTINEL:START --> (marker preserved) │
                    │ ## Session Learnings (from LOCAL)          │
                    │ ### Learning 1                             │
                    │ ### Learning 2                             │
                    │ <!-- SENTINEL:END -->   (marker preserved) │
                    └─────────────────────────────────────────────┘
```

## What to Report

After completing the pull:

1. List files updated
2. Show how many learnings were preserved per skill
3. Note any files skipped (e.g., CLAUDE.md if it existed)
4. Suggest: `git diff` to review, `git commit` when satisfied

## Anti-Patterns

| Don't | Do Instead |
|-------|------------|
| Overwrite entire skill files | Merge: template body + local learnings |
| Overwrite CLAUDE.md blindly | Preserve if exists (has customizations) |
| Forget to report preserved learnings | Always confirm learnings were kept |
| Skip the pull-template skill itself | This skill updates too, preserve its learnings |

<!-- SENTINEL:SESSION_LEARNINGS_START - Do not remove this line -->
## Session Learnings Archive

(Learnings about the pull process itself will accumulate here)
<!-- SENTINEL:SESSION_LEARNINGS_END -->
