#!/usr/bin/env bash
#
# Launch cide, replacing any instance already running.
#
# Every launch during development is a window on someone's desktop, and a loop that starts
# one without stopping the last leaves them stacked up. This stops the previous instance
# first, waits for it to actually go, and reaps any `claude` children it orphaned.
#
# It also refuses to start when the saved workspace would open an unreasonable number of
# windows — the restore path caps this too, but finding out before a window appears is
# better than finding out after.
#
#   ./run.sh                      # normal launch
#   ./run.sh --bench              # IPC transport gate (M0), prints and exits
#   ./run.sh --audit-chrome       # chrome vs the design mock (M3), prints and exits
#   ./run.sh --audit-panes        # pane host registry under churn (M4)
#   ./run.sh --audit-windows      # detach/re-dock and window modes (M5)
#   ./run.sh --fresh              # start from an empty workspace
#   ./run.sh --on-top             # keep the window above others, for screenshots
#
set -uo pipefail

cd "$(dirname "$0")"

BIN=./target/debug/cide
WORKSPACE="${XDG_STATE_HOME:-$HOME/.local/state}/cide/workspace.json"
MAX_WINDOWS=8

env_flags=()
fresh=0

for arg in "$@"; do
  case "$arg" in
    --bench)          env_flags+=(CIDE_BENCH=1) ;;
    --audit-chrome)   env_flags+=(CIDE_AUDIT=1) ;;
    --audit-panes)    env_flags+=(CIDE_AUDIT_PANES=1) ;;
    --audit-windows)  env_flags+=(CIDE_AUDIT_WINDOWS=1) ;;
    --on-top)         env_flags+=(CIDE_ON_TOP=1) ;;
    --fresh)          fresh=1 ;;
    *) echo "unknown option: $arg" >&2; sed -n '15,23p' "$0" >&2; exit 2 ;;
  esac
done

# --- stop whatever is already running -------------------------------------------------

# Matched on the exact binary path rather than the word "cide", which would also match this
# script, an editor holding the source, and the agent session that started it.
mapfile -t running < <(pgrep -f "^${BIN}$" 2>/dev/null || true)

if [ "${#running[@]}" -gt 0 ]; then
  echo "[run] stopping ${#running[@]} running instance(s): ${running[*]}"
  # SIGTERM, not SIGKILL: the app's own handler flushes the workspace and brings its
  # children down through SIGHUP -> SIGTERM -> SIGKILL, which is what lets a `claude`
  # finish writing the transcript that makes its conversation resumable.
  kill -TERM "${running[@]}" 2>/dev/null

  for _ in $(seq 1 40); do
    pgrep -f "^${BIN}$" >/dev/null 2>&1 || break
    sleep 0.25
  done

  if pgrep -f "^${BIN}$" >/dev/null 2>&1; then
    echo "[run] did not exit on SIGTERM; sending SIGKILL"
    pkill -KILL -f "^${BIN}$" 2>/dev/null
    sleep 1
  fi
fi

# A `claude` reparented to init is one the app failed to reap. Left alone they accumulate
# across launches, each holding a PTY and a model connection.
mapfile -t orphans < <(ps -eo ppid=,pid=,comm= | awk '$1 == 1 && $3 == "claude" { print $2 }')
if [ "${#orphans[@]}" -gt 0 ]; then
  echo "[run] reaping ${#orphans[@]} orphaned claude process(es)"
  kill -TERM "${orphans[@]}" 2>/dev/null
fi

# --- sanity-check what the workspace will do ------------------------------------------

if [ "$fresh" = 1 ]; then
  if [ -f "$WORKSPACE" ]; then
    mv "$WORKSPACE" "$WORKSPACE.bak"
    echo "[run] --fresh: previous workspace moved to $WORKSPACE.bak"
  fi
elif [ -f "$WORKSPACE" ]; then
  windows=$(python3 -c "
import json,sys
try:
    print(len(json.load(open('$WORKSPACE'))['windows']))
except Exception:
    print(0)
" 2>/dev/null || echo 0)
  if [ "$windows" -gt "$MAX_WINDOWS" ]; then
    echo "[run] REFUSING: the saved workspace records $windows windows (cap $MAX_WINDOWS)."
    echo "[run] Opening it would put that many windows on screen."
    echo "[run] Inspect it with:  ./target/debug/cide-headless tree"
    echo "[run] Or start clean:   ./run.sh --fresh"
    exit 1
  fi
fi

# --- launch ----------------------------------------------------------------------------

if [ ! -x "$BIN" ]; then
  echo "[run] $BIN is missing; run: cargo build -p cide-app" >&2
  exit 1
fi

echo "[run] starting ${env_flags[*]:-} $BIN"
exec env "${env_flags[@]}" "$BIN"
