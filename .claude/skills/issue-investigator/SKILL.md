---
name: issue-investigator
description: 'Investigate issues, debug problems, find root causes. Use when asked to investigate issue, investigate #, debug issue, find root cause, research bug, explore problem, look into issue, check out issue, analyze issue, why is this broken, or something is not working. Systematic approach using multi-sensor grounding. (project)'
one_liner: "Don't guess—instrument. Add tracing, run code, SEE what happens."
activation_triggers:
  - "investigate issue"
  - "investigate #"
  - "debug issue"
  - "find root cause"
  - "research bug"
  - "explore problem"
  - "look into issue"
  - "check out issue"
  - "analyze issue"
  - "why is this broken"
  - "something is not working"
  - When user reports something is broken
  - When a fix didn't work as expected
related_skills:
  - "[self-improvement-loop](../self-improvement-loop/SKILL.md) (capture learnings after investigation)"
  - "[vision-quest](../vision-quest/SKILL.md) (for problems that resist conventional analysis)"
---

# Issue Investigator

> **TL;DR - INVESTIGATION CHECKLIST**
>
> | Phase | Action | Output |
> |-------|--------|--------|
> | 1. Understand | Read issue, gather context | Clear problem statement |
> | 2. Hypothesize | Form theory about root cause | Testable hypothesis |
> | 3. Instrument | Add logging, tracing, tests | Observable behavior |
> | 4. Verify | Run code, check output | Confirmed/denied hypothesis |
> | 5. Document | Update issue, capture learnings | Knowledge preserved |
>
> **Core principle**: Don't guess—instrument. Add tracing, run code, SEE what happens.

A structured methodology for investigating bugs and issues from discovery through resolution.

## The Investigation Loop

```
┌─────────────────────────────────────────────────────────────┐
│                                                             │
│  ┌────────────────┐                                         │
│  │  UNDERSTAND    │  Read issue, gather context            │
│  │  THE PROBLEM   │                                         │
│  └───────┬────────┘                                         │
│          │                                                  │
│          ▼                                                  │
│  ┌────────────────┐                                         │
│  │  FORM          │  What do you think is wrong?           │
│  │  HYPOTHESIS    │                                         │
│  └───────┬────────┘                                         │
│          │                                                  │
│          ▼                                                  │
│  ┌────────────────┐     ┌────────────────┐                 │
│  │  INSTRUMENT    │────►│  RUN & OBSERVE │                 │
│  │  THE CODE      │     │                │                 │
│  └────────────────┘     └───────┬────────┘                 │
│                                 │                          │
│          ┌──────────────────────┘                          │
│          │                                                  │
│          ▼                                                  │
│  ┌────────────────┐  No   ┌────────────────┐               │
│  │  HYPOTHESIS    │──────►│  NEW           │               │
│  │  CONFIRMED?    │       │  HYPOTHESIS    │───────┐       │
│  └───────┬────────┘       └────────────────┘       │       │
│          │ Yes                                     │       │
│          ▼                                         │       │
│  ┌────────────────┐                               │       │
│  │  DOCUMENT      │◄──────────────────────────────┘       │
│  │  FINDINGS      │                                        │
│  └────────────────┘                                        │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## Phase 1: Understand the Problem

Before investigating, ensure you understand:

1. **What is the expected behavior?**
2. **What is the actual behavior?**
3. **What are the reproduction steps?**
4. **When did this start happening?**

```markdown
## Problem Statement Template

**Expected**: [What should happen]
**Actual**: [What actually happens]
**Repro steps**:
1. Do X
2. Do Y
3. Observe Z

