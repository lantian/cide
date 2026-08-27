/**
 * Checks `src/sidebar/ProblemsPanel/model.ts` — the problems panel's pure core — and pins the
 * three chrome fixes that ship with it.
 *
 * Same shape as `check-status-format.mjs` and `check-git-tree.mjs`, and for the same reason:
 * this project has no JS test runner, and adding one for a handful of pure functions would be
 * a larger commitment than the code it tests.
 *
 * # The failure this exists to prevent
 *
 * A problems panel has exactly one unforgivable bug: showing a confident empty list when
 * nothing has looked. `[]` from a language server and `[]` because no language server exists
 * are opposite claims about the workspace, and every function below is checked for keeping
 * them apart — including `statusBarCounts`, which is what stops the panel and the status bar
 * from ever disagreeing about whether the workspace is clean.
 *
 * # Three layers, because the first two cannot see the third
 *
 * The model is compiled alone and driven under node; the component is then rendered for real
 * through `react-dom/server` (see `ProblemsPanel/smokeEntry.tsx`), because a panel can pass
 * every pure-function check while painting nothing — this repo has shipped that twice. The
 * render layer is also the only one that can see a `styles.x` referencing a rule the
 * stylesheet does not define: that type-checks, evaluates to `undefined`, and React drops the
 * attribute in silence.
 *
 * The three source pins at the end are there because each one is a fix that a later,
 * well-meaning edit would silently undo:
 *   - `StatusBar.tsx` re-growing a diagnostics slot, with its own copy of the "no language
 *     server" sentence,
 *   - `AppHeader.tsx` drawing ⊞/⧉ live again with no handler behind them,
 *   - `App.module.css` letting the bench button paint over the workspace again.
 *
 * Run: `pnpm --dir ui run check:problems`   (or `node ui/scripts/check-problems.mjs`)
 */
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-problems-'))

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
   * `model.ts` deliberately imports nothing — not even a type through the `@/*` alias — so a
   * bare `tsc` with no tsconfig is enough here, unlike `check-git-tree.mjs` which has to
   * synthesise one to resolve `paths`. If this compile ever needs a tsconfig, something has
   * added an import to the module and the node-testability of the core has been lost.
   */
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/sidebar/ProblemsPanel/model.ts',
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

  const model = await import(`file://${join(out, 'model.js')}`)
  const {
    NO_DIAGNOSTICS_SOURCE,
    NO_SOURCE,
    checked,
    countBySeverity,
    groupByFile,
    headline,
    metaFigure,
    severityRank,
    statusBarCounts,
    summaryLine,
    ALL_VISIBLE,
    applyFilters,
    visible,
    STALE_NOTE,
    sourceRows,
  } = model

  const at = (path, line, column, severity, message, extra = {}) => ({
    path,
    line,
    column,
    severity,
    message,
    ...extra,
  })
  const ready = (items, source = 'rust-analyzer') => ({ kind: 'ready', source, items })
  const scanning = (source = 'rust-analyzer') => ({ kind: 'scanning', source })

  // --- The one thing this panel cannot get wrong. ---------------------------------------

  eq(
    statusBarCounts(NO_SOURCE),
    null,
    'no source yields null counts, which is what makes the status bar print `✗ —` and not `✗ 0`',
  )
  deep(
    statusBarCounts(scanning()),
    { errors: 0, warnings: 0, pending: true },
    'a source that has not answered yet counts what it has, marked pending',
  )
  deep(
    statusBarCounts(ready([])),
    { errors: 0, warnings: 0, pending: false },
    'zero becomes a settled figure only once something actually looked',
  )

  eq(checked(NO_SOURCE), false, 'nothing looked')
  eq(checked(scanning()), false, 'looking is not having looked')
  eq(checked(ready([])), true, 'an empty ready snapshot is a real answer')

  eq(metaFigure(NO_SOURCE), '—', 'the header counter withholds a digit when nothing looked')
  eq(metaFigure(scanning()), '—', 'and while one is still starting up')
  eq(metaFigure(ready([])), '0', 'a checked workspace may say zero')
  eq(metaFigure(ready([at('a.rs', 1, 1, 'error', 'boom')])), '1', 'and may say one')

  const unknown = headline(NO_SOURCE)
  eq(unknown.tone, 'unknown', 'no source is neither clean nor a count')
  eq(unknown.detail, NO_DIAGNOSTICS_SOURCE, 'the panel says what the status bar tooltip says')
  ok(
    !/\bno problems\b/i.test(unknown.text),
    'the no-source headline must never claim the workspace is free of problems',
  )
  ok(!/\b0\b/.test(unknown.text), 'nor put a zero in front of the user')

  const waiting = headline(scanning('tsserver'))
  eq(waiting.tone, 'unknown', 'a scan in flight is still an unknown')
  ok(waiting.text.includes('tsserver'), 'and names who it is waiting for')
  ok(
    !/\bno problems\b/i.test(waiting.detail),
    'a partial answer rendered as a clean bill of health is the same bug one round trip earlier',
  )

  const clean = headline(ready([], 'rust-analyzer'))
  eq(clean.tone, 'clean', 'a checked, empty workspace is genuinely clean')
  eq(clean.text, 'No problems found', 'and may say so')
  ok(clean.detail.includes('rust-analyzer'), '"no problems" is only as good as who checked')

  // --- Counting and summarising. ---------------------------------------------------------

  const mixed = [
    at('src/b.rs', 4, 9, 'warning', 'unused variable'),
    at('src/a.rs', 12, 5, 'error', 'mismatched types', { code: 'E0308' }),
    at('src/a.rs', 3, 1, 'hint', 'consider borrowing'),
    at('src/a.rs', 12, 5, 'info', 'expected `u32`'),
    at('src/a.rs', 12, 1, 'error', 'cannot find value'),
  ]

  deep(
    countBySeverity(mixed),
    { error: 2, warning: 1, info: 1, hint: 1, other: 0 },
    'every severity is counted, including the two the status bar does not show',
  )
  deep(
    countBySeverity([]),
    { error: 0, warning: 0, info: 0, hint: 0, other: 0 },
    'nothing counts as zeroes',
  )

  /*
   * The bug this block exists for, and it shipped: an unrecognised severity used to be
   * *dropped*, so a non-empty list counted as all zeroes and `summaryLine` printed
   * "No problems found" — as the headline and as the group's own count — directly above rows
   * the user could see. A panel whose entire premise is "never claim clean without checking"
   * cannot lose an item on the way to its own counter, so unknowns land in `other`.
   */
  const rogue = [
    // The severity annotation is a promise from an external process, not a guarantee.
    at('a.rs', 1, 1, 'catastrophe', 'from the future'),
    // A prototype key, which is the case an `undefined` check silently lets through:
    // `SEVERITY_RANK['constructor']` is a function, not `undefined`.
    at('a.rs', 2, 1, 'constructor', 'from the prototype chain'),
  ]
  deep(
    countBySeverity(rogue),
    { error: 0, warning: 0, info: 0, hint: 0, other: 2 },
    'an unrecognised severity is bucketed as `other`, never dropped',
  )
  for (const items of [mixed, rogue, [...mixed, ...rogue]]) {
    const c = countBySeverity(items)
    eq(
      c.error + c.warning + c.info + c.hint + c.other,
      items.length,
      'the counters total the items — the invariant that makes an empty summary a true statement',
    )
    ok(
      headline(ready(items)).text !== 'No problems found',
      'and so a non-empty list can never headline as a clean bill of health',
    )
  }
  ok(
    severityRank('catastrophe') > severityRank('hint'),
    'an unknown severity sorts after every known one rather than making the comparator non-transitive',
  )
  eq(
    typeof severityRank('constructor'),
    'number',
    'including a prototype key, whose rank was a *function* while the lookup only checked for undefined',
  )
  ok(
    Number.isFinite(severityRank('error') - severityRank('constructor')),
    'so the comparator subtracts two numbers and never yields NaN, which in V8 leaves the list unsorted',
  )
  deep(
    groupByFile(rogue)[0].items.map((d) => d.message),
    ['from the future', 'from the prototype chain'],
    'and a list of unknown severities still comes back sorted rather than in arrival order',
  )

  deep(
    statusBarCounts(ready(mixed)),
    { errors: 2, warnings: 1, pending: false },
    'the status bar sees errors and warnings only',
  )

  eq(
    summaryLine({ error: 2, warning: 1, info: 0, hint: 0, other: 0 }),
    '2 errors, 1 warning',
    'plurals',
  )
  eq(
    summaryLine({ error: 1, warning: 0, info: 0, hint: 0, other: 0 }),
    '1 error',
    'a lone error is singular',
  )
  eq(
    summaryLine({ error: 0, warning: 0, info: 3, hint: 1, other: 0 }),
    '3 infos, 1 hint',
    'severities with none are omitted rather than printed as `0 errors`',
  )
  eq(
    summaryLine({ error: 0, warning: 0, info: 0, hint: 0, other: 2 }),
    '2 other',
    'and rows we could not classify are still reported as rows',
  )
  eq(headline(ready(mixed)).text, '2 errors, 1 warning, 1 info, 1 hint', 'the headline is the summary')
  eq(headline(ready(mixed)).tone, 'counts', 'and is toned as counts')

  // --- Grouping, and the total order a stable list needs. --------------------------------

  const groups = groupByFile(mixed)
  deep(groups.map((g) => g.path), ['src/a.rs', 'src/b.rs'], 'files are ordered by path')
  deep(
    groups[0].items.map((d) => `${d.severity} ${d.line}:${d.column}`),
    ['error 12:1', 'error 12:5', 'info 12:5', 'hint 3:1'],
    'within a file: severity first, then line, then column',
  )
  deep(
    groups[0].counts,
    { error: 2, warning: 0, info: 1, hint: 1, other: 0 },
    'each group carries its own counts, so a collapsed group can still be summarised',
  )
  deep(groupByFile([]), [], 'nothing groups into nothing, not into an empty file')

  // Ties must break deterministically or the list reshuffles under the pointer on re-render.
  const tied = [
    at('x.rs', 1, 1, 'error', 'beta'),
    at('x.rs', 1, 1, 'error', 'alpha'),
  ]
  deep(
    groupByFile(tied)[0].items.map((d) => d.message),
    ['alpha', 'beta'],
    'two diagnostics at the same position still have a stable order',
  )
  deep(
    groupByFile([...tied].reverse())[0].items.map((d) => d.message),
    ['alpha', 'beta'],
    'and it is the same order whichever way they arrived',
  )

  // The panel renders `item.code` after the message; losing it in transit is silent.
  const carried = groupByFile(mixed).flatMap((g) => g.items).filter((d) => d.code !== undefined)
  deep(carried.map((d) => d.code), ['E0308'], 'the producer’s own code survives grouping')

  // --- M12: partial answers, the emit cap, and the filter. --------------------------------

  /*
   * A partial answer now counts, **marked pending** — a reversal, and the old argument is
   * kept here because it was half right. The figure may indeed grow the moment the indexing
   * server finishes; the pending flag is how the bar qualifies it ("3 errors so far — still
   * indexing"). What the old rule actually shipped was worse than a growing figure: `null`
   * shares the bar's dash slot with `unavailable`, whose tooltip says "no language server is
   * running" — so one server stuck in `scanning` (a leaked progress token holds the whole
   * snapshot there for ever) made the bar dead and lying while the editor visibly underlined
   * the very diagnostics being counted.
   */
  const partial = {
    kind: 'scanning',
    source: 'rust-analyzer',
    items: [
      at('src/a.rs', 1, 1, 'error', 'one'),
      at('src/a.rs', 2, 1, 'error', 'two'),
      at('src/b.rs', 3, 1, 'error', 'three'),
    ],
  }
  deep(
    statusBarCounts(partial),
    { errors: 3, warnings: 0, pending: true },
    'a partial answer reports the rows it can see, and says they are still growing',
  )
  eq(checked(partial), false, 'and is still not "checked"')
  eq(metaFigure(partial), '—', 'and still withholds the header digit')
  eq(headline(partial).tone, 'unknown', 'and is still toned unknown')
  ok(
    !/\bno problems\b/i.test(headline(partial).text),
    'a partial answer must never headline as a clean bill of health',
  )
  ok(
    headline(partial).text.includes('rust-analyzer'),
    'it names who has not answered yet',
  )
  ok(
    /3 errors/.test(headline(partial).detail),
    'and says what the sources that *have* answered found, rather than implying nothing was',
  )

  // The emit cap. A header counter that reports its own prefix as the whole is the same quiet
  // lie as an unchecked zero.
  eq(
    metaFigure({ kind: 'ready', source: 'x', items: [at('a.rs', 1, 1, 'error', 'e')], truncated: 213 }),
    '214',
    'the header figure is the true total, not the number of rows shipped',
  )

  // --- The filter, and the axiom applied to it. -------------------------------------------

  const filters = (over) => ({ ...ALL_VISIBLE, ...over })
  const hintAndError = [
    at('src/a.rs', 1, 1, 'error', 'real'),
    at('src/a.rs', 2, 1, 'hint', 'nit', { source: 'clippy' }),
  ]
  const readyBoth = { kind: 'ready', source: 'rust-analyzer', items: hintAndError }

  const noHints = applyFilters(readyBoth, filters({ severities: { ...ALL_VISIBLE.severities, hint: false } }))
  eq(noHints.snapshot.items.length, 1, 'a hidden severity leaves the panel')
  eq(noHints.hidden, 1, 'and is counted rather than forgotten')
  deep(
    statusBarCounts(noHints.snapshot),
    { errors: 1, warnings: 0, pending: false },
    'the bar reads the *filtered* snapshot, so it cannot disagree with the panel',
  )

  const noClippy = applyFilters(readyBoth, filters({ sources: { clippy: false } }))
  eq(noClippy.snapshot.items.length, 1, 'a muted producer leaves too')
  ok(
    visible(at('a.rs', 1, 1, 'error', 'e', { source: 'some-future-linter' }), ALL_VISIBLE),
    'a producer nobody has heard of is SHOWN — the map is exceptions, not a registry',
  )

  const level = applyFilters(readyBoth, filters({ level: 'none' }))
  eq(level.snapshot.items.length, 0, 'the `none` highlighting level hides everything')
  const syntaxOnly = applyFilters(
    { kind: 'ready', source: 'x', items: [
      at('a.rs', 1, 1, 'error', 'parse', { kind: 'syntax' }),
      at('a.rs', 2, 1, 'error', 'types', { kind: 'semantic' }),
    ] },
    filters({ level: 'syntax' }),
  )
  deep(
    syntaxOnly.snapshot.items.map((d) => d.message),
    ['parse'],
    '`syntax` reads the producer\'s own classification rather than guessing from the source name',
  )

  /*
   * The failure this whole block exists for. Filters that hide everything leave an empty list —
   * and an empty list headlined "No problems found" is the confident-clean-bill-of-health lie,
   * this time produced by the panel itself rather than by a missing analyser.
   */
  const allHidden = applyFilters(
    readyBoth,
    filters({ severities: { error: false, warning: false, info: false, hint: false } }),
  )
  eq(allHidden.snapshot.items.length, 0, 'everything filtered out')
  eq(allHidden.hidden, 2, 'and both are counted')
  const hiddenHeadline = headline(allHidden.snapshot, allHidden.hidden)
  ok(
    hiddenHeadline.text !== 'No problems found',
    'a panel whose filters hid every problem must not claim there are none',
  )
  ok(/filters/i.test(hiddenHeadline.text), 'it says the filters are why')
  ok(/2 hidden/.test(hiddenHeadline.detail), 'and how many')

  // A snapshot nothing looked at cannot be filtered into one that did.
  eq(
    applyFilters(NO_SOURCE, filters({})).snapshot.kind,
    'unavailable',
    'filtering must never manufacture a state where something looked',
  )

  // The default argument keeps every pre-M12 call site — and every assertion above — unchanged.
  eq(
    headline({ kind: 'ready', source: 'x', items: [] }).text,
    'No problems found',
    'a genuinely clean workspace still says so when nothing was hidden',
  )

  // --- Staleness, and the analyser list. (M18) --------------------------------------------

  /*
   * The half of the M18 report the model owns.
   *
   * A diagnostic keeps the line number it was published with, and an out-of-editor write — an
   * agent's edit, a `git checkout` — moves the file underneath it. The row must stay clickable (a
   * jump a few lines off beats a dead row) and must say which it is, or the user lands in a
   * comment with nothing on screen to explain why. That was the report, verbatim.
   */
  {
    const mixed = groupByFile([
      at('src/a.rs', 1, 1, 'error', 'boom', { stale: true }),
      at('src/a.rs', 9, 1, 'hint', 'consider'),
      at('src/b.rs', 4, 1, 'warning', 'unused'),
    ])
    eq(mixed[0].stale, true, 'a group with any stale row is marked')
    eq(mixed[1].stale, false, 'and a group with none is not — a mark on everything says nothing')
    eq(
      groupByFile([at('src/a.rs', 1, 1, 'error', 'boom')])[0].stale,
      false,
      'absent means "nothing has said this is out of date", which is what every pre-M18 fixture ' +
        'relies on',
    )
    ok(
      STALE_NOTE.length > 0 && STALE_NOTE.length < 80,
      'the note fits under a group heading in a 252px panel',
    )
  }

  /*
   * `sources` has been on the wire since M12 and the panel drew none of it, so
   * "rust-analyzer is not installed" — the one sentence that explains an empty list — arrived and
   * was discarded. And `restartable` is the field that stops a Restart button appearing on a
   * source that is not a process, where `ProjectDiagnostics::restart` returns immediately.
   */
  {
    deep(sourceRows(NO_SOURCE), [], 'a snapshot with no source array yields no rows, not fake ones')

    const rows = sourceRows({
      kind: 'unavailable',
      reason: 'nothing running',
      sources: [
        {
          id: 'rustAnalyzer',
          label: 'rust-analyzer',
          status: { kind: 'unavailable', reason: 'rust-analyzer is not on PATH.' },
          items: 0,
        },
        { id: 'gopls', label: 'gopls', status: { kind: 'scanning', detail: 'Loading' }, items: 0 },
        { id: 'treeSitter', label: 'tree-sitter', status: { kind: 'ready' }, items: 3 },
        { id: 'claude', label: 'claude', status: { kind: 'ready' }, items: 0 },
        // M22: a language server an extension contributed, under its own binary name, and an
        // extension publishing findings from its own worker. The first is a process cide spawned
        // and can spawn again; the second is not a process at all.
        { id: 'sqls', label: 'sqls', status: { kind: 'ready' }, items: 2 },
        {
          id: 'ext:cide-marketplace.sql',
          label: 'cide-marketplace.sql',
          status: { kind: 'ready' },
          items: 1,
        },
      ],
    })
    deep(
      rows.map((r) => r.restartable),
      [true, true, false, false, true, false],
      'every source that is a process cide spawned offers a restart, and nothing else does. It ' +
        'was a *list* of the two builtins until M22, which was complete right up until an ' +
        'extension could contribute a server — and then `sqls` reported findings into this panel ' +
        'with no way to restart it, which is a failure with no symptom in any gate',
    )
    eq(
      rows[0].detail,
      'rust-analyzer is not on PATH.',
      'the unavailable reason is shown whole: it is the only actionable sentence this panel has',
    )
    eq(rows[1].detail, 'Loading', 'a scanning source shows the server’s own progress line')
    eq(rows[2].detail, 'Ready — 3 findings.', 'and a ready one shows what it found')
    eq(rows[3].detail, 'Ready — nothing found.', 'including nothing, which is a real answer')
    eq(
      sourceRows({
        kind: 'unavailable',
        reason: 'x',
        sources: [{ id: 'gopls', label: 'gopls', status: { kind: 'scanning', detail: '' }, items: 0 }],
      })[0].detail,
      'Starting…',
      'a blank progress line becomes words — an empty line under a name reads as a broken row',
    )
  }

  // --- Source pins: three fixes a later edit would quietly undo. --------------------------

  const read = (rel) => readFileSync(join(UI, rel), 'utf8')

  /*
   * The status bar draws no diagnostics at all any more (M28). The `✗ n ⚠ n` pair was a second
   * rendering of the figure the rail's ⚠ badge carries one row up, and the user asked for the
   * counters beside the branch to go; the whole left group went with them.
   *
   * The pin is inverted rather than deleted, because the failure it guards against is the same
   * one in both directions: a bar that grows its own copy of the panel's no-source sentence.
   * Re-adding the slot means re-importing the sentence, and this line says so.
   */
  const statusBar = read('src/chrome/StatusBar.tsx')
  ok(
    !statusBar.includes('/ProblemsPanel/model'),
    'StatusBar draws no diagnostics, so it reaches into the problems model for nothing',
  )
  ok(
    !statusBar.includes('Diagnostics need a language server.'),
    'and holds no copy of the no-source sentence inline either — two hand-kept copies of a '
      + 'user-visible claim are two claims, and that is what a re-added slot must not do',
  )

  const header = read('src/chrome/AppHeader.tsx')
  ok(
    header.includes('disabled={onSplit === undefined}'),
    'the header’s ⊞ is disabled when no host can split, rather than enabled and inert',
  )
  ok(
    header.includes('disabled={onDetach === undefined}'),
    'and so is ⧉',
  )

  /*
   * The panel is *fed*. (M12)
   *
   * Every assertion above this line is about the model, and the model is perfectly happy with a
   * host that never passes it anything — `snapshot` is optional and defaults to `NO_SOURCE`,
   * which is exactly the shape the panel shipped in for two milestones. So an edit that dropped
   * the prop would leave this whole file green while the panel went back to saying nothing is
   * analysing the workspace, on a build where something is.
   *
   * Pinned as source text rather than as behaviour because there is no way to render `App.tsx`
   * under node — it reaches the workspace store, the key gate and every pane kind.
   */
  const app = read('src/App.tsx')
  ok(
    /<ProblemsPanel[\s\S]{0,400}snapshot=\{/.test(app),
    'App.tsx passes a live snapshot to ProblemsPanel rather than letting it default to NO_SOURCE',
  )
  // The derivation is a single `useMemo` (`diagCounts`) so the rail's badge gets one identity
  // per snapshot rather than a fresh object on every render of the shell. Its second consumer,
  // the status bar, is gone (M28) — hence a pin on the rail's prop rather than on the bar's.
  ok(
    /const diagCounts = useMemo\(\(\) => statusBarCounts\(diagnostics\.snapshot\)/.test(app) &&
      /errors=\{diagCounts\?\.errors \?\? null\}/.test(app),
    'and derives the rail’s badge from that same snapshot, so the panel and the badge cannot '
      + 'disagree about whether the workspace is clean',
  )
  ok(
    !app.includes('diagnostics={diagCounts}'),
    'and does not feed it to the status bar as well — one figure, one renderer',
  )
  ok(
    app.includes('applyFilters('),
    'and applies the user’s filters exactly once, in the host, rather than in each consumer',
  )

  const appCss = read('src/App.module.css')
  ok(
    /\.benchButton\s*\{[^}]*display:\s*none/.test(appCss),
    'the bench button is hidden by default, so it cannot float over the workspace on a normal launch',
  )
  ok(
    appCss.includes('.benchButton.benchButtonArmed'),
    'and there is a class for the CIDE_BENCH-guarded version to put it back',
  )

  // --- And whether the thing actually paints. ---------------------------------------------

  /*
   * Everything above tests the pure core, which a panel can pass while rendering nothing at
   * all — this repo has shipped exactly that, twice, and `check-git-render.mjs` was written
   * the last time it happened. So the component goes through `react-dom/server` for real,
   * with a `ready` snapshot the model has never seen rendered.
   *
   * Built under `node_modules/.cache` rather than the temp dir this script uses for the model
   * compile: the SSR bundle keeps `react-dom/server` external, so node resolves it relative to
   * the output, and under /tmp there is no `node_modules` above it.
   */
  mkdirSync(join(UI, 'node_modules/.cache'), { recursive: true })
  const renderOut = mkdtempSync(join(UI, 'node_modules/.cache', 'cide-problems-render-'))
  let digests
  try {
    execFileSync(
      'node',
      [
        'node_modules/vite/bin/vite.js',
        'build',
        '--ssr', 'src/sidebar/ProblemsPanel/smokeEntry.tsx',
        '--outDir', renderOut,
        '--logLevel', 'error',
      ],
      { cwd: UI, stdio: 'inherit' },
    )
    const printed = []
    const log = console.log
    console.log = (line) => printed.push(line)
    await import(`file://${join(renderOut, 'smokeEntry.js')}`)
    console.log = log
    digests = Object.fromEntries(JSON.parse(printed.at(-1)).map((d) => [d.story, d]))
  } finally {
    rmSync(renderOut, { recursive: true, force: true })
  }

  const story = (name) => digests[name] ?? {}

  eq(story('no-source').meta, '—', 'the rendered header withholds the digit when nothing looked')
  eq(
    story('no-source').claim,
    'No diagnostics source is running',
    'and the rendered claim is the model’s, not a second copy in the JSX',
  )
  eq(story('no-source').explainer, true, 'the surprising state carries its explainer')
  eq(story('scanning').explainer, false, 'a state that is merely pending does not')
  eq(story('no-project').claim, null, 'with no project open the analyser is not the reason')
  eq(story('clean').meta, '0', 'a checked, empty workspace renders a real zero')

  const ready4 = story('ready')
  eq(ready4.meta, '4', 'a ready snapshot renders its count')
  eq(ready4.claim, '2 errors, 1 warning, 1 hint', 'and its summary')
  deep(
    ready4.rows,
    [
      'error|cannot find value',
      'error|mismatched types',
      'hint|consider borrowing',
      'warning|unused variable',
    ],
    'every diagnostic reaches the DOM, worst first, in the order the model sorted them',
  )
  deep(
    ready4.groups,
    ['src/a.rs :: 2 errors, 1 hint', 'src/b.rs :: 1 warning'],
    'grouped by file, each group carrying its own count line',
  )
  deep(
    ready4.glyphClasses,
    [2, 2, 2, 2],
    'each glyph carries its severity colour on top of `.glyph` — the only thing that tells an ' +
      'error from a hint at a glance',
  )
  eq(ready4.rowTag, 'div', 'with no `onOpenLocation`, a row is static text and not a dead button')
  eq(story('ready-wired').rowTag, 'button', 'and a real button once a host can act on the click')

  /*
   * The regression this whole review turned on. Before the `other` bucket, this story rendered
   * `claim: "No problems found"` and `groups: ["src/a.rs :: No problems found"]` above two
   * visible rows, with the header counter reading 2.
   */
  const rogue2 = story('rogue')
  eq(rogue2.meta, '2', 'an unknown severity still counts toward the header figure')
  eq(rogue2.claim, '2 other', 'and is reported rather than silently uncounted')
  deep(rogue2.groups, ['src/a.rs :: 2 other'], 'in the group line too')
  eq(rogue2.rows.length, 2, 'and both rows are on screen to be counted')
  deep(
    rogue2.glyphClasses,
    [2, 2],
    'an unrenderable severity still gets a glyph class rather than a stringified function',
  )
  deep(
    rogue2.glyphs,
    ['minus', 'minus'],
    'and a real mark — including for the prototype key, whose table lookup is a function that ' +
      '`??` passes through, blanking the cell and poisoning the class string',
  )
  ok(
    rogue2.claim !== 'No problems found',
    'a panel that renders problems must never headline that there are none',
  )

  /*
   * The rendered half of M18. The model can be perfectly right about staleness and about the
   * analyser list while the component draws neither — which is exactly the state the panel was in
   * for `snapshot.sources` between M12 and M18, with every model assertion above it green.
   */
  {
    const stale = story('stale')
    deep(stale.staleGroups, ['src/a.rs'], 'only the group whose file moved is marked')
    eq(stale.staleNote, true, 'and the note that says why is on screen')
    eq(stale.staleRows.length, 3, 'every row in that group carries the mark')
    eq(
      stale.rowTag,
      'button',
      'a stale row stays clickable — a jump that may be a few lines off beats a dead row',
    )
    eq(story('ready-wired').staleNote, false, 'and a current list says nothing about staleness')
    deep(story('ready-wired').staleGroups, [], 'nor marks any group')
  }

  {
    const sources = story('sources')
    deep(
      sources.sources,
      [
        'rustAnalyzer :: rust-analyzer is not on PATH.',
        'gopls :: Loading packages',
        'treeSitter :: Ready — 2 findings.',
        'claude :: Ready — nothing found.',
      ],
      'the analyser list is drawn on an `unavailable` snapshot — the state that most needs it',
    )
    eq(
      sources.refresh,
      false,
      'with no `onRefresh` the control is absent, not disabled: there is no condition under ' +
        'which it would work, and dressing a dead thing as live is this panel’s named failure',
    )
    deep(sources.restarts, [], 'and no Restart buttons either')

    const wired = story('sources-wired')
    eq(wired.refresh, true, 'a host that can re-run gets the button the user asked for')
    deep(
      wired.restarts,
      ['rustAnalyzer', 'gopls'],
      'and Restart appears on exactly the two sources that are processes',
    )
  }

  /*
   * The panel's two new controls are *reachable*. (M18)
   *
   * Every render assertion above is satisfied by a component with the right props; none of them
   * can see that `App.tsx` never passes those props, which is precisely how `diagnostics.restart`
   * sat fully built and callable with no caller anywhere in the app from M12 to M18.
   */
  ok(
    /<ProblemsPanel[\s\S]{0,900}onRefresh=\{/.test(app),
    'App.tsx wires the Re-run control, so the button is not merely renderable',
  )
  ok(
    // The window is wider than `onRefresh`'s above because the routing comments sit between
    // the two props since the handlers moved into pinned `useCallback`s.
    /<ProblemsPanel[\s\S]{0,1600}onRestartSource=\{/.test(app),
    'and the per-source Restart, which is the first caller `diagnostics.restart` has ever had',
  )
  {
    const actions = read('src/sidebar/ProblemsPanel/actions.ts')
    ok(
      /diagnosticsApi\s*\.\s*refresh\(/.test(actions) &&
        /diagnosticsApi\s*\.\s*restart\(/.test(actions),
      'both gestures go through the IPC client rather than being faked in the component',
    )
    const dispatch = read('src/keys/dispatch.ts')
    ok(
      dispatch.includes("case 'problems.refresh':") &&
        dispatch.includes('refreshDiagnostics(project.id)'),
      'and the palette/keymap id calls the same function the button does, so they cannot drift',
    )
  }

  for (const [name, d] of Object.entries(digests)) {
    eq(d.unclassed, 0, `${name}: every audit-hooked element carries a class from the stylesheet`)
  }

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('problems panel: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
