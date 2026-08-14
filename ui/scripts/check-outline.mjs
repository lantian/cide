/**
 * Checks `src/editor/memberNav.ts` — the caret-to-structure arithmetic behind the breadcrumb,
 * the File Structure popup's preselection, and Alt+Up / Alt+Down.
 *
 * Same shape as `check-problems.mjs` and `check-git-tree.mjs`, and for the same reason: this
 * project has no JS test runner, and adding one for a handful of pure functions would be a larger
 * commitment than the code it tests.
 *
 * # Why these functions are worth pinning
 *
 * They exist because the alternative is an IPC round trip per caret move — thirty a second under
 * a held arrow key. So the outline is fetched once and every subsequent question is answered here,
 * which makes *here* the only place the answers can be wrong. Two of them are decisions rather
 * than derivations, and both would be "simplified" the wrong way by someone reading the code
 * without the reasoning:
 *
 *   - **the trail tests `range`, not `selection`** — with only the name's span, a caret in a
 *     function body is inside nothing, and the breadcrumb would be empty exactly when it matters;
 *   - **the member walk clamps, it does not wrap** — `listKeys.ts` wraps, deliberately, and
 *     copying that here would make Alt+Down at the end of a file jump hundreds of lines upward.
 *
 * The last section pins the structural restatement of the wire types against `ipc/generated.ts`,
 * which is the one drift this module's import-freedom makes possible.
 *
 * Run: `pnpm --dir ui run check:outline`   (or `node ui/scripts/check-outline.mjs`)
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-outline-'))

let failed = 0
const fail = (what, detail) => {
  console.error(`FAIL ${what}${detail === undefined ? '' : `\n  ${detail}`}`)
  failed++
}
const eq = (actual, expected, what) => {
  if (actual !== expected) {
    fail(what, `actual:   ${JSON.stringify(actual)}\n  expected: ${JSON.stringify(expected)}`)
  }
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
   * `memberNav.ts` deliberately imports nothing, so a bare `tsc` with no tsconfig is enough —
   * unlike `check-git-tree.mjs`, which has to synthesise one to resolve `paths`. If this compile
   * ever needs a tsconfig, something has added an import and the node-testability is gone.
   */
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/editor/memberNav.ts',
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

  const { flattenOutline, enclosingTrail, enclosingIndex, memberStep, trailNames } = await import(
    `file://${join(out, 'memberNav.js')}`
  )

  /** A span covering whole lines, which is all these tests need to distinguish. */
  const span = (startLine, endLine, startColumn = 1, endColumn = 80) => ({
    startLine,
    startColumn,
    endLine,
    endColumn,
  })

  const node = (name, range, selection, children = [], kind = 'function') => ({
    kind,
    name,
    container: null,
    range,
    selection,
    children,
  })

  /*
   * ```rust
   *  1  mod net {                       // 1..20
   *  3      impl Display for Server {   // 3..12
   *  4          fn fmt() {              // 4..6
   *  9          fn other() {            // 9..11
   * 15      fn free() {                 // 15..18
   * 25  fn top() {                      // 25..27
   * ```
   */
  const OUTLINE = [
    node(
      'net',
      span(1, 20),
      span(1, 1, 5, 8),
      [
        node(
          'impl Display for Server',
          span(3, 12),
          span(3, 3, 5, 28),
          [
            node('fmt', span(4, 6), span(4, 4, 16, 19)),
            node('other', span(9, 11), span(9, 9, 16, 21)),
          ],
          'impl',
        ),
        node('free', span(15, 18), span(15, 15, 8, 12)),
      ],
      'module',
    ),
    node('top', span(25, 27), span(25, 25, 4, 7)),
  ]

  // --- flattening ------------------------------------------------------------------------

  deep(
    flattenOutline(OUTLINE).map((r) => `${'  '.repeat(r.depth)}${r.node.name}`),
    [
      'net',
      '  impl Display for Server',
      '    fmt',
      '    other',
      '  free',
      'top',
    ],
    'document order, with depth — the shape the popup indents by',
  )
  deep(flattenOutline([]), [], 'an empty outline flattens to nothing, not to a row')

  // --- the trail -------------------------------------------------------------------------

  deep(
    trailNames(OUTLINE, 5, 1),
    ['net', 'impl Display for Server', 'fmt'],
    'a caret inside a method names every container above it, outermost first',
  )
  deep(
    trailNames(OUTLINE, 16, 1),
    ['net', 'free'],
    'and only the containers it is actually inside',
  )
  deep(trailNames(OUTLINE, 26, 1), ['top'], 'a top-level function has no container')
  deep(
    trailNames(OUTLINE, 22, 1),
    [],
    'a caret between declarations is inside nothing, and says so rather than guessing',
  )

  /*
   * The assertion that fails if the trail is ever switched to `selection`. Line 5 is inside
   * `fmt`'s *body*; its name is on line 4. A caret in a body is where a caret usually is, so this
   * is the common case rather than an edge one.
   */
  ok(
    enclosingTrail(OUTLINE, 5, 1).length === 3,
    'the trail tests `range` (the whole declaration), not `selection` (the name)',
  )

  // A caret on the closing brace of a nested item is still inside it.
  deep(trailNames(OUTLINE, 6, 1), ['net', 'impl Display for Server', 'fmt'], 'closing brace')
  // And on the container's own closing brace, only the container.
  deep(trailNames(OUTLINE, 12, 1), ['net', 'impl Display for Server'], 'container closing brace')

  // --- preselection ----------------------------------------------------------------------

  eq(
    enclosingIndex(OUTLINE, 5, 1),
    2,
    'the popup preselects the innermost symbol the caret is in — `fmt`',
  )
  eq(enclosingIndex(OUTLINE, 26, 1), 5, 'and `top` for a caret in it')
  eq(
    enclosingIndex(OUTLINE, 22, 1),
    4,
    'a caret between declarations preselects the last one above it, not the first of the file',
  )
  eq(enclosingIndex(OUTLINE, 1, 1), 0, 'a caret on the first declaration preselects it')
  eq(enclosingIndex([], 1, 1), -1, 'an empty outline preselects nothing')

  // --- the member walk -------------------------------------------------------------------

  eq(
    memberStep(OUTLINE, 1, 5, 'next')?.startLine,
    3,
    'next from *on* `net`’s name is the impl block',
  )
  eq(
    memberStep(OUTLINE, 1, 1, 'next')?.startLine,
    1,
    'and from the start of that line it is `net` itself, whose name begins at column 5 — the ' +
      'comparison is a position, not a line',
  )
  eq(memberStep(OUTLINE, 5, 1, 'next')?.startLine, 9, 'next from inside `fmt` is `other`')
  eq(
    memberStep(OUTLINE, 5, 1, 'next')?.startColumn,
    16,
    'and lands on the name, not on the `pub` keyword — the reason a symbol carries two spans',
  )

  eq(
    memberStep(OUTLINE, 5, 1, 'prev')?.startLine,
    4,
    'prev from inside a body is that body’s own declaration — IDEA’s behaviour',
  )
  eq(
    memberStep(OUTLINE, 4, 16, 'prev')?.startLine,
    3,
    'and only a caret already on the declaration moves past it',
  )

  eq(
    memberStep(OUTLINE, 1, 1, 'prev'),
    null,
    'prev at the first declaration is null — the caller reports it rather than wrapping',
  )
  eq(memberStep(OUTLINE, 26, 1, 'next'), null, 'next past the last declaration is null')
  eq(memberStep([], 1, 1, 'next'), null, 'an empty outline has no next member')
  eq(memberStep([], 1, 1, 'prev'), null, 'nor a previous one')

  /*
   * The decision this whole module could get wrong by copying `listKeys.ts`. A picker list is a
   * ring the user can see all of; a file is not, and wrapping from the last function to the first
   * is a jump of hundreds of lines with no visual continuity.
   */
  ok(
    memberStep(OUTLINE, 26, 1, 'next') === null && memberStep(OUTLINE, 1, 1, 'prev') === null,
    'the member walk clamps at both ends and never wraps',
  )

  // A caret above everything: next is the first member, prev is nothing.
  eq(memberStep(OUTLINE, 0, 1, 'next')?.startLine, 1, 'next from above the file is the first')
  eq(memberStep(OUTLINE, 0, 1, 'prev'), null, 'prev from above the file is nothing')

  // --- the wire restatement ---------------------------------------------------------------

  /*
   * `memberNav.ts` restates `cide_ipc::Symbol` structurally so it can be compiled alone. That is
   * the one drift its import-freedom buys, so it is checked here: every field the module names has
   * to exist on the generated type. Read as text rather than imported, because importing
   * `generated.ts` would need the alias resolution this compile deliberately does without.
   */
  const generated = readFileSync(join(UI, 'src/ipc/generated.ts'), 'utf8')
  const symbolType = generated.slice(generated.indexOf('export type Symbol = '))
  const symbolBody = symbolType.slice(0, symbolType.indexOf('};') + 2)
  for (const field of ['kind', 'name', 'container', 'range', 'selection', 'children']) {
    ok(
      new RegExp(`\\b${field}:`).test(symbolBody),
      `\`Symbol.${field}\` is named by memberNav.ts and must exist on the generated type`,
    )
  }
  const spanType = generated.slice(generated.indexOf('export type SymbolSpan = '))
  const spanBody = spanType.slice(0, spanType.indexOf('};') + 2)
  for (const field of ['startLine', 'startColumn', 'endLine', 'endColumn']) {
    ok(
      new RegExp(`\\b${field}:`).test(spanBody),
      `\`SymbolSpan.${field}\` is named by memberNav.ts and must exist on the generated type`,
    )
  }

  /*
   * The empty outline must be a **shared, frozen constant**, not a fresh literal.
   *
   * This shipped, and the symptom was not subtle: `EditorPane` reads `symbolsOf` through
   * `useSyncExternalStore`, which compares snapshots with `Object.is`. A fresh `[]` per call makes
   * every read look like a change — React re-renders, reads again, gets another new array, and
   * throws "Maximum update depth exceeded". React 19 unmounts the whole tree on an unhandled
   * throw, so the window came up **completely empty**: no panes, no terminals, no chrome. It fires
   * on the first render of any editor, because "no outline yet" is where every buffer starts.
   *
   * A source pin rather than a behavioural one because `outlineStore.ts` imports `client.ts` and
   * cannot be compiled standalone the way the modules above can. Crude, and it catches exactly the
   * edit that would bring this back.
   */
  const store = readFileSync(join(UI, 'src/editor/outlineStore.ts'), 'utf8')
  ok(
    /const NONE: readonly OutlineNode\[\] = Object\.freeze\(\[\]\)/.test(store),
    'outlineStore keeps one shared frozen empty array for the no-outline case',
  )
  ok(
    !/\?\s*\(outline\.symbols as readonly OutlineNode\[\]\)\s*:\s*\[\]/.test(store),
    '`symbolsOf` must not return a fresh `[]` — that is an infinite render loop, not a style choice',
  )

  // The import-freedom itself, which every assertion above depends on.
  /*
   * Document sync, pinned at the source for the same reason as `outlineStore` above: `docSync.ts`
   * imports `client.ts` and cannot be compiled standalone.
   *
   * These are the three invariants that are invisible when broken. A missing `didSave` leaves the
   * panel frozen at the state the project opened in (`cide-lsp`'s
   * `an_on_disk_edit_alone_never_refreshes_diagnostics` is the measurement); an unflushed save
   * makes the server check text from up to 300 ms ago and publish line numbers for a file that no
   * longer exists; and an unbalanced open/close pair sends `didOpen` twice for one URI the first
   * time somebody splits an editor.
   */
  /*
   * Comments stripped before any "calls X" assertion below.
   *
   * Learned twice in this file's history: a commented-out call still matches `/\bfoo\(/`, so a
   * pin written against raw source passes on exactly the code it exists to catch. The `[^:]`
   * guard keeps a `://` in a string from starting a line comment.
   */
  const code = (src) =>
    src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:])\/\/.*$/gm, '$1')

  const sync = code(readFileSync(join(UI, 'src/editor/docSync.ts'), 'utf8'))
  ok(
    /export function savedDoc[\s\S]{0,400}?flush\(path\)[\s\S]{0,200}?didSave/.test(sync),
    'savedDoc flushes the pending change before didSave, so the server checks what was written',
  )
  ok(
    /refs \+= 1/.test(sync) && /refs -= 1/.test(sync) && /if \(doc\.refs > 0\) return/.test(sync),
    'docSync refcounts by path, so two panes over one file are one open document',
  )
  ok(
    /doc\.version \+= 1/.test(sync),
    'every didChange carries a fresh version — a repeated one may be ignored by the server',
  )

  const pane = code(readFileSync(join(UI, 'src/panes/EditorPane.tsx'), 'utf8'))
  for (const [fn, why] of [
    ['openDoc', 'a file that opens is announced to the language server'],
    ['closeDoc', 'and withdrawn when the last pane goes'],
    ['savedDoc', 'a save re-triggers the check — without this the panel never updates'],
    ['scheduleDoc', 'typing reaches the server, which is the whole point of unsaved diagnostics'],
    ['resetDoc', 'a reload from disk reaches it too'],
  ]) {
    ok(new RegExp(`\\b${fn}\\(`).test(pane), `EditorPane calls ${fn} — ${why}`)
  }

  const source = readFileSync(join(UI, 'src/editor/memberNav.ts'), 'utf8')
  ok(
    !/^\s*import\s/m.test(source),
    'memberNav.ts must import nothing, or this script can no longer compile it standalone',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('outline navigation: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
