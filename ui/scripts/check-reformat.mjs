/**
 * Checks Reformat code — Ctrl+Alt+F. (M26)
 *
 * Two halves, and the first is the one with teeth.
 *
 * **`src/editor/formatModel.ts`**, compiled standalone and run under node. `minimalChange` is
 * the difference between a format that leaves the caret, the scroll, the folds and the undo
 * history where they were, and one that resets all four — and every way it can be wrong is
 * silent. A prefix that is one code unit too long produces a change that is still *correct* and
 * merely larger; a suffix that overlaps the prefix produces a `to` before `from`, which
 * CodeMirror accepts and applies as an empty insertion, quietly deleting text. Nothing throws in
 * either case, no snapshot moves, and the user loses characters.
 *
 * **The wiring**, by grep, because a command that is registered and unreachable is this
 * repository's named worst failure mode and the round trip has four seams that can each be
 * dropped without a type error: the flush before the request, the staleness check after it, the
 * read-only refusal, and the `Unchanged` arm that must dispatch nothing.
 *
 * Run: `pnpm --dir ui run check:reformat`   (or `node ui/scripts/check-reformat.mjs`)
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-format-'))

let failed = 0
const fail = (what, detail) => {
  console.error(`FAIL ${what}${detail === undefined ? '' : `\n  ${detail}`}`)
  failed++
}
const deep = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) fail(what, `actual:   ${a}\n  expected: ${b}`)
}
const ok = (cond, what) => {
  if (!cond) fail(what)
}

try {
  /*
   * `formatModel.ts` deliberately imports nothing, so a bare `tsc` with no tsconfig is enough.
   * If this compile ever needs one, something has added an import and the node-testability that
   * makes this arithmetic checkable at all is gone — which is why the last assertion in this
   * file greps for exactly that.
   */
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/editor/formatModel.ts',
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

  const { minimalChange, isStillCurrent } = await import(`file://${join(out, 'formatModel.js')}`)

  /** Apply a change the way `EditorView.dispatch` would, so every case is checked end to end. */
  const apply = (before, change) =>
    change === null ? before : before.slice(0, change.from) + change.insert + before.slice(change.to)

  // --- identical text dispatches nothing -----------------------------------------------------

  /*
   * The common case: Ctrl+Alt+F on an already-formatted file. `null` and not a zero-width change,
   * because a dispatched transaction dirties the tab and pushes an undo step even when it
   * inserts nothing — so the reflexive keystroke would mark a clean file modified.
   */
  deep(minimalChange('fn main() {}\n', 'fn main() {}\n'), null, 'identical text is no change')
  deep(minimalChange('', ''), null, 'two empty documents are no change')

  // --- the trim is actually minimal ----------------------------------------------------------

  {
    // One line of a three-line file is reindented. The change must not span the file, or the
    // caret on line 3 moves and the undo step swallows everything.
    const before = 'a = 1\n    b = 2\nc = 3\n'
    const after = 'a = 1\nb = 2\nc = 3\n'
    const change = minimalChange(before, after)
    deep(apply(before, change), after, 'reindenting one line reproduces the text')
    // `{ from: 6, to: 10, insert: '' }` — the four spaces, and nothing else. Not merely "smaller
    // than the document": the trim reaches the tightest span there is, because the text after
    // the indent is common to both and the suffix scan walks all the way back to it.
    deep(change, { from: 6, to: 10, insert: '' }, 'the change is exactly the removed indent')
    ok(
      change.to - change.from < before.length,
      'the change must not span the document, or every position in it is remapped',
    )
  }

  {
    // A pure insertion at the very end — the single most common formatter edit there is.
    const change = minimalChange('x = 1', 'x = 1\n')
    deep(change, { from: 5, to: 5, insert: '\n' }, 'appending a final newline is a 0-width insert')
  }

  {
    // A pure insertion at the very start, which is the boundary the prefix scan can get wrong.
    const change = minimalChange('b\n', 'a\nb\n')
    deep(apply('b\n', change), 'a\nb\n', 'inserting a first line reproduces the text')
    ok(change.from === 0, 'an edit at offset 0 starts at 0')
  }

  {
    // Whole-document reindentation: here the change legitimately *is* most of the file, and the
    // trim must not invent a smaller one.
    const before = 'if x:\n  a\n  b\n'
    const after = 'if x:\n    a\n    b\n'
    deep(apply(before, minimalChange(before, after)), after, 'reindenting every line round-trips')
  }

  // --- the overlap guard ---------------------------------------------------------------------

  {
    /*
     * THE ONE THAT MATTERS. Deleting one of two identical adjacent lines: the common prefix and
     * the common suffix both want the same region. Without the guard the suffix scan runs past
     * the prefix, `to` lands *before* `from`, and CodeMirror applies that as an empty insertion
     * at `from` — deleting more than the formatter asked for, with nothing thrown and nothing on
     * screen to say so.
     */
    const before = 'x\nx\n'
    const after = 'x\n'
    const change = minimalChange(before, after)
    ok(change.to >= change.from, `to must not precede from (got from=${change.from} to=${change.to})`)
    deep(apply(before, change), after, 'deleting a repeated line reproduces the text')
  }

  {
    // The same hazard from the other direction: adding a duplicate line.
    const before = 'x\n'
    const after = 'x\nx\n'
    const change = minimalChange(before, after)
    ok(change.to >= change.from, 'to must not precede from when text grows')
    deep(apply(before, change), after, 'duplicating a line reproduces the text')
  }

  {
    // Total replacement, sharing nothing at either end.
    const change = minimalChange('aaa', 'bbb')
    deep(apply('aaa', change), 'bbb', 'a total rewrite reproduces the text')
  }

  {
    // Emptying a buffer, and filling an empty one.
    deep(apply('abc', minimalChange('abc', '')), '', 'deleting everything reproduces the text')
    deep(apply('', minimalChange('', 'abc')), 'abc', 'filling an empty buffer reproduces the text')
  }

  // --- surrogate pairs -----------------------------------------------------------------------

  {
    /*
     * A JS string is UTF-16 and an emoji is two code units. Cutting between them yields a lone
     * surrogate — not a character, rendered as a replacement glyph, and written into the user's
     * file as one. Both boundaries are nudged off such a cut, and both directions are checked
     * because the prefix guard and the suffix guard are separate code.
     */
    const before = 'a🦀b'
    const after = 'a🦀c'
    const change = minimalChange(before, after)
    deep(apply(before, change), after, 'an edit after an emoji reproduces the text')
    ok(
      !/[\uD800-\uDBFF]$/.test(before.slice(0, change.from)),
      'the prefix must not end mid-surrogate-pair',
    )

    const before2 = 'a🦀b'
    const after2 = 'x🦀b'
    const change2 = minimalChange(before2, after2)
    deep(apply(before2, change2), after2, 'an edit before an emoji reproduces the text')
    ok(
      !/^[\uDC00-\uDFFF]/.test(before2.slice(change2.to)),
      'the suffix must not begin mid-surrogate-pair',
    )

    // And an edit that replaces the emoji itself.
    deep(apply('a🦀b', minimalChange('a🦀b', 'a🐙b')), 'a🐙b', 'replacing an emoji round-trips')
  }

  // --- randomised round trip -----------------------------------------------------------------

  {
    /*
     * The property that matters, over shapes hand-written cases keep missing: whatever the two
     * texts, applying the change reproduces the second exactly. Deterministic seed so a failure
     * is reproducible rather than a Heisenbug in CI.
     */
    let seed = 20260825
    const rand = (n) => {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff
      return seed % n
    }
    const alphabet = ['a', 'b', '\n', ' ', '  ', '🦀']
    let checked = 0
    for (let round = 0; round < 2000; round += 1) {
      const make = () => {
        let text = ''
        for (let i = 0, n = rand(12); i < n; i += 1) text += alphabet[rand(alphabet.length)]
        return text
      }
      const before = make()
      const after = make()
      const change = minimalChange(before, after)
      if (apply(before, change) !== after) {
        fail('randomised round trip', `before=${JSON.stringify(before)} after=${JSON.stringify(after)}`)
        break
      }
      if (change !== null && change.to < change.from) {
        fail('randomised round trip', `inverted range for ${JSON.stringify(before)}`)
        break
      }
      checked += 1
    }
    ok(checked === 2000, `all randomised rounds round-tripped (${checked}/2000)`)
  }

  // --- staleness -----------------------------------------------------------------------------

  ok(isStillCurrent('a\n', 'a\n'), 'an untouched buffer is still current')
  ok(!isStillCurrent('a\n', 'ab\n'), 'a buffer typed into during the round trip is not current')

  // --- the wiring ----------------------------------------------------------------------------

  const read = (path) => readFileSync(join(UI, path), 'utf8')

  const orchestrator = read('src/editor/formatDocument.ts')
  /*
   * `didChange` is a 300 ms trailing throttle, so without an *awaited* flush the formatter reads
   * text up to 300 ms old and its answer is applied to a buffer that has moved on. `savedDoc`
   * carries the same argument for `didSave`, and calls its flush load-bearing in as many words.
   */
  ok(/await\s+flushDoc\(/.test(orchestrator), 'formatDocument awaits flushDoc before requesting')
  ok(
    orchestrator.indexOf('await flushDoc') < orchestrator.indexOf('actions.text()'),
    'the flush happens before the text is read, or the bytes sent are not the bytes the server ' +
      'was just told about',
  )
  ok(
    orchestrator.indexOf('await flushDoc') < orchestrator.indexOf('diagnosticsApi.format('),
    'the flush happens before the request, not after it',
  )
  ok(
    /actions\.readOnly\(\)/.test(orchestrator),
    'formatDocument refuses a read-only buffer before spawning or asking anything',
  )
  {
    // Scoped to the arm itself: a regex with a character budget spills into the next `case` and
    // finds that one's `report(`, which is exactly the reporting this arm must not do.
    const at = orchestrator.indexOf("case 'unchanged':")
    ok(at !== -1, 'formatDocument has an unchanged arm')
    const arm = orchestrator.slice(at + 1)
    const body = arm.slice(0, arm.indexOf('    case '))
    ok(
      !/report\(/.test(body),
      'an already-formatted file is silent — a notice on every reflexive Ctrl+Alt+F trains the ' +
        'user to ignore the channel that carries the real refusals',
    )
  }

  /*
   * The staleness re-check and the trim live in `EditorSurface`'s `FormatActions.apply`, not in
   * the orchestrator: that is where the `EditorView` is, and both need the *live* document
   * rather than the one that was sent. Asserted here rather than there being no assertion,
   * because dropping either is silent — the first discards a keystroke, the second resets the
   * caret and the scroll on every format.
   */
  const surface = read('src/editor/EditorSurface.tsx')
  ok(/isStillCurrent\(/.test(surface), 'EditorSurface re-checks the buffer before applying')
  ok(/minimalChange\(/.test(surface), 'EditorSurface trims the change before dispatching')
  ok(
    /userEvent: 'input\.format'/.test(surface),
    "the format transaction carries a userEvent, or the history plugin treats it as anonymous",
  )
  ok(
    !/changes:\s*\{\s*from:\s*0,\s*to:\s*[a-zA-Z.]*doc\.length/.test(surface),
    'nothing dispatches a whole-document replacement — that is what minimalChange exists to avoid',
  )

  const track = read('src/editor/caretTrack.ts')
  ok(/export function focusedFormat\(/.test(track), 'caretTrack exposes focusedFormat()')
  ok(
    !/^\s*import\s/m.test(track),
    'caretTrack.ts must import nothing — check-outline.mjs compiles it standalone too',
  )

  const dispatch = read('src/keys/dispatch.ts')
  ok(/case 'editor\.format':/.test(dispatch), 'dispatch.ts handles editor.format')
  {
    // The `when` clause gates the palette and never the keyboard, so the chord arrives here from
    // a terminal pane and must be answered with a sentence rather than run against no buffer.
    const arm = dispatch.slice(dispatch.indexOf("case 'editor.format':"))
    const body = arm.slice(0, arm.indexOf('\n      case '))
    ok(/unmet\(command,/.test(body), 'editor.format refuses with unmet(...) rather than silently')
  }

  const client = read('src/ipc/client.ts')
  ok(/'format_document'/.test(client), 'client.ts invokes format_document')

  const model = read('src/editor/formatModel.ts')
  ok(
    !/^\s*import\s/m.test(model),
    'formatModel.ts must import nothing, or this script can no longer compile it standalone',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('reformat code: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
