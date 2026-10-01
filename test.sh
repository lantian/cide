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
Groups: rust, macos, ui, no-tauri, shell, fork-pins
  --only GROUP  Run one CI job's checks.
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
  all|rust|macos|ui|no-tauri|shell|fork-pins) ;;
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
    rust|macos) tools='cargo' ;;
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

rust_checks() {
  run rust/fmt cargo fmt --all --check
  if [ "$scope" != macos ]; then
    run rust/contract cargo --locked xtask contract-check
  fi
  run rust/build cargo build --locked --workspace
  # Cargo otherwise stops at the first failed test binary and conceals other crate failures.
  run rust/test cargo test --locked --workspace --no-fail-fast
  run rust/clippy cargo clippy --locked --workspace --all-targets -- -D warnings
  if [ "$scope" != macos ]; then
    run rust/codegen cargo --locked xtask codegen --check
  fi
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
  while IFS= read -r script; do
    run "ui/$script" pnpm --dir ui run "$script"
  done <<< "$scripts"
  run ui/build pnpm --dir ui build
}

if [ "$scope" = all ] || [ "$scope" = shell ]; then
  run shell/runner python3 scripts/check-test-runner.py
  run shell/bash32 bash scripts/check-bash32.sh
fi
if [ "$scope" = all ] || [ "$scope" = no-tauri ]; then
  run structure/no-tauri bash scripts/check-no-tauri.sh
fi
if [ "$scope" = all ] || [ "$scope" = rust ] || [ "$scope" = macos ]; then
  rust_checks
fi
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
