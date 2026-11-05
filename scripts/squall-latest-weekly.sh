#!/bin/zsh -l
# Weekly unattended model-roster refresh. Run by launchd (scripts/launchd/).
#
# 1. Audit the roster against live catalogs (scripts/model-audit.py).
# 2. Exit 0 from the audit = nothing to do: stop.
# 3. Otherwise run the squall-latest skill headless in a throwaway worktree on
#    branch model-refresh/<date>. The skill opens a PR; it never touches main
#    or your working checkout.
#
# Logs: .squall/model-audit/cron.log
# Test without launching Claude: SQUALL_LATEST_DRY_RUN=1 scripts/squall-latest-weekly.sh
# Widen the new-model window:    SQUALL_LATEST_SINCE=2026-10-13 ...
set -uo pipefail

REPO="${0:A:h:h}"
DATE=$(date +%F)
LOG_DIR="$REPO/.squall/model-audit"
LOG="$LOG_DIR/cron.log"
WT="$REPO/../.squall-latest-worktree"
BRANCH="model-refresh/$DATE"
mkdir -p "$LOG_DIR"
exec >>"$LOG" 2>&1
echo "=== $(date -Iseconds) squall-latest weekly"

cd "$REPO" || exit 1
git fetch -q origin main || { echo "fetch failed"; exit 1; }

audit_args=()
[[ -n "${SQUALL_LATEST_SINCE:-}" ]] && audit_args=(--since "$SQUALL_LATEST_SINCE")
python3 -I scripts/model-audit.py "${audit_args[@]}"
rc=$?
REPORT="$LOG_DIR/$DATE.md"
case $rc in
  0) echo "roster healthy, nothing new; done"; exit 0 ;;
  10|20) echo "audit exit $rc; launching skill" ;;
  *) echo "audit crashed (exit $rc)"; exit $rc ;;
esac

if git ls-remote --exit-code --heads origin "$BRANCH" >/dev/null; then
  echo "branch $BRANCH already exists on origin; skipping"
  exit 0
fi

git worktree remove --force "$WT" 2>/dev/null
git worktree add -q -B "$BRANCH" "$WT" origin/main || exit 1
ln -sf "$REPO/.env" "$WT/.env"
# The skill pushes the branch, so the local copy is disposable.
trap 'cd "$REPO" && git worktree remove --force "$WT"; git branch -q -D "$BRANCH" 2>/dev/null' EXIT

cd "$WT" || exit 1
if [[ -n "${SQUALL_LATEST_DRY_RUN:-}" ]]; then
  echo "dry run: worktree $WT on $BRANCH ready; would launch claude now"
  exit 0
fi
claude -p "Run the squall-latest skill in UNATTENDED mode. The audit already ran \
(exit $rc); read the report at $REPORT instead of re-running it. You are on branch \
$BRANCH in a throwaway worktree. If no roster change is warranted, change nothing and \
stop. Otherwise make the changes, pass ./scripts/pre-commit.sh, commit, push the branch, \
and open a PR with gh whose body lists what changed, what was probed, and candidates \
you deliberately did not adopt." \
  --permission-mode bypassPermissions
echo "claude exit $?"
