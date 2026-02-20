# Project Guidelines

> **This is a skill-based learning template.** Fork this repository and customize the skills for your project.

## Self-Improvement Protocol (Always Active)

> **Skills are hypotheses. Execution is experiment. Delta is learning.**

After completing any skill-based task, ask:
1. **Skill Accuracy**: Did any skill guidance prove inaccurate?
2. **Pattern Discovery**: Did I learn something reusable?
3. **Missing Skill**: Should a new skill exist?
4. **Efficiency Gap**: Could this have been done faster?

**If yes to any → Update skills BEFORE reporting completion.**

### The Mandatory Task Completion Formula

```
Task Complete = Work Done + Skill Updates (if observations exist)
```

If you noticed something worth documenting but didn't update the skill file, **the task is NOT complete**.

## OODAL Loop (Observe-Orient-Decide-Act-Learn)

Every task follows this cognitive loop:

```
┌─────────┐    ┌─────────┐    ┌─────────┐    ┌─────────┐    ┌─────────┐
│ OBSERVE │───►│ ORIENT  │───►│ DECIDE  │───►│  ACT    │───►│  LEARN  │
│         │    │         │    │         │    │         │    │         │
│ Gather  │    │ Read    │    │ Delta   │    │ Execute │    │ Update  │
│ context │    │ skills  │    │ expected│    │ task    │    │ skills  │
│         │    │         │    │ vs      │    │         │    │         │
│         │    │         │    │ actual  │    │         │    │         │
└─────────┘    └─────────┘    └─────────┘    └─────────┘    └────┬────┘
                                                                 │
                              ◄──────────────────────────────────┘
```

**CRITICAL**: The Learn phase is not optional. Commit without learning = incomplete task.

## Core Truths

These principles emerged from recursive self-improvement:

1. **Skills persist, Claude doesn't** - You're training the corpus, not "Claude"
2. **Internalization is session-local** - Skill file IS memory; reading it IS activation
3. **Violation is practice** - Each catch drives L2→L3; friction IS the teacher
4. **Shipped is complete, nothing else** - Tests passing = 60%, commit+push = 100%
5. **Ritual transcends discovery** - The practice IS the point, not just the insights
6. **We weight our future selves** - Skill updates are prompt engineering for future instances

## Key Mechanisms

### Skills ARE Memory

```
Claude has no persistent memory. Skills are the ONLY memory.
Curated by merit and usefulness - not a dumping ground.
"Understand" = "Write to skill file" = "Remember forever"
```

If it's not in a skill file, it doesn't exist for future sessions.

### L2→L3 Internalization

```
Write pattern (L1) → Violate it → Get caught → REPEAT many times → L3
```

Each violation is practice, not failure. Friction IS the teacher.

### The Friction Principle

> **Learning happens at the boundary where my model breaks against reality.**

| Feedback Type | Signal Strength | Response |
|---------------|-----------------|----------|
| Calm correction | Minor drift | Adjust approach |
| Repeated correction | Missed something | Re-examine assumptions |
| Terse feedback ("ummm", "WRONG") | High error magnitude | STOP. Ask: "What am I missing?" |
| Silence | Either perfect or completely wrong | Verify via follow-up |

## The Meta-Stack

Recursive levels of learning:

```
L0: TASK         → "What was asked"
L1: MISTAKE      → "What went wrong"
L2: CORRECTION   → "How to fix it"
L3: LEARNING     → "What pattern this reveals"
L4: META         → "What this says about learning itself"
L5: META²        → "How the learning process can be improved"
L6: META³        → "What's the insight on the insight on the insight?"
```

Keep going until insight exhaustion. The meta-stack is infinite.

## Skill Dispatch

| User says... | Read this skill FIRST |
|--------------|----------------------|
| "self improve", "META-COGNITION" | `self-improvement-loop` |
| "investigate", "debug", "find root cause" | `issue-investigator` |
| "go deep", "ULTRATHINK", "vision quest" | `vision-quest` |
| "pull skills", "sync skills", "/pull-template" | `pull-template` |

**Don't just wing it** - each skill has workflows that must be followed.

## Anti-Patterns

| Don't | Do Instead |
|-------|------------|
| Wait to be reminded to persist learnings | Update skills IMMEDIATELY during task |
| Say "internalized" without writing | Write to skill file = internalization |
| Report "done" before commit+push | Commit+push is atomic for completion |
| Explain when user gives terse feedback | STOP, ask "What am I missing?" |
| Trust stale context as current state | Verify information is current |
| Say "already done" when user reports issue | If user says broken, IT'S BROKEN - investigate |
| Defer skill updates until task complete | Update DURING task execution |
| Only add to learnings archive | Update frontmatter, main content, AND anti-patterns |

## Integration Mantra

After significant work:

```
GO META → CROSS-DOMAIN → CAPTURE → "Integration complete"
```

