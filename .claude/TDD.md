# Test-Driven Development

> **Red → Green → Refactor**

TDD is not just about testing - it's about **design**. Writing tests first forces you to think about interfaces before implementations.

## The TDD Cycle

```
┌─────────────────────────────────────────────────────────────┐
│                      TDD CYCLE                               │
│                                                              │
│      ┌───────┐                                               │
│      │  RED  │  Write a failing test                        │
│      └───┬───┘                                               │
│          │                                                   │
│          ▼                                                   │
│      ┌───────┐                                               │
│      │ GREEN │  Write minimal code to pass                  │
│      └───┬───┘                                               │
│          │                                                   │
│          ▼                                                   │
│      ┌──────────┐                                            │
│      │ REFACTOR │  Improve code while tests pass            │
│      └────┬─────┘                                            │
│           │                                                  │
│           └──────────────────────► (back to RED)            │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

## Why TDD?

| Without TDD | With TDD |
|-------------|----------|
| Write code, then test (maybe) | Tests define the contract |
| Discover bugs late | Discover bugs immediately |
| Fear of refactoring | Confidence to refactor |
| Code dictates interface | Interface dictates code |
| "It works on my machine" | Reproducible verification |

## The Rules

1. **Don't write production code without a failing test**
2. **Don't write more test than necessary to fail**
3. **Don't write more production code than necessary to pass**

These rules create a tight feedback loop that guides design.

## TDD for Agent Behavior

Skills are **TDD for agent behavior**:

```
Skill File = Test Specification
  "When X happens, do Y, expect Z"

Execution = Implementation
  Actually do Y, observe what happens

Divergence = Test Failure
  If actual != expected, skill needs refinement

Skill Update = Refactoring
  Adjust the skill based on evidence
```

## Test Types

### Unit Tests
- Test individual functions/methods
- Fast, isolated, no external dependencies
- Run on every change

### Integration Tests
- Test components working together
- May involve databases, APIs, filesystem
- Run before commits

### End-to-End Tests
- Test the full system
- Slowest but highest confidence
- Run before releases

## The Testing Pyramid

```
         /\
        /  \      E2E (few, slow, high confidence)
       /────\
      /      \    Integration (medium)
     /────────\
    /          \  Unit (many, fast, focused)
   /────────────\
```

Most tests should be unit tests. Few should be E2E.

## Anti-Patterns

| Don't | Do Instead |
|-------|------------|
| Write tests after the fact | Write tests first |
| Test implementation details | Test behavior/contracts |
| Mock everything | Mock at boundaries only |
| Skip tests for "simple" code | All code deserves tests |
| Let test suite become slow | Keep tests fast |

## Test Naming

Good test names describe:
1. **What** is being tested
2. **Under what conditions**
3. **Expected outcome**

```
// Bad
test_login()

// Good
test_login_with_valid_credentials_returns_session_token()
test_login_with_invalid_password_returns_401_error()
```

## The Confidence Equation

```
Refactoring Confidence = Test Coverage × Test Quality
```

High coverage with low-quality tests = false confidence.
Focus on testing **behavior**, not **lines of code**.
