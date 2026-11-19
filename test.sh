#!/usr/bin/env bash
# One check runner for local development and .github/workflows/ci.yml. Keep commands here:
# separate local and CI lists let a green partial run get mistaken for the whole suite.
# Bash 3.2 is intentional: this must also run under macOS's /bin/bash.
set -uo pipefail

cd "$(dirname "$0")" || exit 2
root=$PWD

usage() {
  cat <<'EOF'
Usage: ./test.sh [--only GROUP] [--list]

With no arguments, run every CI check for this host and report all failures.
Groups: rust, rust-lint, rust-test, macos, ui, no-tauri, shell, fork-pins
  --only GROUP  Run one CI job's checks. rust is rust-lint and rust-test together.
  --list        Print the commands without installing, building or testing.

Requires the CI toolchain, Node 22, pnpm 9.15.4, Python 3 and Git; Linux also needs
the WebKitGTK/GTK development packages listed in CONTRIBUTING.md.
Each check keeps its complete output under target/ci-checks/.
Linux and macOS execute the same runner on their own hosts; a local Linux run
does not execute macOS-specific code or macOS's system Bash and Git.
EOF
}

scope=all
list_only=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --only)
      if [ "$#" -lt 2 ]; then usage >&2; exit 2; fi
      scope=$2
      shift 2
      ;;
    --list) list_only=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'Unknown option: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
done
case "$scope" in
  all|rust|rust-lint|rust-test|macos|ui|no-tauri|shell|fork-pins) ;;
  *) printf 'Unknown group: %s\n' "$scope" >&2; exit 2 ;;
esac

total=0
failed=0
failures=''
logs=''
export RUST_BACKTRACE=1
export CARGO_TERM_COLOR=always

if [ "$list_only" -eq 0 ]; then
  case "$scope" in
    all) tools='bash cargo git python3 node pnpm' ;;
    rust|rust-lint|rust-test|macos) tools='cargo' ;;
    ui) tools='node pnpm' ;;
    no-tauri) tools='cargo python3' ;;
    shell) tools='bash git python3' ;;
    fork-pins) tools='git' ;;
  esac
  missing=''
  for tool in $tools; do
    command -v "$tool" >/dev/null 2>&1 || missing="$missing $tool"
  done
  if [ -n "$missing" ]; then
    printf 'Missing required tools:%s\nSee CONTRIBUTING.md.\n' "$missing" >&2
    exit 2
  fi
  mkdir -p "$root/target/ci-checks" || exit 2
  logs=$(mktemp -d "$root/target/ci-checks/run-XXXXXXXX") || exit 2
  printf 'Host: %s; group: %s\nLogs: %s\n' "$(uname -s)" "$scope" "$logs"
  trap 'printf "\nInterrupted. Logs: %s\n" "$logs" >&2; exit 130' INT
  trap 'printf "\nTerminated. Logs: %s\n" "$logs" >&2; exit 143' TERM
fi

failure() {
  failed=$((failed + 1))
  failures="${failures}
  - $1"
}

# No errexit: an independent failure must not hide the next check. UI setup is the one
# dependency we guard explicitly, so a failed install cannot validate stale node_modules.
run() {
  local label="$1" began="$SECONDS" log code
  shift
  if [ "$list_only" -eq 1 ]; then
    printf '%s: ' "$label"
    printf '%q ' "$@"
    printf '\n'
    return 0
  fi
  total=$((total + 1))
  log="$logs/$(printf '%03d' "$total").log"
  printf '\n[%s] %s\n' "$total" "$label"
  if [ "${GITHUB_ACTIONS:-}" = true ]; then printf '::group::%s\n' "$label"; fi
  "$@" >"$log" 2>&1
  code=$?
  printf '%s\t%s\t%s\n' "$label" "$code" "$log" >> "$logs/results.tsv"
  if [ "$code" -eq 0 ]; then
    printf 'PASS (%ss) — %s\n' "$((SECONDS - began))" "$log"
  else
    failure "$label (exit $code): $log"
    printf 'Last 120 log lines (complete output: %s):\n' "$log"
    tail -n 120 "$log"
    printf 'FAIL (%ss, exit %s) — %s\n' "$((SECONDS - began))" "$code" "$log" >&2
  fi
  if [ "${GITHUB_ACTIONS:-}" = true ]; then printf '::endgroup::\n'; fi
  return "$code"
}

