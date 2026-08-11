/**
 * Checks `src/chrome/closeConfirm.ts` — the decision and the wording behind the close
 * confirmation.
 *
 * Two properties are worth pinning, and they pull in opposite directions, which is exactly
 * why they are worth pinning:
 *
 *  1. The dialog appears when something would be lost. A file tab with unsaved edits closed
 *     silently before this existed, and that was the highest-priority defect in the tree.
 *  2. The dialog does *not* appear otherwise. A confirmation on every close is one users
 *     dismiss unread, at which point property 1 is decorative.
 *
 * It also pins the naming: the dialog has to say *which* files, and the singular/plural has
 * to be right. "1 files with unsaved changes" in a destructive dialog is the kind of thing
 * that survives for years because nobody owns it.
 *
 * Same shape as `check-status-format.mjs` and `check-picker.mjs` — there is no JS test
 * runner in this project, and this is a pure module the TypeScript in `node_modules` can
 * compile on its own.
 *
 * Run: `pnpm --dir ui run check:close`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-close-'))
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
      'src/chrome/closeConfirm.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
    ],
    { stdio: 'inherit' },
  )

  const {
    atRisk,
    confirmTitle,
    confirmBody,
    confirmLabel,
    unsavedRow,
    sessionRow,
    stateLabel,
    dirname,
    count,
    parkClose,
    advanceClose,
  } = await import(`file://${join(out, 'closeConfirm.js')}`)

  const file = (title, path) => ({ title, path, projectName: 'cide' })
  const session = (paneTitle, state) => ({ paneTitle, projectName: 'cide', state: { state } })

  const nothing = { unsaved: [], sessions: [] }
  const oneFile = { unsaved: [file('main.rs', '/home/dev/work/cide/src/main.rs')], sessions: [] }
  const threeFiles = {
    unsaved: [
      file('main.rs', '/home/dev/work/cide/src/main.rs'),
      file('mod.rs', '/home/dev/work/cide/src/fs/mod.rs'),
      file('mod.rs', '/home/dev/work/cide/src/git/mod.rs'),
    ],
    sessions: [],
  }
  const busyOnly = { unsaved: [], sessions: [session('cide : claude', 'busy')] }
  const both = { unsaved: oneFile.unsaved, sessions: busyOnly.sessions }

  // --- when the dialog appears, and when it must not ---------------------------------
  eq(atRisk(nothing), false, 'nothing at risk means no dialog — the rule that keeps it read')
  ok(atRisk(oneFile), 'an unsaved buffer is always worth asking about')
  ok(atRisk(busyOnly), 'a session mid-turn is worth asking about')

  // --- it names what would be lost ----------------------------------------------------
  eq(
    confirmTitle('tab', oneFile),
    'main.rs has unsaved changes',
    'one file closing one tab is named outright, not counted',
  )
  eq(
    confirmTitle('project', threeFiles),
    '3 files with unsaved changes',
    'several files are counted in the heading and listed in the body',
  )
  eq(confirmTitle('app', busyOnly), '1 Claude session still working', 'singular, not "1 sessions"')
  eq(count(1, 'file'), '1 file', 'the singular is written out')
  eq(count(3, 'file'), '3 files', 'the plural is written out')

  // Two `mod.rs` are the ordinary case in a Rust tree. A list that shows the name alone has
  // told the user nothing, so the row carries the directory.
  eq(
    threeFiles.unsaved.map(unsavedRow),
    [
      { name: 'main.rs', where: '/home/dev/work/cide/src' },
      { name: 'mod.rs', where: '/home/dev/work/cide/src/fs' },
      { name: 'mod.rs', where: '/home/dev/work/cide/src/git' },
    ],
    'each row names the file and the directory that distinguishes it',
  )
  eq(dirname('/a/b/c.rs'), '/a/b', 'the directory part')
  eq(dirname('/top.rs'), '/', 'a file at the root keeps a visible directory')
  eq(dirname('bare.rs'), '', 'a relative name has no directory rather than a wrong one')

  eq(
    sessionRow(session('cide : claude', 'awaitingPermission')),
    { name: 'cide : claude', where: 'waiting for permission' },
    'a session row says what it is doing',
  )
  eq(stateLabel('busy'), 'working', 'the status bar words it this way')
  eq(stateLabel('somethingNew'), 'somethingNew', 'an unknown state shows itself, not "unknown"')

  // --- the two risks are described as the different things they are -------------------
  ok(
    confirmBody('tab', oneFile).includes('nowhere else'),
    'unsaved edits are described as destruction, because that is what they are',
  )
  ok(
    confirmBody('app', busyOnly).includes('resume'),
    'an interrupted turn is described as recoverable, because it is',
  )
  ok(
    confirmBody('project', both).includes('nowhere else') &&
      confirmBody('project', both).includes('resume'),
    'both risks together say both things',
  )
  ok(confirmBody('app', busyOnly).includes('cide'), 'the app scope names the app')
  ok(confirmBody('tab', oneFile).includes('this tab'), 'the tab scope names the tab')

  // --- the destructive button says what it destroys -----------------------------------
  eq(
    confirmLabel(oneFile),
    'Discard changes and close',
    'the button that loses work says so; "OK" would not',
  )
  eq(confirmLabel(busyOnly), 'Close anyway', 'nothing is discarded when only a turn is at stake')

  // --- several refusals from one gesture ----------------------------------------------
  //
  // `Close others` in the tab context menu issues one close per tab and they run
  // concurrently, so two can be refused for unsaved changes in the same tick. The store used
  // to let the second replace the first: the user answered about one file and the other tab
  // stayed open with nothing said. These four assertions are that bug.
  eq(parkClose(null, [], 'a'), { pending: 'a', queued: [] }, 'the first refusal is the dialog')
  eq(
    parkClose('a', [], 'b'),
    { pending: 'a', queued: ['b'] },
    'a second refusal queues behind the dialog rather than replacing it',
  )
  eq(
    parkClose('a', ['b'], 'c'),
    { pending: 'a', queued: ['b', 'c'] },
    'and a third joins the tail, in the order they were refused',
  )
  eq(
    advanceClose(['b', 'c']),
    { pending: 'b', queued: ['c'] },
    'answering one promotes the next — one dialog on screen, every file asked about',
  )
  eq(advanceClose([]), { pending: null, queued: [] }, 'the last answer leaves no dialog')

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('close confirm: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
