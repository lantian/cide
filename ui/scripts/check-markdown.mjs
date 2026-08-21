/**
 * The markdown preview's parser and its arithmetic, compiled and exercised under node. (M20)
 *
 * `ui/src/editor/markdown/` is a markdown implementation written by hand rather than a dependency
 * (see `types.ts` for why), and a hand-written parser with no test suite is a liability, not a
 * saving. This is the suite. Same shape as `check-editor.mjs` and `check-picker.mjs`: no JS test
 * runner in this project, so the TypeScript already in `node_modules` compiles the modules and
 * this file requires the output.
 *
 * Nine things are pinned, in the order they appear below.
 *
 *  1. **Blocks.** Every form the parser claims to support, from a corpus, with the right 1-based
 *     source line on each — the line is not bookkeeping, it is the entire scroll-sync mechanism.
 *  2. **Inline.** Emphasis flanking, code spans, escapes, links in all four spellings, and the
 *     cases that are supposed *not* to be markup.
 *  3. **Nothing is ever markup.** No parse of an HTML-bearing corpus produces a node that is
 *     anything but text. This is the assertion that stands in for a sanitizer, and it is the
 *     reason there is no sanitizer.
 *  4. **Linearity.** Parse time over an 8× length step, on the four inputs that are quadratic in
 *     a naive implementation. `languages/markdown.ts` already paid for this lesson once and
 *     wrote the measurement down; this is the fence around it.
 *  5. **Scroll sync.** The mapping is monotonic, clamped at both ends, round-trips without
 *     walking, and the latch does not oscillate. None of that is visible in a screenshot and all
 *     of it is what makes a split pane feel broken when it is wrong.
 *  6. **The layout rules.** What a narrow pane does to `split`, and that the corner threshold
 *     still covers the band the pane controls reserve — re-derived from the two CSS tokens,
 *     because the failure is three buttons answering the close button's click.
 *  7. **Fence languages.** Every alias resolves to a real grammar, and tokenizing a fence gives
 *     the same classes the buffer would.
 *  8. **Link and path resolution**, including the `..` that climbs out of the tree.
 *  9. **Source assertions** on the two components, which need a DOM and cannot be run here: that
 *     the buffer is hidden with `visibility` and never `display`, that the switch reserves
 *     `--pane-corner-clear`, that no `href` and no `dangerouslySetInnerHTML` exists anywhere in
 *     the directory, and that `onScrollHandle` never entered `EditorSurface`'s build deps.
 *
 * Run: `pnpm --dir ui run check:markdown`
 */
import { execFileSync } from 'node:child_process'
import { createRequire } from 'node:module'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'

const out = resolve('node_modules/.cache/cide-markdown')
let failed = 0
let checks = 0

const eq = (actual, expected, what) => {
  checks++
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}
const ok = (actual, what) => eq(actual === true, true, what)

mkdirSync(out, { recursive: true })

/*
 * One pass, `--lib es2023` and no DOM.
 *
 * Every module compiled here is import-free apart from its siblings, which is the property that
 * makes this check possible at all — the same argument `check-editor.mjs` makes about
 * `foldRanges.ts` and `position.ts`. `fenceTokens.ts` and `links.ts` are the boundary: the first
 * reaches `@codemirror/language` for a `StringStream` and is compiled anyway, because that import
 * has no DOM in it; `MarkdownPreview.tsx` and `MarkdownFrame.tsx` are React and are asserted as
 * source at the bottom.
 */
execFileSync(
  'node',
  [
    'node_modules/typescript/bin/tsc',
    'src/editor/markdown/types.ts',
    'src/editor/markdown/inline.ts',
    'src/editor/markdown/blocks.ts',
    'src/editor/markdown/scrollSync.ts',
    'src/editor/markdown/view.ts',
    'src/editor/markdown/links.ts',
    'src/editor/markdown/fenceTokens.ts',
    'src/editor/highlight.ts',
    'src/editor/languages.ts',
    '--outDir', out,
    '--rootDir', 'src',
    // CommonJS for the two reasons `check-editor.mjs` gives: the grammars are reached through
    // an extensionless `import()` that only resolves downlevelled to `require`, and the check
    // must get the *same* `@lezer/highlight` instance the compiled output does or every tag
    // lookup would miss.
    '--module', 'commonjs',
    '--moduleResolution', 'node10',
    '--target', 'es2022',
    '--strict',
    '--exactOptionalPropertyTypes',
    '--noUncheckedIndexedAccess',
    '--lib', 'es2023',
    '--skipLibCheck',
  ],
  { stdio: 'inherit' },
)

// `ui/package.json` says `"type": "module"` and node inherits that into `node_modules/.cache`.
writeFileSync(join(out, 'package.json'), '{"type":"commonjs"}\n')

const require = createRequire(import.meta.url)
const load = (name) => require(join(out, 'editor', name))

