/**
 * The extension host's pure core: the view model, the protocol, and the rule compiler.
 *
 * # What this is defending
 *
 * Three things a type checker cannot see, and each one is a failure that reaches a user as
 * something other than what it is:
 *
 * 1. **A view-model kind with no renderer.** `ExtPanelView` switches on `ViewBody['kind']` and
 *    TypeScript makes that switch exhaustive — but only against the union it can see. `BODY_KINDS`
 *    is the list the *host* validates an incoming message against, and a kind in one and not the
 *    other is either a panel that renders nothing or a message that is dropped as malformed.
 * 2. **A request with no capability.** `REQUIRES` is the whole gate. A request kind with no entry
 *    is a request that needs nothing, which is exactly the hole the capability system exists to
 *    close, and nothing else in the build would notice.
 * 3. **A rule that matches without consuming.** `StreamLanguage` throws *"Stream parser failed to
 *    advance stream"* after ten no-op calls, which does not mis-colour a buffer — it unmounts it.
 *
 * Everything here is compiled standalone with the TypeScript already in `node_modules` and then
 * executed under node. That is only possible because `viewModel.ts` and `protocol.ts` are
 * import-free apart from types, and it is why they are.
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { createRequire } from 'node:module'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(UI, 'node_modules', '.cache', 'cide-ext-'))
const require = createRequire(join(UI, 'package.json'))
let failed = 0
let checked = 0

const eq = (actual, expected, what) => {
  checked += 1
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}
const ok = (cond, what) => eq(cond === true, true, what)

try {
  execFileSync(
    'node',
    [
      join(UI, 'node_modules', 'typescript', 'bin', 'tsc'),
      '--module', 'commonjs',
      '--target', 'es2022',
      '--moduleResolution', 'node',
      '--skipLibCheck',
      '--outDir', out,
      '--rootDir', join(UI, 'src'),
      join(UI, 'src', 'ext', 'viewModel.ts'),
      join(UI, 'src', 'ext', 'protocol.ts'),
    ],
    { cwd: UI, stdio: 'inherit' },
  )

  const view = require(join(out, 'ext', 'viewModel.js'))
  const protocol = require(join(out, 'ext', 'protocol.js'))

  // --- every body kind has a renderer, and every renderer a kind -----------------------------

  const renderer = readFileSync(join(UI, 'src', 'ext', 'ExtPanelView.tsx'), 'utf8')
  const cases = [...renderer.matchAll(/case '([a-z]+)':/g)].map((m) => m[1]).sort()
  eq(
    [...view.BODY_KINDS].sort(),
    cases,
    'every `ViewBody` kind has a `case` in ExtPanelView and every `case` names a kind — the '
      + 'first half is a panel that renders nothing, and the second is a branch nothing can reach',
  )

  const icons = [...renderer.matchAll(/^\s{2}([a-z]+): '.*',$/gm)].map((m) => m[1])
  for (const icon of view.NODE_ICONS) {
    ok(icons.includes(icon), `the \`${icon}\` icon has a glyph — an icon with none draws nothing`)
  }
  for (const tone of view.NODE_TONES) {
    ok(
      tone === 'normal' || renderer.includes(`styles.tone${tone[0].toUpperCase()}${tone.slice(1)}`),
      `the \`${tone}\` tone has a class`,
    )
  }

  // --- the capability gate covers every request ------------------------------------------------

  eq(
    [...protocol.REQUEST_KINDS].sort(),
    Object.keys(protocol.REQUIRES).sort(),
    'every host request names the capability it needs — a request missing from `REQUIRES` is a '
      + 'request that needs nothing, which is the one hole the capability system exists to close',
  )
  for (const kind of protocol.REQUEST_KINDS) {
    ok(
      protocol.CAPABILITIES.includes(protocol.REQUIRES[kind]),
      `\`${kind}\` needs a capability that exists`,
    )
    const why = protocol.refusal(kind).why
    ok(
      why.includes(protocol.REQUIRES[kind]),
      `and its refusal names that capability in the manifest's spelling, so an author can copy `
        + `it straight into \`capabilities\` (${kind})`,
    )
  }

  // The Rust enum is the authority; this is the copy the worker sees.
  const rust = readFileSync(
    resolve(UI, '..', 'crates', 'cide-ipc', 'src', 'ext.rs'),
    'utf8',
  )
  const declared = [...rust.matchAll(/Self::\w+ => "([a-z:]+)",/g)].map((m) => m[1])
  eq(
    [...protocol.CAPABILITIES].sort(),
    [...new Set(declared)].sort(),
    'the capability names the worker is told about are exactly `cide_ipc::ext::Capability`\'s — '
      + 'a name in one and not the other is a permission that can be granted and never checked, '
      + 'or checked and never granted',
  )

  // --- the host validates before it destructures ----------------------------------------------

  ok(protocol.isWorkerNote({ kind: 'view', panel: 'x', view: { body: {} } }), 'a view note passes')
  ok(protocol.isWorkerNote({ kind: 'log', level: 'info', text: 'hi' }), 'a log note passes')
  ok(
    protocol.isWorkerNote({ kind: 'request', id: 1, request: { kind: 'activeText' } }),
    'a request note passes',
  )
  for (const bad of [
    null,
    'view',
    42,
    { kind: 'view' },
    { kind: 'view', panel: 1, view: {} },
    { kind: 'request', id: 'one', request: { kind: 'activeText' } },
    { kind: 'request', id: 1, request: { kind: 'rm -rf' } },
    { kind: 'something-else' },
  ]) {
    ok(
      protocol.isWorkerNote(bad) === false,
      `a worker can post anything; ${JSON.stringify(bad)} is refused before it is destructured`,
    )
  }

  ok(view.isPanelView({ body: { kind: 'empty', message: 'x' } }), 'a panel view passes')
  for (const bad of [
    null,
    {},
    { body: null },
    { body: { kind: 'canvas' } },
    { body: { kind: 'tree' } },
    { body: { kind: 'list' }, actions: 'no' },
  ]) {
    ok(view.isPanelView(bad) === false, `${JSON.stringify(bad)} is not a panel view`)
  }

  // --- the tree walk honours expansion, depth and the cap --------------------------------------

  const tree = [
    { id: 'a', label: 'a', children: [{ id: 'a1', label: 'a1' }] },
    { id: 'b', label: 'b' },
  ]
  eq(
    view.visibleRows(tree, new Set()).rows.map((r) => r.row.id),
    ['a', 'b'],
    'a collapsed parent hides its children',
  )
  eq(
    view.visibleRows(tree, new Set(['a'])).rows.map((r) => r.row.id),
    ['a', 'a1', 'b'],
    'and an expanded one shows them',
  )
  eq(view.initialExpansion([{ id: 'a', label: 'a', expanded: true }]), ['a'], '`expanded` seeds it')

  // A tree deeper than the cap. Built rather than written out, because the point is the cap.
  let deep = { id: 'leaf', label: 'leaf' }
  for (let at = 0; at < view.MAX_DEPTH + 5; at++) {
    deep = { id: `n${at}`, label: `n${at}`, children: [deep] }
  }
  const all = new Set()
  const collect = (row) => {
    all.add(row.id)
    for (const child of row.children ?? []) collect(child)
  }
  collect(deep)
  const walked = view.visibleRows([deep], all)
  ok(
    walked.rows.length <= view.MAX_DEPTH,
    'a tree deeper than MAX_DEPTH stops there — the one JavaScript thread ADR 0001 protects is '
      + 'the one a runaway contributed tree would freeze',
  )
  ok(walked.truncated > 0, 'and says how much it did not draw, because a silent truncation reads '
    + 'as an extension that lost data')

  // --- the rule compiler ------------------------------------------------------------------------
  //
  // Compiled against the real `streamGrammar.ts`, so the hook it produces is the hook the
  // tokenizer will call. A stub `StringStream` would test this file's idea of one.

  execFileSync(
    'node',
    [
      join(UI, 'node_modules', 'typescript', 'bin', 'tsc'),
      '--module', 'commonjs',
      '--target', 'es2022',
      '--moduleResolution', 'node',
      '--skipLibCheck',
      '--outDir', out,
      '--rootDir', join(UI, 'src'),
      join(UI, 'src', 'editor', 'grammarRules.ts'),
    ],
    { cwd: UI, stdio: 'inherit' },
  )
  const rules = require(join(out, 'editor', 'grammarRules.js'))
  const { StringStream } = require(join(UI, 'node_modules', '@codemirror', 'language'))

  const run = (spec, line) => {
    const { hook, problems } = rules.compileRules(spec)
    if (hook === null) return { tag: null, consumed: 0, problems }
    const stream = new StringStream(line, 2, 2, 0)
    const tag = hook(stream)
    return { tag, consumed: stream.pos, problems }
  }

  eq(
    run([{ pattern: 'key(?=:)', tag: 'propertyName', at: 'lineStart' }], 'key: value').tag,
    'propertyName',
    'a line-start rule matches at the head of a line',
  )
  eq(
    run([{ pattern: 'key(?=:)', tag: 'propertyName' }], 'key: value').consumed,
    3,
    'and consumes exactly what it matched — the lookahead is not consumed',
  )
  ok(
    run([{ pattern: '(?=key)', tag: 'propertyName' }], 'key: value').tag === null,
    'a rule that matches without consuming reports no tag — `StreamLanguage` throws "Stream '
      + 'parser failed to advance stream" after ten of those, which unmounts the buffer rather '
      + 'than mis-colouring it',
  )
  eq(
    run([{ pattern: 'value', tag: 'x' }], 'key: value').tag,
    null,
    'an unanchored pattern is anchored, so it cannot scan forward and swallow the tokens before '
      + 'what it was looking for',
  )
  const broken = rules.compileRules([
    { pattern: '[', tag: 'x' },
    { pattern: 'ok', tag: 'meta' },
  ])
  eq(broken.problems.length, 1, 'a pattern that is not a regex is reported')
  ok(
    broken.hook !== null,
    'and the rules beside it still run — an extension whose fourth rule has a typo should colour '
      + 'what its first three describe',
  )

  // --- the manifest's own shape rules, as the panel restates them -------------------------------

  execFileSync(
    'node',
    [
      join(UI, 'node_modules', 'typescript', 'bin', 'tsc'),
      '--module', 'commonjs',
      '--target', 'es2022',
      '--moduleResolution', 'node',
      '--skipLibCheck',
      '--outDir', out,
      '--rootDir', join(UI, 'src'),
      join(UI, 'src', 'sidebar', 'ExtensionsPanel', 'model.ts'),
    ],
    { cwd: UI, stdio: 'inherit' },
  )
  const model = require(join(out, 'sidebar', 'ExtensionsPanel', 'model.js'))

  const prose = [...rust.matchAll(/Self::\w+ => "([a-z ,]+)",/g)].map((m) => m[1])
  for (const [cap, text] of Object.entries(model.CAPABILITY_PROSE)) {
    ok(
      prose.includes(text),
      `the panel describes \`${cap}\` in \`Capability::describe\`'s own words — a consent sheet `
        + 'that worded a permission differently from the permission it grants would be the worst '
        + 'possible drift in this feature',
    )
  }
  eq(
    Object.keys(model.CAPABILITY_PROSE).sort(),
    [...protocol.CAPABILITIES].sort(),
    'and it has a sentence for every capability, or an extension asks for something the install '
      + 'sheet does not mention',
  )
  ok(
    model.consentLine([]).includes('no permissions'),
    'an extension that asks for nothing says so, rather than showing an empty sentence',
  )
  eq(
    Object.keys(model.CAPABILITY_SHORT).sort(),
    [...protocol.CAPABILITIES].sort(),
    "every capability has a two-word tag for the panel's rows as well as a sentence for the page — "
      + 'one without the other is a row that shows a raw `process:spawn` to somebody deciding',
  )
  for (const [cap, short] of Object.entries(model.CAPABILITY_SHORT)) {
    ok(
      short.length <= 18 && !short.includes(':'),
      `\`${cap}\`'s tag is short and is not the capability string — seven of these can appear on `
        + 'one 252px row',
    )
  }
  eq(
    [...protocol.CAPABILITIES].filter((cap) => model.isStrongCapability(cap)),
    ['process:spawn'],
    'and exactly one is toned apart. Every other capability is a read of something already on '
      + 'screen or already in the repository; this one means the manifest names a binary and cide '
      + "runs it against the user's project",
  )
  ok(
    model.consentLine(['fs:read', 'process:spawn']).includes(' and '),
    'and two are joined as prose rather than as a list',
  )

  // --- search and filtering ---------------------------------------------------------------------
  //
  // Driven over `matchesFilter`/`matchesText` directly rather than through six filtered models,
  // which is a great deal more legible and is why those two are exported.

  const catalog = (over) => ({
    marketplace: 'm',
    extension: over.extension ?? 'x',
    name: over.name ?? 'X',
    version: '1.0.0',
    description: over.description ?? '',
    capabilities: [],
    action: over.action,
    enabled: over.enabled ?? false,
    note: over.note,
  })
  const rows = {
    available: catalog({ extension: 'sql', name: 'SQL', action: 'install' }),
    enabled: catalog({ extension: 'yaml', name: 'YAML', action: 'installed', enabled: true }),
    disabled: catalog({ extension: 'toml', name: 'TOML', action: 'installed', enabled: false }),
    update: catalog({ extension: 'hcl', name: 'HCL', action: 'update', enabled: true }),
    broken: catalog({ extension: 'bad', name: 'Bad', action: 'unavailable', note: 'nope' }),
  }
  const table = {
    all: ['available', 'enabled', 'disabled', 'update', 'broken'],
    // "What have I got" is a question about the disk, not about what is running — a disabled or
    // broken extension is still installed.
    installed: ['enabled', 'disabled', 'update', 'broken'],
    enabled: ['enabled', 'update'],
    disabled: ['disabled', 'broken'],
    updates: ['update'],
    available: ['available'],
    problems: ['broken'],
  }
  for (const [filter, expected] of Object.entries(table)) {
    eq(
      Object.entries(rows)
        .filter(([, row]) => model.matchesFilter(row, filter))
        .map(([name]) => name),
      expected,
      `the \`${filter}\` filter shows exactly what its label promises`,
    )
  }
  eq(
    [...model.EXT_FILTERS].sort(),
    Object.keys(table).sort(),
    'and every filter in the chip row is one the table above covers — a filter with no rule is a '
      + 'chip that shows everything and looks like it did nothing',
  )
  eq(
    model.EXT_FILTERS[0],
    'all',
    'the default is first, so the panel opens on the catalog rather than on a view the user has '
      + 'to undo',
  )
  for (const filter of model.EXT_FILTERS) {
    ok(
      typeof model.FILTER_LABEL[filter] === 'string' && model.FILTER_LABEL[filter] !== '',
      `\`${filter}\` has a label`,
    )
  }

  ok(model.matchesText(rows.enabled, ''), 'an empty query matches everything')
  ok(model.matchesText(rows.enabled, '  '), 'and so does whitespace')
  ok(model.matchesText(rows.enabled, 'YAM'), 'the name matches, case-insensitively')
  ok(model.matchesText(rows.enabled, 'yaml'), 'and so does the id')
  ok(
    model.matchesText(catalog({ action: 'install', description: 'Postgres support' }), 'postgres'),
    'and the description, which is where an extension says what it is for',
  )
  ok(!model.matchesText(rows.enabled, 'rust'), 'and a word in none of the three does not')
  ok(
    !model.matchesText(catalog({ action: 'install', name: 'SQL' }), 'sq l'),
    'substring and not subsequence — `cide-core::commands` ranks a large closed set the user is '
      + 'recalling from; this is a handful of rows they are reading, and a subsequence match over '
      + 'four rows mostly produces surprise',
  )

  const market = (entries) => ({
    id: 'm',
    name: 'm',
    source: '/tmp/m',
    authenticated: false,
    state: { kind: 'ready' },
    entries,
    problems: [],
  })
  const entry = (id, over = {}) => ({
    id,
    name: id.toUpperCase(),
    version: '1.0.0',
    description: '',
    capabilities: [],
    updateAvailable: false,
    ...over,
  })
  const withQuery = (query) =>
    model.buildModel(
      [market([entry('sql'), entry('yaml'), entry('toml')])],
      [
        { marketplace: 'm', extension: 'yaml', name: 'YAML', version: '1.0.0', enabled: true, capabilities: [], problems: [] },
        { marketplace: 'm', extension: 'toml', name: 'TOML', version: '1.0.0', enabled: false, capabilities: [], problems: [] },
      ],
      [],
      [],
      query,
    )

  const unfiltered = withQuery({ text: '', filter: 'all' })
  eq(
    unfiltered.counts,
    { all: 3, installed: 2, enabled: 1, disabled: 1, updates: 0, available: 1, problems: 0 },
    'the counts describe the whole registry',
  )
  eq(unfiltered.empty, undefined, 'and an unfiltered panel has no empty sentence')

  const onlyEnabled = withQuery({ text: '', filter: 'enabled' })
  eq(onlyEnabled.groups[0].rows.map((r) => r.extension), ['yaml'], 'a filter narrows the rows')
  eq(
    onlyEnabled.counts,
    unfiltered.counts,
    'and the counts do **not** move with it. A chip reading 0 only because another chip is '
      + 'selected would be a control that lies about what pressing it does',
  )

  eq(
    withQuery({ text: 'tom', filter: 'all' }).groups[0].rows.map((r) => r.extension),
    ['toml'],
    'and so does a search',
  )
  eq(
    withQuery({ text: 'tom', filter: 'enabled' }).groups.length,
    0,
    'the two combine, and a marketplace with nothing left is dropped while filtering — a heading '
      + 'with nothing under it is noise between the rows that matched',
  )
  ok(
    (withQuery({ text: 'nope', filter: 'all' }).empty ?? '').includes('nope'),
    'a search that matches nothing names what was searched for, so the user can see it when the '
      + 'field has scrolled or they have looked away',
  )
  ok(
    (withQuery({ text: 'sql', filter: 'updates' }).empty ?? '').includes('updates'),
    'and names the filter too when both are narrowing, because "no results" under two constraints '
      + 'is ambiguous about which to relax',
  )
  ok(
    (withQuery({ text: '', filter: 'updates' }).empty ?? '').startsWith('Nothing is'),
    'an empty filter with no search gets the shorter sentence',
  )
  eq(
    model.buildModel([], [], [], [], { text: 'sql', filter: 'installed' }).kind,
    'none',
    'and a search never turns the first-launch screen into a "no results" one — that screen '
      + 'offers a Connect field, and it is the search that is wrong, not the absence of a '
      + 'marketplace',
  )
  eq(
    withQuery({ text: '', filter: 'all' }).query,
    { text: '', filter: 'all' },
    'the query is echoed on the model, so the view need not hold it twice',
  )

  eq(model.buildModel([], [], []).kind, 'none', 'a first launch gets the "connect one" screen')
  const ready = model.buildModel(
    [
      {
        id: 'm',
        name: 'm',
        source: '/tmp/m',
        authenticated: false,
        state: { kind: 'ready' },
        entries: [
          { id: 'sql', name: 'SQL', version: '1.0.0', description: '', capabilities: [], updateAvailable: false },
          { id: 'yaml', name: 'YAML', version: '2.0.0', description: '', capabilities: [], installed: '1.0.0', updateAvailable: true },
        ],
        problems: [],
      },
    ],
    [
      { marketplace: 'm', extension: 'yaml', name: 'YAML', version: '1.0.0', enabled: true, capabilities: [], problems: [] },
      { marketplace: 'm', extension: 'gone', name: 'Gone', version: '1.0.0', enabled: false, capabilities: [], problems: [] },
    ],
    [],
  )
  eq(ready.kind, 'ready', 'a connected marketplace gets rows')
  eq(
    ready.groups[0].rows.map((r) => r.action),
    ['install', 'update'],
    'a row that is not installed offers Install and one behind its marketplace offers Update — '
      + 'three different acts must not share one button whose effect the user cannot predict',
  )
  eq(
    ready.orphans.map((r) => r.extension),
    ['gone'],
    'an installed extension its marketplace no longer lists is still drawn, or there is code on '
      + 'disk with no row to remove it from',
  )
  // The language readout: only what a contribution touched.
  //
  // Through a real marketplace rather than three empty arrays, because `buildModel` short-circuits
  // to the first-launch screen when nothing is connected — and a language contributed by nothing
  // installed is a state that cannot exist.
  const langs = model.buildModel(
    [
      {
        id: 'm',
        name: 'm',
        source: '/tmp/m',
        authenticated: false,
        state: { kind: 'ready' },
        entries: [],
        problems: [],
      },
    ],
    [],
    [],
    [
      { id: 'markdown', label: 'Markdown', extensions: ['md'], source: null },
      { id: 'sql', label: 'SQL', extensions: ['sql'], source: 'm.sql', supersedes: 'cide' },
    ],
  )
  eq(
    langs.languages.map((l) => [l.id, l.by, l.instead]),
    [['sql', 'm.sql', 'was cide']],
    'a language a contribution won is listed with the extension that won it and what it '
      + 'displaced; a builtin nothing touched is not, or the answer is buried in eleven rows '
      + 'nobody needs to read',
  )
  eq(
    langs.languages[0].extensions,
    '.sql',
    'and the extensions are spelled with the dot a user would type',
  )

  eq(
    model.buildModel(
      [{ id: 'm', name: 'm', source: '/tmp/m', authenticated: true, state: { kind: 'failed', error: 'nope' }, entries: [], problems: [] }],
      [],
      [],
    ).groups[0].detail,
    'nope',
    "a marketplace that would not clone shows git's own words, which the user is used to reading",
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log(
    `extensions: ok (${checked} checks, ${view.BODY_KINDS.length} body kinds, `
      + `${protocol.REQUEST_KINDS.length} gated requests, ${protocol.CAPABILITIES.length} capabilities)`,
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}
