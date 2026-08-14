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
 * imports the output. Eleven things are pinned, in the order they appear below.
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
 *
 * The fifth found two more quadratics on its first run — the markdown link matcher and the
 * shell `${…}` matcher, both the same unbounded-scan-then-backtrack shape as the YAML key
 * regex that was fixed by hand a commit earlier. Both are fixed in this change.
 *
 * # What this cannot reach, and does not pretend to
 *
 * `EditorSurface.tsx`, `minimap.ts` and `find.ts` are not compiled here. They need a
 * `CanvasRenderingContext2D`, a scroller with real geometry, a composition event and a live
 * `data-theme` switch — a window, in other words. The minimap's *painting*, IME preedit
 * under fcitx5, and a theme toggle with terminals running are out of reach of any headless
 * check and are still untested. `minimapGeometry.ts` exists so that the half of the minimap
 * that is arithmetic is not out of reach too.
 *
 * Run: `pnpm --dir ui run check:editor`
 */
import { execFileSync } from 'node:child_process'
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
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
  const { basename, languageName, loadLanguage } = load('languages.js')
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
  const appSrc = readFileSync('src/App.tsx', 'utf8')
  ok(
    /requestReveal\(/.test(appSrc),
    'App.tsx calls `requestReveal` — without it a search hit opens the file at the top',
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
     * multi-cursor and takes Go to Definition's mouse gesture away with it. There is no runtime
     * check that could notice.
     */
    const surfaceCode = strip(readFileSync('src/editor/EditorSurface.tsx', 'utf8'))
    ok(
      /clickAddsSelectionRange\.of\(\(event\) => event\.altKey\)/.test(surfaceCode),
      'Alt adds carets: without this override CodeMirror puts multi-cursor back on Ctrl+click, ' +
        'which is the chord Go to Definition needs',
    )
    /*
     * Again scoped, and for the same reason: two independent whole-file greps ANDed together
     * would pass on a file that has a `mousedown` handler doing something else and a
     * `goToDefinition` call somewhere unrelated — which is precisely the arrangement this is
     * supposed to detect.
     */
    const mousedownAt = surfaceCode.indexOf('mousedown:')
    ok(mousedownAt !== -1, 'EditorSurface installs a mousedown handler')
    const mousedownBlock = surfaceCode.slice(mousedownAt, mousedownAt + 900)
    ok(
      /ctrlKey/.test(mousedownBlock) && /goToDefinition\(/.test(mousedownBlock),
      'Ctrl+click navigates from inside that handler: on mousedown, because a `click` fires ' +
        'after CodeMirror has already moved the caret',
    )
    ok(
      /\.focus\(\)/.test(mousedownBlock),
      'the handler focuses the editor: returning true skips the CodeMirror path that would ' +
        'otherwise have done it, leaving the keyboard pointed at the previous pane',
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
    first.set(3, 7)
    eq(focusedCaret(), { path: '/a.rs', line: 3, column: 7 }, 'the only claim answers')

    // A split: the newest claim wins, and focus takes it back — the same stack discipline the
    // status readout uses, and for the same reason.
    const second = claimCaret('/b.rs')
    second.set(1, 1)
    eq(focusedCaret().path, '/b.rs', 'mounting claims the slot')
    first.focus()
    eq(focusedCaret().path, '/a.rs', 'focus takes it back')
    // An unfocused editor still tracks its own caret, so it is right the moment it is focused.
    second.set(9, 2)
    eq(focusedCaret().line, 3, 'and a background editor does not overwrite the foreground one')
    second.focus()
    eq(focusedCaret().line, 9, 'but its position was kept')

    second.release()
    eq(focusedCaret().path, '/a.rs', 'releasing hands the slot down rather than blanking it')
    second.set(1, 1)
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
