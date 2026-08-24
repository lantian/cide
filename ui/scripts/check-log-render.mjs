/**
 * Renders the commit log under node and checks what came out.
 *
 * `pnpm build` proves the log compiles; this proves it paints. It SSR-bundles
 * `src/gitlog/logSmoke.tsx`, runs it, and asserts on the digest it prints: that the mock story
 * renders rows at all, that each row carries its hash, subject, author and date, that the repo
 * strip appears with two roots and not with one, that the graph gutter is drawn parallel to the
 * rows or not at all, and that the three empty states say three different things.
 *
 * Every story goes through the real generated `CommitPage` shape and the real `logStatus` — so
 * this covers the seam that used to be uncovered next door: `sidebar/GitPanel`'s fixtures were
 * once written in the panel's *own* invented shape, which is why they rendered beautifully while
 * the panel was empty against every real repository. `mock.rows > 0` is the assertion that
 * catches the day that happens here, because a fixture in the wrong shape produces exactly zero.
 *
 * Companion to `check-log.mjs`, which drives the model directly. That one owns the rules; this
 * one owns the claim that the rules reach the DOM.
 *
 * Since M19 it also owns one invisible thing: **`data-row-id` on every row**. `LogTab` mounts the
 * commit context menu and resolves a right-click by walking up to the nearest one, so a row
 * without it opens a menu about nothing — and a row with it and a row without look identical, in
 * both themes, at every size. The menu handle itself is optional and this render passes none,
 * which is deliberate: `useContextMenu` reads the window keymap out of a store that touches
 * `document` at module scope, and a `LogView` that called it could not be rendered here at all.
 *
 * Since M20 it owns the **two-revision details pane**: that a two-row selection marks both rows,
 * that the header names the two ends oldest-first, that ⇄ Swap is beside it, and that the file
 * list is the range's and not the anchor commit's. The `compare` story holds a `CommitDetail` and
 * a `RevisionRange` at once — which is the state the real tab is in — so "the wrong half was
 * rendered" is a mistake that produces a plausible list of the wrong files rather than a blank.
 *
 * Two globals are stubbed before the bundle loads. `LogView` is pure and needs neither, but
 * `react-dom/server` is kept external and the module graph around it is not this file's to
 * control. Neither is faked deeper than that — anything that needs a real browser is not
 * something this check can speak to.
 *
 * What this does NOT cover:
 *   - layout. Class names are hashed by the CSS-modules transform and no stylesheet is applied,
 *     so "the gutter is 84px wide" is not a question that can be asked here. `./run.sh
 *     --audit-chrome` is the surface for that.
 *   - interaction. Nothing is clicked. That a row's `onSelect` fetches a detail is `LogTab`'s,
 *     and it is not server-renderable by construction.
 *
 * Run: `pnpm --dir ui run check:log-render`
 */
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'

/*
 * Built inside `node_modules/.cache` rather than the system temp dir. The bundle keeps
 * `react-dom/server` external, so node resolves it relative to wherever the output sits: under
 * /tmp there is no `node_modules` above it and the import fails.
 */
mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync(join('node_modules/.cache', 'cide-logrender-'))

/*
 * The exit code is carried out of the `try` rather than taken there with `process.exit`:
 * `process.exit` skips `finally`, and this build directory lives under `node_modules`, so every
 * failing run would leave one behind.
 */
