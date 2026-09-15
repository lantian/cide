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
# --- the dev profile ---------------------------------------------------------------------
#
# This script launches under the `dev` profile by default, which is what makes it safe to run
# beside an installed cide you are using for real work. A profile moves the whole footprint:
# `$XDG_STATE_HOME/cide-dev` and `$XDG_CONFIG_HOME/cide-dev` instead of `.../cide`, a separate
# Tauri bundle identifier so the WebKit storage, the log and the remembered window geometry
# move too, and a `[DEV] ` prefix on the window title so the two are distinguishable in a task
# switcher. Without it both instances write one `workspace.json`, and the last one to exit
# stamps its layout over the other's.
#
# Consequence worth knowing before you report it as a bug: **a profile starts factory-fresh.**
# Settings live inside `workspace.json`, so a new profile has no installed extensions, no
# global agent roles, the default keymap and the default theme. That is the point, but it is
# surprising the first time.
#
# `--profile default` opts back out and shares the real instance's state.
#
#   ./run.sh                      # normal launch (starts Vite if it is not already up)
#   ./run.sh --release            # build and run the release binary; no dev server
#   ./run.sh --profile <name>     # run under a named profile ("default" = the real instance)
#   ./run.sh --bench              # IPC transport gate (M0), prints and exits
#   ./run.sh --audit-chrome       # chrome vs the design mock (M3), prints and exits
#   ./run.sh --audit-panes        # pane host registry under churn (M4)
#   ./run.sh --audit-windows      # detach/re-dock and window modes (M5)
#   ./run.sh --input-probe        # trace every keyboard emitter into the log (see below)
#   ./run.sh --webgl-renderer     # opt back in to xterm's WebGL renderer (see below)
#   ./run.sh --inspect            # console into the Rust log + WebKit inspector on :9222
#   ./run.sh --fresh              # start from an empty workspace
#   ./run.sh --on-top             # keep the window above others, for screenshots
#
set -uo pipefail

cd "$(dirname "$0")"

# Personal environment, if the developer keeps one. `.env` is gitignored and holds the
# variables a launch here should carry every time without retyping them — above all
# `CIDE_RA_PATH` and `CIDE_GOPLS_PATH`, the fork overrides (see CLAUDE.md). Sourced with
# allexport so plain `KEY=value` lines export without each needing its own `export`.
# A missing file is simply an empty one; a present file is trusted like the script itself,
# because both are the developer's own working tree.
if [ -f .env ]; then
  set -a
  # shellcheck disable=SC1091
  . ./.env
  set +a
fi

MAX_WINDOWS=8
DEV_PORT=1420

# Printed for an unknown option. Content-addressed rather than a line range: this used to be
# `sed -n '20,29p'`, which silently drifted the moment a line was added above it — by the time
# it was replaced it was already omitting two implemented flags.
usage() { sed -n '/^#   \.\/run\.sh/p' "$0" >&2; }

env_flags=()
fresh=0
release=0
# The whole point of this script: an instance that is not the one you work in. `--profile
# default` is the way back out.
profile=dev

while [ $# -gt 0 ]; do
  case "$1" in
    --release)        release=1 ;;
    # A value flag, which is why this loop shifts rather than iterating `$@` directly. Both
    # spellings, because `--profile=x` is what anyone scripting it will reach for.
    --profile)
      [ $# -ge 2 ] || { echo "--profile needs a name" >&2; usage; exit 2; }
      profile="$2"; shift ;;
    --profile=*)      profile="${1#--profile=}" ;;
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
    # Everything needed to see inside the webview: the console forwarded into the Rust log,
    # and WebKitGTK's remote inspector on 127.0.0.1:9222. Both off by default — the bridge
    # costs a synchronous IPC round trip per line, and the inspector opens a listening port.
    --inspect)        env_flags+=(CIDE_CONSOLE_BRIDGE=1 WEBKIT_INSPECTOR_SERVER=127.0.0.1:9222) ;;
    --on-top)         env_flags+=(CIDE_ON_TOP=1) ;;
    --fresh)          fresh=1 ;;
    *) echo "unknown option: $1" >&2; usage; exit 2 ;;
  esac
  shift
done

# `default` is the reserved "no profile" name, matching `cide_core::profile`. Normalised to an
# empty string here so every test below is `[ -n "$profile" ]` rather than two spellings.
[ "$profile" = default ] && profile=""

# Computed *after* the parse loop, and that ordering is load-bearing: `--fresh` renames this
# file, so a `WORKSPACE` resolved before `--profile` was read would move the real instance's
# layout aside when the user asked to reset a profile's.
if [ -n "$profile" ]; then
  env_flags+=("CIDE_PROFILE=$profile")
  STATE_LEAF="cide-$profile"
