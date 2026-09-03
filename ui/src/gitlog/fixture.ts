/**
 * Fixed states of the commit log, reachable without arranging a repository to match. (M19)
 *
 * # Why this file exists
 *
 * Most of what the log has to get right is invisible in the repository a developer happens to
 * have open. This one is a single root with a linear-ish history and no shallow floor, so the
 * merged strip, the budget stop, the `noSuchRef` a monorepo produces, the forty-tag release
 * commit and the graph-is-off sentence are all states nobody would see while building the panel
 * — which is to say they are the states whose first render should not be the first time anyone
 * has looked at them.
 *
 * # The shape is the wire's, and that is the whole discipline
 *
 * Every story here is a real `cide_ipc::history::CommitPage`, built out of the **generated**
 * `CommitRow`, `GraphRow`, `RefChip`, `RepoPage`, `LogResume` and `CommitDetail` types, so it
 * typechecks against the same declarations `git_log` serialises into. `sidebar/GitPanel/fixture.ts`
 * is the precedent and it carries the lesson: its stories used to be written in the panel's own
 * invented shape, so every one of them rendered beautifully while the panel was empty against
 * every real repository. A fixture that does not travel the same road as the data proves nothing,
 * and `check-log-render.mjs`'s `mock.rows > 0` is the assertion that catches the day it stops.
 *
 * Two traps this file exists partly to pin down, both of which produce a silent `undefined` or a
 * `NaN` rather than an error:
 *
 * * **`authored` and `committed` are `bigint`.** ts-rs renders an `i64` as one. `new Date(bigint)`
 *   throws and `Number(undefined)` is `NaN`, which renders as "Invalid Date" with nothing in the
 *   console — so the stories write `n` suffixes and `logModel::when` does the narrowing.
 * * **`oid` is the full forty hex digits and `shortOid` is a prefix of it.** Every follow-up call
 *   passes the full one back, and `matchesText` matches a typed hash as a prefix of the full oid
 *   rather than a substring of the short one. A fixture with a seven-character `oid` would make
 *   that rule untestable and would look right.
 *
 * # There is no `?log-story=` query hook
 *
 * `sidebar/GitPanel/fixture.ts` has one because its panel owns its own data and the story has to
 * be injected under it. `LogView` is pure — every value is a prop — so the smoke entry hands a
 * story straight in, and a query hook would be a second way to reach these states that nothing
 * uses and nothing checks.
 */
import type {
  CommitDetail,
  CommitFile,
  CommitPage,
  CommitRow,
  CommitTotals,
  GraphRow,
  LogResume,
  RefChip,
  RepoInfo,
  RepoPage,
  RevisionChange,
  RevisionRange,
} from '@/ipc/generated'
import {
  comparePair,
  graphOffReason,
  isFiltered,
  logStatus,
  NO_FILTER,
  receivedFilter,
  NO_REVEAL,
  type ComparePair,
  type LogFilter,
  type LogReveal,
} from './logModel'
import { LOG_SPLIT_DEFAULT } from '@/toolwindow/logSplit'
import type { LogViewProps } from './LogView'

export type LogStoryName =
  | 'mock'
  | 'filtered'
  | 'filteredEmpty'
  | 'empty'
  | 'history'
  | 'multi'
  | 'received'
  | 'failed'
  | 'loading'
  | 'detail'
  | 'budget'
  | 'graphOff'
  | 'revealed'
  | 'revealOutside'
  | 'compare'
  | 'nested'

// --- the repositories -------------------------------------------------------------------------
//
// Uuids in the shape `cide_git::repo::discover` mints them (derived from the canonical work-tree
// path, so stable across restarts), because `repoChipFor` looks a row's `repo` up by identity and
// a fixture using short strings would not catch an id that stopped round-tripping.

const CIDE_ID = '6b1f0c20-0000-4000-8000-000000000001'
const VENDOR_ID = '6b1f0c20-0000-4000-8000-000000000002'
const DOCS_ID = '6b1f0c20-0000-4000-8000-000000000003'

const CIDE: RepoInfo = {
  id: CIDE_ID,
  root: '/home/dev/work/cide',
  name: 'cide',
  parent: null,
  isSubmodule: false,
}

const VENDOR: RepoInfo = {
  id: VENDOR_ID,
  root: '/home/dev/work/cide/vendor/hub-core',
  name: 'hub-core',
  parent: null,
  isSubmodule: false,
}

