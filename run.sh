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
# A DEBUG build loads its frontend from the Vite dev server, not from `ui/dist`.
# `tauri.conf.json` sets `devUrl: http://localhost:1420`, and that URL is what the debug
# binary opens. Running it with nothing on 1420 is the white window with "Could not connect
# to localhost" — the app is fine, it simply has no document. So this script starts Vite and
# waits for the port before launching, and stops it again on the way out.
#
# `--release` skips all of that: a release build embeds `frontendDist`, so it needs no server.
#
#   ./run.sh                      # normal launch (starts Vite if it is not already up)
#   ./run.sh --release            # build and run the release binary; no dev server
#   ./run.sh --bench              # IPC transport gate (M0), prints and exits
#   ./run.sh --audit-chrome       # chrome vs the design mock (M3), prints and exits
#   ./run.sh --audit-panes        # pane host registry under churn (M4)
#   ./run.sh --audit-windows      # detach/re-dock and window modes (M5)
#   ./run.sh --input-probe        # trace every keyboard emitter into the log (see below)
#   ./run.sh --fresh              # start from an empty workspace
#   ./run.sh --on-top             # keep the window above others, for screenshots
#
set -uo pipefail

cd "$(dirname "$0")"

WORKSPACE="${XDG_STATE_HOME:-$HOME/.local/state}/cide/workspace.json"
MAX_WINDOWS=8
DEV_PORT=1420

env_flags=()
fresh=0
release=0

for arg in "$@"; do
  case "$arg" in
    --release)        release=1 ;;
    --bench)          env_flags+=(CIDE_BENCH=1) ;;
    --audit-chrome)   env_flags+=(CIDE_AUDIT=1) ;;
    --audit-panes)    env_flags+=(CIDE_AUDIT_PANES=1) ;;
    --audit-windows)  env_flags+=(CIDE_AUDIT_WINDOWS=1) ;;
    # Answers "which of xterm's two input emitters is the input method firing" — the only
    # thing that can, short of a debugger. Off by default because it logs four synchronous
    # IPC round trips per character; see `ui/src/terminal/inputHost.ts`.
    --input-probe)    env_flags+=(CIDE_INPUT_PROBE=1) ;;
    # The DOM renderer is the default; this opts back in to WebGL. Only useful for measuring
    # throughput on a flood of output — an upstream defect makes it permanently repaint the
    # whole screen every frame once its glyph atlas fills. See `crates/cide-app/src/windows.rs`.
    --webgl-renderer) env_flags+=(CIDE_RENDERER=webgl) ;;
    --on-top)         env_flags+=(CIDE_ON_TOP=1) ;;
    --fresh)          fresh=1 ;;
    *) echo "unknown option: $arg" >&2; sed -n '20,29p' "$0" >&2; exit 2 ;;
  esac
done

if [ "$release" = 1 ]; then
  BIN=./target/release/cide
else
  BIN=./target/debug/cide
fi

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
  if [ "$release" = 1 ]; then
    echo "[run] $BIN is missing; run: pnpm --dir ui build && cargo build --release -p cide-app" >&2
  else
    echo "[run] $BIN is missing; run: cargo build -p cide-app" >&2
  fi
  exit 1
fi

# --- refuse a binary older than the Rust it is supposed to be ---------------------------
#
# This script starts Vite, which hot-reloads the frontend the instant a file changes, and then
# launches a binary it never rebuilds. So an ordinary edit-and-relaunch loop leaves the two
# halves of the app at different commits, silently, and the failure is invisible from both
# sides: Tauri deserialises each command argument *by name* (`tauri/src/ipc/command.rs`, and
# there is no whole-payload struct and no `deny_unknown_fields`), so a stale binary does not
# reject a new payload — it ignores the fields it has not heard of and carries on. The symptom
# is a feature that is simply absent, or a pane that comes up blank, with no error anywhere.
#
# Hours were spent on a reported bug asking whether this had happened. Refusing here is cheaper
# than any handshake: the check is one `find`, and the answer is available before a window
# exists rather than after.
#
# **Only `*.rs` and the manifests, and only under `crates/`.** A guard that refuses when
# nothing is wrong gets bypassed and then deleted, so its false-positive rate is the whole
# design. Two things would have given it one:
#
#   * `crates/*/bindings/*.ts` — ts-rs rewrites every one of them on every `cargo test`, so a
#     plain `find crates` refuses after any test run, with the binary perfectly current.
#     Verified, not assumed: their mtime moves on a bare `cargo test -p cide-ipc`.
#   * `xtask/` — not linked into `cide` at all. Editing the build tool says nothing about
#     whether the binary is stale.
newer=$(find crates -name '*.rs' -newer "$BIN" -type f 2>/dev/null; \
        find crates -name Cargo.toml -newer "$BIN" -type f 2>/dev/null; \
        find Cargo.toml Cargo.lock -newer "$BIN" -type f 2>/dev/null)
newer=$(printf '%s\n' "$newer" | grep -v '^$' | head -n 3)
if [ -n "$newer" ]; then
  echo "[run] REFUSING: $BIN is older than the Rust sources it was built from."
  echo "[run] Newer than the binary, among others:"
  echo "$newer" | sed 's/^/[run]   /'
  echo "[run] This script rebuilds the frontend but never the binary, so launching now would"
  echo "[run] run a new UI against an old backend — which fails silently, not loudly."
  if [ "$release" = 1 ]; then
    echo "[run] Fix:  pnpm --dir ui build && cargo build --release -p cide-app"
  else
    echo "[run] Fix:  cargo build -p cide-app"
  fi
  exit 1
fi

# --- the frontend the binary will ask for ----------------------------------------------

port_open() { (exec 3<>/dev/tcp/127.0.0.1/"$DEV_PORT") 2>/dev/null; }

vite_pid=""
cleanup() {
  # Only ever the Vite this script started. One that was already running belongs to whoever
  # started it — killing their dev server on our way out would be a surprising thing to do.
  if [ -n "$vite_pid" ]; then
    echo "[run] stopping the dev server this script started ($vite_pid)"
    kill -TERM "$vite_pid" 2>/dev/null
  fi
}
trap cleanup EXIT INT TERM

if [ "$release" = 1 ]; then
  if [ ! -f ui/dist/index.html ]; then
    echo "[run] ui/dist is missing; run: pnpm --dir ui build" >&2
    exit 1
  fi
else
  if port_open; then
    echo "[run] dev server already listening on $DEV_PORT"
  else
    echo "[run] starting the Vite dev server (the debug build loads its UI from :$DEV_PORT)"
    pnpm --dir ui dev >/tmp/cide-vite.log 2>&1 &
    vite_pid=$!

    # Wait for the port rather than sleeping: Vite's start time depends on the cache, and a
    # fixed sleep is either too short (white window again) or wasted every launch.
    for _ in $(seq 1 100); do
      port_open && break
      # If Vite died, stop waiting for a port that is never coming.
      kill -0 "$vite_pid" 2>/dev/null || break
      sleep 0.1
    done

    if ! port_open; then
      echo "[run] the dev server did not come up on $DEV_PORT. Last lines:" >&2
      tail -n 15 /tmp/cide-vite.log >&2
      echo "[run] Or run without one:  ./run.sh --release" >&2
      exit 1
    fi
    echo "[run] dev server ready on $DEV_PORT"
  fi
fi

# --- launch ----------------------------------------------------------------------------

echo "[run] starting ${env_flags[*]:-} $BIN"
# Not `exec`: this shell has to outlive the app to stop the dev server it started.
env "${env_flags[@]}" "$BIN"
status=$?
echo "[run] cide exited with status $status"
exit "$status"
