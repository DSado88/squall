#!/bin/zsh -l
# Build squall from origin/main and install it everywhere it runs:
#
#   MBP    david  ~/.local/bin/squall                (copied file; Claude Code + Codex MCP entries)
#   Studio david  ~/.local/bin/squall                (copied file)
#   Studio ori    /opt/orchid/ori/.local/bin/squall  (copied, sudo, ori:orchid-svc)
#
# Nothing is installed anywhere until every check passes: the full pre-commit gate
# (fmt, clippy, tests) on the exact commit, an MCP smoke test of the build listing the
# whole roster, and a hash check + smoke test of the staged copy on the Studio as both
# david and ori.
#
# The MBP copy used to be a symlink into the dev checkout. The weekly launchd job can't
# write to ~/Documents, so it is a real file now. To try an uncommitted dev build in
# Claude Code or Codex: install -m 755 target/release/squall ~/.local/bin/squall
# Codex's entry has no keys in its env; it sets cwd to this repo so the repo .env loads.
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
# ServerAlive* turns a stalled link into an error instead of a hang.
SSH_OPTS=(-o BatchMode=yes -o ConnectTimeout=15 -o ServerAliveInterval=15 -o ServerAliveCountMax=4)
SSH=(ssh "${SSH_OPTS[@]}" "$STUDIO")

WT="" STAGE=""
cleanup() {
  [[ -n "$WT" ]] && git -C "$REPO" worktree remove --force "$WT" 2>/dev/null
  [[ -n "$STAGE" ]] && "${SSH[@]}" "rm -rf '$STAGE'" 2>/dev/null
  return 0
}
trap cleanup EXIT

cd "$REPO"
git -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=60 fetch -q origin main
SHA=$(git rev-parse origin/main)
echo "deploying origin/main ${SHA:0:8}"

# Every roster key must show up in listmodels for the local build (real env loaded).
# Assignment form, so a failed extraction stops the deploy instead of checking nothing.
keys=$(git show "${SHA}:src/config.rs" | python3 -I scripts/model-audit.py --roster-keys -)
[[ -n "$keys" ]] || { echo "no roster keys found in $SHA; refusing to deploy"; exit 1; }
EXPECT=()
for key in ${(f)keys}; do EXPECT+=(--expect "$key"); done

# --- build + gate ------------------------------------------------------------------
# Build in this checkout only when it is clean, on main, and exactly at SHA after a
# fast-forward; never disturb work in progress or ship unpushed local commits.
branch=$(git symbolic-ref --short -q HEAD || true)
if [[ "$branch" == main ]] && git diff --quiet && git diff --cached --quiet \
   && git merge -q --ff-only "$SHA" 2>/dev/null && [[ "$(git rev-parse HEAD)" == "$SHA" ]]; then
  BUILD_DIR="$REPO"
  BIN="$REPO/target/release/squall"
  unset CARGO_TARGET_DIR
else
  WT=$(mktemp -d)/squall-deploy
  git worktree add -q --detach "$WT" "$SHA"
  BUILD_DIR="$WT"
  export CARGO_TARGET_DIR="$REPO/target/deploy"
  BIN="$CARGO_TARGET_DIR/release/squall"
fi
echo "running the pre-commit gate on ${SHA:0:8}"
(cd "$BUILD_DIR" && ./scripts/pre-commit.sh) \
  || { echo "pre-commit gate failed on ${SHA:0:8}; nothing installed"; exit 1; }
(cd "$BUILD_DIR" && cargo build --release -q)
HASH=$(shasum -a 256 "$BIN" | cut -d' ' -f1)
python3 -I scripts/mcp-smoke.py "$BIN" "${EXPECT[@]}"