const { parseMarkdown } = load('markdown/blocks.js')
const { parseInline, normaliseLabel } = load('markdown/inline.js')
const sync = load('markdown/scrollSync.js')
const view = load('markdown/view.js')
const links = load('markdown/links.js')
const fences = load('markdown/fenceTokens.js')
const { tokenClassFor } = load('highlight.js')

/** The blocks of `src`, as `kind:line` pairs — the shape most assertions below are about. */
const shape = (src) => parseMarkdown(src).blocks.map((b) => `${b.kind}:${b.line}`)
const blocks = (src) => parseMarkdown(src).blocks
const first = (src) => blocks(src)[0]

// ---------------------------------------------------------------------------------------
// 1. Blocks
// ---------------------------------------------------------------------------------------

eq(shape('# a\n\npara\n'), ['heading:1', 'paragraph:3'], 'a heading and a paragraph, with lines')
eq(first('### three').level, 3, 'ATX level')
eq(first('###### six').level, 6, 'six is the deepest')
eq(first('####### seven').kind, 'paragraph', 'seven hashes is not a heading')
eq(first('## Title ##').body[0].text, 'Title', 'a closing hash run is decoration')
eq(first('Setext\n======').kind, 'heading', 'setext =')
eq(first('Setext\n======').level, 1, 'setext = is h1')
eq(first('Setext\n------').level, 2, 'setext - is h2')

eq(first('---').kind, 'rule', 'a thematic break')
eq(first('- - -').kind, 'rule', 'a spaced break beats the bullet it also matches')
eq(first('***').kind, 'rule', 'stars')
eq(first('___').kind, 'rule', 'underscores')

eq(first('```rust\nfn f() {}\n```').lang, 'rust', 'the fence info word is the language')
eq(first('```rust\nfn f() {}\n```').text, 'fn f() {}', 'and the body excludes both fences')
eq(first('~~~\nx\n~~~').kind, 'code', 'tilde fences')
eq(first('```\na\n```\nb\n').text, 'a', 'a fence closed by its own marker')
eq(first('```\nunclosed\n').text, 'unclosed', 'a fence that reaches EOF still ends')
eq(first('    indented\n').kind, 'code', 'four spaces is a code block')
eq(first('    indented\n').lang, null, 'indented code has no language')
eq(
  first('```\n# not a heading\n```').text,
  '# not a heading',
  'nothing inside a fence is anything but text',
)

eq(shape('> quoted\n'), ['quote:1'], 'a blockquote')
eq(blocks('> a\n> b\n')[0].body[0].body.map((n) => n.text).join(''), 'a b', 'quote lines join')
eq(blocks('> a\nlazy\n')[0].body[0].body.map((n) => n.text).join(''), 'a lazy', 'lazy continuation')
eq(shape('> a\n\n# after\n'), ['quote:1', 'heading:3'], 'a blank line ends the quote')
eq(blocks('> a\n\n# after\n')[0].body.length, 1, '...and the heading is a sibling, not a child')
eq(blocks('> > deep\n')[0].body[0].kind, 'quote', 'quotes nest')

const list = first('- one\n- two\n')
eq(list.kind, 'list', 'a bullet list')
eq(list.ordered, false, 'not ordered')
eq(list.tight, true, 'no blank lines, so tight')
eq(list.items.length, 2, 'two items')
eq(list.items.map((i) => i.line), [1, 2], 'each item knows its line')
eq(first('- a\n\n- b\n').tight, false, 'a blank line between items makes it loose')
eq(first('- a\n- b\n\n1. c\n').tight, true, 'a blank line before a *different* list does not')
eq(first('1. a\n2. b\n').ordered, true, 'an ordered list')
eq(first('3. a\n').start, 3, 'and it starts where it says')
eq(first('1) a\n').ordered, true, 'the paren spelling')
eq(blocks('- a\n* b\n').length, 2, 'changing the bullet character starts a second list')
eq(first('- outer\n  - inner\n').items[0].body[1].kind, 'list', 'lists nest by indentation')

eq(first('- [ ] todo\n').items[0].checked, false, 'an unticked task item')
eq(first('- [x] done\n').items[0].checked, true, 'a ticked one')
eq(first('- [X] done\n').items[0].checked, true, 'capital X too')
eq(first('- plain\n').items[0].checked, null, 'an ordinary item is not a task')
eq(
  first('- [ ] todo\n').items[0].body[0].body.map((n) => n.text).join(''),
  'todo',
  'the marker is stripped from the text',
)
eq(first('- ![x](y)\n').items[0].checked, null, 'an image is not a tick')

const table = first('| a | b |\n| --- | ---: |\n| 1 | 2 |\n')
eq(table.kind, 'table', 'a GFM table')
eq(table.align, [null, 'right'], 'alignment from the delimiter row')
eq(first('| a | b |\n| :-: | :-- |\n').align, ['center', 'left'], 'the other two')
eq(table.rows.length, 1, 'one body row')
eq(table.rows[0].line, 3, 'which knows its line')
eq(first('a | b\n--- | ---\n').kind, 'table', 'outer pipes are optional')
eq(
  first('one | two\n---\n').kind,
  'heading',
  'a sentence with a pipe under a line of dashes is a setext heading, not a one-column table',
)
eq(first('| a | b |\n| --- | ---: |\n| 1 |\n').rows[0].cells.length, 2, 'a short row is padded')
eq(first('| a |\n| --- |\n| x \\| y |\n').rows[0].cells[0][0].text, 'x | y', 'an escaped pipe')

