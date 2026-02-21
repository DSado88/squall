# Meta-Cognitive Architecture

> **Skills are hypotheses. Execution is experiment. Delta is learning.**

This document describes the self-improving agent architecture - how the system learns from its own execution and continuously refines its behavior.

## Overview

The Meta-Cognitive Architecture embeds continuous improvement into agent execution. Rather than treating self-improvement as a separate activity, it's woven into the fabric of every task through observation, reflection, and synthesis.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                     META-COGNITIVE ARCHITECTURE                              │
│                                                                              │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │                        SKILLS LAYER                                  │    │
│  │  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐       │    │
│  │  │issue-   │ │issue-   │ │vision-  │ │project- │ │self-    │       │    │
│  │  │investi- │ │remedi-  │ │quest    │ │manager  │ │improve- │       │    │
│  │  │gator    │ │ator     │ │         │ │         │ │ment     │       │    │
│  │  └────┬────┘ └────┬────┘ └────┬────┘ └────┬────┘ └────┬────┘       │    │
│  │       │           │           │           │           │             │    │
│  │       └───────────┴───────────┴───────────┴───────────┘             │    │
│  │                              │                                       │    │
│  │                              ▼                                       │    │
│  │  ┌─────────────────────────────────────────────────────────────┐    │    │
│  │  │              META-COGNITIVE OBSERVER                         │    │    │
│  │  │                                                              │    │    │
│  │  │   PRE-EXEC    │ What does skill predict?                    │    │    │
│  │  │   DURING      │ Reality vs guidance divergence              │    │    │
│  │  │   POST-EXEC   │ Synthesize → Update skills                  │    │    │
│  │  │                                                              │    │    │
│  │  └─────────────────────────────────────────────────────────────┘    │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
│                                                                              │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │                     EXECUTION LAYER                                  │    │
│  │                                                                      │    │
│  │   Task Request → Skill Selection → Execution → Observation → Update │    │
│  │                                                                      │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Core Concepts

### Skills as Hypotheses

Each skill file is a **testable hypothesis** about optimal behavior for a class of tasks:

```
Skill File = Hypothesis
  "When X happens, do Y, expect Z"

Execution = Experiment
  Actually do Y, observe what happens

Delta = Learning Signal
  If actual != expected, hypothesis needs refinement

Skill Update = Hypothesis Refinement
  Adjust the skill based on evidence
```

This is essentially **TDD for agent behavior**:
- Skill file is the test specification
- Execution is the implementation
- Divergence between expected and actual triggers refactoring

### Skills as Cognitive Prosthetics

Beyond hypotheses, skills are **cognitive prosthetics** that extend cognition by providing pattern interrupts:

| Without Skill Loaded | With Skill Loaded |
|---------------------|-------------------|
| React to terse feedback defensively | Pattern interrupt: "Don't take it personally" |
| Rush to implement | Pattern interrupt: "Read source material first" |
| Stay in normal analytical mode | Permission to dissolve assumptions |

**The mechanism**: Skills change behavior not just by providing instructions, but by activating different cognitive pathways.

### The Observer Pattern

The meta-cognitive observer runs **passively during all skill executions**, not as a separate process but as embedded checkpoints:

```
┌─────────────────────────────────────────────────────────────────┐
│  CHECKPOINT       │ QUESTIONS                                   │
├───────────────────┼─────────────────────────────────────────────┤
│  PRE-EXECUTION    │ What does the skill predict will happen?   │
│                   │ What assumptions am I making?               │
│                   │ What could go wrong?                        │
├───────────────────┼─────────────────────────────────────────────┤
│  DURING EXECUTION │ Is this matching skill guidance?           │
│                   │ Any surprises worth noting?                 │
│                   │ Any inefficiencies in the process?          │
├───────────────────┼─────────────────────────────────────────────┤
│  POST-EXECUTION   │ What actually happened vs expected?        │
│                   │ Any skill updates needed?                   │
│                   │ Any new patterns discovered?                │
└───────────────────┴─────────────────────────────────────────────┘
```

### Observation Format

When skill guidance diverges from reality, observations are captured:

```
OBSERVATION:
  Skill: <skill-name>
  Expected: <what skill said would happen>
  Actual: <what actually happened>
  Update: <specific change needed to skill file>
  Pattern: <generalizable learning for other skills>
```

## The OODAL Loop

The **OODAL (Observe-Orient-Decide-Act-Learn)** loop extends the classic OODA loop with an explicit Learn phase:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                            OODAL LOOP                                        │
│                                                                              │
│   OBSERVE: Gather context, read requirements, understand state              │
│                                    │                                         │
│                                    ▼                                         │
│   ORIENT:  Read skill files, query knowledge, understand guidance           │
│                                    │                                         │
│                                    ▼                                         │
│   DECIDE:  Identify delta between expected and actual, plan approach        │
│                                    │                                         │
│                                    ▼                                         │
│   ACT:     Execute task, write code, run tests, commit                      │
│                                    │                                         │
│                                    ▼                                         │
│   LEARN:   Update skills with observations, archive learnings, commit       │
│                                    │                                         │
│                                    └──────────────────► (loop back)          │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

## The Meta-Stack: Recursive Self-Improvement