# CI runs rust-lint and rust-test as two parallel jobs; `rust` (and a full run) is both, in
# the order they ran as one job. macOS runs both in one job and skips the two gates whose
# answer does not depend on the host. Every step belongs to exactly one of lint and test —
# check-test-runner.py pins that the two add up to `rust`.
rust_checks() {
  local lint=1 tests=1
  case "$scope" in
    rust-lint) tests=0 ;;
    rust-test) lint=0 ;;
  esac
  if [ "$lint" -eq 1 ]; then
    rust_lint_head
  fi
  if [ "$tests" -eq 1 ]; then
    # Cargo otherwise stops at the first failed test binary and conceals other crate failures.
    run rust/test cargo test --locked --workspace --no-fail-fast
  fi
  if [ "$lint" -eq 1 ]; then
    run rust/clippy cargo clippy --locked --workspace --all-targets -- -D warnings
    if [ "$scope" != macos ]; then
      run rust/codegen cargo --locked xtask codegen --check
    fi
  fi
}

rust_lint_head() {
  run rust/fmt cargo fmt --all --check
  if [ "$scope" != macos ]; then
    run rust/contract cargo --locked xtask contract-check
  fi
  # `check`, not `build`. What this step uniquely proves is that the workspace compiles with
  # the features it ships with: `cargo test --workspace` and `clippy --all-targets` both unify
  # dev-dependency features into every crate, and cide-remote's dev-dependency turns on tokio's
  # `test-util` (see its Cargo.toml) — so neither of them would notice shipped code reaching
  # `tokio::time::pause`. That difference is also why the old `cargo build` was so dear: a
  # different tokio means a different hyper, reqwest, tauri, every tauri plugin and all
  # twenty-two workspace crates, so build and test each compiled their own copy of the stack
  # (4–13 min of CI per run spent on artifacts nothing else used). Type-checking catches the
  # same class without codegen or linking; linking itself is still exercised, by the test
  # binaries and the `cide` binary that `cargo test` builds for cide-app's integration tests.
  run rust/check cargo check --locked --workspace
}

ui_checks() {
  local scripts script
  if ! run ui/install pnpm --dir ui install --frozen-lockfile; then
    printf 'UI checks skipped: dependency installation failed.\n' >&2
    return 1
  fi
  run ui/typecheck pnpm --dir ui exec tsc --noEmit
  # Enumeration keeps a newly added check in both the local suite and CI automatically.
  if ! scripts=$(node -p "Object.keys(require('./ui/package.json').scripts ?? {}).filter(s => s.startsWith('check:')).join('\n')"); then
    total=$((total + 1))
    failure 'ui/enumerate: could not read ui/package.json'
    return 1
  fi
  if [ -z "$scripts" ]; then
    printf 'No check:* scripts in ui/package.json; refusing an empty UI suite.\n' >&2
    total=$((total + 1))
    failure 'ui/enumerate: no check:* scripts in ui/package.json'
    return 1
  fi
  if [ "$list_only" -eq 1 ]; then
    while IFS= read -r script; do
      run "ui/$script" pnpm --dir ui run "$script"
    done <<< "$scripts"
  else
    run_ui_scripts "$scripts"
  fi
  run ui/build pnpm --dir ui build
}

