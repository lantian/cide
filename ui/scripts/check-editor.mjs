/**
 * The editor's pure modules, compiled and exercised under node.
 *
 * M9 shipped without a single automated check. Nothing compiled anything under
 * `src/editor/` except `tsc --noEmit`, and the cost of that showed up immediately:
 * `lineEndings.ts` classified a document whose breaks were all bare `\r` as LF and rewrote
 * every line of it on save. A whole-file rewrite of a user's file is the worst thing this
 * milestone can do, and it survived review because nothing ran the function.
 *
 * Same shape as `check-status-format.mjs` and `check-picker.mjs`: no JS test runner in this
 * project, so the TypeScript already in `node_modules` compiles the modules and this file
 * imports the output. Twelve things are pinned, in the order they appear below.
 *
 *  1. **Line endings.** A round trip — capture, hand the text to CodeMirror, restore — is
 *     byte-identical for LF, CRLF, bare CR *and* mixed. This is the regression that already
 *     bit, and the mixed case is the one that was still open until the per-line record
 *     landed: `Mixed` restored nothing, so the first save of such a file rewrote every break
 *     that was not already `\n`.
 *  2. **The language table.** Extension and whole-name lookup, and that an extension nobody
 *     has heard of degrades to `Plain Text` and a null extension rather than throwing.
 *  3. **The highlight table.** Every tag name the grammars actually emit resolves to a token
 *     class, every class is styled by `EditorSurface.module.css`, and the class → CSS
 *     custom property map the *canvas* uses agrees with the stylesheet the *buffer* uses.
 *     Those two disagreeing is precisely the failure `highlight.ts` exists to prevent, and
 *     it is invisible without a screenshot of a minimap.
 *  4. **The minimap's arithmetic.** Rows, the sliding window, bar extents, the viewport
 *     rectangle and click-to-line — everything in `minimapGeometry.ts`.
 *  5. **The stream grammars.** Every `token()` call advances the stream, over a corpus of
 *     each language crossed with every *other* language's source; and tokenizing a line
 *     scales with its length rather than its square.
 *  6. **The size gate's unit.** `byteSize.ts` gives the same answer as `TextEncoder` without
 *     allocating the encoding — the gate is spelled in bytes and was being compared against
 *     UTF-16 code units, which measures a CJK file at a third of its size.
 *  7. **The open-buffer registry.** What `file.save` and *Save and close* both go through:
 *     which tab a save means, which tabs can be saved, what happens when one cannot, and
 *     that "no editor here" is distinguishable from "the write failed".
 *  8. **The reveal queue and its clamp.** What a search-result click goes through: that a
 *     request made before the editor mounts is parked rather than dropped, that one made
 *     while it is open still arrives, that neither the queue nor the registry leaks — and
 *     that a target pointing past the end of a file that has changed since the search
 *     clamps instead of throwing out of `dispatch` and taking the window down.
 *  9. **Send lines to Claude.** The line arithmetic, the singular/plural label, the `#L10-20`
 *     mention spelling — and that the menu item and the ⌥⏎ binding both actually call it.
 * 10. **The status bar's file readout.** `Markdown · UTF-8 · LF · Ln 7, Col 48` is composed
 *     in `editor/` and painted in `chrome/`, over one slot that any number of split editors
 *     claim: that the newest holds it, that focus takes it back, that closing hands it down
 *     rather than blanking the bar, and that a released handle can no longer write to it.
 * 11. **Arriving where a mention landed.** What *send lines to Claude* does after the send:
 *     which tab has to be activated, whose maximize has to go, which window has to be raised
 *     — and the two cases where the answer is "none of it, say so instead". Every branch is a
 *     way for the user to be told nothing happened when something did, which is the report
 *     this feature has now been filed under twice.
 * 12. **Keys into the editor.** Undo/redo, the find bar and the focus handoff — three reports
 *     with one root, which is a binding that exists and a surface that cannot reach it. The
 *     match ordinal (`findMatches.ts`) and the reveal plan (`revealRequest.ts::planReveal`) are
 *     driven directly; the rest is source assertions, because every one of those bugs was a
 *     *missing call* rather than a wrong function and a module's own tests cannot see one.
 * 13. **Position memory and navigation history.** Two features that both have to say where
 *     somebody is in a file, sharing one type (`position.ts`) and deliberately not one store.
 *     The clamp, the "is this worth an IPC call" rule and the restore veto are driven; so is
 *     the whole of what Back and Forward decide (`navHistory.ts`) — the merge rule, the
 *     discarded forward tail, the refresh-from-the-live-caret, the cap, and a four-thousand-op
 *     fuzz walk that the cursor may never leave the array during. Then the call sites, which
 *     is where this class of bug actually lives: that `EditorSurface` installs the tracker and
 *     consults the veto, that `EditorPane` feeds both props and *flushes* rather than cancels
 *     on unmount, and — the one that matters most — that **no `requestReveal` survives outside
 *     the `jump.ts` seam**, because "remember to record the jump too" spread over eight call
 *     sites is a rule enforced by memory.
 *
 * The fifth found two more quadratics on its first run — the markdown link matcher and the
 * shell `${…}` matcher, both the same unbounded-scan-then-backtrack shape as the YAML key
 * regex that was fixed by hand a commit earlier. Both are fixed in this change.
 *
 * # What this cannot reach, and does not pretend to
 *
 * `EditorSurface.tsx`, `minimap.ts`, `find.ts` and `viewTracker.ts` are not compiled here. They need a
 * `CanvasRenderingContext2D`, a scroller with real geometry, a composition event and a live
 * `data-theme` switch — a window, in other words. The minimap's *painting*, IME preedit
 * under fcitx5, and a theme toggle with terminals running are out of reach of any headless
 * check and are still untested. `minimapGeometry.ts` exists so that the half of the minimap
 * that is arithmetic is not out of reach too, and `findMatches.ts` was split out of `find.ts`
 * in M12 for exactly the same reason — as were `position.ts` and `navHistory.ts`, which is why
 * `viewTracker.ts` and `jump.ts` are left holding a `getBoundingClientRect` and a `Map` and no
 * decisions at all.
 *
 * What section 12 adds for those three files is *source* assertions rather than execution. That
 * is a weaker gate and it is chosen deliberately: `mount()` not focusing its field, `onKeyDown`
 * not bridging to `search-panel` scope, and a diff pane with no `history()` are all absences,
 * and an absence is exactly what a test of the surrounding code passes over. Where a decision
 * could be lifted out into something runnable it was — that is what `planReveal` is.
 *
 * Run: `pnpm --dir ui run check:editor`
 */
import { execFileSync } from 'node:child_process'
import { mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { join, resolve } from 'node:path'

/*
 * Built under `node_modules/.cache` rather than the system temp dir, which is what
 * `check-status-format.mjs` uses. `highlight.ts` imports `@codemirror/language` and
 * `@lezer/highlight` for real, and node resolves those from wherever the output sits: under
 * /tmp there is no `node_modules` above it and the import fails. `check-git-render.mjs`
 * builds here for the same reason.
 */
mkdirSync('node_modules/.cache', { recursive: true })
const out = resolve('node_modules/.cache/cide-editor')
rmSync(out, { recursive: true, force: true })
mkdirSync(out, { recursive: true })

let failed = 0
// Counted and printed on success. `ok` on its own tells you nothing about whether the run
// asserted three things or three hundred, and a check that quietly stops asserting is the
// failure mode this whole file exists to answer.
let checked = 0

const eq = (actual, expected, what) => {
  checked += 1
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    failed += 1
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
  }
}

