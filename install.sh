#!/usr/bin/env bash
set -euo pipefail

# Squall installer — builds binary, registers global MCP server, installs global skills.
#
# Usage:
#   ./install.sh              # full install
#   ./install.sh --skills     # skills only (skip build + MCP registration)
#   ./install.sh --build      # build + MCP only (skip skills)

SQUALL_DIR="$(cd "$(dirname "$0")" && pwd)"
INSTALL_BIN="${HOME}/.local/bin/squall"
SKILLS_DIR="${HOME}/.claude/skills"

# Parse flags
DO_BUILD=true
DO_SKILLS=true
if [[ "${1:-}" == "--skills" ]]; then
    DO_BUILD=false
elif [[ "${1:-}" == "--build" ]]; then
    DO_SKILLS=false
fi

# ── Build ──────────────────────────────────────────────────────────────────────

if $DO_BUILD; then
    echo "Building squall (release, global-memory enabled)..."
    cd "$SQUALL_DIR"
    cargo build --release 2>&1

    # Install binary
    mkdir -p "$(dirname "$INSTALL_BIN")"
    cp target/release/squall "$INSTALL_BIN"
    chmod +x "$INSTALL_BIN"
    echo "Installed binary to $INSTALL_BIN"

    # Register as global MCP server (user scope → ~/.claude.json)
    # This is idempotent — overwrites existing squall entry.
    #
    # API keys: The installer preserves any keys already set in ~/.claude.json.
    # If squall is not yet registered, you'll need to set env vars manually:
    #   claude mcp update squall -e XAI_API_KEY=your_key -e OPENROUTER_API_KEY=your_key
    #
    # Check if squall is already registered (preserve env vars)
    if command -v claude &>/dev/null; then
        EXISTING_ENV=""
        if command -v python3 &>/dev/null && [ -f "${HOME}/.claude.json" ]; then
            EXISTING_ENV=$(python3 -c "
import json, sys
try:
    with open('${HOME}/.claude.json') as f:
        cfg = json.load(f)
    env = cfg.get('mcpServers', {}).get('squall', {}).get('env', {})
    for k, v in env.items():
        print(f'-e {k}={v}')
except:
    pass
" 2>/dev/null || true)
        fi

        echo "Registering squall as global MCP server..."
        # Remove existing entry first (claude mcp add fails if it exists)
        claude mcp remove squall 2>/dev/null || true
        # shellcheck disable=SC2086
        claude mcp add --scope user --transport stdio squall "$INSTALL_BIN" $EXISTING_ENV
        echo "Registered squall in ~/.claude.json"
    else
        echo "Warning: 'claude' CLI not found — skipping MCP registration."
        echo "Run manually: claude mcp add --scope user --transport stdio squall $INSTALL_BIN"
    fi
fi

# ── Skills ─────────────────────────────────────────────────────────────────────

if $DO_SKILLS; then
    echo "Installing global skills to $SKILLS_DIR..."
    mkdir -p "$SKILLS_DIR"

    # Squall skills to install globally
    SQUALL_SKILLS=(
        squall-unified-review
        squall-research
        squall-deep-research
        squall-review
        squall-deep-review
    )

    for skill in "${SQUALL_SKILLS[@]}"; do
        src="${SQUALL_DIR}/.claude/skills/${skill}/SKILL.md"
        if [ -f "$src" ]; then
            mkdir -p "${SKILLS_DIR}/${skill}"
            cp "$src" "${SKILLS_DIR}/${skill}/SKILL.md"
            echo "  Installed skill: ${skill}"
        else
            echo "  Skipped (not found): ${skill}"
        fi
    done

    echo "Skills installed. Available as slash commands in all projects."
fi

# ── Summary ────────────────────────────────────────────────────────────────────

echo ""
echo "Done. Squall is ready."
if $DO_BUILD; then
    echo "  Binary:  $INSTALL_BIN"
    echo "  MCP:     ~/.claude.json (user scope)"
fi
if $DO_SKILLS; then
    echo "  Skills:  $SKILLS_DIR/squall-*"
fi
echo ""
echo "Verify: claude mcp list | grep squall"
