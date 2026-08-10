/**
 * Checks `src/panes/exitMarker.ts` — the one place a session's exit code becomes something a
 * user reads.
 *
 * Worth pinning because the whole chain behind it is invisible from the pane. `cide-pty`
 * reaps the child and keeps the `ExitStatus` that `wait()` consumes; `lifecycle::report_exit`
 * publishes it as `SessionState::Exited { code }`; `TerminalPane` writes this string. Every
 * step but the last has a Rust test, and the last step is where the previous version of all
 * of it died: the code arrived at the frontend correctly and was dropped one line before the
 * screen, so a `claude` the OOM reaper killed and a `claude` that finished cleanly printed
 * the identical `— exited —`.
 *
 * The three numbers below are the ones that used to be indistinguishable — `1`, `137` and
 * `143` all arrive as a flat 1 from `portable-pty` unless the signal name is mapped back.
 *
 * Same shape as `check-close-confirm.mjs` and `check-picker.mjs` — there is no JS test runner
 * in this project, and this is a pure module the TypeScript in `node_modules` can compile on
 * its own.
 *
 * Run: `pnpm --dir ui run check:exit`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-exit-'))
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}

const ok = (actual, what) => eq(actual, true, what)

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/panes/exitMarker.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
    ],
    { stdio: 'inherit' },
  )

  const { exitMarkerText, exitMarkerBytes, showsCode, markFor } = await import(
    `file://${join(out, 'exitMarker.js')}`
  )

  // --- the distinction the Rust side exists to preserve --------------------------------
  //
  // If these three ever read alike again, everything upstream of here is decoration.
  const failure = exitMarkerText(1)
  const oomKilled = exitMarkerText(137)
  const quitByTheApp = exitMarkerText(143)
  eq(failure, '— exited (1) —', 'an ordinary failure names its status')
  eq(oomKilled, '— exited (137) —', 'a SIGKILL reads as 137, not as 1')
  eq(quitByTheApp, '— exited (143) —', 'a SIGTERM reads as 143, not as 1')
  eq(
    new Set([failure, oomKilled, quitByTheApp]).size,
    3,
    'the machine killing a session and the session failing print the same line',
  )

  // --- and the cases that must stay quiet ----------------------------------------------
  eq(exitMarkerText(0), '— exited —', 'a clean finish does not print "(0)" at the user')
  eq(
    exitMarkerText(undefined),
    '— exited —',
    'the rehydration path has only a boolean and must not invent a number',
  )
  eq(
    exitMarkerText(-1),
    '— exited —',
    'UNKNOWN_EXIT_CODE means "this build cannot say"; "(-1)" invites a lookup that fails',
  )
  eq(showsCode(0), false, 'zero is not shown')
  eq(showsCode(1), true, 'non-zero is shown')
  eq(showsCode(undefined), false, 'no code is not shown')
  eq(showsCode(-1), false, 'an unreadable status is not shown')
  eq(showsCode(1.5), false, 'a non-integer is not a shell status and is not shown')

  // --- the framing ----------------------------------------------------------------------
  //
  // Checked because the marker is written into a terminal, not into the DOM: a missing reset
  // leaves every later byte the child prints dim, and a missing leading CRLF puts this on the
  // end of whatever half-line the child died mid-way through.
  const ESC = String.fromCharCode(27)
  const bytes = exitMarkerBytes(137)
  ok(bytes.startsWith(`\r\n${ESC}[2m`), 'the marker starts a fresh line and opens dim')
  ok(bytes.endsWith(`${ESC}[0m\r\n`), 'the marker closes SGR so later output is not left dim')
  ok(bytes.includes('— exited (137) —'), 'the framing wraps the text rather than replacing it')

  // --- the second path in, the one that used to lose the number -------------------------
  //
  // Exit reaches a pane from two directions: the `cide://session-state` event, which carries
  // `code`, and a one-shot question asked at attach time for a child that died before this
  // pane existed. The question was `session_has_exited`, which answers a boolean, so the
  // rehydration path printed a bare `— exited —` over a status the registry was still
  // holding. `markFor` is the decision that path should be making instead.
  eq(markFor({ kind: 'running' }), null, 'a live child is not marked')
  eq(
    markFor({ kind: 'exited', code: 137 }),
    { code: 137 },
    'a reaped child hands over the status it really ended with',
  )
  eq(
    markFor({ kind: 'unknown' }),
    {},
    'a session the registry never held has no code, and that is the only bare marker',
  )
  eq(
    markFor({ kind: 'reaping' }),
    null,
    'gone but not yet reaped declines: `markExited` is one-shot, so marking now would beat ' +
      'the event that carries the number and print the codeless line instead',
  )
  eq(
    exitMarkerText(markFor({ kind: 'exited', code: 143 })?.code),
    '— exited (143) —',
    'the answer feeds the marker unchanged — no second place for a code to be dropped',
  )
  eq(
    exitMarkerText(markFor({ kind: 'unknown' })?.code),
    '— exited —',
    'and the case with no code still does not invent one',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('exit marker: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
