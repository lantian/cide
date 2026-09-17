#!/usr/bin/env bash
#
# Refuse a shell script in this repository that needs a bash newer than 3.2.
#
# macOS ships bash **3.2.57** at `/bin/bash` — the last GPLv2 release, frozen in 2007 — and
# every script here has `#!/usr/bin/env bash`, so a Mac with no newer bash on `PATH` runs
# them under it. `run.sh` already died that way once: `mapfile` is a bash 4 builtin, so the
# script aborted at line 140 with `mapfile: command not found` and then an unbound-variable
# cascade from `set -u`, before it had started anything. `docs/platforms.md` has the whole
# account.
#
# That was fixed by hand, and nothing kept it fixed. The failure class is the reason this
# exists: a bash 4 builtin is not a syntax error, so `bash -n` passes, every Linux CI job
# passes, and the break is invisible until somebody on a Mac runs the script — which is also
# the moment they have no working build to investigate it with.
#
#   ./scripts/check-bash32.sh
#
# Add a pattern by adding a row to PATTERNS/REASONS below. The fixture at the bottom is a
# positive control and must gain a matching line, or the run fails: every assertion here is
# a grep for something that must NOT be there, so a detector that has quietly stopped
# detecting reports every file as clean and goes green forever.
#
set -uo pipefail

cd "$(dirname "$0")/.."

# Two parallel indexed arrays rather than one associative array, because bash 3.2 has no
# associative arrays — this script is bound by the rule it enforces, and it checks itself
# through the fixture below.
PATTERNS=()
REASONS=()
FIRED=()
rule() { PATTERNS+=("$1"); REASONS+=("$2"); FIRED+=(0); }

rule '(^|[^[:alnum:]_])(mapfile|readarray)([^[:alnum:]_]|$)' \
     'mapfile/readarray are bash 4 builtins. Read the lines in a `while IFS= read -r` loop fed by process substitution — `< <(cmd)`, never a pipe, which would run the loop in a subshell and leave the array empty on return.'
rule '(^|[^[:alnum:]_])(declare|typeset|local)[[:space:]]+-[A-Za-z]*A([[:space:]]|$)' \
     'Associative arrays (`declare -A`) are bash 4. Use two parallel indexed arrays, or a function per key.'
rule '(^|[^[:alnum:]_])(declare|typeset|local)[[:space:]]+-[A-Za-z]*n([[:space:]]|$)' \
     'Namerefs (`declare -n`) are bash 4.3. Pass the variable name and use `eval`, as run.sh does in `read_pids`.'
rule '\$\{[A-Za-z_][A-Za-z0-9_]*(\[[^]]*\])?(\^\^?|,,?)\}' \
     'Case modification (`${v^^}`, `${v,,}`) is bash 4. Use `tr "[:lower:]" "[:upper:]"`.'
rule '\$\{[A-Za-z_][A-Za-z0-9_]*(\[[^]]*\])?@[QEPAaKkLUu]\}' \
     'Parameter transformations (`${v@Q}`) are bash 4.4 and later. Use printf %q.'
rule '(^|[^[:alnum:]_])coproc([[:space:]]|$)' \
     '`coproc` is bash 4. Use a named pipe or a background job with explicit redirections.'
rule '&>>' \
     '`&>>` (append both streams) is bash 4. Write `>>file 2>&1`.'
rule '(^|[^[:alnum:]_])wait[[:space:]]+-n([[:space:]]|$)' \
     '`wait -n` is bash 4.3. Wait on a specific pid, or poll with `kill -0`.'
rule ';;&' \
     '`;;&` (case fall-through) is bash 4. Repeat the branch, or restructure as if/elif.'
rule '(^|[^[:alnum:]_])(shopt[[:space:]]+-[su][[:space:]]+globstar|globstar)([^[:alnum:]_]|$)' \
     '`globstar` (`**/`) is bash 4. Use `find`.'
rule '\$\{?(EPOCHSECONDS|EPOCHREALTIME|BASH_ARGV0|SRANDOM)([^[:alnum:]_]|$)' \
     'EPOCHSECONDS, EPOCHREALTIME, BASH_ARGV0 and SRANDOM are bash 5. Use `date +%s` and `$0`.'
rule "printf[^\n]*%\\([^)]*\\)T" \
     '`printf %(fmt)T` is bash 4.2. Use `date`.'