/**
 * A third root that does **not** have the branch the `multi` story filters by.
 *
 * It contributes no rows and exists only to be the repository `missingRefNote` names. That is the
 * monorepo case `cide_git::log` deliberately does not refuse the whole query for — refusing would
 * make the branch filter useless in exactly the workspace it exists for — which puts the whole
 * burden of saying so on the panel, and leaves it untested unless a story arranges it.
 */
const DOCS: RepoInfo = {
  id: DOCS_ID,
  root: '/home/dev/work/cide/docs-site',
  name: 'docs-site',
  parent: null,
  isSubmodule: false,
}

/**
 * The instant every story is rendered "now".
 *
 * Fixed, so a digest is the same in March as in December: `logModel::when` omits the year for a
 * commit from the current year and prints it otherwise, and a `Date.now()` here would make that
 * boundary — the thing the check is *for* — move under the assertion once a year.
 */
export const STORY_NOW = Date.UTC(2026, 2, 14, 12, 0, 0)

/** Unix seconds, from a UTC date, as the wire spells them. */
function at(y: number, m: number, d: number, h = 12): bigint {
  return BigInt(Math.floor(Date.UTC(y, m - 1, d, h) / 1000))
}

// --- the rows ---------------------------------------------------------------------------------

/** A full forty-hex oid from a seven-character stem, so `shortOid` really is a prefix of `oid`. */
function full(stem: string): string {
  return (stem + '0'.repeat(40)).slice(0, 40)
}

function chip(kind: RefChip['kind'], name: string, refFull: string, current = false): RefChip {
  return { kind, name, full: refFull, current }
}

function row(
  stem: string,
  summary: string,
  author: string,
  authored: bigint,
  over: Partial<CommitRow> = {},
): CommitRow {
  return {
    repo: CIDE_ID,
    oid: full(stem),
    shortOid: stem,
    summary,
    author,
    authorEmail: `${author.split(' ')[0]?.toLowerCase() ?? 'dev'}@example.com`,
    authored,
    // Equal to `authored` unless a story is about the two differing. A rebase leaves them apart
    // and the list sorts by the committed one; every row here that is not about that says so by
    // making them the same value rather than by two arbitrary numbers a reader has to compare.
    committed: authored,
    parents: [],
    pruned: 0,
    refs: [],
    ...over,
  }
}

/*
 * The history the `mock`, `budget` and `graphOff` stories share.
 *
 * Six commits with one merge in them, because a linear list renders a graph nobody can tell from
 * a blank column. Read top to bottom, newest first, as the panel draws it:
 *
 *     ●  a1b2c3d  tip of main, HEAD, origin/main
 *     ●  b2c3d4e  merge of feature/login          two parents
 *     │╲
 *     │ ●  c3d4e5f  the merged branch's own commit  lane 1
 *     ●╱  d4e5f60  where the branch left            both lanes close here
 *     ●  e5f6a71  tagged v0.18.0 and thirty-nine others — the chip cap's case
 *     ●  f60a7b8  the root commit                    no parents
 */
const MAIN: CommitRow[] = [
  row('a1b2c3d', 'The awaiting chip’s geometry is a gate, not an eyeball', 'Ivan Vorontsov', at(2026, 3, 12), {
    parents: [full('b2c3d4e')],
    refs: [
      chip('head', 'HEAD', 'HEAD', true),
      chip('localBranch', 'main', 'refs/heads/main', true),
      chip('remoteBranch', 'origin/main', 'refs/remotes/origin/main'),
    ],
  }),
  row('b2c3d4e', 'Merge branch ’feature/login’', 'Ivan Vorontsov', at(2026, 3, 11), {
    parents: [full('d4e5f60'), full('c3d4e5f')],
  }),
  row('c3d4e5f', 'Go to definition asks the other question from inside an interface', 'Mira Kovács', at(2026, 3, 10), {
    parents: [full('d4e5f60')],
  }),
  row('d4e5f60', 'Ignored files are shown by default, including on disks that already said no', 'Ivan Vorontsov', at(2026, 2, 28), {
    parents: [full('e5f6a71')],
  }),
  /*
   * The release commit, and the reason `MAX_CHIPS` exists.
   *
   * Forty tags is not a hypothetical: a monorepo that tags every crate on release produces
   * exactly this, and forty chips push the subject — the only column anyone scans — off the row.
   */
  row('e5f6a71', 'A pane that reattaches gets the buffer it left', 'Ivan Vorontsov', at(2019, 11, 2), {
    parents: [full('f60a7b8')],
    refs: [
      chip('localBranch', 'release/0.18', 'refs/heads/release/0.18'),
      ...Array.from({ length: 40 }, (_, i) =>
        chip('tag', `v0.18.${i}`, `refs/tags/v0.18.${i}`),
      ),
    ],
  }),
  row('f60a7b8', 'Initial commit', 'Ivan Vorontsov', at(2019, 1, 4)),
]