eq(blocks('[r]: http://x\n').length, 0, 'a lone link definition renders nothing')
eq(
  parseMarkdown('[r]: http://x "T"\n\nsee [r]\n').blocks[0].body[1].href,
  'http://x',
  'a definition before its use',
)
eq(
  parseMarkdown('see [r]\n\n[r]: http://x\n').blocks[0].body[1].href,
  'http://x',
  'and a definition *after* its use, which is why there are two passes',
)
eq(
  parseMarkdown('```\n[r]: http://x\n```\n\n[r]\n').blocks[1].body[0].text,
  '[r]',
  'a definition inside a fence is code, and defines nothing',
)
eq(normaliseLabel('  Foo   Bar '), 'foo bar', 'labels fold case and collapse whitespace')

eq(
  shape('# h\n\npara\n\n- l\n\n> q\n\n```\nc\n```\n\n---\n\n| a |\n| - |\n'),
  ['heading:1', 'paragraph:3', 'list:5', 'quote:7', 'code:9', 'rule:13', 'table:15'],
  'every block form in one document, each on its real source line',
)

// ---------------------------------------------------------------------------------------
// 2. Inline
// ---------------------------------------------------------------------------------------

const NO_REFS = new Map()
const inline = (src) => parseInline(src, NO_REFS)
const kinds = (src) => inline(src).map((n) => n.kind)
const text = (nodes) =>
  nodes
    .map((n) => (n.kind === 'text' || n.kind === 'code' ? n.text : n.body ? text(n.body) : ''))
    .join('')

eq(kinds('plain'), ['text'], 'plain text')
eq(kinds('*a*'), ['em'], 'single stars are emphasis')
eq(kinds('**a**'), ['strong'], 'double stars are strong')
eq(kinds('_a_'), ['em'], 'underscores too')
eq(kinds('~~a~~'), ['del'], 'GFM strikethrough')
eq(kinds('***a***'), ['em'], 'triple nests')
eq(inline('***a***')[0].body[0].kind, 'strong', '...strong inside emphasis')
eq(kinds('snake_case_name'), ['text'], 'an underscore inside a word is not emphasis')
eq(kinds('a*b*c'), ['text', 'em', 'text'], 'a star inside a word is')
eq(kinds('* not emphasis'), ['text'], 'a star followed by a space opens nothing')
eq(kinds('*unclosed'), ['text'], 'an unmatched delimiter is its own characters')
eq(text(inline('*unclosed')), '*unclosed', '...all of them')
eq(kinds('`code`'), ['code'], 'a code span')
eq(inline('`` ` ``')[0].text, '`', 'a double-backtick span holds a backtick')
eq(inline('`*not emphasis*`')[0].text, '*not emphasis*', 'nothing inside a code span is markup')
eq(kinds('`unclosed'), ['text'], 'an unmatched backtick is a backtick')
eq(text(inline('\\*escaped\\*')), '*escaped*', 'backslash escapes')
eq(kinds('\\*escaped\\*'), ['text'], '...and produce no emphasis')

eq(kinds('[a](b)'), ['link'], 'an inline link')
eq(inline('[a](b "T")')[0].title, 'T', 'with a title')
eq(inline('[a](<b c>)')[0].href, 'b c', 'an angle destination may hold a space')
eq(inline('[a](b(c))')[0].href, 'b(c)', 'balanced parens in a destination')
eq(kinds('![a](b)'), ['image'], 'an image')
eq(inline('![a **b**](c)')[0].alt, 'a b', 'alt text is flattened')
eq(kinds('[no such ref]'), ['text'], 'an unresolved shortcut is text')
eq(kinds('[a][no]'), ['text'], 'and so is an unresolved full reference')
const refs = new Map([['r', { href: 'http://x', title: null }]])
eq(parseInline('[r]', refs)[0].kind, 'link', 'a shortcut reference resolves')
eq(parseInline('[text][r]', refs)[0].kind, 'link', 'a full reference resolves')
eq(parseInline('[r][]', refs)[0].kind, 'link', 'a collapsed reference resolves')
eq(text(parseInline('[text][r]', refs)), 'text', '...and keeps its own text')
eq(kinds('[a `]` b](c)'), ['link'], 'a bracket inside a code span does not close the link')
eq(kinds('[**bold**](c)'), ['link'], 'a link with emphasis in it')
eq(inline('[**bold**](c)')[0].body[0].kind, 'strong', '...which is actually parsed')