else
  STATE_LEAF="cide"
fi
STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/$STATE_LEAF"
WORKSPACE="$STATE_DIR/workspace.json"
# `cide_app::instance::LOCK_FILE`, beside the workspace in the same profile directory. Read by
# `lock_holder` below to tell this profile's instance from another's.
LOCKFILE="$STATE_DIR/instance.lock"

if [ "$release" = 1 ]; then
  BIN=./target/release/cide
else
  BIN=./target/debug/cide
fi

# --- stop whatever is already running -------------------------------------------------

# Read a pid per line into an array, portably.
#
# `mapfile` was here and is a **bash 4** builtin. macOS ships bash 3.2.57 at `/bin/bash` and the
# shebang is `/usr/bin/env bash`, so on a Mac with no newer bash this script died at this line
# before it started anything — `mapfile: command not found`, then an unbound-variable cascade from
# `set -u`. See `docs/platforms.md`.
#
# The `< <(...)` process substitution is deliberate and not a pipe: a pipe would run the loop in a
# subshell and the array would be empty the moment it returned.
read_pids() {
  local __var="$1" __line
  eval "$__var=()"
  while IFS= read -r __line; do
    [ -n "$__line" ] || continue
    eval "$__var+=(\"\$__line\")"
  done
}

# Matched on the exact binary path rather than the word "cide", which would also match this
# script, an editor holding the source, and the agent session that started it. Anchored `-f`
# behaves the same on BSD `pgrep` as on procps', which was checked rather than assumed.
read_pids candidates < <(pgrep -f "^${BIN}$" 2>/dev/null || true)

# Which profile a running process belongs to, read from its own environment.
#
# The binary path alone is no longer enough. It does separate this script from an installed
# build — `^./target/debug/cide$` cannot match an AppImage — but `./run.sh` and
# `./run.sh --profile default` are the *same* path, and killing across that line is exactly
# what the profile exists to prevent. `grep -qxF` because the match must be exact: without
# `-x` the profile `dev` matches a process running `dev2`. An unreadable `environ` is a pid
# that died between `pgrep` and here, which is not a match.
# Whether the profile's lock file names this pid.
#
# `cide_app::instance` writes `{pid, exe, profile}` into `instance.lock` beside `workspace.json`
# as its own one-instance guard, so the profile already keeps a record of which process owns it.
# The file lives in the profile's *own* state directory, so its path already encodes the profile;
# the `profile` field is compared as well, because a file that disagrees with the directory it is
# in is a file this should not be trusting.
#
# Used only where `/proc` is not — see [`in_this_profile`].
lock_names() {
  [ -r "$LOCKFILE" ] || return 1
  # The three values go in as **arguments**, not interpolated into the program text: `$profile`
  # is whatever the user typed after `--profile`, and a name containing a quote would otherwise
  # close the string literal it was pasted into. A developer script running a developer's own
  # argument is not an attack surface, but a quoting bug here would look like "the profile filter
  # stopped working", which is the expensive kind of wrong.
  python3 -c '
import json, sys
lock, pid, profile = sys.argv[1], int(sys.argv[2]), sys.argv[3] or None
try:
    rec = json.load(open(lock))
except Exception:
    sys.exit(1)
sys.exit(0 if rec.get("pid") == pid and rec.get("profile") == profile else 1)
' "$LOCKFILE" "$1" "$profile" 2>/dev/null
}

# Which profile a running process belongs to.
#
# The binary path alone is not enough. It separates this script from an installed build —
# `^./target/debug/cide$` cannot match an AppImage — but `./run.sh` and `./run.sh --profile
# default` are the *same* path, and killing across that line is exactly what the profile exists to
# prevent.
#
# Two roads, and `/proc` is tried **first** on purpose. It is live truth: the kernel's copy of the
# process's own environment, which cannot be stale and cannot describe a pid the kernel has since
# reissued. The lock file is a *written record* and can be both. So on Linux this behaves exactly
# as it did before the file was consulted at all, and the lock is what makes the function work on
# a platform that has no `/proc`.
#
# `grep -qxF` because the match must be exact: without `-x` the profile `dev` matches a process
# running `dev2`. An unreadable `environ` is a pid that died between `pgrep` and here, a process
# owned by somebody else, or a kernel with no `/proc` — the first two are not a match and the
# third falls through.
#
# **And no third road: a process this cannot identify is left alone.** macOS does not let one
# process read another's environment — `ps -E` and `ps eww` both decline — so on a Mac an instance
# that never took the lock (`CIDE_ALLOW_SECOND_INSTANCE`) is a stranger. That is the safe
# direction, and it is deliberately the opposite of the bias in `cide_app::instance`, which starts
# the app on every doubt: the doubt here is about *killing somebody else's application*, so silence
# means leave it alone. The cost is a launch that then refuses because the lock is held, which is a
# loud and correct failure rather than a quiet and wrong one.
in_this_profile() {
  local pid="$1" env_file="/proc/$1/environ"

  if [ -r "$env_file" ]; then
    if [ -n "$profile" ]; then
      tr '\0' '\n' < "$env_file" 2>/dev/null | grep -qxF "CIDE_PROFILE=$profile"
    else
      # The default profile is the *absence* of the variable, so this is the mirror image.
      ! tr '\0' '\n' < "$env_file" 2>/dev/null | grep -q '^CIDE_PROFILE='
    fi
    return
  fi

  lock_names "$pid"
}