/**
 * One row of `MAIN` by index.
 *
 * A function and not `MAIN[i]`, because `noUncheckedIndexedAccess` types that as
 * `CommitRow | undefined` and the alternatives are a `!` at every use — which is exactly the
 * assertion that later turns a renumbered fixture into an `undefined` row — or a `??` fallback
 * that would render a second, wrong commit and look fine.
 */
function pick(index: number): CommitRow {
  const found = MAIN[index]
  if (found === undefined) throw new Error(`no fixture row at ${index}`)
  return found
}

/*
 * The gutter for `MAIN`, index for index.
 *
 * Written by hand rather than derived, because deriving it here would be a second implementation
 * of `cide_git::lanes` and the fixture would then agree with itself rather than with the backend.
 * `Enter` is the top half of a segment arriving at this row's dot, `Exit` the bottom half leaving
 * toward a parent below, `Pass` a line crossing the row untouched — and a merge is *two* exits,
 * which is how `LogView` gets to draw one without being told it is a merge.
 */
const MAIN_GRAPH: GraphRow[] = [
  { lane: 0, color: 0, edges: [{ kind: 'exit', bottom: 0, color: 0, dangling: false }], overflow: false },
  {
    lane: 0,
    color: 0,
    edges: [
      { kind: 'enter', top: 0, color: 0 },
      { kind: 'exit', bottom: 0, color: 0, dangling: false },
      { kind: 'exit', bottom: 1, color: 1, dangling: false },
    ],
    overflow: false,
  },
  {
    lane: 1,
    color: 1,
    edges: [
      { kind: 'pass', lane: 0, color: 0 },
      { kind: 'enter', top: 1, color: 1 },
      { kind: 'exit', bottom: 1, color: 1, dangling: false },
    ],
    overflow: false,
  },
  {
    lane: 0,
    color: 0,
    edges: [
      { kind: 'enter', top: 0, color: 0 },
      { kind: 'enter', top: 1, color: 1 },
      { kind: 'exit', bottom: 0, color: 0, dangling: false },
    ],
    overflow: false,
  },
  {
    lane: 0,
    color: 0,
    edges: [
      { kind: 'enter', top: 0, color: 0 },
      { kind: 'exit', bottom: 0, color: 0, dangling: false },
    ],
    overflow: false,
  },
  // The root: an `Enter` and no `Exit`. Inferred rather than signalled, exactly as a merge is.
  { lane: 0, color: 0, edges: [{ kind: 'enter', top: 0, color: 0 }], overflow: false },
]

// --- pages --------------------------------------------------------------------------------------

function repoPage(info: RepoInfo, stop: RepoPage['stop'], scanned: number): RepoPage {
  return { repo: info.id, name: info.name, stop, scanned }
}

/**
 * A continuation token, in the real shape.
 *
 * Written out rather than stubbed with `{} as LogResume`, because `resume !== null` is the *only*
 * honest reading of "there is more" — `CommitPage.stop` cannot be used for it — so a story about
 * the Load-more button has to carry one that would deserialise.
 */
function resume(watermark: bigint, scanned: number): LogResume {
  return {
    version: 1,
    key: 'q1-f3a9c2b17d4e5f60',
    repos: [
      {
        repo: CIDE_ID,
        frontier: [{ oid: full('f60a7b8'), key: watermark, paths: [] }],
        hidden: [],
        watermark,
        recent: [full('f60a7b8')],
        scanned,
        done: false,
      },
    ],
    lanes: { lanes: [{ column: 0, awaits: full('f60a7b8'), color: 0 }], nextColor: 2 },
  }
}

function page(over: Partial<CommitPage> = {}): CommitPage {
  return {
    commits: MAIN,
    graph: { kind: 'rows', rows: MAIN_GRAPH, lanes: 2, overflow: false },
    resume: null,
    repos: [repoPage(CIDE, 'exhausted', MAIN.length)],
    stop: 'exhausted',
    scanned: MAIN.length,
    cancelled: false,
    renames: [],
    followed: false,
    followCapped: false,
    ...over,
  }
}