eq(kinds('<https://x.y>'), ['link'], 'an autolink')
eq(inline('<a@b.co>')[0].href, 'mailto:a@b.co', 'a bare email autolinks to mailto')
eq(kinds('see https://x.y here'), ['text', 'link', 'text'], 'a bare URL')
eq(inline('see https://x.y.')[0 + 1].href, 'https://x.y', 'trailing punctuation is not the URL')
eq(inline('(see https://x.y)')[1].href, 'https://x.y', 'nor is an unbalanced closing paren')
eq(inline('https://en.wikipedia.org/wiki/Ruby_(gem)')[0].href, 'https://en.wikipedia.org/wiki/Ruby_(gem)', 'a balanced one is')
eq(kinds('www.example.com'), ['link'], 'www. linkifies')
eq(kinds('https://'), ['text'], 'a scheme with nothing after it is not a link')

eq(kinds('a  \nb'), ['text', 'break', 'text'], 'two trailing spaces is a hard break')
eq(kinds('a\\\nb'), ['text', 'break', 'text'], 'and so is a trailing backslash')
eq(text(inline('a\nb')), 'a b', 'a soft break is a space')

// ---------------------------------------------------------------------------------------
// 3. Nothing is ever markup
// ---------------------------------------------------------------------------------------

/*
 * The assertion that stands in for a sanitizer, and the reason there is no sanitizer.
 *
 * `types.ts` has no node that can carry markup, so this is really checking that the *parser*
 * never invents one — that an `<img onerror=…>` comes back as the characters somebody typed. If
 * this ever fails, the fix is not to filter the output; it is that a node was added to the type
 * that should not exist.
 */
const HOSTILE = [
  '<script>alert(1)</script>',
  '<img src=x onerror=alert(1)>',
  '<iframe src="https://evil"></iframe>',
  '<b>bold?</b>',
  '<div>\n\nstill text\n\n</div>',
  '<!-- comment -->',
  '<svg><use href="#x"/></svg>',
  '&lt;script&gt;',
  '<a href="javascript:alert(1)">x</a>',
]
const walkInline = (nodes, seen) => {
  for (const node of nodes) {
    seen.add(node.kind)
    if (node.body) walkInline(node.body, seen)
  }
}
const walkBlocks = (list, seen) => {
  for (const block of list) {
    if (block.body && block.kind !== 'quote') walkInline(block.body, seen)
    if (block.kind === 'quote') walkBlocks(block.body, seen)
    if (block.kind === 'list') for (const item of block.items) walkBlocks(item.body, seen)
    if (block.kind === 'table') {
      for (const cell of block.head) walkInline(cell, seen)
      for (const row of block.rows) for (const cell of row.cells) walkInline(cell, seen)
    }
  }
}
const KNOWN = new Set(['text', 'code', 'strong', 'em', 'del', 'link', 'image', 'break'])
for (const source of HOSTILE) {
  const seen = new Set()
  walkBlocks(parseMarkdown(source).blocks, seen)
  eq(
    [...seen].filter((kind) => !KNOWN.has(kind)),
    [],
    `no node kind outside the safe set from ${JSON.stringify(source)}`,
  )
}
eq(
  text(inline('<script>alert(1)</script>')),
  '<script>alert(1)</script>',
  'raw HTML survives as its own characters, which is what React will escape',
)
eq(kinds('<a href="javascript:alert(1)">x</a>'), ['text'], 'a hand-written anchor is not a link')
eq(links.targetKind('javascript:alert(1)'), 'external', 'a javascript: URL is external, never local')
eq(links.targetKind('data:text/html,x'), 'external', 'and so is data:')
eq(links.resolveLocal('/a/b', 'javascript:alert(1)'), null, 'neither resolves to a path')

// ---------------------------------------------------------------------------------------
// 4. Linearity
// ---------------------------------------------------------------------------------------

/*
 * `languages/markdown.ts` records the measurement this exists to prevent: an unbounded link
 * scan took "3.8 s for a 160,000-character line of `[`, against 0.13 s for the same line with
 * this bound". The four inputs below are the ones that are quadratic in a naive parser — an
 * unbounded bracket scan, a delimiter stack over an array instead of a linked list, a per-line
 * table scan, and an unterminated fence.
 *
 * The ratio, not the absolute time: CI machines are slower than this one and a wall-clock
 * threshold is a flaky test. Eight times the input for at most sixteen times the work leaves
 * room for warm-up and GC while still failing anything genuinely quadratic, which at 8× would
 * be 64.
 */
