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
 * imports the output. Five things are pinned, in the order they appear below.
 *
 *  1. **Line endings.** A round trip — detect, hand the text to CodeMirror, restore — is
 *     byte-identical for LF, CRLF and bare CR. This is the regression that already bit.
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
      'src/editor/languages.ts',
      'src/editor/highlight.ts',
      'src/editor/minimapGeometry.ts',
      'src/editor/streamGrammar.ts',
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

  const { countLines, detectLineEnding, restoreLineEndings } = load('lineEndings.js')
  const { basename, languageName, loadLanguage } = load('languages.js')
  const { TOKEN_ROLES, PLAIN_TOKEN, TOKEN_VAR_BY_CLASS, cideHighlightStyle } = load('highlight.js')
  const geo = load('minimapGeometry.js')
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

    const restored = restoreLineEndings(throughCodeMirror(text), detectLineEnding(text))
    if (ending === 'Mixed') {
      // The honest statement of a known loss, asserted so it cannot change unnoticed: a
      // file with more than one ending shape comes back LF throughout, because by save time
      // the buffer has only `\n` left and there is no single answer to put back. Every line
      // that was not already LF is rewritten. Preserving them needs the *sequence* of breaks
      // carried alongside the document, which is a design decision, not a fix.
      eq(restored, throughCodeMirror(text), `mixed endings come back LF-only: ${what}`)
      ok(restored !== text || !/\r/.test(text), `mixed endings are not preserved: ${what}`)
    } else {
      eq(restored, text, `round trip is byte-identical: ${what}`)
    }
  }

  // A 5,000-line CRLF file is the case that motivates all of this: one character typed into
  // it must not turn into 5,000 changed lines in the git panel.
  const big = 'const x = 1;\r\n'.repeat(5000)
  eq(detectLineEnding(big), 'CRLF', 'a large CRLF file')
  eq(countLines(big), 5001, 'a large CRLF file counts its lines')
  eq(restoreLineEndings(throughCodeMirror(big), 'CRLF'), big, 'a large CRLF file round-trips')

  eq(restoreLineEndings('a\nb', 'LF'), 'a\nb', 'restoring LF is the identity')
  eq(restoreLineEndings('a\nb', 'Mixed'), 'a\nb', 'restoring Mixed is the identity')
  eq(restoreLineEndings('a\nb', 'CR'), 'a\rb', 'restoring CR')
  eq(restoreLineEndings('a\nb', 'CRLF'), 'a\r\nb', 'restoring CRLF')

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