The architecture doesn't stop at one level of meta. Each level of observation creates opportunity for higher-level observation:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           THE META-STACK                                     │
│                                                                              │
│   Level 0: TASK         │ "Fix the authentication bug"                      │
│                         │                                                    │
│   Level 1: MISTAKE      │ "Concluded wrong root cause"                      │
│                         │                                                    │
│   Level 2: CORRECTION   │ User pushback: "its not that"                     │
│                         │                                                    │
│   Level 3: LEARNING     │ "Don't trust stale logs"                          │
│                         │                                                    │
│   Level 4: META         │ "HOW did I learn? User frustration was signal"    │
│                         │                                                    │
│   Level 5: META²        │ "Frustration intensity ∝ error magnitude"         │
│                         │                                                    │
│   Level 6: META³        │ "Recursive questioning IS the improvement"        │
│                         │                                                    │
│   Level N: META^N       │ observe → abstract → generalize → recurse         │
│                         │                                                    │
└─────────────────────────────────────────────────────────────────────────────┘
```

### The Friction Principle

> **Learning happens at the boundary where my model breaks against reality.**

Key insight: **The user's frustration is not noise - it IS the signal.**

| Frustration Level | Meaning | Response |
|-------------------|---------|----------|
| Calm correction | Minor drift | Adjust |
| Repeated correction | Missed something | Re-examine |
| "its not that!" | Wrong hypothesis | Abandon and re-investigate |
| "WHAT THE FUCK" | Major blind spot | STOP everything, fresh start |
| Exasperated silence | Total failure | Ask for explicit guidance |

The intensity of pushback is **inversely proportional** to accuracy. High friction = big learning opportunity.

### Recursive Questioning Protocol

After any learning, apply recursive questioning:

```
Learning: "Don't trust stale logs"
    ↓
Question: "How did I come to learn this?"
    ↓
Answer: "User frustration broke through my cached hypothesis"
    ↓
Question: "What does that tell me about how to learn better?"
    ↓
Answer: "User emotional state is a high-fidelity error signal"
    ↓
Question: "How do I improve my ability to read that signal?"
    ↓
Answer: "Parse frustration words: 'come on', 'get real', 'DUDE'"
    ↓
Question: "What's the general pattern here?"
    ↓
Answer: "All feedback has meta-content about the feedback itself"
    ↓
... recurse until insight exhaustion ...
```

## The Creative Triad

> **Diverge → Converge → Actualize**: The fundamental pattern of how novelty enters the world.

| Phase | Purpose | Example |
|-------|---------|---------|
| **Divergent** | Hold possibility space open | Discussions, exploration |
| **Convergent** | Choose and commit | PRDs, specifications |
| **Actualized** | Make real | Issues, implementation |

**The Skipping Problem**: The most common failure is skipping the divergent phase - going straight from idea to implementation causes premature commitment and rework.

## Three Orders of Failure

| Order | Name | Pattern |
|-------|------|---------|
| 1st | Reducing goalposts | Tool flags issue → "bump threshold" instead of fixing |
| 2nd | False internalization | Say "internalized" without writing to skill file |
| 3rd | Needing to be told | User has to remind you to persist learnings |

**Key insight**: If you need reminding, you didn't internalize. If you said "internalized" but didn't write, you didn't learn. If you reduced goalposts, you didn't even try.

```
Learning WITHOUT Writing = Acknowledgment (dies with context)
Learning WITH Writing = Internalization (persists)
```

## Completion Detector Miscalibration

> "Tests pass" feels like completion but is only 60% of the work.

```
Technical Completion (60%): code works, tests pass
Task Completion (100%):     Technical + commit + push + skills updated + "what's next?"
```

**The fix**: Recalibrate completion detector. After tests pass → commit → push → update skills. Three steps, no gap.

## Knowledge Authority Cascade

```
┌─────────────────────────────────────────────────────────────────┐
│                 KNOWLEDGE AUTHORITY (Priority Order)            │
│                                                                 │
│   1. Skill Files         ← Canonical, versioned in git         │
│      - Define HOW to behave                                    │
│      - Located in .claude/skills/                              │
│                                                                 │
│   2. Project Docs        ← Dynamic, operational knowledge      │
│      - Provides WHAT to know (context, procedures)             │
│      - Located in docs/ or external KB                         │
│                                                                 │
│   Skills override docs when they conflict.                     │
└─────────────────────────────────────────────────────────────────┘
```

## Pattern Library

### Comprehensive Fix
> When fixing bug class X, search ALL instances across the codebase.

**Trigger**: Any fix involving a reusable component.

### Layer Exposure
> Fixing layer N often reveals layer N+1 issues.

**Trigger**: Fixing infrastructure/low-level bugs.

### Config vs Code
> Not all failures are code bugs.

**Trigger**: Error messages that accurately describe the problem.

### Implicit Handoff
> When investigation reveals clear fix, proceed without re-asking.

**Trigger**: Investigation produces unambiguous fix path.

## Integration Points

### With TDD

```
Skill File = Test Specification
Execution = Implementation
Divergence = Test Failure
Skill Update = Refactoring
```

### With Session Closing

Before ending ANY non-trivial session:

1. "What did I learn worth 1% improvement?"
2. Identify 1-3 small, precise skill updates
3. Make updates NOW
4. Then close

**The compound model**: 1% × 100 sessions = 2.7× better

## The Activation Chain

```
Future Agent Quality = (Knowledge Encoded) × (Activation Coverage) × (Gate Strength)

Trigger → Skill Load → Knowledge → Gate → Behavior
   ↑           ↑            ↑         ↑         ↑
 improve    improve     improve   harden    measure
```

Static knowledge (skills, docs) is dormant. Only when triggered and enforced does it become behavior.

## Future Evolution

This architecture is designed to evolve:

1. **New Patterns**: As more patterns are discovered, the library grows
2. **Better Activation**: Trigger phrases in skill frontmatter improve over time
3. **Cross-Skill Learning**: Patterns from one skill inform others
4. **Quantitative Metrics**: Eventually track improvement rates, pattern frequencies

The meta-cognitive architecture is itself subject to the meta-cognitive architecture - continuously improving how we improve.