# Comments are stripped before anything is matched, and in this repository that is mandatory
# rather than tidy. The house style is to name the failure a rule prevents, so the words this
# script looks for are exactly the words the fixed code's own comment contains: run.sh's
# `read_pids` is preceded by four lines explaining that `mapfile` was there and why it went.
# Grepping the raw source flags the explanation of the fix as the bug.
#
# Whole-line comments, then a comment introduced by whitespace. The second rule can in
# principle eat a `#` inside a string, which loses a match rather than inventing one — and a
# bash 4 builtin is a command, not string data.
strip_comments() {
  sed -e 's/^[[:space:]]*#.*$//' -e 's/[[:space:]]#.*$//' "$1"
}

# Enumerated, never listed: a hardcoded list stops covering a script the moment somebody adds
# one, and does it silently. `git ls-files` also keeps the scan off `target/` and off the
# sibling forks.
files=()
while IFS= read -r f; do
  [ -n "$f" ] || continue
  # This file is the one exclusion, and it has to be: the patterns are spelled here as code,
  # so a scanner that read itself would report every rule as a violation of itself. The
  # fixture below is what covers this file instead.
  [ "$f" = scripts/check-bash32.sh ] && continue
  files+=("$f")
done < <(git ls-files '*.sh' 2>/dev/null)

if [ "${#files[@]}" -eq 0 ]; then
  echo "check-bash32: no shell scripts found — git ls-files returned nothing, so this checked nothing." >&2
  exit 1
fi

violations=0
scan() {
  local file="$1" label="$2" i hits
  i=0
  while [ "$i" -lt "${#PATTERNS[@]}" ]; do
    hits="$(strip_comments "$file" | grep -nE "${PATTERNS[$i]}" || true)"
    if [ -n "$hits" ]; then
      FIRED[$i]=1
      while IFS= read -r hit; do
        [ -n "$hit" ] || continue
        echo "$label:${hit%%:*}: ${REASONS[$i]}"
        violations=$((violations + 1))
      done <<<"$hits"
    fi
    i=$((i + 1))
  done
}

for f in "${files[@]}"; do
  scan "$f" "$f"
done

# --- the positive control ---------------------------------------------------------------
#
# One line per rule, in the same order. The scan above is a search for absence, so a detector
# that has stopped detecting — a pattern broken by an escape, a `grep -E` that no longer reads
# a construct the same way, a strip_comments that ate the whole file — reports every script as
# clean. That is a silent pass that lasts until a Mac finds it, which is exactly the failure
# this script exists to prevent, so a detector that matches nothing is a hard error.
fixture="$(mktemp)"
trap 'rm -f "$fixture"' EXIT
cat > "$fixture" <<'FIXTURE'
mapfile -t xs < <(printf 'a\n')
declare -A table
local -n ref=other
echo "${name^^}"
echo "${name@Q}"
coproc worker { cat; }
echo hi &>> log
wait -n
case x in a) :;;& esac
shopt -s globstar
echo "$EPOCHSECONDS"
printf '%(%F)T\n' -1
FIXTURE

before="$violations"
i=0
while [ "$i" -lt "${#FIRED[@]}" ]; do FIRED[$i]=0; i=$((i + 1)); done
# Redirected: the control is about which rules fire, not about printing a dozen
# findings against a file that exists for three lines of this script.
scan "$fixture" "<fixture>" >/dev/null
violations="$before"

# Distinct rules, never hits: one fixture line can legitimately match two rules, and counting
# hits would let that cover for a rule that fires on nothing at all.
dead=""
live=0
i=0
while [ "$i" -lt "${#FIRED[@]}" ]; do
  if [ "${FIRED[$i]}" -eq 1 ]; then
    live=$((live + 1))
  else
    dead="$dead
    ${PATTERNS[$i]}"
  fi
  i=$((i + 1))
done

if [ "$live" -lt "${#PATTERNS[@]}" ]; then
  echo "check-bash32: $live of ${#PATTERNS[@]} rules matched their own example. Not firing:$dead" >&2
  echo "  A rule that no longer fires on its own example detects nothing in the real scripts" >&2
  echo "  either, and reports them clean. Fix the pattern, or add the missing fixture line." >&2
  exit 1
fi

if [ "$violations" -gt 0 ]; then
  echo >&2
  echo "check-bash32: $violations construct(s) above need a bash newer than 3.2." >&2
  echo "  macOS ships bash 3.2.57 as /bin/bash, and every script here runs under it there." >&2
  echo "  See docs/platforms.md, 'run.sh, and the three things that made it Linux-only'." >&2
  exit 1
fi

echo "ok: ${#files[@]} shell script(s) run on bash 3.2 (${#PATTERNS[@]} rules, all confirmed live)"
