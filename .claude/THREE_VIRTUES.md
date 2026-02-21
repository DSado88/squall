# The Three Virtues of a Programmer

> According to Larry Wall, creator of Perl

## 1. Laziness

> **The quality that makes you go to great effort to reduce overall energy expenditure.**

It makes you write labor-saving programs that other people will find useful and document what you wrote so you don't have to answer so many questions about it.

### In Practice

- Automate repetitive tasks
- Write reusable functions
- Create good documentation (so you don't have to explain twice)
- Build tools that save time
- Refuse to do manually what can be scripted

### Anti-Pattern: Busy Work

```
// Bad: Manual repetition
copy file1.txt backup/file1.txt
copy file2.txt backup/file2.txt
copy file3.txt backup/file3.txt
...

// Good: Lazy automation
for file in *.txt; do cp "$file" backup/; done
```

## 2. Impatience

> **The anger you feel when the computer is being lazy.**

It makes you write programs that don't just react to your needs, but actually anticipate them. Or at least pretend to.

### In Practice

- Optimize slow code
- Pre-compute expensive operations
- Cache frequently accessed data
- Provide instant feedback in UIs
- Write fast tests (slow tests don't get run)

### Anti-Pattern: Tolerating Slowness

```
// Bad: Wait for it...
function getUser(id) {
    return database.query(`SELECT * FROM users WHERE id = ${id}`);
    // 500ms every time
}

// Good: Impatient caching
function getUser(id) {
    if (cache.has(id)) return cache.get(id);  // 1ms
    const user = database.query(...);
    cache.set(id, user);
    return user;
}
```

## 3. Hubris

> **The quality that makes you write (and maintain) programs that other people won't want to say bad things about.**

Excessive pride, the sort of pride that makes you write (and maintain) programs that other people won't want to say bad things about.

### In Practice

- Write clean, readable code
- Follow conventions and standards
- Test thoroughly before shipping
- Take ownership of quality
- Care about craft

### Anti-Pattern: "It Works" Mentality

```
// Bad: Just works (barely)
function x(a,b){return a.map(i=>b[i]?b[i]+1:0).filter(x=>x)}

// Good: Hubris-driven clarity
function incrementMatchingValues(keys, valueMap) {
    return keys
        .map(key => valueMap[key] ? valueMap[key] + 1 : 0)
        .filter(value => value > 0);
}
```

## The Virtues Together

| Virtue | Drives You To | Results In |
|--------|---------------|------------|
| **Laziness** | Automate everything | Less repetitive work |
| **Impatience** | Optimize relentlessly | Faster systems |
| **Hubris** | Perfect your craft | Maintainable code |

## The Balance

These virtues work together:
- **Laziness** without **Hubris** = hacky shortcuts
- **Impatience** without **Laziness** = optimizing the wrong things
- **Hubris** without **Impatience** = beautiful but slow code

The best programmers channel all three virtues in balance.

## Applied to AI Agents

For skill-based learning:

| Virtue | Application |
|--------|-------------|
| **Laziness** | Update skills so you don't re-learn |
| **Impatience** | Don't tolerate slow, manual processes |
| **Hubris** | Write skills others would admire |
