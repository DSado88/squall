#!/usr/bin/env bash
set -uo pipefail

# Pre-commit checks for Squall.
# Run manually: ./scripts/pre-commit.sh
# Or install as git hook: cp scripts/pre-commit.sh .git/hooks/pre-commit

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

fail=0

step() { printf "  %-30s" "$1..."; }
ok()   { printf "${GREEN}ok${NC}\n"; }
err()  { printf "${RED}FAIL${NC}\n"; fail=1; }

echo "Running pre-commit checks..."
echo ""

# 1. Format check
step "rustfmt"
if cargo fmt --check >/dev/null 2>&1; then ok; else err; echo "    Run: cargo fmt"; fi

# 2. Clippy (default features = global-memory)
step "clippy"
if cargo clippy --all-targets -- -D warnings >/dev/null 2>&1; then ok; else err; echo "    Run: cargo clippy --all-targets -- -D warnings"; fi

# 3. Clippy without default features
step "clippy (no default features)"
if cargo clippy --all-targets --no-default-features -- -D warnings >/dev/null 2>&1; then ok; else err; echo "    Run: cargo clippy --all-targets --no-default-features -- -D warnings"; fi

# 4. Tests (default features)
step "tests"
if cargo test >/dev/null 2>&1; then ok; else err; echo "    Run: cargo test"; fi

# 5. Tests (no default features)
step "tests (no default features)"
if cargo test --no-default-features >/dev/null 2>&1; then ok; else err; echo "    Run: cargo test --no-default-features"; fi

echo ""
if [ $fail -ne 0 ]; then
    echo -e "${RED}Pre-commit checks failed.${NC}"
    exit 1
else
    echo -e "${GREEN}All checks passed.${NC}"
fi