running=()
strangers=()
# `${#candidates[@]}` first: under `set -u`, bash 3.2 treats `"${empty[@]}"` as an unbound
# variable and aborts. bash 4.4 relaxed that, which is why this never showed on Linux.
if [ "${#candidates[@]}" -gt 0 ]; then
  for pid in "${candidates[@]}"; do
    if in_this_profile "$pid"; then running+=("$pid"); else strangers+=("$pid"); fi
  done
fi

if [ "${#running[@]}" -gt 0 ]; then
  echo "[run] stopping ${#running[@]} running instance(s): ${running[*]}"
  # SIGTERM, not SIGKILL: the app's own handler flushes the workspace and brings its
  # children down through SIGHUP -> SIGTERM -> SIGKILL, which is what lets a `claude`
  # finish writing the transcript that makes its conversation resumable.
  kill -TERM "${running[@]}" 2>/dev/null

  # Polled with `kill -0` over the collected pids rather than by re-running `pgrep`, because
  # a fresh `pgrep` cannot express the profile filter and would wait on processes this run
  # has deliberately left alone.
  for _ in $(seq 1 40); do
    alive=0
    for pid in "${running[@]}"; do kill -0 "$pid" 2>/dev/null && alive=1; done
    [ "$alive" = 0 ] && break
    sleep 0.25
  done

  survivors=()
  for pid in "${running[@]}"; do kill -0 "$pid" 2>/dev/null && survivors+=("$pid"); done
  if [ "${#survivors[@]}" -gt 0 ]; then
    echo "[run] did not exit on SIGTERM; sending SIGKILL"
    kill -KILL "${survivors[@]}" 2>/dev/null
    sleep 1
  fi
fi

# Warned about, never killed — leaving them alone is the whole point. But the build below
# rewrites `target/debug/cide-hook` underneath them, and a running instance resolves the hook
# by path on every spawn. A stale-shape hook reports into a newer IDE with no error and no log
# line; see the note on `cide-hook` in the build section. So this has to be said out loud.
if [ "${#strangers[@]}" -gt 0 ]; then
  echo "[run] NOTE: ${#strangers[@]} instance(s) of $BIN are running under another profile: ${strangers[*]}"
  echo "[run] They are being left alone, but rebuilding will replace the cide-hook they spawn."
  echo "[run] Restart them once this build finishes if their hooks start misbehaving."
fi

# A `claude` reparented to init is one the app failed to reap. Left alone they accumulate
# across launches, each holding a PTY and a model connection.
#
# The basename is compared, not the whole field, because **`comm` is a full path on macOS**:
# procps prints `claude`, BSD `ps` prints `/opt/homebrew/bin/claude`, so `$3 == "claude"` matched
# nothing on a Mac and the reaping silently did not happen. Reparenting is to pid 1 on both —
# `launchd` there, `init` here.
read_pids orphans < <(ps -eo ppid=,pid=,comm= 2>/dev/null |
  awk '$1 == 1 { n = split($3, seg, "/"); if (seg[n] == "claude") print $2 }')
if [ "${#orphans[@]}" -gt 0 ]; then
  echo "[run] reaping ${#orphans[@]} orphaned claude process(es)"
  kill -TERM "${orphans[@]}" 2>/dev/null
fi

# --- sanity-check what the workspace will do ------------------------------------------