// --- the detail pane ------------------------------------------------------------------------

function file(path: string, added: number, deleted: number, over: Partial<CommitFile> = {}): CommitFile {
  return {
    path,
    oldPath: null,
    status: 'modified',
    binary: false,
    lines: { kind: 'counted', added, deleted },
    ...over,
  }
}

const TOTAL: CommitTotals = { files: 3, added: 64, deleted: 12, partial: false }

const DETAIL: CommitDetail = {
  commit: pick(0),
  message:
    'The awaiting chip’s geometry is a gate, not an eyeball\n\n' +
    'The chip measured the terminal’s last row and guessed. It now reads the gate the\n' +
    'session already publishes, so a resize cannot move the answer.\n',
  committer: 'Ivan Vorontsov',
  committerEmail: 'ivan@example.com',
  against: { kind: 'parent', index: 0, oid: full('b2c3d4e') },
  files: [
    file('ui/src/panes/AwaitingChip.tsx', 41, 9),
    file('ui/src/panes/awaiting.ts', 23, 3),
    file('ui/src/panes/awaitingChip.ts', 0, 0, {
      oldPath: 'ui/src/panes/chipGeometry.ts',
      status: 'renamed',
    }),
  ],
  filesTruncated: false,
  merge: false,
  total: TOTAL,
}

/**
 * A commit that touches several directories under one shared parent. (M27)
 *
 * `DETAIL` above cannot exhibit a tree: all three of its files are in one directory, and the
 * compare story's four are in two directories that are each a single chain from the root. Both
 * render identically whether the pane builds a real tree or merely buckets files by their whole
 * directory path — which is why the list spent two milestones drawing one row per directory with
 * nothing nested inside anything, and every render assertion stayed green.
 *
 * So this one is shaped to tell them apart, and every part of it is load-bearing:
 *
 * - `panes`, `sidebar/GitPanel` and `store` share the parent `ui/src`, so a bucketing grouper
 *   draws three top-level rows spelling `ui/src/` three times and a tree draws one heading with
 *   three under it.
 * - `sidebar/GitPanel` is a chain worth compacting *below* a heading, which is the case that
 *   distinguishes "compact single-child chains" from "compact the root".
 * - `crates/cide-git/src` is a chain compacted *at* the root, so the two cases sit side by side.
 * - `README.md` is at the repository root, which draws no heading at all and must survive the
 *   directories-before-files rule by landing last rather than by vanishing.
 */
const NESTED_TOTAL: CommitTotals = { files: 6, added: 132, deleted: 47, partial: false }

const NESTED: CommitDetail = {
  commit: pick(0),
  message: 'Changed files are a tree, not a list of directories\n',
  committer: 'Ivan Vorontsov',
  committerEmail: 'ivan@example.com',
  against: { kind: 'parent', index: 0, oid: full('b2c3d4e') },
  files: [
    file('ui/src/panes/GitDiffPane.tsx', 38, 11),
    file('ui/src/panes/mergeModel.ts', 12, 4),
    file('ui/src/sidebar/GitPanel/model.ts', 27, 9),
    file('ui/src/store/workspace.ts', 6, 2),
    file('crates/cide-git/src/stage.rs', 44, 21),
    file('README.md', 5, 0),
  ],
  filesTruncated: false,
  merge: false,
  total: NESTED_TOTAL,
}

// --- the range between two commits (M20) -------------------------------------------------------

/**
 * One row of a `RevisionRange`, which is **not** a `CommitFile`.
 *
 * Deliberately built from the other generated type rather than reusing `file()` above, because
 * the two shapes differ in exactly the place a shared helper would paper over: a `CommitFile`
 * carries `lines: {kind: 'counted', added, deleted}` — a tagged union, because a large merge has
 * no counts at all — and a `RevisionChange` carries bare `additions`/`deletions` numbers. A
 * fixture that reached the range pane in `CommitFile` shape would render `+undefined −undefined`
 * and typecheck, which is the class of failure `sidebar/GitPanel/fixture.ts` was rewritten for.
 */
function change(
  path: string,
  additions: number,
  deletions: number,
  over: Partial<RevisionChange> = {},
): RevisionChange {
  return { path, oldPath: null, status: 'modified', binary: false, additions, deletions, ...over }
}