# --- stage + verify on the Studio (nothing installed yet) ----------------------------
# mktemp: an unpredictable, david-owned dir, so no other account can pre-plant or swap
# the binary between upload and install. 755 so ori can run the smoke test from it.
STAGE=$("${SSH[@]}" 'd=$(mktemp -d /tmp/squall-deploy.XXXXXX) && chmod 755 "$d" && echo "$d"')
[[ "$STAGE" == /tmp/squall-deploy.* ]] || { echo "bad staging dir from $STUDIO: $STAGE"; exit 1; }
scp -q "${SSH_OPTS[@]}" "$BIN" "$STUDIO:$STAGE/squall"
scp -q "${SSH_OPTS[@]}" scripts/mcp-smoke.py "$STUDIO:$STAGE/mcp-smoke.py"
"${SSH[@]}" zsh -s -- "$STAGE" "$HASH" <<'VERIFY'
set -euo pipefail
STAGE=$1 HASH=$2
chmod 755 "$STAGE/squall"; chmod 644 "$STAGE/mcp-smoke.py"
[[ "$(shasum -a 256 "$STAGE/squall" | cut -d' ' -f1)" == "$HASH" ]] \
  || { echo "staged binary hash mismatch"; exit 1; }
# No --expect: a bare SSH session has no API keys or CLIs, so listmodels lists nothing.
cd /tmp
python3 "$STAGE/mcp-smoke.py" "$STAGE/squall"
sudo -n -H -u ori python3 "$STAGE/mcp-smoke.py" "$STAGE/squall"
VERIFY

# --- install -------------------------------------------------------------------------
mkdir -p "${MBP_BIN:h}"
if [[ -f "$MBP_BIN" && ! -L "$MBP_BIN" ]] && ! cmp -s "$BIN" "$MBP_BIN"; then
  cp -p "$MBP_BIN" "$MBP_BIN.bak-$STAMP"
fi
install -S -m 755 "$BIN" "$MBP_BIN"   # -S renames over the target, replacing an old symlink
sh -c "$PRUNE" "$MBP_BIN"

"${SSH[@]}" zsh -s -- "$STAGE" "$STAMP" "$ORI_BIN" <<'INSTALL'
set -euo pipefail
STAGE=$1 STAMP=$2 ORI_BIN=$3
NEW="$STAGE/squall"
PRUNE='ls -1r "$0".bak-2[0-9][0-9][0-9][0-9][0-9][0-9][0-9]-[0-9][0-9][0-9][0-9] 2>/dev/null | tail -n +4 | while read -r f; do rm -f "$f"; done'

mkdir -p ~/.local/bin
# Back up only when the binary actually changes, so re-runs don't pile up copies.
if [[ -f ~/.local/bin/squall ]] && ! cmp -s "$NEW" ~/.local/bin/squall; then
  cp -p ~/.local/bin/squall ~/.local/bin/squall.bak-$STAMP
fi
install -S -m 755 "$NEW" ~/.local/bin/squall
sh -c "$PRUNE" ~/.local/bin/squall

# test -f first: on a first install there is nothing to back up, and cmp would fail.
if sudo -n test -f "$ORI_BIN" && ! sudo -n cmp -s "$NEW" "$ORI_BIN"; then
  sudo -n cp -p "$ORI_BIN" "$ORI_BIN.bak-$STAMP"
fi
sudo -n install -S -o ori -g orchid-svc -m 755 "$NEW" "$ORI_BIN"
# Under sudo because david cannot list ori's bin directory.
sudo -n sh -c "$PRUNE" "$ORI_BIN"
INSTALL

# --- verify --------------------------------------------------------------------------
fail=0
[[ "$(shasum -a 256 "$MBP_BIN" | cut -d' ' -f1)" == "$HASH" ]] \
  || { echo "FAIL hash mismatch on MBP:$MBP_BIN"; fail=1; }
# cwd is the repo, whose .env supplies keys, so the full roster must be listed.
python3 -I scripts/mcp-smoke.py "$MBP_BIN" "${EXPECT[@]}" || fail=1
for target in "~/.local/bin/squall" "$ORI_BIN"; do
  remote_hash=$("${SSH[@]}" "sudo -n shasum -a 256 $target" | cut -d' ' -f1)
  [[ "$remote_hash" == "$HASH" ]] || { echo "FAIL hash mismatch on $STUDIO:$target"; fail=1; }
done
(( fail )) && { echo "deploy verification FAILED"; exit 1; }

mkdir -p "${STATE:h}"
echo "$SHA" > "$STATE"
echo "deployed ${SHA:0:8}: MBP + Studio david + Studio ori installed and verified"