const LINEAR = [
  ['brackets', (n) => '['.repeat(n)],
  ['stars', (n) => '*'.repeat(n)],
  ['emphasis pairs', (n) => '*a* '.repeat(Math.floor(n / 4))],
  ['underscore pairs', (n) => '_a_ '.repeat(Math.floor(n / 4))],
  ['backticks', (n) => '`'.repeat(n)],
  ['table rows', (n) => `| a | b |\n| - | - |\n${'| 1 | 2 |\n'.repeat(Math.floor(n / 10))}`],
  ['list items', (n) => Array.from({ length: Math.floor(n / 10) }, (_, i) => `- ${i}`).join('\n')],
  ['unclosed fence', (n) => '```\n' + 'x\n'.repeat(Math.floor(n / 2))],
  ['prose', (n) => 'the quick brown fox jumps over what a lazy dog. '.repeat(Math.floor(n / 48))],
]
const time = (source) => {
  const start = process.hrtime.bigint()
  parseMarkdown(source)
  return Number(process.hrtime.bigint() - start) / 1e6
}
for (const [name, make] of LINEAR) {
  // Warm, so the first run's JIT does not count as the small input's cost.
  time(make(20_000))
  const small = time(make(20_000))
  const large = time(make(160_000))
  /*
   * `|| large < 5` is not a loosening. Several of these inputs parse the 160,000-character case
   * in under two milliseconds, where the *small* measurement is mostly timer granularity and the
   * ratio is measuring noise — `stars` reported 21x one run and 4x the next, from 0.3ms and
   * 1.4ms. A run that fast is not quadratic; a quadratic one at 160,000 characters is not fast.
   */
  const ratio = large / Math.max(small, 0.05)
  ok(
    ratio < 16 || large < 5,
    `parsing ${name} is linear: 8x the input took ${ratio.toFixed(1)}x the time (${large.toFixed(1)}ms)`,
  )
}

// ---------------------------------------------------------------------------------------
// 5. Scroll sync
// ---------------------------------------------------------------------------------------

const GEO = {
  anchors: [
    { line: 1, top: 0 },
    { line: 10, top: 100 },
    { line: 20, top: 300 },
    { line: 50, top: 900 },
  ],
  contentHeight: 1200,
  viewportHeight: 400,
  docLines: 60,
}
eq(sync.anchorIndexFor(GEO.anchors, 0), -1, 'a line before every anchor')
eq(sync.anchorIndexFor(GEO.anchors, 1), 0, 'exactly the first')
eq(sync.anchorIndexFor(GEO.anchors, 15), 1, 'between two')
eq(sync.anchorIndexFor(GEO.anchors, 999), 3, 'past the last')

eq(sync.previewTopFor(GEO, 1), 0, 'the top of the document is the top of the preview')
eq(sync.previewTopFor(GEO, 10), 100, 'an anchor maps exactly')
eq(sync.previewTopFor(GEO, 15), 200, 'and between anchors it interpolates')
eq(sync.previewTopFor(GEO, 60), 800, 'past the last anchor it is clamped to the scroll limit')
ok(sync.previewTopFor(GEO, 10_000) <= 800, 'a line past the end of the document cannot overscroll')
ok(sync.previewTopFor({ ...GEO, anchors: [] }, 5) === 0, 'a document with no anchors is at zero')
ok(
  sync.previewTopFor({ ...GEO, contentHeight: 100 }, 50) === 0,
  'a preview shorter than its viewport does not scroll at all',
)

let previous = -1
let monotonic = true
for (let line = 1; line <= 60; line++) {
  const top = sync.previewTopFor(GEO, line)
  if (top < previous) monotonic = false
  previous = top
}
ok(monotonic, 'scrolling the buffer down never scrolls the preview up')

eq(sync.sourceLineFor(GEO, 0), 1, 'the inverse at the top')
eq(sync.sourceLineFor(GEO, 100), 10, 'the inverse at an anchor')
eq(sync.sourceLineFor(GEO, 200), 15, 'and between them')
ok(sync.sourceLineFor(GEO, -50) === 1, 'a negative scrollTop clamps')

/*
 * The round trip must not walk. The two functions are not literally inverses — the mapping is
 * many-to-one wherever a block spans several source lines — but line → top → line drifting by
 * more than a line would make a split pane creep every time focus moved between the halves.
 */
let worst = 0
const scrollLimit = GEO.contentHeight - GEO.viewportHeight
for (let line = 1; line <= 60; line++) {
  const top = sync.previewTopFor(GEO, line)
  /*
   * Only the lines that are actually reachable. Everything past the point where the preview hits
   * its scroll limit maps to that one offset — that is what "the document ends" means — so the
   * inverse of the clamped value is the first line that reaches the bottom, for every line after
   * it. Asserting a round trip there would be asserting that the preview can scroll past its own
   * end.
   */
  if (top >= scrollLimit) break
  worst = Math.max(worst, Math.abs(sync.sourceLineFor(GEO, top) - line))
}
ok(worst <= 1, `a reachable line survives a round trip through both mappings (drifted ${worst})`)

const latch = sync.newLatch()
ok(sync.claimDriver(latch, 'editor', 1000), 'an idle latch is claimable')
ok(sync.claimDriver(latch, 'editor', 1010), 'the driver keeps it')
ok(!sync.claimDriver(latch, 'preview', 1010), 'and the follower cannot take it')
ok(
  !sync.claimDriver(latch, 'preview', 1000 + sync.SYNC_HOLD_MS - 1),
  'not even at the end of the hold',
)
ok(
  sync.claimDriver(latch, 'preview', 1010 + sync.SYNC_HOLD_MS),
  'once the driver goes quiet, the other half may take over',
)
sync.releaseDriver(latch)
ok(sync.claimDriver(latch, 'preview', 0), 'release makes it free immediately')