/**
 * Four commits apart: `d4e5f60` (older) to `a1b2c3d` (newer).
 *
 * The two endpoints are rows **0 and 3** of `MAIN` — not adjacent, and clicked in neither order
 * by the story, because the whole point of `logModel::comparePair` is that the page's order and
 * not the click sequence decides which is which. A rename is in the list for the same reason the
 * commit detail has one: `RevisionChange.oldPath` is what makes a renamed file diff against the
 * right blob, and dropping it renders the file as a whole-file add.
 */
const RANGE: RevisionRange = {
  new: { kind: 'commit', oid: full('a1b2c3d') },
  old: { kind: 'commit', oid: full('d4e5f60') },
  newOid: full('a1b2c3d'),
  oldOid: full('d4e5f60'),
  newSummary: 'The awaiting chip\u2019s geometry is a gate, not an eyeball',
  oldSummary: 'Ignored files are shown by default, including on disks that already said no',
  files: [
    change('ui/src/panes/AwaitingChip.tsx', 41, 9),
    change('ui/src/panes/awaiting.ts', 23, 3),
    change('crates/cide-fs/src/index.rs', 7, 112),
    change('ui/src/panes/awaitingChip.ts', 0, 0, {
      oldPath: 'ui/src/panes/chipGeometry.ts',
      status: 'renamed',
    }),
  ],
  truncated: false,
}

// --- the stories --------------------------------------------------------------------------------

/** Everything one story hands the pure view, before the callbacks are attached. */
export interface LogStory {
  readonly name: LogStoryName
  readonly page: CommitPage | null
  readonly filter: LogFilter
  readonly repos: readonly RepoInfo[]
  readonly branches: readonly string[]
  readonly detail: CommitDetail | null
  readonly selected: string | null
  /**
   * The second selected row — the other end of a comparison — or `null`. (M20)
   *
   * A field of the story and not something derived from `range`, because the two are the halves
   * that can disagree: the *list* highlights two rows and the *pane* shows a range, and a story
   * that derived one from the other could not exhibit the state where only one of them happened.
   */
  readonly secondary: string | null
  /** What `git_diff_revision_files` answered, or `null` for a one-commit story. */
  readonly range: RevisionRange | null
  readonly busy: boolean
  readonly failed: string | null
  readonly path: string | null
  /** What became of a "show me this commit" request — see `logModel::LogReveal`. */
  readonly reveal: LogReveal
}

const BRANCHES = ['main', 'feature/login', 'release/0.18', 'origin/main']

/** `FetchOutcome::received` for the `received` story: two full oids, as Rust spells it. */
const RECEIVED_RANGE = `${'1'.repeat(40)}..${'2'.repeat(40)}`

function story(name: LogStoryName, over: Partial<LogStory> = {}): LogStory {
  return {
    name,
    page: page(),
    filter: NO_FILTER,
    repos: [CIDE],
    branches: BRANCHES,
    detail: null,
    selected: null,
    secondary: null,
    range: null,
    busy: false,
    failed: null,
    path: null,
    reveal: NO_REVEAL,
    ...over,
  }
}

/*
 * The needle the `filtered` story narrows by.
 *
 * It hits exactly one of the six rows — "A pane that reattaches" — which makes
 * `filtered.rows < mock.rows` a *narrowing* and not the degenerate case where the story is empty
 * and the inequality holds for the wrong reason. `empty` is the story about an empty list, and
 * the two must not be able to pass each other's assertions.
 */
const FILTERED_TEXT = 'pane'

/*
 * A page from a project with two roots.
 *
 * The graph is **off**, and that is not the fixture being lazy: `cide_git::log::graph_off` returns
 * `Merged` for any merged scope, because two repositories share no DAG and a line crossing
 * hundreds of foreign rows is worse than no line. The repo chip carries the structure instead,
 * which is the pair of facts `repoStripOn` decides with one count.
 */
const MERGED_ROWS: CommitRow[] = [
  pick(0),
  row('9a8b7c6', 'Bump the vendored hub-core to 4.2', 'Mira Kovács', at(2026, 3, 11, 9), {
    repo: VENDOR_ID,
    parents: [full('8b7c6d5')],
    refs: [chip('localBranch', 'main', 'refs/heads/main', true)],
  }),
  pick(3),
  row('8b7c6d5', 'Drop the last of the C shims', 'Mira Kovács', at(2026, 2, 20), {
    repo: VENDOR_ID,
  }),
]

