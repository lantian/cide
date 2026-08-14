/**
 * Checks `src/windows/detachedPane.ts` — what a `pane:<uuid>` window may show.
 *
 * Worth a check because the failure it replaces was invisible to `tsc` and nearly invisible to
 * a reader. `DetachedPaneWindow` asked `needsSession(pane)`, got `false` for an `editor` pane —
 * correctly; an editor runs no child — and used that as *"so it is not orphaned"*, leaving
 * `TerminalPane` as the only remaining branch. A detached editor pane would have **spawned a
 * shell in a window titled `main.rs`**. Nothing about that is a type error, and the two lines
 * that produced it read fine in isolation.
 *
 * It was out of reach only because `layout::take_pane` refuses the last pane of a tab and a file
 * tab opens with one; splitting the file tab first — offered on the pane's own menu — puts
 * Detach right there. "Implemented and mis-wired", not "not implemented".
 *
 * Same shape as `check-window-controls.mjs`: a bare `tsc` over one import-free file, then import
 * the output and assert.
 *
 * Run: `pnpm --dir ui run check:detached`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-detached-'))

let failed = 0
const fail = (what, detail) => {
  console.error(`FAIL ${what}${detail === undefined ? '' : `\n  ${detail}`}`)
  failed++
}
const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) fail(what, `actual:   ${a}\n  expected: ${b}`)
}
const ok = (cond, what) => {
  if (!cond) fail(what)
}

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/windows/detachedPane.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
    ],
    { cwd: UI, stdio: 'inherit' },
  )

  const { detachedContent, needsSession } = await import(`file://${join(out, 'detachedPane.js')}`)

  const kindOf = (pane) => detachedContent(pane).kind

  // --- the panes this window is for --------------------------------------------------------

  eq(
    kindOf({ kind: 'claude', session: 'a2f1' }),
    'terminal',
    'a torn-out Claude pane arrives with its session and attaches to it — the whole feature',
  )
  eq(kindOf({ kind: 'shell', session: 'a2f1' }), 'terminal', 'and so does a shell')

  // --- the pane that would have spawned a shell in a window titled main.rs ------------------

  eq(
    kindOf({ kind: 'editor', session: null }),
    'unsupported',
    'AN EDITOR IS REFUSED, not rendered as a terminal. `EditorPane` needs a TabId — the ' +
      'buffer is registered per tab in `openBuffers.ts` — and this window holds a pane that ' +
      'has been taken out of its tab. Falling through to `TerminalPane` spawns a shell.',
  )
  eq(
    kindOf({ kind: 'diff', session: null }),
    'unsupported',
    'and so is a diff, for the same reason and by the same default',
  )
  ok(
    detachedContent({ kind: 'editor', session: null }).message.includes('Redock'),
    'the refusal names the way out; the window offers exactly one other control',
  )
  ok(
    !detachedContent({ kind: 'editor', session: null }).message.includes('no session'),
    'and it does NOT say "this pane has no session, redock it to start one" — an editor will ' +
      'never have one, so that sentence is an instruction to press a button that fixes nothing',
  )

  // --- the ordering that makes the sentence right ------------------------------------------

  eq(
    kindOf({ kind: 'claude', session: null }),
    'orphaned',
    'a pane that DOES run a child and arrived without one is orphaned — the state that ' +
      'should not occur, rendered rather than ignored, because a fall-through to a spawn ' +
      'starts a child whose id no window records',
  )

  // --- a kind nobody has written yet -------------------------------------------------------

  ok(
    needsSession({ kind: 'notebook', session: null }),
    'an unknown kind is assumed to run a child, so a `PaneKind` added later ends up refused ' +
      'as orphaned rather than silently spawning a shell — the safe direction of the guess',
  )
  eq(
    kindOf({ kind: 'notebook', session: 'a2f1' }),
    'terminal',
    'and one that arrives with a session is treated as the terminal-ish thing it claims to be',
  )

  // --- the source pins ----------------------------------------------------------------------

  const window = readFileSync(join(UI, 'src/windows/DetachedPaneWindow.tsx'), 'utf8')
  ok(
    window.includes('detachedContent('),
    'the window asks this module rather than re-deriving the rule — re-deriving it is how it ' +
      'went wrong the first time',
  )
  ok(
    !/needsSession\(pane\)\s*&&/.test(window),
    'and the two-line version it grew out of is gone, not left beside its replacement',
  )
  ok(
    /content\.kind === 'terminal'/.test(window),
    'TerminalPane is rendered for the terminal answer ONLY. Any other shape of this branch — ' +
      'a negated orphan check, a truthiness test — is how a shell gets spawned for an editor',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('detached pane window: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
