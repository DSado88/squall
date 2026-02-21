# Let It Crash

> The Erlang philosophy of fault tolerance

## The Principle

> **Don't try to prevent all errors. Instead, make the system resilient to errors.**

Rather than writing defensive code to handle every possible failure, let processes crash and have supervisors restart them in a known-good state.

## Why Let It Crash?

### The Problem with Defensive Programming

```
// Defensive: Try to handle every error
function processData(data) {
    if (data === null) return handleNull();
    if (data === undefined) return handleUndefined();
    if (typeof data !== 'object') return handleWrongType();
    if (!data.hasOwnProperty('value')) return handleMissingValue();
    if (typeof data.value !== 'number') return handleWrongValueType();
    if (data.value < 0) return handleNegative();
    if (data.value > MAX_VALUE) return handleOverflow();
    // Finally, the actual logic...
    return data.value * 2;
}
```

Problems:
- Error handling obscures the happy path
- Each branch needs testing
- Easy to miss edge cases
- Code grows complex

### The Let It Crash Alternative

```
// Let it crash: Handle the happy path, crash on unexpected
function processData(data) {
    return data.value * 2;  // Crash if data isn't what we expect
}

// Supervisor restarts on crash with clean state
supervisor.watch(processData, {
    onCrash: () => restart(),
    maxRestarts: 5,
    window: '1m'
});
```

Benefits:
- Code is simple and focused
- Unexpected states don't accumulate
- Restart gives clean slate
- Supervisor handles recovery

## When to Let It Crash

| Situation | Let It Crash? | Why |
|-----------|---------------|-----|
| Unexpected null | Yes | State corruption, restart fresh |
| Network timeout | Maybe | Retry first, then crash |
| User input error | No | Expected, handle gracefully |
| Out of memory | Yes | Can't recover, need restart |
| Config missing | Yes | Can't continue without it |
| File not found | Depends | Expected? Handle. Unexpected? Crash. |

## The Supervisor Hierarchy

```
┌─────────────────────────────────────────────────────────────┐
│                     APPLICATION                              │
│                          │                                   │
│                    ┌─────┴─────┐                             │
│                    │ TOP SUPER │                             │
│                    └─────┬─────┘                             │
│              ┌───────────┼───────────┐                       │
│              ▼           ▼           ▼                       │
│         ┌────────┐  ┌────────┐  ┌────────┐                  │
│         │ Super  │  │ Super  │  │ Super  │                  │
│         └────┬───┘  └────┬───┘  └────┬───┘                  │
│              │           │           │                       │
│         ┌────┴────┐ ┌────┴────┐ ┌────┴────┐                 │
│         ▼    ▼    ▼ ▼    ▼    ▼ ▼    ▼    ▼                 │
│        [W]  [W]  [W][W]  [W]  [W][W]  [W]  [W]              │
│                                                              │
│  W = Worker (crashes are contained)                          │
│  Super = Supervisor (restarts workers)                       │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

Crashes propagate up until a supervisor handles them. Each level can have different restart strategies.

## Restart Strategies

| Strategy | Behavior | Use When |
|----------|----------|----------|
| **One-for-One** | Restart only the crashed process | Processes are independent |
| **One-for-All** | Restart all processes | Processes are interdependent |
| **Rest-for-One** | Restart crashed + processes started after it | Ordered dependencies |

## Applied to AI Agents

The Let It Crash philosophy applies to skill-based learning:

| Concept | Application |
|---------|-------------|
| **Process = Session** | Each session is isolated |
| **Crash = Context Loss** | Session dies, learnings lost (unless captured) |
| **Supervisor = Skill Files** | Skills "restart" the agent with known-good patterns |
| **Clean State = Fresh Session** | Each session starts from skill files |

### The Skill File as Supervisor

```
Session crashes (context lost)
         │
         ▼
┌─────────────────┐
│   Skill Files   │  ← Known-good state
│                 │
│  - Patterns     │
│  - Anti-patterns│
│  - Workflows    │
│                 │
└────────┬────────┘
         │
         ▼
New session starts with accumulated wisdom
```

This is why **updating skill files** is critical. They're the supervisory layer that preserves knowledge across session "crashes."

## Anti-Patterns

| Don't | Do Instead |
|-------|------------|
| Catch and swallow all errors | Let unexpected errors crash |
| Accumulate corrupted state | Restart with clean state |
| Add defensive code everywhere | Focus on the happy path |
| Ignore crashes | Monitor and learn from them |

## The Balance

Let It Crash doesn't mean:
- Ignore all errors
- Never validate input
- Skip logging

It means:
- Handle expected errors gracefully
- Let unexpected errors crash
- Have supervisors restart cleanly
- Learn from crash patterns