const ok = (condition, what) => {
  checked += 1
  if (!condition) {
    failed += 1
    console.error(`FAIL ${what}`)
  }
}

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/editor/lineEndings.ts',
      'src/editor/byteSize.ts',
      'src/editor/openBuffers.ts',
      'src/editor/revealRequest.ts',
      'src/editor/languages.ts',
      'src/editor/highlight.ts',
      'src/editor/minimapGeometry.ts',
      'src/editor/streamGrammar.ts',
      'src/editor/sendToClaude.ts',
      'src/editor/revealTarget.ts',
      'src/editor/statusReadout.ts',
      // M12. All three are import-free for exactly this reason.
      'src/editor/lintMap.ts',
      'src/editor/caretTrack.ts',
      'src/editor/highlightLevel.ts',
      // The find bar's arithmetic, split out of `find.ts` — which needs a window — for exactly
      // this reason. See section 12.
      'src/editor/findMatches.ts',
      // M15, section 14. Autosave's whole policy — pure and import-free, because a feature that
      // writes the user's files on a timer must have its refusals somewhere a check can drive
      // them rather than inside a `useEffect`.
      'src/editor/autosave.ts',
      // M12, section 13: the two position features. `position.ts` is the type both of them
      // speak; `navHistory.ts` is the whole of what Back and Forward decide.
      'src/editor/position.ts',
      'src/editor/navHistory.ts',
      // M14, section 14: every decision behind Ctrl+hover and Ctrl+click. Import-free for exactly
      // this reason — the two bugs this project has paid most for both hid in a rule written
      // inside an event handler, which is where nothing can compile it.
      'src/editor/codeIntelGate.ts',
      '--outDir', out,
      '--rootDir', 'src',
      // CommonJS, and this is load-bearing twice over. `languages.ts` reaches its grammars
      // through `import('./languages/rust')`, which under ESM would need a `.js` suffix that
      // the source (correctly, for Vite) does not have; downlevelled to `require` it
      // resolves. And the check itself must get the *same* module instances as the compiled
      // output, or `@lezer/highlight`'s tags would be two different sets of objects and
      // every `style()` lookup would miss.
      '--module', 'commonjs',
      '--moduleResolution', 'node10',
      '--target', 'es2022',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
      '--lib', 'es2023',
      // `@types/react-dom` is auto-included from `node_modules/@types` and does not compile
      // without the DOM lib. Matches the project tsconfig, and what `check-picker.mjs` does.
      '--skipLibCheck',
    ],
    { stdio: 'inherit' },
  )

  // `ui/package.json` says `"type": "module"`, and node inherits that all the way down into
  // `node_modules/.cache`. Without this the CommonJS emitted above is read as ESM and dies
  // on its first `exports.` — which is why `check-picker.mjs`, building into /tmp where
  // there is no package.json overhead, does not need it.
  writeFileSync(join(out, 'package.json'), '{"type":"commonjs"}\n')

  const require = createRequire(import.meta.url)
  const load = (name) => require(join(out, 'editor', name))

  const { captureLineEndings, countLines, detectLineEnding, restoreLineEndings } =
    load('lineEndings.js')
  const { exceedsBytes, utf8ByteLength } = load('byteSize.js')
  const buffers = load('openBuffers.js')
  const reveal = load('revealRequest.js')
  const {
    basename,
    languageName,
    loadLanguage,
    SCRATCH_TYPES,
    defaultScratchType,
    filterScratchTypes,
  } = load('languages.js')
  const { TOKEN_ROLES, PLAIN_TOKEN, TOKEN_VAR_BY_CLASS, cideHighlightStyle } = load('highlight.js')
  const geo = load('minimapGeometry.js')
  const send = load('sendToClaude.js')
  const revealPlan = load('revealTarget.js')
  const readout = load('statusReadout.js')
  const { StringStream } = require('@codemirror/language')
  const { Text } = require('@codemirror/state')
  const { tags } = require('@lezer/highlight')

  // ---------------------------------------------------------------------------------------
  // 1. Line endings
  // ---------------------------------------------------------------------------------------

  /*
   * What CodeMirror does to a document on the way in and on the way out, and nothing else.
   * `EditorState.create` splits on this exact pattern (`DefaultSplit` in `@codemirror/state`)
   * and `Text.toString()` rejoins with `\n`. The join is the real one, imported above, so if
   * CodeMirror ever stops normalising, this check stops passing for the right reason.
   */
  const throughCodeMirror = (text) => Text.of(text.split(/\r\n?|\n/)).toString()

  const documents = {
    'LF, trailing newline': ['a\nb\nc\n', 'LF', 4],
    'LF, no trailing newline': ['a\nb\nc', 'LF', 3],
    'CRLF, trailing newline': ['a\r\nb\r\nc\r\n', 'CRLF', 4],
    'CRLF, no trailing newline': ['a\r\nb\r\nc', 'CRLF', 3],
    // The regression. `DiffPane`'s counter sees no `\n` here and calls it a file with no
    // breaks; CodeMirror splits on `\r` all the same, so classifying this as LF rewrites
    // every line of a classic-Mac or old-Excel export on the first save.
    'bare CR, trailing': ['a\rb\rc\r', 'CR', 4],
    'bare CR, no trailing': ['a\rb\rc', 'CR', 3],
    'one bare CR': ['a\rb', 'CR', 2],
    empty: ['', 'LF', 1],
    'no break at all': ['no break here', 'LF', 1],
    'a single LF': ['\n', 'LF', 2],
    'a single CR': ['\r', 'CR', 2],
    'a single CRLF': ['\r\n', 'CRLF', 2],
    // `\n\r` is an LF break followed by a CR break, not a reversed CRLF. Two breaks, mixed.
    'LF then CR': ['a\n\rb', 'Mixed', 3],
    'mixed LF and CRLF': ['a\nb\r\nc\n', 'Mixed', 4],
    'mixed CR and CRLF': ['a\rb\r\nc', 'Mixed', 3],
    'CR inside a CRLF file': ['a\r\nb\rc\r\n', 'Mixed', 4],
  }

  for (const [what, [text, ending, lines]] of Object.entries(documents)) {
    eq(detectLineEnding(text), ending, `detectLineEnding: ${what}`)
    eq(countLines(text), lines, `countLines: ${what}`)

    /*
     * The whole claim, for every shape including `Mixed`: load it, hand it to CodeMirror,
     * hand it back, and get the same bytes.
     *
     * `Mixed` used to be exempt from this — `restoreLineEndings` returned the buffer
     * unchanged and every break that was not already `\n` was rewritten on the first save,
     * which is precisely the whole-file rewrite the module exists to prevent. It is no
     * longer exempt, and the exemption must not come back.
     */
    const captured = captureLineEndings(text)
    eq(captured.ending, ending, `captureLineEndings agrees on the classification: ${what}`)
    eq(
      restoreLineEndings(throughCodeMirror(text), captured),
      text,
      `round trip is byte-identical: ${what}`,
    )

    // A uniform document must not allocate a break per line; a mixed one must record one
    // per break, or the restore above is passing on an array that happens to be long enough.
    if (ending === 'Mixed') {
      eq(captured.breaks?.length, lines - 1, `the record has one break per line: ${what}`)
    } else {
      eq(captured.breaks, null, `a uniform document stores no per-line record: ${what}`)
    }
  }

  // A 5,000-line CRLF file is the case that motivates all of this: one character typed into
  // it must not turn into 5,000 changed lines in the git panel.
  const big = 'const x = 1;\r\n'.repeat(5000)
  eq(detectLineEnding(big), 'CRLF', 'a large CRLF file')
  eq(countLines(big), 5001, 'a large CRLF file counts its lines')
  eq(
    restoreLineEndings(throughCodeMirror(big), captureLineEndings(big)),
    big,
    'a large CRLF file round-trips',
  )

  // The same file with one stray LF in it: the mixed case as it actually occurs, and the
  // size at which "a mixed file is rewritten whole" stops being a footnote.
  {
    const strays = `first\n${'const x = 1;\r\n'.repeat(5000)}`
    const captured = captureLineEndings(strays)
    eq(captured.ending, 'Mixed', 'a large mostly-CRLF file with one LF is mixed')
    eq(captured.fill, '\r\n', 'and a new line in it gets the shape the file mostly uses')
    eq(restoreLineEndings(throughCodeMirror(strays), captured), strays, 'and it round-trips')
  }

  /*
   * Recording the sequence has to cost one pass, not one pass per break.
   *
   * The shape that catches it is a stray `\r` near the *top* of an otherwise LF file: an
   * implementation that asks `indexOf('\r', i)` again after each break scans the whole
   * remaining document every time, once there is no `\r` left to find.
   *
   * Measured on this box: 7 ms for the one-pass version, 1,336 ms for the naive one, on the
   * same input. The ceiling is 250 ms — well clear of a box thirty times slower than this
   * one, and five times below the failure. Half a million lines rather than fifty thousand because
   * `indexOf` is vectorised: at 50,000 the naive shape still finishes in 20 ms and the
   * assertion would have no teeth at all.
   */
  {
    const strayAtTop = `a\rb\n${'x\n'.repeat(500_000)}`
    const started = Date.now()
    const captured = captureLineEndings(strayAtTop)
    const elapsed = Date.now() - started
    eq(captured.ending, 'Mixed', 'one bare CR at the top of an LF file is mixed')
    eq(captured.breaks?.length, 500_002, 'every break is recorded')
    eq(captured.fill, '\n', 'and the file is overwhelmingly LF')
    ok(elapsed < 250, `recording 500,000 breaks is one pass (took ${elapsed}ms)`)
    eq(restoreLineEndings(throughCodeMirror(strayAtTop), captured), strayAtTop, 'and it round-trips')
  }

  /*
   * What happens to a mixed file that is actually *edited*, which is the only reason any of
   * this exists. Endings are reapplied by index, so an edit that keeps the line count keeps
   * every ending; one that changes it shifts the tail, and lines past the end of the record
   * get `fill`.
   */
  {
    const original = 'a\r\nb\nc\r\n'
    const captured = captureLineEndings(original)
    eq(captured.ending, 'Mixed', 'the worked example is mixed')
    eq(captured.fill, '\r\n', 'CRLF is what it mostly uses')

    const buffer = throughCodeMirror(original)
    eq(buffer, 'a\nb\nc\n', 'CodeMirror flattens it to LF')
    eq(
      restoreLineEndings('aX\nb\nc\n', captured),
      'aX\r\nb\nc\r\n',
      'a typed character rewrites one line and no others',
    )
    eq(
      restoreLineEndings('a\nb\nc\nd\n', captured),
      'a\r\nb\nc\r\nd\r\n',
      'an appended line gets the fill, and the lines above it keep their own',
    )
    eq(
      restoreLineEndings('a\nb\n', captured),
      'a\r\nb\n',
      'a deleted last line takes its break with it',
    )
  }

  // `fill` is the file's own answer, and the tiebreak has to be the file's too — otherwise
  // the ending of a newly typed line depends on the order this implementation tests three
  // shapes in.
  eq(captureLineEndings('a\r\nb\nc').fill, '\r\n', 'a tie goes to the shape that appears first')
  eq(captureLineEndings('a\nb\r\nc').fill, '\n', 'and the other way round')
  eq(captureLineEndings('a\rb\nc\nd').fill, '\n', 'otherwise the most common shape wins')

  eq(restoreLineEndings('a\nb', captureLineEndings('x\ny')), 'a\nb', 'restoring LF is the identity')
  eq(restoreLineEndings('a\nb', captureLineEndings('x\ry')), 'a\rb', 'restoring CR')
  eq(restoreLineEndings('a\nb', captureLineEndings('x\r\ny')), 'a\r\nb', 'restoring CRLF')

  // ---------------------------------------------------------------------------------------
  // 2. The language table
  // ---------------------------------------------------------------------------------------

  eq(basename('/a/b/c.rs'), 'c.rs', 'basename: posix')
  eq(basename('C:\\a\\b\\c.rs'), 'c.rs', 'basename: windows')
  eq(basename('c.rs'), 'c.rs', 'basename: bare')
  eq(basename(''), '', 'basename: empty')

  const names = {
    '/src/main.rs': 'Rust',
    '/src/App.tsx': 'TSX',
    '/src/main.ts': 'TypeScript',
    '/src/main.mts': 'TypeScript',
    '/src/vite.config.js': 'JavaScript',
    '/src/App.jsx': 'JSX',
    'setup.py': 'Python',
    'stub.pyi': 'Python',
    'main.c': 'C',
    'main.h': 'C',
    'main.cpp': 'C++',
    'main.go': 'Go',
    'Main.java': 'Java',
    'run.sh': 'Shell',
    'package.json': 'JSON',
    '.vscode/settings.jsonc': 'JSON',
    'Cargo.toml': 'TOML',
    'Cargo.lock': 'TOML',
    'pnpm-lock.yaml': 'YAML',
    '.github/workflows/ci.yml': 'YAML',
    'README.md': 'Markdown',
    // Case-folded before lookup, on both halves of the table.
    'SRC/MAIN.RS': 'Rust',
    'DOCKERFILE': 'Shell',
    Dockerfile: 'Shell',
    Makefile: 'Shell',
    '/home/u/.bashrc': 'Shell',
    '/home/u/.gitconfig': 'TOML',
    // A leading dot makes a hidden file, not an extension. `.gitignore` and `.env` would
    // reach the same answer either way — `gitignore` is in no table — so the guard is only
    // observable on a hidden file whose whole name *is* a known extension, and that is what
    // these three pin. Without them the `dot > 0` in `lookup` can be relaxed to `dot >= 0`
    // and nothing here notices.
    '.gitignore': 'Plain Text',
    '.env': 'Plain Text',
    '.rs': 'Plain Text',
    '.md': 'Plain Text',
    '/home/u/.json': 'Plain Text',
    // Two dots is a hidden file that does have an extension, and it keeps it.
    '.eslintrc.json': 'JSON',
    '.github/dependabot.yml': 'YAML',
    // The degradations. None of these may throw, and none may guess.
    'LICENSE': 'Plain Text',
    'notes.txt': 'Plain Text',
    'archive.tar.gz': 'Plain Text',
    'weird.': 'Plain Text',
    '': 'Plain Text',
    '/': 'Plain Text',
    'a.b.c.rs': 'Rust',
    'trailing.rs ': 'Plain Text',
  }
  for (const [path, expected] of Object.entries(names)) {
    eq(languageName(path), expected, `languageName(${JSON.stringify(path)})`)
  }

  /*
   * The scratch type list, against the table it must not disagree with.
   *
   * This is the assertion the whole `SCRATCH_TYPES`-lives-in-`languages.ts` decision exists to
   * make possible, and the failure it prevents is silent and specific: an extension the picker
   * offers that `lookup` does not know produces a scratch labelled *YAML* that opens with no
   * highlighting and a status bar reading `Plain Text`. Nothing about that is visible to `tsc`,
   * to a screenshot, or to a reviewer reading two files.
   *
   * Both directions are pinned: the label the picker prints is the label the status bar will
   * print, **and** a grammar actually loads. `.txt` is the one deliberate exception — it is not
   * in `BY_EXTENSION` and must not be added, because that would be a language with no grammar —
   * so it is required to resolve to `Plain Text` and to load nothing, which is exactly what
   * every unknown extension does.
   */
  ok(SCRATCH_TYPES.length >= 8, `the picker offers ${SCRATCH_TYPES.length} types`)
  for (const type of SCRATCH_TYPES) {
    ok(
      /^[a-z0-9]{1,12}$/.test(type.ext),
      `SCRATCH_TYPES ${JSON.stringify(type.ext)} is a shape cide_core::scratch::check_ext accepts`,
    )
    eq(
      languageName(`scratch.${type.ext}`),
      type.label,
      `a scratch offered as ${JSON.stringify(type.label)} opens as that language`,
    )
    const grammar = await loadLanguage(`scratch.${type.ext}`)
    if (type.label === 'Plain Text') {
      eq(grammar, null, 'Plain Text is the absence of a grammar, not a grammar')
    } else {
      ok(grammar !== null, `and ${type.label} actually loads a grammar`)
    }
  }
  eq(
    new Set(SCRATCH_TYPES.map((t) => t.ext)).size,
    SCRATCH_TYPES.length,
    'no extension is offered twice — the second row would create a file the first one names',
  )

  /*
   * Which row the picker opens on. Matched on the resolved **label**, so a `.tsx` buffer
   * preselects TypeScript rather than falling through to the first row because `tsx` is not
   * itself an offered extension.
   */
  /*
   * The picker's filter. (M15)
   *
   * `ScratchType.tsx` grew a text field, which reverses its own header's argument that there was
   * "nothing worth fuzzy-matching in `Rust`, `Go`, `JSON`". The ranking behind it lives here,
   * beside the list, for the same reason the list lives here — and, just as usefully, so that it
   * can be driven at all. A predicate inside the component would need a DOM to test and would
   * therefore not be tested.
   *
   * What is pinned is the *ordering*, not just membership. Plain substring would answer `js`
   * with whatever row happens to be highest in the list, and the whole point of two keystrokes
   * plus Enter is that the top row is the one meant.
   */
  const labels = (query) => filterScratchTypes(query).map((t) => t.label)

  eq(labels(''), SCRATCH_TYPES.map((t) => t.label),
    'an empty query is the whole list in list order — the popup opens looking exactly as it '
    + 'always has')
  eq(labels('sql')[0], 'SQL', 'the exact extension wins: `sql` lands on SQL')
  eq(labels('rs')[0], 'Rust', 'and `rs` on Rust, whose label does not contain those letters '
    + 'in that order at all')
  eq(labels('md')[0], 'Markdown', 'and `md` on Markdown')
  eq(labels('c')[0], 'C', 'an exact extension beats a prefix: `c` is C, not C++')
  ok(labels('c').includes('C++'), 'and C++ is still offered underneath it')
  eq(labels('js')[0], 'JavaScript', '`js` is JavaScript — the extension, exactly')
  eq(labels('java')[0], 'JavaScript',
    'and `java` reaches JavaScript by label prefix. Java is not an offered type, so there is '
    + 'nothing here for the extension tier to have found first')
  eq(labels('ty')[0], 'TypeScript', 'a label prefix: `ty`')
  ok(labels('script').includes('TypeScript') && labels('script').includes('JavaScript'),
    'a label substring reaches both scripts')
  eq(labels('SQL')[0], 'SQL', 'matching is case-insensitive — nobody types `sql` in capitals '
    + 'looking for `.sql`, and nobody types it in lower case looking for `SQL`')
  eq(labels('  rust  ')[0], 'Rust', 'and the query is trimmed, so a stray space does not empty '
    + 'the list')
  eq(labels('zzzz'), [], 'a query that matches nothing returns nothing — which the popup says '
    + 'out loud rather than showing an empty box')
  ok(
    labels('t').length < SCRATCH_TYPES.length,
    'a one-letter query still narrows the list rather than reordering all of it',
  )
  // The ordering has to be total: ties fall back to list order, so the top row for a query is
  // decided rather than left to a comparator that returned 0.
  eq(labels('o')[0], 'Go', '`o` is `go` by extension substring before any label match')

  eq(defaultScratchType(null), 0, 'with no file open the picker opens on the first row')
  eq(defaultScratchType('/p/src/main.rs'), 0, 'a Rust buffer opens on Rust, which is first')
  eq(
    SCRATCH_TYPES[defaultScratchType('/p/ui/src/App.tsx')]?.label,
    'TypeScript',
    'a TSX buffer opens on TypeScript — the offered row that loads the same grammar',
  )
  eq(
    SCRATCH_TYPES[defaultScratchType('/p/ui/src/app.mjs')]?.label,
    'JavaScript',
    'and a .mjs one opens on JavaScript, not on TypeScript — the label wins over the module',
  )
  eq(
    SCRATCH_TYPES[defaultScratchType('/p/Main.java')]?.label,
    'C',
    'a language nobody offers falls back to the offered row that loads the same grammar, ' +
      'not to whatever happens to be first in the list',
  )
  eq(
    SCRATCH_TYPES[defaultScratchType('/p/archive.tar.gz')]?.label,
    'Plain Text',
    'a file whose extension resolves to nothing opens on the row that also resolves to ' +
      'nothing — the same fallback, not a second rule naming "txt"',
  )
  eq(
    SCRATCH_TYPES[defaultScratchType('/p/notes.txt')]?.label,
    'Plain Text',
    'and Plain Text is reachable as a preselection, not only as a row',
  )

  eq(await loadLanguage('nope.qqq'), null, 'an unknown extension loads no grammar')
  eq(await loadLanguage(''), null, 'an empty path loads no grammar')
  eq(await loadLanguage('LICENSE'), null, 'an extensionless unknown loads no grammar')
  ok((await loadLanguage('main.rs')) !== null, 'a known extension loads a grammar')
  ok((await loadLanguage('Cargo.lock')) !== null, 'a whole-name match loads a grammar')

  // ---------------------------------------------------------------------------------------
  // 3. The highlight table
  // ---------------------------------------------------------------------------------------

  /*
   * `@codemirror/language` turns a stream parser's tag *name* into `Tag` objects with
   * `createTokenType`, which is not exported. This is that function's rule, and only that
   * rule: split on `.`, look each part up in `tags`, and treat a function-valued part as a
   * modifier applied to what came before. Getting this wrong here would make the assertions
   * below test a fiction, so it is deliberately the smallest possible restatement.
   */
  const tagsFor = (name) => {
    let found = []
    for (const part of name.split('.')) {
      const value = tags[part]
      if (value === undefined) return []
      if (typeof value === 'function') found = found.map(value)
      else found = Array.isArray(value) ? value : [value]
    }
    return found
  }
  const classOf = (name) => cideHighlightStyle.style(tagsFor(name)) ?? null

  // The mock's own six roles, plus `?`. These are the assertions a screenshot would make.
  eq(classOf('keyword'), 'cide-tk-keyword', 'keywords are the keyword class')
  eq(classOf('typeName'), 'cide-tk-type', 'types')
  eq(classOf('variableName.function'), 'cide-tk-function', 'calls')
  eq(classOf('meta'), 'cide-tk-attribute', 'attributes')
  eq(classOf('string'), 'cide-tk-string', 'strings')
  eq(classOf('comment'), 'cide-tk-comment', 'comments')
  eq(classOf('controlOperator'), 'cide-tk-control', "`?` gets its own role, not the operator's")

  // Subtag fallback is what lets the table have twelve entries instead of forty. `atom` is a
  // subtag of `keyword` and `bool` of `literal`; if lezer ever reparents them, the mock's
  // purple `None` and green `true` go plain and nothing else would say so.
  eq(classOf('atom'), 'cide-tk-keyword', 'an atom falls back to the keyword rule')
  eq(classOf('bool'), 'cide-tk-number', 'a bool falls back to the literal rule')
  eq(classOf('macroName'), 'cide-tk-function', 'a macro reads as a call')
  eq(classOf('labelName'), 'cide-tk-function', 'a YAML anchor reads as a call')
  eq(classOf('propertyName'), 'cide-tk-property', 'a key')
  eq(classOf('number'), 'cide-tk-number', 'a number')
  eq(classOf('operator'), 'cide-tk-operator', 'an operator')
  eq(classOf('bracket'), 'cide-tk-operator', 'a bracket, via punctuation')
  eq(classOf('punctuation'), 'cide-tk-operator', 'punctuation')
  eq(classOf('heading'), 'cide-tk-heading', 'a markdown heading')
  eq(classOf('strong'), 'cide-tk-heading', 'bold shares the heading role')
  eq(classOf('emphasis'), 'cide-tk-emphasis', 'italic')
  eq(classOf('quote'), 'cide-tk-emphasis', 'a block quote')
  eq(classOf('link'), 'cide-tk-emphasis', 'a link')
  eq(classOf('monospace'), 'cide-tk-emphasis', 'inline code')
  // Deliberate: an ordinary identifier is body text, and colouring every one of them is how
  // a syntax theme turns into noise.
  eq(classOf('variableName'), null, 'a plain identifier has no role')

  /*
   * The buffer paints from `EditorSurface.module.css`; the canvas minimap paints from
   * `TOKEN_VAR_BY_CLASS`. `highlight.ts` exists so those two cannot disagree, and this is
   * the assertion that makes that true rather than merely intended — the failure it catches
   * is a keyword being purple in the text and blue in the map, which nothing else here can
   * see.
   */
  const surfaceCss = readFileSync('src/editor/EditorSurface.module.css', 'utf8')
  const cssVarByClass = new Map()
  for (const [, selector, body] of surfaceCss.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const colour = /color:\s*var\((--[a-z-]+)\)/.exec(body)
    if (colour === null) continue
    for (const [, cls] of selector.matchAll(/\.(cide-tk-[a-z]+)\b/g)) cssVarByClass.set(cls, colour[1])
  }

  for (const role of TOKEN_ROLES) {
    // Deliberately *not* `TOKEN_VAR_BY_CLASS.get(cls) === role.token`: that map is built by
    // mapping over `TOKEN_ROLES` two lines below the table, so such an assertion restates
    // the constructor and passes whatever the table says. The comparison worth making is the
    // one across the two paint paths — the canvas's map against the stylesheet the buffer
    // resolves — with the table itself checked against the stylesheet separately.
    eq(TOKEN_VAR_BY_CLASS.get(role.cls), cssVarByClass.get(role.cls), `${role.cls}: the canvas and the buffer agree`)
    eq(cssVarByClass.get(role.cls), role.token, `${role.cls} is styled to match in the buffer`)
  }
  eq(cssVarByClass.size, TOKEN_ROLES.length, 'the stylesheet has no token class the table lacks')

  // Every custom property either table names has to exist in *both* themes, or the canvas
  // silently falls back to `UNRESOLVED` grey while the buffer inherits whatever it inherits.
  const tokensCss = readFileSync('src/styles/tokens.css', 'utf8')
  /*
   * Which palette block belongs to which theme.
   *
   * The question asked here is deliberately *not* "does this property resolve in the light
   * theme" — by CSS's own rules it always does, because the palette's selector is
   * `:root, [data-theme='dark']` and `:root` matches the <html> element whatever its
   * `data-theme` says. That is exactly what made the previous version of this check unable
   * to fail: deleting `--purple` from the `[data-theme='light']` block left it green, while
   * a light-theme buffer painted keywords with the dark palette's `#b48ead` and a light
   * `--text` of `#d7d7dd` would have been invisible on `#f5f3f0`.
   *
   * So the question is the useful one instead: does each theme give the property *its own*
   * value rather than inheriting the other theme's? A block naming any theme belongs to the
   * theme it names; a block naming none is theme-independent — the bare `:root` that holds
   * the fonts and the fixed sizes — and counts for both. Per block and not per selector
   * part, or the `:root` in front of the comma pulls the dark palette back into the light
   * theme and we are where we started.
   *
   * The quoting is matched literally on purpose: rewritten as `[data-theme="light"]` this
   * stops matching and the light assertions fail loudly, rather than quietly reverting to
   * "whatever `:root` said".
   */
  const ownedBy = (selector, theme) =>
    selector.includes('[data-theme=')
      ? selector.includes(`[data-theme='${theme}']`)
      : selector.includes(':root')
  const declaredIn = (theme) => {
    const declared = new Set()
    for (const [, selector, body] of tokensCss.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
      if (!ownedBy(selector, theme)) continue
      for (const [, name] of body.matchAll(/(--[a-z0-9-]+)\s*:/g)) declared.add(name)
    }
    return declared
  }
  const dark = declaredIn('dark')
  const light = declaredIn('light')
  // `ownedBy` is the whole reason the loop below can fail, so it is checked itself: hand one
  // theme's block to the other and every assertion under it goes green regardless.
  ok(!ownedBy(":root,\n[data-theme='dark']", 'light'), 'the dark palette is not read as light')
  ok(ownedBy(":root,\n[data-theme='dark']", 'dark'), 'the dark palette is read as dark')
  ok(ownedBy(':root', 'light') && ownedBy(':root', 'dark'), 'a theme-free :root block feeds both')
  // `--sel`, `--border` and `--accent` are read by `minimap.ts`'s palette, not by a role.
  const needed = [...TOKEN_ROLES.map((r) => r.token), PLAIN_TOKEN, '--accent', '--sel', '--border']
  for (const token of new Set(needed)) {
    ok(dark.has(token), `the dark palette gives ${token} its own value`)
    ok(light.has(token), `the light palette gives ${token} its own value`)
  }

  // The stylesheet reserves the minimap's gutter with `--w-minimap`; the canvas is sized by
  // `MINIMAP_WIDTH`. `minimap.ts` says in a comment that they are the same number.
  const width = /--w-minimap:\s*(\d+)px/.exec(tokensCss)
  eq(Number(width?.[1]), geo.MINIMAP_WIDTH, '--w-minimap and MINIMAP_WIDTH agree')

  // ---------------------------------------------------------------------------------------
  // 4. The minimap's arithmetic
  // ---------------------------------------------------------------------------------------

  eq(geo.mapRows(0), 1, 'a collapsed pane still has one row rather than zero')
  eq(geo.mapRows(2), 1, 'less than one pitch is one row')
  eq(geo.mapRows(300), 100, '300px of column is 100 rows')
  eq(geo.mapRows(301), 100, 'a partial row at the bottom is not a row')

  eq(geo.firstMapLine(50, 100, 0, 0), 1, 'a document that fits starts at the top')
  eq(geo.firstMapLine(100, 100, 400, 900), 1, 'a document that exactly fits never slides')
  /*
   * The case that makes the `totalLines <= rows` guard load-bearing rather than tidy: a 50
   * line file in a 300px pane fits the map's 100 rows *and* scrolls the buffer, because a
   * text line is ~19px and a map row is 3. Without the guard the fraction multiplies a
   * negative `totalLines - rows` and the answer is line -24, which `doc.line()` throws on.
   */
  eq(geo.firstMapLine(50, 100, 325, 650), 1, 'a fitting document does not slide while scrolling')
  eq(geo.firstMapLine(50, 100, 650, 650), 1, 'nor at the bottom of that scroll')
  for (const total of [1, 2, 50, 99, 100, 101, 5000]) {
    for (const top of [0, 1, 325, 650, 10_000]) {
      const first = geo.firstMapLine(total, 100, top, 650)
      ok(first >= 1, `the window never starts before line 1 (${total} lines, scrolled ${top})`)
      ok(first <= total, `nor past the end (${total} lines, scrolled ${top})`)
    }
  }
  eq(geo.firstMapLine(1000, 100, 0, 900), 1, 'unscrolled, the window is the head of the file')
  eq(geo.firstMapLine(1000, 100, 450, 900), 451, 'half scrolled, the window is half way down')
  eq(geo.firstMapLine(1000, 100, 900, 900), 901, 'fully scrolled, the last row is the last line')
  eq(geo.lastMapLine(1000, 100, 901), 1000, 'and the window ends exactly at the end of the file')
  eq(geo.firstMapLine(1000, 100, 5000, 900), 901, 'past the end clamps rather than running off')
  eq(geo.firstMapLine(1000, 100, -40, 900), 1, 'rubber-banding above the top clamps too')
  eq(geo.firstMapLine(1000, 100, 100, 0), 1, 'a scroller with no range has no fraction')
  eq(geo.lastMapLine(30, 100, 1), 30, 'the window stops at the end of a short document')

  {
    // 96px column, 5px of padding each side, 110 columns to the full width.
    const full = geo.barRect(0, 110, 0)
    eq(full.x, 5, 'a bar starts after the left padding')
    eq(full.width, 86, 'a 110-column line fills the drawable width')
    eq(full.height, 2, 'the mock says 2px bars in a 3px pitch')
    eq(geo.barRect(0, 10, 7).y, 21, 'row 7 sits seven pitches down')
    eq(geo.barRect(0, 220, 0).width, 86, 'a line past the scale clips rather than overflowing')
    ok(geo.barRect(0, 1, 0).width >= 1, 'a one-character line is still a visible bar')
    // A deeply indented line must not push its bar out of the column.
    for (const indent of [0, 1, 8, 40, 109, 110, 400, 5000]) {
      for (const length of [1, 5, 80, 512]) {
        const bar = geo.barRect(indent, length, 0)
        ok(bar.x >= 5, `bar x stays inside the padding (indent ${indent}, length ${length})`)
        ok(
          bar.x + bar.width <= geo.MINIMAP_WIDTH,
          `bar stays inside the column (indent ${indent}, length ${length})`,
        )
      }
    }
  }

  eq(geo.viewportRect(1, 100, 10, 20), { top: 27, height: 33 }, 'the visible range, 11 rows of it')
  eq(geo.viewportRect(1, 100, 5, 5), { top: 12, height: 3 }, 'a one-line viewport is one row tall')
  eq(geo.viewportRect(50, 149, 1, 60), { top: 0, height: 33 }, 'clipped at the top of the window')
  eq(
    geo.viewportRect(50, 149, 140, 500),
    { top: 270, height: 30 },
    'clipped at the bottom of the window',
  )
  eq(geo.viewportRect(50, 149, 1, 20), null, 'a viewport entirely above the window draws nothing')
  eq(geo.viewportRect(50, 149, 200, 300), null, 'a viewport entirely below it draws nothing')
  {
    // Whatever the two scroll readings say, the rectangle stays on the canvas.
    const rows = 100
    for (const from of [1, 40, 99, 100, 140, 260]) {
      const rect = geo.viewportRect(50, 149, from, from + 30)
      if (rect === null) continue
      ok(rect.top >= 0, `viewport top is on the canvas (from ${from})`)
      ok(rect.height > 0, `viewport height is positive (from ${from})`)
      ok(rect.top + rect.height <= rows * geo.ROW_PITCH, `viewport fits the column (from ${from})`)
    }
  }

  eq(geo.lineMetrics('', 4), { indent: 0, length: 0 }, 'an empty line draws nothing')
  eq(geo.lineMetrics('    ', 4), { indent: 0, length: 0 }, 'a line of spaces draws nothing')
  eq(geo.lineMetrics('\t\t', 4), { indent: 0, length: 0 }, 'a line of tabs draws nothing')
  eq(geo.lineMetrics('abc', 4), { indent: 0, length: 3 }, 'an unindented line')
  eq(geo.lineMetrics('  abc  ', 4), { indent: 2, length: 3 }, 'trailing space is not content')
  eq(geo.lineMetrics('\tabc', 4), { indent: 4, length: 3 }, 'a tab is worth its visual width')
  eq(geo.lineMetrics('\t\tabc', 4), { indent: 8, length: 3 }, 'two tabs')
  eq(geo.lineMetrics('  \tabc', 4), { indent: 6, length: 3 }, 'spaces then a tab')
  eq(geo.lineMetrics('\tabc', 8), { indent: 8, length: 3 }, 'the tab width comes from the state')
  eq(
    geo.lineMetrics('x'.repeat(4000), 4),
    { indent: 0, length: geo.LINE_SCAN_LIMIT },
    'a minified line is measured to the scan limit and no further',
  )
  eq(
    geo.lineMetrics('  ' + 'x'.repeat(4000), 4),
    { indent: 2, length: geo.LINE_SCAN_LIMIT - 2 },
    'and its indentation still counts',
  )

  eq(geo.lineAtOffset(0, 1, 100), 1, 'the top pixel is the first mapped line')
  eq(geo.lineAtOffset(2, 1, 100), 1, 'anywhere in a row is that row')
  eq(geo.lineAtOffset(3, 1, 100), 2, 'one pitch down is the next line')
  eq(geo.lineAtOffset(30, 901, 1000), 911, 'the offset is relative to the mapped window')
  eq(geo.lineAtOffset(9000, 1, 100), 100, 'below the last row is the last line, not past it')
  eq(geo.lineAtOffset(-9, 1, 100), 1, 'above the first row is the first line')
  eq(geo.lineAtOffset(0, 901, 1000), 901, 'a click at the top of a slid window')

  // ---------------------------------------------------------------------------------------
  // 5. The stream grammars
  // ---------------------------------------------------------------------------------------

  const grammars = {
    rust: require(join(out, 'editor/languages/rust.js')).spec,
    go: require(join(out, 'editor/languages/go.js')).spec,
    typescript: require(join(out, 'editor/languages/typescript.js')).spec,
    python: require(join(out, 'editor/languages/python.js')).spec,
    clike: require(join(out, 'editor/languages/clike.js')).spec,
    shell: require(join(out, 'editor/languages/shell.js')).spec,
    markdown: require(join(out, 'editor/languages/markdown.js')).spec,
    sql: require(join(out, 'editor/languages/sql.js')).spec,
    json: require(join(out, 'editor/languages/data.js')).json,
    toml: require(join(out, 'editor/languages/data.js')).toml,
    yaml: require(join(out, 'editor/languages/data.js')).yaml,
  }

  /**
   * Tokenize one document the way `StreamLanguage` does, and refuse to let it stall.
   *
   * CodeMirror gives a parser ten tries to move the stream and then throws "Stream parser
   * failed to advance stream", which takes the buffer down — the file opens and never
   * colours, or the editor throws on a keystroke. The assertion here is stricter than
   * CodeMirror's: *every* call must advance. Nothing in `streamGrammar.ts` has a reason to
   * return without consuming, and the strict form names the offending character instead of
   * a stall ten calls later.
   */
  const tokenize = (spec, source, where) => {
    const state = spec.startState()
    const emitted = []
    for (const line of source.split('\n')) {
      if (line.length === 0) continue
      const stream = new StringStream(line, 4, 2)
      let calls = 0
      while (!stream.eol()) {
        stream.start = stream.pos
        const tag = spec.token(stream, state)
        if (stream.pos <= stream.start) {
          failed += 1
          console.error(
            `FAIL ${where}: token() returned ${JSON.stringify(tag)} without advancing at ` +
              `${JSON.stringify(line.slice(stream.pos, stream.pos + 24))}`,
          )
          return emitted
        }
        if (tag !== null && tag !== undefined) emitted.push({ tag, text: stream.current() })
        if (++calls > line.length + 8) {
          failed += 1
          console.error(`FAIL ${where}: more tokens than characters on ${JSON.stringify(line)}`)
          return emitted
        }
      }
    }
    return emitted
  }

  const sources = {
    /*
     * Go, added in M12 when `.go` stopped pointing at the `clike` table. Every line is a shape
     * that table got wrong or a shape the hook exists for.
     */
    go: [
      'package main',
      'import "fmt"',
      '// A raw string spanning lines, with a backslash that is not an escape:',
      'var path = `C:\\new\\table`',
      'var query = `SELECT *',
      'FROM t`',
      "var r = 'x'",
      "var esc = '\\n'",
      "var quote = '\\''",
      'type Server struct{ addr string `json:"addr"` }',
      'var ch chan int = make(chan int, 1)',
      'func (s *Server) Serve(ctx context.Context) error {',
      '\tdefer close(ch)',
      '\tgo func() { ch <- 1 }()',
      '\tselect {',
      '\tcase v := <-ch:',
      '\t\tfmt.Println(v, len(path), cap(ch))',
      '\tdefault:',
      '\t}',
      '\tfor i := range xs {',
      '\t\tif i > 0 { continue }',
      '\t}',
      '\treturn nil',
      '}',
      '/* a block comment /* that does not nest in Go */',
      'var after = 1',
    ].join('\n'),
    rust: [
      '#[derive(Debug, Clone)] // a note after the attribute',
      'pub fn main(argv: &[String]) -> Result<(), Error> {',
      "    let s = r#\"a raw \"string\" with #\"# ;",
      "    let c = 'x'; let l: &'static str = \"hi\\n\";",
      '    /* nested /* comment */ still a comment */',
      '    println!("{}", 1_000u64 + 0xffu8 as u64);',
      '    let v = compute()?;',
      '    macro_rules! m { () => {} }',
      '}',
    ].join('\n'),
    typescript: [
      '@Component({ selector: "app" })',
      'export class A<T extends Map<string, number>> implements B {',
      '  readonly x = `template ${a + b} literal`;',
      '  private y = /re?ge[x]/g;',
      '  async run(): Promise<void> { await this.z?.(); }',
      '}',
      'const n = 1_000.5e-3, m = 0b1010;',
    ].join('\n'),
    python: [
      '@decorator.with_args(1)',
      'class C(Base):',
      '    """A docstring',
      '    spanning lines."""',
      '    def f(self, *args, **kw) -> int:',
      "        s = f'{x!r} and ' + rb'\\x00'",
      '        return len([i for i in range(10) if i % 2])',
    ].join('\n'),
    clike: [
      '#include <stdio.h>',
      '#define MAX(a, b) ((a) > (b) ? (a) : (b))',
      'static const char *s = "a \\"quoted\\" thing";',
      'int main(void) { /* comment */ return 0; }',
      'func (r *Repo) Get(ctx context.Context) (int, error) { return 0, nil }',
    ].join('\n'),
    shell: [
      '#!/usr/bin/env bash',
      'set -euo pipefail',
      'readonly DIR="${1:-$PWD}"',
      'for f in "$DIR"/*.txt; do',
      '  printf \'%s\\n\' "${f##*/}" | tr -d \'\\r\'',
      'done',
      'echo $? $$ $@ $1',
    ].join('\n'),
    markdown: [
      '# A heading',
      'Setext',
      '======',
      '',
      'Some **bold** and _italic_ and `code` and [a link](https://x.y) and <https://z>.',
      '',
      '> a quote',
      '- a list item',
      '| a | table |',
      '',
      '```rust',
      'fn not_highlighted() {}',
      '```',
      '',
      'Trailing https://bare.url prose.',
    ].join('\n'),
    json: [
      '{',
      '  "name": "cide", // a jsonc comment',
      '  "nested": { "a": [1, 2.5, true, null] },',
      '  "escaped\\"key": "value"',
      '}',
    ].join('\n'),
    toml: [
      '# a comment',
      '[package]',
      'name = "cide"',
      'edition = "2024"',
      'multi = """',
      'a triple quoted value',
      '"""',
      '[[bin]]',
      'path = "src/main.rs"',
      '"quoted.key" = 1',
      'dotted.key = 2',
    ].join('\n'),
    /*
     * SQL, added in M15 with the grammar. Every line is a shape a C-like table gets wrong: the
     * shouted keywords the case-folding flag exists for, the doubled quote that is an escape
     * here and nowhere else, `--` as a line comment (which is a decrement operator in every
     * other language in this map), and a capitalised table name that must NOT come out purple.
     */
    sql: [
      '-- Recent sessions, per project.',
      'CREATE TABLE IF NOT EXISTS Sessions (',
      '  id       UUID PRIMARY KEY,',
      '  project  TEXT NOT NULL REFERENCES Projects(id) ON DELETE CASCADE,',
      "  title    VARCHAR(200) DEFAULT 'untitled',",
      '  started  TIMESTAMPTZ NOT NULL DEFAULT current_timestamp,',
      '  turns    INTEGER      NOT NULL DEFAULT 0',
      ');',
      'select p.name, count(s.id) as n',
      '  from Projects p left join Sessions s on s.project = p.id',
      " where p.name like 'cide%' and s.title <> 'it''s a test'",
      ' group by p.name having count(s.id) > 0',
      ' order by n desc nulls last',
      ' limit 10 offset 0;',
      '/* A block comment with a /* that does not nest */',
      'INSERT INTO Sessions (id, project) VALUES (:id, :project) ON CONFLICT DO NOTHING;',
      'UPDATE "Sessions" SET turns = turns + 1 WHERE id = $1 RETURNING turns;',
      'SELECT 1e3, 0x1f, 3.14, -7 FROM t;',
      'DROP TABLE Sessions;',
    ].join('\n'),
    yaml: [
      '---',
      'name: ci',
      'on:',
      '  push:',
      '    branches: [main]',
      'jobs:',
      '  build:',
      '    steps:',
      '      - uses: actions/checkout@v5',
      '        with: &anchor { url: "https://example.com/a:b" }',
      '      - run: *anchor',
      '...',
    ].join('\n'),
  }

  // Every grammar over every corpus. A shell hook has no business being handed Rust, which
  // is exactly why it is: the paths a language never takes on its own input are the ones
  // where a non-advancing branch hides.
  for (const [lang, spec] of Object.entries(grammars)) {
    for (const [corpus, source] of Object.entries(sources)) {
      const emitted = tokenize(spec, source, `${lang} over ${corpus}`)
      ok(emitted.length > 0, `${lang} over ${corpus} produced tokens`)
    }
    // And over the shapes that are nobody's valid input.
    const adversarial = [
      '',
      ' ',
      '\t\t',
      '"unterminated',
      "'unterminated",
      '`unterminated',
      '/* unterminated',
      'r#"unterminated',
      '"""',
      '${',
      '#[',
      '[',
      ']',
      '\\',
      '$',
      '&',
      '*',
      '_',
      '#',
      '<',
      '>',
      '|',
      '!',
      '-',
      ':',
      '...',
      '---',
      '```',
      '0x',
      '1e',
      '1.',
      'é漢😀',
      'a\u00a0b',
      '   \t  indented: value',
      '\u0000\u001f',
    ].join('\n')
    tokenize(spec, adversarial, `${lang} over adversarial input`)
  }

  // Every tag name the grammars actually emit has to resolve to a class, or the token is
  // painted as body text and no one finds out without opening the file and looking.
  const emittedTags = new Set()
  for (const [lang, spec] of Object.entries(grammars)) {
    for (const source of Object.values(sources)) {
      for (const { tag } of tokenize(spec, source, `${lang} tag sweep`)) emittedTags.add(tag)
    }
  }
  // The one role that is deliberately body text. Listed rather than skipped by a `null`
  // check, so that a *new* tag arriving without a colour is a failure and not a shrug.
  const PLAIN_BY_DESIGN = new Set(['variableName'])
  for (const tag of [...emittedTags].sort()) {
    ok(tagsFor(tag).length > 0, `the tag name ${JSON.stringify(tag)} is one lezer knows`)
    if (PLAIN_BY_DESIGN.has(tag)) {
      eq(classOf(tag), null, `the tag ${JSON.stringify(tag)} is body text on purpose`)
    } else {
      ok(classOf(tag) !== null, `the tag ${JSON.stringify(tag)} has a colour in TOKEN_ROLES`)
    }
  }
  // The corpora above are meant to reach every interesting role; if one stops doing so the
  // sweep silently tests less than it says it does.
  for (const expected of ['keyword', 'comment', 'string', 'meta', 'propertyName', 'heading']) {
    ok(emittedTags.has(expected), `the corpus still exercises the ${expected} role`)
  }

  /*
   * SQL, as tokens. (M15)
   *
   * "Does the stream advance" is the sweep above, and it would pass for a grammar that painted
   * a whole query as `variableName`. The scratch picker now offers SQL, and the standing rule
   * for that list is that a type it offers is a type the editor can actually highlight — so
   * what is asserted here is that the four ways SQL is not C are all handled, because each of
   * them is a way for the highlighting to be *present and wrong*.
   */
  {
    const byText = new Map()
    for (const { tag, text } of tokenize(grammars.sql, sources.sql, 'sql roles')) {
      if (!byText.has(text)) byText.set(text, tag)
    }
    eq(byText.get('SELECT'), 'keyword',
      'sql: a shouted keyword is a keyword. The tables are written once, in lowercase, so '
      + 'without `caseInsensitiveKeywords` the entire conventional SQL style highlights as '
      + 'nothing at all')
    eq(byText.get('select'), 'keyword', 'sql: and so is the same word in lower case')
    eq(byText.get('CREATE'), 'keyword', 'sql: `CREATE`')
    eq(byText.get('TIMESTAMPTZ'), 'typeName', 'sql: a shouted type is a type')
    eq(byText.get('current_timestamp'), 'atom', 'sql: `current_timestamp` is a value')
    eq(byText.get('NULL'), 'atom', 'sql: `NULL` is a value, not a keyword')
    eq(byText.get('count'), 'variableName.function', 'sql: an aggregate is drawn as a call')
    eq(byText.get('Sessions'), 'variableName',
      'sql: a capitalised table name is NOT a type. `capitalisedIsType` is true for C-like and '
      + 'false here, and with it on every table in a shouted query comes out purple')
    eq(byText.get('-- Recent sessions, per project.'), 'comment',
      'sql: `--` starts a line comment. In every other language in this map it is a decrement '
      + 'operator, so this is the one that a shared table would silently get wrong')
    eq(byText.get("'untitled'"), 'string', 'sql: a single-quoted literal is a string')
    eq(byText.get('"Sessions"'), 'string',
      'sql: a quoted identifier is painted as a string — the knowing inaccuracy recorded in '
      + '`languages/sql.ts`, because telling one from a literal needs the dialect')
    /*
     * `escapes: false` — SQL escapes a quote by doubling it, not with a backslash.
     *
     * Two cases, and the **second** is the one that matters. The first version of this block
     * asserted only the first and a mutation flipping `escapes` to `true` survived it, because
     * a doubled quote tokenizes identically either way — there is no backslash in `'it''s'` for
     * the escape rule to act on. That is the "pins today's behaviour rather than the property"
     * trap, caught by mutation rather than by reading.
     *
     * The property `escapes: false` actually buys is what happens to a **backslash before the
     * closing quote**. A Windows path in a literal — `'C:\temp\'` — ends there in SQL and does
     * not end there in C. With `escapes: true` the closing quote is eaten as an escape, the
     * string runs off the end of the line, and every keyword after it loses its colour for the
     * rest of the statement.
     */
    const doubled = tokenize(grammars.sql, "select 'it''s a test' as t;\n", 'sql doubled quote')
    eq(
      doubled.filter((t) => t.tag === 'string').map((t) => t.text).join(''),
      "'it''s a test'",
      'sql: every character of a literal containing a doubled quote is painted as a string — a '
        + 'doubled quote does not end it',
    )
    const backslash = tokenize(
      grammars.sql,
      "select 'C:\\temp\\' as p from t;\n",
      'sql backslash in a literal',
    )
    eq(
      backslash.find((t) => t.text === 'as')?.tag,
      'keyword',
      'sql: a backslash before the closing quote does NOT escape it. `escapes: true` would eat '
        + 'the quote, run the string to the end of the line, and leave every keyword after it '
        + 'uncoloured — which is what a Windows path in a literal looks like',
    )
    eq(
      backslash.find((t) => t.text === 'from')?.tag,
      'keyword',
      'sql: and the rest of the statement with it',
    )
    eq(backslash.at(-1)?.text, ';', 'sql: right through to the terminator')
    // `--` again, this time proving the *operator* path is not what claims it.
    const decrement = tokenize(grammars.sql, 'select a --b\nfrom t;\n', 'sql line comment')
    ok(
      decrement.some((t) => t.tag === 'comment' && t.text === '--b'),
      'sql: `--b` is a comment to end of line, not an operator followed by an identifier',
    )
  }

  // What the mock actually specifies for Rust, asserted as tokens rather than as a colour.
  {
    const byText = new Map()
    for (const { tag, text } of tokenize(grammars.rust, sources.rust, 'rust roles')) {
      if (!byText.has(text)) byText.set(text, tag)
    }
    eq(byText.get('pub'), 'keyword', 'rust: `pub` is a keyword')
    eq(byText.get('#[derive(Debug, Clone)]'), 'meta', 'rust: an attribute stops at its bracket')
    eq(byText.get('// a note after the attribute'), 'comment', 'rust: and the comment after it')
    eq(byText.get('main'), 'variableName.function', 'rust: a call')
    eq(byText.get('println!'), 'macroName', 'rust: a macro')
    eq(byText.get('?'), 'controlOperator', 'rust: `?` is its own operator')
    eq(byText.get("'static"), 'typeName', 'rust: a lifetime is not a character literal')
    eq(byText.get('Result'), 'typeName', 'rust: a capitalised name is a type')
    eq(byText.get('1_000u64'), 'number', 'rust: a separated literal with a suffix')
  }

  /*
   * Go, and specifically the things `clike` got wrong before this grammar existed. Every
   * assertion below failed against the Java table `.go` used to be pointed at.
   */
  {
    const byText = new Map()
    const all = tokenize(grammars.go, sources.go, 'go roles')
    for (const { tag, text } of all) {
      if (!byText.has(text)) byText.set(text, tag)
    }
    for (const word of ['func', 'defer', 'go', 'chan', 'select', 'range', 'package', 'type']) {
      eq(byText.get(word), 'keyword', `go: \`${word}\` is a keyword`)
    }
    eq(byText.get('nil'), 'atom', 'go: `nil` is an atom, not an identifier')
    eq(byText.get('error'), 'typeName', 'go: `error` is a predeclared type')
    eq(byText.get('string'), 'typeName', 'go: and so is `string`')
    eq(byText.get('len'), 'variableName.function', 'go: a builtin reads as a call')
    eq(byText.get('Server'), 'typeName', 'go: a capitalised name is a type')

    /*
     * The raw string, which is the whole reason this grammar has a hook. A backtick-quoted
     * literal contains backslashes that are **not** escapes; a grammar that treated them as such
     * would end the literal early and paint the rest of the line as code.
     */
    const strings = all.filter((t) => t.tag === 'string').map((t) => t.text)
    ok(
      strings.some((text) => text.includes('C:') && text.includes('\\')),
      'go: a raw string keeps its backslashes rather than reading them as escapes',
    )
    ok(
      strings.some((text) => text.includes('SELECT')),
      'go: a raw string may span lines',
    )
    ok(strings.some((text) => text === "'x'"), 'go: a rune literal is one token')
    ok(
      strings.some((text) => text === "'\\''"),
      'go: and an escaped quote inside one does not cut it short',
    )

    // Go's block comments do **not** nest: the first close marker ends one, whatever is inside.
    // Rust's do, and copying that setting here would swallow everything after a commented-out
    // region containing a comment, to the end of the file.
    const afterComment = tokenize(
      grammars.go,
      '/* a /* b */\nvar after = 1\n',
      'go non-nesting comments',
    )
    eq(
      afterComment.find((t) => t.text === 'var')?.tag,
      'keyword',
      'go: code after a comment containing `/*` is still code',
    )
  }
  {
    const tagOf = (spec, source, text) => {
      for (const token of tokenize(spec, source, 'role spot check')) {
        if (token.text === text) return token.tag
      }
      return null
    }
    eq(tagOf(grammars.yaml, 'name: ci', 'name'), 'propertyName', 'yaml: a block-mapping key')
    eq(tagOf(grammars.yaml, 'x: &a 1', '&a'), 'labelName', 'yaml: an anchor')
    eq(tagOf(grammars.yaml, 'url: https://a/b:c', 'url'), 'propertyName', 'yaml: only the head key')
    eq(tagOf(grammars.toml, '[[bin]]', '[[bin]]'), 'meta', 'toml: an array-of-tables header')
    eq(tagOf(grammars.toml, 'edition = "2024"', 'edition'), 'propertyName', 'toml: a bare key')
    eq(tagOf(grammars.json, '{"a": 1}', '"a"'), 'propertyName', 'json: a quoted key')
    eq(tagOf(grammars.json, '["a", 1]', '"a"'), 'string', 'json: the same text as a value')
    eq(tagOf(grammars.markdown, '# H', '# H'), 'heading', 'markdown: a heading')
    eq(tagOf(grammars.markdown, '[a](b)', '[a](b)'), 'link', 'markdown: a link')
    eq(tagOf(grammars.shell, '#!/bin/sh', '#!/bin/sh'), 'meta', 'shell: a shebang')
    eq(tagOf(grammars.shell, 'echo ${A}', '${A}'), 'propertyName', 'shell: a brace expansion')
    eq(tagOf(grammars.clike, '#include <a.h>', '#include <a.h>'), 'meta', 'clike: a directive')
    eq(tagOf(grammars.python, '@dec', '@dec'), 'meta', 'python: a decorator')
    eq(tagOf(grammars.typescript, '@Dec class A {}', '@Dec'), 'meta', 'typescript: a decorator')

    /*
     * `atLineStart` exists because `stream.sol()` is false by the time a hook sees an
     * indented key — the generic path eats the leading whitespace and returns first. Every
     * key in the corpora above happens to sit in column 0, so replacing the whole function
     * with `stream.pos === 0` passed the rest of this file. These are the indented cases.
     */
    eq(tagOf(grammars.yaml, '  push: 1', 'push'), 'propertyName', 'yaml: an indented key')
    eq(tagOf(grammars.toml, '  key = 1', 'key'), 'propertyName', 'toml: an indented bare key')
    eq(tagOf(grammars.markdown, '  ## H', '## H'), 'heading', 'markdown: an indented heading')

    // The two lookup tables, checked where the fallbacks cannot answer for them: `u64` is
    // lowercase so `capitalisedIsType` cannot make it a type, and `len` has no `(` after it
    // so `callSyntax` cannot make it a call.
    eq(tagOf(grammars.rust, 'let n: u64 = 1;', 'u64'), 'typeName', 'rust: a type from the table')
    eq(tagOf(grammars.python, 'f = len', 'len'), 'variableName.function', 'python: a builtin')

    // `bracket` and `punctuation` share the `--dim` role, so nothing about a colour says
    // which one a `(` got; lezer's `t.bracket` is a different tag all the same.
    eq(tagOf(grammars.rust, 'f(x)', '('), 'bracket', 'rust: a paren is a bracket')
    eq(
      tagOf(grammars.typescript, 'const n = 1_000.5e-3;', '1_000.5e-3'),
      'number',
      'a numeric literal keeps its separators, fraction and exponent in one token',
    )
  }
  {
    /*
     * Three shapes where a state that fails to close swallows the rest of the file. All
     * three are what their modules' comments say they are for, and all three survived being
     * broken until these assertions existed: what the corpora asserted was that *some* token
     * came out, and a document tokenized entirely as one string satisfies that.
     */
    const apostrophe = tokenize(grammars.python, "it's fine\nx = 1\n", 'python apostrophe')
    ok(
      apostrophe.some((t) => t.tag === 'number' && t.text === '1'),
      "python: an apostrophe in prose does not open a string that eats the next line",
    )

    const nested = tokenize(grammars.rust, '/* a /* b */ still */ let x = 1;\n', 'rust nesting')
    eq(
      nested.filter((t) => t.tag === 'comment').map((t) => t.text),
      ['/* a /* b */ still */'],
      'rust: a nested block comment closes on its outer terminator, not its inner one',
    )
    ok(
      nested.some((t) => t.tag === 'keyword' && t.text === 'let'),
      'rust: and the code after it is code again',
    )

    /*
     * A fence is a toggle, and the closing one has to turn it off. Written as a set rather
     * than a toggle, every line after the first code block in a README is painted as code —
     * and the corpus above could not see it, because it ends inside the same document and
     * `monospace` is emitted by the inline-code span either way.
     */
    const fenced = tokenize(grammars.markdown, '```rust\nfn x() {}\n```\n# after\n', 'md fences')
    ok(
      fenced.some((t) => t.tag === 'monospace' && t.text === 'fn x() {}'),
      'markdown: a fenced line is code rather than prose',
    )
    ok(
      fenced.some((t) => t.tag === 'heading' && t.text === '# after'),
      'markdown: and the closing fence ends the block instead of reopening it',
    )

    // The hash count decides the terminator: `"` inside `r#"…"#` is content, not the end.
    const raw = tokenize(grammars.rust, 'let s = r#"a "quoted" thing"#;\n', 'rust raw strings')
    eq(
      raw
        .filter((t) => t.tag === 'string')
        .map((t) => t.text)
        .join(''),
      'r#"a "quoted" thing"#',
      'rust: a raw string closes on `"#` and not on the quote inside it',
    )
  }
  {
    /*
     * The bug `streamGrammar.ts` says cost an afternoon, asserted so it cannot come back.
     * `StringStream.match` answers `true` on a hit and a *falsy* value on a miss, so testing
     * it with `!== false` reads every ordinary `"…"` in a TOML or Python file as the opening
     * of a triple-quoted string — and the string then runs to the end of the document. The
     * symptom on this repo's own `Cargo.toml` was 5,771 string tokens and two keys, which is
     * exactly the shape of the two assertions below.
     */
    const emitted = tokenize(grammars.toml, 'name = "cide"\nedition = "2024"\n', 'toml quoting')
    eq(
      emitted.filter((t) => t.tag === 'string').map((t) => t.text),
      ['"cide"', '"2024"'],
      'toml: an ordinary quoted value is one token and closes on its own line',
    )
    eq(
      emitted.filter((t) => t.tag === 'propertyName').map((t) => t.text),
      ['name', 'edition'],
      'toml: and every key after the first one survives',
    )
    // The other direction, so the fix cannot be "stop supporting triple quotes": a real one
    // still spans lines and still ends, leaving the code after it tokenized as code.
    const triple = tokenize(grammars.python, 'x = """a\nb"""\ny = 1\n', 'python triple quotes')
    ok(
      triple.some((t) => t.tag === 'number' && t.text === '1'),
      'python: a triple-quoted string ends at its closing triple rather than running on',
    )
  }

  /*
   * Tokenizing a line has to cost time proportional to its length.
   *
   * Two hooks have already been quadratic. `[^:]*` in the YAML key regex scanned to the end
   * of the line and backtracked over the whole scan for every token on it, at 4.8 s for a
   * 200,000-character line; the markdown link regex did the same at every `[` that did not
   * open a link, and the shell `${` matcher at every expansion that was never closed. All
   * three are the same mistake, none of them freezes the window — CodeMirror parses under a
   * time budget — and all three simply mean the colour never arrives while a core spins.
   *
   * Two assertions, because neither is sound alone.
   *
   * The ceiling is absolute: no shape may take a quarter of a second for 100,000 characters.
   * The slowest *honest* shapes are the two bounded scans themselves — a line of `[` under
   * markdown and a line of `${` under shell each pay their 512-character bound at every
   * position — and they measure 48 ms and 45 ms here, against 1.5 s for the same shapes with
   * the bound taken off. 250 ms is therefore not twenty times above honest work; it is about
   * five times above it and six times below the failure, which is as central as it can be
   * placed and is roughly the geometric mean of the two. That is the honest margin, and it
   * is why the ratio below rather than this line is the real instrument: a box three times
   * slower than this one moves both numbers together and the ratio does not move at all.
   *
   * The ratio is the sharper instrument: an eightfold length step should cost about eight
   * times as much, and 20 sits between a linear 8 and a quadratic 64. It is skipped when the
   * smaller run is too fast to time — a shape a language answers by skipping to the end of
   * the line measures at a few hundred nanoseconds, and dividing by that is noise, not a
   * measurement. Those shapes are still covered by the ceiling. Both runs are best-of-three
   * for the same reason.
   *
   * A language stops being measured after its first breach. That is not tidiness: with the
   * YAML hook's quadratic put back, finishing all twenty-four shapes for all nine grammars
   * took long enough that the run had to be killed, and a regression check nobody waits for
   * is a regression check nobody runs. One named failure is the whole message anyway.
   */
  const SMALL = 12_500
  const LARGE = 100_000
  const RATIO_LIMIT = 20
  const CEILING_MS = 250
  /** Below this the smaller run is timer noise and the ratio means nothing. */
  const MEASURABLE_MS = 0.05

  const timeLine = (spec, line) => {
    let best = Infinity
    for (let run = 0; run < 3; run++) {
      const state = spec.startState()
      const stream = new StringStream(line, 4, 2)
      const started = process.hrtime.bigint()
      while (!stream.eol()) {
        stream.start = stream.pos
        spec.token(stream, state)
        if (stream.pos <= stream.start) return Infinity
      }
      const elapsed = Number(process.hrtime.bigint() - started) / 1e6
      if (elapsed < best) best = elapsed
    }
    return best
  }

  // Each of these is a shape that has, or could have, no terminator anywhere on the line —
  // which is what makes a scanning regex run to the end and come back.
  const shapes = {
    'unclosed brackets': '[',
    'unclosed brace expansions': '${',
    'unclosed link text': '[ab ',
    punctuation: ',',
    'emphasis marks': '*',
    underscores: '_',
    backticks: '`',
    quotes: '"',
    apostrophes: "'",
    hashes: '#',
    'angle brackets': '<',
    colons: ':',
    dashes: '-',
    ampersands: '&',
    dollars: '$',
    bangs: '!',
    'attribute openers': '#[',
    'raw string openers': 'r#"',
    'macro bangs': 'a! ',
    words: 'ab ',
    'key-value pairs': 'a: b, ',
    'links and prose': '[a](b) ',
    backslashes: '\\',
    identifiers: 'a_b$c ',
  }

  for (const [lang, spec] of Object.entries(grammars)) {
    const before = failed
    for (const [shape, unit] of Object.entries(shapes)) {
      const line = (length) => unit.repeat(Math.ceil(length / unit.length)).slice(0, length)
      const small = timeLine(spec, line(SMALL))
      const large = timeLine(spec, line(LARGE))
      ok(
        large < CEILING_MS,
        `${lang} tokenizes ${LARGE} chars of ${shape} under ${CEILING_MS}ms ` +
          `(took ${large.toFixed(1)}ms)`,
      )
      if (small >= MEASURABLE_MS) {
        const ratio = large / small
        ok(
          ratio < RATIO_LIMIT,
          `${lang} is linear in line length on ${shape}: ` +
            `${small.toFixed(2)}ms at ${SMALL} chars, ${large.toFixed(1)}ms at ${LARGE} ` +
            `(x${ratio.toFixed(1)}, limit x${RATIO_LIMIT})`,
        )
      }
      if (failed > before) {
        console.error(`  (skipping the remaining shapes for ${lang}; one is enough to fix)`)
        break
      }
    }
  }

  // ---------------------------------------------------------------------------------------
  // 6. The size gate's unit
  // ---------------------------------------------------------------------------------------

  {
    /*
     * `TextEncoder` is the authority here, deliberately: `byteSize.ts` exists to give the
     * same answer without allocating the encoding, so the assertion worth making is that it
     * does, on every shape where the two could differ.
     */
    const encoder = new TextEncoder()
    /*
     * `[text, units, bytes]`, and the last two are the point of the table.
     *
     * Comparing only against `TextEncoder` is a differential test with no fixture: every
     * sample would still pass after being edited down to `'ab'`, and the row that was there
     * to exercise four-byte encoding would go on reporting a pass while exercising nothing.
     * Pinning both lengths is what makes a sample stay its own category — and the pair of
     * them is exactly what `exceedsBytes` shortcuts on, so a row where they are equal and a
     * row where they differ by three are testing different code.
     */
    const samples = {
      empty: ['', 0, 0],
      ascii: ['const x = 1;\n', 13, 13],
      'two-byte': ['héllo wörld', 11, 13],
      'three-byte': ['漢字とかな', 5, 15],
      'four-byte, a surrogate pair': ['😀🎉', 4, 8],
      'a lone high surrogate': ['\ud800', 1, 3],
      'a lone low surrogate': ['\udc00', 1, 3],
      'a surrogate that is not a pair': ['a\ud800b', 3, 5],
      'a pair split by a stray': ['\ud83d😀', 3, 7],
      mixed: ['a é 漢 😀 z', 10, 15],
      // The boundary between the two-byte and three-byte forms, from both sides.
      'U+07FF': ['߿', 1, 2],
      'U+0800': ['ࠀ', 1, 3],
    }
    for (const [what, [text, units, expected]] of Object.entries(samples)) {
      eq(text.length, units, `the ${what} sample is still ${units} UTF-16 units`)
      eq(encoder.encode(text).length, expected, `the ${what} sample is still ${expected} bytes`)
      eq(utf8ByteLength(text), encoder.encode(text).length, `utf8ByteLength: ${what}`)
      // And the shortcut agrees with the count it is a shortcut for, at limits either side
      // of the answer and at the answer itself.
      const bytes = encoder.encode(text).length
      for (const limit of [0, 1, bytes - 1, bytes, bytes + 1, 1024]) {
        if (limit < 0) continue
        eq(exceedsBytes(text, limit), bytes > limit, `exceedsBytes(${what}, ${limit})`)
      }
    }

    /*
     * The bug this module was written for. 400,000 CJK characters are 400,000 UTF-16 code
     * units and 1.2 MB of file, so `source.length > HIGHLIGHT_LIMIT_BYTES` says "small
     * enough to highlight" about a document a third larger than the limit — and the file
     * whose stream parse is slowest is exactly the one that gets a stream parser.
     */
    const HIGHLIGHT_LIMIT = 1024 * 1024
    const cjk = '漢'.repeat(400_000)
    eq(cjk.length > HIGHLIGHT_LIMIT, false, 'the old test called a 1.2 MB file small')
    eq(exceedsBytes(cjk, HIGHLIGHT_LIMIT), true, 'and the byte test does not')
    // The two shortcuts, on documents big enough that taking the count instead would show.
    eq(exceedsBytes('a'.repeat(2 * HIGHLIGHT_LIMIT), HIGHLIGHT_LIMIT), true, 'long is over')
    eq(exceedsBytes('a'.repeat(1000), HIGHLIGHT_LIMIT), false, 'short is under')
    eq(exceedsBytes('漢'.repeat(300_000), HIGHLIGHT_LIMIT), false, '900 KB of CJK is under')

    // 5 MB of ASCII must not cost a walk of five million code units to reject.
    {
      const huge = 'const x = 1;\n'.repeat(400_000)
      const started = Date.now()
      eq(exceedsBytes(huge, HIGHLIGHT_LIMIT), true, 'a 5 MB file is over the limit')
      const elapsed = Date.now() - started
      ok(elapsed < 10, `the length shortcut answers without a scan (took ${elapsed}ms)`)
    }
  }

  // ---------------------------------------------------------------------------------------
  // 7. The open-buffer registry
  // ---------------------------------------------------------------------------------------

  /*
   * `saveTab` is what `keys/dispatch.ts` calls for `file.save`, so its three answers are the
   * three things Ctrl+S can mean: this tab was written, this tab could not be written, and
   * there is no editor here at all. The last one has to be distinguishable from the other
   * two — a Ctrl+S with a terminal focused is not a failed save — which is why it is `null`
   * and not a rejected promise.
   */
  {
    const saved = []
    const ok_ = (tab) => () => {
      saved.push(tab)
      return Promise.resolve()
    }

    buffers.registerBuffer('tab-a', ok_('tab-a'))
    buffers.registerBuffer('tab-b', ok_('tab-b'))
    buffers.registerBuffer('tab-slow', () => new Promise((resolve) => setTimeout(resolve, 1)))
    buffers.registerBuffer('tab-rejects', () => Promise.reject(new Error('disk full')))
    buffers.registerBuffer('tab-throws', () => {
      throw new Error('synchronous')
    })

    eq(
      buffers.registeredBuffers().sort(),
      ['tab-a', 'tab-b', 'tab-rejects', 'tab-slow', 'tab-throws'],
      'every registered tab is listed',
    )

    eq(buffers.canSaveAll(['tab-a', 'tab-b']), true, 'canSaveAll: both are live')
    eq(buffers.canSaveAll(['tab-a', 'nope']), false, 'canSaveAll: one is not')
    eq(buffers.canSaveAll([]), false, 'canSaveAll: nothing to save is not "yes"')

    eq(buffers.saveTab(null), null, 'saveTab: no focused tab saves nothing')
    eq(buffers.saveTab('nope'), null, 'saveTab: a tab with no editor saves nothing')

    const saving = buffers.saveTab('tab-a')
    ok(saving !== null, 'saveTab: a registered tab returns a promise')
    await saving
    eq(saved, ['tab-a'], 'saveTab: and ran that tab’s saver, and only that one')

    // A saver that throws where it should have rejected must not throw out of `saveTab` —
    // that call happens inside a keydown handler, and an exception there skips the
    // `preventDefault` that keeps `^S` away from a PTY.
    let threw = false
    let rejected = false
    try {
      await buffers.saveTab('tab-throws')?.catch(() => {
        rejected = true
      })
    } catch {
      threw = true
    }
    eq(threw, false, 'saveTab: a synchronous throw does not escape')
    eq(rejected, true, 'saveTab: it arrives as a rejection instead')

    saved.length = 0
    const result = await buffers.saveAll(['tab-b', 'tab-rejects', 'nope', 'tab-a'])
    eq(result.failed, ['tab-rejects', 'nope'], 'saveAll: reports what it could not write')
    eq(saved, ['tab-b', 'tab-a'], 'saveAll: and wrote the rest, in the order it was given')

    buffers.unregisterBuffer('tab-a')
    eq(buffers.saveTab('tab-a'), null, 'unregisterBuffer: the tab is gone')
    eq(buffers.canSaveAll(['tab-a']), false, 'unregisterBuffer: and cannot be saved')

    /*
     * Which tab `file.save` means.
     *
     * `keys/dispatch.ts` cannot be compiled here — it reaches the IPC client, and that
     * reaches `@tauri-apps/api` — so the decision lives in `openBuffers.ts` and the
     * dispatcher holds one line that feeds it `useWorkspace.getState().boot`. These fixtures
     * are the shapes of `Bootstrap.role`, which is a three-variant union.
     */
    const shell = (active, projects) => ({ role: { kind: 'shell', active }, workspace: { projects } })
    eq(buffers.focusedTabOf(null), null, 'focusedTabOf: nothing is bootstrapped yet')
    eq(buffers.focusedTabOf(shell(null, {})), null, 'focusedTabOf: a shell with no project')
    eq(
      buffers.focusedTabOf(shell('p1', { p1: { activeTab: 'tab-7' } })),
      'tab-7',
      'focusedTabOf: the active project’s active tab',
    )
    eq(
      buffers.focusedTabOf(shell('gone', { p1: { activeTab: 'tab-7' } })),
      null,
      'focusedTabOf: an active project the mirror no longer holds',
    )
    /*
     * The one that a wrong implementation gets wrong silently. A detached window shares the
     * whole `Workspace` with the shell window, so `projects[…].activeTab` there names the tab
     * the *shell* is showing — a different file. The role names the right one.
     */
    for (const kind of ['detachedPane', 'detachedTab']) {
      eq(
        buffers.focusedTabOf({
          role: { kind, project: 'p1', tab: 'torn-out' },
          workspace: { projects: { p1: { activeTab: 'tab-7' } } },
        }),
        'torn-out',
        `focusedTabOf: a ${kind} window answers from its role, not the project`,
      )
    }

    for (const tab of buffers.registeredBuffers()) buffers.unregisterBuffer(tab)
    eq(buffers.registeredBuffers(), [], 'the registry is a module singleton and is left empty')
  }

  // ---------------------------------------------------------------------------------------
  // 8. The reveal queue and its clamp
  // ---------------------------------------------------------------------------------------

  /*
   * The queue.
   *
   * The case that decides the whole design is the *first* one below: the sidebar requests the
   * reveal and opens the tab in the same turn, so the editor mounts afterwards. A module that
   * delivered to whatever was mounted at request time would drop exactly the request the user
   * made — and would pass any test written the obvious way round, because a file that is
   * already open works either way.
   */
  {
    const at = (line, column, endColumn) => ({ line, column, endColumn })
    /** A mounted editor that records what it was told to reveal. */
    const editor = () => {
      const seen = []
      const receive = (target) => seen.push(`${target.line}:${target.column}-${target.endColumn}`)
      return { seen, receive }
    }

    /*
     * A mount is only committed once the stack it was made on has emptied.
     *
     * `registerReveal` holds a claimed request until the next microtask, because React
     * `StrictMode` — which `main.tsx` keeps on deliberately — mounts every effect twice in
     * development: setup, cleanup, setup, synchronously inside one commit. Everything in this
     * file runs on one stack, so without this the whole section would look like one enormous
     * StrictMode remount and no disposer here would be a real tab close.
     */
    const settle = () => new Promise((resolve) => queueMicrotask(resolve))

    eq(reveal.pendingReveals(), [], 'the queue starts empty')
    eq(reveal.revealReceivers(), [], 'and so does the registry')

    // 0. StrictMode's throwaway mount. This is not a hypothetical: the first version of this
    //    module spent the request on the discarded view, and the caret did not move in `pnpm
    //    dev` for the exact case the user reported — a hit in a file that was not open.
    {
      reveal.requestReveal('/w/strict.rs', at(7, 3, 8))
      const thrownAway = editor()
      const live = editor()
      const stopThrownAway = reveal.registerReveal('/w/strict.rs', thrownAway.receive) // setup
      stopThrownAway() //                                                                cleanup
      const stopLive = reveal.registerReveal('/w/strict.rs', live.receive) //             setup
      eq(live.seen, ['7:3-8'], 'a mount discarded by StrictMode does not swallow the request')
      await settle()
      eq(reveal.pendingReveals(), [], 'and the surviving mount spends it for good')
      stopLive()
      eq(reveal.pendingReveals(), [], 'closing that editor afterwards does not resurrect it')
      eq(reveal.revealReceivers(), [], 'and the registry is clear')
    }

    // 0b. The same remount, but the deadline. A re-parked request keeps the timestamp of the
    //     click; refreshing it would let a request whose file never opens live one TTL longer
    //     for every discarded mount, which is the drift `REVEAL_TTL_MS` exists to forbid. The
    //     real delay is what makes an unrefreshed stamp distinguishable from a refreshed one.
    {
      reveal.requestReveal('/w/stamp.rs', at(5, 1, 2))
      const clickedAt = Date.now()
      await new Promise((resolve) => setTimeout(resolve, 6))
      const thrownAway = editor()
      const stopThrownAway = reveal.registerReveal('/w/stamp.rs', thrownAway.receive)
      stopThrownAway() // Before any microtask, so this is the StrictMode cleanup.
      eq(reveal.pendingReveals(), ['/w/stamp.rs'], 'the discarded mount puts the request back')
      eq(
        reveal.pendingReveals(clickedAt + reveal.REVEAL_TTL_MS + 1),
        [],
        'and its deadline still runs from the click, not from the remount',
      )
      reveal.claimReveal('/w/stamp.rs')
    }

    // 1. A hit in a file that is not open: requested first, mounted second.
    {
      reveal.requestReveal('/w/a.rs', at(12, 5, 9))
      eq(reveal.pendingReveals(), ['/w/a.rs'], 'a request with nothing mounted is parked')
      const pane = editor()
      const stop = reveal.registerReveal('/w/a.rs', pane.receive)
      eq(pane.seen, ['12:5-9'], 'and the editor that mounts for that path is handed it')
      eq(reveal.pendingReveals(), [], 'the request is spent')
      await settle()

      // Spent, not broadcast: a second pane opened on the same file later is not scrolled to
      // a search the user has moved on from.
      const later = editor()
      const stopLater = reveal.registerReveal('/w/a.rs', later.receive)
      eq(later.seen, [], 'a later mount for the same path gets nothing')
      stopLater()
      stop()
      eq(reveal.revealReceivers(), [], 'and both unregistered themselves')
      // The hold is one turn, not a timer: a tab closed long after the reveal was shown must
      // not put it back, or reopening the file by hand would replay a search the user has
      // finished with — which is the whole point of `REVEAL_TTL_MS`.
      eq(reveal.pendingReveals(), [], 'and a real close does not re-park what was already shown')
    }

    // 2. A file that is already open. No mount will follow, so parking would drop it.
    {
      const pane = editor()
      const stop = reveal.registerReveal('/w/b.rs', pane.receive)
      reveal.requestReveal('/w/b.rs', at(3, 1, 4))
      eq(pane.seen, ['3:1-4'], 'a request for an open file is applied to it now')
      eq(reveal.pendingReveals(), [], 'and nothing is left parked for a mount that never comes')
      stop()
    }

    // 3. Two hits in quick succession. The second click is the one the user meant.
    {
      reveal.requestReveal('/w/c.rs', at(1, 1, 2))
      reveal.requestReveal('/w/c.rs', at(40, 7, 11))
      eq(reveal.pendingReveals(), ['/w/c.rs'], 'a second request for a path does not accumulate')
      const pane = editor()
      const stop = reveal.registerReveal('/w/c.rs', pane.receive)
      eq(pane.seen, ['40:7-11'], 'and it supersedes the first rather than queueing behind it')
      await settle()
      stop()
    }
    {
      // The same, but the file was already open: both are delivered, in order, because each
      // one moved a caret the user could see.
      const pane = editor()
      const stop = reveal.registerReveal('/w/d.rs', pane.receive)
      reveal.requestReveal('/w/d.rs', at(1, 1, 2))
      reveal.requestReveal('/w/d.rs', at(40, 7, 11))
      eq(pane.seen, ['1:1-2', '40:7-11'], 'two hits in an open file both move the caret')
      stop()
    }
    {
      // Different files do not supersede each other: both tabs are opening.
      reveal.requestReveal('/w/e.rs', at(2, 1, 2))
      reveal.requestReveal('/w/f.rs', at(3, 1, 2))
      eq(reveal.pendingReveals(), ['/w/e.rs', '/w/f.rs'], 'two paths are parked independently')
      const e = editor()
      const f = editor()
      const stopE = reveal.registerReveal('/w/e.rs', e.receive)
      const stopF = reveal.registerReveal('/w/f.rs', f.receive)
      eq([e.seen, f.seen], [['2:1-2'], ['3:1-2']], 'and each mount takes its own')
      await settle()
      stopE()
      stopF()
      eq(reveal.pendingReveals(), [], 'and neither leaves anything behind when it closes')
    }

    // 4. A split: one file, two panes, one gesture. Neither pane is more right than the other
    //    and this module cannot see which has focus, so both are told.
    {
      const left = editor()
      const right = editor()
      const stopLeft = reveal.registerReveal('/w/split.rs', left.receive)
      const stopRight = reveal.registerReveal('/w/split.rs', right.receive)
      reveal.requestReveal('/w/split.rs', at(9, 2, 3))
      eq([left.seen, right.seen], [['9:2-3'], ['9:2-3']], 'both panes over one file are revealed')

      stopLeft()
      reveal.requestReveal('/w/split.rs', at(10, 1, 2))
      eq(left.seen.length, 1, 'a closed pane stops being told')
      eq(right.seen, ['9:2-3', '10:1-2'], 'and the one still open keeps being told')
      // The disposer is keyed on the receiver, not the path — closing one pane must not
      // disconnect the other, and calling it twice must not disconnect anything at all.
      stopLeft()
      reveal.requestReveal('/w/split.rs', at(11, 1, 2))
      eq(right.seen.length, 3, 'a disposer called twice does not take the other pane down')
      stopRight()
      eq(reveal.revealReceivers(), [], 'the last pane out clears the entry')
      reveal.requestReveal('/w/split.rs', at(12, 1, 2))
      eq(reveal.pendingReveals(), ['/w/split.rs'], 'and a later request parks again')
      eq(reveal.claimReveal('/w/split.rs'), { line: 12, column: 1, endColumn: 2 }, 'claimed')
      eq(reveal.claimReveal('/w/split.rs'), null, 'and a claim spends it')
      eq(reveal.claimReveal('/w/never-asked-for'), null, 'a path nobody asked about claims null')
    }

    /*
     * 5. A path that never opens. Both bounds, because they answer different failures: the cap
     *    is the memory, and the deadline is the behaviour. Without the deadline a request
     *    parked for a file that never opened would fire when the user opens that file by hand
     *    an hour later, moving the caret for a search they have forgotten.
     */
    {
      for (const path of reveal.pendingReveals()) reveal.claimReveal(path)
      for (let i = 0; i < reveal.PENDING_LIMIT + 4; i++) {
        reveal.requestReveal(`/w/never-${i}.rs`, at(i + 1, 1, 2))
      }
      eq(reveal.pendingReveals().length, reveal.PENDING_LIMIT, 'the queue cannot grow past its cap')
      eq(
        reveal.pendingReveals()[0],
        '/w/never-4.rs',
        'and it is the oldest request that is dropped, not the newest',
      )
      // A repeated request counts as new: re-inserted rather than updated in place, or the
      // path the user just clicked would be first in line for eviction.
      reveal.requestReveal('/w/never-4.rs', at(99, 1, 2))
      reveal.requestReveal('/w/fresh.rs', at(1, 1, 2))
      ok(
        reveal.pendingReveals().includes('/w/never-4.rs'),
        'a re-requested path goes to the back of the eviction queue',
      )

      const stale = Date.now() + reveal.REVEAL_TTL_MS + 1
      eq(reveal.claimReveal('/w/fresh.rs', Date.now()), { line: 1, column: 1, endColumn: 2 },
        'a request claimed at once is honoured')
      reveal.requestReveal('/w/fresh.rs', at(1, 1, 2))
      eq(reveal.pendingReveals(stale), [], 'nothing in the queue survives the deadline')
      eq(reveal.claimReveal('/w/fresh.rs', stale), null, 'and a stale request is not applied')
      eq(reveal.claimReveal('/w/fresh.rs'), null, 'looking at it spent it, stale or not')

      for (const path of reveal.pendingReveals()) reveal.claimReveal(path)
      eq(reveal.pendingReveals(), [], 'the queue is a module singleton and is left empty')
      eq(reveal.revealReceivers(), [], 'and so is the registry')
    }
  }

  /*
   * The clamp.
   *
   * `Text` is CodeMirror's own, so `doc.line()` throws here exactly where it throws in the
   * editor — which is the whole point. The file can have been edited, truncated or replaced
   * between the search running and the click landing, and an out-of-range dispatch does not
   * merely miss: it throws inside CodeMirror, escapes the sidebar's click handler, and takes
   * the React root — every terminal in the window with it.
   */
  {
    const doc = Text.of(['first line', 'héllo wörld', '', 'last'])
    const range = (line, column, endColumn) => reveal.revealRange(doc, { line, column, endColumn })
    const span = (r) => [r.from, r.to]
    const threw = (fn) => {
      try {
        fn()
        return false
      } catch {
        return true
      }
    }

    // The unclamped arithmetic, so the assertions below are known to be load-bearing rather
    // than merely true. If `doc.line` ever stops throwing on these, the clamp still has to.
    ok(threw(() => doc.line(0)), 'doc.line(0) throws, which is what the line clamp prevents')
    ok(threw(() => doc.line(5)), 'and so does a line past the end of the document')

    eq(span(range(1, 1, 6)), [0, 5], 'a hit at the top of the file')
    eq(span(range(1, 7, 11)), [6, 10], 'and one further along the line')
    // Line 4 is the last, and it has no trailing break: `from` 24, four characters.
    eq(span(range(4, 1, 5)), [24, 28], 'a hit on the last line')
    eq(span(range(4, 1, 5))[1], doc.length, 'which ends exactly at the end of the document')
    eq(span(range(3, 1, 1)), [23, 23], 'a hit on an empty line is an empty selection on it')

    /*
     * UTF-16 units, not bytes. `héllo wörld` is 11 units and 13 bytes, so `w` is at column 7
     * here and at byte 8 on the wire — `SearchModel.hitPosition` is what converts, and this
     * is the assertion that says which of the two conventions arrives. Handed the byte
     * offsets instead, this selection would start on the `ö`.
     */
    eq(span(range(2, 7, 11)), [17, 21], 'columns are UTF-16 units on a non-ASCII line')
    eq(doc.sliceString(17, 21), 'wörl', 'and land on the characters they name')
    eq(doc.sliceString(18, 22), 'örld', 'where the byte columns would have landed one late')

    // The file changed since the search ran. Every one of these throws unclamped.
    eq(span(range(4, 200, 400)), [28, 28], 'a hit past the end of a shortened line')
    eq(span(range(2, 5, 900)), [15, 22], 'a match running past the end of its line stops at it')
    eq(span(range(99, 1, 2)), [24, 25], 'a hit past the end of a shortened file clamps to the last line')
    eq(span(range(0, 1, 2)), [0, 1], 'line 0 is the first line')
    eq(span(range(-5, 1, 2)), [0, 1], 'and so is a negative line')
    eq(span(range(1, 0, 3)), [0, 2], 'column 0 is the start of the line')
    eq(span(range(1, -9, 3)), [0, 2], 'and so is a negative column')
    eq(span(range(1, 6, 2)), [5, 5], 'an endColumn before the column is an empty selection')
    eq(span(range(1, 3, 3)), [2, 2], 'an empty match is an empty selection at the caret')
    eq(span(range(1.7, 2.9, 4.2)), [1, 3], 'a fractional position is truncated, not rounded up')

    /*
     * `NaN` fails every comparison a clamp is made of, so it sails through one written the
     * obvious way and throws at the end of it. It cannot come out of `hitPosition` today; it
     * costs one `Number.isFinite` to make sure it can never reach `doc.line`.
     */
    const wild = [NaN, Infinity, -Infinity, 1e21, -1e21, 0, -1, 4.5, Number.MAX_SAFE_INTEGER]
    for (const line of wild) {
      for (const column of wild) {
        for (const endColumn of wild) {
          const what = `line ${line}, column ${column}, endColumn ${endColumn}`
          let r = null
          ok(!threw(() => (r = range(line, column, endColumn))), `revealRange survives ${what}`)
          if (r === null) continue
          ok(r.from >= 0 && r.to <= doc.length, `and stays inside the document on ${what}`)
          ok(r.from <= r.to, `and never runs backwards on ${what}`)
          // On the document, and on *one line* of it: a selection that spilled onto the next
          // line would be a highlight over text the search never matched.
          ok(
            doc.lineAt(r.from).number === doc.lineAt(r.to).number,
            `and stays on one line on ${what}`,
          )
        }
      }
    }

    // A one-line document with no break at all, and an empty one: `line.to === line.from`, so
    // every column on them clamps to the same position.
    for (const [what, text] of Object.entries({ empty: '', 'no break': 'abc' })) {
      const tiny = Text.of(text.split('\n'))
      for (const target of [{ line: 1, column: 1, endColumn: 2 }, { line: 9, column: 90, endColumn: 99 }]) {
        const r = reveal.revealRange(tiny, target)
        ok(r.from >= 0 && r.to <= tiny.length, `a ${what} document clamps to itself`)
      }
    }
  }

  // --- and that the one caller which can request a reveal actually does ------------------
  //
  // `revealRequest.ts` is a registry: the receiving half lives in `EditorSurface` and the
  // requesting half is `App.tsx`, which is the only place that knows a search hit was
  // clicked. Everything above tests the module in isolation, and the module passed its own
  // tests while nothing in the app ever called it — the file opened at the top and the
  // user's report stood. That is this project's most repeated defect, so it gets a gate.
  //
  // A source assertion. It proves the call is written, not that the caret lands — the clamp
  // and the queue above cover the landing. Both halves are checked, because either one alone
  // is a feature that does nothing: a four-argument callback that discards three arguments
  // reads as correct, and so does a `requestReveal` nobody calls.
  // Comments stripped, and that is the whole point of this line.
  //
  // This assertion used to read raw source for `requestReveal(`. App.tsx stopped calling it
  // when jumps moved behind `jumpTo`, and the only remaining match was the *prose explaining
  // that it no longer calls it* — so the check passed on a file that navigated nowhere.
  // Mutation-tested: deleting the `jumpTo` from the search-hit handler left the gate green.
  // It is the exact trap this file warns about elsewhere, sprung on itself.
  const appSrc = readFileSync('src/App.tsx', 'utf8')
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/(^|[^:])\/\/.*$/gm, '$1')
  ok(
    /jumpTo\(/.test(appSrc),
    'App.tsx routes navigation through `jumpTo` — without it a search hit opens the file at ' +
      'the top, and the jump is missing from the Back stack',
  )
  ok(
    /onOpenHit=\{\(path, line, column, endColumn\)/.test(appSrc),
    'App.tsx receives all four `onOpenHit` arguments — dropping them is how this shipped dead',
  )

  // ---------------------------------------------------------------------------------------
  // 9. Send lines to Claude
  // ---------------------------------------------------------------------------------------

  /*
   * The user's report was "send lines to Claude does nothing from selected text", and the two
   * halves of that are checked in two different ways because they fail in two different ways.
   *
   * The arithmetic and the wording are exercised directly — a caret is not a range, and the
   * label must never say "lines 12–12". The wiring is a source assertion, the same shape as the
   * `requestReveal` gate above and for the same reason: this project's most repeated defect is
   * a module that passes its own tests while nothing calls it, and `useSendToClaude` calling
   * `claudeSend.lines` is worth exactly as much as `codeMenu` and `EditorSurface` calling
   * `useSendToClaude`.
   */
  {
    eq(send.rangeOf('', 12, 12), null, 'an empty selection is a caret, not a range')
    eq(send.rangeOf('', 12, 14), null, 'a zero-width selection across lines is still a caret')
    eq(send.rangeOf('x', 12, 12), { lineStart: 12, lineEnd: 12 }, 'one line')
    eq(send.rangeOf('x\ny', 12, 13), { lineStart: 12, lineEnd: 13 }, 'two lines')
    // A selection dragged upwards hands its head and anchor over in the other order.
    eq(send.rangeOf('x\ny', 13, 12), { lineStart: 12, lineEnd: 13 }, 'a backwards drag')

    eq(send.sendLabel(null), 'Send this file to Claude', 'no selection sends the file')
    eq(
      send.sendLabel({ lineStart: 12, lineEnd: 12 }),
      'Send line 12 to Claude',
      'one line is singular — "lines 12–12" is how a user learns not to read the label',
    )
    eq(
      send.sendLabel({ lineStart: 12, lineEnd: 20 }),
      'Send lines 12–20 to Claude',
      'a span names both ends',
    )

    // 1-based in the label, 0-based on the wire, converted once in `cmd::file::claude_send_lines`.
    // This string is what a user will see in the prompt, so it is what the log has to record.
    eq(send.mentionLabel('/src/main.rs', null), '@/src/main.rs', 'a whole-file mention')
    eq(
      send.mentionLabel('/src/main.rs', { lineStart: 10, lineEnd: 20 }),
      '@/src/main.rs#L10-20',
      'the documented mention spelling, 1-based',
    )
    eq(
      send.mentionLabel('/src/main.rs', { lineStart: 10, lineEnd: 10 }),
      '@/src/main.rs#L10',
      'a single line carries no range',
    )

    const hookSrc = readFileSync('src/editor/useSendToClaude.ts', 'utf8')
    ok(
      /claudeSend\.lines\(/.test(hookSrc),
      'useSendToClaude actually invokes the command — the whole report was that it did not',
    )
    ok(
      !/\.catch\(/.test(hookSrc),
      'the send is deliberately uncaught: `Failures` shows the rejection, and a `.catch` here ' +
        'restores the silent no-op this change exists to remove',
    )

    const menuSrc = readFileSync('src/editor/codeMenu.tsx', 'utf8')
    ok(/toClaude\.send\(/.test(menuSrc), 'the context-menu item is wired to the send')
    ok(
      // Anchored to the start of a line, because the file *discusses* the command at length
      // in a comment and the discussion is the reason it is gone.
      !/^\s*command: 'claude\.mention\.file'/m.test(menuSrc),
      'no chip for `claude.mention.file`: App.tsx does not dispatch it, so the chip would ' +
        'advertise a shortcut that does nothing',
    )


    /*
     * Go to definition, pinned at the source.
     *
     * Nothing gated this item before: the identifier appeared only in codeMenu.tsx, so a version
     * that was drawn, enabled and wired to nothing would have shipped through a fully green gate.
     * That is the defect this project has shipped more times than any other, so the assertions
     * below are about *being called*, not about being defined.
     *
     * Comments stripped first — codeMenu.tsx argues at length about the disabled reason it used to
     * carry, and a pin written against raw source would match the argument instead of the code.
     */
    const strip = (src) =>
      src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:])\/\/.*$/gm, '$1')
    const menuCode = strip(menuSrc)
    /*
     * Scoped to the item, not to the file.
     *
     * A whole-file grep for `goToDefinition(` passes on a file that imports it and calls it from
     * somewhere else entirely, and an earlier version of this check did exactly that. The block
     * from the item's `id` to the start of the next entry is what has to contain the call.
     */
    const itemBlock = (src, id) => {
      const at = src.indexOf(`id: '${id}'`)
      if (at === -1) return ''
      const next = src.indexOf("id: '", at + 10)
      return src.slice(at, next === -1 ? src.length : next)
    }
    const gotoItem = itemBlock(menuCode, 'goToDefinition')
    ok(gotoItem.length > 0, 'the Go to definition item still exists')
    ok(
      /run:/.test(gotoItem) && /goToDefinition\(/.test(gotoItem),
      'the Go to definition item carries a `run` that calls the helper — it was drawn and ' +
        'disabled for two milestones, and being drawn is not the same as being wired',
    )
    ok(
      !/does not (yet )?resolve|Needs a language server|No language server yet/.test(gotoItem),
      'no leftover "there is no language server" excuse on the item: a client that sends ' +
        '`textDocument/definition` makes every wording of that sentence false',
    )
    ok(
      /goToDefinition\(/.test(strip(readFileSync('src/keys/dispatch.ts', 'utf8'))),
      'the keyboard route reaches the same helper — a command id with no dispatch case is listed ' +
        'in the palette and inert',
    )

    /*
     * The click modifiers, which are pure convention and therefore silently reversible.
     *
     * CodeMirror's default for `clickAddsSelectionRange` is `ctrlKey` off macOS, so *deleting*
     * this override does not break a build or a type — it quietly hands Ctrl+click back to
     * multi-cursor and takes the whole Ctrl gesture away with it. There is no runtime check that
     * could notice.
     *
     * **This moved in M14**, and the assertion moved with it. The facet and the `mousedown` now
     * live in `ctrlLink.ts` beside the hover, because the underline is a promise about what the
     * click will do and two handlers in two files would each hold their own idea of which word is
     * under the pointer. Pinning them here would have kept passing against an `EditorSurface` that
     * still carried a dead copy.
     */
    const linkCode = strip(readFileSync('src/editor/ctrlLink.ts', 'utf8'))
    ok(
      /clickAddsSelectionRange\.of\(\(event\) => event\.altKey\)/.test(linkCode),
      'Alt adds carets: without this override CodeMirror puts multi-cursor back on Ctrl+click, ' +
        'which is the chord the Ctrl gesture needs',
    )
    /*
     * Again scoped, and for the same reason: two independent whole-file greps ANDed together
     * would pass on a file that has a `mousedown` handler doing something else and a
     * `ctrlActivate` call somewhere unrelated — which is precisely the arrangement this is
     * supposed to detect.
     */
    const mousedownAt = linkCode.indexOf('mousedown:')
    ok(mousedownAt !== -1, 'ctrlLink installs a mousedown handler')
    const mousedownBlock = linkCode.slice(mousedownAt, mousedownAt + 900)
    ok(
      /holdsCtrl\(event\)/.test(mousedownBlock) && /ctrlActivate\(/.test(mousedownBlock),
      'Ctrl+click acts from inside that handler: on mousedown, because a `click` fires ' +
        'after CodeMirror has already moved the caret',
    )
    ok(
      /\.focus\(\)/.test(mousedownBlock),
      'the handler focuses the editor: returning true skips the CodeMirror path that would ' +
        'otherwise have done it, leaving the keyboard pointed at the previous pane',
    )
    ok(
      /ctrlLink\(project, path\)/.test(strip(readFileSync('src/editor/EditorSurface.tsx', 'utf8'))),
      'and EditorSurface installs the extension — a gesture nothing mounts is the defect this ' +
        'project ships most often',
    )

    const surfaceSrc = readFileSync('src/editor/EditorSurface.tsx', 'utf8')
    ok(
      /key: 'Alt-Enter'/.test(surfaceSrc),
      'EditorSurface binds ⌥⏎ — without it the gesture is mouse-only, which is a gesture most ' +
        'people never find',
    )
  }

  // ---------------------------------------------------------------------------------------
  // 10. The status bar's file readout
  // ---------------------------------------------------------------------------------------

  /*
   * `crates › cide-core › src › lib.rs · Rust · UTF-8 · LF · Ln 7, Col 48` moved out of the
   * editor's 28px breadcrumb row and into the status bar, which put a single slot behind an
   * unbounded number of editors. Everything that can go wrong with that is invisible on screen
   * until it is annoying: a split whose second pane reports the position of the pane you are
   * not typing in, a closed tab that leaves its caret on the bar forever, a trail computed
   * against a root that had not loaded yet, a listener the bar never drops.
   *
   * The trail arithmetic and the claim stack are exercised directly. The wiring is a source
   * assertion, the same gate `requestReveal` and `useSendToClaude` get above: a channel with
   * no publisher and a bar with no subscriber both pass every test a module writes about
   * itself.
   */
  {
    eq(
      readout.formatReadout({ language: 'Markdown', ending: 'LF', cursor: 'Ln 7, Col 48' }),
      'Markdown · UTF-8 · LF · Ln 7, Col 48',
      'the readout is the line the user reported, separators and all',
    )

    // --- the trail -------------------------------------------------------------------------
    const ROOT = '/home/lantian/work/cide'
    eq(
      readout.pathTrail(`${ROOT}/crates/cide-core/src/lib.rs`, ROOT),
      ['crates', 'cide-core', 'src', 'lib.rs'],
      'a file inside the project is drawn relative to it',
    )
    eq(
      readout.pathTrail(`${ROOT}/crates/cide-core/src/lib.rs`, `${ROOT}/`),
      ['crates', 'cide-core', 'src', 'lib.rs'],
      'a root with a trailing slash means the same thing',
    )
    eq(
      readout.pathTrail('/etc/hosts', ROOT),
      ['etc', 'hosts'],
      'a file outside the project shows its own path rather than a relative fiction',
    )
    eq(
      readout.pathTrail('/etc/hosts'),
      ['etc', 'hosts'],
      'and so does one opened with no project at all',
    )
    eq(readout.pathTrail('/etc/hosts', ''), ['etc', 'hosts'], 'an empty root is no root')
    eq(
      readout.pathTrail(`${ROOT}-other/src/lib.rs`, ROOT),
      ['home', 'lantian', 'work', 'cide-other', 'src', 'lib.rs'],
      'a sibling whose name merely starts with the root is not inside it — the trailing ' +
        'separator is what makes `cide-other` fall through to its own path',
    )

    // --- the slot --------------------------------------------------------------------------
    const RS = ['crates', 'cide-core', 'src', 'lib.rs']
    const TOML = ['Cargo.toml']
    const line = (trail, detail) => ({ trail, detail })

    eq(readout.statusReadout(), line([], ''), 'no editor open means an empty slot the bar hides')

    const seen = []
    const stop = readout.subscribeStatusReadout((l) => seen.push(l))
    eq(seen, [line([], '')], 'a subscriber is told the current line at once, not on the next change')

    const first = readout.claimStatusReadout(RS, 'Rust · UTF-8 · LF · Ln 1, Col 1')
    eq(
      readout.statusReadout(),
      line(RS, 'Rust · UTF-8 · LF · Ln 1, Col 1'),
      'a mounted editor claims the slot, trail and all',
    )

    first.set('Rust · UTF-8 · LF · Ln 9, Col 3')
    eq(readout.statusReadout().detail, 'Rust · UTF-8 · LF · Ln 9, Col 3', 'the caret moves it')
    const beforeIdle = seen.length
    first.set('Rust · UTF-8 · LF · Ln 9, Col 3')
    eq(seen.length, beforeIdle, 'setting the same line again notifies nobody')

    // The boot race: the project record lands after the editor did, and the trail it was
    // claimed with was drawn against the fallback root.
    first.setTrail(['cide', 'crates', 'cide-core', 'src', 'lib.rs'])
    eq(
      readout.statusReadout().trail,
      ['cide', 'crates', 'cide-core', 'src', 'lib.rs'],
      'a root that arrives late redraws the trail rather than leaving an absolute path',
    )
    const beforeSameTrail = seen.length
    first.setTrail(['cide', 'crates', 'cide-core', 'src', 'lib.rs'])
    eq(seen.length, beforeSameTrail, 'and an unchanged trail notifies nobody')
    first.setTrail(RS)

    // A split: the second pane mounts and takes the bar, which is right — it is the file the
    // user just opened.
    const second = readout.claimStatusReadout(TOML, 'TOML · UTF-8 · LF · Ln 1, Col 1')
    eq(readout.statusReadout().trail, TOML, 'the newest editor holds it')

    // …and typing in the first pane takes it back. Without this the bar reports the other
    // pane's `Ln 1, Col 1` while the user types, which is a wrong answer rather than a
    // missing one.
    first.set('Rust · UTF-8 · LF · Ln 10, Col 1')
    eq(
      readout.statusReadout().trail,
      TOML,
      'an unfocused pane moving its own caret does not steal the bar',
    )
    first.focus()
    eq(
      readout.statusReadout(),
      line(RS, 'Rust · UTF-8 · LF · Ln 10, Col 1'),
      'focusing it does, and brings the position it had been keeping',
    )
    eq(readout.readoutClaims().length, 2, 'focus reorders the claims rather than adding one')

    // Closing the focused pane hands the bar back to the one still open, rather than blanking
    // it — the other half of the split is still on screen with a caret in it.
    first.release()
    eq(readout.statusReadout().trail, TOML, 'releasing hands the slot down the stack')
    first.set('Rust · UTF-8 · LF · Ln 99, Col 1')
    first.setTrail(['gone.rs'])
    first.focus()
    eq(
      readout.statusReadout().trail,
      TOML,
      'a released handle is inert — a torn-down CodeMirror cannot write to the bar',
    )

    second.release()
    eq(readout.statusReadout(), line([], ''), 'the last editor closing empties the slot')
    eq(readout.readoutClaims(), [], 'and leaves no claim behind')
    eq(seen.at(-1), line([], ''), 'the subscriber was told, so both boxes empty and the CSS hides them')

    // The claim copies what it is handed: a caller mutating its own array afterwards — a
    // `useMemo` result it still owns — must not silently redraw the bar.
    const owned = ['src', 'main.rs']
    const third = readout.claimStatusReadout(owned)
    owned.push('nope')
    eq(readout.statusReadout().trail, ['src', 'main.rs'], 'the claim holds its own copy')
    third.release()

    stop()
    const quiet = seen.length
    readout.claimStatusReadout(RS, 'Python · UTF-8 · LF · Ln 1, Col 1').release()
    eq(seen.length, quiet, 'an unsubscribed listener stops being called')

    // `StatusBar` guards its `setTrail` with this, so it has to answer the way React needs.
    ok(readout.sameTrail(RS, [...RS]), 'equal trails compare equal across array identities')
    ok(!readout.sameTrail(RS, RS.slice(0, 3)), 'a shorter trail is a different trail')
    ok(!readout.sameTrail(RS, ['crates', 'cide-app', 'src', 'lib.rs']), 'so is a renamed segment')

    // --- and that every half of the wire is connected --------------------------------------
    const surfaceSrc = readFileSync('src/editor/EditorSurface.tsx', 'utf8')
    ok(
      /claimStatusReadout\(/.test(surfaceSrc) && /readout\?\.release\(\)/.test(surfaceSrc),
      'EditorSurface claims the slot and releases it — a claim without the release leaves a ' +
        'closed file’s caret on the bar',
    )
    ok(
      /setTrail\(segments\)/.test(surfaceSrc),
      'and redraws the trail when the root arrives, which is the one thing the claim cannot ' +
        'carry from mount',
    )
    ok(
      !/styles\.breadcrumbs/.test(surfaceSrc),
      'the breadcrumb row is gone — the whole point of the move was the 28px it cost every ' +
        'pane in a split',
    )

    const barSrc = readFileSync('src/chrome/StatusBar.tsx', 'utf8')
    ok(
      /subscribeStatusReadout\(/.test(barSrc),
      'StatusBar subscribes — without it the line is computed for nobody',
    )
    ok(
      // The span is written to from outside React. A child in JSX would be restored by the
      // next render of the bar, which happens on every branch change and every token tick.
      /<span className=\{styles\.readout\}[^>]*\/>/.test(barSrc),
      'and renders the readout span childless, so a re-render cannot overwrite the live text',
    )
    ok(
      /sameTrail\(prev, line\.trail\) \? prev : line\.trail/.test(barSrc),
      'while the trail goes through state guarded by `sameTrail` — an unguarded setter would ' +
        're-render the bar on every keystroke, which is what the DOM write above avoids',
    )

    const barCss = readFileSync('src/chrome/StatusBar.module.css', 'utf8')
    for (const slot of ['readout', 'path']) {
      ok(
        new RegExp(`\\.${slot}:empty\\s*\\{[^}]*display:\\s*none`).test(barCss),
        `an empty .${slot} is removed from the flex row, gap included, rather than left as a hole`,
      )
    }
    ok(
      /\.path\s*\{[^}]*direction:\s*rtl/.test(barCss) &&
        /\.crumb\s*\{[^}]*direction:\s*ltr/.test(barCss),
      'the trail still clips from the left, so a squeezed bar drops `crates ›` and keeps the ' +
        'file name',
    )
    ok(
      !/\.path\s*\{[^}]*display:\s*flex/.test(barCss),
      'and stays a block box: `text-overflow: ellipsis` needs inline content, and a flex ' +
        'container drops the ellipsis silently',
    )
  }

  // ---------------------------------------------------------------------------------------
  // 11. Arriving where the mention landed
  // ---------------------------------------------------------------------------------------

  /*
   * > *"send lines to Claude also should switch to console that added a selected text"*
   *
   * `useMentionTarget` falls back to the project's console — its first tab's Claude pane — so
   * the destination is routinely a tab the user is not looking at. `planReveal` decides what
   * stands between this window and that pane, and every branch of it is a case that ends with
   * the user seeing nothing happen if it is decided wrongly. None of it is reachable from a
   * screenshot: the interesting shapes are a background tab, a maximize hiding a laid-out
   * pane, a pane torn out into a window of its own, and a pane that has closed.
   *
   * The wiring is a source assertion for the reason every other one in this file is: this
   * project's most repeated defect is a module that passes its own tests while nothing calls
   * it, and a planner nobody runs after a send is precisely the reported bug with a test
   * suite attached.
   */
  {
    const shell = (projects, active = projects[0] ?? null) => ({ kind: 'shell', projects, active })
    const tabOf = (id, panes, focused, maximized = null) => ({
      id,
      tree: { focused, maximized, panes: Object.fromEntries(panes.map((p) => [p, {}])) },
    })
    // `windows` defaults to the ordinary arrangement — one shell, holding `p1` and drawing it.
    // The interesting shape is a shell that *holds* the project and is showing another one,
    // which `stacked` mode (the default) makes an everyday state, and it is passed explicitly.
    const bootOf = (role, project, windows = { 'shell:1': shell(['p1']) }) => ({
      role,
      workspace: { projects: { p1: project }, windows },
    })

    // The shape the complaint is about: the console is tab `t0`, the user is in file tab `t1`.
    const twoTabs = {
      activeTab: 't1',
      detached: {},
      tabs: [tabOf('t0', ['console'], 'console'), tabOf('t1', ['editor'], 'editor')],
    }

    eq(
      revealPlan.planReveal(bootOf(shell(['p1']), twoTabs), 'p1', 'console'),
      { here: true, tab: 't0', activateProject: false, activateTab: true, clearMaximize: false, focusPane: false, blocked: null },
      'a console in a background tab is reached by activating its tab — the reported case',
    )
    // `focusPane` is false above because `t0`'s tree already names `console` as focused. That
    // is not a detail: a redundant `pane_focus` is a round trip and a re-render of every pane
    // in the tab, paid on a gesture the user makes repeatedly.
    eq(
      revealPlan.planReveal(bootOf(shell(['p1']), { ...twoTabs, activeTab: 't0' }), 'p1', 'console'),
      { here: true, tab: 't0', activateProject: false, activateTab: false, clearMaximize: false, focusPane: false, blocked: null },
      'a console already in front asks for nothing at all',
    )

    // A maximized sibling: the pane is mounted and laid out at full size — `TabContent` and
    // `SplitTree` both keep it that way — and completely invisible. Nothing else in the plan
    // would notice, so the user would be switched to a tab that still does not show the pane.
    const maximized = {
      activeTab: 't0',
      detached: {},
      tabs: [tabOf('t0', ['console', 'shell'], 'shell', 'shell')],
    }
    eq(
      revealPlan.planReveal(bootOf(shell(['p1']), maximized), 'p1', 'console'),
      { here: true, tab: 't0', activateProject: false, activateTab: false, clearMaximize: true, focusPane: true, blocked: null },
      'a pane hidden under another pane’s maximize has that maximize cleared',
    )
    eq(
      revealPlan.planReveal(bootOf(shell(['p1']), maximized), 'p1', 'shell').clearMaximize,
      false,
      'and a pane that *is* the maximized one keeps it — that is already the view it wants',
    )

    // The two windows. `here` decides whether Rust is asked to raise anything, so getting it
    // wrong is either a missing raise or a focus steal aimed at the window already in front.
    const detached = {
      activeTab: 't0',
      detached: { torn: {} },
      tabs: [tabOf('t0', ['console'], 'console')],
    }
    eq(
      revealPlan.planReveal(bootOf(shell(['p1']), detached), 'p1', 'torn'),
      { here: false, tab: null, activateProject: false, activateTab: false, clearMaximize: false, focusPane: false, blocked: null },
      'a torn-out pane has no tab to activate: the raise is the whole of the reveal',
    )
    eq(
      revealPlan.planReveal(bootOf({ kind: 'detachedPane', pane: 'torn' }, detached), 'p1', 'torn').here,
      true,
      'and its own window does not ask to raise itself',
    )
    eq(
      revealPlan.planReveal(bootOf({ kind: 'detachedPane', pane: 'torn' }, detached), 'p1', 'console').here,
      false,
      'while an editor detached into its own window does have to raise the shell — the case ' +
        'mentionTarget’s fallback makes ordinary',
    )
    eq(
      revealPlan.planReveal(bootOf(shell(['other']), twoTabs), 'p1', 'console').here,
      false,
      'a shell that does not hold the project is not the window showing its console',
    )

    /*
     * The project the shell is *holding* but not *drawing*.
     *
     * `stacked` is the default window mode: every open project docks into one shell that
     * renders `role.active` and nothing else. So `tab_activate` alone moves a tab in a project
     * that is not on screen, and the raise that follows brings the shell forward still showing
     * the other project — the reveal reports success and the user sees nothing happen, which
     * is the exact report this feature answers.
     *
     * Reached from a detached editor: `useMentionTarget` names *its* project, which need not
     * be the one the shell was left on.
     */
    const stacked = { 'shell:1': shell(['p1', 'p2'], 'p2') }
    eq(
      revealPlan.planReveal(bootOf({ kind: 'detachedPane', pane: 'torn' }, twoTabs, stacked), 'p1', 'console'),
      { here: false, tab: 't0', activateProject: true, activateTab: true, clearMaximize: false, focusPane: false, blocked: null },
      'a shell parked on another project is switched to the one the mention landed in',
    )
    eq(
      revealPlan.planReveal(bootOf(shell(['p1', 'p2'], 'p1'), twoTabs, { 'shell:1': shell(['p1', 'p2'], 'p1') }), 'p1', 'console')
        .activateProject,
      false,
      'and a shell already drawing it is not switched to it again',
    )
    eq(
      revealPlan.planReveal(bootOf({ kind: 'detachedPane', pane: 'torn' }, twoTabs, {}), 'p1', 'console')
        .activateProject,
      false,
      'while no shell at all leaves nothing to switch — the raise says so instead',
    )
    // `perProject` mode: the project has a shell of its own that is already drawing it, and a
    // second shell merely holds it. Activating would drag that second window onto a project it
    // is not the one being raised for.
    eq(
      revealPlan.planReveal(
        bootOf({ kind: 'detachedPane', pane: 'torn' }, twoTabs, {
          'shell:1': shell(['p1', 'p2'], 'p2'),
          'shell:2': shell(['p1'], 'p1'),
        }),
        'p1',
        'console',
      ).activateProject,
      false,
      'and one shell drawing the project is enough, however many others merely hold it',
    )

    // The two "cannot be revealed at all" answers. Both are sentences rather than nulls,
    // because the caller says them out loud — a mention in a prompt nobody can see is exactly
    // as invisible as no mention at all.
    for (const [what, plan] of [
      ['a pane that has closed', revealPlan.planReveal(bootOf(shell(['p1']), twoTabs), 'p1', 'gone')],
      ['a project that has closed', revealPlan.planReveal(bootOf(shell(['p1']), twoTabs), 'p9', 'console')],
    ]) {
      ok(typeof plan.blocked === 'string' && plan.blocked.length > 10, `${what} is refused in words`)
      ok(!plan.activateProject && !plan.activateTab && !plan.clearMaximize && !plan.focusPane && plan.tab === null,
        `${what} asks for nothing to be moved`)
    }

    // --- and that the send actually runs it ------------------------------------------------
    const hookSrc = readFileSync('src/editor/useSendToClaude.ts', 'utf8')
    ok(
      /void sending\.then\(/.test(hookSrc) && !/sending\.finally\(/.test(hookSrc),
      'the reveal hangs off the send’s `then` — on `finally` a failed send would still switch ' +
        'tabs, which is the one thing the brief rules out',
    )
    /*
     * And it is the pane the send ANSWERED with that is revealed, never the one it was aimed
     * at. This assertion used to read `revealPane(target.project, target.pane)`, which was
     * right while `claude_send_lines` could only ever deliver to the pane it was given.
     * It cannot any more: the frontend's rule picks `tabs[0]`'s first Claude pane in map
     * order without asking whether that pane has a `claude` running — and in a restored
     * workspace it usually does not, because only the console's primary Claude pane spawns
     * eagerly and the rest sit at a resume splash. Rust therefore reroutes to a Claude that
     * can receive and says which one it used.
     *
     * So this is strengthened rather than relaxed: revealing `target.pane` after delivering
     * somewhere else is *worse* than the error it replaced — the user is taken to an empty
     * prompt while their selection sits in another conversation, and nothing on screen says
     * so. Both halves are pinned, because dropping either one restores that.
     */
    ok(
      /revealPane\(target\.project, sent\.pane\)/.test(hookSrc),
      'the pane the send resolved to is the one revealed',
    )
    ok(
      !/revealPane\(target\.project, target\.pane\)/.test(hookSrc),
      'and never the pane it was aimed at — that pane is a preference, and when it could not ' +
        'receive, going there shows an empty prompt beside a mention that landed elsewhere',
    )
    // `report(rerouted(` and not merely the two names: the helper is defined in this file and
    // `sent.fallback` is read again for the stuck-reveal wording, so a looser pattern is
    // satisfied with every call to it deleted. Checked by mutation.
    ok(
      /report\(rerouted\(/.test(hookSrc),
      'a reroute is said out loud. A selection that arrives in conversation B while the user ' +
        'believes it is in A is the one genuinely harmful outcome this gesture has, and the ' +
        'reveal alone does not name the conversation',
    )
    ok(
      /diag\.log\(`editor: sent \$\{mentionLabel\(path, span\)\} to \$\{sent\.pane\}`\)/.test(hookSrc),
      'and the log records the RESOLVED pane. It used to log the asked-for one, unconditionally ' +
        'and before the answer, so a real log of this failure read “sent … to 07565bbd” on the ' +
        'line under the server’s own “no connected claude in this pane; dropped”',
    )
    ok(
      /view\.state\.doc !== doc/.test(hookSrc),
      'guarded on the document, so a user who carried on typing does not have the keyboard ' +
        'taken out from under them mid-word',
    )

    const revealSrc = readFileSync('src/editor/revealPane.ts', 'utf8')
    ok(
      /plan\.activateProject/.test(revealSrc) &&
        revealSrc.indexOf('projectApi.activate(') < revealSrc.indexOf('ws.activateTab('),
      'the project is brought to the front before its tab is — a tab activated inside a ' +
        'project the shell is not drawing moves something nobody can see',
    )
    ok(
      revealSrc.indexOf('sendFocus.revealPane(') > revealSrc.indexOf('ws.focusPane('),
      'the window is raised after the domain has moved, so it comes forward already showing ' +
        'the right tab',
    )
    ok(
      /scrollToBottom\(\)/.test(revealSrc),
      'and the transcript is scrolled to the prompt — a pane the user had scrolled up in shows ' +
        'the mention off screen below the fold',
    )
  }

  /* ------------------------------------------------------ M12: diagnostics in the buffer */

  {
    const { lintRanges, offsetOf } = await import(`file://${join(out, 'editor/lintMap.js')}`)

    /** A three-line document: offsets 0-4, 6-14, 16-20. */
    const doc = {
      lines: 3,
      length: 21,
      line(n) {
        return [
          { from: 0, to: 5 },
          { from: 6, to: 15 },
          { from: 16, to: 21 },
        ][n - 1]
      },
    }
    const at = (line, column, endLine, endColumn, extra = {}) => ({
      line,
      column,
      endLine,
      endColumn,
      severity: 'error',
      message: 'boom',
      ...extra,
    })

    eq(offsetOf(doc, 1, 1), 0, '1-based line and column become a 0-based offset')
    eq(offsetOf(doc, 2, 3), 8, 'and the offset is relative to that line’s start')

    /*
     * The clamp, and it is not defensive programming. The buffer is edited while the analyser is
     * still thinking, so a diagnostic for a line that no longer exists is routine — and
     * `EditorView.dispatch` *throws* on an out-of-range range, with no error boundary above it.
     * An unclamped offset takes the React root down and every terminal in the window with it.
     */
    eq(offsetOf(doc, 999, 1), 16, 'a line past the end clamps to the last line')
    eq(offsetOf(doc, 2, 999), 15, 'a column past the end clamps to that line’s end')
    eq(offsetOf(doc, 0, 0), 0, 'and a position before the start clamps forward')
    eq(offsetOf({ lines: 0, length: 0, line: () => ({ from: 0, to: 0 }) }, 5, 5), 0, 'empty doc')

    // A zero-width range draws *nothing* — a real error with no squiggle, which reads as a
    // missed diagnostic rather than as a zero-width one.
    const point = lintRanges([at(1, 2, 1, 2)], doc)
    ok(point[0].to > point[0].from, 'an empty span is widened so it is actually drawn')

    // A producer that reports only a point, and the panel fixtures that predate end positions.
    const noEnd = lintRanges([{ line: 1, column: 2, severity: 'error', message: 'm' }], doc)
    ok(noEnd[0].to > noEnd[0].from, 'a diagnostic with no end position still gets a span')

    // An inverted range is one CodeMirror rejects outright.
    const inverted = lintRanges([at(2, 5, 1, 1)], doc)
    ok(inverted[0].to >= inverted[0].from, 'an end before its start is never sent as one')

    eq(
      lintRanges([at(3, 1, 3, 2), at(1, 1, 1, 2), at(2, 1, 2, 2)], doc).map((r) => r.from),
      [0, 6, 16],
      'ranges are sorted, because `setDiagnostics` throws on an unsorted list',
    )

    eq(
      lintRanges([at(1, 1, 1, 2, { source: 'rust-analyzer', code: 'E0308' })], doc)[0].source,
      'rust-analyzer E0308',
      'the producer and its code are joined for the dimmed half of the tooltip',
    )
    ok(
      !('source' in lintRanges([at(1, 1, 1, 2)], doc)[0]),
      'and the property is absent rather than undefined — CodeMirror’s type refuses the latter',
    )
    eq(
      lintRanges([at(1, 1, 1, 2, { severity: 'catastrophe' })], doc)[0].severity,
      'error',
      'an unrecognised severity is drawn rather than dropped — the `other`-bucket rule, as a squiggle',
    )
  }

  /* -------------------------------------------------- M12: the caret slot and the level */

  {
    const { claimCaret, focusedCaret, resetCaretsForTest } = await import(
      `file://${join(out, 'editor/caretTrack.js')}`
    )
    resetCaretsForTest()
    eq(focusedCaret(), null, 'no editor, no caret')

    const first = claimCaret('/a.rs')
    /*
     * `lines` is the document's size, added with Go to line, which has to say "this file has 892
     * lines" *before* the user commits to a number — the clamp in `revealRange` can only answer
     * afterwards, by moving the caret somewhere they did not name.
     *
     * Asserted as a whole object rather than field by field, and that is deliberate: `eq`
     * compares `JSON.stringify`, which **drops `undefined` values**, so `set(3, 7)` against the
     * three-argument signature would have produced `{path, line, column}` and compared equal to
     * the old expectation. A shape assertion that silently tolerates a missing field is the kind
     * of green this file exists to stop.
     */
    first.set(3, 7, 120)
    eq(
      focusedCaret(),
      { path: '/a.rs', line: 3, column: 7, lines: 120 },
      'the only claim answers, and carries the document size with the position',
    )
    ok(
      Object.hasOwn(focusedCaret(), 'lines'),
      'the size is a present property, not an implicit undefined that JSON.stringify would hide',
    )

    // A split: the newest claim wins, and focus takes it back — the same stack discipline the
    // status readout uses, and for the same reason.
    const second = claimCaret('/b.rs')
    eq(focusedCaret().lines, 1, 'a fresh claim reports one line, never zero — an empty doc is one')
    second.set(1, 1, 4)
    eq(focusedCaret().path, '/b.rs', 'mounting claims the slot')
    first.focus()
    eq(focusedCaret().path, '/a.rs', 'focus takes it back')
    eq(focusedCaret().lines, 120, 'and the size follows the position it belongs to')
    // An unfocused editor still tracks its own caret, so it is right the moment it is focused.
    second.set(9, 2, 40)
    eq(focusedCaret().line, 3, 'and a background editor does not overwrite the foreground one')
    second.focus()
    eq(focusedCaret().line, 9, 'but its position was kept')
    eq(focusedCaret().lines, 40, 'size included')

    second.release()
    eq(focusedCaret().path, '/a.rs', 'releasing hands the slot down rather than blanking it')
    second.set(1, 1, 4)
    eq(focusedCaret().path, '/a.rs', 'and a released handle can no longer write')
    first.release()
    eq(focusedCaret(), null, 'the last release empties the slot')
  }

  {
    const { levelFor, overrideFor, setLevel, clearLevel, reducedCount, resetLevelsForTest } =
      await import(`file://${join(out, 'editor/highlightLevel.js')}`)
    resetLevelsForTest()

    eq(levelFor('/a.rs', 'all'), 'all', 'with no override, a path follows the workspace default')
    eq(overrideFor('/a.rs'), null, 'and reports that it has none')
    setLevel('/a.rs', 'none')
    eq(levelFor('/a.rs', 'all'), 'none', 'an override wins')
    eq(levelFor('/b.rs', 'all'), 'all', 'and is per path')

    // "I chose the default for this file" and "this file follows the default" are different
    // intentions, and a later change to the default should move only the second.
    setLevel('/b.rs', 'all')
    eq(overrideFor('/b.rs'), 'all', 'choosing the default still records a choice')
    eq(levelFor('/b.rs', 'none'), 'all', 'so a changed default does not move it')

    eq(reducedCount('all'), 1, 'the panel can report how many editors are quieter than the default')
    clearLevel('/a.rs')
    eq(overrideFor('/a.rs'), null, 'clearing returns the path to the default')
    eq(reducedCount('all'), 0, 'and the count follows')
  }

  /* ------------------------------------------ M12: the lint CSS a later edit would undo */

  {
    const css = readFileSync('src/editor/EditorSurface.module.css', 'utf8')

    /*
     * `@codemirror/lint` bakes `#d11` and `orange` into inline-SVG `background-image` data URIs,
     * and a data URI cannot read a custom property — so a stock install survives a theme switch
     * with the wrong colours. Overriding it is mandatory, and these pins are what stop the
     * override being tidied away.
     */
    for (const [rule, token] of [
      ['cm-lintRange-error', '--red'],
      ['cm-lintRange-warning', '--yellow'],
      ['cm-lintRange-info', '--blue'],
      ['cm-lintRange-hint', '--faint'],
    ]) {
      const block = css.slice(css.indexOf(rule), css.indexOf(rule) + 220)
      ok(block.includes(`var(${token})`), `${rule} is drawn with ${token}, not a literal colour`)
    }
    ok(
      /\.cm-lintRange\)[^}]*background-image:\s*none/.test(css),
      'the stock SVG underline is turned off, or the literal colours come back with it',
    )
    /*
     * Declarations only. The first version of this matched the *comments* — including this
     * file's own header, which names `#f5f5f5` as the base theme's hardcoded background, and the
     * note above the lint block naming `#d11` as what CodeMirror ships. An assertion that a file
     * may not *mention* a colour is not the assertion anybody wanted.
     */
    const declarations = css.replace(/\/\*[\s\S]*?\*\//g, '')
    ok(
      !/#d11|\borange\b/.test(declarations),
      'no literal diagnostic colour is actually declared — only tokens reach the rules',
    )

    /*
     * The geometry. `.cm-lineNumbers` takes `flex: 1` of the gutter reservation, so the lint
     * column takes its width out of the numbers' share unless the reservation grows by exactly
     * that much — and every line number shifts left the moment a file gains its first diagnostic.
     */
    ok(/\.cm-gutters\)[^}]*min-width:\s*70px/.test(css), 'the gutter reserves room for the marker')
    ok(/\.cm-lint-marker\)[^}]*width:\s*14px/.test(css), 'and the marker is a fixed 14px')
    ok(
      css.split('.cm-gutters)').length === 2,
      'one `.cm-gutters` rule, so the width is not decided by cascade order',
    )
  }

  /* ------------------------------------------------ M12: the editor is actually wired up */

  {
    const surface = readFileSync('src/editor/EditorSurface.tsx', 'utf8')
    ok(
      surface.includes('setDiagnostics('),
      'diagnostics are pushed with `setDiagnostics`, not pulled by `linter()` — ours come from Rust',
    )
    ok(!surface.includes('lintKeymap'), 'and `lintKeymap` is not installed: there is no lint panel')
    ok(
      surface.includes('lintSlot.of(') && surface.includes('lintSlotRef.current'),
      'the gutter lives in a Compartment, so turning it off does not rebuild the view',
    )
    const pane = readFileSync('src/panes/EditorPane.tsx', 'utf8')
    ok(
      pane.includes('diagnostics={diagnostics}') && pane.includes('highlight={level}'),
      'and EditorPane actually feeds it — an unfed editor draws nothing and passes every test above',
    )
  }

  // ---------------------------------------------------------------------------------------
  // 12. Keys into the editor: undo/redo, the find bar, and the focus handoff
  // ---------------------------------------------------------------------------------------

  /*
   * Three reports, one root: a binding that exists and a surface that cannot reach it.
   *
   * The arithmetic half — the match ordinal and the reveal plan — is driven directly, because it
   * is import-free for exactly that reason. The rest is source assertions, which is the same gate
   * `useSendToClaude` and the Ctrl+click block get above and for the same reason: `find.ts`,
   * `EditorSurface.tsx` and `DiffPane.tsx` need a window, and every bug in this section was a
   * *missing call* rather than a wrong function — the kind of defect a module's own tests cannot
   * see, because the module was right.
   */
  {
    const { COUNT_CAP, countLabel, tally } = load('findMatches.js')

    /*
     * Section 9's `strip` is scoped to its own block, and every file read below argues about the
     * thing it is being checked for — `find.ts` discusses focus and the `main-field` attribute at
     * length, `codeMenu.tsx` discusses undo's absence from the palette. A pin written against raw
     * source would match the prose instead of the code, which is how a check certifies the bug it
     * was written for.
     */
    const strip = (src) => src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:])\/\/.*$/gm, '$1')

    /*
     * The find bar's number. `@codemirror/search` wraps silently in both directions, and the bar
     * used to render a bare total — so passing the last match and landing back on the first was
     * indistinguishable from the key having done nothing. `3 of 12` is the wrap indicator.
     */
    const at = (from, to) => ({ from, to })
    const three = [at(0, 2), at(10, 12), at(20, 22)]

    eq(tally(three, at(0, 2)), { total: 3, capped: false, ordinal: 1 }, 'the first match is 1')
    eq(tally(three, at(20, 22)), { total: 3, capped: false, ordinal: 3 }, 'the last match is the total')
    eq(
      tally(three, at(10, 12)).ordinal,
      2,
      'and the middle one is what it looks like — an off-by-one here is invisible on screen',
    )
    eq(
      tally(three, at(5, 7)).ordinal,
      null,
      'a selection that is not a match has no ordinal, so the bar falls back to the total',
    )
    eq(
      tally(three, at(0, 3)).ordinal,
      null,
      'both ends must agree: a caret parked at the *start* of a match is not standing on it',
    )
    eq(tally([], at(0, 0)), { total: 0, capped: false, ordinal: null }, 'no matches is zero, not null')

    // Wrap-around, which is the whole reason the ordinal exists: `findNext` past the last match
    // reselects the first, and the number going 3 → 1 is the only signal a sighted user gets.
    eq(countLabel(tally(three, at(20, 22))), '3 of 3', 'standing on the last match')
    eq(countLabel(tally(three, at(0, 2))), '1 of 3', 'and one press later, back at the top')

    // The cap. Counting is an unbudgeted `SearchCursor` walk, so past it the bar stops walking —
    // and the ordinal has to go with it, because the walk may have stopped before the selection.
    const many = { *[Symbol.iterator]() { for (let i = 0; i < 5000; i++) yield at(i * 4, i * 4 + 2) } }
    const capped = tally(many, at(0, 2))
    eq(capped.capped, true, 'past the cap the walk reports that it stopped')
    eq(capped.total, COUNT_CAP + 1, 'and stops one past it rather than running to the end')
    eq(capped.ordinal, null, 'the ordinal is suppressed: this walk may never have reached the selection')
    eq(countLabel(capped), `${COUNT_CAP}+`, 'and the label says so rather than naming a figure')

    // Laziness is a correctness property here, not a nicety: an eager consumer would walk a
    // five-megabyte document to the end on every keystroke, which is what the cap exists to stop.
    let produced = 0
    const counted = { *[Symbol.iterator]() { while (true) { produced++; yield at(produced * 4, produced * 4 + 2) } } }
    tally(counted, at(0, 0))
    ok(produced <= COUNT_CAP + 1, `the walk is abandoned at the cap (took ${produced} matches)`)

    eq(countLabel({ total: 1, capped: false, ordinal: null }), '1 match', 'the singular is singular')
    eq(countLabel({ total: 12, capped: false, ordinal: null }), '12 matches', 'and the plural is not')

    /*
     * The two lines that were missing from `find.ts`.
     *
     * Scoped to the member, never the file: `find.ts` *discusses* focus and the `main-field`
     * attribute in a comment, and that comment is precisely what made the missing focus call look
     * deliberate to a reviewer. A whole-file grep would have passed on the broken version.
     */
    const findSrc = readFileSync('src/editor/find.ts', 'utf8')
    const findCode = strip(findSrc)
    const memberBlock = (src, name) => {
      const start = src.indexOf(name)
      if (start === -1) return ''
      // To the next member of the class: a `private`/`readonly`/`update(`/`destroy(` boundary.
      const next = src
        .slice(start + name.length)
        .search(/\n {2}(private|readonly|update\(|destroy\(|constructor\()/)
      return next === -1 ? src.slice(start) : src.slice(start, start + name.length + next)
    }

    const mountBlock = memberBlock(findCode, 'mount(): void {')
    ok(mountBlock.length > 0, 'FindPanel still has a mount()')
    ok(
      /this\.input\.focus\(\)/.test(mountBlock) && /this\.input\.select\(\)/.test(mountBlock),
      'mount() focuses and selects the field: `openSearchPanel`\'s closed-panel branch only ' +
        'dispatches `togglePanel`, so without this Ctrl+F opened the bar and the query the user ' +
        'typed next went into their file',
    )

    const keyBlock = memberBlock(findCode, 'onKeyDown = (event: KeyboardEvent)')
    ok(keyBlock.length > 0, 'FindPanel still has a keydown handler')
    ok(
      /runScopeHandlers\(this\.view, event, 'search-panel'\)/.test(keyBlock),
      'the handler bridges into `search-panel` scope, and does it first: the panel is a sibling ' +
        'of contentDOM, so without this F3, Shift+F3 and Ctrl+G are dead in the one place the ' +
        'caret actually is after Ctrl+F',
    )
    ok(
      keyBlock.indexOf('runScopeHandlers') < keyBlock.indexOf("'Enter'"),
      'scope handlers run before the hand-rolled Enter, matching upstream — the other order ' +
        'would let Enter shadow a binding scoped to this panel',
    )
    ok(
      /import \{[^}]*\brunScopeHandlers\b[^}]*\} from '@codemirror\/view'/s.test(findCode),
      'runScopeHandlers comes from @codemirror/view — the scope string is matched by value, so a ' +
        'typo in either half fails silently and for ever',
    )
    ok(
      /searchKeymap\.filter\(\(binding\) => binding\.run !== gotoLine\)/.test(findCode),
      'searchKeymap is installed whole except CodeMirror\'s own gotoLine, and the one exception ' +
        'is filtered by command identity rather than by key string — a `key === \'Mod-Alt-g\'` ' +
        'test would stop matching silently the day upstream re-spells the chord',
    )
    ok(
      /'Next match \(F3, Enter\)'/.test(findCode) && /'Previous match \(Shift\+F3, Shift\+Enter\)'/.test(findCode),
      'the arrow buttons name F3: these are CodeMirror bindings, so they carry no palette chip ' +
        'and the bar is the only surface that can document them',
    )
    ok(
      /tr\.isUserEvent\('select\.search'\)/.test(findCode),
      'the counter recounts on `select.search` — the marker findNext/findPrevious stamp — and ' +
        'not on bare `selectionSet`, which a held arrow key fires thirty times a second',
    )

    /*
     * Undo, which was reachable by key and by nothing else, and absent entirely from the one
     * editable surface whose Accept writes a file.
     *
     * The composed keymap is built from the real packages, so a `defaultKeymap` upgrade that
     * quietly claimed Mod-z would fail here rather than in a bug report.
     */
    const { closeBracketsKeymap } = require('@codemirror/autocomplete')
    const { defaultKeymap, historyKeymap, indentWithTab } = require('@codemirror/commands')
    const { searchKeymap } = require('@codemirror/search')
    const composed = [
      ...closeBracketsKeymap,
      ...defaultKeymap,
      ...historyKeymap,
      ...searchKeymap,
      indentWithTab,
    ]
    for (const chord of ['Mod-z', 'Mod-y', 'Mod-u']) {
      const claiming = composed.filter((b) => b.key === chord)
      eq(claiming.length, 1, `exactly one binding in the composed editor keymap claims ${chord}`)
      ok(
        historyKeymap.includes(claiming[0]),
        `${chord} is history's — anything else claiming it has shadowed undo, which fails as ` +
          'silently as a feature that was never built',
      )
    }
    ok(
      historyKeymap.some((b) => b.linux === 'Ctrl-Shift-z'),
      'redo also answers Ctrl+Shift+Z on Linux, which is the chord half the world reaches for',
    )
    ok(
      historyKeymap.some((b) => b.key === 'Mod-y' && b.mac === 'Mod-Shift-z'),
      'and Ctrl+Y is redo off macOS — the `mac:` property overrides `key:` only there, so the ' +
        'chord the report asked for is already bound and needs nothing added',
    )

    const diffSrc = strip(readFileSync('src/panes/DiffPane.tsx', 'utf8'))
    const sharedAt = diffSrc.indexOf('const shared =')
    ok(sharedAt !== -1, 'DiffPane still builds a shared extension list')
    const sharedLine = diffSrc.slice(sharedAt, diffSrc.indexOf('\n', sharedAt))
    ok(
      /history\(\)/.test(sharedLine) && /keymap\.of\(historyKeymap\)/.test(sharedLine),
      'the diff pane has an undo history: it is the one editable surface whose Accept writes a ' +
        'file, and it shipped with no `history()` at all, so Ctrl+Z there did nothing',
    )

    const menuCode12 = strip(readFileSync('src/editor/codeMenu.tsx', 'utf8'))
    for (const [id, fn] of [['undo', 'undo('], ['redo', 'redo(']]) {
      const block = menuCode12.slice(
        menuCode12.indexOf(`id: '${id}'`),
        menuCode12.indexOf("id: '", menuCode12.indexOf(`id: '${id}'`) + 10),
      )
      ok(block.length > 0, `the ${id} menu item exists`)
      ok(
        block.includes('run:') && block.includes(fn),
        `the ${id} item carries a run that calls the command — undo was keyboard-only, which is ` +
          'a capability nobody who did not already know the chord could find',
      )
      ok(
        block.includes('Depth('),
        `the ${id} item greys itself from ${id}Depth rather than being always enabled — a menu ` +
          'entry that runs and does nothing is the same lie as a chord that does nothing',
      )
    }

    /*
     * The focus handoff, which is the same defect as the find bar's in different clothing: a
     * caret that moved and a keyboard that did not follow it.
     *
     * Driven rather than grepped. The decision is `planReveal`'s, in an import-free module,
     * precisely so it is not four lines inside an effect inside a component that needs a window
     * — which is where all three of this round's bugs were hiding.
     */
    const doc4 = Text.of(['aaaa', 'bbbb', 'cccc', 'dddd'])
    const base = { line: 2, column: 2, endColumn: 3 }
    eq(
      [reveal.planReveal(doc4, base).focus, reveal.planReveal(doc4, base).center],
      [false, false],
      'a plain reveal takes neither the keyboard nor the middle of the viewport — a click on a ' +
        'search result must leave focus in the results list or the ArrowDown walk ends at one hit',
    )
    eq(reveal.planReveal(doc4, { ...base, focus: true }).focus, true, 'a named jump asks for focus')
    eq(reveal.planReveal(doc4, { ...base, align: 'center' }).center, true, 'and for the centre')
    eq(
      reveal.planReveal(doc4, { ...base, focus: undefined, align: undefined }).focus,
      false,
      'an explicitly undefined flag is "leave it alone", not "whatever the last caller passed"',
    )
    eq(
      [reveal.planReveal(doc4, { line: 99, column: 1, endColumn: 2, focus: true }).range.from],
      [15],
      'and the clamp still runs: a line past the end of the file lands on the last one',
    )

    /*
     * The call sites. Both symbol pickers and Go to line accept by unmounting the card whose
     * `<input>` had focus, so `activeElement` falls to `<body>` — the flag is what stops the
     * caret moving to a place the keyboard cannot then be used in.
     */
    const appSrc = strip(readFileSync('src/App.tsx', 'utf8'))
    const symbolAt = appSrc.indexOf('goToSymbol:')
    ok(symbolAt !== -1, 'App still wires goToSymbol')
    ok(
      /focus: true/.test(appSrc.slice(symbolAt, symbolAt + 400)),
      'goToSymbol asks for focus: Ctrl+F12 → ⏎ used to move the caret and leave the keyboard on ' +
        '<body>, so the next arrow key went nowhere',
    )
    const lineAt = appSrc.indexOf('goToLine:')
    ok(lineAt !== -1, 'App wires goToLine')
    // Bounded at the property's own `},` rather than by a character count: the next action in
    // the object is `openFile`, which *does* call `fileApi.open`, so a fixed window would make
    // the "reveals without opening" assertion below read its neighbour and fail for the wrong
    // reason — or, with the arguments the other way round, pass for one.
    const lineBlock = appSrc.slice(lineAt, appSrc.indexOf('},', lineAt))
    ok(lineBlock.length > 40, 'and the goToLine block was bounded rather than collapsing to nothing')
    ok(
      /focus: true/.test(lineBlock) && /align: 'center'/.test(lineBlock),
      'and so does Go to line, centred — a jump to line 4000 that lands flush against the bottom ' +
        'edge shows the destination with none of the code around it',
    )
    ok(
      /jumpTo\(/.test(lineBlock) && !/fileApi\.open\(/.test(lineBlock),
      'Go to line reveals without opening: the popup will not open without a caret, so the file ' +
        'is by construction the focused editor\'s and its tab is already active',
    )
    ok(
      /case 'navigate\.line'/.test(strip(readFileSync('src/keys/dispatch.ts', 'utf8'))),
      'and Ctrl+G reaches a dispatch arm — a registered command with no case is a palette row ' +
        'that swallows the keystroke and does nothing',
    )
  }


  // ---------------------------------------------------------------------------------------
  // 13. Position memory and navigation history: one type, two stores
  // ---------------------------------------------------------------------------------------

  /*
   * Two features that both have to say where somebody is in a file. They share `FilePosition`
   * and nothing else, and the whole of what either of them *decides* is in these two
   * import-free modules — which is the point: the alternative was a rule inside a `useEffect`
   * in a component that needs a window, and this project has now paid for that three times.
   */
  {
    const pos = load('position.js')
    const nav = load('navHistory.js')

    /* ------------------------------------------------ the shared type and its clamp */

    const view = (path, line, column, topLine) => ({ path, line, column, topLine })

    eq(
      pos.clampView(view('/a.rs', 400, 9, 380), 120),
      view('/a.rs', 120, 9, 120),
      'a position recorded against a longer version of the file lands on the last line — ' +
        '`doc.line(n)` THROWS past the end, and that exception escapes through `dispatch` into ' +
        'an effect with no error boundary, taking the React root and every terminal with it',
    )
    eq(
      pos.clampView(view('/a.rs', 0, 0, 0), 120),
      view('/a.rs', 1, 1, 1),
      'and zero — which is what a 0-based caller would send — clamps up rather than down',
    )
    eq(
      pos.clampView(view('/a.rs', NaN, NaN, NaN), 120),
      view('/a.rs', 1, 1, 1),
      'NaN is clamped rather than passed: it fails every comparison a clamp is made of, so it ' +
        'sails through one written the obvious way and throws at the end of it',
    )
    eq(
      pos.clampView(view('/a.rs', 12, 4, 7), 0),
      view('/a.rs', 1, 4, 1),
      'a document of no lines is one empty line — CodeMirror\'s own arithmetic',
    )
    eq(
      pos.clampView(view('/a.rs', 12.7, 4.2, 7.9), 120),
      view('/a.rs', 12, 4, 7),
      'and fractions are truncated rather than rounded into a line the user never named',
    )
    eq(
      pos.clampView(view('/a.rs', 12, 4, 400), 120).topLine,
      120,
      'the top line is clamped too, not only the caret — restoring a viewport past the end of ' +
        'a shortened file is the same throw',
    )

    ok(
      pos.samePosition({ path: '/a', line: 1, column: 1 }, { path: '/a', line: 1, column: 1 }),
      'two spellings of one place are one place',
    )
    ok(
      !pos.samePosition({ path: '/a', line: 1, column: 1 }, { path: '/a', line: 1, column: 2 }),
      'and the column is part of it',
    )
    ok(pos.samePosition(null, null), 'nowhere is nowhere')
    ok(!pos.samePosition(null, { path: '/a', line: 1, column: 1 }), 'and nowhere is not somewhere')

    /* The rung of the write ladder that costs nothing: a report that is not news. */
    ok(
      !pos.worthNoting(view('/a', 4, 2, 1), view('/a', 4, 2, 1)),
      'an unchanged view is not worth an IPC call — a click that lands the caret where it ' +
        'already was, a scroll that ends where it started, a focus change that moves nothing',
    )
    ok(pos.worthNoting(null, view('/a', 4, 2, 1)), 'the first observation always is')
    ok(
      pos.worthNoting(view('/a', 4, 2, 1), view('/a', 4, 2, 2)),
      'and a scroll with the caret still is: `topLine` is half of what "the same lines" means',
    )

    /*
     * The top-line correction — a **measured** bug, twice over, in both directions.
     *
     * Seeding `positions.json` with `topLine: 200`, launching the real app and reading the value
     * back gave 199: `scrollIntoView`'s default 5px `yMargin` put the restored line just below
     * the viewport top, so the hit test landed on the line above, and the launch after that
     * would have given 198. `yMargin: 0` fixed that and a strict "entirely visible" rule then
     * over-corrected to 201, because the instrumented geometry says the line's top is at 66 and
     * the scroller's *border box* starts at 68. Half a line is the threshold that is stable
     * under both.
     */
    const ROW = 19
    eq(
      pos.topVisibleLine(200, 66, ROW, 68, 4000),
      200,
      'two pixels under a border is still the line you are reading — a stricter rule sent a ' +
        'seeded 200 back as 201, measured against the real app',
    )
    eq(
      pos.topVisibleLine(199, 100 - ROW + 4, ROW, 100, 4000),
      200,
      'but a line cut off by more than half of itself is not: the next one is the top line, ' +
        'which is what stops a restore creeping a line per relaunch',
    )
    eq(
      pos.topVisibleLine(200, 105, ROW, 105, 4000),
      200,
      'a line flush with the viewport top is the answer, unchanged',
    )
    eq(
      pos.topVisibleLine(200, 106, ROW, 105, 4000),
      200,
      'and a line starting BELOW the top is never corrected upward',
    )
    eq(
      pos.topVisibleLine(4000, 0, ROW, 1000, 4000),
      4000,
      'the correction never walks past the end of the document',
    )
    eq(
      pos.topVisibleLine(7, 0, 0, 1000, 4000),
      7,
      'a zero line height is a pane that has not been laid out — the hit line stands rather ' +
        'than the arithmetic dividing by nothing and walking off the end',
    )
    eq(
      pos.topVisibleLine(12, 100, 76, 130, 4000),
      12,
      'and a WRAPPED line is measured against its own height: four rows cut by 30px is still ' +
        'mostly on screen, so it is still the line you are reading',
    )

    /*
     * Idempotence, which is the property that actually matters: restore → read back → restore
     * must be a fixed point, or the viewport creeps every time the file is reopened.
     */
    for (const cut of [0, 0.5, 2, 4, 9]) {
      eq(
        pos.topVisibleLine(300, 68 - cut, ROW, 68, 4000),
        300,
        `a restore leaving ${cut}px of the line above the fold reads back as the same line`,
      )
    }

    /* ------------------------------------------------ an explicit navigation outranks memory */

    eq(
      pos.planRestore(view('/a.rs', 40, 3, 30), 120, false),
      view('/a.rs', 40, 3, 30),
      'with nothing parked, the remembered place is restored',
    )
    eq(
      pos.planRestore(view('/a.rs', 40, 3, 30), 120, true),
      null,
      'a parked reveal VETOES the restore: Go to definition into a file you had scrolled must ' +
        'land on the definition, not where you were last week. Today the ordering also falls ' +
        'out of the restore running before `registerReveal` — which is correctness that lasts ' +
        'until somebody swaps two statements, so it is a rule here as well',
    )
    eq(pos.planRestore(null, 120, false), null, 'and a file with no memory restores nothing')
    eq(
      pos.planRestore(view('/a.rs', 999, 3, 999), 10, false),
      view('/a.rs', 10, 3, 10),
      'the restore is clamped on the way out, not trusted from the store',
    )

    /* ------------------------------------------------ the history: what Back actually walks */

    const at = (path, line) => ({ path, line, column: 1 })
    const A1 = at('/a.rs', 10)
    const A2 = at('/a.rs', 500)
    const B1 = at('/b.rs', 20)
    const C1 = at('/c.rs', 30)

    eq(nav.EMPTY_HISTORY.index, -1, 'an empty history stands nowhere')
    ok(!nav.canWalk(nav.EMPTY_HISTORY, 'back'), 'and cannot go back')
    ok(!nav.canWalk(nav.EMPTY_HISTORY, 'forward'), 'or forward')

    const one = nav.record(nav.EMPTY_HISTORY, A1, B1)
    eq(
      one.entries.map((e) => `${e.path}:${e.line}`),
      ['/a.rs:10', '/b.rs:20'],
      'the FIRST jump records the origin as well as the destination — without it Back from ' +
        'the very first jump has nowhere to go, which is the state a user is in when they ' +
        'first reach for the button',
    )
    eq(one.index, 1, 'and the cursor stands on the destination')

    const back = nav.walk(one, 'back', null)
    eq(back.to.path, '/a.rs', 'Back returns to the origin')
    eq(back.history.index, 0, 'and the cursor moves rather than the list')
    eq(
      back.history.entries.length,
      2,
      'walking does NOT push: a Back that recorded itself could never reach the entry before ' +
        'last',
    )
    eq(nav.walk(back.history, 'back', null), null, 'and there is nothing before the origin')
    eq(nav.walk(back.history, 'forward', null).to.path, '/b.rs', 'Forward comes back')

    /* The live caret refreshes the entry being left — that is what makes a round trip honest. */
    {
      const moved = { path: '/b.rs', line: 640, column: 12 }
      const walked = nav.walk(one, 'back', moved)
      eq(
        walked.history.entries[1],
        moved,
        'the entry being left is refreshed from the LIVE caret, so Back-then-Forward returns ' +
          'you to where you actually were and not to where you landed ten minutes ago',
      )
      eq(
        nav.walk(one, 'back', { path: '/z.rs', line: 5, column: 1 }).history.entries[1].line,
        20,
        'but only when the caret is still in that file — a caret elsewhere means the user ' +
          'changed tabs by hand, and writing it in would corrupt the stack several presses later',
      )
    }

    /* The merge rule: why Back does not walk five entries to get out of one function. */
    {
      const merged = nav.record(nav.record(nav.EMPTY_HISTORY, A1, B1), B1, at('/b.rs', 22))
      eq(
        merged.entries.map((e) => `${e.path}:${e.line}`),
        ['/a.rs:10', '/b.rs:22'],
        `a jump of ${2} lines inside one file replaces the entry rather than adding one — ` +
          'MERGE_LINES is 3, about a signature and its first statement',
      )
      const kept = nav.record(nav.record(nav.EMPTY_HISTORY, A1, B1), B1, at('/b.rs', 200))
      eq(kept.entries.length, 3, 'and a real move inside the same file is still an entry')
      eq(nav.MERGE_LINES, 3, 'the merge distance is what the comment says it is')
    }

    /* The forward tail, discarded exactly as a browser discards it. */
    {
      const two = nav.record(nav.record(nav.EMPTY_HISTORY, A1, B1), B1, C1)
      eq(two.entries.length, 3, 'three places visited')
      const stepped = nav.walk(two, 'back', null)
      eq(stepped.history.index, 1, 'standing on the middle one')
      const fresh = nav.record(stepped.history, B1, at('/d.rs', 1))
      eq(
        fresh.entries.map((e) => e.path),
        ['/a.rs', '/b.rs', '/d.rs'],
        'jumping somewhere new from the middle discards what was ahead — keeping it would ' +
          'make Forward go somewhere you have never been from here',
      )
      ok(!nav.canWalk(fresh, 'forward'), 'so there is nothing ahead any more')
    }

    /* `UNKNOWN_LINE`: the Explorer names a file, not a place in it. */
    {
      eq(nav.UNKNOWN_LINE, 0, 'a line of 0 is out of range by construction — lines are 1-based')
      const opened = nav.record(nav.EMPTY_HISTORY, A1, { path: '/b.rs', line: 0, column: 1 })
      eq(opened.entries.length, 2, 'an open with no position is still a visit')
      const later = nav.record(opened, { path: '/b.rs', line: 700, column: 4 }, C1)
      eq(
        later.entries.map((e) => `${e.path}:${e.line}`),
        ['/a.rs:10', '/b.rs:700', '/c.rs:30'],
        '"somewhere in this file" matches ANY place in that file, so the first live caret ' +
          'replaces it in place — otherwise the Explorer would leave two entries for one visit',
      )
    }

    /* The cap, and the cursor that has to move with it. */
    {
      let h = nav.EMPTY_HISTORY
      for (let i = 0; i < 200; i++) h = nav.record(h, null, at('/f.rs', i * 100))
      eq(h.entries.length, nav.NAV_CAP, `the list is held at ${nav.NAV_CAP}`)
      eq(h.index, nav.NAV_CAP - 1, 'and the cursor still stands on the newest entry')
      eq(
        h.entries[0].line,
        (200 - nav.NAV_CAP) * 100,
        'the OLDEST entries go, not the newest — evicting from the wrong end would make Back ' +
          'walk into places the user has already come back from',
      )
      ok(nav.isCoherent(h), 'and the result is still walkable')
    }

    /*
     * A fuzz walk. The one failure mode of an index into an array is an index out of it, and
     * the symptom would be Back silently doing nothing for ever, with no error anywhere.
     */
    {
      let seed = 12345
      const rand = (n) => {
        seed = (seed * 1103515245 + 12345) & 0x7fffffff
        return seed % n
      }
      let h = nav.EMPTY_HISTORY
      let broke = null
      for (let i = 0; i < 4000 && broke === null; i++) {
        const op = rand(4)
        const place = at(`/f${rand(6)}.rs`, rand(400))
        if (op === 0) h = nav.record(h, null, place)
        else if (op === 1) h = nav.record(h, place, at(`/f${rand(6)}.rs`, rand(400)))
        else {
          const stepped = nav.walk(h, op === 2 ? 'back' : 'forward', rand(2) === 0 ? null : place)
          if (stepped !== null) h = stepped.history
        }
        if (!nav.isCoherent(h)) broke = `${i}: ${JSON.stringify(h)}`
      }
      eq(broke, null, 'four thousand random record/walk operations never put the cursor out of ' +
        'bounds or the list over its cap')
    }

    /* ------------------------------------------------ the call sites, which is where the bugs are */

    const stripJs = (src) => src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:])\/\/.*$/gm, '$1')

    /*
     * **The seam.** Eight gestures used to call `requestReveal` directly, and "remember to
     * record the jump too" spread over eight call sites is a rule enforced by memory — the
     * ninth navigation somebody adds next year is the one that forgets. This is the assertion
     * that stops that, and it is deliberately a whitelist of *files* rather than a count.
     */
    {
      const allowed = new Set([
        // The seam itself.
        'src/editor/jump.ts',
        // The module that defines it.
        'src/editor/revealRequest.ts',
        // The member walk, which is deliberately NOT recorded: it is bound to a held key, and
        // ten presses would be ten entries the merge rule cannot collapse. `navHistory.ts`'s
        // header carries the whole list of what is and is not recorded.
        'src/keys/dispatch.ts',
        // Fixtures and the surface that answers a reveal rather than making one.
        'src/editor/EditorSurface.tsx',
      ])
      /*
       * Comments are stripped before the search, and that is not tidiness: `App.tsx` *discusses*
       * `requestReveal` at length in the block explaining why it no longer calls it, and a
       * whole-file grep would report the explanation as the violation. This is the same lesson
       * section 12 records — a pin written against raw source certifies the prose rather than
       * the code.
       */
      const offenders = execFileSync('grep', ['-rl', 'requestReveal(', 'src'], {
        encoding: 'utf8',
      })
        .split('\n')
        .filter((f) => f !== '' && !allowed.has(f))
        .filter((f) => /requestReveal\(/.test(stripJs(readFileSync(f, 'utf8'))))
      eq(
        offenders,
        [],
        'every navigation goes through `editor/jump.ts` — a `requestReveal` anywhere else is a ' +
          'jump that moves the caret and never reaches the Back stack',
      )
    }

    const jumpSrc = stripJs(readFileSync('src/editor/jump.ts', 'utf8'))
    ok(
      /record\(/.test(jumpSrc) && /walk\(/.test(jumpSrc),
      'jump.ts drives navHistory rather than reimplementing it',
    )
    ok(
      /role\.kind === 'shell'/.test(jumpSrc),
      'and it refuses to record in a non-shell realm: a detached pane is a separate JS realm ' +
        'with its own module instances, and the tab it opens mounts in the shell window',
    )
    ok(
      /if \(to\.line === UNKNOWN_LINE\) return/.test(jumpSrc),
      'an open with no position reveals NOTHING — a `requestReveal` there would land the caret ' +
        'on line 1 and overwrite the view memory\'s restore, turning one feature into a bug in ' +
        'the other',
    )

    const dispatchSrc = stripJs(readFileSync('src/keys/dispatch.ts', 'utf8'))
    for (const id of ['navigate.back', 'navigate.forward']) {
      ok(
        new RegExp(`case '${id}'`).test(dispatchSrc),
        `${id} reaches a dispatch arm — a registered command with no case is a palette row and ` +
          'a mouse button that swallow the gesture and do nothing',
      )
    }
    ok(
      /navigate\(command === 'navigate\.back'/.test(dispatchSrc),
      'and the arm calls into jump.ts rather than deciding anything itself',
    )

    /* The producer, the restore, and the two props that carry them. */
    const surfaceSrc = stripJs(readFileSync('src/editor/EditorSurface.tsx', 'utf8'))
    ok(
      /viewTracker\(path, \(seen\) => \{/.test(surfaceSrc),
      'EditorSurface installs the view tracker — an editor that never reports its position ' +
        'leaves the whole feature storing nothing, and every assertion above still passes',
    )
    ok(
      /observedRef\.current = seen/.test(surfaceSrc),
      'and keeps its own last observation: on a reload from disk that is the ONLY source that ' +
        'is current, since the `at` prop was fetched when the tab opened and the store holds ' +
        'whatever the 500 ms debounce last managed to send',
    )
    ok(
      /planRestore\(remembered, view\.state\.doc\.lines, pendingReveals\(\)\.includes\(path\)\)/.test(
        surfaceSrc,
      ),
      'the restore consults planRestore, and hands it the parked-reveal answer — the veto is ' +
        'the rule that keeps Go to definition outranking a remembered position',
    )
    ok(
      surfaceSrc.indexOf('planRestore(') < surfaceSrc.indexOf('registerReveal('),
      'and it runs BEFORE registerReveal spends a parked request, so an explicit navigation ' +
        'always lands last',
    )
    ok(
      /y: 'start',\n\s*yMargin: 0,/.test(surfaceSrc),
      "the viewport is restored with y: 'start' AND yMargin: 0 — the recorded line goes to the " +
        'very TOP, which is what "the same lines" means. The default 5px margin puts it one ' +
        'line down from where the tracker reads it back, and that error is cumulative: measured ' +
        'against the real app, a seeded 200 came back as 199',
    )
    ok(
      /topVisibleLine\(/.test(stripJs(readFileSync('src/editor/viewTracker.ts', 'utf8'))),
      'and the tracker applies the correction rather than reporting the raw hit test',
    )

    const paneSrc = stripJs(readFileSync('src/panes/EditorPane.tsx', 'utf8'))
    ok(
      /at=\{load\.at\}/.test(paneSrc) && /onView=\{reportPosition\}/.test(paneSrc),
      'EditorPane feeds the surface both halves — an unfed prop is this project\'s most-repeated ' +
        'defect, and it passes every test of the code around it',
    )
    ok(
      /fileApi\.position\(path\)/.test(paneSrc) && /Promise\.all\(/.test(paneSrc),
      'and fetches the position BESIDE the text rather than after it: sequencing them would put ' +
        'the editor on screen at line 1 and then jump it, which reads as the app losing your place',
    )
    ok(
      /useEffect\(\(\) => \(\) => sendPosition\(\), \[sendPosition\]\)/.test(paneSrc),
      'the unmount FLUSHES the pending note rather than cancelling it — tab close, project ' +
        'switch and detach all arrive as an unmount, and every one of them is a moment the user ' +
        'expects to come back to. The selection cleanup beside it cancels, and that asymmetry ' +
        'is the whole point',
    )
    ok(
      /clearTimeout\(selectionTimer\.current\)/.test(paneSrc),
      'while the selection report still cancels: it describes a caret that is no longer on screen',
    )

    /* Write amplification: the note must not travel the workspace-mutation path. */
    const clientSrc = readFileSync('src/ipc/client.ts', 'utf8')
    const noteAt = clientSrc.indexOf('notePosition:')
    ok(noteAt !== -1, 'the client exposes notePosition')
    ok(
      /invoke<void>\('file_note_position'/.test(clientSrc.slice(noteAt, noteAt + 300)),
      'and it is its own command, not a workspace mutation — `WorkspaceState::update` clones the ' +
        'tree twice, re-validates it and broadcasts it to every window, per scroll',
    )
    const rustNote = readFileSync('../crates/cide-app/src/cmd/file.rs', 'utf8')
    const rustAt = rustNote.indexOf('pub fn file_note_position')
    ok(rustAt !== -1, 'and Rust has the handler')
    ok(
      !/state\.update/.test(rustNote.slice(rustAt, rustAt + 400)),
      'which does NOT go through WorkspaceState::update: no rev bump, no `cide://workspace-changed`',
    )
  }

  // ---------------------------------------------------------------------------------------
  // 14. Ctrl+hover and Ctrl+click
  // ---------------------------------------------------------------------------------------

  /*
   * Two gestures that have to be one, and a cost budget that is the whole design.
   *
   * The underline that appears under Ctrl is a *promise* about what the click will do. If they can
   * disagree the feature is worse than neither: an affordance that lies gets ignored, and the
   * gesture it advertises goes unused with it. And a naive `mousemove → invoke` does not merely
   * spend requests — the outbound queue to a language server is `bounded(256)` and drops
   * *notifications* when it is full, so a hover flood desyncs rust-analyzer's copy of the buffer,
   * which is the exact failure `docSync.ts` exists to prevent.
   *
   * So the ladder, the cache, the LRU, the backoff and the modifier-release rule are all pure
   * functions in `codeIntelGate.ts`, driven here — and the wiring is pinned by source assertions,
   * because every one of the bugs in this class has been a *missing call* rather than a wrong
   * function, and no test of the surrounding code can see an absence.
   */
  {
    const gate = load('codeIntelGate.js')
    const strip = (src) =>
      src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:])\/\/.*$/gm, '$1')

    /* --- the one rule both gestures read ------------------------------------------------ */

    eq(gate.intent('definition'), 'jump', 'a reference jumps')
    eq(gate.intent('declaration'), 'usages', 'a declaration lists usages — IDEA’s rule')
    eq(gate.intent('notFound'), 'none', 'a keyword does nothing')
    eq(gate.intent('unavailable'), 'none', 'and neither does a server that could not be asked')

    /*
     * The agreement, asserted as an identity rather than as two lists.
     *
     * `underlines` is *derived* from `intent`, so there is no way to make the underline appear for
     * a kind the click will not act on. A second hand-maintained list of "underlinable" kinds is
     * exactly the drift this is here to make impossible.
     */
    for (const kind of ['definition', 'declaration', 'notFound', 'unavailable']) {
      eq(
        gate.underlines(kind),
        gate.intent(kind) !== 'none',
        `the underline and the click agree about ${kind}`,
      )
    }

    /* --- gate 3: is it even an identifier? ---------------------------------------------- */

    for (const name of ['variableName', 'variableName.function', 'typeName', 'propertyName']) {
      ok(gate.askableToken(name), `${name} is worth asking about`)
    }
    for (const name of ['comment', 'string', 'number', 'keyword', 'punctuation', 'operator']) {
      ok(!gate.askableToken(name), `${name} can never resolve — asking would be pure cost`)
    }
    // Fails **open**, and both cases are real: a buffer past `HIGHLIGHT_LIMIT_BYTES` loads no
    // language at all, and `StreamLanguage` parses lazily. Neither is evidence that the thing
    // under the pointer is punctuation.
    ok(gate.askableToken(null), 'no syntax tree means "we do not know", not "no"')
    // A sub-tag folds onto its base, so a grammar that starts qualifying a name it used to emit
    // bare does not silently change the answer.
    ok(!gate.askableToken('string.special'), 'a qualified reject is still a reject')

    /*
     * Every tag the grammars can emit is classified, one way or the other.
     *
     * This is the assertion that keeps the reject-list honest as the grammars grow: a new token
     * name that is in neither set is a name whose askability nobody decided, and because the list
     * fails open it would start costing a round trip per hover with nothing to say why.
     */
    {
      const sources = ['src/editor/streamGrammar.ts']
      for (const file of readdirSync('src/editor/languages')) {
        if (file.endsWith('.ts')) sources.push(join('src/editor/languages', file))
      }
      const emitted = new Set()
      for (const file of sources) {
        for (const [, tag] of strip(readFileSync(file, 'utf8')).matchAll(/return '([a-zA-Z.]+)'/g)) {
          emitted.add(tag)
        }
      }
      ok(emitted.size >= 15, `read ${emitted.size} token names out of the grammars`)
      const classified = new Set([...gate.REJECTED_TOKENS, ...gate.ASKED_TOKENS])
      const unclassified = [...emitted].filter((tag) => !classified.has(tag)).sort()
      eq(unclassified, [], 'every tag a grammar emits is either asked about or rejected by name')
      // And the two sets are disjoint, or one of them is a claim nothing enforces.
      eq(
        gate.ASKED_TOKENS.filter((tag) => gate.REJECTED_TOKENS.includes(tag)),
        [],
        'no tag is in both lists',
      )
    }

    /* --- gate 1: the pointer is really on a glyph --------------------------------------- */

    const rect = { left: 100, right: 140, top: 50, bottom: 66 }
    ok(gate.onGlyph(120, 58, rect, 8), 'dead centre')
    ok(gate.onGlyph(94, 58, rect, 8), 'one character width of slack to the left')
    ok(!gate.onGlyph(80, 58, rect, 8), 'and not two')
    // The case this gate exists for: `posAtCoords` in precise mode answers with the *line-end*
    // position for a pointer parked in the empty space right of a line, so without the rect check
    // hovering blank space underlines the last word of the line — and the click keeps that promise.
    ok(!gate.onGlyph(400, 58, rect, 8), 'the empty space past the end of a line is not a glyph')
    ok(!gate.onGlyph(120, 20, rect, 8), 'nor is the line above')
    ok(!gate.onGlyph(120, 90, rect, 8), 'nor the one below — the vertical test has no slack')

    /* --- gate 4: the cache -------------------------------------------------------------- */

    /*
     * Keyed on the **word range**, which is the single biggest saving in the ladder: crossing a
     * fourteen-character identifier is one entry rather than fourteen. Keying on `(line, column)`
     * — the obvious choice — multiplies the cache by the identifier's length and misses on every
     * re-entry at a different pixel.
     */
    eq(
      gate.keyFor('/w/a.rs', 3, 100, 106),
      gate.keyFor('/w/a.rs', 3, 100, 106),
      'the same word is the same key',
    )
    ok(
      gate.keyFor('/w/a.rs', 3, 100, 106) !== gate.keyFor('/w/a.rs', 4, 100, 106),
      'an edit changes the key, so every answer for that file becomes unreachable at once',
    )
    ok(
      gate.keyFor('/w/a.rs', 3, 100, 106) !== gate.keyFor('/w/b.rs', 3, 100, 106),
      'and two files never share one',
    )
    ok(
      gate.keyFor('/w/a.rs', 3, 100, 106) !== gate.keyFor('/w/a.rs', 3, 100, 120),
      'and the whole range is in it — two identifiers starting at the same offset in two ' +
        'generations of the buffer are different questions',
    )
    /*
     * The prefix and the key agree, which is a real bug this caught while it was being written:
     * `forgetCodeIntel` drops a file's answers by prefix, and the first version built that prefix
     * by hand with a **space** while `keyFor` separated with a NUL. It matched nothing, so
     * unmounting a buffer forgot nothing — invisible on screen, and invisible to any test of
     * either function on its own.
     */
    ok(
      gate.keyFor('/w/a.rs', 3, 100, 106).startsWith(gate.keyPrefix('/w/a.rs')),
      'every key for a file starts with that file’s prefix — the eviction path depends on it',
    )
    ok(
      !gate.keyFor('/w/a.rs.bak', 3, 100, 106).startsWith(gate.keyPrefix('/w/a.rs')),
      'and a longer path is not swept up by a shorter one’s prefix',
    )

    const answer = (kind, askedMs = gate.HOVER_TIMEOUT_MS, at = 0) => ({
      kind,
      askedMs,
      at,
    })

    {
      const cache = new Map()
      gate.remember(cache, 'a', answer('definition'))
      gate.remember(cache, 'b', answer('declaration'))
      eq(gate.recall(cache, 'a')?.kind, 'definition', 'a remembered answer comes back')
      eq(gate.recall(cache, 'z'), undefined, 'and an unknown key is a miss, not a throw')

      /*
       * Delete-before-set, so `Map` insertion order is a real LRU rather than first-seen order.
       * `recall` re-inserts too — an identifier the user keeps returning to must not be the one
       * evicted, which is exactly what a read-only recall would produce.
       */
      gate.remember(cache, 'c', answer('notFound'))
      // `a` was read most recently of the first two, so `b` is the oldest.
      eq([...cache.keys()], ['b', 'a', 'c'], 'reading an entry makes it the newest')
      // And **re-remembering** one moves it too. Without the delete before the set, `Map` keeps
      // an existing key in its original position, so a hot entry would be evicted before a cold
      // one that has never been touched since it was written.
      gate.remember(cache, 'b', answer('definition'))
      eq([...cache.keys()], ['a', 'c', 'b'], 'and so does writing over one')

      const big = new Map()
      for (let i = 0; i < gate.CACHE_MAX + 50; i++) gate.remember(big, `k${i}`, answer('notFound'))
      eq(big.size, gate.CACHE_MAX, 'the cache is bounded')
      eq(big.has('k0'), false, 'and it is the oldest that goes')
      eq(big.has(`k${gate.CACHE_MAX + 49}`), true, 'the newest survives')
    }

    /* --- the invariant that keeps the hover from over-promising -------------------------- */

    /*
     * **The hover can never underline something the click will not act on; the click may act where
     * the hover stayed quiet.** A hover asks with a 600 ms deadline and a click with five seconds,
     * so a hover's "could not be asked" is *not* an answer for a click — which re-asks properly
     * rather than inheriting a shrug. Wrong in the safe direction, and this is where that is
     * enforced rather than merely intended.
     */
    ok(gate.HOVER_TIMEOUT_MS < gate.CLICK_TIMEOUT_MS, 'the hover is the impatient one')
    for (const kind of ['definition', 'declaration', 'notFound']) {
      ok(
        gate.usable(answer(kind), gate.CLICK_TIMEOUT_MS, 0),
        `a definite ${kind} answers for anybody while it is fresh`,
      )
      // ...but not for ever, and the old comment here said "only an edit can stale it, and that
      // changes the key" — which was true of an edit to the file the pointer is IN and false of
      // an edit to the file the answer POINTS AT. The key carries only the hovering file's
      // generation, so inserting lines above a declaration in another file invalidated nothing
      // and Ctrl+click went on navigating to a line that had moved, with the underline agreeing.
      ok(
        !gate.usable(answer(kind), gate.CLICK_TIMEOUT_MS, gate.DEFINITE_TTL_MS + 1),
        `a definite ${kind} expires — an edit to the file it POINTS AT cannot change this key`,
      )
      ok(
        gate.usable(answer(kind), gate.CLICK_TIMEOUT_MS, gate.DEFINITE_TTL_MS - 1),
        `and it is not re-asked on every hover inside the window (${kind})`,
      )
    }
    ok(
      !gate.usable(answer('unavailable', gate.HOVER_TIMEOUT_MS), gate.CLICK_TIMEOUT_MS, 0),
      'a hover’s shrug is not an answer for a click',
    )
    ok(
      gate.usable(answer('unavailable', gate.CLICK_TIMEOUT_MS), gate.HOVER_TIMEOUT_MS, 0),
      'but a click’s is an answer for a hover',
    )
    /*
     * And the backoff expires. `unavailable` is three situations behind one sentence — no server
     * for this file type, no server running, the server is still indexing — and the third is
     * *expected* for the first minute of a session. Caching it forever means a buffer that never
     * underlines anything again after one early hover.
     */
    ok(
      gate.usable(answer('unavailable', gate.HOVER_TIMEOUT_MS, 1000), gate.HOVER_TIMEOUT_MS, 1500),
      'a fresh shrug suppresses re-asking',
    )
    ok(
      !gate.usable(
        answer('unavailable', gate.HOVER_TIMEOUT_MS, 1000),
        gate.HOVER_TIMEOUT_MS,
        1000 + gate.UNKNOWN_BACKOFF_MS + 1,
      ),
      'and stops suppressing once the server has had time to finish indexing',
    )

    /* --- the ladder, and what a drag actually costs -------------------------------------- */

    /*
     * **The cost claim, measured rather than asserted.**
     *
     * The whole design of this feature is a budget: a naive `mousemove → invoke` is one request
     * per pointer sample, and the failure that produces is not a slow underline — the outbound
     * queue to a language server is `bounded(256)` and *drops notifications* when full, so a
     * dropped `didChange` desyncs rust-analyzer's copy of the buffer permanently.
     *
     * So the ladder is replayed here against a synthetic drag, with a clock, counting the requests
     * it would produce. `hoverPlan` exists as a pure function precisely so this is possible: a
     * budget whose justification lives inside a `mousemove` handler is a budget nothing can check.
     *
     * The drag: 120 pointer samples at 8 ms apart — a second of motion at a compositor's sampling
     * rate — across a line of eight identifiers separated by punctuation, then the pointer stops
     * on the last one.
     */
    {
      const words = []
      // Eight identifiers of six characters, four columns of punctuation between them. `null` is
      // "not on a word", which is what gate 2 and gate 3 answer for the gaps.
      for (let i = 0; i < 8; i++) words.push({ from: i * 10, to: i * 10 + 6 })
      const at = (column) => {
        const word = words.find((w) => column >= w.from && column < w.to)
        return word ?? null
      }

      /*
       * A pointer that has stopped produces **no further `mousemove` events** — which is the whole
       * mechanism, so the replay has to model it that way rather than feeding repeated samples at
       * the same coordinates. `endAt` is when the user stopped looking; the timer matures on its
       * own between the last move and it.
       */
      const replay = (samples, endAt) => {
        const cache = new Map()
        let shown = null
        let timer = null
        let asks = 0
        const steps = { clear: 0, keep: 0, draw: 0, hide: 0, wait: 0 }
        const mature = (t) => {
          if (timer === null || t - timer.t < gate.SETTLE_MS) return
          asks += 1
          gate.remember(cache, gate.keyFor('/w/a.rs', 0, timer.word.from, timer.word.to), {
            kind: 'definition',
            askedMs: gate.HOVER_TIMEOUT_MS,
            at: t,
          })
          shown = timer.word
          timer = null
        }
        for (const { t, column } of samples) {
          mature(t)
          const word = at(column)
          const cached =
            word === null
              ? undefined
              : gate.recall(cache, gate.keyFor('/w/a.rs', 0, word.from, word.to))
          const step = gate.hoverPlan(word, shown, cached)
          steps[step] += 1
          if (step === 'clear' || step === 'hide') {
            shown = null
            timer = null
          } else if (step === 'draw') {
            shown = word
            timer = null
          } else if (step === 'wait') {
            shown = null
            timer = { t, word }
          }
        }
        mature(endAt)
        return { asks, steps }
      }

      // One second of motion at 8 ms per sample, crossing the whole line, and the pointer keeps
      // going — nothing settles.
      const dragging = []
      for (let i = 0; i < 120; i++) dragging.push({ t: i * 8, column: i * 0.62 })
      const drag = replay(dragging, 960)
      eq(
        drag.asks,
        0,
        'a drag across a line of identifiers costs ZERO requests — the settle is a trailing ' +
          'debounce, so continuous motion starves it, which is exactly what is wanted because ' +
          'there is nothing worth showing while the pointer is moving',
      )
      ok(
        drag.steps.wait > 0,
        'and it is not zero because the ladder rejected everything — the timer really is being ' +
          'armed and re-armed',
      )

      // The same drag, and then the pointer stops. One request, when it stops.
      const settled = replay(dragging, 960 + gate.SETTLE_MS)
      eq(settled.asks, 1, 'stopping costs exactly one request')

      /*
       * And going back to a word it has already resolved.
       *
       * The cache is keyed on the **word range**, so re-entering an identifier draws on the frame
       * it is entered, with no request and no settle delay. Keying on `(line, column)` — the
       * obvious choice — would have multiplied the cache by each identifier's length and missed on
       * every re-entry at a different pixel, which is the saving this measures.
       */
      const revisit = [...dragging]
      for (let i = 0; i < 20; i++) revisit.push({ t: 1120 + i * 8, column: 74 - i })
      for (let i = 0; i < 21; i++) revisit.push({ t: 1280 + i * 8, column: 54 + i })
      const again = replay(revisit, 1448 + gate.SETTLE_MS)
      eq(
        again.asks,
        1,
        'a there-and-back drag over six identifiers asks once, where it first stopped — not once ' +
          'per identifier crossed, and not again for the one it returns to',
      )
      ok(
        again.steps.draw > 0,
        'because the return re-draws that one straight out of the cache',
      )
      ok(
        again.steps.keep > 0,
        'and moving WITHIN an identifier that is already underlined does nothing at all — no ' +
          'dispatch, no timer, no request, which is most of what a pointer does',
      )

      /*
       * The pointer resting on a keyword, a comment or punctuation costs nothing at all. Gate 3
       * rejects those before the timer is ever armed, which is why hovering a page of Rust is
       * mostly free: comments, strings, keywords and punctuation are most of what a pointer
       * crosses.
       */
      const overGaps = []
      for (let i = 0; i < 40; i++) overGaps.push({ t: i * 8, column: 6 + (i % 4) })
      eq(
        replay(overGaps, 1000).asks,
        0,
        'a pointer that settles on something that is not an identifier asks nothing',
      )
    }

    /* --- releasing the modifier --------------------------------------------------------- */

    /*
     * **The key identity is checked before the mask, and that is not a style choice.**
     * WebKitGTK reports the modifier state from *before* the release, so on the Ctrl keyup itself
     * `ctrlKey` is still true — a `!ev.ctrlKey` test alone never fires on the platform this ships
     * on, and the underline would stay on screen until the pointer moved. `keys/switcher.ts` pays
     * for the same engine behaviour and writes it out at length.
     */
    ok(
      gate.endsCtrlHold({ key: 'Control', ctrlKey: true, metaKey: false }),
      'the Ctrl keyup ends the hold even though WebKitGTK still reports Ctrl as held',
    )
    ok(
      gate.endsCtrlHold({ key: 'Meta', ctrlKey: false, metaKey: true }),
      'and so does ⌘, which is the macOS spelling of this gesture',
    )
    // The fallback still earns its place: Ctrl+Alt released Alt-first delivers a keyup for `Alt`
    // whose mask no longer carries Ctrl, and that is a genuine end of the hold.
    ok(
      gate.endsCtrlHold({ key: 'Alt', ctrlKey: false, metaKey: false }),
      'a keyup for another key with the modifier already gone ends it too',
    )
    ok(
      !gate.endsCtrlHold({ key: 'Shift', ctrlKey: true, metaKey: false }),
      'but releasing Shift mid-hold does not',
    )
    ok(gate.holdsCtrl({ ctrlKey: false, metaKey: true }), '⌘ counts as the hold')
    ok(!gate.holdsCtrl({ ctrlKey: false, metaKey: false }), 'and nothing else does')

    /* --- the wiring, which is where this class of bug actually lives -------------------- */

    const linkSrc = strip(readFileSync('src/editor/ctrlLink.ts', 'utf8'))
    const intelSrc = strip(readFileSync('src/editor/codeIntel.ts', 'utf8'))

    /*
     * **One resolver.** This is the structural half of the hover/click agreement: if the click
     * asked its own question the two could answer differently for the same word, and no assertion
     * about `intent` would catch it.
     */
    eq(
      (intelSrc.match(/\.probe\(/g) ?? []).length,
      1,
      'exactly one call to the probe command in the whole app, and both gestures go through it',
    )
    for (const file of ['src/editor/ctrlLink.ts', 'src/editor/EditorSurface.tsx', 'src/keys/dispatch.ts']) {
      ok(
        !/\.probe\(/.test(strip(readFileSync(file, 'utf8'))),
        `and ${file} does not reach past it`,
      )
    }
    ok(
      /resolveWord\(/.test(linkSrc) && /resolveWord\(/.test(intelSrc),
      'the hover resolves through the same function the click does',
    )
    ok(
      /underlines\(/.test(linkSrc),
      'and decides whether to draw from `underlines`, which is derived from `intent` — a second ' +
        'list of underlinable kinds is the drift this whole arrangement prevents',
    )
    ok(
      /intent\(answer\.kind\)/.test(intelSrc),
      'while the click switches on `intent` itself',
    )
    /*
     * One ladder, and the click climbs it too.
     *
     * Both gestures resolve the pointer through `targetAtPoint`, which is gates 1 to 3 followed by
     * `wordTargetAt` — so both ask about the **word**, not the pointer's own column, which is what
     * makes them compute the same cache key for the same identifier. A click that used
     * `posAtCoords` directly (as it did before M14) would resolve a different position from the one
     * the underline was drawn for, and the affordance would be wrong at exactly the pixels where
     * gate 1 disagrees with CodeMirror.
     */
    const clickAt = linkSrc.indexOf('mousedown:')
    ok(
      /const word = targetAtPoint\(/.test(linkSrc.slice(clickAt, clickAt + 900)),
      'the click resolves through the same ladder the hover does — so a click on the blank space ' +
        'past the end of a line is refused exactly where the underline was',
    )
    ok(
      (linkSrc.match(/targetAtPoint\(/g) ?? []).length >= 3,
      'and there are two call sites, not two implementations',
    )
    ok(
      /wordTargetAt\(/.test(linkSrc.slice(linkSrc.indexOf('function targetAtPoint'))),
      'with `wordTargetAt` at the bottom of it — the one place a word’s start column is decided',
    )

    /* The five removals, each a bug if missed. */
    ok(/endsCtrlHold\(/.test(linkSrc), 'Ctrl released takes the underline down')
    ok(/mouseleave/.test(linkSrc), 'so does the pointer leaving the text')
    ok(
      /addEventListener\('blur'/.test(linkSrc) && /visibilitychange/.test(linkSrc),
      'so does Alt+Tab — which delivers the keyup to the OTHER application, so nothing else here ' +
        'would ever hear that the hold ended',
    )
    ok(
      /tr\.docChanged/.test(linkSrc),
      'so does an edit, in the StateField — including the agent rewriting the file under the user',
    )
    ok(
      /scrollDOM\.addEventListener\('scroll'/.test(linkSrc),
      'and so does a scroll: the pointer is stationary and the TEXT under it moves, which leaves ' +
        'the underline on the wrong word. Listened for directly, because CodeMirror only routes ' +
        'the scroller it recognises to its observers',
    )
    ok(
      /event\.buttons !== 0/.test(linkSrc),
      'and a drag is suppressed, or selecting text flickers mark spans in and out under the pointer',
    )

    /*
     * The hover must never *report*. `goToDefinition`'s `report()` turns every non-`found` answer
     * into a toast through `unhandledrejection`; on a path that fires whenever the pointer settles
     * that is a toast every time somebody rests the mouse on a comment. This is the single most
     * likely integration mistake in the whole feature.
     */
    ok(
      !/Promise\.reject/.test(linkSrc) && !/notify\(/.test(linkSrc),
      'the hover swallows every outcome silently — no toast, no notice',
    )

    /* Gate 5 is a *trailing* debounce, not a throttle: starvation under motion is the point. */
    {
      const considerAt = linkSrc.indexOf('private consider(')
      ok(considerAt !== -1, 'the hover has a consider step')
      const consider = linkSrc.slice(considerAt, considerAt + 1600)
      ok(
        /this\.clear\(\)\s+const mine = this\.generation\s+this\.settle = setTimeout\(/.test(
          consider,
        ),
        'the settle timer is CLEARED immediately before it is re-armed, on every move — a ' +
          'throttle here would fire once per interval for the whole of a drag, which is the ' +
          'opposite of what is wanted: there is nothing worth showing while the pointer moves',
      )
      ok(/SETTLE_MS/.test(consider), 'and it is the shared constant, not a number written here')
    }
    ok(gate.SETTLE_MS > 0 && gate.SETTLE_MS <= 300, 'and settles fast enough to be aimed with')

    /*
     * The mark has to be styled, or the affordance is invisible and nothing else would notice.
     *
     * The class name is read **out of the source** rather than written here, so a rename on either
     * side fails: a hand-copied literal in this script would keep passing against a stylesheet that
     * no longer matches the decoration.
     */
    {
      const declared = /Decoration\.mark\(\{ class: '([a-z-]+)' \}\)/.exec(linkSrc)
      ok(declared !== null, 'the hover declares its mark class')
      const css = readFileSync('src/editor/EditorSurface.module.css', 'utf8')
      const rule = new RegExp(`\\.body :global\\(\\.${declared?.[1] ?? 'x'}\\)\\s*\\{([^}]*)\\}`)
      const body = rule.exec(css)
      ok(
        body !== null,
        `\`${declared?.[1]}\` is styled beside the token classes — a decoration whose class ` +
          'nobody styles draws nothing at all',
      )
      ok(
        /var\(--[a-z-]+\)/.test(body?.[1] ?? ''),
        'with a token and not a literal, or it survives a theme switch as the wrong colour',
      )
      ok(
        /cursor:\s*pointer/.test(body?.[1] ?? ''),
        'and the pointer becomes a hand over the word — which is the half that tells the user ' +
          'where the click will land',
      )
    }

    /* --- Find usages: the call sites --------------------------------------------------- */

    ok(
      /findUsages\(/.test(strip(readFileSync('src/keys/dispatch.ts', 'utf8'))),
      '⌥F7 reaches the helper — a command id with no dispatch case is listed in the palette and inert',
    )
    {
      const menuCode = strip(readFileSync('src/editor/codeMenu.tsx', 'utf8'))
      const at = menuCode.indexOf("id: 'findUsages'")
      ok(at !== -1, 'the context menu has a Find usages item')
      const next = menuCode.indexOf("id: '", at + 10)
      const item = menuCode.slice(at, next === -1 ? menuCode.length : next)
      ok(
        /run:/.test(item) && /findUsages\(/.test(item),
        'and it carries a `run` that calls the helper, rather than being drawn and dead',
      )
    }
    ok(
      /findUsages\(/.test(intelSrc) && /intent\(answer\.kind\)/.test(intelSrc),
      'and the declaration branch of Ctrl+click goes to the same helper',
    )

    /* One usage jumps, and it jumps through the seam that records the Back stack. */
    ok(
      /rows\.length === 1/.test(intelSrc) && /jumpTo\(/.test(intelSrc),
      'a single usage jumps rather than opening a popup for one row',
    )
    ok(
      !/requestReveal\(/.test(intelSrc),
      'through `jumpTo` and never `requestReveal` — this is the gesture people most want to undo, ' +
        'and the discriminator can send them somewhere they did not ask to go',
    )
    /*
     * Zero usages is a sentence, and it is an `info` — "used nowhere" is a *result*, and
     * `role="alert"` in red for a result is the accessibility equivalent of a modal dialog
     * announcing a success. Scoped to the branch, because the neighbouring "no declaration here"
     * arm also notifies and a whole-file grep would pass on a file where only that one survived.
     */
    {
      const at = intelSrc.indexOf('rows.length === 0')
      ok(at !== -1, 'the empty-result branch exists')
      const branch = intelSrc.slice(at, at + 200)
      ok(
        /notify\(/.test(branch),
        'zero usages is a sentence and not a silent no-op',
      )
      ok(
        /kind: 'info'/.test(branch),
        'and an info notice, not an error',
      )
    }
    /*
     * The grace timer has to be cleared on **both** exits. Clearing it only on success leaves a
     * failed search opening an empty popup 150 ms after the toast that explained the failure.
     */
    eq(
      (intelSrc.match(/clearTimeout\(grace\)/g) ?? []).length,
      2,
      'the grace timer is cleared on the answer path AND on the rejection path',
    )
    ok(/GRACE_MS/.test(intelSrc), 'and the delay is the named constant')
    /*
     * Three generation checks, one per way an answer can arrive late: the timer firing, the
     * answer landing, and the rejection landing. Two of three is a popup that opens over whatever
     * the user did next.
     */
    eq(
      (intelSrc.match(/isCurrentUsages\(/g) ?? []).length,
      3,
      'every path that could act on a stale answer checks the generation first — the timer, the ' +
        'answer and the rejection',
    )

    /* Cancellation, which is two things and only one of them is visible. */
    {
      const storeSrc = strip(readFileSync('src/overlays/usagesStore.ts', 'utf8'))
      ok(
        /usagesCancel\(/.test(storeSrc),
        'dismissing tells the SERVER to stop, not only the popup — without it "Escape cancelled ' +
          'it" and "Escape stopped showing it" are indistinguishable and only the second is true',
      )
      ok(
        /generation: generation \+ 1/.test(storeSrc),
        'and bumps the generation, which is what makes a cancelled search silent rather than a toast',
      )
      ok(
        /useEffect\(\(\) => cancelUsages, \[\]\)/.test(
          strip(readFileSync('src/overlays/UsagesPopup.tsx', 'utf8')),
        ),
        'the popup cancels on unmount — the project closing under a running search has no other ' +
          'call site to put it in',
      )
    }

    /*
     * And the trap the brief names outright: nothing may gate the *request* on a source being
     * Ready. rust-analyzer's indexing is a sequence of progress tokens and "nothing in flight" is
     * true in every gap between them — this repository has already shipped one bug reading such a
     * gap as "indexed, no results", which is what `READY_SETTLE` exists for.
     */
    ok(
      !/kind === 'ready'/.test(intelSrc) && !/Ready/.test(intelSrc),
      'the search is never gated on a readiness flag — the timeout is the readiness answer',
    )
  }

  // ---------------------------------------------------------------------------------------
  // 14. Autosave: every refusal, and the call sites that have to ask
  // ---------------------------------------------------------------------------------------

  /*
   * This feature writes the user's files on a timer. Its refusals are the difference between an
   * IDE and one that silently destroys work, and three of them were named as blocking: never
   * over a pending Claude `openDiff`, never while the conflict bar is up, never a read-only
   * buffer.
   *
   * So the policy is a pure function over a record of facts, driven here as a truth table, and
   * the *mechanism* — two timers, a blur arm, a cleanup — is pinned by source assertions over
   * comment-stripped source. Both halves are needed and neither substitutes for the other: a
   * correct `shouldAutosave` nobody calls is this project's signature defect, and a call site
   * with the rule inlined is a rule nothing can compile.
   */
  {
    const auto = load('autosave.js')
    const strip = (src) => src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:])\/\/.*$/gm, '$1')

    /** Every fact true in the "yes, save" direction. Each case below turns exactly one off. */
    const YES = {
      enabled: true,
      dirty: true,
      readOnly: false,
      conflict: false,
      agentDiff: false,
      windowFocused: true,
      focusInsideEditor: false,
      overlayOpen: false,
      contextMenuOpen: false,
    }
    const may = (reason, over = {}) => auto.shouldAutosave(reason, { ...YES, ...over })

    /* ------------------------------------------------------- the baseline, both reasons */

    eq(may('blur'), true, 'a dirty buffer whose caret left it is written')
    eq(may('idle'), true, 'and so is one nobody has touched for a minute')

    /* ---------------------------------------------- the five unconditional refusals */

    for (const reason of ['blur', 'idle']) {
      eq(may(reason, { enabled: false }), false,
        `${reason}: the setting is off, so nothing is written. It is a `
        + '`ToggleRow` in Settings ▸ Editor and it has to actually do something — five of the '
        + 'seven fields in that section were wired to nothing when this was written')
      eq(may(reason, { dirty: false }), false,
        `${reason}: a clean buffer is not written. Not an optimisation — a save is a `
        + 'didSave, which makes rust-analyzer re-run flycheck over the workspace, so an '
        + 'unguarded blur save would be a cargo check per alt-tab')
      eq(may(reason, { readOnly: true }), false,
        `${reason}: THE BLOCKING ONE — a read-only buffer is never written. External Libraries `
        + 'sources are read-only by design, and writing one corrupts a crate every project on '
        + 'the machine compiles against')
      eq(may(reason, { conflict: true }), false,
        `${reason}: THE BLOCKING ONE — while the conflict bar is up, nothing is written. The `
        + 'bar is a question the user has not answered, and autosave answering it for them, in '
        + 'the direction that discards whatever changed the file, is the worst thing this '
        + 'feature could do')
      eq(may(reason, { agentDiff: true }), false,
        `${reason}: THE BLOCKING ONE — a Claude \`openDiff\` of this file is on screen and the `
        + 'agent turn is blocked on it. The collision is the ordinary gesture: the user clicks '
        + 'the diff tab to look at it, which is a tab switch, which is a blur, which would '
        + 'write the file underneath a proposal computed against the old bytes')
    }

    /* ------------------------------------------- what only a *blur* has to think about */

    eq(may('blur', { windowFocused: false }), true,
      'the OS window was deactivated, so the file is written — IDEA\'s frame-deactivation save, '
      + 'which is most of what people mean by autosave')
    eq(may('blur', { windowFocused: false, focusInsideEditor: true }), true,
      'AND THE ORDER MATTERS: `document.activeElement` does not move when a window is '
      + 'deactivated, so the editor still contains focus. Testing `focusInsideEditor` first '
      + 'would veto the one save this feature is famous for')
    eq(may('blur', { windowFocused: false, overlayOpen: true }), true,
      'and a palette left open does not veto it either — the user has left the application')

    eq(may('blur', { focusInsideEditor: true }), false,
      'the find bar took the caret, which is a CodeMirror panel INSIDE `view.dom`. One test '
      + 'covers it and anything else that ever lives in there')
    eq(may('blur', { overlayOpen: true }), false,
      'Ctrl+P is not leaving the file')
    eq(may('blur', { contextMenuOpen: true }), false,
      'and neither is a right-click')

    eq(may('idle', { focusInsideEditor: true }), true,
      'but an IDLE timer does not care where the caret is: the user stopped typing a minute '
      + 'ago, and where they stopped is not information')
    eq(may('idle', { overlayOpen: true }), true, 'nor whether an overlay happens to be open')
    eq(may('idle', { windowFocused: false }), true, 'nor whether the window is focused')

    /* --------------------------------------------------- the debounce and its ceiling */

    eq(auto.AUTOSAVE_IDLE_MS, 60_000, 'a minute of no edits')
    ok(auto.AUTOSAVE_CEILING_MS > auto.AUTOSAVE_IDLE_MS, 'and a ceiling above it')
    eq(auto.autosaveDelay(0, 0, 0), auto.AUTOSAVE_IDLE_MS, 'a fresh edit waits the idle time')
    eq(
      auto.autosaveDelay(auto.AUTOSAVE_CEILING_MS - 1000, auto.AUTOSAVE_CEILING_MS - 1000, 0),
      1000,
      'SOMEBODY WHO KEEPS TYPING still gets saved. A straight debounce restarts on every '
        + 'keystroke, so twenty minutes of steady typing would never autosave at all — this '
        + 'project has written that starvation down twice already (`docSync`, `gitCountStore`), '
        + 'and the ceiling measured from when the buffer went dirty is the answer',
    )
    eq(auto.autosaveDelay(999_999, 0, 0), 0, 'a delay already past is zero, never negative')
    eq(auto.autosaveDelay(0, Number.NaN, 0), null, 'and a nonsensical timestamp arms nothing')

    /* ------------------------------------------------- the call sites, over stripped source */

    const autoSrc = strip(readFileSync('src/editor/autosave.ts', 'utf8'))
    const surface = strip(readFileSync('src/editor/EditorSurface.tsx', 'utf8'))
    const pane = strip(readFileSync('src/panes/EditorPane.tsx', 'utf8'))
    const sections = strip(readFileSync('src/settings/sections.tsx', 'utf8'))

    ok(!/^\s*import /m.test(autoSrc),
      '`autosave.ts` imports nothing, which is what lets this script compile and drive it — the '
      + 'property `openBuffers.ts` is kept import-free for')

    // The gate is asked, and by the surface rather than re-derived there.
    ok(/config\.allow\(reason, domFacts\(view\)\)/.test(surface),
      'the surface asks the policy before every unasked write, and passes the DOM facts it is '
      + 'the only thing that can read')
    ok(!/conflict|agentDiff/.test(surface),
      'and knows nothing about the conflict bar or a pending agent diff — those are the pane\'s '
      + 'facts, and a copy of them here would be a second answer to "may I write this file"')
    ok(/shouldAutosave\(reason, \{/.test(pane),
      'the pane assembles the facts and hands them to `shouldAutosave` rather than deciding '
      + 'anything itself — every failure mode in this feature is a refusal that was not made, '
      + 'and a refusal inside a `useCallback` is one nothing can compile')

    // The blur arm exists, on the losing edge of CodeMirror's own focus tracking.
    ok(/update\.focusChanged\)?\s*\{[\s\S]{0,400}?autosaveIf\(update\.view, 'blur'\)/.test(surface),
      'the losing edge of `focusChanged` is what triggers a blur save. One hook covers a click '
      + 'into another pane, a tab switch (hidden tabs are `visibility: hidden`, never '
      + 'unmounted) and the OS window being deactivated — a `window.addEventListener("blur")` '
      + 'would double-fire against it and would be a per-pane listener over a window fact')
    ok(/document\.hasFocus\(\)/.test(surface),
      'and it reads `document.hasFocus()`, which is the only thing that separates "the caret '
      + 'moved" from "the window went away"')
    ok(/view\.dom\.contains\(document\.activeElement\)/.test(surface),
      'and whether focus is still inside the editor, which is how the find bar is excluded')

    // The timers, and the cleanup that is the whole defence against the worst bug available.
    ok(/setTimeout\([\s\S]{0,200}?config\.idleMs\)/.test(surface), 'the idle timer is armed')
    ok(/setTimeout\([\s\S]{0,200}?config\.ceilingMs\)/.test(surface), 'and so is the ceiling')
    ok(/if \(dirtyRef\.current\) arm\(update\.view\)/.test(surface),
      'armed from a DOCUMENT change and not from a selection change: "inactive" in a buffer '
      + 'means the text stopped changing, and a person reading a dirty file and scrolling would '
      + 'otherwise never get a save')
    {
      const cleanup = surface.slice(surface.lastIndexOf('return () => {'))
      ok(/disarm\(\)/.test(cleanup),
        'THE MOST IMPORTANT LINE: the build effect\'s cleanup clears both timers. A `reloadKey` '
        + 'bump rebuilds the view, and a destroyed CodeMirror view still answers '
        + '`view.state.doc` — so an orphaned timer would write the pre-reload buffer straight '
        + 'over the file that replaced it, a minute later, with the correct contents already on '
        + 'screen')
    }
    {
      const effect = surface.slice(surface.indexOf('const host = hostRef.current'))
      const deps = /\}, \[path, reloadKey\]\)/.exec(effect)
      ok(deps !== null,
        'the build effect is still keyed on `[path, reloadKey]` and nothing else — a changed '
        + 'identity there tears the view down and takes the user\'s unsaved edits')
      ok(/const autosaveCb = useRef\(autosave\)/.test(surface),
        'which is why `autosave` is held in a ref: the pane rebuilds the object every render, '
        + 'and listing it as a dependency would rebuild the editor on every keystroke')
    }

    // The failure has to produce a sentence.
    ok(/notify\(`cide could not save/.test(pane),
      'A FAILED AUTOSAVE PRODUCES A SENTENCE. `void diag.log(...)` writes to a file the user '
      + 'never opens, and a save that happened on a timer while they were looking at a browser '
      + 'is the one that most needs saying out loud — this is also the population where writes '
      + 'actually fail, because a root-owned mode-644 file reports `writable: true`')
    ok(/cause === 'autosave'/.test(pane),
      'and the report is told apart from a failed Ctrl+S, which the user watched not work')
    ok(/throw error/.test(pane),
      'and the rejection is rethrown either way, so the tab stays dirty and the close guard '
      + 'still puts the buffer in front of the user')

    // The precondition token, which is what stops autosave widening the `sed -i` hole.
    ok(/cause === 'autosave' \? stampRef\.current : null/.test(pane),
      'an autosave carries the file\'s stamp and an explicit Ctrl+S does not. The conflict bar '
      + 'is raised by `cide://session-tool`, which a `cargo fmt` never sends — so before this, '
      + 'switching to a terminal, formatting, and clicking back would silently clobber it')
    ok(/stampRef\.current = stamp/.test(pane),
      'and the token moves with the write, or the second autosave compares against the file as '
      + 'it was when the tab opened and refuses for ever')
    ok(/fileChanged\(error\)/.test(pane) && /setConflict\(true\)/.test(pane),
      'and a refused precondition raises the conflict bar rather than a failure toast — which '
      + 'is what finally gives the `sed -i` path the bar the agent path has had since M12')

    // And the setting is read, not merely offered.
    ok(/settings\.editor\.autosave/.test(pane),
      'THE TOGGLE IS READ. Four toggles and a number in this very settings section persist, '
      + 'survive a relaunch and change nothing — `tabSize`, `insertSpaces`, `showMinimap`, '
      + '`wordWrap` and `trimTrailingWhitespaceOnSave` all have no reader, and no gate in this '
      + 'repository can see that. This one ships with an assertion that it is wired')
    ok(/checked=\{editor\.autosave\}/.test(sections) && /autosave: v/.test(sections),
      'and it is offered in Settings ▸ Editor, in both directions')
  }

  if (failed > 0) {
    console.error(`\n${failed} failure(s) out of ${checked} checks`)
  } else {
    console.log(`editor: ok (${checked} checks)`)
  }
} finally {
  // `process.exit` would skip this, and the build directory lives under `node_modules`, so
  // every failing run would leave one behind. Same reasoning as `check-git-render.mjs`.
  rmSync(out, { recursive: true, force: true })
}

process.exit(failed > 0 ? 1 : 0)
