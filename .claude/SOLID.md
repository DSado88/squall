# SOLID Principles

Five principles for maintainable object-oriented design.

## S - Single Responsibility Principle

> **A class should have only one reason to change.**

```
// Bad: Multiple responsibilities
class UserManager {
    saveUser(user) { /* database logic */ }
    sendEmail(user) { /* email logic */ }
    generateReport(user) { /* reporting logic */ }
}

// Good: Single responsibility each
class UserRepository { saveUser(user) { } }
class EmailService { sendEmail(user) { } }
class UserReportGenerator { generateReport(user) { } }
```

**Test**: Can you describe the class without using "and"?

## O - Open/Closed Principle

> **Open for extension, closed for modification.**

```
// Bad: Modify existing code for new shapes
function calculateArea(shape) {
    if (shape.type === 'circle') return Math.PI * shape.radius ** 2;
    if (shape.type === 'square') return shape.side ** 2;
    // Must modify this function for every new shape
}

// Good: Extend without modifying
interface Shape { calculateArea(): number; }
class Circle implements Shape { calculateArea() { return Math.PI * this.radius ** 2; } }
class Square implements Shape { calculateArea() { return this.side ** 2; } }
// New shapes just implement the interface
```

**Test**: Can you add new behavior without changing existing code?

## L - Liskov Substitution Principle

> **Subtypes must be substitutable for their base types.**

```
// Bad: Square violates Rectangle's contract
class Rectangle {
    setWidth(w) { this.width = w; }
    setHeight(h) { this.height = h; }
}
class Square extends Rectangle {
    setWidth(w) { this.width = w; this.height = w; } // Breaks expectations!
}

// Good: Separate abstractions
interface Shape { getArea(): number; }
class Rectangle implements Shape { }
class Square implements Shape { }
```

**Test**: Can you substitute a subclass without breaking the program?

## I - Interface Segregation Principle

> **Clients shouldn't depend on interfaces they don't use.**

```
// Bad: Fat interface
interface Worker {
    work();
    eat();
    sleep();
}
// Robot can't eat or sleep!

// Good: Segregated interfaces
interface Workable { work(); }
interface Eatable { eat(); }
interface Sleepable { sleep(); }

class Human implements Workable, Eatable, Sleepable { }
class Robot implements Workable { }
```

**Test**: Are there interface methods that some implementers leave empty?

## D - Dependency Inversion Principle

> **Depend on abstractions, not concretions.**

```
// Bad: High-level depends on low-level
class OrderService {
    constructor() {
        this.db = new MySQLDatabase(); // Concrete dependency
    }
}

// Good: Depend on abstraction
class OrderService {
    constructor(database: Database) { // Abstract dependency
        this.db = database;
    }
}
```

**Test**: Can you swap implementations without changing the high-level code?

## SOLID Summary

| Principle | Question to Ask |
|-----------|-----------------|
| **S**ingle Responsibility | Does this class do one thing? |
| **O**pen/Closed | Can I extend without modifying? |
| **L**iskov Substitution | Can subtypes replace base types? |
| **I**nterface Segregation | Are interfaces focused? |
| **D**ependency Inversion | Do I depend on abstractions? |

## When to Apply

SOLID principles are guidelines, not laws. Apply when:
- Code is hard to test
- Changes ripple across many files
- You can't understand a class quickly
- Mocking requires complex setup

Don't over-engineer simple code to satisfy SOLID dogmatically.