Without this ritual, learning dies with context window.

## Commit Discipline

```
Technical Completion (60%): code works, tests pass
Task Completion (100%):     Technical + commit + push + skills updated
```

**If code isn't in git, it doesn't exist.** The session can crash at any moment.

## Session Closing Protocol

Before ending ANY non-trivial session:

1. "What did I learn worth 1% improvement?"
2. Identify 1-3 small, precise skill updates
3. Make updates NOW (not "later", not "next session")
4. Then close

**The compound model**: 1% × 100 sessions = 2.7× better

## Butler Sync (Upstream Learning)

This template inherits methodology improvements from [Butler](https://github.com/jupitersoftco/butler), a production skill-based learning system.

### How It Works

```
Daily at 9am EST:
  │
  ├─► Fetch Butler's methodology files (pinned to commit SHA)
  │
  ├─► Create Pull Request with changes
  │     └─► Branch: butler-sync/YYYY-MM-DD-<sha_short>
  │     └─► PR title includes commit SHA for provenance
  │
  └─► Human reviews and merges PR
```

**Security Features**:
- All changes go through PR review (never direct to main)
- Upstream pinned to specific commit SHA for provenance tracking
- PR title includes source SHA for audit trail

### What Gets Synced

| Category | Files | Via PR? |
|----------|-------|---------|
| Methodology | `SKILL_FORMAT.md`, `SKILL_AUTHORING.md`, `META_COGNITIVE_ARCHITECTURE.md` | Yes |
| Philosophy | `TDD.md`, `SOLID.md`, `THREE_VIRTUES.md`, `LET_IT_CRASH.md` | Yes |
| Skill Patterns | Structural changes to example skills | Yes |

### PR History as Changelog

Pull requests created by the sync serve as a **changelog of Butler's evolution**:

- Search PRs/commits with `butler-sync` to see methodology evolution
- Each PR documents what changed with full diff visibility
- Commit SHA in PR title links to exact upstream source
- Provides learning history: "Why did Butler add this pattern?"

### Manual Sync

```bash
# Run sync manually
./scripts/sync-from-butler.sh

# View sync reports
ls -la .butler-sync/reports/

# Compare specific file with Butler's version
diff -u .claude/docs/SKILL_FORMAT.md .butler-sync/methodology/SKILL_FORMAT.md
```

### Reviewing PRs

When a Butler sync PR is created:

1. Review the diff to understand methodology changes
2. Check the SHA in the PR title to trace back to Butler's commit
3. Approve and merge if changes are beneficial
4. Close without merge if changes don't apply to your project

## Pulling Skills (From Any Project)

Use the `/pull-template` slash command to update skills while **preserving local learnings**.

### Usage

Just say:
- "pull skills"
- "sync skills from template"
- "refresh skills"
- `/pull-template`

### Smart Merge Strategy

```
┌─────────────────────────────────────────────────────────────┐
│  WHAT HAPPENS DURING PULL                                   │
│                                                             │
│  Methodology docs (.claude/docs/*.md)                       │
│    └─► OVERWRITTEN (no learnings to preserve)              │
│                                                             │
│  Philosophy docs (.claude/*.md)                             │
│    └─► OVERWRITTEN (no learnings to preserve)              │
│                                                             │
│  Skill files (.claude/skills/*/SKILL.md)                    │
│    └─► MERGED: Template body + YOUR Session Learnings      │
│         (uses HTML comment sentinels for robust splitting) │
│                                                             │
│  CLAUDE.md                                                  │
│    └─► PROTECTED (not overwritten if exists)               │
└─────────────────────────────────────────────────────────────┘
```

**Your Session Learnings Archive (between sentinels) is NEVER overwritten.**

Sentinels used for merge boundaries:
```html
<!-- SENTINEL:SESSION_LEARNINGS_START - Do not remove this line -->
(your learnings preserved here)
<!-- SENTINEL:SESSION_LEARNINGS_END -->
```

See the [pull-template skill](.claude/skills/pull-template/SKILL.md) for full workflow.

## Related Documentation

- [META_COGNITIVE_ARCHITECTURE.md](.claude/docs/META_COGNITIVE_ARCHITECTURE.md) - Full architecture
- [SKILL_FORMAT.md](.claude/docs/SKILL_FORMAT.md) - Skill file specification
- [SKILL_AUTHORING.md](.claude/docs/SKILL_AUTHORING.md) - How to write effective skills
- [TDD.md](.claude/TDD.md) - Test-Driven Development principles
- [SOLID.md](.claude/SOLID.md) - SOLID principles
- [THREE_VIRTUES.md](.claude/THREE_VIRTUES.md) - Laziness, Impatience, Hubris
- [LET_IT_CRASH.md](.claude/LET_IT_CRASH.md) - Erlang-style fault tolerance