# The check:* scripts run CIDE_CHECK_JOBS at a time (default: every core) and are then reported
# one by one, in package.json order, exactly as `run` would have — same numbering, same logs,
# same results.tsv rows, same PASS/FAIL lines. They are independent by construction: each one
# compiles into its own mkdtemp directory (check-editor and check-markdown into their own fixed
# one) and only reads the source. One after another they took 180 s on a 32-core machine and
# 5–8 min on CI, almost all of it a node and a tsc starting up 142 times.
#
# A check whose status file is missing — its shell killed, xargs stopped early — is reported as
# a failure with exit 125, never skipped: a parallel runner that drops a result is how a red
# check turns into an absent one.
#
# Checks that assert on their own wall-clock time run afterwards, one at a time, with the
# machine to themselves: `check:markdown` failed in its first parallel run with "parsing
# underscore pairs is linear: median 8x the input took 16.7x the time" — a ratio of two
# timings, and contention moves the two by different amounts. A new check that times itself
# belongs in this list (or in CIDE_SERIAL_CHECKS for one run).
ui_serial_checks='check:editor check:markdown check:openspec-performance'
run_ui_scripts() {
  local scripts="$1" jobs stem log label code secs n serial child
  jobs=${CIDE_CHECK_JOBS:-$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 2)}
  serial=" ${CIDE_SERIAL_CHECKS-$ui_serial_checks} "
  printf '\nRunning %s UI checks, %s at a time; results follow in order.\n' \
    "$(printf '%s\n' "$scripts" | wc -l | tr -d ' ')" "$jobs"
  # NUL-separated (log stem, script) pairs, because the log directory may contain spaces.
  : > "$logs/ui-parallel.list"
  : > "$logs/ui-serial.list"
  n=$total
  while IFS= read -r script; do
    n=$((n + 1))
    case "$serial" in
      *" $script "*) printf '%s\0%s\0' "$logs/$(printf '%03d' "$n")" "$script" >> "$logs/ui-serial.list" ;;
      *) printf '%s\0%s\0' "$logs/$(printf '%03d' "$n")" "$script" >> "$logs/ui-parallel.list" ;;
    esac
  done <<< "$scripts"
  # The child's last command is a printf, so it never exits 255 — the status that makes
  # xargs stop launching the rest. An empty list never reaches xargs: GNU xargs would run
  # the child once with no arguments.
  child='
    began=$SECONDS
    pnpm --dir ui run "$2" >"$1.log" 2>&1
    printf "%s %s\n" "$?" "$((SECONDS - began))" >"$1.status"
  '
  if [ -s "$logs/ui-parallel.list" ]; then
    xargs -0 -n 2 -P "$jobs" bash -c "$child" run-ui-check < "$logs/ui-parallel.list"
  fi
  if [ -s "$logs/ui-serial.list" ]; then
    xargs -0 -n 2 -P 1 bash -c "$child" run-ui-check < "$logs/ui-serial.list"
  fi
  rm -f "$logs/ui-parallel.list" "$logs/ui-serial.list"
  while IFS= read -r script; do
    total=$((total + 1))
    stem="$logs/$(printf '%03d' "$total")"
    log="$stem.log"
    label="ui/$script"
    printf '\n[%s] %s\n' "$total" "$label"
    if [ "${GITHUB_ACTIONS:-}" = true ]; then printf '::group::%s\n' "$label"; fi
    code=125
    secs=0
    if [ -f "$stem.status" ]; then
      read -r code secs < "$stem.status"
      rm -f "$stem.status"
    else
      printf 'test.sh: no exit status was recorded for %s; it did not finish.\n' "$script" >> "$log"
    fi
    printf '%s\t%s\t%s\n' "$label" "$code" "$log" >> "$logs/results.tsv"
    if [ "$code" -eq 0 ]; then
      printf 'PASS (%ss) — %s\n' "$secs" "$log"
    else
      failure "$label (exit $code): $log"
      printf 'Last 120 log lines (complete output: %s):\n' "$log"
      tail -n 120 "$log"
      printf 'FAIL (%ss, exit %s) — %s\n' "$secs" "$code" "$log" >&2
    fi
    if [ "${GITHUB_ACTIONS:-}" = true ]; then printf '::endgroup::\n'; fi
  done <<< "$scripts"
}

if [ "$scope" = all ] || [ "$scope" = shell ]; then
  run shell/runner python3 scripts/check-test-runner.py
  run shell/bash32 bash scripts/check-bash32.sh
fi
if [ "$scope" = all ] || [ "$scope" = no-tauri ]; then
  run structure/no-tauri bash scripts/check-no-tauri.sh
fi
case "$scope" in
  all|rust|rust-lint|rust-test|macos) rust_checks ;;
esac
if [ "$scope" = all ] || [ "$scope" = ui ]; then
  ui_checks
fi
if [ "$scope" = all ] || [ "$scope" = fork-pins ]; then
  run dependencies/fork-pins bash scripts/check-fork-pins.sh
fi

if [ "$list_only" -eq 1 ]; then
  [ "$failed" -eq 0 ]
  exit $?
fi
printf '\nChecks finished: %s passed, %s failed.\nLogs: %s\n' "$((total - failed))" "$failed" "$logs"
if [ "$failed" -gt 0 ]; then
  printf 'Failures:%s\n' "$failures" >&2
  exit 1
fi
