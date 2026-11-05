#!/bin/zsh -l
# Build squall from origin/main and install it everywhere it runs:
#
#   MBP    david  ~/.local/bin/squall                (copied file; Claude Code's MCP entry)
#   Studio david  ~/.local/bin/squall                (copied file)
#   Studio ori    /opt/orchid/ori/.local/bin/squall  (copied, sudo, ori:orchid-svc)
#
# The MBP copy used to be a symlink into the dev checkout. The weekly launchd job can't
# write to ~/Documents, so it is a real file now. To try an uncommitted dev build in
# Claude Code: install -m 755 target/release/squall ~/.local/bin/squall
#
# Each install is an atomic replace, so running MCP servers keep the old binary and the
# next Claude session picks up the new one. No daemon restart is needed for ori.
# Keeps the 3 newest backups it made next to each copy.
#
# Usage: scripts/deploy-squall.sh            (deploy origin/main)
# Env:   SQUALL_STUDIO_HOST (default: studio)
set -euo pipefail

REPO="${0:A:h:h}"
STUDIO="${SQUALL_STUDIO_HOST:-studio}"
STAMP=$(date +%Y%m%d-%H%M)
ORI_BIN=/opt/orchid/ori/.local/bin/squall
MBP_BIN="$HOME/.local/bin/squall"
# Keep the 3 newest backups THIS script made (bak-YYYYMMDD-HHMM, sorted by name since
# cp -p preserves the old mtime). Hand-made backups like bak-pre021 are never touched.
# Runs under sh so a no-match glob is harmless.
PRUNE='ls -1r "$0".bak-2[0-9][0-9][0-9][0-9][0-9][0-9][0-9]-[0-9][0-9][0-9][0-9] 2>/dev/null | tail -n +4 | while read -r f; do rm -f "$f"; done'
STATE="$REPO/.squall/model-audit/deployed-sha"
SSH=(ssh -o BatchMode=yes -o ConnectTimeout=15 "$STUDIO")

cd "$REPO"
git fetch -q origin main
SHA=$(git rev-parse origin/main)
echo "deploying origin/main ${SHA:0:8}"

# Every roster key must show up in listmodels for the local build (real env loaded).
EXPECT=()
for key in $(python3 -I -c '
import re, tomllib, sys
src = open(sys.argv[1]).read()
toml = re.search(r"const BUILTIN_DEFAULTS: &str = r#\"(.*?)\"#;", src, re.S).group(1)
print("\n".join(tomllib.loads(toml)["models"]))' <(git show origin/main:src/config.rs)); do
  EXPECT+=(--expect "$key")
done

# --- build -------------------------------------------------------------------
# Build in this checkout only when it is clean and on main (fast-forwarded first); never
# disturb work in progress. Otherwise build origin/main in a scratch worktree.
branch=$(git symbolic-ref --short -q HEAD || true)
if [[ "$branch" == main ]] && git diff --quiet && git diff --cached --quiet \
   && git merge -q --ff-only origin/main 2>/dev/null; then
  cargo build --release -q
  BIN="$REPO/target/release/squall"
else
  WT=$(mktemp -d)/squall-deploy
  git worktree add -q --detach "$WT" origin/main
  trap 'git -C "$REPO" worktree remove --force "$WT"' EXIT
  (cd "$WT" && CARGO_TARGET_DIR="$REPO/target/deploy" cargo build --release -q)
  BIN="$REPO/target/deploy/release/squall"
fi
HASH=$(shasum -a 256 "$BIN" | cut -d' ' -f1)
python3 -I scripts/mcp-smoke.py "$BIN" "${EXPECT[@]}"

# --- install on the MBP ---------------------------------------------------------
mkdir -p "${MBP_BIN:h}"
if [[ -f "$MBP_BIN" && ! -L "$MBP_BIN" ]] && ! cmp -s "$BIN" "$MBP_BIN"; then
  cp -p "$MBP_BIN" "$MBP_BIN.bak-$STAMP"
fi
install -S -m 755 "$BIN" "$MBP_BIN"   # -S renames over the target, replacing an old symlink
sh -c "$PRUNE" "$MBP_BIN"

# --- ship to Studio ------------------------------------------------------------
TMP="/tmp/squall-deploy-${SHA:0:8}"
scp -q -o BatchMode=yes "$BIN" "$STUDIO:$TMP"
"${SSH[@]}" zsh -s -- "$TMP" "$STAMP" "$ORI_BIN" <<'REMOTE'
set -euo pipefail
TMP=$1 STAMP=$2 ORI_BIN=$3
# Keep the 3 newest backups THIS script made (bak-YYYYMMDD-HHMM, sorted by name since
# cp -p preserves the old mtime). Hand-made backups like bak-pre021 are never touched.
# Runs under sh so a no-match glob is harmless, and under sudo for ori because david
# cannot list ori's bin directory.
PRUNE='ls -1r "$0".bak-2[0-9][0-9][0-9][0-9][0-9][0-9][0-9]-[0-9][0-9][0-9][0-9] 2>/dev/null | tail -n +4 | while read -r f; do rm -f "$f"; done'

mkdir -p ~/.local/bin
# Back up only when the binary actually changes, so re-runs don't pile up copies.
if [[ -f ~/.local/bin/squall ]] && ! cmp -s "$TMP" ~/.local/bin/squall; then
  cp -p ~/.local/bin/squall ~/.local/bin/squall.bak-$STAMP
fi
install -S -m 755 "$TMP" ~/.local/bin/squall
sh -c "$PRUNE" ~/.local/bin/squall

if ! sudo -n cmp -s "$TMP" "$ORI_BIN"; then sudo -n cp -p "$ORI_BIN" "$ORI_BIN.bak-$STAMP"; fi
sudo -n install -S -o ori -g orchid-svc -m 755 "$TMP" "$ORI_BIN"
sudo -n sh -c "$PRUNE" "$ORI_BIN"
rm -f "$TMP"
REMOTE

# --- verify ------------------------------------------------------------------
fail=0
[[ "$(shasum -a 256 "$MBP_BIN" | cut -d' ' -f1)" == "$HASH" ]] \
  || { echo "FAIL hash mismatch on MBP:$MBP_BIN"; fail=1; }
# cwd is the repo, whose .env supplies keys, so the full roster must be listed.
python3 -I scripts/mcp-smoke.py "$MBP_BIN" "${EXPECT[@]}" || fail=1
for target in "~/.local/bin/squall" "$ORI_BIN"; do
  remote_hash=$("${SSH[@]}" "sudo -n shasum -a 256 $target" | cut -d' ' -f1)
  [[ "$remote_hash" == "$HASH" ]] || { echo "FAIL hash mismatch on $STUDIO:$target"; fail=1; }
done
# The hash proves the roster; this proves the binary starts and speaks MCP there. No
# --expect: a bare SSH session has no API keys or CLIs, so listmodels lists nothing.
"${SSH[@]}" "cd /tmp && python3 - ~/.local/bin/squall" < scripts/mcp-smoke.py || fail=1
"${SSH[@]}" "cd /tmp && sudo -n -H -u ori python3 - $ORI_BIN" < scripts/mcp-smoke.py || fail=1
(( fail )) && { echo "deploy verification FAILED"; exit 1; }

mkdir -p "${STATE:h}"
echo "$SHA" > "$STATE"
echo "deployed ${SHA:0:8}: MBP + Studio david + Studio ori installed and verified"