export const LOG_STORIES: readonly LogStory[] = [
  story('mock'),

  // The wire has answered for this needle: `logStatus` sees a filter, the graph is off with
  // `Filtered`, and the rows are the two the backend kept.
  story('filtered', {
    filter: { ...NO_FILTER, text: FILTERED_TEXT },
    page: page({
      commits: MAIN.filter((r) => r.summary.toLowerCase().includes(FILTERED_TEXT)),
      graph: { kind: 'off', reason: 'filtered' },
      scanned: MAIN.length,
      repos: [repoPage(CIDE, 'exhausted', MAIN.length)],
    }),
  }),

  /*
   * The same filter, narrowed to nothing.
   *
   * A story of its own and not a variant of `empty`, because the two are the states most easily
   * confused and the whole point of the sentence table is that they are not the same: one is a
   * repository with no commits and the other is a question with no answers. This is also the only
   * story that renders the *inline* Clear control — a user staring at "no commit matches" is
   * looking at the sentence and not at the bar three rows above it, and making them find the
   * control that caused it is the difference between "no results" and "this thing is broken".
   */
  story('filteredEmpty', {
    filter: { ...NO_FILTER, author: 'nobody', text: 'zzzz' },
    page: page({
      commits: [],
      graph: { kind: 'off', reason: 'filtered' },
      scanned: 240,
      stop: 'exhausted',
      repos: [repoPage(CIDE, 'exhausted', 240)],
    }),
  }),

  // An unborn HEAD: `cide_git::log` reports it as an empty page rather than an error, because a
  // freshly created project is a normal state and a dialog on opening one would be absurd.
  story('empty', {
    page: page({
      commits: [],
      graph: { kind: 'rows', rows: [], lanes: 0, overflow: false },
      scanned: 0,
      repos: [repoPage(CIDE, 'exhausted', 0)],
    }),
  }),

  // A History tab: one path, follow on, and a rename hop crossed inside the page.
  story('history', {
    path: 'ui/src/panes/awaitingChip.ts',
    page: page({
      commits: [pick(0), pick(4)],
      /*
       * One lane, and the older row's parent edge **dangles**: under `Simplify::Default` a path
       * filter rewrites a row's parents onto the first surviving ancestor, and here that ancestor
       * is below the page. The line therefore fades out rather than pointing at a node that never
       * arrives — the difference between a graph that admits its edge and one with a line to
       * nowhere.
       */
      graph: {
        kind: 'rows',
        rows: [
          { lane: 0, color: 0, edges: [{ kind: 'exit', bottom: 0, color: 0, dangling: false }], overflow: false },
          {
            lane: 0,
            color: 0,
            edges: [
              { kind: 'enter', top: 0, color: 0 },
              { kind: 'exit', bottom: 0, color: 0, dangling: true },
            ],
            overflow: false,
          },
        ],
        lanes: 1,
        overflow: false,
      },
      followed: true,
      renames: [
        {
          repo: CIDE_ID,
          oid: full('a1b2c3d'),
          from: 'ui/src/panes/chipGeometry.ts',
          to: 'ui/src/panes/awaitingChip.ts',
          similarity: 88,
        },
      ],
      scanned: 240,
    }),
  }),

  story('multi', {
    repos: [CIDE, VENDOR, DOCS],
    page: page({
      commits: MERGED_ROWS,
      graph: { kind: 'off', reason: 'merged' },
      // Two roots answered and one has no such branch. The page-level stop is the *weakest* of
      // the three, so `NoSuchRef` wins over `Exhausted` — and because there are still rows,
      // `logStatus` returns `null` and `missingRefNote` is the only thing that can report it.
      repos: [
        repoPage(CIDE, 'exhausted', 6),
        repoPage(VENDOR, 'exhausted', 4),
        repoPage(DOCS, 'noSuchRef', 0),
      ],
      stop: 'noSuchRef',
      scanned: 10,
    }),
    filter: { ...NO_FILTER, branch: { kind: 'branch', name: 'release/0.18' } },
  }),

  /*
   * *View commits* on a pull's notice, in a project with several roots. (M37)
   *
   * The filter carries the repository the range was received into, and the branch box has to say
   * so — in a project with three roots the chips on the rows would otherwise be the only thing
   * that did. The rows are the mock's; what this story exists to render is the bar.
   */
  story('received', {
    repos: [CIDE, VENDOR, DOCS],
    filter: receivedFilter(RECEIVED_RANGE, VENDOR_ID),
  }),

  // A `GitError` that reached the panel as a sentence. The string is what
  // `chrome/branchModel.ts::explain` produces from `{kind: 'io', detail: {detail: '…'}}` — the
  // shape that prints `[object Object]` when nobody unpacks it. `LogTab` had a cut-down copy of
  // that unpacking until the commit actions arrived and needed all fifteen of the new arms.
  story('failed', {
    page: null,
    failed: 'could not read /home/dev/work/cide/.git/HEAD: Permission denied',
  }),

  story('loading', { page: null, busy: true }),

  story('detail', { selected: full('a1b2c3d'), detail: DETAIL }),

  // The same pane over a commit that actually nests — see `NESTED` for why `detail` cannot make
  // the claim and this one can.
  story('nested', { selected: full('a1b2c3d'), detail: NESTED }),

  /*
   * The budget stop, which is the whole reason `LogStop` has six variants.
   *
   * Twenty thousand commits examined, four rows produced, and a live frontier — so the foot of
   * the list must offer *keep looking*, not "no more commits". A list that drew this as the end
   * silently loses the answer the user was searching for.
   */
  story('budget', {
    // A **path** and not an author, deliberately: `graph_off` returns `Filtered` for an author or
    // text filter, and a story that carried both a filter and a lane structure would be a page
    // the backend cannot produce. A path under `Simplify::Default` keeps its graph — the parents
    // are rewritten onto surviving ancestors — which is why `git log --graph -- path` works.
    path: 'crates/cide-git/src/lanes.rs',
    page: page({
      commits: MAIN.slice(0, 4),
      graph: {
        kind: 'rows',
        rows: [
          ...MAIN_GRAPH.slice(0, 3),
          // The last row of a truncated page: its parent is not in the page, so the segment
          // fades out rather than pointing at a node that never arrives.
          { lane: 0, color: 0, edges: [{ kind: 'enter', top: 0, color: 0 }, { kind: 'exit', bottom: 0, color: 0, dangling: true }], overflow: false },
        ],
        lanes: 2,
        overflow: false,
      },
      resume: resume(at(2026, 2, 28), 20000),
      stop: 'budget',
      scanned: 20000,
      repos: [repoPage(CIDE, 'budget', 20000)],
    }),
  }),

  // `--full-history` over a path: the parents are not rewritten, so the row set is not closed
  // under any parent function and there is no graph to draw. The sentence is the deliverable.
  story('graphOff', {
    path: 'crates/cide-git/src/log.rs',
    page: page({ graph: { kind: 'off', reason: 'fullHistory' } }),
  }),

  /*
   * A blame click whose commit **is** in the loaded page: the row is selected and nothing is
   * said.
   *
   * The commit chosen is `e5f6a71`, five rows down and from 2019 — not the tip — because the
   * degenerate way to pass this story is to reveal whatever happens to be first. It is also the
   * row the gutter scrolls to, and `check-log-render.mjs` can see the selection but not the
   * scroll: `react-dom/server` runs no effects, which is exactly why the scroll lives in an
   * effect and the *selection* is what carries the state into the markup.
   */
  story('revealed', {
    selected: full('e5f6a71'),
    reveal: { kind: 'found', oid: full('e5f6a71') },
  }),

  /*
   * The common case, and the one the whole reveal seam exists for: the commit is older than the
   * page, or outside the filter, so it is not in the list at all.
   *
   * Its detail was fetched directly and is in the pane, under a note saying where it is not — and
   * beside the button that re-roots the walk at it. Without this state the click opens a log that
   * visibly does not contain the commit it was about, which is the "did anything happen?" shape
   * this codebase keeps re-fixing.
   *
   * A filter is set as well, because that is how a user gets here without a deep repository and
   * it is what makes the button's label ("Clear filters *and* find it") true rather than
   * aspirational.
   */
  story('revealOutside', {
    filter: { ...NO_FILTER, author: 'mira', text: 'shims' },
    page: page({
      commits: [],
      graph: { kind: 'off', reason: 'filtered' },
      scanned: 240,
      repos: [repoPage(CIDE, 'exhausted', 240)],
    }),
    selected: full('e5f6a71'),
    detail: { ...DETAIL, commit: pick(4) },
    reveal: { kind: 'outside', oid: full('e5f6a71') },
  }),

  /*
   * Two rows selected: the details pane is a range and not a commit. (M20)
   *
   * `selected` is the newer of the two and `secondary` the older, which is what a Ctrl+click on
   * `d4e5f60` after a plain click on `a1b2c3d` leaves behind — but the story is deliberately not
   * a statement about the click order, because `comparePair` decides the orientation from the
   * page. What this fixture *is* a statement about is that both rows are marked selected in the
   * list while the pane shows the range between them: the two halves of one answer, and either of
   * them can go missing on its own.
   *
   * `detail` is set as well, and that is not redundant. The anchor's detail is fetched on every
   * gesture, including the ones that make a pair, so that deselecting one row falls straight back
   * to the commit view instead of flashing "Reading the commit…" — this story is the state that
   * has both in hand, which is the state the real tab is in.
   */
  story('compare', {
    selected: full('a1b2c3d'),
    secondary: full('d4e5f60'),
    detail: DETAIL,
    range: RANGE,
  }),
]

