#!/bin/bash
# Captures the product screenshots in light and dark, then composes the exports.
#   branding/screenshots/tools/capture.sh [toolbox] [python]
# toolbox: a toolbox with gnome-shell, gjs and the drainscope packages to show (default
# drainscope-f44). python: an interpreter with source/requirements.txt (default python3).
# Runs a headless shell per scheme on scratch XDG directories: the real session, its daemon and
# its database are not touched. Never run it while another nested or headless shell runs.
set -eu
TOOLBOX=${1:-drainscope-f44}
PYTHON=${2:-python3}
TOOLS=$(cd "$(dirname "$0")" && pwd)
WORK=$(mktemp -d /tmp/drainscope-capture.XXXX)
for scheme in default prefer-dark; do
  toolbox run -c "$TOOLBOX" env PLAN="$TOOLS/plan.json" SCHEME=$scheme TIMEOUT=150 \
    "$TOOLS/session.sh" "$WORK/$scheme" > "$WORK/$scheme.log" 2>&1
  [ -e "$WORK/$scheme/done" ] || { echo "capture ($scheme) did not finish: see $WORK/$scheme/shell.log"; exit 1; }
  if grep -q 'JS ERROR' "$WORK/$scheme/shell.log"; then
    grep 'JS ERROR' "$WORK/$scheme/shell.log"
    exit 1
  fi
  echo "captured $scheme"
done
"$PYTHON" -I "$TOOLS/compose.py" "$WORK/default/shots" "$WORK/prefer-dark/shots"
rm -rf "$WORK"
