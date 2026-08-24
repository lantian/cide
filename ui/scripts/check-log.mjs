/**
 * Checks `src/gitlog/logModel.ts` — every decision the commit log makes about what a row says,
 * which sentence an empty list shows, and what the filter bar asks for — and, since M19, the
 * **row context menu**: which of the eight lines a commit offers, which of them are disabled and
 * why, and what the reset dialog's answer becomes on the wire.
 *
 * The menu model lives in `src/gitlog/logMenu.ts` — its own module, because `LogTab.tsx` cannot
 * be compiled here at all; the long version is at the head of that section below.
 * `check-log-actions.mjs` owns the other half — every sentence those lines and their dialogs
 * show — and nothing is asserted twice.
 *
 * Same shape as `check-branches.mjs` and for the same reason: this project has no JS test runner,
 * and adding one for two dozen pure functions would be a larger commitment than the code it
 * tests. So the decisions live in a DOM-free module and this script compiles that one file and
 * drives it. A temp `tsconfig.json` with `paths: {'@/*': ['src/*']}` is what lets it, because the
 * model type-imports the generated DTOs — `import type`, so nothing is emitted, but tsc still has
 * to resolve them.
 *
 * # What is worth pinning here
 *
 * Four classes of silent failure, each of which has a real precedent in this repository:
 *
 * 1. **A wire rename that never reached the model.** The generated field names are read out of
 *    `src/ipc/generated.ts` first, so a rename in `crates/cide-ipc/src/history.rs` that
 *    `cargo xtask codegen` propagates fails here rather than producing `undefined` in the oid
 *    column. This is the join that kept the git panel blank for a milestone.
 * 2. **Two states that say the same thing.** Nine different situations produce a list with no
 *    rows in it, and to the user they call for six different next moves. A view that collapsed
 *    any two of them would be invisible in a screenshot, so the distinctness is asserted rather
 *    than trusted — and so is the rule that a zero-row list is *never* silent.
 * 3. **The two halves of the filter disagreeing.** The text box narrows the loaded page locally
 *    while the debounced request is in flight, and the backend narrows the walk. They are two
 *    implementations of one rule; the assertions below pin every way they are the same and every
 *    documented way they are not.
 * 4. **A date that goes stale or goes absurd.** Dates here are absolute by house rule
 *    (`sidebar/GitPanel/ShelfList.tsx` sets it), and a rebase writes commit timestamps in the
 *    future — which a relative renderer prints as `-3m`.
 * 5. **A menu line that is listed and dead.** `menus/model.ts` resolves `run` to `null` the
 *    moment a `disabledReason` is present, so an item carrying both looks wired and does nothing
 *    at all when clicked. Nothing on screen distinguishes that from a working line until it is
 *    pressed, which is the whole reason `disabled: boolean` does not exist in this app.
 *
 * What this does NOT cover, and nothing here should be read as claiming:
 *   - that the panel renders. `check-log-render.mjs` server-renders `LogView` against the same
 *     stories and asserts on the markup; this file never touches React.
 *   - that a menu *opens*, or that clicking a line reaches Rust. `check-menus.mjs` owns the menu
 *     system's geometry and keyboard, the `#[tauri::command]`s are tested in
 *     `crates/cide-git/tests/`, and the wire between the two is pinned here only as "this call
 *     site exists in the source" — which is the class of bug this feature actually had.
 *   - that the *backend* filter matches what `matchesText` matches. That is `cide_git::log`'s
 *     rule, tested in Rust against the real `git` binary. What is asserted here is that the
 *     needle this side sends and the needle this side matches on are derived the same way, and
 *     that the two documented divergences are exactly the two that are documented.
 *   - that `toLocaleDateString` produces any particular string. It is the platform's, and it
 *     differs by locale and by ICU version. The assertions are about which *fields* it was asked
 *     for and about the year boundary, never about the punctuation.
 *   - anything about paging correctness. `mergePage` de-duping is a display invariant; whether
 *     two pages are gapless is `crates/cide-git/tests/`' problem.
 *
 * Run: `pnpm --dir ui run check:log`
 */
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const ROOT = resolve(UI, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-log-'))

let failed = 0
let checked = 0
const fail = (what, detail) => {
  failed += 1
  console.error(`FAIL ${what}${detail === undefined ? '' : `\n  ${detail}`}`)
}
const eq = (actual, expected, what) => {
  checked += 1
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) fail(what, `actual:   ${a}\n  expected: ${b}`)
}
const ok = (cond, what) => {
  checked += 1
  if (!cond) fail(what)
}

/**
 * Source with comments removed.
 *
 * A grep over raw source matches the *explanation* of a rule as happily as the rule, so an
 * assertion written that way stays green after the code is deleted and only the prose is left.
 * That is a lesson this repository has paid for; `check-branches.mjs` and `check-paths.mjs` strip
 * for the same reason. It matters more here than there, because the generated DTOs are almost
 * entirely doc comment and half of them name their own siblings' field names inside it.
 */
const stripComments = (source) => {
  let text = ''
  let i = 0
  while (i < source.length) {
    const ch = source[i]
    if (ch === '/' && source[i + 1] === '/') {
      while (i < source.length && source[i] !== '\n') i++
      continue
    }
    if (ch === '/' && source[i + 1] === '*') {
      i += 2
      while (i < source.length && !(source[i] === '*' && source[i + 1] === '/')) i++
      i += 2
      continue
    }
    if (ch === "'" || ch === '"' || ch === '`') {
      const quote = ch
      text += ch
      i++
      while (i < source.length && source[i] !== quote) {
        if (source[i] === '\\') {
          text += source[i]
          i++
        }
        if (i < source.length) {
          text += source[i]
          i++
        }
      }
      text += quote
      i++
      continue
    }
    text += ch
    i++
  }
  return text
}