/** One story by name. Throws rather than returning `undefined`: a typo is a bug in the check. */
export function logStory(name: LogStoryName): LogStory {
  const found = LOG_STORIES.find((s) => s.name === name)
  if (found === undefined) throw new Error(`no log story named ${name}`)
  return found
}

/** The props `LogView` takes, minus the four callbacks and the details node. */
export type StoryView = Omit<
  LogViewProps,
  'onSelect' | 'onMore' | 'onFilter' | 'onRefresh' | 'onFindCommit' | 'details'
>

/**
 * A story, turned into props the way `LogTab` turns a response into props.
 *
 * Here and not in the smoke entry, because this *is* the derivation under test: which rows the
 * view is given, whether the gutter travels with them, which sentence the status shows. A smoke
 * entry that rebuilt it inline would be asserting on a copy of `LogTab`'s logic rather than on
 * the logic itself, which is the failure mode `sidebar/GitPanel/fixture.ts` was written to end.
 */
export function storyView(s: LogStory): StoryView {
  const graph = s.page !== null && s.page.graph.kind === 'rows' ? s.page.graph.rows : []
  const graphOff =
    s.page !== null && s.page.graph.kind === 'off' ? graphOffReason(s.page.graph.reason) : null
  const rows = s.page?.commits ?? []
  return {
    rows,
    /*
     * The Log tab's split, so the digest is stable and comparable across stories.
     *
     * A History story would carry `LOG_SPLIT_HISTORY`, but the split arrives as a prop from the
     * workspace rather than from the story, and pinning the *default* here is what makes the
     * render check able to assert that the grid is driven by the number at all — see
     * `check-log-render.mjs`. The two constants are pinned against Rust in `check:toolwindow`.
     */
    split: LOG_SPLIT_DEFAULT,
    // Parallel to the rows or absent — never partially parallel. `LogTab` withdraws the gutter
    // for exactly the same reason while the local narrow is live: a graph one row out of step is
    // worse than no graph, because it is wrong rather than missing.
    graph: graph.length === rows.length ? graph : [],
    graphOff,
    status: logStatus({
      loading: s.busy,
      failed: s.failed,
      noRepo: s.repos.length === 0,
      rows: rows.length,
      filtered: isFiltered(s.filter),
      path: s.path,
      stop: s.page?.stop ?? null,
    }),
    page: s.page,
    selected: s.selected,
    secondary: s.secondary,
    busy: s.busy,
    now: STORY_NOW,
    filter: s.filter,
    branches: s.branches,
    repos: s.repos,
    reveal: s.reveal,
  }
}

/**
 * Which way round a story's two selected commits are, or `null` when only one is.
 *
 * Here rather than in the smoke entry for the same reason `storyView` is: this **is** the
 * derivation under test. `logModel::comparePair` orders the pair by the page's own order, and a
 * smoke entry that laid the two out by hand would be asserting that a fixture agrees with itself
 * — the exact failure `sidebar/GitPanel/fixture.ts` was rewritten to end. `swapped: false`,
 * because the flip is a gesture and no story is currently about one.
 */
export function storyPair(s: LogStory): ComparePair | null {
  return comparePair(
    { primary: s.selected, secondary: s.secondary, swapped: false },
    (s.page?.commits ?? []).map((r) => r.oid),
  )
}
