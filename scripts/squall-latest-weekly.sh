#!/bin/zsh -l
# Weekly unattended model-roster refresh + deploy. Run by launchd (scripts/launchd/).
#
# 1. Audit the roster against live catalogs (scripts/model-audit.py).
# 2. Audit exit 10/20 (new entries / dead model): run the squall-latest skill headless
#    in a throwaway worktree. It commits and pushes main only after pre-commit passes.
# 3. If origin/main differs from what was last deployed (this run's change, or anything
#    else merged since), rebuild and install on MBP + Studio david + Studio ori
#    (scripts/deploy-squall.sh).
#
# Never touches your working checkout unless it is clean and on main (then it is
# fast-forwarded and rebuilt in place, which is what updates the MBP binary).
#
# Logs: .squall/model-audit/cron.log   Failures also raise a macOS notification.
# Test without launching Claude or deploying: SQUALL_LATEST_DRY_RUN=1 scripts/squall-latest-weekly.sh
# Widen the new-model window:                 SQUALL_LATEST_SINCE=2026-10-13 ...
set -uo pipefail

REPO="${0:A:h:h}"
DATE=$(date +%F)
LOG_DIR="$REPO/.squall/model-audit"
LOG="$LOG_DIR/cron.log"
STATE="$LOG_DIR/deployed-sha"
WT="$REPO/../.squall-latest-worktree"
BRANCH="model-refresh/$DATE"
mkdir -p "$LOG_DIR"
exec >>"$LOG" 2>&1
echo "=== $(date -Iseconds) squall-latest weekly"

notify() {
  echo "NOTIFY: $1"
  osascript -e "display notification \"$1\" with title \"squall-latest\"" 2>/dev/null
}

cd "$REPO" || exit 1
git fetch -q origin main || { notify "git fetch failed"; exit 1; }

# --- 1. audit -------------------------------------------------------------------
audit_args=()
[[ -n "${SQUALL_LATEST_SINCE:-}" ]] && audit_args=(--since "$SQUALL_LATEST_SINCE")
python3 -I scripts/model-audit.py "${audit_args[@]}"
rc=$?
REPORT="$LOG_DIR/$DATE.md"
case $rc in
  0) echo "roster healthy, nothing new" ;;
  10|20) echo "audit exit $rc; launching skill" ;;
  *) notify "model audit crashed (exit $rc)"; exit $rc ;;
esac

# --- 2. update roster -----------------------------------------------------------------
if (( rc != 0 )); then
  git worktree remove --force "$WT" 2>/dev/null
  git worktree add -q -B "$BRANCH" "$WT" origin/main || { notify "worktree failed"; exit 1; }
  ln -sf "$REPO/.env" "$WT/.env"
  if [[ -n "${SQUALL_LATEST_DRY_RUN:-}" ]]; then
    echo "dry run: worktree $WT on $BRANCH ready; would launch claude now"
  else
    (cd "$WT" && claude -p "Run the squall-latest skill in UNATTENDED mode. The audit \
already ran (exit $rc); read the report at $REPORT instead of re-running it. You are in a \
throwaway worktree of origin/main. If no roster change is warranted, change nothing and \
stop. Otherwise make the changes, pass ./scripts/pre-commit.sh, commit, and push with \
'git push origin HEAD:main' (if rejected: rebase onto origin/main, re-run pre-commit, push \
again). Do not deploy; the wrapper does that. End with a short summary of what changed, \
what was probed, and candidates you deliberately did not adopt." \
      --permission-mode bypassPermissions)
    claude_rc=$?
    echo "claude exit $claude_rc"
    (( claude_rc != 0 )) && notify "roster update run failed (exit $claude_rc); see cron.log"
  fi
  git worktree remove --force "$WT"
  git branch -q -D "$BRANCH" 2>/dev/null
fi

# --- 3. deploy ------------------------------------------------------------------
git fetch -q origin main
head=$(git rev-parse origin/main)
deployed=$(cat "$STATE" 2>/dev/null || echo none)
if [[ "$head" == "$deployed" ]]; then
  echo "origin/main ${head:0:8} already deployed"
elif [[ -n "${SQUALL_LATEST_DRY_RUN:-}" ]]; then
  echo "dry run: would deploy ${head:0:8} (last deployed ${deployed:0:8})"
else
  if scripts/deploy-squall.sh; then
    notify "deployed squall ${head:0:8} to MBP + Studio (david, ori)"
  else
    notify "deploy of ${head:0:8} FAILED; see cron.log"
    exit 1
  fi
fi
