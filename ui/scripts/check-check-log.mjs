/**
 * Checks `src/sidebar/TasksPanel/checkLogModel.ts`, the decoration that makes a gate or verify
 * log readable in the job-log viewer. (M83)
 *
 * Compiled standalone (the module is import-free) and driven over a log shaped like the real
 * ones — `tools/ci/milestone.sh` around `tools/dev/check.sh` — and then over the viewer's own
 * parser, so the claim checked is what the user sees: steps as sections, the failed one red.
 *
 * The failures worth pinning:
 *   - **A passing summary must not be red.** "0 failed" contains `failed`; colouring it would
 *     make every green run look red at a glance.
 *   - **A runner's own colour wins.** A line that already carries SGR is left alone.
 *   - **Sections balance.** An unclosed section swallows the rest of the log into a fold.
 *
 * Run: `pnpm --dir ui run check:check-log`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-check-log-'))
let failed = 0
const ok = (cond, what) => {
  if (!cond) {
    console.error(`FAIL ${what}`)
    failed++
  }
}
const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/sidebar/TasksPanel/checkLogModel.ts',
      'src/gitlab/jobLogModel.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2023',
      '--moduleResolution', 'bundler',
      '--strict',
    ],
    { stdio: 'inherit' },
  )
  const { decorateCheckLog, lineTone } = await import(`file://${join(out, 'sidebar/TasksPanel/checkLogModel.js')}`)
  const { parseJobLog } = await import(`file://${join(out, 'gitlab/jobLogModel.js')}`)

  eq(lineTone('Tests: 701 passed, 0 failed'), 'pass', 'a passing summary with "0 failed" is green')
  eq(lineTone('check.sh: STEP FAILED: unit tests (exit 1)'), 'fail', 'a failed step is red')
  eq(lineTone('BOT FAIL: player control disabled/hidden'), 'fail', 'a bot failure is red')
  eq(lineTone('WARNING: 5 ObjectDB instances were leaked'), 'warn', 'a warning is yellow')
  eq(lineTone('\u001b[32mcoloured by the runner FAILED\u001b[0m'), null, 'a runner’s own colour wins')
  eq(lineTone('$ tools/ci/milestone.sh slice'), 'command', 'the command cide ran is framed')
  eq(lineTone('plain progress line'), null, 'an ordinary line is left alone')

  const log = [
    '$ tools/ci/milestone.sh slice',
    '# in /repo at 1 (unix ms)',
    '',
    '==============================================================',
    ' milestone slice: pre-push gate (tools/dev/check.sh)',
    '==============================================================',
    'unit: 701 passed, 0 failed',
    '==============================================================',
    ' milestone slice: bot playthrough',
    '==============================================================',
    'BOT FAIL: player control disabled/hidden: ui.wayshrine.enter',
    '',
    '# exit 1',
  ].join('\n')
  const entries = parseJobLog(decorateCheckLog(log))
  const sections = entries.filter((e) => e.kind === 'section')
  eq(
    sections.map((s) => s.title),
    ['milestone slice: pre-push gate (tools/dev/check.sh)', 'milestone slice: bot playthrough'],
    'each banner is a section named by its title, and the rules are gone',
  )
  const botLine = sections[1]?.children.find((c) => c.kind === 'line' && c.text.includes('BOT FAIL'))
  ok(botLine !== undefined, 'the bot failure is inside the bot step')
  ok(botLine?.spans.some((s) => s.style.color !== undefined), 'and it is coloured')
  const exitLine = entries.find((e) => e.kind === 'line' && e.text.includes('# exit 1'))
  ok(exitLine !== undefined, 'the exit line closes the last section and stands at top level')
  const open = (decorateCheckLog(log).match(/section_start:/g) ?? []).length
  const close = (decorateCheckLog(log).match(/section_end:/g) ?? []).length
  eq(open, close, 'every section is closed')
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\ncheck-check-log: ${failed} failure(s)`)
  process.exit(1)
}
console.log('check-check-log: ok')
