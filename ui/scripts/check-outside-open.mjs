/**
 * Checks `src/terminal/outsideOpen.ts` — when cide may ask *"open a file from outside this
 * project?"*, and what that dialog says.
 *
 * Same shape as `check-fs-clipboard.mjs` and `check-paths.mjs`: no JS test runner in this
 * project, so the rules live in a pure import-free module and the TypeScript in `node_modules`
 * compiles it on its own for node to drive.
 *
 * # The two failures this exists to prevent
 *
 * 1. **A dialog offered for a refusal nothing can answer.** Rust refuses a terminal-named path
 *    for five different reasons and exactly one of them is overrulable. An *Open anyway* button
 *    on a device node, a FIFO, a 2 GB log or a path that no longer exists would be pressed, would
 *    fail, and would teach the user that approving is how you make cide stop complaining — which
 *    is the only way a confirmation like this can fail. So `kind === 'outside'` **and**
 *    `real !== null`, and both halves are pinned from both directions below.
 * 2. **A dialog that does not name what it is about.** The entire safeguard is the user reading
 *    the path, so a basename, a count, or a message with the path elided is the feature not
 *    working while looking exactly as though it does. Every case here asserts the path is in the
 *    list, and the symlink case asserts *both* paths are.
 *
 * The source pins at the foot are the other half: this project has now found more than a dozen
 * features that were complete, correct and reachable from nothing, so the rules being right is
 * not evidence that anything calls them.
 *
 * Run: `pnpm --dir ui run check:outside-open`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-outside-open-'))

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
  /*
   * A bare `tsc` with no tsconfig, deliberately — the same rule `check-paths.mjs` states:
   * `outsideOpen.ts` imports nothing, and if this compile ever needs a tsconfig then an import
   * has crept in and the rules have stopped being node-testable, which is the whole reason they
   * are in a module of their own.
   */
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/terminal/outsideOpen.ts',
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

  const { outsideAsk } = await import(`file://${join(out, 'outsideOpen.js')}`)

  const SECRET = '/home/lantian/.claude/.credentials.json'
  const refusal = (over) => ({
    kind: 'outside',
    message: `${SECRET} is outside this project's roots.`,
    path: SECRET,
    real: SECRET,
    ...over,
  })

  // --- rule 1: only a refusal the user can actually overrule may ask -----------------------

  const ask = outsideAsk(refusal())
  ok(ask !== null, 'the ordinary out-of-project file is a question, not a verdict')
  eq(ask.target, SECRET, 'the approval is bound to the canonical target Rust reported')
  eq(ask.files, [SECRET], 'and the dialog names it')

  for (const kind of ['missing', 'notAFile', 'tooLarge', 'failed']) {
    eq(
      outsideAsk(refusal({ kind, real: null })),
      null,
      `\`${kind}\` is final: a device node, a FIFO, a 2 GB log and a path that no longer ` +
        'exists are refused for reasons no approval touches, and an "Open anyway" on one of ' +
        'them is a button that cannot work',
    )
  }
  eq(
    outsideAsk(refusal({ kind: 'notAFile', real: '/dev/zero' })),
    null,
    'and the kind is checked as well as the target — a `real` on a non-outside refusal must ' +
      'not be enough to raise a dialog, whatever put it there',
  )

  eq(
    outsideAsk({
      kind: 'outside',
      message: '../../.ssh/id_rsa is outside this project\'s roots.',
      path: '../../.ssh/id_rsa',
      real: null,
    }),
    null,
    'A MALFORMED PATH NEVER ASKS. `real: null` is Rust saying there is nothing here to ' +
      'approve — it refuses any `..` component whatever is sent back — so a dialog for it ' +
      'would be a question whose "yes" is refused again',
  )

  // --- rule 2: it names the full path, and the real target when they differ ----------------

  const LINK = '/home/lantian/work/cide/src/notes.md'
  const moved = outsideAsk(refusal({ path: LINK, real: SECRET }))
  eq(
    moved.files,
    [LINK, SECRET],
    'a symlink inside the project pointing out of it is the case where the path on screen ' +
      'looks ordinary and the target is the whole point, so BOTH are named',
  )
  eq(moved.target, SECRET, 'and what gets approved is where it actually goes')
  ok(
    moved.body !== outsideAsk(refusal()).body,
    'the two cases do not share a sentence: one says "this is not in your project" and the ' +
      'other has to say "and it resolves somewhere else again"',
  )
  ok(
    ask.files.every((f) => f.startsWith('/')),
    'every path in the list is absolute and complete — `credentials.json` looks like a ' +
      'project file and `/home/you/.claude/.credentials.json` does not, and that difference ' +
      'is the entire safeguard',
  )
  ok(
    ask.title.length > 10 && ask.body.length > 40 && ask.confirmLabel.length > 0,
    'a confirmation with nothing to read is a confirmation nobody reads',
  )
  ok(ask.mark !== '−', 'nothing is being removed, so the removal dash would be a lie')

  // --- a rejection that is not a refusal at all --------------------------------------------

  for (const junk of [null, undefined, 'a string', 42, {}, { kind: 'outside' }, { kind: 5 }]) {
    eq(
      outsideAsk(junk),
      null,
      `a rejection with no refusal shape (${JSON.stringify(junk) ?? 'undefined'}) answers ` +
        'null, which lands the caller on notifyFailure — the honest outcome for something ' +
        'nobody can parse',
    )
  }
  eq(
    outsideAsk({ kind: 'outside', message: 'm', path: '/p', real: 7 }),
    null,
    'and a non-string `real` is not a target: a cast would have let this through rule 1',
  )

  // --- the source pins ----------------------------------------------------------------------

  const read = (rel) => readFileSync(join(UI, rel), 'utf8')

  const app = read('src/App.tsx')
  ok(
    app.includes('outsideAsk('),
    'App.tsx asks the module rather than matching on the refusal itself — a rule in a ' +
      '`.catch` is in the one place no check script can compile',
  )
  ok(
    app.includes('requestOutsideOpen('),
    'and it parks the question, so the refusal reaches a dialog rather than a log line',
  )
  eq(
    app.split('<OutsideOpenGate />').length - 1,
    2,
    'THE GATE IS RENDERED IN BOTH WINDOW KINDS. `App.tsx` returns early for a `pane:<uuid>` ' +
      'window, so one copy means a ctrl+click in a torn-out pane asks nobody and opens ' +
      'nothing — which is exactly the defect `restorePlan` had',
  )

  const links = read('src/terminal/pathLinks.ts')
  ok(
    links.includes('outsidePaths('),
    'pathLinks.ts offers out-of-project candidates at all — without this call the whole ' +
      'feature is unreachable by the gesture it was built for',
  )
  ok(
    links.includes('statPaths('),
    'and it asks the disk about them: the index cannot answer for a path outside every root, ' +
      'so without this every such candidate is silently "not a file" and never lights up',
  )
  ok(
    /OUTSIDE_TTL_MS/.test(links),
    'out-of-project answers age out. `cide://fs-changed` is the project watcher\'s and does ' +
      'not reach them, so an entry in the exactly-invalidated cache would go stale for ever',
  )

  const client = read('src/ipc/client.ts')
  ok(
    client.includes("invoke<TabId>('terminal_open_path'"),
    'the open still goes through the checked command; `tab_open_file` enforces nothing at all',
  )
  ok(
    // Anchored to the invoke's own payload, not merely to the word appearing in the file: the
    // first version of this pin matched the *parameter declaration* and stayed green while the
    // argument was dropped from the object being sent — a dialog whose answer never left the
    // webview, which is precisely the shape of failure the pins exist to catch.
    /invoke<TabId>\('terminal_open_path',\s*\{[^}]*approvedTarget:/.test(client),
    'and the approval is in what it SENDS — a dialog whose answer never leaves the webview ' +
      'is a dialog that opens nothing',
  )

  // Comments stripped, because this file *discusses* the thing it must not do — the argument
  // for why there is no standing approval belongs next to the code that does not have one.
  const uncommented = (rel) =>
    read(rel)
      .replace(/\/\*[\s\S]*?\*\//g, '')
      .replace(/^\s*\/\/.*$/gm, '')

  const store = uncommented('src/chrome/outsideOpenStore.ts')
  ok(
    !/localStorage|sessionStorage|persist\(|dontAsk|rememberChoice|approvedDirs/i.test(store),
    'NO STANDING APPROVAL. One approval becoming a capability for every later line naming a ' +
      'sibling file is precisely what this gate exists to prevent, and the second line is ' +
      'written by whoever wrote the first',
  )
  ok(
    !/tauri|invoke\(/.test(store),
    'and it holds no durable state either: if a remembered approval is ever wanted it belongs ' +
      'in Rust, scoped per project and revocable in Settings, not as a flag one webview keeps',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('out-of-project open: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
