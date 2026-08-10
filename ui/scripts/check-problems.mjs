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
 * The three source pins at the end are there because each one is a fix that a later,
 * well-meaning edit would silently undo:
 *   - `StatusBar.tsx` re-growing its own copy of the "no language server" sentence,
 *   - `AppHeader.tsx` drawing ⊞/⧉ live again with no handler behind them,
 *   - `App.module.css` letting the bench button paint over the workspace again.
 *
 * Run: `pnpm --dir ui run check:problems`   (or `node ui/scripts/check-problems.mjs`)
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
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
  eq(
    statusBarCounts(scanning()),
    null,
    'a source that has not answered yet has pending counts, not zero ones',
  )
  deep(
    statusBarCounts(ready([])),
    { errors: 0, warnings: 0 },
    'zero becomes reportable only once something actually looked',
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
    { error: 2, warning: 1, info: 1, hint: 1 },
    'every severity is counted, including the two the status bar does not show',
  )
  deep(countBySeverity([]), { error: 0, warning: 0, info: 0, hint: 0 }, 'nothing counts as zeroes')
  deep(
    // The severity annotation is a promise from an external process, not a guarantee.
    countBySeverity([at('a.rs', 1, 1, 'catastrophe', 'from the future')]),
    { error: 0, warning: 0, info: 0, hint: 0 },
    'an unrecognised severity is dropped from the counters rather than creating a NaN column',
  )
  ok(
    severityRank('catastrophe') > severityRank('hint'),
    'and sorts after every known severity rather than making the comparator non-transitive',
  )

  deep(
    statusBarCounts(ready(mixed)),
    { errors: 2, warnings: 1 },
    'the status bar sees errors and warnings only',
  )

  eq(summaryLine({ error: 2, warning: 1, info: 0, hint: 0 }), '2 errors, 1 warning', 'plurals')
  eq(summaryLine({ error: 1, warning: 0, info: 0, hint: 0 }), '1 error', 'a lone error is singular')
  eq(
    summaryLine({ error: 0, warning: 0, info: 3, hint: 1 }),
    '3 infos, 1 hint',
    'severities with none are omitted rather than printed as `0 errors`',
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
    { error: 2, warning: 0, info: 1, hint: 1 },
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

  // --- Source pins: three fixes a later edit would quietly undo. --------------------------

  const read = (rel) => readFileSync(join(UI, rel), 'utf8')

  const statusBar = read('src/chrome/StatusBar.tsx')
  ok(
    statusBar.includes("from '@/sidebar/ProblemsPanel/model'"),
    'StatusBar imports the no-source sentence rather than keeping a second copy of it',
  )
  ok(
    !statusBar.includes('Diagnostics need a language server.'),
    'and does not hold that copy inline — two hand-kept copies of a user-visible claim are two claims',
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

  const appCss = read('src/App.module.css')
  ok(
    /\.benchButton\s*\{[^}]*display:\s*none/.test(appCss),
    'the bench button is hidden by default, so it cannot float over the workspace on a normal launch',
  )
  ok(
    appCss.includes('.benchButton.benchButtonArmed'),
    'and there is a class for the CIDE_BENCH-guarded version to put it back',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('problems panel: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
