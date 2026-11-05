#!/bin/zsh
# Install (or refresh) the weekly squall-latest launchd job. Idempotent: re-run any time,
# and re-run after rotating API keys in .env.
#
# Why a separate clone: launchd jobs cannot read ~/Documents (macOS privacy protection:
# "Operation not permitted"), and Full Disk Access cannot be granted to a script or a
# bare binary. So the job runs entirely from its own clone outside ~/Documents:
#
#   ~/.squall-latest/squall        machine-owned clone, reset to origin/main every run
#                                  (anything edited there is discarded)
#   ~/.squall-latest/squall/.env   copy of this repo's .env (mode 600)
#   ~/.squall-latest/logs/         launchd stdout/stderr
#   ~/.squall-latest/squall/.squall/model-audit/   audit reports + cron.log
#
# The job deploys to the MBP (~/.local/bin/squall) + Studio david + ori. It never
# touches this checkout.
set -euo pipefail

SRC="${0:A:h:h}"
BASE="$HOME/.squall-latest"
CLONE="$BASE/squall"
LABEL=com.squall.latest-weekly
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"

mkdir -p "$BASE/logs"
if [[ ! -d "$CLONE/.git" ]]; then
  git clone -q "$(git -C "$SRC" remote get-url origin)" "$CLONE"
fi
# Unattended pushes can't rely on the interactive credential cache; gh's token lives in
# the login keychain, which a launchd agent in the GUI session can read.
git -C "$CLONE" config credential.helper ""
git -C "$CLONE" config --add credential.helper '!gh auth git-credential'
git -C "$CLONE" fetch -q origin main
git -C "$CLONE" checkout -q -B main origin/main
git -C "$CLONE" reset -q --hard origin/main

install -m 600 "$SRC/.env" "$CLONE/.env"
# Carry the last deployed SHA over so the first run doesn't redeploy an unchanged main.
if [[ -f "$SRC/.squall/model-audit/deployed-sha" ]]; then
  mkdir -p "$CLONE/.squall/model-audit"
  cp "$SRC/.squall/model-audit/deployed-sha" "$CLONE/.squall/model-audit/deployed-sha"
fi

sed -e "s#__CLONE__#$CLONE#g" -e "s#__LOGS__#$BASE/logs#g" \
  "$SRC/scripts/launchd/$LABEL.plist" > "$PLIST"
plutil -lint -s "$PLIST"
launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$PLIST"
echo "installed $LABEL -> $CLONE ($(git -C "$CLONE" rev-parse --short HEAD))"
echo "run now: launchctl kickstart gui/$(id -u)/$LABEL"