// ---------------------------------------------------------------------------------------
// 6. The layout rules
// ---------------------------------------------------------------------------------------

eq(view.MD_VIEWS, ['text', 'split', 'preview'], 'three layouts, in control order')
eq(Object.keys(view.MD_VIEW_LABELS).sort(), ['preview', 'split', 'text'], 'each has a label')
eq(view.effectiveView('split', 900), { chosen: 'split', layout: 'split', overruled: false }, 'a wide pane splits')
eq(
  view.effectiveView('split', 300),
  { chosen: 'split', layout: 'preview', overruled: true },
  'a narrow pane shows the preview alone, and says the choice was overruled',
)
eq(
  view.effectiveView('text', 300).layout,
  'text',
  'width overrules split and nothing else — a narrow buffer is still a buffer',
)
eq(view.effectiveView('split', 0).layout, 'split', 'an unmeasured pane is not treated as narrow')
eq(view.nextView('text'), 'split', 'the cycle')
eq(view.nextView('preview'), 'text', '...wraps')

eq(view.clampRatio(0.5), 0.5, 'an ordinary ratio')
eq(view.clampRatio(0), view.MIN_RATIO, 'a divider dragged off the left edge')
eq(view.clampRatio(1), view.MAX_RATIO, 'and off the right')
eq(view.clampRatio(Number.NaN), view.DEFAULT_RATIO, 'a pane of zero width gives NaN, not a crash')

/*
 * The switch's corner threshold, re-derived from the two tokens `--pane-corner-clear` is the sum
 * of. `check-rows.mjs` re-derives `--h-findbar` from four declarations for the same reason: a
 * design number copied into JavaScript is a number that silently stops matching, and here the
 * failure is not cosmetic — it is three buttons drawn inside the close button's hit box, which
 * `panes/EditorPane.module.css` records happening once already.
 */
const tokens = readFileSync(join('src', 'styles', 'tokens.css'), 'utf8')
const paneCss = readFileSync(join('src', 'layout', 'PaneTitleBar.module.css'), 'utf8')
const px = (source, name) => {
  const m = new RegExp(`${name}:\\s*(\\d+)px`).exec(source)
  return m === null ? null : Number(m[1])
}
const minimap = px(tokens, '--w-minimap')
const corner = px(paneCss, '--pane-corner')
ok(minimap !== null, '`--w-minimap` is still declared in tokens.css')
ok(corner !== null, '`--pane-corner` is still declared in PaneTitleBar.module.css')
const SWITCH_PX = 3 * 22 + 4
ok(
  view.MD_CONTROL_MIN_PX >= minimap + corner + SWITCH_PX,
  `MD_CONTROL_MIN_PX (${view.MD_CONTROL_MIN_PX}) still clears the pane cluster's reserved band `
    + `(${minimap} + ${corner}) plus the switch itself (${SWITCH_PX})`,
)

/*
 * The preview's size cap is the editor's own, which is the rule README states for
 * `MAX_IMAGE_BYTES`: two caps that can drift produce a state where a file is too big for one half
 * of a feature and small enough for the other — here, a split pane with a blank right-hand side
 * and nothing on screen saying why.
 */
const surface = readFileSync(join('src', 'editor', 'EditorSurface.tsx'), 'utf8')
const limit = /export const HIGHLIGHT_LIMIT_BYTES = ([^\n]+)/.exec(surface)
ok(limit !== null, "EditorSurface still exports HIGHLIGHT_LIMIT_BYTES")
// eslint-disable-next-line no-new-func -- the literal is `1024 * 1024`, read out of our own source.
eq(view.PREVIEW_LIMIT_BYTES, Function(`return ${limit[1]}`)(), 'the preview cap is the editor cap')

// ---------------------------------------------------------------------------------------
// 7. Fence languages
// ---------------------------------------------------------------------------------------

eq(fences.fenceLanguage('rust'), 'rust', 'a spelled-out language name')
eq(fences.fenceLanguage('rs'), 'rust', 'and its extension')
eq(fences.fenceLanguage('RUST'), 'rust', 'case does not matter')
eq(fences.fenceLanguage('ts'), 'typescript', 'typescript')
eq(fences.fenceLanguage('console'), 'shell', 'a terminal transcript reads as shell')
eq(fences.fenceLanguage('cpp'), 'clike', 'C++ shares the C-like table')
eq(fences.fenceLanguage(null), null, 'a fence with no info word')
eq(fences.fenceLanguage(''), null, 'or an empty one')
eq(fences.fenceLanguage('text'), null, '`text` deliberately resolves to no grammar')
eq(fences.fenceLanguage('nosuchlang'), null, 'and so does something nobody has heard of')

/*
 * Every alias resolves. A dead entry here is a fence that silently stops being coloured, which
 * nothing else in this repository would notice.
 */