try {
  // --- the wire shape, pinned against the generated types ----------------------------------
  //
  // Sliced out of `generated.ts` exactly as `check-branches.mjs` does it, and then stripped of
  // comments before the field names are looked for — otherwise `[`CommitPage::commits`]` inside a
  // *neighbouring* doc comment satisfies an assertion about a field that has been deleted.

  const generated = readFileSync(join(UI, 'src', 'ipc', 'generated.ts'), 'utf8')
  const shape = (name) => {
    const at = generated.indexOf(`export type ${name} = `)
    if (at < 0) return ''
    const end = generated.indexOf('};', at)
    return generated.slice(at, end < 0 ? generated.length : end)
  }
  for (const [type, fields] of [
    [
      'CommitRow',
      [
        'repo',
        'oid',
        'shortOid',
        'summary',
        'author',
        'authorEmail',
        'authored',
        'committed',
        'parents',
        'pruned',
        'refs',
      ],
    ],
    [
      'CommitPage',
      [
        'commits',
        'graph',
        'resume',
        'repos',
        'stop',
        'scanned',
        'cancelled',
        'renames',
        'followed',
        'followCapped',
      ],
    ],
    [
      'LogQuery',
      [
        'scope',
        'refs',
        'cursor',
        'path',
        'follow',
        'simplify',
        'firstParent',
        'author',
        'text',
        'limit',
        'scanLimit',
        'graph',
        'graphLanes',
      ],
    ],
    [
      'CommitDetail',
      [
        'commit',
        'message',
        'committer',
        'committerEmail',
        'against',
        'files',
        'filesTruncated',
        'merge',
        'total',
      ],
    ],
    ['CommitFile', ['path', 'oldPath', 'status', 'binary', 'lines']],
    ['RefChip', ['kind', 'name', 'full', 'current']],
    ['GraphRow', ['lane', 'color', 'edges', 'overflow']],
    ['RepoPage', ['repo', 'name', 'stop', 'scanned']],
  ]) {
    const body = stripComments(shape(type))
    ok(body !== '', `${type} exists in generated.ts`)
    for (const field of fields) {
      ok(new RegExp(`\\b${field}\\b`).test(body), `${type}.${field} is still on the wire`)
    }
  }

  // `LogQuery` has thirteen fields and `deny_unknown_fields` in Rust, so a partial object is a
  // deserialisation failure rather than a defaulted query. `client.ts::logQuery` is the one place
  // that fills them; this pins the count so a fourteenth cannot arrive unnoticed.
  eq(
    stripComments(shape('LogQuery')).match(/\b\w+(?=:)/g)?.length,
    13,
    'LogQuery still has exactly thirteen fields — `logQuery(scope, over)` fills every one, and a '
      + 'new one that nobody fills is a `deny_unknown_fields` deserialisation error at runtime',
  )

  // The `bigint` trap. ts-rs renders an `i64` as one, `new Date(bigint)` throws, and
  // `Number(undefined)` is `NaN` — which renders as "Invalid Date" with nothing in the console.
  ok(
    /authored: bigint, committed: bigint/.test(stripComments(shape('CommitRow'))),
    'a row still carries its two timestamps as `bigint`, which is why `when` narrows with '
      + '`Number()` rather than handing the value straight to `Date`',
  )

  // The gap `matchesText` documents: the wire matches the whole message, and the row does not
  // carry one. If a `message` ever appears on `CommitRow`, the local pass should widen to use it
  // and this assertion is where that conversation starts.
  ok(
    !/\bmessage\b/.test(stripComments(shape('CommitRow'))),
    'a row carries `summary` and not the whole message, which is the documented reason the local '
      + 'text pass is narrower than the backend’s',
  )

  // Every stop the model has to have an answer for.
  eq(
    stripComments(generated)
      .match(/export type LogStop = ([^;]*);/)?.[1]
      ?.split('|')
      .map((s) => s.trim().replace(/"/g, '')),
    ['exhausted', 'page', 'budget', 'shallow', 'cursorLost', 'noSuchRef'],
    'the six stop variants, in the generated order — every one of them has to be reachable in '
      + 'the UI, which is what the `logStatus` and `moreLabel` tables below are for',
  )

  eq(
    stripComments(generated)
      .match(/export type GraphOff = ([^;]*);/)?.[1]
      ?.split('|')
      .map((s) => s.trim().replace(/"/g, '')),
    ['filtered', 'fullHistory', 'merged', 'disabled', 'rerooted'],
    'and the five reasons a page can have no graph, each of which `graphOffReason` answers',
  )

  // --- compile the model on its own ---------------------------------------------------------

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
        // `src`, not `src/gitlog`: the model reaches `@/ipc/generated` through the alias, and
        // tsc requires every source file to sit under `rootDir`.
        rootDir: join(UI, 'src'),
        baseUrl: UI,
        paths: { '@/*': ['src/*'] },
        types: [],
      },
      // Four import-free modules, compiled together and executed below. `branchFilter`,
      // `fileRows` and `fileMenu` joined in M21 — each is a rule with edge cases that only a
      // real repository reaches, which is exactly the shape that belongs in a check rather than
      // in a component.
      files: [
        join(UI, 'src', 'gitlog', 'logModel.ts'),
        join(UI, 'src', 'gitlog', 'branchFilter.ts'),
        join(UI, 'src', 'gitlog', 'fileRows.ts'),
        join(UI, 'src', 'gitlog', 'fileMenu.ts'),
        // The log's own click rule lives beside the git tree's, so the two are visible together.
        join(UI, 'src', 'sidebar', 'clickSemantics.ts'),
      ],
    }),
  )
  execFileSync('node', ['node_modules/typescript/bin/tsc', '--project', tsconfig], {
    stdio: 'inherit',
    cwd: UI,
  })

  const m = await import(`file://${join(out, 'gitlog', 'logModel.js')}`)
  const bf = await import(`file://${join(out, 'gitlog', 'branchFilter.js')}`)
  const fr = await import(`file://${join(out, 'gitlog', 'fileRows.js')}`)
  const fm = await import(`file://${join(out, 'gitlog', 'fileMenu.js')}`)
  const cs = await import(`file://${join(out, 'sidebar', 'clickSemantics.js')}`)

  // --- fixtures -------------------------------------------------------------------------------

  const full = (stem) => (stem + '0'.repeat(40)).slice(0, 40)
  const chip = (kind, name) => ({ kind, name, full: `refs/${kind}/${name}`, current: false })
  const at = (y, mo, d) => BigInt(Math.floor(Date.UTC(y, mo - 1, d, 12) / 1000))

  const row = (stem, summary, author, authored, over = {}) => ({
    repo: 'repo-1',
    oid: full(stem),
    shortOid: stem,
    summary,
    author,
    authorEmail: `${author.toLowerCase()}@example.com`,
    authored,
    committed: authored,
    parents: [],
    pruned: 0,
    refs: [],
    ...over,
  })

  const NOW = Date.UTC(2026, 2, 14, 12, 0, 0)

  const ROWS = [
    row('a1b2c3d', 'The awaiting chip is a gate', 'Ivan', at(2026, 3, 12)),
    row('deadbee', 'Go to definition from inside an interface', 'Mira', at(2026, 3, 10)),
    row('c0ffee1', 'A pane that reattaches gets its buffer', 'Ivan', at(2019, 11, 2)),
  ]

  const page = (over = {}) => ({
    commits: ROWS,
    graph: { kind: 'rows', rows: [], lanes: 0, overflow: false },
    resume: null,
    repos: [{ repo: 'repo-1', name: 'cide', stop: 'exhausted', scanned: 3 }],
    stop: 'exhausted',
    scanned: 3,
    cancelled: false,
    renames: [],
    followed: false,
    followCapped: false,
    ...over,
  })

  // --- ref chips: order, cap, overflow --------------------------------------------------------

  eq(
    m.orderChips([
      chip('tag', 'v1.0.0'),
      chip('remoteBranch', 'origin/main'),
      chip('localBranch', 'main'),
      chip('head', 'HEAD'),
    ]).shown.map((c) => `${c.kind}:${c.name}`),
    ['head:HEAD', 'localBranch:main', 'remoteBranch:origin/main'],
    'HEAD, then locals, then remote-tracking, then tags — the current branch is the row every '
      + 'other one is relative to, which is why `branchModel::visibleBranches` puts it first too',
  )

  eq(
    m.orderChips([chip('localBranch', 'zeta'), chip('localBranch', 'alpha')]).shown.map((c) => c.name),
    ['alpha', 'zeta'],
    'inside one kind the order is the name, so two runs of one repository draw one order',
  )

  eq(m.MAX_CHIPS, 3, 'three chips, then a count')

  {
    // A release commit that tags every crate in a monorepo. Not hypothetical; it is what the
    // cap exists for, and forty chips would push the subject — the only column anyone scans —
    // off the row entirely.
    const forty = Array.from({ length: 40 }, (_, i) => chip('tag', `v0.18.${i}`))
    const { shown, more } = m.orderChips([chip('head', 'HEAD'), ...forty])
    eq(shown.length, m.MAX_CHIPS, 'forty tags still draw exactly three chips')
    eq(more, 38, 'and the other thirty-eight are counted rather than dropped')
    eq(shown[0].kind, 'head', 'with HEAD surviving the cut, because it sorts first')
  }

  eq(m.orderChips([]).more, 0, 'no chips is no overflow, not `+0`')

  // --- what clicking a chip asks for (M21) -----------------------------------------------------
  //
  // `chipTarget` is the whole decision, and `null` from it is what makes the view render a
  // `<span>` instead of a `<button>` — one answer to "is this a control", not two that can drift.
  // `check-log-render.mjs` owns the claim that the two answers reach the DOM as two elements.
  //
  // The fixture `chip()` above builds `refs/<kind>/<name>`, which is not a refname any repository
  // produces. These use the real spellings `cide_git::log::chips` emits, because the entire point
  // of this function is which of the two strings on a chip goes on the wire.

  {
    const ref = (kind, name, refFull, current = false) => ({ kind, name, full: refFull, current })

    const HEAD_CHIP = ref('head', 'HEAD', 'HEAD', true)
    const MAIN = ref('localBranch', 'main', 'refs/heads/main')
    const ORIGIN_MAIN = ref('remoteBranch', 'origin/main', 'refs/remotes/origin/main')
    const TAG = ref('tag', 'v1.2.0', 'refs/tags/v1.2.0')

    const HEAD = { kind: 'head' }
    const ALL = { kind: 'all' }

    // The four kinds, from the default walk. Three of them are `Rev` on the full refname and the
    // fourth is `Head`, and each of those is a separate argument — see the function's comment.
    eq(
      m.chipTarget(MAIN, HEAD),
      { kind: 'rev', spec: 'refs/heads/main' },
      'a local branch re-roots the walk on `LogRefs::Rev` at its **full refname**',
    )
    eq(
      m.chipTarget(ORIGIN_MAIN, HEAD),
      { kind: 'rev', spec: 'refs/remotes/origin/main' },
      'and so does a remote-tracking one — `Branch { name: "origin/main" }` would go through '
        + '`find_branch_tip`, which tries `refs/heads` first and would find a *local* branch of '
        + 'that name if the repository had one',
    )
    eq(
      m.chipTarget(TAG, HEAD),
      { kind: 'rev', spec: 'refs/tags/v1.2.0' },
      'a tag too, and this is the kind that has no alternative at all: `find_branch_tip` looks '
        + 'only in heads and remotes, so `LogRefs::Branch` for a tag answers `NoSuchRef`',
    )
    eq(
      m.chipTarget(HEAD_CHIP, ALL),
      { kind: 'head' },
      'the HEAD chip re-roots on `LogRefs::Head` and not on `Rev { spec: "HEAD" }`. The two '
        + 'resolve to one commit and are not one value: `isFiltered` counts a `Rev` as a filter, '
        + 'so the second would light up *Clear filters* and push the `<select>` onto its synthetic '
        + '"Revision HEAD" option in front of a walk identical to the default — and '
        + '`seed_from_refs`’s `Head` arm treats an unborn HEAD as an empty page where '
        + '`revparse("HEAD")` is a `BadRevspec` the tab would render as a failure',
    )

    // `full`, never `name`. The type is the first line of defence — `RefChipLike` has no `name`
    // field, so the wrong string is not in scope — and this is the second, because a widened
    // parameter type would silently reopen it.
    for (const chipRef of [MAIN, ORIGIN_MAIN, TAG]) {
      const target = m.chipTarget(chipRef, ALL)
      eq(target.spec, chipRef.full, `${chipRef.kind} sends its full refname`)
      ok(
        target.spec !== chipRef.name,
        `…and never its short name: "${chipRef.name}" is ambiguous, and a walk re-rooted on the `
          + 'wrong ref is a plausible list of real commits off a different branch, which is a bug '
          + 'the user cannot see',
      )
    }
    {
      // The pathological pair git permits and this repository has written down twice: a *local*
      // branch called `origin/main` (`refs/heads/origin/main`) beside the remote-tracking ref of
      // the same name. Both chips draw the string `origin/main`.
      const localNamedLikeARemote = ref('localBranch', 'origin/main', 'refs/heads/origin/main')
      ok(
        m.chipTarget(localNamedLikeARemote, ALL).spec !== m.chipTarget(ORIGIN_MAIN, ALL).spec,
        'two chips that draw the same text ask two different questions. Resolving by name would '
          + 'send both through `find_branch_tip`, which tries `refs/heads` first and would answer '
          + 'the local one for both',
      )
    }

    // The chip of the ref the walk already starts at. Inert, so it renders as text: a control
    // that does nothing when pressed is indistinguishable from a broken one.
    eq(
      m.chipTarget(MAIN, { kind: 'rev', spec: 'refs/heads/main' }),
      null,
      'the chip the walk is already rooted on is not a control',
    )
    eq(
      m.chipTarget(MAIN, { kind: 'branch', name: 'main' }),
      null,
      '…including when the root was chosen from the `<select>` as a `Branch`, which is the same '
        + 'ref by another name. Calling it live would re-root onto `Rev { refs/heads/main }` — the '
        + 'same commits — and degrade the control’s label from "main" to "Revision refs/heads/main"',
    )
    eq(
      m.chipTarget(ORIGIN_MAIN, { kind: 'branch', name: 'origin/main' }),
      null,
      '…and the remote spelling too, mirroring `find_branch_tip`’s local-then-remote lookup, which '
        + 'is what `LogRefs::Branch` actually resolves through',
    )
    eq(
      m.chipTarget(ref('localBranch', 'main', 'refs/heads/main', true), HEAD),
      null,
      'under the default walk it is `current` that decides, and that flag is a fact rather than a '
        + 'guess: `LogRefs::Head` is *defined* as the ref HEAD points at, so the chip carrying '
        + '`current` is the same request spelled a second way',
    )
    eq(m.chipTarget(HEAD_CHIP, HEAD), null, '…which is also why the HEAD chip is inert by default')
    ok(
      m.chipTarget(ORIGIN_MAIN, HEAD) !== null,
      'but a chip merely sitting on the same *commit* stays live. `main` and `origin/main` agree '
        + 'for the minute after a push and disagree after the next fetch; this module holds chips '
        + 'and not oids, and a rule that guessed "same commit, same request" would be wrong '
        + 'silently',
    )
    for (const chipRef of [HEAD_CHIP, MAIN, ORIGIN_MAIN, TAG]) {
      ok(
        m.chipTarget(chipRef, ALL) !== null,
        `under *All branches* the ${chipRef.kind} chip is live — \`LogRefs::All\` seeds every tip, `
          + 'so narrowing to one of them is a real change and no chip is where the walk starts',
      )
    }

    // The `+n` chip. It is not a `RefChip` and never reaches this function; `orderChips` hands
    // back a *count*, so there is no value to pass. The guard is for the next caller.
    eq(
      typeof m.orderChips([chip('tag', 'a'), chip('tag', 'b'), chip('tag', 'c'), chip('tag', 'd')]).more,
      'number',
      'the overflow is a count and not a chip, so there is nothing for `chipTarget` to be given — '
        + 'which is why the view draws it as a `<span>` with no branch of its own',
    )
    eq(
      m.chipTarget(ref('tag', '+38', ''), HEAD),
      null,
      '…and a chip that names no ref is inert rather than sent. `Rev { spec: "" }` is a '
        + '`revparse("")`, which comes back `BadRevspec` and replaces the list with a failure '
        + 'sentence in answer to a click that should have done nothing',
    )

    // Every target has to survive the branch control, which re-renders from `filter.branch` the
    // moment the chip sets it. A value the `<select>` cannot spell renders the control **blank**
    // — the bug the synthetic `rev` option was added for.
    for (const chipRef of [HEAD_CHIP, MAIN, ORIGIN_MAIN, TAG]) {
      const target = m.chipTarget(chipRef, ALL)
      eq(
        m.branchFromValue(m.branchValue(target)),
        target,
        `the ${chipRef.kind} chip’s target round-trips through the branch control`,
      )
    }

    // The tooltip. Derived from `chipTarget` rather than from a second predicate, so it cannot
    // claim one thing while the click does another.
    eq(
      m.chipTitle(MAIN, HEAD),
      'refs/heads/main\nStart the log here.',
      'a live chip keeps the refname as its first line — the disambiguation it carries `full` for '
        + '— and says what pressing it does on the second',
    )
    eq(
      m.chipTitle(HEAD_CHIP, ALL),
      'HEAD\nStart the log at the current checkout.',
      'the HEAD chip is the way *back* and says so: "start the log here" would be wrong for the '
        + 'one chip that means "return to the default"',
    )
    eq(
      m.chipTitle(MAIN, { kind: 'rev', spec: 'refs/heads/main' }),
      'refs/heads/main\nThe log already starts here.',
      'and an inert one says why. A `<span>` and a `<button>` differ by a cursor and a hover '
        + 'underline and by nothing at all in a screenshot',
    )
  }

  // --- dates: absolute, and sane at the boundaries --------------------------------------------

  {
    const thisYear = m.when(at(2026, 3, 12), NOW)
    const older = m.when(at(2019, 11, 2), NOW)
    ok(!/2026/.test(thisYear), 'a commit from this year omits the year — it is noise on every row')
    ok(/2019/.test(older), 'an older one prints it, because "02 Nov" alone is a different claim')
    ok(thisYear !== '' && older !== '', 'and neither is empty, which is what a `NaN` date renders as')
    ok(!/Invalid/.test(thisYear + older), 'nor "Invalid Date", which is what a raw bigint produces')
  }

  {
    // Clock skew on a rebase: `git commit --date` and a machine with a wrong clock both produce
    // a commit timestamped in the future. The mock this feature was drawn from showed "2h ago",
    // which for this row renders as "-3m" or "in 40 days" depending on the library.
    const future = BigInt(Math.floor(NOW / 1000) + 86400 * 40)
    const rendered = m.when(future, NOW)
    ok(!/ago|in \d|^-/.test(rendered), 'a future timestamp degrades to a date, never to "in 40 days"')
    eq(
      rendered,
      m.when(future, Number(future) * 1000),
      'and the same timestamp renders identically however far "now" is from it, which is what '
        + '"absolute, not relative" *means* — `ShelfList.tsx:8-10` sets that house rule and gives '
        + 'the reason: a relative string has to be recomputed to stay true',
    )
  }

  eq(
    m.when(at(2026, 3, 12), NOW),
    m.when(Number(at(2026, 3, 12)), NOW),
    'a bigint and a number are the same date — the wire sends the first and every fixture the '
      + 'second, and a `Number()` forgotten at one call site is a silent "Invalid Date"',
  )

  ok(
    m.rowTitle(ROWS[0]).startsWith(full('a1b2c3d')),
    'the tooltip leads with the full oid, because the short one on screen has to be resolvable',
  )
  ok(
    m.rowTitle(ROWS[0]).includes('<ivan@example.com>'),
    'and carries the address, because two people share a name far more often than an address',
  )

  // --- paging ---------------------------------------------------------------------------------

  {
    // The walk can legitimately repeat a row across a page boundary under real clock skew — a
    // descendant committed with an earlier timestamp than its ancestor. `cide_git::log` bounds
    // that with a recent-oid ring and cannot eliminate it without carrying every emitted oid.
    const first = m.mergePage([], page())
    const second = m.mergePage(first, page({ commits: [ROWS[2], row('f00d123', 'older still', 'Ivan', at(2018, 5, 1))] }))
    eq(second.length, 4, 'a row repeated across a page boundary is merged once, not twice')
    eq(
      new Set(second.map((r) => r.oid)).size,
      second.length,
      'so no two rows share a React key — a duplicate key is a list that silently stops updating',
    )
    eq(second.map((r) => r.shortOid), ['a1b2c3d', 'deadbee', 'c0ffee1', 'f00d123'], 'and order is kept')
  }

  eq(m.hasMore(null), false, 'no page is not "more"')
  eq(m.hasMore(page()), false, 'a null resume is the only honest way to say there is no more')
  eq(m.hasMore(page({ resume: { version: 1 }, stop: 'exhausted' })), true, '…and `stop` is never it')

  // --- the foot of the list ---------------------------------------------------------------------

  {
    const label = m.moreLabel(page({ stop: 'budget', scanned: 20000 }))
    ok(/^Searched /.test(label), '`budget` says how far the walk got')
    ok(label.includes((20000).toLocaleString()), '…with the count, grouped as the locale groups it')
    ok(/keep looking/.test(label), '…and offers to continue')
    ok(
      !/no more|end of|nothing/i.test(label),
      'and above all it does not say the history is over. `budget` is the *expected* outcome of a '
        + 'narrow filter over a deep repository, and a list that drew it as the end would silently '
        + 'lose the answer the user was looking for — which is the whole reason the budget exists',
    )
  }

  ok(
    /shallow/i.test(m.moreLabel(page({ stop: 'shallow' }))),
    'a shallow clone says so, rather than presenting its graft point as the project’s first commit',
  )
  eq(m.moreLabel(page({ stop: 'page' })), 'Load more', 'an ordinary short page is just "Load more"')

  // --- every empty state says something, and no two say the same thing ---------------------------

  const base = {
    loading: false,
    failed: null,
    noRepo: false,
    rows: 0,
    filtered: false,
    path: null,
    stop: 'exhausted',
  }

  const STATES = {
    failed: { ...base, failed: 'could not read .git/HEAD: Permission denied' },
    noRepo: { ...base, noRepo: true },
    loading: { ...base, loading: true, stop: null },
    noSuchRef: { ...base, stop: 'noSuchRef' },
    cursorLost: { ...base, stop: 'cursorLost' },
    budget: { ...base, stop: 'budget' },
    shallow: { ...base, stop: 'shallow' },
    filtered: { ...base, filtered: true },
    path: { ...base, path: 'crates/cide-git/src/log.rs' },
    unborn: { ...base },
  }

  const sentences = Object.entries(STATES).map(([name, input]) => [name, m.logStatus(input)])
  for (const [name, sentence] of sentences) {
    ok(typeof sentence === 'string' && sentence.length > 0, `${name} produces a sentence`)
  }
  eq(
    new Set(sentences.map(([, s]) => s)).size,
    sentences.length,
    'and all ten are different. Two states that say the same thing are indistinguishable to the '
      + 'user, and the difference between them is the difference between waiting, clearing a '
      + 'filter, scrolling further and making the first commit',
  )

  eq(m.logStatus(STATES.filtered), 'No commit matches these filters.', 'the filtered sentence, verbatim')
  eq(m.logStatus(STATES.loading), 'Reading the log…', 'the loading sentence, verbatim')
  ok(/no git repository/.test(m.logStatus(STATES.noRepo)), 'a project with no repository says so')
  ok(
    m.logStatus(STATES.budget) !== m.logStatus(STATES.filtered),
    '`budget` outranks the filter, because "no commit matches" is a claim about the whole history '
      + 'and a budgeted walk has not read it',
  )
  ok(
    m.logStatus({ ...STATES.budget, filtered: true, path: 'a.rs' }) === m.logStatus(STATES.budget),
    '…and it outranks the path sentence for the same reason, whichever of them is also set',
  )
  eq(
    m.logStatus({ ...base, failed: 'boom', rows: 12 }),
    'boom',
    'a failure is shown even with rows on screen: those rows are from before the failure and '
      + 'silently keeping them is how a stale list looks live',
  )

  // The exhaustive half: a list with no rows is never silent, and a list with rows never shouts.
  {
    let silentEmpty = 0
    let noisyFull = 0
    for (const loading of [false, true]) {
      for (const failedText of [null, 'boom']) {
        for (const noRepo of [false, true]) {
          for (const rows of [0, 7]) {
            for (const filtered of [false, true]) {
              for (const path of [null, 'a.rs']) {
                for (const stop of [null, 'exhausted', 'page', 'budget', 'shallow', 'cursorLost', 'noSuchRef']) {
                  const answer = m.logStatus({ loading, failed: failedText, noRepo, rows, filtered, path, stop })
                  if (rows === 0 && answer === null) silentEmpty += 1
                  if (rows > 0 && failedText === null && !noRepo && answer !== null) noisyFull += 1
                }
              }
            }
          }
        }
      }
    }
    eq(silentEmpty, 0, 'no combination leaves an empty list with nothing on it — "ready with zero rows" is the state this table exists to make unrepresentable')
    eq(noisyFull, 0, 'and a list that has rows shows them rather than a sentence over the top of them')
  }

  // --- the graph's five off-reasons ---------------------------------------------------------------

  {
    const reasons = ['filtered', 'fullHistory', 'merged', 'rerooted']
    const said = reasons.map((r) => m.graphOffReason(r))
    for (const [i, sentence] of said.entries()) {
      ok(typeof sentence === 'string' && sentence.length > 0, `graphOffReason(${reasons[i]}) says something`)
    }
    eq(new Set(said).size, said.length, 'and no two reasons share a sentence')
    eq(
      m.graphOffReason('disabled'),
      null,
      '`disabled` is the one the caller chose, and explaining a switch back to the person who '
        + 'flipped it is noise',
    )
  }

  // --- the ref a merged scope could not resolve -----------------------------------------------

  {
    const filter = { ...m.NO_FILTER, branch: { kind: 'branch', name: 'release/0.18' } }
    const merged = page({
      repos: [
        { repo: 'r1', name: 'cide', stop: 'exhausted', scanned: 6 },
        { repo: 'r2', name: 'hub-core', stop: 'exhausted', scanned: 4 },
        { repo: 'r3', name: 'docs-site', stop: 'noSuchRef', scanned: 0 },
      ],
      stop: 'noSuchRef',
    })
    eq(m.missingRef(merged), ['docs-site'], 'the root that could not resolve the ref, by name')
    eq(
      m.missingRefNote(merged, filter),
      'No “release/0.18” in docs-site.',
      'and the sentence names it. `cide_git::log` deliberately does not refuse the whole query — '
        + 'refusing would make the branch filter useless in exactly the monorepo it exists for — '
        + 'so the page has rows, `logStatus` is null, and this is the only place it can be said',
    )
    eq(m.missingRefNote(page(), filter), null, 'nothing to say when every root resolved it')
    eq(m.missingRefNote(null, filter), null, 'and nothing to say before the first page arrives')
    ok(
      m
        .missingRefNote(
          page({
            repos: [
              { repo: 'r2', name: 'hub-core', stop: 'noSuchRef', scanned: 0 },
              { repo: 'r3', name: 'docs-site', stop: 'noSuchRef', scanned: 0 },
            ],
          }),
          filter,
        )
        .includes('hub-core and docs-site'),
      'two of them are joined rather than listed twice',
    )
  }

  // --- the filter bar ------------------------------------------------------------------------

  eq(m.NO_FILTER, { branch: { kind: 'head' }, author: '', text: '' }, 'the unfiltered value')
  eq(m.isFiltered(m.NO_FILTER), false, 'and it is not a filter')
  eq(m.isFiltered({ ...m.NO_FILTER, author: '  ' }), false, 'whitespace is not a filter either')
  eq(m.isFiltered({ ...m.NO_FILTER, text: 'fix' }), true, 'text is')
  eq(m.isFiltered({ ...m.NO_FILTER, author: 'ivan' }), true, 'an author is')
  eq(m.isFiltered({ ...m.NO_FILTER, branch: { kind: 'all' } }), true, 'and so is re-rooting the walk')

  {
    const set = { branch: { kind: 'all' }, author: 'ivan', text: 'fix' }
    eq(m.clearFilters(set), m.NO_FILTER, 'Clear clears all three, the branch included')
    ok(
      m.clearFilters(m.NO_FILTER) === m.NO_FILTER,
      'and clearing an already-clear bar returns the *same object*, so `LogTab`’s reference '
        + 'comparison does not start a walk that returns the page already on screen',
    )
  }

  eq(m.sameFilter(m.NO_FILTER, { ...m.NO_FILTER }), true, 'two clear filters ask the same question')
  eq(
    m.sameFilter({ ...m.NO_FILTER, branch: { kind: 'branch', name: 'a' } }, { ...m.NO_FILTER, branch: { kind: 'branch', name: 'b' } }),
    false,
    'two branches do not',
  )
  eq(m.sameBranch({ kind: 'head' }, { kind: 'all' }), false, 'and neither do two kinds')

  // The `<select>` round trip. A branch may legitimately be called `head` or `all`, and a bare
  // name as the option value would silently re-root onto HEAD for either of them.
  for (const refs of [
    { kind: 'head' },
    { kind: 'all' },
    { kind: 'branch', name: 'main' },
    { kind: 'branch', name: 'head' },
    { kind: 'branch', name: 'all' },
    { kind: 'branch', name: 'feature/login' },
    { kind: 'rev', spec: 'v1.0..HEAD' },
  ]) {
    eq(
      m.branchFromValue(m.branchValue(refs)),
      refs,
      `the branch control round-trips ${JSON.stringify(refs)}`,
    )
  }
  eq(m.branchFromValue('nonsense'), { kind: 'head' }, 'an unknown value falls back to HEAD, never throws')

  // --- the local narrow and the wire query, which must not be able to diverge -------------------

  eq(
    Object.keys(m.filterOverrides(m.NO_FILTER)).sort(),
    ['author', 'refs', 'text'],
    'the bar sets exactly three fields of the query. A fourth appearing here would be spread over '
      + '`logQuery`’s default and change a query nobody asked to change',
  )
  eq(
    m.filterOverrides(m.NO_FILTER),
    { refs: { kind: 'head' }, author: null, text: null },
    'an empty box is `null` and not `""` — the two mean the same thing to `cide_git::log::needle`, '
      + 'but `LogResume`’s query fingerprint hashes the field, so two spellings of one question '
      + 'would produce two incompatible resume tokens',
  )
  eq(
    m.filterOverrides({ branch: { kind: 'all' }, author: '  Ivan  ', text: '  fix  ' }),
    { refs: { kind: 'all' }, author: 'Ivan', text: 'fix' },
    'and the needles are trimmed exactly once, here, on the way to the wire',
  )

  {
    // The agreement. What can be asserted is that the needle the wire is asked for and the needle
    // the local pass matches on are derived the same way, and that the two divergences are
    // exactly the two `matchesText` documents.
    const needles = ['', '   ', 'pane', 'PANE', 'Pane', 'a1b2', 'a1b2c3d', 'dead', 'eadb', 'ivan', 'zzz']
    for (const needle of needles) {
      const over = m.filterOverrides({ ...m.NO_FILTER, text: needle })
      const blank = needle.trim() === ''
      eq(over.text, blank ? null : needle.trim(), `the wire gets the trimmed needle for ${JSON.stringify(needle)}`)
      for (const r of ROWS) {
        if (blank) {
          ok(m.matchesText(r, needle), 'a blank box keeps every row, exactly as a null needle does')
        }
      }
    }

    ok(m.matchesText(ROWS[2], 'pane'), 'a summary substring matches')
    eq(
      m.matchesText(ROWS[2], 'PANE'),
      m.matchesText(ROWS[2], 'pane'),
      'case-insensitively, as `contains_ci` is',
    )
    ok(!m.matchesText(ROWS[2], 'zzz'), 'and a needle in nothing matches nothing')

    // The oid rule, mirrored from `cide_git::log`: a prefix of the **full** oid, gated at four
    // hex digits, and never a substring.
    ok(m.matchesText(ROWS[1], 'dead'), 'four hex digits match as an oid prefix')
    ok(m.matchesText(ROWS[1], 'deadbee'), '…and so does the whole short oid')
    ok(!m.matchesText(ROWS[1], 'eadb'), 'but the middle of a hash does not — the backend does `starts_with`')
    ok(
      !m.matchesText(row('abc1234', 'nothing here', 'Nobody', at(2026, 1, 1)), 'abc'),
      'and three hex digits are below git’s own floor for an abbreviation, so they are not an oid '
        + 'search at all — the backend gates at four and this gates at four',
    )

    // The two documented one-way gaps, asserted so that removing either one fails here rather
    // than being discovered as a row that flickers.
    ok(
      !m.matchesText(ROWS[0], 'ivan'),
      'the local pass does NOT match the author, and this is the invariant the whole rule is '
        + 'shaped by: the wire’s `text` greps the message only, so matching the author here '
        + 'would make the local pass *wider* and a row kept on a name would DISAPPEAR a fifth of '
        + 'a second later when the answer lands. Every other gap leans the other way and only '
        + 'ever adds rows. The author box is how you ask that question, durably',
    )
    ok(
      !m.matchesText(row('bbbb111', 'Fix the thing', 'Nobody', at(2026, 1, 1)), 'ticket-4471'),
      'and the local pass cannot see a body, so a commit found by its message body arrives with '
        + 'the wire answer and not before — the list only ever grows when the answer lands, which '
        + 'is the safe direction',
    )
  }

  {
    // Narrowing on a word that is in a *subject*, not on an author name — see above.
    const narrowed = m.applyLocalText(ROWS, m.matchesText(ROWS[0], ROWS[0].summary.split(' ')[0])
      ? ROWS[0].summary.split(' ')[0]
      : '')
    ok(narrowed.length > 0 && narrowed.length <= ROWS.length, 'the local pass narrows in place')
    ok(
      narrowed.every((r) => m.matchesText(r, ROWS[0].summary.split(' ')[0])),
      'and keeps exactly the rows that match',
    )
    eq(m.applyLocalText(ROWS, '   ').length, ROWS.length, 'a blank box narrows nothing')
    ok(m.applyLocalText(ROWS, '') !== ROWS, '…and still copies, so nothing downstream aliases the page')
  }

  eq(m.FILTER_DEBOUNCE_MS, 200, 'the debounce, which `LogTab` imports rather than restating')
  {
    const tab = stripComments(readFileSync(join(UI, 'src', 'gitlog', 'LogTab.tsx'), 'utf8'))
    ok(
      tab.includes('FILTER_DEBOUNCE_MS'),
      'and `LogTab` really uses it — a second literal is how the model and the wiring end up with '
        + 'two different debounces and only one of them documented',
    )
    ok(
      !/\b200\b/.test(tab.replace(/FILTER_DEBOUNCE_MS/g, '')),
      '…with no stray copy of the number left beside it',
    )
  }

  // --- refreshing when git moves underneath ------------------------------------------------------
  //
  // The predicate that decides whether a coalesced filesystem burst is worth a fresh `git_log`
  // and a fresh `git blame` of every annotated buffer. It is narrower than `FsChange.git` on
  // purpose — that flag covers the index — and every one of the four cases below is a way for
  // this feature to be either useless or a background process nobody asked for.

  {
    const burst = (over = {}) => ({ paths: [], truncated: false, git: true, ...over })
    const under = (name) => `/home/dev/work/cide/.git/${name}`

    eq(
      m.gitRefsMoved(burst({ paths: [under('index')] })),
      false,
      'a `git add` rewrites `.git/index` and moves no ref, so it must NOT restart the walk. This '
        + 'is the case the whole predicate exists for: `git status` rewrites the index too, and an '
        + 'editor plugin or a `watch -n1 git status` runs it in a loop — answering each one with a '
        + 'frontier walk over a hundred thousand commits is a background process the user did not '
        + 'ask for, and it throws away the page they are reading each time',
    )
    eq(
      m.gitRefsMoved(burst({ paths: [under('index'), under('index.lock')] })),
      false,
      '…and the lock file counts as the index, because it is half of every index write: git '
        + 'creates it, writes it and renames it over `index`, so which of the two survives '
        + 'coalescing is not something the answer may depend on',
    )

    eq(
      m.gitRefsMoved(burst({ paths: [under('index'), under('refs/heads/main')] })),
      true,
      'a `git commit` writes both, and the ref is what matters — one non-index git path in the '
        + 'burst is a ref move however many index writes came with it',
    )
    for (const name of ['HEAD', 'ORIG_HEAD', 'FETCH_HEAD', 'packed-refs', 'refs/heads/feature/login', 'refs/remotes/origin/main', 'refs/tags/v0.18.0']) {
      eq(
        m.gitRefsMoved(burst({ paths: [under(name)] })),
        true,
        `\`${name}\` is a ref move — checkout, rebase, pull, gc-ed repository, branch, fetch and `
          + 'tag respectively, and every one of them changes what a log shows',
      )
    }

    eq(
      m.gitRefsMoved(burst({ paths: [under('index')], truncated: true })),
      true,
      'a TRUNCATED burst counts as a ref move whatever survived in the list. `FsChange.truncated` '
        + 'means the paths are a prefix and the receiver should re-read what it cares about — '
        + 'reading the prefix and concluding "index only" is exactly how a `git commit` inside a '
        + '`cargo build` goes unnoticed',
    )
    eq(
      m.gitRefsMoved(burst({ paths: [], truncated: true })),
      true,
      '…including a truncated burst with no paths left at all',
    )

    eq(
      m.gitRefsMoved(burst({ git: false, paths: [under('refs/heads/main')] })),
      false,
      '`git: false` is the one hard false, whatever the paths say. The flag is the watcher’s own '
        + 'answer — `Filter::is_git_path` over the list it actually watches — and second-guessing '
        + 'it from a path spelling here would be a second implementation of it',
    )
    eq(
      m.gitRefsMoved(burst({ git: false, paths: [], truncated: true })),
      false,
      '…and it outranks `truncated` too, so a big non-git burst is not a log refresh',
    )

    eq(
      m.gitRefsMoved(burst({ paths: ['/home/dev/work/cide/src/main.rs'] })),
      true,
      'the flag set with no `.git` path in the list degrades TOWARDS refreshing. The two disagree '
        + 'only when this rule’s path parsing missed something the watcher saw — a `$GIT_DIR` '
        + 'elsewhere, a separator this did not think of — and a refresh nobody needed costs one '
        + 'walk while one that never happens is the stale panel this exists inside',
    )
    eq(
      m.gitRefsMoved(burst({ paths: ['/home/dev/work/cide/src/main.rs', under('index')] })),
      false,
      '…but an ordinary edit riding along with an index write is still index-only. A burst carries '
        + 'both kinds of path and only the git ones may be classified — reading `src/main.rs` as '
        + '"not the index, therefore a ref" would make every save during a `git add` a refresh',
    )
    eq(
      m.gitRefsMoved(burst({ paths: ['/home/dev/work/cide/src/dotgit/index'] })),
      true,
      'a `.git` **segment** and not a substring: `src/dotgit/index` is not in a git directory, so '
        + 'it is not a git path at all and cannot be the thing that makes a burst index-only',
    )
    eq(
      m.gitRefsMoved(burst({ paths: [under('worktrees/agent/index')] })),
      false,
      'a linked worktree’s own index is still an index. `repo::watch_dirs` returns both the '
        + 'worktree and the common directory, so both spellings arrive here',
    )
    eq(
      m.gitRefsMoved(burst({ paths: ['C:\\work\\cide\\.git\\index'] })),
      false,
      'and the separator is not assumed. The wire carries whatever `PathBuf` printed; a rule that '
        + 'silently answered "no git path here" on Windows would turn this refresh off rather '
        + 'than fail',
    )
  }

  // --- revealing one commit -----------------------------------------------------------------------

  {
    const OID = 'e5f6a71' + '0'.repeat(33)

    eq(m.NO_REVEAL, { kind: 'none' }, 'the resting state')
    eq(m.revealNote(m.NO_REVEAL), null, 'which says nothing')
    eq(
      m.revealNote({ kind: 'found', oid: OID }),
      null,
      'and neither does a reveal that landed: the row is selected and scrolled to, which IS the '
        + 'answer — a banner repeating it would be noise on the one path that worked',
    )

    const said = ['looking', 'outside', 'missing'].map((kind) => m.revealNote({ kind, oid: OID }))
    for (const [i, sentence] of said.entries()) {
      ok(
        typeof sentence === 'string' && sentence.length > 0,
        `the ${['looking', 'outside', 'missing'][i]} state says something`,
      )
      ok(sentence.includes('e5f6a71'), '…and names the commit, abbreviated as the rows abbreviate it')
      ok(!sentence.includes(OID), '…and not as forty characters, which does not fit a sentence')
    }
    eq(
      new Set(said).size,
      3,
      'three different sentences. "still looking", "it is not in this list" and "it is not there '
        + 'at all" are three different next moves, and the whole argument in `logStatus`’s header '
        + 'applies again here',
    )

    eq(
      m.revealFindable({ kind: 'outside', oid: OID }),
      true,
      'only the out-of-filter state offers *Clear filters and find it* — the commit is known to '
        + 'exist, so re-rooting the walk at it will certainly find it',
    )
    for (const kind of ['none', 'looking', 'found', 'missing']) {
      eq(
        m.revealFindable({ kind, oid: OID }),
        false,
        `and ${kind} does not: it would be a control that either cannot work or races the answer`,
      )
    }

    eq(
      m.findCommitFilter(OID),
      { branch: { kind: 'rev', spec: OID }, author: '', text: '' },
      'the button clears both boxes AND re-roots the walk at the commit. Clearing alone would not '
        + 'be enough and would look like it should be: the commit is below the page, not hidden by '
        + 'a filter, so an unfiltered page one still starts at HEAD and lands the user back on the '
        + 'note they just pressed',
    )
    eq(m.isFiltered(m.findCommitFilter(OID)), true, '…and that is itself a filter, so *Clear filters* stays offered')
    eq(
      m.branchFromValue(m.branchValue(m.findCommitFilter(OID).branch)),
      { kind: 'rev', spec: OID },
      '…and it round-trips through the branch control, which is what stops the `<select>` going '
        + 'blank the moment the button beside it is pressed',
    )

    eq(m.shortenOid(OID), 'e5f6a71', 'seven characters, as `CommitRow.shortOid` and `BlameCommit.shortOid` are')
    eq(
      m.shortenOid('v1.0..HEAD'),
      'v1.0..HEAD',
      'and anything that is not forty hex digits is left alone — "v1.0..H" is not an abbreviation '
        + 'of that revision expression, it is a different one',
    )
    ok(m.revLabel(OID).includes('e5f6a71'), 'the branch control names a re-rooted walk by its short oid')
    ok(!m.revLabel(OID).includes(OID), '…and not by forty characters in a 130px control')
  }

  // --- the wiring, over stripped source ------------------------------------------------------------
  //
  // Every bug in this round was a *missing call* rather than a wrong function: a flag nobody read,
  // a store function nobody invoked, a command that existed and was never reached. A model's own
  // assertions cannot see one, so the call sites are pinned here — the same arrangement
  // `check-editor.mjs` uses for the same class of failure.

  {
    const tab = stripComments(readFileSync(join(UI, 'src', 'gitlog', 'LogTab.tsx'), 'utf8'))
    const store = stripComments(readFileSync(join(UI, 'src', 'editor', 'blameStore.ts'), 'utf8'))
    const pane = stripComments(readFileSync(join(UI, 'src', 'panes', 'EditorPane.tsx'), 'utf8'))

    ok(
      /onFsChanged\(/.test(tab) && /gitRefsMoved\(change\)/.test(tab),
      'the Log tab subscribes to `cide://fs-changed` and asks `gitRefsMoved` — the flag had no '
        + 'consumer at all for a whole milestone, and a predicate with no caller is the same bug '
        + 'one layer up',
    )
    ok(
      /onFsChanged\(/.test(store) && /gitRefsMoved\(change\)/.test(store),
      '…and so does the blame store, which is the other half: a `git commit` in a bash pane has to '
        + 'move the gutter as well as the list',
    )
    ok(
      /refreshBlame|sweep\(projects\)/.test(store),
      '…and the subscription really sweeps the annotated buffers, rather than being a listener '
        + 'that observes and does nothing',
    )
    for (const [source, name] of [[tab, 'LogTab'], [store, 'blameStore']]) {
      ok(
        /if \((?:git)?[Tt]ick(?:\.current)? !== null\) return/.test(source),
        `${name} throttles rather than debounces: the timer is skipped while one is armed and `
          + 'never restarted on a new trigger. A debounce is starved indefinitely by a steady '
          + 'writer — a `cargo watch`, a fetch loop — and this project has written that trap down '
          + 'twice already (`gitCountStore`, `docSync`)',
      )
    }

    ok(
      /export function requestLogReveal\(/.test(tab),
      'the log exports a reveal seam. Without one a blame click could only open the panel at HEAD, '
        + 'which after a click on a 2019 line reads as broken',
    )
    ok(
      /requestLogReveal\(/.test(pane),
      '…and `EditorPane` uses it, rather than opening the tool window and stopping there',
    )
    ok(
      !/Showing the log/.test(pane),
      'and the notice that stood in for it is GONE. It named the oid so the user could finish the '
        + 'gesture by hand; leaving it beside a working reveal would be a toast on every blame '
        + 'click saying what the panel is already showing',
    )
    /*
     * Retargeting is GONE from this list, and that reverses what stood here in M20.
     *
     * It read: *"a single click in a commit's file list can retarget… which is thirty tabs for
     * one walk down a forty-file commit"*. True, and it solved the wrong problem — the fix for
     * "walking the list opens thirty tabs" is that walking the list opens **none**, which is what
     * was asked for twice. `tab_retarget_revision_diff` still exists and is still called from the
     * working-tree side; nothing here calls it.
     */
    ok(
      /logFileClick\(/.test(tab),
      'the changed-file list routes through `logFileClick`, the log’s own rule, and not the git '
        + 'tree’s conditional one',
    )
    ok(
      !/onDoubleClick/.test(tab),
      'and the old `onDoubleClick` is gone. The browser fires `click` twice for a double-click '
        + '(detail 1, then 2) AND fires `dblclick`, so keeping both handlers would open the tab '
        + 'twice for one gesture',
    )

    // The ref chips, whose whole decision is in this file and whose *wiring* is not. (M21)
    const view = stripComments(readFileSync(join(UI, 'src', 'gitlog', 'LogView.tsx'), 'utf8'))

    // --- a refresh keeps the depth the reader paged to -------------------------------------------
    //
    // > *"'Load more' in git panel not working, it cycle me through same commits, i'm not able to
    // > list to first commit."*
    //
    // The soft refresh — added to stop the list flickering under a tree several agents are
    // writing to — re-asked the question and *replaced* the rows with page one. Right for one
    // page, silently destructive for five: every refresh threw away everything the reader had
    // paged to, and `gitRefsMoved` fires on any truncated burst, so on a busy tree the list
    // snapped back to the top between clicks. Nothing in the suite could see it, because both
    // halves are correct on their own.

    ok(
      /load\(false, rowsRef\.current\.length\)/.test(tab),
      'a soft refresh re-walks to the number of rows already on screen, not to the first page',
    )
    ok(
      /rowsRef\.current = rows/.test(tab) && !/\[project, scope, path, applied, refreshes, rows\]/.test(tab),
      '…read through a ref, and `rows` is NOT in the refresh effect\u2019s dependency list. It '
        + 'would make every *Load more* immediately refresh everything it had just appended',
    )
    ok(
      /keepDepth > 0 \? \{ limit: keepDepth \}/.test(tab),
      'and the depth reaches the query as `limit`, left at zero otherwise so Rust\u2019s own '
        + '`LIMIT_DEFAULT` stays the single authority for a first load and for *Load more*',
    )

    // --- a History tab shows one file's diff, not the commit's file list (M21) --------------------
    //
    // > *"'Show history for this file' … right side prints file tree with all changes of commit
    // > but it should show diff only for this file"*
    //
    // The tab was opened having already named the path, so the commit's other thirty files
    // answer a question nobody asked and put the one they did behind a click.

    ok(
      /<RevisionDiffPane/.test(tab),
      'a History tab draws `RevisionDiffPane` \u2014 the same component a revision diff tab uses, '
        + 'which is why it grew an export. It already fetches one file\u2019s hunks, is read-only, '
        + 'carries the unified/split toggle the request asks for, and handles `oldPath` so a '
        + 'rename diffs against the right blob',
    )
    ok(
      /if \(path === null \|\| selectedRepo === null \|\| selected === null\) return null/.test(tab),
      '\u2026chosen on `path`, which IS what makes a tab a History tab \u2014 it is the field the '
        + 'query differs by, so no second flag can disagree with it',
    )
    ok(
      /compare\?\.next \?\? \(\{ kind: 'commit', oid: selected \}/.test(tab),
      'and a two-row selection diffs the file BETWEEN those commits rather than showing the '
        + 'newer one\u2019s own change \u2014 the reading a pair already means everywhere else in '
        + 'this panel, and the useful one for a tab that is about one file across time',
    )

    // --- one click selects, two open (M21) -------------------------------------------------------
    //
    // Reported twice. The first fix threaded a selection through and kept `gitTreeClick`, whose
    // rule opens on a single click whenever a diff is already on screen — so the first
    // double-click opened a tab and every click afterwards silently replaced it. The second
    // report is what settled that the rule itself was wrong for this surface, not merely
    // misapplied, which is why the log now has its own.

    eq(
      cs.logFileClick({ gesture: 'single', expandable: false }),
      { select: true, toggle: false, open: false },
      'a single click on a file selects and does NOT open. Unconditional — there is no "unless a '
        + 'diff is already up" here, which is exactly where the git tree differs',
    )
    eq(
      cs.logFileClick({ gesture: 'double', expandable: false }),
      { select: true, toggle: false, open: true },
      'and a double-click opens, having already selected on its first half',
    )
    eq(
      cs.logFileClick({ gesture: 'single', expandable: true }),
      { select: true, toggle: true, open: false },
      'a directory heading folds on a single click',
    )
    eq(
      cs.logFileClick({ gesture: 'double', expandable: true }),
      { select: false, toggle: false, open: false },
      '\u2026and does nothing on the second half, or the double-click would fold it and put it '
        + 'straight back',
    )
    ok(
      !/gitTreeClick/.test(tab),
      'and `LogTab` no longer reaches for `gitTreeClick`. Leaving the import would leave the '
        + 'conditional rule one edit away from coming back',
    )
    ok(
      !/retargetTab/.test(tab),
      '\u2026nor for `retargetTab`. `diffOpenMode` maps *single* to retarget and a single click '
        + 'no longer opens anything, so the branch was unreachable code that looked load-bearing',
    )

    // --- the indent is the house number ----------------------------------------------------------

    const tree = readFileSync(join(UI, 'src', 'sidebar', 'FileTree.tsx'), 'utf8')
    const houseIndent = Number(/const INDENT = (\d+)/.exec(tree)?.[1])
    eq(
      fr.INDENT,
      houseIndent,
      `the log's file list indents by the same ${houseIndent}px the file tree does, read out of `
        + '`FileTree.tsx` rather than copied. IDEA indents both its trees by one number and ours '
        + 'used two (12 and 14) until somebody measured; a third invented here would be the same '
        + 'mistake again',
    )
    ok(
      /marginLeft: depth \* INDENT/.test(view),
      'and it is applied to a slot every row has, files included \u2014 not as row padding. A '
        + 'file with no twisty slot lands its icon where a directory\u2019s twisty is, so its '
        + 'label lines up exactly with the directory\u2019s label and the nesting disappears',
    )
    ok(
      !/fileRowNested/.test(view),
      '\u2026and the padding-based version is gone rather than left beside the working one',
    )

    // --- folding a directory (M21) -----------------------------------------------------------------

    const NESTED = [
      { path: 'ui/src/a.ts', oldPath: null, counts: null },
      { path: 'ui/src/b.ts', oldPath: null, counts: null },
      { path: 'crates/x/y.rs', oldPath: null, counts: null },
      { path: 'README.md', oldPath: null, counts: null },
    ]
    const shut = new Set(['ui/src'])
    eq(
      fr.groupedRows(NESTED, shut).map((r) => `${r.kind}:${r.label}`),
      ['dir:ui/src', 'dir:crates/x', 'file:y.rs', 'file:README.md'],
      'a folded directory keeps its heading and loses its files. Dropping the heading too would '
        + 'leave no way to unfold it',
    )
    eq(
      fr.groupedRows(NESTED, shut).find((r) => r.kind === 'dir')?.count,
      2,
      'the heading carries how many it is hiding \u2014 a silent folded row is indistinguishable '
        + 'from an empty directory',
    )
    ok(
      fr.groupedRows(NESTED, shut).some((r) => r.label === 'README.md'),
      'a root-level file is not folded by anything: it has no heading, so folding it would hide '
        + 'it behind a row that does not name it',
    )
    eq(
      fr.fileRows(NESTED, false, shut).length,
      NESTED.length,
      'and the FLAT reading ignores the set entirely. There are no headings there, so honouring '
        + 'it would make files vanish from a list offering no way to bring them back',
    )
    eq(
      fr.groupedRows(NESTED, new Set(['nope'])).length,
      fr.groupedRows(NESTED).length,
      'an entry naming a directory this commit does not touch is ignored, not an error \u2014 the '
        + 'set outlives the commit it was built against, by design',
    )
    eq([...fr.toggleCollapsed(new Set(), 'a')], ['a'], 'toggling folds')
    eq([...fr.toggleCollapsed(new Set(['a']), 'a')], [], '\u2026and unfolds')
    eq(
      [...fr.toggleCollapsed(new Set(['a']), 'b')].sort(),
      ['a', 'b'],
      '\u2026leaving the others alone, and returning a NEW set so a `useState` setter sees a change',
    )

    // --- the details pane reads top-down: files, then the message (M21) ------------------------
    //
    // A source check, because the order lives in `LogTab`'s own `Details` and that component
    // cannot be server-rendered — it reaches the IPC client and the workspace store, which is why
    // `check-log-render.mjs` renders the shared list directly instead.
    //
    // The order matters for a reason a screenshot would not show: the list is the part that is
    // clicked, arrowed through and right-clicked, and it is the only part whose length is
    // unbounded. Under a message of unknown height its first row started somewhere different for
    // every commit, so the row under the pointer moved whenever the selection did.

    const listAt = tab.indexOf('<ChangedFileList')
    const messageAt = tab.indexOf('data-audit="logCommitMessage"')
    ok(listAt !== -1 && messageAt !== -1, 'the details pane draws both halves')
    ok(
      listAt < messageAt,
      'the file list comes FIRST and the commit message after it. Reversing these puts an '
        + 'unbounded, interactive list below a block of prose whose height changes with every '
        + 'selection',
    )
    ok(
      !/<pre style=\{\{ whiteSpace/.test(tab),
      'and the message is styled by the stylesheet rather than an inline `style` \u2014 it needs '
        + '`overflow-wrap`, because a URL in a commit message has no space in it and gives the '
        + 'whole pane a horizontal scrollbar without one',
    )

    // --- the rows are a file list, not a summary (M21) -------------------------------------------

    ok(
      /<FileIcon/.test(view),
      'the changed-file rows draw the shared `FileIcon`, so they read as the same kind of thing '
        + 'the file tree and the search panel draw',
    )
    ok(
      /from '@\/icons\/FileIcon'/.test(view) && !/from '@\/icons'/.test(view),
      '\u2026imported from the component\u2019s own module and NOT the `@/icons` barrel. The '
        + 'barrel re-exports `useIconTheme`, whose module graph reaches the workspace store and '
        + 'therefore xterm, whose addons touch `self` at import time \u2014 fatal under '
        + '`check-log-render.mjs`, which loads this file in node',
    )
    ok(
      /theme: IconTheme/.test(view) && !/useIconTheme/.test(view),
      'and the theme arrives as a prop rather than from the hook, which is the same split: pure '
        + 'below, store above',
    )
    ok(
      /onSelect\?\.\(path\)[\s\S]{0,120}onOpen\?\.\(path, oldPath, e\.detail\)/.test(view),
      'one click selects and then the click count decides whether it also opens \u2014 in that '
        + 'order, from one handler, so a double-click leaves the row it opened looking selected',
    )

    ok(
      /chipTarget\(chip, filter\.branch\)/.test(view),
      'the chip asks `chipTarget` rather than deciding for itself, so "is this a control" has one '
        + 'answer and not two that drift',
    )
    ok(
      /onFilter\(\{ \.\.\.filter, branch: target \}\)/.test(view),
      '…and applies it by replacing the branch field only — the same field the `<select>` beside '
        + 'it sets, so one value cannot have two meanings depending on which control set it',
    )
    ok(
      /e\.stopPropagation\(\)/.test(view),
      'and stops the click. The row underneath selects on click and adds a compare endpoint on '
        + 'Ctrl+click; without this a chip would re-root the walk AND select a row that is about '
        + 'to be the first row of a completely different list',
    )
    ok(
      /tabIndex=\{-1\}/.test(view),
      'the chips are not tab stops. The list is one `tabIndex={0}` scroller with no roving '
        + 'tabindex over its rows, so default stops would put up to `MAX_CHIPS` of them on every '
        + 'row — a hundred and fifty Tab presses on a fifty-row page, which is a worse regression '
        + 'for a keyboard user than a chip they cannot reach. The branch `<select>` is the '
        + 'keyboard path, and `LogView.tsx` says in full which case it does not cover',
    )
  }

  // --- the repo strip ----------------------------------------------------------------------------

  {
    const repos = [
      { id: 'r1', root: '/w/cide', name: 'cide', parent: null, isSubmodule: false },
      { id: 'r2', root: '/w/cide/vendor/hub', name: 'hub-core', parent: null, isSubmodule: false },
    ]
    eq(m.primaryRepo(repos).name, 'cide', 'the first root, which is the only choice stable across restarts')
    eq(m.primaryRepo([]), null, 'and `null` rather than a throw when there is none')
    eq(m.repoStripOn([repos[0]]), false, 'one repository draws no strip — every row would say the same thing')
    eq(m.repoStripOn(repos), true, 'two do, and a merged walk is also the case with no graph to look at')
    eq(m.repoChipFor(repos, 'r2'), { name: 'hub-core', color: 'var(--lane-1)' }, 'coloured by position')
    eq(
      m.repoChipFor(repos, 'r9'),
      null,
      'a row from a repository the list has never heard of draws no chip. A uuid in a 60px chip '
        + 'is worse than nothing, and the next refresh fixes it',
    )
  }

  // --- lane colours ------------------------------------------------------------------------------

  eq(m.laneColor(0), 'var(--lane-0)', 'a lane is a token, not a hex value — the graph follows the theme')
  eq(m.laneColor(7), 'var(--lane-7)', 'the last of the eight')
  eq(m.laneColor(8), 'var(--lane-0)', 'and the ninth wraps, rather than naming a property nothing defines')
  eq(m.laneColor(23), 'var(--lane-7)', 'as does anything further out — a forged resume token cannot blank a line')

  {
    // The three copies of "eight", pinned to each other. A palette that grew in Rust and not in
    // the stylesheet paints nothing at all for the new index, which reads as a missing line.
    const lanes = readFileSync(join(ROOT, 'crates', 'cide-git', 'src', 'lanes.rs'), 'utf8')
    eq(
      /pub const PALETTE: u16 = (\d+);/.exec(lanes)?.[1],
      '8',
      '`cide_git::lanes::PALETTE` is still eight, which is the number `laneColor` takes a modulo by',
    )
    const tokens = readFileSync(join(UI, 'src', 'styles', 'tokens.css'), 'utf8')
    for (let i = 0; i < 8; i++) {
      ok(new RegExp(`--lane-${i}\\s*:`).test(tokens), `--lane-${i} is defined in tokens.css`)
    }
    ok(
      !/--lane-8\s*:/.test(tokens),
      'and there is no ninth, so the modulo and the stylesheet agree about where the palette ends',
    )
  }

  /* --- selecting two commits (M20) ---------------------------------------------------------------
   *
   * The reducer behind Ctrl+click and Shift+click, and the function that decides which of the two
   * selected commits is the newer side of the diff. Every rule here is invisible when it is wrong:
   * a pair ordered by the click sequence instead of by the page renders a perfectly plausible diff
   * with every hunk inverted — additions drawn as deletions — and nothing on screen says so.
   *
   * `order` is the page's own order, newest first. The fixtures below use four rows so that "the
   * third Ctrl+click replaces the older endpoint" has a middle to be wrong about.
   */
  {
    const A = full('a1b2c3d') // newest
    const B = full('b2c3d4e')
    const C = full('c3d4e5f')
    const D = full('d4e5f60') // oldest
    const ORDER = [A, B, C, D]
    const OFF_PAGE = full('9999999')

    const PLAIN = { ctrl: false, shift: false }
    const CTRL = { ctrl: true, shift: false }
    const SHIFT = { ctrl: false, shift: true }
    const BOTH = { ctrl: true, shift: true }

    /** `selectRow` from `NO_SELECTION`, one gesture per element. */
    const clicks = (...gestures) =>
      gestures.reduce((sel, [oid, mods]) => m.selectRow(sel, oid, mods, ORDER), m.NO_SELECTION)

    eq(
      m.NO_SELECTION,
      { primary: null, secondary: null, swapped: false },
      'nothing is selected to begin with, and `swapped` is a field of the selection rather than '
        + 'state beside it — a flip that outlived the pair it was made on would silently invert a '
        + 'diff the user never flipped',
    )
    ok(
      m.selectRow(m.NO_SELECTION, A, PLAIN, ORDER) !== m.NO_SELECTION,
      'and the reducer returns a new value rather than mutating the shared constant, which would '
        + 'make every untouched Log tab in the window start out with somebody else’s selection',
    )

    // --- a plain click replaces ------------------------------------------------------------------

    eq(
      clicks([A, PLAIN]),
      { primary: A, secondary: null, swapped: false },
      'a plain click selects one commit',
    )
    eq(
      clicks([A, CTRL], [B, PLAIN]),
      { primary: B, secondary: null, swapped: false },
      '…and a plain click on top of a pair replaces the whole thing. It is the gesture that means '
        + '"I am looking at this one", and quietly keeping the other endpoint would leave the pane '
        + 'showing a range the user thought they had dismissed',
    )
    eq(
      clicks([A, PLAIN], [A, PLAIN]),
      { primary: A, secondary: null, swapped: false },
      '…and clicking the same row twice is the same selection, not a toggle. A list where the '
        + 'second click deselects is a list you cannot double-click',
    )

    // --- Ctrl adds, and Ctrl removes ---------------------------------------------------------------

    eq(
      clicks([A, PLAIN], [C, CTRL]),
      { primary: A, secondary: C, swapped: false },
      'Ctrl adds the second endpoint and leaves the anchor where it was',
    )
    eq(
      clicks([A, PLAIN], [C, CTRL], [C, CTRL]),
      { primary: A, secondary: null, swapped: false },
      'Ctrl on the second endpoint removes it, leaving an ordinary one-commit selection',
    )
    eq(
      clicks([A, PLAIN], [C, CTRL], [A, CTRL]),
      { primary: C, secondary: null, swapped: false },
      '…and Ctrl on the ANCHOR removes that one instead, promoting the other. Not "clear '
        + 'everything": the user deselected one row, and answering by deselecting both is the '
        + 'gesture doing twice what it said',
    )
    eq(
      clicks([A, CTRL]),
      { primary: A, secondary: null, swapped: false },
      'Ctrl on an empty list is a plain click. There is nothing to add to, and refusing would make '
        + 'the first Ctrl+click of every session do nothing',
    )

    // --- the third Ctrl+click ----------------------------------------------------------------------

    eq(
      clicks([A, PLAIN], [C, CTRL], [B, CTRL]),
      { primary: A, secondary: B, swapped: false },
      'a third Ctrl+click replaces the OLDER endpoint and keeps the newer one. Two is what a diff '
        + 'has; ignoring the click would make Ctrl+click read as a dead control on exactly the '
        + 'click where the user is trying to say something, and starting again would throw away '
        + 'the endpoint they spent two clicks establishing',
    )
    eq(
      clicks([C, PLAIN], [A, CTRL], [D, CTRL]),
      { primary: A, secondary: D, swapped: false },
      '…and "older" is decided by the page, not by which slot it is in: here the ANCHOR is the '
        + 'older of the two and it is the one that goes',
    )

    // --- Shift extends and keeps the ends ------------------------------------------------------------

    eq(
      clicks([A, PLAIN], [D, SHIFT]),
      { primary: A, secondary: D, swapped: false },
      'Shift extends from the anchor and keeps only the two ends. A range of commits is not a '
        + 'thing this pane can show — `git_diff_revision_files` takes two sides — so selecting the '
        + 'whole span and silently diffing its ends would be a selection that lies',
    )
    eq(
      clicks([A, PLAIN], [C, SHIFT], [D, SHIFT]),
      { primary: A, secondary: D, swapped: false },
      '…and a second Shift re-extends from the same anchor rather than from where the last one '
        + 'landed, which is what every list in every editor does',
    )
    eq(
      clicks([A, PLAIN], [A, SHIFT]),
      { primary: A, secondary: null, swapped: false },
      'Shift onto the anchor itself is a range of one, which is a single selection',
    )
    eq(clicks([B, SHIFT]), { primary: B, secondary: null, swapped: false }, 'Shift with no anchor is a plain click')
    eq(
      clicks([A, PLAIN], [C, BOTH]),
      clicks([A, PLAIN], [C, SHIFT]),
      'Ctrl+Shift extends, because Shift is tested first. A slipped finger doing NOTHING is the '
        + 'worse of the two answers, and every list these users also use extends here',
    )

    // --- which end is newer ---------------------------------------------------------------------------

    eq(m.comparePair(m.NO_SELECTION, ORDER), null, 'nothing selected is not a comparison')
    eq(m.comparePair(clicks([A, PLAIN]), ORDER), null, '…and neither is one commit')
    eq(
      m.comparePair({ primary: A, secondary: A, swapped: false }, ORDER),
      null,
      '…and neither is a commit paired with itself, which *Compare with…* can produce by resolving '
        + 'a name that lands on the selected row. It has no files in it, and an empty list under a '
        + '"Comparing a1b2c3d … a1b2c3d" header is indistinguishable from a request that failed',
    )

    eq(
      m.comparePair(clicks([A, PLAIN], [C, CTRL]), ORDER),
      { newer: A, older: C },
      'the pair is ordered by the page: `a1b2c3d` is row 0 and `c3d4e5f` is row 2',
    )
    eq(
      m.comparePair(clicks([C, PLAIN], [A, CTRL]), ORDER),
      m.comparePair(clicks([A, PLAIN], [C, CTRL]), ORDER),
      'and clicking bottom-then-top gives the SAME pair as top-then-bottom. This is the assertion '
        + 'the whole `order` parameter exists for: a user clicks the commit they noticed first, '
        + 'which is as often the lower of the two, and deriving "newer" from the click sequence '
        + 'shows them a diff with every hunk inverted and nothing on screen saying so',
    )
    eq(
      m.comparePair(clicks([D, PLAIN], [B, SHIFT]), ORDER),
      { newer: B, older: D },
      '…and a Shift extension upwards is ordered the same way, by the page and not by the anchor',
    )

    eq(
      m.comparePair({ primary: A, secondary: OFF_PAGE, swapped: false }, ORDER),
      { newer: A, older: OFF_PAGE },
      'an endpoint the page does not hold sorts as the OLDER side. *Compare with…* can name a tag '
        + 'or a `HEAD~200` that is below the frontier, `git rev-parse` answers with an oid and no '
        + 'date, and asking for one would be a second round trip to orient a diff the user is '
        + 'about to look at. So it is a guess — and ⇄ Swap is the correction',
    )
    eq(
      m.comparePair({ primary: OFF_PAGE, secondary: A, swapped: false }, ORDER),
      { newer: A, older: OFF_PAGE },
      '…whichever slot it is in, so the guess does not depend on which gesture set it',
    )

    // --- ⇄ Swap ---------------------------------------------------------------------------------------

    const pairAC = clicks([A, PLAIN], [C, CTRL])
    eq(
      m.comparePair(m.swapPair(pairAC), ORDER),
      { newer: C, older: A },
      'Swap reads the same two commits the other way round. It has to CHANGE the answer for a pair '
        + 'that is fully on the page, which is why the flip is a third field: exchanging `primary` '
        + 'and `secondary` would be undone by the very re-sort above and the control would '
        + 'visibly do nothing',
    )
    eq(
      m.swapPair(m.swapPair(pairAC)),
      pairAC,
      '…and pressing it twice is where you started. An involution, because it toggles one boolean '
        + 'rather than moving oids between slots',
    )
    eq(
      m.swapPair(clicks([A, PLAIN])),
      clicks([A, PLAIN]),
      'Swap with one commit selected is a no-op rather than a throw. The control is only drawn '
        + 'beside a pair, but a keystroke that raises an exception because nothing is selected is '
        + 'worse than one that does nothing',
    )
    eq(
      m.selectRow(m.swapPair(pairAC), B, CTRL, ORDER).swapped,
      false,
      'and any gesture that changes WHICH commits are selected clears the flip. Carried over onto '
        + 'a new pair it would invert a diff the user never flipped, and the header would still '
        + 'read the right way round',
    )

    // --- what the pane says about a pair ----------------------------------------------------------------

    eq(
      m.compareTitle(D, A),
      'Comparing d4e5f60 … a1b2c3d',
      'the header abbreviates both ends and puts the OLDER on the left — the direction time runs '
        + 'in every range expression git accepts (`old..new`) and the direction the patch is '
        + 'computed in',
    )
    eq(
      m.compareTitle(D, m.WORKING_TREE_LABEL),
      'Comparing d4e5f60 … working tree',
      '…and a side that is not an oid is left exactly as it is. *Compare with working tree* has no '
        + 'hash for its newer side, and a blind `slice(0, 7)` would print "working"',
    )

    eq(m.changeCounts(12, 3), '+12 −3', 'a row’s counts, with a real minus sign (U+2212)')
    ok(!m.changeCounts(1, 1).includes('-'), '…and not a hyphen-minus, as everywhere else in this app')
    eq(m.changeCounts(0, 0), '+0 −0', 'a rename with no edits still shows both numbers rather than nothing')

    eq(m.rangeTruncatedNote(false), null, 'an uncapped list says nothing about a cap')
    ok(
      m.rangeTruncatedNote(true)?.includes((2000).toLocaleString()),
      'and a capped one NAMES the cap. A silently short list reads as a complete answer and there '
        + 'is nothing on screen to contradict it; a user who knows the number is 2 000 knows the '
        + 'next move is to narrow the range',
    )
    {
      // The number is Rust's. `RevisionRange.truncated` carries the flag and not the cap, so the
      // sentence restates it — and a restatement nobody pins is a sentence that will one day name
      // a number the backend stopped using.
      const rust = readFileSync(join(ROOT, 'crates', 'cide-git', 'src', 'revision.rs'), 'utf8')
      const cap = /pub const RANGE_FILE_CAP: usize = ([0-9_]+);/.exec(rust)?.[1]?.replace(/_/g, '')
      eq(
        String(m.RANGE_FILE_CAP),
        cap,
        '`logModel::RANGE_FILE_CAP` is `cide_git::revision::RANGE_FILE_CAP`. They are two numbers '
          + 'in two languages and only one of them is enforced',
      )
    }

    ok(
      m.CROSS_REPO_COMPARE.length > 30 && /repositor/i.test(m.CROSS_REPO_COMPARE),
      'and the one refusal this feature has is a sentence naming what is wrong. Two commits from '
        + 'two roots of a merged walk share no object database, `git_diff_revision_files` takes '
        + 'one `repo`, and there is no request to make',
    )

    // --- the wire shapes this all lands on ----------------------------------------------------------

    for (const [type, fields] of [
      ['RevisionChange', ['path', 'oldPath', 'status', 'binary', 'additions', 'deletions']],
      [
        'RevisionRange',
        ['new', 'old', 'newOid', 'oldOid', 'newSummary', 'oldSummary', 'files', 'truncated'],
      ],
      ['ResolvedRev', ['spec', 'from', 'to', 'range', 'label']],
    ]) {
      const body = stripComments(shape(type))
      ok(body !== '', `${type} exists in generated.ts`)
      for (const field of fields) {
        ok(new RegExp(`\\b${field}\\b`).test(body), `${type}.${field} is still on the wire`)
      }
    }
    eq(
      stripComments(generated)
        .match(/export type RevSide = ([^;]*);/)?.[1]
        ?.match(/"kind": "(\w+)"/g),
      ['"kind": "commit"', '"kind": "firstParent"', '"kind": "workingTree"'],
      'and `RevSide` still has exactly these three arms. Two of the nine pairings are illegal — '
        + '`FirstParent` is meaningless as `new` and `WorkingTree` as `old` — and `cide_git` '
        + 'refuses both by name, so a fourth arm added here is a pairing table nobody has decided',
    )
  }

  /* --- the row menu, as a value ----------------------------------------------------------------
   *
   * `actionsFor` — which of the seven a row offers — is `check-log-actions.mjs`'s, and so is every
   * sentence in the dialogs. What is left, and what is asserted here, is the **menu model**: the
   * lines a right-click actually draws, which of them are dead and why, and what the reset
   * dialog's answer turns into on the wire. Every one of those is a silent failure when it goes
   * wrong — a line that does nothing when clicked, a `--hard` performed where `--soft` was
   * selected — and none of them is visible to `tsc` or to a Rust test.
   *
   * # Why it is its own module
   *
   * The model lives in `src/gitlog/logMenu.ts` and not in `LogTab.tsx`, where the menu is wired.
   * `LogTab.tsx` cannot be compiled here at all: it pulls in `@/ipc/client`, `@/store/workspace`
   * — which calls `document.createElement` at module scope — and the whole of `keys/dispatch.ts`.
   * Faking a browser deep enough to load that would be a check that proves things about the fake.
   *
   * It was briefly a `#region` inside `LogTab.tsx` that this script sliced out as **text** and
   * compiled in a temp directory. That worked, and it compiled the real source, but the slicing
   * was the fragile part: a renamed marker or a stray brace broke it in a way that read as a test
   * failure rather than a build one. A file is what `chrome/menuModel.ts` and `menus/model.ts`
   * already use one folder over, so this names one.
   *
   * `logMenu.ts` imports only `chrome/logActions.ts`, which has no imports of any kind — so the
   * two compile together under a bare `tsc` with a path alias and nothing else comes with them.
   */
  {
    // The wiring still has to be in `LogTab.tsx` — the model is only worth checking if the menu
    // actually uses it — so that join is asserted over the source further down, in the wiring
    // section. Here we compile the model itself.
    const menuConfig = join(out, 'tsconfig.menu.json')
    const menuJs = join(out, 'menu-js')
    writeFileSync(
      menuConfig,
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
          outDir: menuJs,
          rootDir: join(UI, 'src'),
          baseUrl: UI,
          paths: { '@/*': ['src/*'] },
          types: [],
        },
        files: [
          join(UI, 'src', 'gitlog', 'logMenu.ts'),
          join(UI, 'src', 'chrome', 'logActions.ts'),
        ],
      }),
    )
    execFileSync('node', ['node_modules/typescript/bin/tsc', '--project', menuConfig], {
      stdio: 'inherit',
      cwd: UI,
    })
    /*
     * `tsc`'s `paths` is a *resolution* hint and not a rewrite: the emitted JS keeps `@/…`
     * verbatim, and node has no idea what that is. So the one specifier is rewritten to the
     * relative path of the file `tsc` just emitted beside it. `check-git-tree.mjs` does the same
     * thing to `.js`-suffix a relative import; this is that trick with an alias in front of it.
     *
     * `verbatimModuleSyntax` erases the type-only imports, so there is exactly one specifier to
     * rewrite — and what survives is the real `actionsFor`, byte for byte, not a stand-in.
     */
    const emitted = join(menuJs, 'gitlog', 'logMenu.js')
    writeFileSync(
      emitted,
      readFileSync(emitted, 'utf8').replace(
        /from '@\/chrome\/logActions'/g,
        "from '../chrome/logActions.js'",
      ),
    )
    const cm = await import(`file://${emitted}`)

    ok(
      !/\b(document|navigator|window)\b/.test(
        stripComments(readFileSync(join(UI, 'src', 'gitlog', 'logMenu.ts'), 'utf8')),
      ),
      'the menu model is still DOM-free. `navigator.clipboard` is resolved by the host and '
        + 'arrives as an absent `copy` action; a model that read a DOM global could not run '
        + 'under node at all',
    )


    // --- the branch box's list (M21) -----------------------------------------------------------
    //
    // It replaced a `<select>`, which could not be typed into. Three of these rules are the ones
    // an obvious implementation gets wrong.

    const BRANCHES = ['main', 'release/8.2', 'feature/login', 'origin/main', 'fix/Login-crash']

    eq(
      bf.branchRows(BRANCHES, '', 'head', 'Current branch').map((r) => r.label),
      ['Current branch', 'All branches', ...BRANCHES],
      'an empty query is the FULL list, not an empty one. That is the focused-but-untyped state '
        + 'and it is the whole reason the box is worth having over a text field',
    )
    eq(
      bf.branchRows(BRANCHES, 'login', 'head', 'Current branch').map((r) => r.label),
      ['feature/login', 'fix/Login-crash'],
      'typing narrows to a case-insensitive substring, and drops the two synthetic rows when '
        + 'they do not match — they are readings, not branches, and leaving them pinned above a '
        + 'filtered list makes the first arrow-down land on one of them',
    )
    ok(
      bf.branchRows(BRANCHES, 'Login', 'head', 'x').length === 2,
      '…case-insensitively in both directions: the query is folded and so is the name',
    )

    // The cap, and the order it is applied in. This is the one that is silently wrong.
    const MANY = Array.from({ length: 120 }, (_, i) => `topic/branch-${i}`)
    eq(
      bf.branchRows(MANY, '', 'head', 'x').filter((r) => !r.synthetic).length,
      bf.BRANCH_LIST_MAX,
      'the list caps at fifty branches',
    )
    eq(
      bf.branchRows(MANY, 'branch-119', 'head', 'x').map((r) => r.label),
      ['topic/branch-119'],
      'but filtering happens BEFORE the cap, so the hundred-and-twentieth branch is reachable by '
        + 'typing its name. Capping first would leave it permanently unreachable while the box '
        + 'looked like it was working',
    )
    eq(bf.branchOverflowNote(MANY, ''), '70 more — keep typing to narrow', 'and the list says so')
    eq(
      bf.branchOverflowNote(BRANCHES, ''),
      null,
      '…and says nothing when nothing is hidden, rather than "0 more"',
    )

    // A revspec is not a branch and cannot be matched — it has to be carried, or the box goes
    // blank while holding a real filter. `Clear filters and find it` is what sets one.
    ok(
      bf
        .branchRows(BRANCHES, '', 'r:a1b2c3d', 'a1b2c3d')
        .some((r) => r.value === 'r:a1b2c3d' && r.synthetic),
      'a chosen revision is carried into the list so the control can show what it is walking',
    )
    ok(
      !bf.branchRows(BRANCHES, '', 'head', 'x').some((r) => r.value.startsWith('r:')),
      '…and is absent otherwise, rather than an option that only ever re-selects itself',
    )

    // Keyboard. Clamped, not wrapping, and -1 is a real state.
    eq(bf.moveHighlight(5, -1, 'ArrowDown'), 0, 'the first ↓ lands on row one, not row two')
    eq(bf.moveHighlight(5, 4, 'ArrowDown'), 4, 'and ↓ at the bottom stays')
    eq(
      bf.moveHighlight(5, 0, 'ArrowUp'),
      -1,
      '↑ off the top returns to "nothing highlighted", which is what lets Enter commit the typed '
        + 'text instead of a row the user never looked at',
    )
    eq(bf.moveHighlight(0, -1, 'ArrowDown'), -1, 'an empty list has nowhere to go')

    const two = bf.branchRows(BRANCHES, 'login', 'head', 'x')
    eq(bf.commitChoice(two, 1)?.label, 'fix/Login-crash', 'Enter takes the highlighted row')
    eq(
      bf.commitChoice(two, -1),
      null,
      'and with several matches and no highlight it takes nothing — guessing the first would '
        + 'pick a branch the user never looked at',
    )
    eq(
      bf.commitChoice(bf.branchRows(BRANCHES, 'release', 'head', 'x'), -1)?.label,
      'release/8.2',
      'but with exactly ONE match Enter takes it. Requiring ↓ first to highlight the only row on '
        + 'screen is a keystroke that carries no information',
    )

    // --- grouping the changed files (M21) --------------------------------------------------------

    const FILES = [
      { path: 'ui/src/a.ts', oldPath: null, counts: null },
      { path: 'README.md', oldPath: null, counts: null },
      { path: 'ui/src/b.ts', oldPath: null, counts: null },
      { path: 'crates/x/y.rs', oldPath: null, counts: null },
    ]

    eq(
      fr.flatRows(FILES).map((r) => r.label),
      ['ui/src/a.ts', 'README.md', 'ui/src/b.ts', 'crates/x/y.rs'],
      'flat is every whole path in the order given',
    )
    ok(
      fr.flatRows(FILES).every((r) => r.kind === 'file' && r.depth === 0),
      '…with no headings and no indent',
    )
    eq(
      fr.groupedRows(FILES).map((r) => `${r.kind}:${r.label}`),
      ['dir:ui/src', 'file:a.ts', 'file:b.ts', 'dir:crates/x', 'file:y.rs', 'file:README.md'],
      'the tree gathers each directory’s files under its heading, in the order each directory '
        + 'FIRST appeared — not alphabetically. Re-sorting would make the two arrangements '
        + 'disagree about which file is first, which is two views of one commit that cannot be '
        + 'compared by eye',
    )
    eq(
      fr.groupedRows(FILES).at(-1)?.label,
      'README.md',
      '…with one departure from the input order, and only one: a level draws its DIRECTORIES '
        + 'before its own files, so the root-level `README.md` sits below both headings rather '
        + 'than between them. That is what both other trees in this app do, and what every file '
        + 'manager does — a folder buried between two files is a folder nobody finds',
    )
    ok(
      fr.groupedRows(FILES).find((r) => r.label === 'README.md')?.depth === 0,
      'a file at the repository root gets no heading and no indent. There is no directory to '
        + 'name, and inventing `/` or `(root)` puts a row on screen matching nothing',
    )
    eq(
      fr.groupedRows(FILES).filter((r) => r.kind === 'file').length,
      FILES.length,
      'grouping regroups and never hides: every file is still a row',
    )

    // Two directories sharing a text prefix but not a path segment. A `startsWith` grouper puts
    // these together and the mistake is invisible until someone has both.
    eq(
      fr
        .groupedRows([
          { path: 'src/app/main.ts', oldPath: null, counts: null },
          { path: 'src/apple/core.ts', oldPath: null, counts: null },
        ])
        .filter((r) => r.kind === 'dir')
        .map((r) => `${r.depth}:${r.label}`),
      ['0:src', '1:app', '1:apple'],
      '`src/app` and `src/apple` are two directories under one parent, not one directory and not '
        + 'two roots. A prefix test rather than a segment test files them together and nothing '
        + 'else notices; a bucket-per-path grouper draws them as two unrelated top-level rows, '
        + 'which is what this list did until M27',
    )

    // Renames, which are the reason the choice is offered at all.
    eq(
      fr
        .groupedRows([{ path: 'ui/src/new.ts', oldPath: 'ui/src/old.ts', counts: null }])
        .filter((r) => r.kind === 'file')
        .map((r) => r.label),
      ['old.ts → new.ts'],
      'a rename inside one directory shows both basenames — the heading already says where they '
        + 'are, and dropping the arrow would make a rename look like an ordinary edit',
    )
    eq(
      fr
        .groupedRows([{ path: 'ui/src/new.ts', oldPath: 'crates/old.rs', counts: null }])
        .filter((r) => r.kind === 'file')
        .map((r) => r.label),
      ['crates/old.rs → new.ts'],
      'a rename ACROSS directories shows the old path whole. Filed under the new directory and '
        + 'trimmed to basenames it would read `old.rs → new.ts` with no hint that the file moved',
    )
    // --- it is a TREE, not a list of directories (M27) --------------------------------------------
    //
    // > *"changed files in commit view (bottom panel) currently has wrong tree - each folder is a
    // > row, but this should be a real tree, like file tree"*
    //
    // The old grouper bucketed files by their whole directory path and drew one row per bucket.
    // With two directories that is indistinguishable from a tree, which is why every assertion
    // above kept passing; with four it is a flat list of long paths and `ui/src/panes` and
    // `ui/src/sidebar/GitPanel` are two unrelated top-level rows with no shared parent. Nesting
    // was not merely missing — a bucket key has no parent, so it was unrepresentable.
    //
    // This is the fixture that can tell the two apart: three directories under one shared parent,
    // one of which is itself a chain worth compacting.

    const DEEP = [
      { path: 'ui/src/panes/GitDiffPane.tsx', oldPath: null, counts: null },
      { path: 'ui/src/panes/mergeModel.ts', oldPath: null, counts: null },
      { path: 'ui/src/sidebar/GitPanel/model.ts', oldPath: null, counts: null },
      { path: 'ui/src/store/workspace.ts', oldPath: null, counts: null },
      { path: 'crates/cide-git/src/stage.rs', oldPath: null, counts: null },
      { path: 'README.md', oldPath: null, counts: null },
    ]

    eq(
      fr.groupedRows(DEEP).map((r) => `${r.depth}:${r.kind}:${r.label}`),
      [
        '0:dir:ui/src',
        '1:dir:panes',
        '2:file:GitDiffPane.tsx',
        '2:file:mergeModel.ts',
        '1:dir:sidebar/GitPanel',
        '2:file:model.ts',
        '1:dir:store',
        '2:file:workspace.ts',
        '0:dir:crates/cide-git/src',
        '1:file:stage.rs',
        '0:file:README.md',
      ],
      'three directories under one `ui/src` parent are drawn UNDER it, one level deeper — not as '
        + 'three top-level rows spelling `ui/src/` three times. Depth is the whole claim: the '
        + 'labels alone were right in the flat grouper too',
    )
    eq(
      fr.groupedRows(DEEP).filter((r) => r.kind === 'dir').map((r) => r.dir),
      ['ui/src', 'ui/src/panes', 'ui/src/sidebar/GitPanel', 'ui/src/store', 'crates/cide-git/src'],
      'a compacted row publishes its DEEPEST path as `dir`, however many segments its label '
        + 'joined. That is the id `toggleCollapsed` is handed and the one a fold outlives its '
        + 'commit under — keying it on the shallow end would fold a different directory in the '
        + 'next commit, or none at all',
    )
    eq(
      fr.groupedRows(DEEP).find((r) => r.dir === 'ui/src')?.count,
      4,
      '\u2026and its count is every file at or BELOW it, not the files it holds directly \u2014 '
        + '`ui/src` holds none of its own, and a folded heading saying `0` is a heading claiming '
        + 'to hide nothing while hiding four',
    )
    eq(
      fr.groupedRows(DEEP, new Set(['ui/src'])).map((r) => `${r.depth}:${r.kind}:${r.label}`),
      ['0:dir:ui/src', '0:dir:crates/cide-git/src', '1:file:stage.rs', '0:file:README.md'],
      'folding a parent takes its nested HEADINGS with it, not just its own files. A fold that '
        + 'left `panes` and `store` on screen under a shut parent would be a disclosure that '
        + 'discloses nothing \u2014 and is exactly what a bucket-per-path grouper does, because '
        + 'it never knew `ui/src/panes` was inside `ui/src`',
    )
    eq(
      fr.groupedRows(DEEP, new Set(['ui/src/panes'])).map((r) => `${r.kind}:${r.label}`),
      [
        'dir:ui/src',
        'dir:panes',
        'dir:sidebar/GitPanel',
        'file:model.ts',
        'dir:store',
        'file:workspace.ts',
        'dir:crates/cide-git/src',
        'file:stage.rs',
        'file:README.md',
      ],
      '\u2026and folding a nested one hides only what is under it, leaving its parent and its '
        + 'siblings alone',
    )
    eq(
      fr.groupedRows(DEEP).filter((r) => r.kind === 'file').length,
      DEEP.length,
      'nesting regroups and never hides: every file is still a row',
    )

    eq(
      fr.fileRows(FILES, true).length,
      fr.groupedRows(FILES).length,
      '`fileRows` is the one entry point, so no caller can draw a third arrangement',
    )
    eq(fr.fileRows(FILES, false).length, fr.flatRows(FILES).length, '…either way round')

    // --- the changed-file menu (M21) -------------------------------------------------------------

    const target = { path: 'ui/src/a.ts', oldPath: null }
    const plain = fm.fileMenu({ target, deleted: false, newerIsWorkingTree: false })
    eq(
      plain.map((i) => i.id),
      ['showDiff', 'openFile', 'compareLocal', 'compareBeforeLocal'],
      'four items, in order: read the commit, open the file, then the two that end on disk',
    )
    ok(
      plain.every((i) => i.disabledReason === undefined),
      'and all live for an ordinary modified file',
    )

    const gone = fm.fileMenu({ target, deleted: true, newerIsWorkingTree: false })
    eq(
      gone.filter((i) => i.disabledReason !== undefined).map((i) => i.id),
      ['openFile', 'compareLocal', 'compareBeforeLocal'],
      'a deleted file has nothing on disk to open or compare against',
    )
    eq(
      gone.find((i) => i.id === 'showDiff')?.disabledReason,
      undefined,
      '…but *Show diff* still works, and that asymmetry is the point: the deletion itself is a '
        + 'perfectly readable diff and is usually the thing the reader came for',
    )

    const local = fm.fileMenu({ target, deleted: false, newerIsWorkingTree: true })
    eq(
      local.filter((i) => i.disabledReason !== undefined).map((i) => i.id),
      ['compareLocal', 'compareBeforeLocal'],
      'a range already ending at the working tree cannot be compared with the working tree — the '
        + 'answer is a diff with no hunks, which looks like a failed request rather than like '
        + '"these are the same"',
    )
    eq(
      local.find((i) => i.id === 'openFile')?.disabledReason,
      undefined,
      '…and *Open file* is untouched by that, because it is not a comparison',
    )
    ok(
      plain.every((i) => i.disabledReason === undefined || i.disabledReason.length > 12),
      'every reason is a sentence rather than a word — the rule `disabledReason` replaced '
        + '`disabled` for',
    )

    // Ctrl+D, and the three chords it must NOT claim.
    const chord = (over) => ({
      key: 'd',
      ctrlKey: true,
      metaKey: false,
      altKey: false,
      shiftKey: false,
      ...over,
    })
    ok(fm.isDiffShortcut(chord({})), 'Ctrl+D is the shortcut')
    ok(fm.isDiffShortcut(chord({ key: 'D' })), '…with Caps Lock on too')
    ok(
      !fm.isDiffShortcut(chord({ ctrlKey: false, metaKey: true })),
      'but not ⌘D, which is *duplicate* in most editors on a Mac',
    )
    ok(!fm.isDiffShortcut(chord({ altKey: true })), 'not Ctrl+Alt+D, which the desktop may own')
    ok(
      !fm.isDiffShortcut(chord({ shiftKey: true })),
      'and not Ctrl+Shift+D — a different chord, and claiming it here would take it from every '
        + 'other surface in the window',
    )
    eq(
      fm.PRIMARY_FILE_ACTION,
      'showDiff',
      'the keyboard does what a double-click does, recorded as one value so editing one of them '
        + 'is visibly editing both',
    )

    // --- the two joins this file cannot invent -------------------------------------------------

    const menuModel = stripComments(readFileSync(join(UI, 'src', 'menus', 'model.ts'), 'utf8'))
    ok(
      /run:\s*enabled\s*&&\s*entry\.submenu === undefined\s*\?\s*\(entry\.run\s*\?\?\s*null\)\s*:\s*null/
        .test(menuModel),
      '`resolveMenu` still drops the handler of a disabled item. That single line is why an item '
        + 'carrying BOTH a `disabledReason` and a `run` looks wired and is dead, and it is what '
        + 'the assertion below is protecting against. The `submenu` clause beside it (M19) is the '
        + 'same rule for the other pair that cannot both be true: a row that opens a list is '
        + 'opened by the click that would have run it, so shipping both leaves one unreachable',
    )
    {
      // The interface body alone: `ElementFacts` further down the same file has a real
      // `disabled?: boolean`, because it describes a DOM element rather than a menu line.
      const at = menuModel.indexOf('export interface MenuItem {')
      const body = menuModel.slice(at, menuModel.indexOf('}', at))
      ok(at >= 0, '`MenuItem` is still declared in menus/model.ts')
      ok(
        !/\bdisabled\s*\??\s*:/.test(body),
        '…and there is still no `disabled: boolean` on it to reach for instead of a sentence',
      )
      ok(/disabledReason\?:/.test(body), '…only `disabledReason`, which cannot be supplied silently')
    }

    const NO_CLIP = /export const NO_CLIPBOARD = '([^']*)'/.exec(
      readFileSync(join(UI, 'src', 'chrome', 'menuModel.ts'), 'utf8'),
    )?.[1]
    ok(
      typeof NO_CLIP === 'string' && NO_CLIP !== '',
      '`chrome/menuModel.ts` still exports NO_CLIPBOARD as a plain string literal. It is read out '
        + 'of the source rather than restated here on purpose: a check asserting on prose it also '
        + 'defines proves nothing',
    )

    const dialog = stripComments(
      readFileSync(join(UI, 'src', 'chrome', 'ConfirmDestructive.tsx'), 'utf8'),
    )
    ok(
      /choices\.find\(\(c\) => c\.id === state\.chosen\) \?\? choices\[0\]/.test(dialog),
      '`ConfirmDestructive` still falls back to the FIRST choice when nothing matches `chosen`. '
        + '`pickedId` is a copy of that rule — the component resolves it privately and the caller '
        + 'is the one that has to send the answer over the wire — so the day it changes there is '
        + 'the day this fails, rather than the day a reset performs a mode nobody selected',
    )

    // --- the item list -------------------------------------------------------------------------

    const HEAD_OID = full('a1b2c3d')
    const OLD_OID = full('e5f6a71')
    const commitRow = (oid) => ({
      oid,
      shortOid: oid.slice(0, 7),
      summary: 'fix the parser',
      parents: [full('bbbb111')],
    })
    /** Every handler, recording which one fired. */
    const spy = () => {
      const fired = []
      const on = {
        compare: () => fired.push('compare'),
        swapSides: () => fired.push('swapSides'),
        compareWorkingTree: () => fired.push('compareWorkingTree'),
        compareWith: () => fired.push('compareWith'),
        revert: () => fired.push('revert'),
        cherryPick: () => fired.push('cherryPick'),
        reset: () => fired.push('reset'),
        tag: () => fired.push('tag'),
        branch: () => fired.push('branch'),
        detach: () => fired.push('detach'),
        amend: () => fired.push('amend'),
        copy: (oid) => fired.push(`copy:${oid}`),
      }
      return { fired, on }
    }
    /** Ids in order, separators as `--` — which is what makes the grouping assertable. */
    const ids = (entries) => entries.map((e) => (e.kind === 'separator' ? '--' : e.id))
    const item = (entries, id) => entries.find((e) => e.id === id)

    const atHead = spy()
    const headMenu = cm.commitMenu({
      row: commitRow(HEAD_OID),
      head: HEAD_OID,
      noClipboard: NO_CLIP,
      crossRepo: m.CROSS_REPO_COMPARE,
      pair: null,
      on: atHead.on,
    })
    const older = spy()
    const oldMenu = cm.commitMenu({
      row: commitRow(OLD_OID),
      head: HEAD_OID,
      noClipboard: NO_CLIP,
      crossRepo: m.CROSS_REPO_COMPARE,
      pair: null,
      on: older.on,
    })

    eq(
      ids(headMenu),
      [
        'compareWorkingTree', 'compareWith',
        '--', 'revert', 'cherryPick',
        '--', 'reset', 'amend',
        '--', 'tag', 'branch',
        '--', 'detach',
        '--', 'copyOid',
      ],
      'the tip row with one commit selected draws ten lines in five groups: read this against '
        + 'something, apply this patch elsewhere, rewrite what is here, point a new ref at it, go '
        + 'and stand on it — and then copy its name',
    )
    ok(
      ids(headMenu)[0].startsWith('compare'),
      '…and the READ-ONLY group is first. Everything below it writes to the repository and two of '
        + 'them can destroy work; the first item under the pointer when a menu opens is the one a '
        + 'slipped click lands on, and *Revert* was that item until this group existed',
    )
    eq(
      ids(oldMenu),
      [
        'compareWorkingTree', 'compareWith',
        '--', 'revert', 'cherryPick',
        '--', 'reset',
        '--', 'tag', 'branch',
        '--', 'detach',
        '--', 'copyOid',
      ],
      'and any older row draws the same list without *Amend*. Derived and not disabled: amending '
        + 'anything but the tip is an interactive rebase, which is a different act, and an item '
        + 'that is present and dead is a claim that something is possible here',
    )
    eq(
      headMenu.filter((e) => e.kind !== 'separator').map((e) => e.label),
      [
        'Compare with working tree',
        'Compare with…',
        'Revert',
        'Cherry-pick',
        'Reset here…',
        'Amend…',
        'Tag…',
        'New branch from here…',
        'Check out this commit (detached)',
        'Copy revision number',
      ],
      'the labels, verbatim. Four of them end in an ellipsis and six do not, and the difference '
        + 'is the promise: a `…` means something will ask before anything happens',
    )

    /* --- the same menu with two rows selected ------------------------------------------------------
     *
     * *Compare* and *Swap sides* are derived from the selection, exactly as *Amend* is derived
     * from the row. The failure this guards against is the ordinary one for a conditional item:
     * they appear on a one-row selection, where "compare" is about a pair that does not exist and
     * the swap flips nothing.
     */
    const PAIR = { newer: HEAD_OID, older: OLD_OID, sameRepo: true }
    const paired = spy()
    const pairMenu = cm.commitMenu({
      row: commitRow(HEAD_OID),
      head: HEAD_OID,
      noClipboard: NO_CLIP,
      crossRepo: m.CROSS_REPO_COMPARE,
      pair: PAIR,
      on: paired.on,
    })
    eq(
      ids(pairMenu).slice(0, 4),
      ['compare', 'compareSwap', 'compareWorkingTree', 'compareWith'],
      'two rows selected adds *Compare* and *Swap sides* at the head of the same group',
    )
    eq(
      ids(pairMenu).slice(4),
      ids(headMenu).slice(2),
      '…and changes nothing else about the menu. A conditional group that also reorders what is '
        + 'below it is how a user’s muscle memory for *Revert* becomes a click on *Reset here…*',
    )
    eq(
      [item(pairMenu, 'compare').label, item(pairMenu, 'compareSwap').label],
      ['Compare', 'Swap sides'],
      '…with those labels, verbatim',
    )
    eq(
      [item(headMenu, 'compare'), item(headMenu, 'compareSwap')],
      [undefined, undefined],
      'and neither is drawn for a one-row selection. Derived and not disabled, because "compare" '
        + 'with one row selected is not a precondition that is missing — it is a different '
        + 'gesture, and the two items beside it are the route to it',
    )
    item(pairMenu, 'compare').run()
    item(pairMenu, 'compareSwap').run()
    eq(paired.fired, ['compare', 'swapSides'], 'each runs its own action and no other')

    // --- the one refusal: two roots of a merged walk ------------------------------------------------

    const crossed = spy()
    const crossMenu = cm.commitMenu({
      row: commitRow(HEAD_OID),
      head: HEAD_OID,
      noClipboard: NO_CLIP,
      crossRepo: m.CROSS_REPO_COMPARE,
      pair: { ...PAIR, sameRepo: false },
      on: crossed.on,
    })
    eq(
      item(crossMenu, 'compare').disabledReason,
      m.CROSS_REPO_COMPARE,
      'two commits from two repositories cannot be diffed, and the item says which sentence '
        + '`logModel` gives for it — the SAME string the details pane shows, passed in the way '
        + '`noClipboard` is, so the menu and the pane cannot become two explanations of one refusal',
    )
    eq(item(crossMenu, 'compare').run, undefined, '…and carries no handler beside the reason')
    ok(
      ids(crossMenu).includes('compare'),
      '…but is still listed. Absent, it would be indistinguishable from the feature not existing: '
        + 'the user has two rows selected and the pane says nothing',
    )
    ok(
      item(crossMenu, 'compareSwap').run !== undefined,
      'while *Swap sides* stays live. It is state and not an act — it flips the header, costs '
        + 'nothing and is undone by pressing it again — and disabling it would leave the user with '
        + 'a header they cannot even reorder, which reads as a second failure',
    )
    ok(
      item(pairMenu, 'compare').disabledReason === undefined,
      'and a same-repository pair carries no reason at all, so the sentence means something when '
        + 'it does appear',
    )

    for (const [name, entries] of [['a paired selection', pairMenu], ['a cross-repo pair', crossMenu]]) {
      for (const entry of entries) {
        if (entry.kind === 'separator') continue
        ok(
          !(entry.disabledReason !== undefined && entry.run !== undefined),
          `${name}: \`${entry.id}\` does not carry both a disabledReason and a run`,
        )
        ok(
          entry.disabledReason !== undefined || entry.run !== undefined,
          `${name}: \`${entry.id}\` carries one of the two`,
        )
      }
    }

    // --- the two comparisons every row offers ---------------------------------------------------------

    for (const id of ['compareWorkingTree', 'compareWith']) {
      const probe = spy()
      const entries = cm.commitMenu({
        row: commitRow(OLD_OID),
        head: HEAD_OID,
        noClipboard: NO_CLIP,
        crossRepo: m.CROSS_REPO_COMPARE,
        pair: null,
        on: probe.on,
      })
      eq(item(entries, id).disabledReason, undefined, `\`${id}\` is never disabled`)
      item(entries, id).run()
      eq(probe.fired, [id], `…and runs its own action and no other`)
    }

    // --- nothing is listed and dead ---------------------------------------------------------------

    for (const [name, entries] of [['the tip row', headMenu], ['an older row', oldMenu]]) {
      for (const entry of entries) {
        if (entry.kind === 'separator') continue
        ok(
          !(entry.disabledReason !== undefined && entry.run !== undefined),
          `${name}: \`${entry.id}\` does not carry both a disabledReason and a run. `
            + '`resolveMenu` resolves `run` to null the moment a reason is present, so an item '
            + 'with both looks wired and does nothing at all when clicked',
        )
        ok(
          entry.disabledReason !== undefined || entry.run !== undefined,
          `${name}: \`${entry.id}\` carries one of the two. An item with neither is disabled by `
            + 'construction with the generic "Not available here", which is the sentence a caller '
            + 'is supposed to improve on',
        )
        if (entry.disabledReason !== undefined) {
          ok(
            entry.disabledReason.length > 12,
            `${name}: \`${entry.id}\`'s reason is a sentence rather than a word — a greyed-out `
              + 'line with no explanation is the failure `disabledReason` replaced `disabled` for',
          )
        }
      }
    }

    // --- amend, which the host may or may not be able to serve ---------------------------------

    eq(item(oldMenu, 'amend'), undefined, '*Amend…* is on no row but the tip')

    // Wired: the host fetches the message and parks it through `chrome/panelRequests`, and the
    // menu's only job is to say so. Asserted by *firing* it rather than by reading `run !== undefined`
    // — the ternary that chooses between the two arms could pick the wrong one and still leave a
    // function there.
    atHead.fired.length = 0
    item(headMenu, 'amend').run()
    eq(atHead.fired, ['amend'], '*Amend…* dispatches the host\'s amend action')
    eq(
      item(headMenu, 'amend').disabledReason,
      undefined,
      '…and is not also carrying a reason it cannot be clicked, which would draw it greyed out '
        + 'with a working handler behind it',
    )

    // Unwired: a window with no sidebar. The item stays on the row, because whether *this* window
    // can reveal a commit box is a different question from whether the commit is the tip, and
    // dropping the line would answer the second one wrongly.
    const noPanel = cm.commitMenu({
      row: commitRow(HEAD_OID),
      head: HEAD_OID,
      pair: null,
      sameRepo: true,
      detached: false,
      busy: null,
      noClipboard: false,
      on: { ...spy().on, amend: undefined },
    })
    eq(item(noPanel, 'amend').run, undefined, 'with no host seam *Amend…* dispatches nothing')
    eq(item(noPanel, 'amend').disabledReason, cm.AMEND_ELSEWHERE, '…and says so instead')
    ok(
      /Git panel/.test(cm.AMEND_ELSEWHERE) && /Amend/.test(cm.AMEND_ELSEWHERE),
      '…naming where to go and what to tick there. "Not available" would be a defect report; this '
        + 'is a direction',
    )

    // --- copy revision number ---------------------------------------------------------------------

    const noClip = cm.commitMenu({
      row: commitRow(OLD_OID),
      head: HEAD_OID,
      noClipboard: NO_CLIP,
      crossRepo: m.CROSS_REPO_COMPARE,
      pair: null,
      on: { ...spy().on, copy: undefined },
    })
    eq(
      item(noClip, 'copyOid').disabledReason,
      NO_CLIP,
      'with no clipboard the line is disabled and carries `menuModel.ts`’s own sentence — the same '
        + 'one *Copy path* uses in the tab strip and the project menu. WebKitGTK does not always '
        + 'expose one, and a copy that silently does nothing is indistinguishable from a dead item',
    )
    eq(item(noClip, 'copyOid').run, undefined, '…and carries no handler beside the reason')
    eq(
      ids(noClip).includes('copyOid'),
      true,
      '…but is still listed. Hidden, the user would conclude the app cannot copy an oid at all',
    )
    item(oldMenu, 'copyOid').run()
    eq(
      older.fired,
      [`copy:${OLD_OID}`],
      'with a clipboard it copies the FULL forty hex digits, not the seven the row shows. The gap '
        + 'is the entire reason the line exists — nobody can read the rest off the screen',
    )
    ok(OLD_OID.length === 40, '…and the fixture really is forty characters, so that means something')

    // --- what each line runs ------------------------------------------------------------------------

    for (const id of ['revert', 'cherryPick', 'reset', 'tag', 'branch', 'detach']) {
      const probe = spy()
      const entries = cm.commitMenu({
        row: commitRow(HEAD_OID),
        head: HEAD_OID,
        noClipboard: NO_CLIP,
        crossRepo: m.CROSS_REPO_COMPARE,
        // One row selected, which is every story in this section but the two-row one below.
        pair: null,
        on: probe.on,
      })
      item(entries, id).run()
      eq(probe.fired, [id], `\`${id}\` runs its own action and no other`)
    }

    eq(
      headMenu.filter((e) => e.danger === true).map((e) => e.id),
      ['reset'],
      'the red is on *Reset here…* and on nothing else. Not on the detached checkout: '
        + '`logActions::detachConfirm` marks that dialog `↗` rather than `−` precisely because '
        + 'nothing is destroyed, and spending the colour there is how it stops meaning anything '
        + 'on the line that opens a `--hard`',
    )

    // --- the reset dialog's answer, as a request ------------------------------------------------------

    for (const [type, fields] of [
      ['ResetRequest', ['target', 'kind', 'shelveFirst', 'force']],
      ['ReplayRequest', ['commit', 'mode', 'mainline']],
      ['TagRequest', ['name', 'target', 'message', 'force']],
    ]) {
      const body = stripComments(shape(type))
      ok(body !== '', `${type} exists in generated.ts`)
      for (const field of fields) {
        ok(new RegExp(`\\b${field}\\b`).test(body), `${type}.${field} is still on the wire`)
      }
      eq(
        body.match(/\b\w+(?=:)/g)?.length,
        fields.length,
        `…and ${type} still has exactly ${fields.length} fields. The builder in LogTab declares `
          + 'this shape structurally rather than importing it, so a field added in Rust is one '
          + 'the request would silently omit',
      )
    }

    eq(cm.resetKindOf('soft'), 'soft', 'the radio’s id IS the wire’s ResetKind — no second table')
    eq(cm.resetKindOf('mixed'), 'mixed', '…for all three')
    eq(cm.resetKindOf('hard'), 'hard', '…including the destructive one')
    eq(
      cm.resetKindOf(null),
      'soft',
      'and an unanswered dialog resolves to the mode that changes no file. This is why '
        + '`logActions::resetChoices` is ordered least-destructive-first',
    )
    eq(cm.resetKindOf('HARD'), 'soft', '…as does anything unrecognised, rather than being cast')

    eq(
      cm.pickedId([{ id: 'soft' }, { id: 'mixed' }, { id: 'hard' }], 'hard'),
      'hard',
      'the chosen mode is the chosen mode',
    )
    eq(
      cm.pickedId([{ id: 'soft' }, { id: 'mixed' }, { id: 'hard' }], null),
      'soft',
      '…and an untouched radio group is the first choice, which is how `ConfirmDestructive` '
        + 'paints it. The two must agree or the dialog describes one mode and performs another',
    )
    eq(cm.pickedId([], 'hard'), null, 'a dialog with no modes has no chosen one')

    eq(
      cm.resetRequestFor(HEAD_OID, cm.resetKindOf('mixed'), false),
      { target: HEAD_OID, kind: 'mixed', shelveFirst: null, force: false },
      'the request is built from the CHOSEN mode. This is the assertion that stands between a '
        + 'user picking Soft and cide sending Hard',
    )
    {
      const shelved = cm.resetRequestFor(HEAD_OID, cm.resetKindOf('hard'), true)
      eq(shelved.kind, 'hard', 'a hard reset asked for is a hard reset sent')
      ok(
        typeof shelved.shelveFirst === 'string' && shelved.shelveFirst.includes(HEAD_OID.slice(0, 8)),
        'and the shelve-first checkbox becomes a NAME. `ResetRequest.shelveFirst` is an '
          + '`Option<String>` rather than a bool plus a name precisely so there is no way to ask '
          + 'for a shelf without saying what to call it',
      )
      ok(
        !shelved.force,
        '…with `force` still false. It is the second click past a guard that has already listed '
          + 'what is at stake, and this dialog is the first',
      )
    }
    eq(
      cm.resetRequestFor(HEAD_OID, 'hard', false).shelveFirst,
      null,
      'an unticked box shelves nothing, which for a `--hard` is the path with no undo — and is '
        + 'exactly why `SHELVE_FIRST_DEFAULT` is true',
    )

    // --- revert and cherry-pick ------------------------------------------------------------------------

    eq(
      cm.replayRequestFor(OLD_OID, null),
      { commit: OLD_OID, mode: 'commit', mainline: null },
      'a replay is always in Commit mode and always starts with no mainline. Commit mode ADDS a '
        + 'commit, which is why the menu raises no dialog for it; working-tree mode writes into '
        + 'the tree the user is standing in and would need one, and there is no gesture for it here',
    )
    eq(cm.replayRequestFor(OLD_OID, 2).mainline, 2, 'the retry carries the parent the picker chose')

    eq(cm.mainlineOf('1'), 1, 'git numbers parents from 1 and so does the picker')
    eq(cm.mainlineOf('2'), 2, '…and the other side of a two-parent merge')
    eq(
      cm.mainlineOf('0'),
      null,
      '0 is not a parent. `git revert -m 0` is an error, and sending it would put the same picker '
        + 'back on screen',
    )
    for (const bad of [null, '', 'x', '-1']) {
      eq(
        cm.mainlineOf(bad),
        null,
        `\`${JSON.stringify(bad)}\` yields null and not NaN. \`JSON.stringify(NaN)\` is \`null\`, `
          + 'which on the wire is "no mainline given" — the exact refusal that raised the picker, '
          + 'so a NaN here is an infinite loop of dialogs the user cannot leave',
      )
    }

    // --- tags -------------------------------------------------------------------------------------------

    eq(
      cm.tagRequestFor(' v1.2.0 ', HEAD_OID, '', false),
      { name: 'v1.2.0', target: HEAD_OID, message: null, force: false },
      'an empty message box makes a LIGHTWEIGHT tag — `null`, never `""`. Which field is present '
        + '*is* the choice, so there is no `annotated` flag that could disagree with it',
    )
    eq(
      cm.tagRequestFor('v1.2.0', HEAD_OID, '   ', false).message,
      null,
      '…and a box with nothing but spaces in it is the same answer, not an annotated tag whose '
        + 'message is whitespace',
    )
    eq(
      cm.tagRequestFor('v1.2.0', HEAD_OID, 'the parser release', true),
      { name: 'v1.2.0', target: HEAD_OID, message: 'the parser release', force: true },
      'a message makes it annotated, which is what `git describe` and most release tooling need. '
        + '`force` is the second click, after `forceTagConfirm` has said where the tag points now',
    )

    // --- reading a refusal --------------------------------------------------------------------------------

    eq(
      cm.errorField({ kind: 'tagExists', detail: { name: 'v1', oid: HEAD_OID } }, 'tagExists', 'oid'),
      HEAD_OID,
      '`TagExists` carries where the tag points NOW, and that oid is the whole of the question '
        + '`forceTagConfirm` asks. "The name is taken" is not answerable; "move it off a1b2c3d?" is',
    )
    eq(
      cm.errorField({ kind: 'invalidTagName', detail: { name: 'v 1' } }, 'tagExists', 'oid'),
      null,
      '…and a different refusal is not mistaken for it, which is what stops a bad name opening a '
        + '*Move tag* dialog',
    )
    for (const bad of [null, undefined, 'boom', 42, {}, { kind: 'tagExists' }, { kind: 'tagExists', detail: null }]) {
      eq(
        cm.errorField(bad, 'tagExists', 'oid'),
        null,
        `a rejection of \`${JSON.stringify(bad) ?? 'undefined'}\` answers null rather than throwing. `
          + 'A rejection really can be `null`, and `(error as {kind}).kind` throws on it — turning '
          + 'a refusal the user could have answered into an unhandled rejection inside the catch '
          + 'block that was meant to handle it',
      )
    }
  }

  // --- the wiring, over stripped source -----------------------------------------------------------
  //
  // The model above says what the menu *decides*. These say it is connected: every bug in the
  // round that added the commit actions would have been a missing call rather than a wrong
  // function, and a model's own assertions cannot see one.

  {
    const tab = stripComments(readFileSync(join(UI, 'src', 'gitlog', 'LogTab.tsx'), 'utf8'))

    ok(
      /useContextMenu\(/.test(tab) && /commitMenu\(/.test(tab),
      'the Log tab opens a context menu and builds it from the model. `cide-git` shipped revert, '
        + 'cherry-pick, reset, tag and detached checkout with 33 differential tests against the '
        + 'real `git` and NOTHING in the app could call any of them',
    )
    ok(
      /rowMenu=\{\{ onContextMenu, menu \}\}/.test(tab),
      '…and hands the handle to `LogView`, rather than calling the hook inside the view. '
        + '`useContextMenu` reads the window keymap out of `@/store/workspace`, which imports '
        + '`layout/paneHosts`, which touches `document` at module scope — `check-log-render.mjs` '
        + 'server-renders that view',
    )
    for (const [object, method] of [
      ['commitActions', 'revert'],
      ['commitActions', 'cherryPick'],
      ['commitActions', 'resetPreview'],
      ['commitActions', 'reset'],
      ['commitActions', 'tag'],
      ['commitActions', 'checkoutDetached'],
      ['branch', 'create'],
    ]) {
      // `\\s*` between the two halves, because prettier breaks a promise chain across lines the
      // moment `.then` is attached — which is every one of these.
      ok(
        new RegExp(`\\b${object}\\s*\\.\\s*${method}\\(`).test(tab),
        `\`${object}.${method}(…)\` is actually called — the seam is used, not merely imported`,
      )
    }
    ok(
      /resetPreview\([\s\S]{0,400}?\.then\(\(preview\)/.test(tab),
      'the reset preview is fetched BEFORE the dialog opens, not while it closes. The Hard row '
        + 'says "DISCARD 3 changed files" and has to name all three while the user is still '
        + 'deciding — which is why `git_reset_preview` is a read with no broadcast of its own',
    )
    ok(
      /checkoutDetached\(project, repo, row\.oid, mode\)/.test(tab)
        && /'refuse'/.test(tab)
        && /'stashAndRestore'/.test(tab),
      'the detached checkout asks with `refuse` first and only offers a stash after the refusal '
        + 'named the files. Offering the stash up front would stash a working tree that never '
        + 'needed moving — the rule `branch.checkout` documents and this call inherits',
    )
    ok(
      /registerTagDialog\(\(target\)/.test(tab) && /registerTagDialog\(null\)/.test(tab),
      'the tag dialog is registered on mount and cleared on unmount. NOTHING anywhere registered '
        + 'one before this, so `git.tag.new` — a listed palette command — reported "nothing in '
        + 'this window can ask what to call the tag" every single time it was run',
    )
    ok(
      /openTagDialog\(row\.oid\)/.test(tab),
      '…and the row menu goes through the same seam rather than reaching for `setPrompt`, so the '
        + 'menu, the palette row and any future binding open one dialog and not three',
    )
    for (const helper of ['replayNote(', 'resetNote(', 'detachNote(', 'tagNote(']) {
      ok(
        tab.includes(helper),
        `every outcome reports through \`${helper}…\`. An action that succeeds silently is `
          + 'indistinguishable from one that did nothing, and the log looks identical after all '
          + 'six of these',
      )
    }
    ok(
      !/function explain\(/.test(tab),
      'and the cut-down `explain` this file used to carry is gone. `chrome/branchModel.ts` has an '
        + 'arm for all fifteen of the commit actions’ refusals; two unpackings would mean the '
        + 'page-load failure and the menu failure describing the same `GitError` two ways',
    )
    ok(
      /explain\(error\)/.test(tab) && !/String\(error\)/.test(tab),
      '…and nothing reaches the user as `String(error)`, which for a `{kind, detail}` object is '
        + 'the literal text `[object Object]`',
    )

    /* --- comparing two commits: the wiring (M20) -------------------------------------------------
     *
     * Two backend commands shipped with **zero** callers — `git_diff_revision_files` and
     * `git_resolve_rev` — which is the same shape of gap the commit actions had one milestone
     * ago: tested in Rust, reachable from nothing. The model above says what the gesture decides;
     * these say it is connected, and a missing call is exactly the bug a model's own assertions
     * cannot see.
     */

    const view = stripComments(readFileSync(join(UI, 'src', 'gitlog', 'LogView.tsx'), 'utf8'))

    for (const [object, method] of [
      ['gitLog', 'diffFiles'],
      ['gitLog', 'resolve'],
    ]) {
      ok(
        new RegExp(`\\b${object}\\s*\\.\\s*${method}\\(`).test(tab),
        `\`${object}.${method}(…)\` is actually called. It had no caller at all before this — a `
          + 'command that is tested against the real `git` and reachable from nothing is a feature '
          + 'that does not exist',
      )
    }
    for (const fn of ['selectRow(', 'comparePair(', 'swapPair(']) {
      ok(
        tab.includes(fn),
        `the tab goes through \`logModel::${fn}…\` rather than deciding it inline. That is what `
          + 'makes the rule drivable from here at all; a `switch` on `e.ctrlKey` inside an '
          + '`onClick` is reviewable only by reading it',
      )
    }
    ok(
      /secondary=\{selection\.secondary\}/.test(tab),
      'and the second endpoint reaches the list, so BOTH selected rows are drawn selected. The '
        + 'pane is showing the range between them and a list that highlighted one end would be '
        + 'half a description of what is on screen',
    )
    ok(
      /crossRepo: CROSS_REPO_COMPARE/.test(tab) && /pair:/.test(tab),
      'the row menu is handed the pair and the refusal sentence, so *Compare* and *Swap sides* '
        + 'appear on exactly the selection they are about',
    )
    ok(
      /<RangeDetails/.test(tab) && /export function RangeDetails/.test(view),
      'the two-revision pane is `LogView`’s and not the wiring half’s. `check-log-render.mjs` '
        + 'server-renders that file and cannot load this one, so a compare header assembled here '
        + 'would be a header no check could look at',
    )
    ok(
      /export function ChangedFileList/.test(view)
        && /<ChangedFileList/.test(tab)
        && /<ChangedFileList/.test(view),
      'and BOTH file lists are the same component — the whole list since M21, not just the row. '
        + 'Grouping, the indent and the summary line had to join the row inside it, and a second '
        + 'assembly of those would be a second place for the two panes to disagree about what '
        + 'they are showing. The click rule on those rows — one click selects, two open — is '
        + '`logFileClick`, and a second spelling of it is a second thing to keep in step with '
        + '`sidebar/clickSemantics.ts`',
    )
    ok(
      /gestureOf\(detailCount\)/.test(tab) && /logFileClick\(/.test(tab),
      '…routed through one `openRevisionFile`, which is the only place either list turns a click '
        + 'count into a decision. Two call sites is how one of the two lists ends up with a '
        + 'different answer to the same gesture',
    )
    ok(
      /openRevisionFile\(compare\.repo, compare\.next, compare\.prev, file, oldPath, detailCount\)/
        .test(tab),
      '…and the range’s rows carry `oldPath` through. Under the older side a renamed file is at '
        + 'its OLD path, and asking for the new one there answers "added, whole file" — a wall of '
        + 'green with no error anywhere',
    )

    // The two illegal pairings, which Rust refuses by name (`cide_git::revision`'s header has the
    // table). Nothing here should ever be able to construct one, so the assertion is about the
    // source rather than about a runtime guard: there is no runtime guard, on purpose.
    ok(
      /next: \{ kind: 'workingTree' \}/.test(tab),
      '`WorkingTree` is built as the NEWER side',
    )
    ok(
      !/\bprev[^\n]*'workingTree'/.test(tab),
      '…and never as the older one. `RevSide::WorkingTree` as `old` is refused by name in Rust, so '
        + 'a swap that produced it would be a control whose only outcome is a typed error',
    )
    ok(
      !/\bnext[^\n]*'firstParent'/.test(tab),
      'and `FirstParent` is never the newer side either — it is the first parent *of the other '
        + 'side*, so it is meaningless there and Rust says so',
    )
    ok(
      /swappable: false/.test(tab),
      'which is why the working-tree comparison offers no ⇄ Swap at all. Derived and not '
        + 'disabled, the same rule *Amend* follows',
    )
  }
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) console.error(`\ncheck-log: ${failed} failure(s)`)
else console.log(`check-log: ok (${checked} assertions)`)
process.exit(failed > 0 ? 1 : 0)