let failed = 0
let checked = 0
try {
  execFileSync(
    'node',
    [
      'node_modules/vite/bin/vite.js',
      'build',
      '--ssr', 'src/gitlog/logSmoke.tsx',
      '--outDir', out,
      '--logLevel', 'error',
    ],
    { stdio: 'inherit' },
  )

  globalThis.window = globalThis
  globalThis.location = { search: '' }
  const printed = []
  const log = console.log
  console.log = (line) => printed.push(line)
  await import(`file://${resolve(out, 'logSmoke.js')}`)
  console.log = log

  const byName = Object.fromEntries(JSON.parse(printed.at(-1)).map((d) => [d.story, d]))

  const eq = (actual, expected, what) => {
    checked += 1
    const a = JSON.stringify(actual)
    const b = JSON.stringify(expected)
    if (a !== b) {
      console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
      failed++
    }
  }
  const ok = (cond, what) => eq(cond, true, what)

  eq(
    Object.keys(byName).sort(),
    [
      'budget',
      'compare',
      'detail',
      'empty',
      'failed',
      'filtered',
      'filteredEmpty',
      'graphOff',
      'history',
      'loading',
      'mock',
      'multi',
      'nested',
      'revealOutside',
      'revealed',
    ],
    'every story rendered — a story that throws takes the whole digest with it, so the roll call '
      + 'comes first',
  )

  // --- it paints at all ---------------------------------------------------------------------

  ok(
    byName.mock.rows > 0,
    'the mock renders rows. A fixture that reaches the view in the wrong shape produces exactly '
      + 'zero, which is what the empty-panel bug looked like for a whole milestone next door',
  )
  eq(byName.mock.rows, 6, "…six of them, which is the mock history's length")

  // --- each row says who, what, when ------------------------------------------------------------

  for (const cell of byName.mock.cells) {
    const [oid, subject, author, date] = cell.split('|')
    ok(/^[0-9a-f]{7}$/.test(oid), `a row leads with its short hash (${JSON.stringify(cell)})`)
    ok(subject.length > 0, `…carries its subject (${JSON.stringify(cell)})`)
    ok(author.length > 0, `…names its author (${JSON.stringify(cell)})`)
    ok(date.length > 0, `…and dates it (${JSON.stringify(cell)})`)
    ok(
      !/ago|Invalid|NaN/.test(date),
      `…absolutely, and not as "Invalid Date" from an unnarrowed bigint (${JSON.stringify(cell)})`,
    )
  }

  /* --- the row's identity, for the context menu ------------------------------------------------
   *
   * `LogTab` mounts the menu and resolves a right-click by walking up to the nearest
   * `[data-row-id]`, exactly as `sidebar/GitPanel/GitPanelHost.tsx` does. Nothing about that is
   * visible: a row with the attribute and a row without look identical, and what breaks without
   * it is every commit action on every row, in silence.
   */

  eq(
    byName.mock.rowIds.length,
    byName.mock.rows,
    'every row carries a `data-row-id` on its own outermost element — the handle the commit menu '
      + 'resolves a right-click through. One missing is one row whose menu opens about nothing',
  )
  ok(
    byName.mock.rowIds.every((id) => /^[^:]+:[0-9a-f]{40}$/.test(id)),
    '…as `repo:oid`, with the FULL forty-hex oid. `CommitRow.shortOid` is a display abbreviation '
      + 'and every follow-up call passes the full one back, so a handle built from the short form '
      + 'would resolve to a row and then act on a truncated revision',
  )
  eq(
    new Set(byName.mock.rowIds).size,
    byName.mock.rows,
    '…and they are distinct, which is what makes the lookup a lookup',
  )
  eq(
    new Set(byName.multi.rowIds.map((id) => id.split(':')[0])).size,
    2,
    'a merged walk’s handles name two different repositories. That prefix is the whole reason the '
      + 'handle is not the oid alone: two roots can hold the same commit — a vendored submodule, a '
      + 'fork — and the action has to run in the one the row came from',
  )
  eq(
    byName.empty.rowIds,
    [],
    'and a list with no rows has no handles, rather than one for a row that is not drawn',
  )

  eq(
    byName.mock.cells[0].split('|')[0],
    'a1b2c3d',
    'the newest commit is first — the list is newest-first and the fixture is written that way',
  )
  ok(
    !/\b20\d\d\b/.test(byName.mock.cells[0].split('|')[3]),
    'a commit from the story’s own year omits the year',
  )
  ok(
    /\b2019\b/.test(byName.mock.cells[4].split('|')[3]),
    '…and an older one prints it, which is the boundary `check-log.mjs` owns and this one shows '
      + 'reaching the row',
  )

  // --- ref chips -------------------------------------------------------------------------------

  eq(
    byName.mock.chips.slice(0, 3),
    ['head:HEAD', 'localBranch:main', 'remoteBranch:origin/main'],
    'the tip’s chips, in `orderChips` order, with their kind on the element',
  )
  ok(
    byName.mock.chipClasses.every((n) => n === 2),
    'every kinded chip carries two classes — the base one and its kind. One class means '
      + '`styles[kind]` resolved to undefined, which is a chip that renders in the wrong colour '
      + 'and looks deliberate',
  )
  eq(
    byName.mock.chipOverflow,
    ['+38'],
    'the release commit’s forty tags become three chips and a `+38`. Forty chips would push the '
      + 'subject — the only column anyone scans — off the row',
  )
  eq(
    byName.mock.cells[4].split('|')[1],
    'A pane that reattaches gets the buffer it left',
    '…and the subject is still there beside them, which is the thing the cap is protecting',
  )

  // --- a chip is a control, or it is text — never a control that declines to act (M21) ---------
  //
  // `logModel::chipTarget` owns the rule and `check-log.mjs` drives it directly; what is asserted
  // here is the only half that file cannot see — that the two answers reach the DOM as two
  // different elements. A `<span>` where a `<button>` belonged is a feature that silently is not
  // there; a `<button>` where a `<span>` belonged hovers like a control and then does nothing,
  // which is the dead-menu-item state `cide-core::commands` refuses to represent.

  eq(
    byName.mock.chipTags.slice(0, 3),
    ['span', 'span', 'button'],
    'the default walk starts at HEAD, so the tip’s HEAD chip and its current branch are text — '
      + 'clicking either would re-root on the walk that is already on screen — while `origin/main` '
      + 'is a control, because a remote-tracking ref is a different question that will give a '
      + 'different answer after the next fetch',
  )
  eq(
    byName.mock.chipTitles[0],
    'HEAD\nThe log already starts here.',
    'and the inert one says so in its tooltip. A `<span>` and a `<button>` differ by a cursor and '
      + 'a hover underline and by nothing at all in a screenshot, so the sentence is what stops '
      + 'the inert case reading as a control that is broken',
  )
  eq(
    byName.mock.chipTitles[2],
    'refs/remotes/origin/main\nStart the log here.',
    '…and the live one keeps the full refname as its first line, which is the disambiguation the '
      + 'chip carried `full` for: `main` alone names two refs',
  )
  eq(
    byName.multi.chipTags,
    ['button', 'button', 'button', 'button'],
    'with the walk re-rooted on `release/0.18`, every chip on screen is live — including the HEAD '
      + 'chip, which is the only one that can put the walk back on the current checkout and is the '
      + 'reason it is not unconditionally inert',
  )
  eq(
    byName.multi.chipTitles[0],
    'HEAD\nStart the log at the current checkout.',
    '…and it says which of the two things it does, because "start the log here" would be wrong '
      + 'for the one chip that means "go back"',
  )
  eq(
    byName.mock.chipMoreIsButton,
    false,
    'the `+38` chip is never a control: it names a count, not a ref, so there is nothing for it '
      + 'to re-root the walk on. Expanding the row into its full list of refs would be a '
      + 'reasonable feature and a different one',
  )
  eq(
    byName.mock.chipTags.length,
    byName.mock.chips.length,
    'every chip that carries a `data-kind` was also counted as an element — the two regexes read '
      + 'the same set, so a chip drawn as some third thing cannot slip past both',
  )

  // --- the repo strip ----------------------------------------------------------------------------

  eq(byName.mock.repoStrip, false, 'one repository draws no strip: every row would say the same thing')
  eq(
    byName.multi.repoStrip,
    true,
    'two roots do, because a merged walk has no graph — `cide_git::log` reports `GraphOff::Merged`, '
      + 'two repositories sharing no DAG — and the chip is what carries the structure instead',
  )
  eq(
    [...new Set(byName.multi.repoChips)].sort(),
    ['cide', 'hub-core'],
    'and it names the repository each row came from, out of `CommitRow.repo`',
  )
  eq(
    byName.multi.graphCells,
    0,
    'with no graph beside it — the strip appearing and the gutter vanishing are two halves of one '
      + 'fact, decided by one count',
  )
  eq(
    byName.multi.refNote,
    'No “release/0.18” in docs-site.',
    'the third root has no such branch. The page still has rows, so `logStatus` is null, and this '
      + 'note is the only place a per-repository `NoSuchRef` can be seen',
  )

  // --- the graph ---------------------------------------------------------------------------------

  eq(
    byName.mock.graphCells,
    byName.mock.rows,
    'the gutter is drawn once per row. Parallel or absent, never in between: a graph one row out '
      + 'of step is worse than no graph, because it is wrong rather than missing',
  )
  eq(byName.mock.graphOff, null, 'and nothing to explain while it is on')
  eq(byName.graphOff.graphCells, 0, '`--full-history` over a path has no gutter at all')
  eq(
    byName.graphOff.graphOff,
    'The graph is hidden in full-history mode.',
    '…and says why, rather than leaving a blank column that reads as a history with no branches',
  )
  eq(
    byName.filtered.graphOff,
    'The graph is hidden while a filter is on.',
    'a filtered list is a subsequence, not a DAG — drawing edges between its rows would assert a '
      + 'reachability nothing computed',
  )
  eq(
    byName.multi.graphOff,
    'The graph is hidden when several repositories are merged.',
    'and a merged one says its own reason',
  )
  ok(
    new Set([byName.graphOff.graphOff, byName.filtered.graphOff, byName.multi.graphOff]).size === 3,
    'three reasons, three sentences',
  )

  // --- the three empty states -------------------------------------------------------------------

  eq(byName.empty.rows, 0, 'an unborn HEAD renders no rows')
  eq(byName.empty.status, 'This repository has no commits yet.', '…and says so, verbatim')
  eq(byName.loading.rows, 0, 'a first load renders none either')
  eq(byName.loading.status, 'Reading the log…', '…and says *that*, verbatim')
  eq(
    byName.failed.status,
    'could not read /home/dev/work/cide/.git/HEAD: Permission denied',
    'a failure shows the message and not `[object Object]` — a `GitError` is a tagged object and '
      + '`String(error)` is the bug `check:branches` exists for',
  )
  eq(
    new Set([byName.empty.status, byName.loading.status, byName.failed.status]).size,
    3,
    'and the three are three different sentences. Two states that say the same thing are '
      + 'indistinguishable to the user and invisible in a screenshot',
  )

  eq(
    byName.filteredEmpty.status,
    'No commit matches these filters.',
    'a filter that matched nothing says *that*, and not "this repository has no commits yet" — '
      + 'the two are the states most easily confused and they call for opposite next moves',
  )
  eq(
    byName.filteredEmpty.clearEmpty,
    true,
    '…and offers the way out beside the sentence, not only in the bar three rows above it',
  )
  eq(byName.empty.clearEmpty, false, 'while an unfiltered empty list has nothing to clear')
  eq(
    new Set([
      byName.empty.status,
      byName.loading.status,
      byName.failed.status,
      byName.filteredEmpty.status,
    ]).size,
    4,
    'four empty-ish states, four sentences',
  )

  // --- the filter bar ------------------------------------------------------------------------------

  eq(
    byName.mock.controls,
    ['logBranch', 'logAuthor', 'logText', 'logRefresh'],
    'branch, author, text, refresh — in that order, and all four present on an unfiltered log',
  )
  eq(byName.mock.branch, 'head', 'the branch control starts on the current branch')
  eq(
    byName.multi.branch,
    'b:release/0.18',
    '…and a chosen branch is tagged `b:`, so a branch actually named `head` cannot silently '
      + 're-root onto HEAD',
  )
  eq(byName.mock.clear, false, 'no *Clear filters* until something is set')
  eq(byName.filtered.clear, true, 'and one as soon as something is')
  ok(
    byName.filtered.rows < byName.mock.rows,
    'the filtered story is a narrowing of the mock and not a different list',
  )
  ok(byName.filtered.rows > 0, '…and not the empty list, which would satisfy the inequality for the wrong reason')
  eq(
    byName.filtered.status,
    null,
    'with rows on screen there is no sentence over the top of them',
  )
  eq(
    byName.multi.clear,
    true,
    'a branch that is not HEAD counts as a filter too — otherwise the only way back is to know '
      + 'which control did it',
  )

  // --- the foot of the list ------------------------------------------------------------------------

  eq(
    byName.mock.more,
    null,
    'an exhausted page offers nothing to press. `resume` is the authority for that and `stop` '
      + 'never is',
  )
  eq(
    byName.budget.more,
    `Searched ${(20000).toLocaleString()} commits — keep looking?`,
    'a budgeted walk offers to keep looking. Drawing `budget` as the end of history is how the '
      + 'answer the user was searching for is silently lost, which is the whole reason the budget '
      + 'is designed the way it is',
  )
  ok(
    !/no more|end of/i.test(byName.budget.more),
    '…and does not claim the history is over',
  )
  eq(
    byName.budget.graphCells,
    byName.budget.rows,
    'and the truncated page still draws its gutter, dangling edge and all',
  )

  // --- the details pane ------------------------------------------------------------------------------

  eq(byName.detail.detailFiles, 3, "the selected commit's three changed files reach the pane")
  eq(byName.mock.detailFiles, 0, 'and nothing is drawn there before a commit is picked')

  /* --- comparing two commits (M20) --------------------------------------------------------------
   *
   * The whole point of the gesture is that the details pane stops being about one commit. Every
   * assertion below is about a thing that is invisible when it goes wrong in the usual way: a
   * header that still names one commit, a swap control that a conditional quietly dropped, a file
   * list that came from `CommitDetail` instead of `RevisionRange` — which typechecks, because
   * `LogTab` holds both at once, and renders a plausible list of the wrong files.
   */

  eq(
    byName.compare.selectedOids,
    ['a1b2c3d', 'd4e5f60'],
    'both ends of a comparison are marked `aria-selected`, in the list\u2019s own order. The pane '
      + 'beside them is showing the range BETWEEN them, and a list that highlighted one end would '
      + 'be half a description of what is on screen',
  )
  eq(
    byName.detail.selectedOids,
    ['a1b2c3d'],
    '\u2026while an ordinary selection marks exactly one, which is the state every other story is '
      + 'in and the one this must not have changed',
  )
  eq(byName.mock.selectedOids, [], 'and an untouched list marks none')

  eq(
    byName.compare.rangeHeader,
    'Comparing d4e5f60 \u2026 a1b2c3d',
    'the header says which two, oldest on the LEFT \u2014 the direction time runs in every range '
      + 'expression git accepts (`old..new`) and the direction the patch is computed in. The '
      + 'fixture selects the newer commit as the anchor and the older as the second endpoint, and '
      + '`comparePair` orders them from the page rather than from that',
  )
  eq(
    byName.detail.rangeHeader,
    null,
    '\u2026and a one-commit selection has no such header. The two panes are exclusive: a story '
      + 'showing both would be a pane that had grown a second answer instead of switching',
  )
  eq(
    byName.compare.rangeSwap,
    true,
    'the \u21c4 Swap control is beside it. Which side is which is a guess whenever an endpoint is '
      + 'not on the loaded page \u2014 `git rev-parse` answers with an oid and no date \u2014 so '
      + 'the control is the correction, and without it the guess is final',
  )
  eq(byName.detail.rangeSwap, false, '\u2026and nothing to swap when one commit is selected')

  eq(
    byName.compare.rangeSummaries,
    'Ignored files are shown by default, including on disks that already said no \u2192 '
      + 'The awaiting chip\u2019s geometry is a gate, not an eyeball',
    'the two endpoints name themselves, old \u2192 new, in the same direction as the header. They '
      + 'come from `RevisionRange.newSummary`/`oldSummary` and not from the loaded page, because '
      + '*Compare with\u2026* can name a revision that is not in it',
  )

  eq(
    byName.compare.rangeCount,
    '4 files',
    'the range\u2019s own file count \u2014 four, which is `RevisionRange.files.length` and not '
      + 'the three `CommitDetail` is holding for the same story. Those three are what a pane that '
      + 'rendered the wrong half would show, and they would look entirely reasonable',
  )
  // Against the FLAT reading, which is the one that shows whole paths in the range's own order.
  // Grouping is the default and reorders the rows under their directories, so these claims —
  // which are about `RevisionChange.oldPath` and about ordering — belong to the arrangement that
  // preserves both. The grouped reading is asserted on its own terms further down.
  eq(
    byName.compare.flatLabels,
    [
      'ui/src/panes/AwaitingChip.tsx',
      'ui/src/panes/awaiting.ts',
      'crates/cide-fs/src/index.rs',
      'ui/src/panes/chipGeometry.ts \u2192 ui/src/panes/awaitingChip.ts',
    ],
    'every changed file is a row, in the range\u2019s order, and the renamed one draws both paths. '
      + '`RevisionChange.oldPath` is what makes that diff against the right blob: under the older '
      + 'side the file is at its old path, and asking for the new one there answers "added, whole '
      + 'file" \u2014 a wall of green with no error anywhere',
  )
  eq(
    byName.compare.rangeCounts,
    ['+41 \u22129', '+23 \u22123', '+0 \u22120', '+7 \u2212112'],
    'each row carries `+n \u2212m` from `additions`/`deletions`. A real minus sign and not a '
      + 'hyphen, as everywhere else in this app \u2014 and the counts are per file, which is what '
      + '`RevisionChange` carries and `CommitFile.lines` does not always have. The order is the '
      + 'grouped one, because that is what is on screen: the counts follow their rows',
  )
  ok(
    byName.compare.rangeCounts.length === byName.compare.groupedLabels.length,
    '\u2026one per row, so a count cannot end up beside the wrong path',
  )

  // --- grouped and flat are two readings of one list (M21) --------------------------------------

  eq(
    byName.compare.fileDirs,
    ['ui/src/panes', 'crates/cide-fs/src'],
    'a directory heading per directory, in the order each one\u2019s first file appeared. Not '
      + 'alphabetical: re-sorting would make the two arrangements disagree about which file is '
      + 'first, which is two views of one commit that cannot be compared by eye',
  )
  eq(
    byName.compare.flatFileDirs,
    [],
    '\u2026and the flat reading draws none at all, which is the whole difference between them',
  )
  eq(
    [...byName.compare.groupedLabels].sort().length,
    byName.compare.flatLabels.length,
    'both arrangements draw the same number of files \u2014 grouping regroups, it never hides',
  )
  eq(
    byName.compare.groupedLabels,
    [
      'AwaitingChip.tsx',
      'awaiting.ts',
      'chipGeometry.ts \u2192 awaitingChip.ts',
      'index.rs',
    ],
    'grouped, a row is its basename \u2014 the heading above it already says the directory, and '
      + 'repeating it is what made a list of thirty paths mostly prefix. The rename keeps its '
      + 'arrow with both basenames, because the two halves are in the same directory; a rename '
      + 'that crossed directories would show the old path whole instead',
  )
  ok(
    byName.compare.treeToggle && byName.compare.treeTogglePressed,
    'the grouping control is drawn and reads as pressed while grouping is on',
  )
  ok(
    !byName.compare.flatTogglePressed,
    '\u2026and not pressed when it is off. `aria-pressed` is the state a screen reader gets, and '
      + 'a toggle that never changes it is a control that silently does nothing to anyone not '
      + 'looking at the icon',
  )
  ok(
    !byName.mock.treeToggle,
    'and there is no control at all with no files to arrange, rather than one that is drawn dead',
  )

  // --- the list reads as a file list (M21) ------------------------------------------------------
  //
  // It was plain text rows, which is what a *summary* looks like. It is a list you click, select
  // in and open from, so it draws what the file tree draws.

  eq(
    byName.compare.fileIcons,
    4,
    'every file row carries a `FileIcon`, from the same table the file tree reads. A row that '
      + 'lost its icon still renders its text, so nothing else in the suite would notice',
  )
  eq(
    byName.compare.dirIcons,
    2,
    '\u2026and every directory heading carries a folder icon, so a heading here and the same '
      + 'directory in the tree are the same picture',
  )
  eq(
    byName.compare.selectedRow,
    'AwaitingChip.tsx',
    'the selected row is marked with `aria-current`, not by CSS alone \u2014 a highlight a screen '
      + 'reader is not told about is a selection that does not exist for anyone using one',
  )
  eq(
    (byName.compare.fileIcons ?? 0) + (byName.compare.dirIcons ?? 0),
    byName.compare.groupedLabels.length + byName.compare.fileDirs.length,
    'one icon per row and no more: an extra `<img>` anywhere in a row would draw a second glyph '
      + 'beside the first',
  )

  // --- the divider is a fraction, and it comes from the tab (M21) -------------------------------
  //
  // > *"such git log panel … should be devided by 50%/50% … coz we currently checking diff of
  // > one file"*
  //
  // The details column was `var(--w-log-details)` — a fixed 420px whatever the tab was for — and
  // `ToolWindowState::log_split` was stored, clamped, round-tripped and read by nobody. So the
  // panel could not be 50/50 for a History tab and 45/55 for the Log tab, because it could not
  // be anything but 420px for either.

  eq(
    byName.mock.logColumns,
    'minmax(0, 450fr) minmax(0, 550fr)',
    'the grid is two fractions built from the tab\u2019s split, not a fixed details width. 450 is '
      + '`LOG_SPLIT_DEFAULT`, which the fixture supplies as the Log tab\u2019s',
  )
  ok(
    byName.mock.logColumns?.includes('minmax(0,'),
    '\u2026with `minmax(0, …)` on both tracks, or a long path in either half pushes the other off '
      + 'the panel',
  )

  // --- the nesting is visible (M21) -------------------------------------------------------------
  //
  // > *"files under the folder are on the same line, but it should have more space, like in
  // > file tree"*
  //
  // The first attempt indented by 21px of `padding-left` and produced no visible nesting at all,
  // by arithmetic rather than by taste: a heading is `padding 6 + twisty 9 + gap 6 + icon 15 +
  // gap 6` before its label, so its label sits at 42px — and a nested file at `padding-left: 21`
  // with no twisty slot put *its* label at 21 + 15 + 6 = 42px too. Exactly aligned, which is what
  // "on the same line" describes. Nothing in the suite could see it, because both readings drew
  // the right rows in the right order.

  eq(
    byName.compare.rowIndents,
    ['19px', '19px', '19px', '19px'],
    'every file in this story is one level deep, so each carries one INDENT of margin on its '
      + 'twisty slot. Zero here is the flat list that was reported twice',
  )
  eq(
    byName.compare.rowIndents?.every((m) => m === '19px'),
    true,
    '\u2026and it is 19px \u2014 `sidebar/FileTree.tsx`\u2019s `INDENT`, measured off the IDEA '
      + 'reference, shared with `GitPanel/ChangesTree.tsx`. A number invented here is how the two '
      + 'trees ended up indenting by 12 and 14',
  )

  // --- folding a directory (M21) ----------------------------------------------------------------
  //
  // Rendered a third time with the first directory folded, because a prop that exists and hides
  // nothing type-checks perfectly.

  eq(
    byName.compare.dirExpanded,
    ['true', 'true'],
    'every heading is a disclosure and says it is open',
  )
  eq(
    byName.compare.foldedExpanded,
    ['false', 'true'],
    '\u2026and the folded one says it is shut, while its neighbour is untouched. `aria-expanded` '
      + 'and not `aria-pressed`: the row owns the rows beneath it, which is a disclosure rather '
      + 'than a toggle',
  )
  eq(
    byName.compare.foldedDirs,
    byName.compare.fileDirs,
    'folding hides files, never the heading \u2014 a heading that vanished with its contents '
      + 'would leave no way to bring them back',
  )
  eq(
    byName.compare.foldedLabels,
    ['index.rs'],
    'the folded directory\u2019s three files are gone and the other directory\u2019s file stays. '
      + 'This is the assertion the prop alone could not make',
  )
  ok(
    (byName.compare.foldedLabels?.length ?? 0) < byName.compare.groupedLabels.length,
    '\u2026so folding really does remove rows',
  )
  // --- it is a TREE, not a list of directories (M27) --------------------------------------------
  //
  // > *"changed files in commit view (bottom panel) currently has wrong tree - each folder is a
  // > row, but this should be a real tree, like file tree"*
  //
  // Every assertion above is about the `compare` story, whose four files sit in two directories
  // that are each a single chain from the root. That story renders identically whether the pane
  // builds a real tree or merely buckets files by their whole directory path, which is why it
  // stayed green for two milestones while the list drew one flat row per directory. The `nested`
  // story exists to be the fixture that can tell them apart — three directories under one shared
  // parent, one of them a chain worth compacting below a heading.

  eq(
    byName.nested.fileDirs,
    ['ui/src', 'panes', 'sidebar/GitPanel', 'store', 'crates/cide-git/src'],
    'the shared parent `ui/src` gets a heading of its OWN, and its three children are named by '
      + 'their own segment under it \u2014 not `ui/src/panes`, `ui/src/store`, `ui/src/sidebar/'
      + 'GitPanel` as three unrelated top-level rows, which is what a bucket-per-directory '
      + 'grouper draws and what was on screen',
  )
  eq(
    byName.nested.dirIndents,
    ['0', '19px', '19px', '19px', '0'],
    '\u2026and the three children are indented one level under it while both roots sit at zero. '
      + 'This is the whole change stated as a number: a heading is a row IN the tree now rather '
      + 'than a bucket label, and the labels and their order are the same strings in the same '
      + 'sequence either way \u2014 only the margins say which picture is on screen',
  )
  eq(
    byName.nested.rowIndents,
    ['38px', '38px', '38px', '38px', '19px', '0'],
    'a file sits one level below its own heading, so a file in `ui/src/panes` is two levels in, '
      + 'one in `crates/cide-git/src` is one, and `README.md` at the repository root is none. '
      + 'Four files at the same depth as the one in the compacted root is the flat list again',
  )
  eq(
    byName.nested.groupedLabels,
    ['GitDiffPane.tsx', 'mergeModel.ts', 'model.ts', 'workspace.ts', 'stage.rs', 'README.md'],
    'a row is still its basename \u2014 the heading chain above it says the rest \u2014 and the '
      + 'root-level `README.md` is LAST rather than between two headings: a level draws its '
      + 'directories before its own files, which is what both other trees in this app do',
  )
  eq(
    byName.nested.groupedLabels.length,
    byName.nested.flatLabels.length,
    '\u2026and nesting regroups without hiding: the same six files either way round',
  )
  eq(
    byName.nested.flatFileDirs,
    [],
    'the flat reading of the same commit draws no headings at all, which is still the whole '
      + 'difference between the two arrangements',
  )
  eq(
    byName.nested.dirIcons,
    5,
    'one folder icon per heading, the parent included \u2014 five headings for six files, which '
      + 'a flat grouper could not produce from this commit at all (it has four directories)',
  )

  // Folded, and the fold lands on a NESTED directory: `logSmoke` folds the first file's own
  // directory, which here is `ui/src/panes` rather than a top-level row.

  eq(
    byName.nested.foldedExpanded,
    ['true', 'false', 'true', 'true', 'true'],
    'folding a nested directory shuts that row and leaves its PARENT open \u2014 a fold that '
      + 'closed `ui/src` with it, or that could not be aimed at a child at all, is the flat list '
      + 'showing through',
  )
  eq(
    byName.nested.foldedLabels,
    ['model.ts', 'workspace.ts', 'stage.rs', 'README.md'],
    '\u2026and it takes only its own two files, leaving its siblings\u2019 files on screen',
  )
  eq(
    byName.nested.foldedDirs,
    byName.nested.fileDirs,
    '\u2026and every heading is still drawn, the folded one included: a heading that vanished '
      + 'with its contents would leave no way to bring them back',
  )

  eq(
    byName.compare.rangeTruncated,
    null,
    'an uncapped range says nothing about a cap',
  )
  // Mutual exclusion, re-anchored (M21). It used to be `detailFiles === 0`, which worked only
  // because the smoke entry drew the single-commit pane as `<div>`s of its own. That pane now
  // renders the real `ChangedFileList`, so both panes emit the same `logFile` buttons and the
  // count can no longer tell them apart. The header can: `RangeDetails` is the only thing that
  // draws one, and the claim was always about which pane is showing rather than about a tag.
  //
  // The state is genuinely reachable and that is why it is asserted: the story holds a
  // `CommitDetail` too, because the anchor's detail is fetched on every gesture so that
  // deselecting one row falls straight back to it.
  ok(
    byName.compare.rangeHeader !== null && byName.detail.rangeHeader === null,
    'the compare story draws the range header and the single-commit story does not \u2014 one '
      + 'pane or the other, never both',
  )

  // --- the single-commit pane is untouched ------------------------------------------------------
  //
  // The compare pane reuses `LogView`'s changed-file row, and the row grew an optional counts
  // span to do it. `counts: null` has to render byte-identically to what was there before, or
  // every commit in the log quietly gained a column.

  eq(
    byName.compare.rows,
    byName.mock.rows,
    'the compare story is the mock history with two rows selected \u2014 the list itself is '
      + 'unchanged, so any difference below is the details pane and not the fixture',
  )
  eq(
    byName.detail.detailFiles,
    3,
    "the one-commit pane draws the commit's three files through the shared list",
  )
  eq(
    byName.detail.rangeCounts,
    [],
    '\u2026and no per-file counts at all. `CommitDetail.total.partial` means a large merge has '
      + 'none to print, and a column of `+0 \u22120` would be a number the reader could believe',
  )

  // --- revealing one commit --------------------------------------------------------------------
  //
  // A click on a blame line asks the log to show one commit, and the commit is *usually* older
  // than the loaded page — that is the common case, not the edge. Both outcomes are stories
  // because both of them are ways the gesture can look like it did nothing.

  eq(
    byName.revealed.selectedOid,
    'e5f6a71',
    'a reveal whose commit is loaded marks that row `aria-selected` — and the fifth row, not the '
      + 'first, so a view that revealed whatever happened to be at the top could not pass',
  )
  eq(
    byName.revealed.revealNote,
    null,
    '…and says nothing beside it. The selected row IS the answer, and a banner repeating it would '
      + 'be noise on the one path that worked',
  )
  eq(byName.revealed.revealFind, false, '…so there is nothing to press either')
  eq(
    byName.mock.selectedOid,
    null,
    'while an untouched list marks no row at all — `aria-selected="true"` on a row nobody chose '
      + 'is how a reveal appears to have happened when none did',
  )
  eq(
    byName.detail.selectedOid,
    'a1b2c3d',
    'and an ordinary click marks its own row, which is the same attribute doing the same job — a '
      + 'reveal adds no second notion of "selected"',
  )

  eq(
    byName.revealOutside.revealNote,
    'Commit e5f6a71 is outside the commits this list is showing.',
    'a commit older than the page — or outside the filter — says so, verbatim. Without this the '
      + 'click opens a log that visibly does not contain the commit it was about, which is the '
      + '"did anything happen?" state this codebase keeps re-fixing',
  )
  eq(
    byName.revealOutside.selectedOid,
    null,
    '…and marks no row, because the commit really is not in the list. A highlighted row here '
      + 'would be the wrong commit wearing the right one’s selection',
  )
  eq(
    byName.revealOutside.detailFiles,
    3,
    '…but its detail is in the pane all the same, fetched directly with `git_commit_detail`. The '
      + 'user can read the commit they clicked without the list containing it, which is the whole '
      + 'point of handling this case rather than reporting it',
  )
  eq(
    byName.revealOutside.revealFind,
    true,
    '…with the way out beside the sentence. `logModel::findCommitFilter` re-roots the walk at the '
      + 'commit, which is what actually finds it — clearing the filters alone would land back on '
      + 'this same note, because the commit is below page one and not hidden by a box',
  )
  eq(
    byName.revealOutside.status,
    'No commit matches these filters.',
    'and the list beside it still says its own thing. Two sentences about two different questions '
      + '— "this filter matched nothing" and "the commit you asked for is elsewhere" — and '
      + 'collapsing them would answer neither',
  )
  ok(
    byName.revealOutside.status !== byName.revealOutside.revealNote,
    '…which are, therefore, not the same sentence',
  )
  eq(
    byName.filteredEmpty.revealNote,
    null,
    'while the same empty filtered list with no reveal outstanding shows no note — the block is '
      + 'the answer to a gesture, not decoration on an empty state',
  )

  if (failed === 0) console.log(`log render: ok (${checked} assertions)`)
  else console.error(`\n${failed} failure(s)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}

process.exit(failed > 0 ? 1 : 0)