**Context**: [When it started, what changed, environment details]
```

## Phase 2: Form Hypothesis

Based on the problem statement, form a testable hypothesis:

| Symptom | Possible Causes | How to Test |
|---------|-----------------|-------------|
| Function returns wrong value | Logic error, bad input, state corruption | Add logging, check inputs/outputs |
| Code throws exception | Null reference, type error, missing dependency | Check stack trace, add guards |
| Performance is slow | N+1 queries, missing cache, inefficient algorithm | Add timing, profile code |
| UI doesn't update | State not propagating, event not firing | Add console logs, check state |

**The 5 Whys**: Keep asking "why?" until you reach the root cause:

```
1. Why is the test failing? → The function returns null
2. Why does it return null? → The database query returns empty
3. Why is the query empty? → The WHERE clause is wrong
4. Why is it wrong? → The ID is being passed as string, not int
5. Why is it a string? → The API doesn't parse the parameter
```

## Phase 3: Instrument the Code

Don't guess. Add observability:

### Logging
```javascript
console.log('[DEBUG] functionName called with:', { arg1, arg2 });
console.log('[DEBUG] state before:', JSON.stringify(state));
// ... operation ...
console.log('[DEBUG] state after:', JSON.stringify(state));
console.log('[DEBUG] returning:', result);
```

### Tracing
```python
import traceback
print(f"[TRACE] {function_name}: entering")
print(f"[TRACE] {function_name}: args = {args}")
try:
    result = actual_operation()
    print(f"[TRACE] {function_name}: success = {result}")
    return result
except Exception as e:
    print(f"[TRACE] {function_name}: error = {e}")
    traceback.print_exc()
    raise
```

### Assertions
```rust
assert!(value > 0, "Expected positive value, got {}", value);
debug_assert_eq!(expected, actual, "Mismatch at step {}", step);
```

## Phase 4: Run and Observe

Execute the code and observe:

1. **Run the specific failing case**
2. **Capture all output** (logs, errors, state)
3. **Compare expected vs actual** at each step
4. **Identify the divergence point**

```bash
# Run with verbose output
DEBUG=* npm test -- --grep "failing test"

# Capture output to file for analysis
./program 2>&1 | tee debug.log

# Search logs for anomalies
grep -E "(ERROR|WARN|unexpected)" debug.log
```

## Phase 5: Document Findings

After investigation, document:

```markdown
## Investigation Summary

**Root cause**: [What was actually wrong]

**Evidence**:
- [Log output showing the issue]
- [Code location: file:line]
- [Relevant stack trace]

**Fix approach**: [How to fix it]

**Prevention**: [How to prevent similar issues]
```

## Multi-Root-Cause Syndrome

Sometimes "still broken" means there are multiple issues:

```
Fix A ──► "Still broken" ──► Fix B ──► "Still broken" ──► Fix C ──► Works!
```

When this happens:
1. Document each root cause separately
2. Don't assume the first fix was wrong
3. Layer by layer debugging

## Distinguishing Bug Types

Not all issues are code bugs:

| Type | Symptoms | Investigation |
|------|----------|---------------|
| **Code bug** | Logic error, wrong output | Debug code, add tests |
| **Config issue** | Works in one env, not another | Check config files, env vars |
| **Data issue** | Works with some data, not others | Check data integrity, edge cases |
| **Environment issue** | Works locally, fails in CI | Check dependencies, versions |
| **Race condition** | Intermittent failures | Add timing logs, check async code |

## Anti-Patterns

| Don't | Do Instead |
|-------|------------|
| Guess at the root cause | Instrument and observe |
| Fix without understanding | Understand the full chain |
| Trust stale information | Verify current state |
| Assume single root cause | Consider multiple factors |
| Skip documentation | Document findings immediately |
| Jump to fix without test | Write failing test first |
| Ignore intermittent failures | They're real bugs too |

## Quick Reference

### Common Investigation Commands

```bash
# Search codebase for patterns
grep -rn "pattern" src/

# Find recent changes to a file
git log --oneline -20 path/to/file

# Check what changed between versions
git diff HEAD~5 path/to/file

# Find who last modified a line
git blame path/to/file | grep "pattern"

# Run tests with verbose output
npm test -- --verbose
pytest -v -s
cargo test -- --nocapture
```

### Investigation Questions Checklist

- [ ] What is the exact error message?
- [ ] When did this start happening?
- [ ] What changed recently?
- [ ] Can I reproduce it consistently?
- [ ] What are the inputs/outputs at each step?
- [ ] Where does expected diverge from actual?
- [ ] Is this a code, config, data, or environment issue?

<!-- SENTINEL:SESSION_LEARNINGS_START - Do not remove this line -->
## Session Learnings Archive

(Add learnings here as investigations reveal patterns)
<!-- SENTINEL:SESSION_LEARNINGS_END -->
