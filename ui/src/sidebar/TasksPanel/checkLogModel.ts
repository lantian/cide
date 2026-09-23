/**
 * Make a check's log readable. (M83)
 *
 * A gate or verify log is whatever the project's scripts printed: banners, test runners, a
 * game engine's diagnostics — mostly without colour, because none of them thought they had a
 * terminal. It is drawn by the GitLab job-log viewer (`gitlab/JobLog.tsx`), which renders SGR
 * colour and collapsible `section_start`/`section_end` markers. This turns a plain log into that
 * dialect, **as text**, so there is one renderer and one parser:
 *
 * - **A banner is a section.** `=====` / title / `=====` — what `tools/dev/check.sh` and
 *   `tools/ci/milestone.sh` print before each step — opens a section named by the title, closing
 *   the one before. A long gate becomes a list of its steps, and the failed one is findable.
 * - **A line that says how it went is coloured**, and only when the line carries no colour of its
 *   own: a runner's own palette always wins over a guess.
 * - **The frame cide adds** — the `$ command` it ran and the `# exit N` it ended with — is dim
 *   and bold respectively, the latter green for 0 and red otherwise.
 *
 * Import-free, so `check:check-log` can compile and drive it standalone.
 */

const ESC = '\u001b'
const RED = `${ESC}[31m`
const GREEN = `${ESC}[32m`
const YELLOW = `${ESC}[33m`
const DIM = `${ESC}[2m`
const BOLD = `${ESC}[1m`
const RESET = `${ESC}[0m`

/** A run of `=` or `-` long enough to be a banner rule, and nothing else on the line. */
const RULE = /^\s*(={10,}|-{10,})\s*$/

const FAIL = /\b(FAIL(ED|URE|S)?|ERROR|SCRIPT ERROR|panicked|Traceback|fatal|refusing)\b/
const PASS = /\b(PASS(ED)?|passed|succeeded|SUCCESS)\b|^\s*ok\b|:\s*ok\s*$/
const WARN = /\b(WARN(ING)?|deprecated)\b/

/** How one uncoloured line should look, or `null` for as it is. Exported for the check. */
export function lineTone(line: string): 'fail' | 'pass' | 'warn' | 'command' | null {
  if (line.includes(ESC)) return null
  if (line.startsWith('$ ')) return 'command'
  // A pass is judged before a fail on purpose: "0 failed" in a passing summary names a failure
  // count, and colouring that line red would cry wolf on every green run.
  if (/\b0 (failed|failures|errors)\b/.test(line) && PASS.test(line)) return 'pass'
  if (FAIL.test(line)) return 'fail'
  if (PASS.test(line)) return 'pass'
  if (WARN.test(line)) return 'warn'
  return null
}

/** The log in the job-log viewer's dialect. See the header. */
export function decorateCheckLog(raw: string): string {
  const lines = raw.split('\n')
  const out: string[] = []
  let open: string | null = null
  let n = 0
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i] ?? ''
    // A banner: rule, title, rule. Its title opens a section; the rules are dropped.
    const title = lines[i + 1]
    if (RULE.test(line) && title !== undefined && RULE.test(lines[i + 2] ?? '') && title.trim() !== '') {
      if (open !== null) out.push(`section_end:0:${open}\r${ESC}[0K`)
      n += 1
      open = `step_${n}`
      out.push(`section_start:0:${open}\r${ESC}[0K${BOLD}${title.trim()}${RESET}`)
      i += 2
      continue
    }
    const exit = /^# (exit (\d+)|stopped after .*|killed by a signal)$/.exec(line)
    if (exit !== null) {
      if (open !== null) {
        out.push(`section_end:0:${open}\r${ESC}[0K`)
        open = null
      }
      const good = exit[2] === '0'
      out.push(`${BOLD}${good ? GREEN : RED}${line}${RESET}`)
      continue
    }
    switch (lineTone(line)) {
      case 'command':
        out.push(`${DIM}${line}${RESET}`)
        break
      case 'fail':
        out.push(`${RED}${line}${RESET}`)
        break
      case 'pass':
        out.push(`${GREEN}${line}${RESET}`)
        break
      case 'warn':
        out.push(`${YELLOW}${line}${RESET}`)
        break
      default:
        out.push(line)
    }
  }
  if (open !== null) out.push(`section_end:0:${open}\r${ESC}[0K`)
  return out.join('\n')
}