const aliasSource = readFileSync(join('src', 'editor', 'markdown', 'fenceTokens.ts'), 'utf8')
const aliasBlock = aliasSource.slice(
  aliasSource.indexOf('const FENCE_ALIASES'),
  aliasSource.indexOf('}', aliasSource.indexOf('const FENCE_ALIASES')),
)
const aliases = [...aliasBlock.matchAll(/^\s+'?([\w+-]+)'?:\s*'([\w+]+)',/gm)].map((m) => m[1])
ok(aliases.length >= 10, `the alias table was found and scanned (${aliases.length} entries)`)
eq(
  aliases.filter((word) => fences.fenceLanguage(word) === null),
  [],
  'every fence alias resolves to a grammar the registry actually has',
)

const rust = require(join(out, 'editor/languages/rust.js')).spec
const lines = fences.tokenizeFence(rust, 'fn main() {\n    // hi\n}')
eq(lines.length, 3, 'a fence is tokenized line by line')
eq(lines[0].map((t) => t.text).join(''), 'fn main() {', 'and every character survives')
eq(lines[0][0].cls, tokenClassFor('keyword'), '`fn` gets the class the buffer would give it')
eq(
  lines[1].map((t) => t.cls),
  [null, tokenClassFor('comment')],
  'indentation is its own uncoloured run, and the whole comment after it is one more',
)
const py = require(join(out, 'editor/languages/python.js')).spec
eq(
  fences.tokenizeFence(py, 'x = """\nstill a string\n"""')[1][0].cls,
  tokenClassFor('string'),
  'tokenizer state carries across lines, which is why a fence cannot be done per line',
)
ok(fences.fenceIsHighlightable('short'), 'a small fence is coloured')
ok(!fences.fenceIsHighlightable('x'.repeat(view.FENCE_HIGHLIGHT_LIMIT_BYTES + 1)), 'a huge one is not')

// ---------------------------------------------------------------------------------------
// 8. Links and paths
// ---------------------------------------------------------------------------------------

eq(links.targetKind('https://x.y'), 'external', 'a URL')
eq(links.targetKind('//x.y/z'), 'external', 'protocol-relative')
eq(links.targetKind('#anchor'), 'fragment', 'a fragment')
eq(links.targetKind('./a.md'), 'local', 'a relative path')
eq(links.targetKind('/a/b.md'), 'local', 'an absolute one')

eq(links.dirOf('/a/b/README.md'), '/a/b', 'the directory of a path')
eq(links.dirOf('/README.md'), '/', 'at the root')
eq(links.dirOf('README.md'), '', 'a bare filename has no directory')

eq(links.resolveLocal('/a/b', 'c.md'), '/a/b/c.md', 'a sibling')
eq(links.resolveLocal('/a/b', './c.md'), '/a/b/c.md', 'explicitly a sibling')
eq(links.resolveLocal('/a/b', '../c.md'), '/a/c.md', 'one level up')
eq(links.resolveLocal('/a/b/c', '../../d.md'), '/a/d.md', 'two')
eq(links.resolveLocal('/a/b', '/x/y.md'), '/x/y.md', 'an absolute target ignores the directory')
eq(links.resolveLocal('/a/b', 'c.md#head'), '/a/b/c.md', 'a fragment is not part of the filename')
eq(links.resolveLocal('/a/b', 'c.png?v=2'), '/a/b/c.png', 'nor is a query string')
eq(links.resolveLocal('/a', '../../../etc/passwd'), null, 'a path that climbs out of the tree')
eq(links.resolveLocal('/a/b', ''), null, 'an empty target')
eq(links.resolveLocal('/a/b', 'https://x'), null, 'an external target is not a path')

eq(links.slugify('The state loop'), 'the-state-loop', 'a heading slug')
eq(links.slugify('`code` and *stars*!'), 'code-and-stars', 'punctuation is dropped')
eq(links.slugify('Ünïcödé Wörds'), 'ünïcödé-wörds', 'letters outside ASCII are letters')
const seen = new Map()
eq(links.uniqueSlug('Notes', seen), 'notes', 'the first of a name')
eq(links.uniqueSlug('Notes', seen), 'notes-1', 'the second')
eq(links.uniqueSlug('Notes', seen), 'notes-2', 'the third')
eq(links.uniqueSlug('!!!', new Map()), 'section', 'a heading with no word characters still gets one')

ok(links.looksLikeImage('/a/b.png'), 'png')
ok(links.looksLikeImage('/a/b.SVG'), 'svg, whatever the case')
ok(!links.looksLikeImage('/a/b.md'), 'a markdown file is not an image')

// ---------------------------------------------------------------------------------------
// 9. Source assertions on the parts that need a DOM
// ---------------------------------------------------------------------------------------

const strip = (source) => source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '')
const read = (...parts) => readFileSync(join('src', ...parts), 'utf8')
const frame = strip(read('editor', 'markdown', 'MarkdownFrame.tsx'))
const preview = strip(read('editor', 'markdown', 'MarkdownPreview.tsx'))
const frameCss = strip(read('editor', 'markdown', 'MarkdownFrame.module.css'))
const pane = strip(read('panes', 'EditorPane.tsx'))
const surfaceStripped = strip(surface)

