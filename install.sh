#!/usr/bin/env bash
set -euo pipefail

# Squall installer — builds binary, registers global MCP server, installs global skills.
#
# Prerequisites: cp .env.example .env && fill in your API keys
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

# ── Prerequisite: .env must exist with API keys ───────────────────────────────

ENV_FILE="${SQUALL_DIR}/.env"
if [ ! -f "$ENV_FILE" ]; then
    echo "Error: No .env file found."
    echo ""
    echo "  cp .env.example .env"
    echo "  # Then fill in your API keys. See .env.example for signup links."
    echo ""
    exit 1
fi

# Count non-empty API keys
KEY_COUNT=$(grep -cE '^[A-Z_]+_API_KEY=.+' "$ENV_FILE" 2>/dev/null || echo "0")
if [ "$KEY_COUNT" -eq 0 ]; then
    echo "Error: .env exists but has no API keys set."
    echo ""
    echo "  Fill in at least one *_API_KEY in .env"
    echo "  See .env.example for required keys and signup links."
    echo ""
    exit 1
fi
echo "Found $KEY_COUNT API key(s) in .env"

# ── Build ──────────────────────────────────────────────────────────────────────

if $DO_BUILD; then
    echo "Building squall (release, global-memory enabled)..."
    cd "$SQUALL_DIR"
    cargo build --release 2>&1

    # Install binary (symlink avoids macOS com.apple.provenance SIGKILL on copied binaries)
    mkdir -p "$(dirname "$INSTALL_BIN")"
    ln -sf "${SQUALL_DIR}/target/release/squall" "$INSTALL_BIN"
    echo "Installed binary to $INSTALL_BIN -> target/release/squall"

    # Register as global MCP server (user scope → ~/.claude.json).
    # SECURITY: API keys are written directly to the JSON config file, never
    # passed as CLI args (which would be visible in `ps aux`).
    if command -v claude &>/dev/null; then
        echo "Registering squall as global MCP server..."
        # Remove existing entry first (claude mcp add fails if it exists)
        claude mcp remove squall 2>/dev/null || true
        # Register binary path only — no -e flags (keys injected below)
        claude mcp add --scope user --transport stdio squall "$INSTALL_BIN"

        # Inject API keys directly into ~/.claude.json (avoids process-list leakage)
        if command -v python3 &>/dev/null; then
            python3 -c "
import json, os, sys

claude_cfg = os.path.expanduser('~/.claude.json')
if not os.path.exists(claude_cfg):
    print('Warning: ~/.claude.json not found after mcp add — keys not injected', file=sys.stderr)
    sys.exit(0)

# Read .env
env_keys = {}
with open('${ENV_FILE}') as f:
    for line in f:
        line = line.strip()
        if not line or line.startswith('#') or '=' not in line:
            continue
        k, v = line.split('=', 1)
        k, v = k.strip(), v.strip()
        if k.endswith('_API_KEY') and v:
            env_keys[k] = v

# Read config
with open(claude_cfg) as f:
    cfg = json.load(f)

# Navigate to squall's env (create if absent)
servers = cfg.setdefault('mcpServers', {})
squall = servers.setdefault('squall', {})
existing_env = squall.get('env', {})

# Merge: preserve non-API-key vars, .env API keys take precedence
merged = {k: v for k, v in existing_env.items() if not k.endswith('_API_KEY')}
merged.update(env_keys)
squall['env'] = merged

# Write back atomically (temp + rename)
tmp = claude_cfg + '.tmp'
with open(tmp, 'w') as f:
    json.dump(cfg, f, indent=2)
    f.write('\n')
os.rename(tmp, claude_cfg)
print(f'Injected {len(env_keys)} API key(s) into ~/.claude.json')
" 2>/dev/null || echo "Warning: Python3 key injection failed — add keys manually to ~/.claude.json"
        else
            echo "Warning: python3 not found — API keys not injected."
            echo "Add keys manually to ~/.claude.json under mcpServers.squall.env"
        fi
        echo "Registered squall in ~/.claude.json ($KEY_COUNT API keys)"
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
    echo "  MCP:     ~/.claude.json (user scope, $KEY_COUNT API keys)"
fi
if $DO_SKILLS; then
    echo "  Skills:  $SKILLS_DIR/squall-*"
fi
echo ""
echo "Verify: claude mcp list | grep squall"
echo "Restart Claude Code to pick up the new binary."