if [ "$fresh" = 1 ]; then
  if [ -f "$WORKSPACE" ]; then
    mv "$WORKSPACE" "$WORKSPACE.bak"
    echo "[run] --fresh: previous ${profile:-default} workspace moved to $WORKSPACE.bak"
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
    echo "[run] REFUSING: the ${profile:-default} profile's saved workspace records $windows windows (cap $MAX_WINDOWS)."
    echo "[run] Opening it would put that many windows on screen."
    # `cide-headless` links `cide-core`, so it reads whichever profile it is told to — the
    # hint has to carry it or it prints the wrong workspace and reads as a contradiction.
    if [ -n "$profile" ]; then
      echo "[run] Inspect it with:  CIDE_PROFILE=$profile ./target/debug/cide-headless tree"
      echo "[run] Or start clean:   ./run.sh --profile $profile --fresh"
    else
      echo "[run] Inspect it with:  ./target/debug/cide-headless tree"
      echo "[run] Or start clean:   ./run.sh --profile default --fresh"
    fi
    exit 1
  fi
fi

# --- build the binary, rather than refusing to launch an old one ------------------------
#
# This script starts Vite, which hot-reloads the frontend the instant a file changes. If it
# then launched a binary it never rebuilt, the two halves of the app would sit at different
# commits, silently: Tauri deserialises each command argument *by name*
# (`tauri/src/ipc/command.rs` — no whole-payload struct, no `deny_unknown_fields`), so a stale
# binary does not reject a new payload. It ignores the fields it has not heard of and carries
# on, and the symptom is a feature that is simply absent or a pane that comes up blank, with no
# error anywhere. Hours went into a reported bug that turned out to be exactly this.
#
# **This used to REFUSE, by comparing mtimes with `find -newer`, and that was wrong twice over.**
#
# Its own comment said a guard that refuses when nothing is wrong gets bypassed and then
# deleted, so its false-positive rate is the whole design — and it named two sources it had
# avoided (`bindings/*.ts`, rewritten by every `cargo test`; `xtask/`, not linked in). It
# missed the one that matters: **cargo fingerprints file *contents*, `find` compares mtimes.**
# A manifest that is touched but unchanged — which `cargo` itself does, and any tool that
# rewrites a file with the same bytes — makes `find` report it as newer while cargo correctly
# has nothing to do. The guard then refused for ever and told the user to run
# `cargo build -p cide-app`, which finished in 0.16s and cleared nothing. A guard whose
# suggested fix cannot satisfy it is worse than no guard.
#
# So: ask cargo, which is the only thing that knows. When the binary is current this costs
# ~0.2s; when it is not, rebuilding is precisely what the old guard was asking for anyway.
# `cide-hook` is built alongside, and it is not optional. The app resolves the hook binary as
# `current_exe().parent()/cide-hook`, so a run.sh that builds only `cide-app` launches a new
# IDE against whatever `cide-hook` happened to be in `target/debug` from some earlier build.
# Every hook then reports in an old frame shape and the failure is silent — no error, no log
# line, just chrome that never updates. That is how the finished-turn notification survived
# four rounds of fixes: the code read correctly and the binary answering was stale.
build=(cargo build -p cide-app -p cide-hook)
if [ "$release" = 1 ]; then
  build+=(--release)
fi
echo "[run] ${build[*]}"
if ! "${build[@]}"; then
  echo "[run] REFUSING: the backend did not build, so there is nothing safe to launch." >&2
  echo "[run] The compiler's own output is above." >&2
  exit 1
fi

if [ ! -x "$BIN" ]; then
  echo "[run] REFUSING: $BIN is still missing after a successful build." >&2
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

echo "[run] starting ${env_flags[*]:-} $BIN  (profile: ${profile:-default})"
# Not `exec`: this shell has to outlive the app to stop the dev server it started.
# `env -u CIDE_PROFILE` rather than simply omitting it from `env_flags`, and this is not
# belt-and-braces. This script is very often launched from a shell *inside* a cide pane, and a
# child of a profiled instance inherits `CIDE_PROFILE=dev` from it. Without the explicit unset,
# `./run.sh --profile default` run from a dev pane — the single most likely way anyone reaches
# for the escape hatch — would silently come up in the dev profile anyway.
#
# The `${#env_flags[@]}` guard is bash 3.2 again: with no flags and `--profile default` the array
# is empty, and `"${empty[@]}"` under `set -u` aborts there. That combination is precisely the
# escape hatch — the one invocation somebody reaches for when something has gone wrong — so it is
# the worst one to have die on an unbound variable.
if [ -n "$profile" ]; then
  env "${env_flags[@]}" "$BIN"
elif [ "${#env_flags[@]}" -gt 0 ]; then
  env -u CIDE_PROFILE "${env_flags[@]}" "$BIN"
else
  env -u CIDE_PROFILE "$BIN"
fi
status=$?
echo "[run] cide exited with status $status"
exit "$status"
