/**
 * Checks `src/chrome/notices.ts` — the rules behind the toast stack.
 *
 * Same shape as `check-branches.mjs` and `check-git-tree.mjs`, and for the same reason: this
 * project has no JS test runner, and adding one for a handful of pure functions would be a
 * larger commitment than the code it tests.
 *
 * # Why these particular rules are worth a gate
 *
 * Every one of them fails **silently**. That is the whole argument.
 *
 *   * `admit` collapses by message. That is right for a user retrying one gesture and
 *     catastrophic for a command that speaks for several repositories: five submodules each
 *     reporting "Already up to date with origin" collapse into one toast, and the surface
 *     that was built to prove a command is not dead then under-reports four of them with
 *     nothing on screen to say so. `git.pull` aggregates before it notifies precisely because
 *     of this, and the collapse is pinned here so the aggregation stays necessary rather than
 *     becoming a habit nobody remembers the reason for.
 *   * `describe` is what stands between a tagged `GitError` on the wire and `[object Object]`
 *     in a toast. A control that says `[object Object]` is a control the user reads as broken.
 *   * The cap drops the *oldest*. Dropping the newest instead would hide the failure that just
 *     happened behind two that already had their chance to be read.
 *
 * What this does NOT cover, and nothing here should be read as claiming: that the toast is
 * rendered, that `role="alert"` reaches a screen reader, or that `unhandledrejection` fires.
 * Those are JSX and a live window; `Failures.tsx` owns them.
 *
 * Run: `pnpm --dir ui run check:notices`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-notices-'))

let failed = 0
const fail = (what, detail) => {
  failed += 1
  console.error(`FAIL ${what}${detail === undefined ? '' : `\n  ${detail}`}`)
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
  const tsconfig = join(out, 'tsconfig.json')
  writeFileSync(
    tsconfig,
    JSON.stringify({
      compilerOptions: {
        target: 'es2022',
        module: 'esnext',
        moduleResolution: 'bundler',
        strict: true,
        exactOptionalPropertyTypes: true,
        noUncheckedIndexedAccess: true,
        verbatimModuleSyntax: true,
        skipLibCheck: true,
        noEmitOnError: true,
        outDir: out,
        rootDir: join(UI, 'src'),
        baseUrl: UI,
        paths: { '@/*': ['src/*'] },
        types: [],
      },
      files: [join(UI, 'src', 'chrome', 'notices.ts')],
    }),
  )
  execFileSync('node', ['node_modules/typescript/bin/tsc', '--project', tsconfig], {
    stdio: 'inherit',
    cwd: UI,
  })

  const m = await import(`file://${join(out, 'chrome', 'notices.js')}`)

  // --- what gets in ---------------------------------------------------------------------

  const notice = (id, text, kind = 'error') => ({ id, kind, text })

  eq(
    m.admit([], notice(1, 'boom')).map((n) => n.text),
    ['boom'],
    'the first notice is admitted',
  )
  const two = m.admit(m.admit([], notice(1, 'boom')), notice(2, 'other'))
  eq(two.map((n) => n.text), ['boom', 'other'], 'a different message is appended, oldest first')

  const same = m.admit(two, notice(3, 'boom'))
  eq(same, two, 'a repeat is declined')
  ok(
    same === two,
    'and declined by returning the SAME array — a new array of equal contents would '
      + 're-render every `useSyncExternalStore` reader on every duplicate',
  )
  eq(
    m.admit([notice(1, 'boom', 'error')], notice(2, 'boom', 'info')).length,
    1,
    'the comparison is the message alone: an error and a report that say the same words are '
      + 'the same words, and two toasts would explain no more than one',
  )

  eq(m.MAX_SHOWN, 3, 'three at once — beyond that the stack covers the pane it is reporting on')
  let stack = []
  for (const [i, text] of ['a', 'b', 'c', 'd', 'e'].entries()) {
    stack = m.admit(stack, notice(i + 1, text))
  }
  eq(
    stack.map((n) => n.text),
    ['c', 'd', 'e'],
    'the cap drops the OLDEST — the newest notice is the one the user just caused, and '
      + 'hiding it behind two that have already been on screen is the wrong way round',
  )

  // --- turning a rejection into a sentence -------------------------------------------------

  eq(m.describe('boom'), 'boom', 'a thrown string is already a sentence')
  eq(m.describe(new Error('boom')), 'boom', 'an Error keeps its message')
  eq(
    m.describe({ message: 'no such file' }),
    'no such file',
    'the wire form is `{kind, message}` — `message` is the half written for a person',
  )
  eq(
    m.describe({ kind: 'notFastForward', detail: { ahead: 2 } }),
    'notFastForward',
    'a raw tagged error with no message falls back to its tag rather than to `[object '
      + 'Object]`. It is a poor sentence, which is why `dispatch.ts` runs git errors through '
      + '`branchModel.explain` first — but it must never be worse than poor.',
  )
  /*
   * The tail. `String(reason)` was the whole of it, and for any object without a `message` or
   * a `kind` that is the literal string `[object Object]` — a toast that reads as a rendering
   * bug rather than as a command that failed, which is the exact symptom this surface exists
   * to prevent, produced by the surface itself.
   */
  for (const odd of [null, undefined, {}, { message: '' }, { kind: 7 }, [], '']) {
    const said = m.describe(odd)
    ok(
      !said.includes('[object Object]'),
      `describe never renders an object literally: ${JSON.stringify(odd)} — got ${JSON.stringify(said)}`,
    )
    ok(
      said.length > 0,
      `describe never produces an empty toast: ${JSON.stringify(odd)}`,
    )
  }
  eq(
    m.describe({}),
    'A command failed and its error carried no message. The console has the details.',
    'and when there is genuinely nothing to say, it says that and points at the console — '
      + 'the listener in `Failures.tsx` logs the real object before it reports',
  )
  ok(
    m.describe({ path: '/w/a.rs', code: 42 }).includes('/w/a.rs'),
    'a shape with no known field is still shown, so an unrecognised error is diagnosable',
  )
  ok(
    m.describe({ blob: 'x'.repeat(4000) }).length <= 240,
    'but capped — a rejected `invoke` can carry a whole DTO and a toast is four lines high',
  )
  eq(m.describe(42), '42', 'anything with a useful `String` still uses it')

  // --- the one hint a user can act on ------------------------------------------------------

  ok(
    m.hintFor('Command pane_add_row not found')?.includes('cargo build'),
    'the stale-binary case is named precisely: `run.sh` hot-reloads the frontend past a '
      + 'pre-built binary, and without this the message reads as an internal error',
  )
  eq(m.hintFor('unknown command: x') !== undefined, true, "and the other spelling Tauri uses")
  eq(m.hintFor('main has no upstream branch to pull from'), undefined, 'nothing else gets it')

  // --- the store, which is what `dispatch.ts` reaches for ----------------------------------

  m.clearNotices()
  eq(m.getSnapshot(), [], 'it starts empty')
  ok(
    m.getServerSnapshot() === m.getServerSnapshot(),
    'the server snapshot is a stable reference — a fresh `[]` per call is an infinite '
      + 'render loop in `useSyncExternalStore`',
  )

  let woken = 0
  const stop = m.subscribe(() => {
    woken += 1
  })
  m.notify('pulled 3 commits')
  eq(m.getSnapshot().length, 1, 'notify puts one on screen')
  eq(m.getSnapshot()[0].kind, 'info', 'and a report is the default kind — the failures come '
    + 'in through `notifyFailure`, which cannot get the kind wrong')
  eq(woken, 1, 'and wakes the subscriber')

  m.notify('pulled 3 commits')
  eq(m.getSnapshot().length, 1, 'a duplicate is still declined through the store')
  eq(woken, 1, 'and does not wake anyone, because nothing changed')

  m.notifyFailure({ message: 'Command pane_add_row not found' })
  const failure = m.getSnapshot()[1]
  eq(failure.kind, 'error', 'notifyFailure reports an error')
  ok(failure.hint !== undefined, 'and derives the hint rather than making every caller remember it')

  m.notify('quiet', { kind: 'info' })
  eq(
    m.getSnapshot()[2].hint,
    undefined,
    'a report never carries the stale-binary hint: it is a fact about a failed command, and '
      + 'attaching it to a success would tell a user to rebuild after a pull that worked',
  )

  m.notify('with a body', { detail: 'a1b2c3d4  Add the thing' })
  eq(
    m.getSnapshot().map((n) => n.text),
    ['Command pane_add_row not found', 'quiet', 'with a body'],
    'the cap applies through the store too',
  )

  const id = m.getSnapshot()[0].id
  m.dismiss(id)
  eq(m.getSnapshot().length, 2, 'dismiss removes exactly the one it names')
  ok(m.getSnapshot().every((n) => n.id !== id), 'and not by index, which shifts under it')

  stop()
  const before = woken
  m.notify('after unsubscribing')
  eq(woken, before, 'an unsubscribed listener is not called')
  m.clearNotices()

  /* ------------------------------------------------------- the text has to be copyable out */
  //
  // A notice exists to carry something the user cannot get anywhere else — the sentence a
  // command failed with, the path it refused, the stderr of a resolver that could not run — and
  // the toast is gone the moment it is dismissed. Unselectable, that means retyping an
  // `os error 28` by hand.
  //
  // Asserted on the stylesheet because there is no DOM here, and asserted as a PAIR because the
  // engine makes the pair load-bearing: WebKitGTK drops unprefixed `user-select` entirely, so an
  // opt-in written only in the modern spelling inherits the root's prefixed `none` and silently
  // takes copy-out away while reading as correct. check:css-prefix enforces the pairing across
  // every stylesheet; this pins that the opt-in exists here at all.
  // Comments stripped first. Without it the selector list picks up the prose immediately above
  // the rule — which names `.summary` and `.dismiss` in order to say they are excluded — and the
  // negative assertions below would fail on their own explanation.
  const css = readFileSync(join(UI, 'src', 'chrome', 'Failures.module.css'), 'utf8').replace(
    /\/\*[\s\S]*?\*\//g,
    '',
  )
  const optIn = /(^|\})[^{]*\{[^}]*-webkit-user-select:\s*text[^}]*\buser-select:\s*text[^}]*\}/m
  const rule = optIn.exec(css)?.[0] ?? ''
  eq(rule !== '', true, 'the toast opts back in to selection, in both spellings')
  for (const part of ['.text', '.hint', '.detail']) {
    eq(
      rule.includes(part),
      true,
      `${part} is selectable — it is one of the three things a notice carries that is worth copying`,
    )
  }
  // ...and the click targets are NOT, so a drag beginning on the disclosure triangle opens it
  // rather than starting a selection.
  for (const part of ['.summary', '.dismiss']) {
    eq(rule.includes(part), false, `${part} stays unselectable — it is a control, not text`)
  }
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\ncheck-notices: ${failed} failure(s)`)
  process.exit(1)
}
console.log('check-notices: ok')