ok(
  /visibility:\s*hidden/.test(frameCss) && !/display:\s*none/.test(frameCss),
  'the hidden buffer uses `visibility`, never `display: none` — a display:none CodeMirror '
    + 'measures zero columns and re-wraps the whole document twice, which is the same trap '
    + '`layout/paneHosts.ts` states for hidden terminal tabs',
)
ok(
  /right:\s*calc\(4px \+ var\(--pane-corner-clear/.test(frameCss),
  'the switch reserves `--pane-corner-clear`, so it cannot be drawn inside the pane cluster\'s '
    + 'live hit box and answer the close button\'s click',
)
ok(
  /-webkit-user-select:\s*text/.test(frameCss) && /[^-]user-select:\s*text/.test(frameCss),
  'the preview is selectable, written both ways — WebKitGTK drops the unprefixed property and '
    + 'the dev server lowers nothing',
)

ok(
  !/dangerouslySetInnerHTML/.test(preview + frame),
  'nothing in the preview sets inner HTML. There is no node that could carry markup, and this '
    + 'is the assertion that keeps it that way',
)
ok(!/\bhref=/.test(preview), 'no `href` is written anywhere in the preview: an `<a href>` in a '
  + 'Tauri webview is a way to navigate the application itself away from its own document, which '
  + 'is the hazard `terminal/xterm.ts` closes for OSC 8 links')
ok(
  /role="link"/.test(preview) && /tabIndex=\{0\}/.test(preview),
  '...so the link is put back in the accessibility tree and the tab order by hand',
)
ok(
  /requestImage\(/.test(preview) && /image\.read|imageApi\s*\n?\s*\.read/.test(read('editor', 'markdown', 'images.ts')),
  'local images go through `image.read`, which is what grants the asset scope one file at a time '
    + 'after the size, type and header refusals have run',
)

ok(
  /whenResizeSettles\(/.test(frame) && !/new ResizeObserver\(\(\) => \{\s*setWidth/.test(frame),
  'the resize observer defers through `layout/resizeGesture.ts` rather than reacting live — '
    + 'reacting per frame of a divider drag is what `check:resize` exists to prevent',
)
ok(
  /beginResizeGesture\(\)/.test(frame) && /endResizeGesture\(\)/.test(frame),
  'the divider drag announces itself as a resize gesture, so xterm fits, session resizes and '
    + 'minimap repaints defer until the user lets go',
)
ok(
  /cancelResizeSettle\(/.test(frame),
  'and cancels its deferred work on unmount, so nothing measures a detached node',
)
ok(
  /style\.flexBasis = /.test(frame) && /setSplitRatio\(next\)/.test(frame),
  'the drag writes the DOM directly and commits once, on pointerup',
)

const buildDeps = /\}, \[path, reloadKey\]\)/.test(surfaceStripped)
ok(buildDeps, "EditorSurface's build effect is still keyed on `[path, reloadKey]` and nothing else")
ok(
  /scrollHandleCb\.current/.test(surfaceStripped) && !/\[path, reloadKey, onScrollHandle\]/.test(surfaceStripped),
  'the scroll handle is held in a ref and never entered the build effect\'s dependency array — '
    + 'widening it costs the undo history, the scroll and any unsaved edits',
)
ok(
  /scrollHandleCb\.current\?\.\(null\)/.test(surfaceStripped),
  'and it is handed back as null in the cleanup, so nothing can scroll a destroyed view',
)

ok(
  /markdownView: mdViewRef\.current/.test(pane),
  'an ordinary position note carries the layout, so a scroll does not reset it to the default',
)
ok(
  /markdownView: next/.test(pane),
  'and choosing a layout notes it immediately rather than waiting for the scroll debounce',
)
ok(
  /setMdView\(at\?\.markdownView \?\? 'text'\)/.test(pane),
  'the layout is seeded from the same `ViewPosition` the scroll position comes from',
)
/*
 * The re-parenting that would cost something is a *layout* change, not the markdown-or-not
 * branch: the second only flips when `path` does, and `EditorSurface` rebuilds on a path change
 * anyway. So what is asserted is that the frame keeps the surface in one element across all
 * three layouts — a `layout === 'split' ? <div>{children}</div> : children` in there would
 * unmount the editor, and with it the undo history and any unsaved edits, on every click of the
 * switch.
 */
ok(/function MaybeMarkdown/.test(pane), 'the markdown branch is a component, so its hooks are its own')
ok(
  /<div\s+ref=\{bufferRef\}[\s\S]{0,600}\{children\}/.test(frame),
  'the frame renders the surface inside one unconditional element in every layout, so switching '
    + 'layout never unmounts the editor',
)
ok(
  !/layout === '(split|preview|text)' \? \(?\s*<div[\s\S]{0,120}\{children\}/.test(frame),
  '...and there is no branch around it that would',
)

console.log(`${checks - failed} of ${checks} markdown checks passed`)
if (failed > 0) {
  console.error(`\n${failed} failure(s) out of ${checks} checks`)
  process.exit(1)
}
