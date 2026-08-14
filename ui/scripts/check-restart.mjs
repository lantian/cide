/**
 * Checks `src/panes/restartRule.ts` — what a pane whose child has died offers the user.
 *
 * The defect it answers: double Ctrl+C quits the Claude CLI, and the pane it left behind was a
 * dead end. `claude.restart` was in the command registry marked `unavailable`, so the palette
 * greyed it out and no key could be bound to it; the terminal's context menu explained in a
 * comment why it was not there; typing into the pane wrote to a dead pty and the failure was
 * discarded; and the project console's pane cannot be closed. Quitting the app was the only way
 * out.
 *
 * So the rule is pinned here rather than left inside a render, which is where both of this
 * project's paid-for lessons say a rule goes to hide. Three properties, and each is a control
 * that would otherwise be wrong in a way nothing fails on:
 *
 *   * a **live** pane offers nothing — a Restart bar over a working terminal is worse than none;
 *   * an exited pane **always** offers a way back, shells included;
 *   * Resume appears **only** when a transcript really exists, and never for a shell. A Resume
 *     button that spawns `claude --resume` for a transcript that is gone is a control that fails
 *     after it is pressed, which is the same defect one layer down.
 *
 * CommonJS emit and `require`, not ESM: the module imports `./exitMarker` for the status line
 * (one wording, not two), and Node's ESM loader will not resolve an extensionless relative
 * import. Same reason `check-key-gate.mjs` compiles its four modules that way.
 *
 * Run: `pnpm --dir ui run check:restart`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-restart-'))
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}
const ok = (cond, what) => eq(cond, true, what)

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/panes/restartRule.ts',
      '--outDir', out,
      '--rootDir', 'src',
      '--module', 'commonjs',
      '--moduleResolution', 'node10',
      '--target', 'es2022',
      '--strict',
      '--exactOptionalPropertyTypes',
    ],
    { stdio: 'inherit' },
  )

  const require = createRequire(import.meta.url)
  const { restartOffer } = require(join(out, 'panes/restartRule.js'))

  const facts = (over = {}) => ({ kind: 'claude', exited: true, resumable: false, ...over })

  /* --------------------------------------------------------- nothing over a living pane */

  for (const kind of ['claude', 'shell']) {
    for (const resumable of [false, true]) {
      eq(
        restartOffer(facts({ kind, resumable, exited: false })),
        null,
        `a live ${kind} pane offers nothing (resumable=${resumable})`,
      )
    }
  }

  /* ------------------------------------------------- and always something over a dead one */

  for (const kind of ['claude', 'shell']) {
    for (const resumable of [false, true]) {
      for (const code of [undefined, 0, 1, 130, 137]) {
        const offer = restartOffer(facts({ kind, resumable, code }))
        ok(offer !== null, `an exited ${kind} pane offers a way back (code=${code})`)
        eq(offer.primary.mode, 'fresh', 'the accented control starts a new session')
        ok(offer.primary.label.length > 0, 'and it is labelled')
      }
    }
  }

  /* ------------------------------------------------------------------ fresh versus resume */

  eq(
    restartOffer(facts({ kind: 'claude', resumable: true })).secondary,
    { mode: 'resume', label: 'Resume this conversation' },
    'a Claude pane with a transcript offers resume beside the fresh start',
  )
  eq(
    restartOffer(facts({ kind: 'claude', resumable: false })).secondary,
    null,
    'and offers nothing to resume when the transcript is gone — a button that would fail',
  )
  eq(
    restartOffer(facts({ kind: 'shell', resumable: true })).secondary,
    null,
    'a shell is never resumable however the flag is set: `bash` has no conversation, and Rust ' +
      'answers `Fresh` for every non-Claude pane for the same reason',
  )
  eq(
    restartOffer(facts({ kind: 'claude', resumable: true })).primary.mode,
    'fresh',
    'fresh stays the primary action even when resume is available — the user asked for fresh, ' +
      'and resume is one click away rather than the default',
  )

  /* ------------------------------------------------------------------------- the wording */

  // The same sentence the transcript carries, from the same function, because the bar sits over
  // the last lines of the pane and may be covering the `— exited (n) —` line itself.
  eq(
    restartOffer(facts({ code: 137 })).status,
    '— exited (137) —',
    'the bar repeats the status, so the number is never the thing it hides',
  )
  eq(
    restartOffer(facts({ code: 0 })).status,
    '— exited —',
    'a clean finish still does not print "(0)" — one rendering of a code, in exitMarker.ts',
  )
  eq(
    restartOffer(facts({ code: undefined })).status,
    '— exited —',
    'and a code nothing can speak for is not invented here either',
  )
  eq(
    restartOffer(facts({ kind: 'shell' })).primary.label,
    'Restart',
    'a shell is restarted, not "given a new session" — it never had one',
  )
  eq(
    restartOffer(facts({ kind: 'claude' })).primary.label,
    'Start a new session',
    'and a Claude pane says what a fresh start actually costs: a new conversation',
  )

  if (failed > 0) {
    console.error(`\ncheck-restart: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log('restart offer: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
