/**
 * A git diff, with the per-hunk and per-line gestures M10's staging engine was built for.
 *
 * `cide-git` has supported `Selection::Hunks` and `Selection::Lines` since M10 and nothing
 * could produce either: `wholeFiles()` in `useGitPanel` was the only selection the UI could
 * build, so the headline feature was reachable by no gesture at all. This pane is the
 * gesture.
 *
 * # Two components, for the reason `DiffPane` and `ClaudeDiffPane` are two
 *
 * {@link GitDiffView} is pure: a diff, a selection, and callbacks. It talks to nothing, so a
 * fixture can render it under node — which is what `ui/scripts/check-diff-render.mjs` does,
 * and how the claim below is checked against real markup rather than against intent.
 * {@link GitDiffPane} is the wiring: it fetches, holds the selection, and turns a button into
 * a command.
 *
 * # What it is not
 *
 * Not `DiffPane`. That one is a `@codemirror/merge` view over two whole documents, built to
 * *answer* a Claude proposal (accept / accept as proposed / reject), and its unit of
 * interaction is the document. This one is a unified diff whose unit of interaction is the
 * line, because that is what a `Selection` names. Sharing a component between them would mean
 * a merge view that also has line checkboxes, and a `MergeView` chunk is not a
 * `DiffHunkView` — mapping between the two is exactly the kind of index arithmetic that
 * stages the wrong line.
 *
 * # The rule this pane exists to honour
 *
 * **What is highlighted is what is sent.** The painted set comes from
 * `diffSelection.highlightedKeys` and the wire selection from `diffSelection.toSelection`;
 * `check-diff-selection.mjs` proves those two denote the same positions over every subset of
 * a sample diff, and `check-diff-render.mjs` proves the rows this component actually marks
 * are that same set.
 *
 * # Sides are not interchangeable
 *
 * `cide_git` re-derives the diff before applying a selection, and *which* diff depends on the
 * operation: `stage` uses index→worktree, `unstage` uses HEAD→index, `commit` and `shelve`
 * use HEAD→worktree. A set of positions in one of those is a different set in another, so
 * each side offers only the operation it is valid for, and switching sides clears the
 * selection rather than carrying it across.
 */
import {
  Fragment,
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactNode,
  type UIEvent,
} from 'react'
import { collapseRuns, UNCOMMITTED_BUCKET } from '@/editor/blameModel'
import { cancelResizeSettle, whenResizeSettles } from '@/layout/resizeGesture'
import {
  getDiffView,
  getServerDiffView,
  setDiffView,
  subscribeDiffView,
} from '@/editor/diffViewMode'
import {
  countLines,
  hunkMarks,
  hunkState,
  highlightedKeys,
  mark,
  pathSelection,
  selectEverything,
  toSelection,
  toggleHunk,
  toggleLine,
  type Marks,
} from '@/sidebar/GitPanel/diffSelection'
import {
  clearPartial,
  getServerSnapshot,
  getSnapshot,
  setPartial,
  subscribe,
} from '@/sidebar/GitPanel/partialStore'
import {
  BLAME_DEFAULT,
  diag,
  events,
  git as gitApi,
  gitLog,
  history as historyApi,
  type BlameFile,
  type DiffHunkView,
  type DiffLineView,
  type DiffSide,
  type DiffSpec,
  type DiffView,
  type FileDiff,
  type FileState,
  type LineOrigin,
  type ProjectId,
  type RepoId,
  type RevisionDiff,
  type RevSide,
} from '@/ipc/client'
import { markDiffPaneAvailable } from '@/sidebar/GitPanel/diffHost'
import { noteRepoRoots, repoRoot, touchesFile } from '@/sidebar/GitPanel/repoRoots'
// The one predicate for "git moved", shared with the log so the two surfaces cannot disagree
// about whether an index-only burst counts. See the `onFsChanged` subscription below.
import { gitRefsMoved } from '@/gitlog/logModel'
import { blameFor, blameRefusal, type BlameLookup } from './diffBlame'
import { explain, kindOf } from '@/chrome/branchModel'
import {
  columnRows,
  hunkSegments,
  presentSegments,
  changeAnchors,
  changePositions,
  splitLines,
  wholeFileSegments,
  type ColumnRow,
  type DisplaySegment,
} from './diffRows'
import { connectorShapes } from './diffConnector'
import { insertMarkers, mapScroll, rowSpans, type SyncGeometry } from './diffSync'
import { lineTokens, type DiffTokens } from './diffTokens'
import {
  NO_CURSOR,
  claimChangeNav,
  stepIndex,
  type ChangeNav,
  type ChangeNavSlot,
} from './changeNav'
import { diffTabOnScreen, revisionTabOnScreen, sameSide } from './diffTabs'
import { Icon } from '@/icons/Icon'

import styles from './GitDiffPane.module.css'

/*
 * Announced at import time: the git panel refuses to open a diff tab in a build whose shell
 * never linked this module, because the tab would draw as an empty pane with nothing to say
 * why. See `sidebar/GitPanel/diffHost.ts`.
 */
markDiffPaneAvailable()

/**
 * Above this many rows the hunks open collapsed.
 *
 * There is no virtualisation here on purpose — a unified diff is a list of rows on a shared
 * column grid, and windowing it would break both find-in-page and a text selection dragged
 * across hunks. The cap is what keeps a 40,000-line refactor from laying out 40,000 nodes
 * before the first frame; every hunk still has its header, so nothing is hidden, only folded.
 */
const COLLAPSE_ABOVE = 2_000

/** Which operation a side can perform. The other two would be refused, so they are absent. */
export type DiffOp = 'stage' | 'unstage' | 'commit'

export function opFor(side: DiffSide): DiffOp {
  switch (side) {
    case 'unstaged':
      return 'stage'
    case 'staged':
      return 'unstage'
    case 'combined':
      return 'commit'
  }
}

/**
 * The row's tint class.
 *
 * A switch rather than `styles[line.origin]`: CSS Modules types its default export as an
 * index signature, so a typo would compile and paint `undefined` into the class list.
 */
function rowClass(origin: LineOrigin): string {
  switch (origin) {
    case 'addition':
      return styles.addition ?? ''
    case 'deletion':
      return styles.deletion ?? ''
    case 'context':
      return styles.context ?? ''
  }
}

/**
 * Below this pane width, side-by-side is refused and the pane draws unified instead.
 *
 * A split diff needs two code columns and two gutters. The gutters are fixed
 * (`16px + 5ch + 2ch` per side, about 74px at this font), so at 860px each side has roughly
 * 45 monospace characters — already tight for real code and the point at which wrapping turns
 * every line into three. Below it the pane stops showing a diff and starts showing a column of
 * confetti.
 *
 * The alternative was horizontal scrolling, and it lost for a specific reason rather than a
 * general one: this pane's gesture is *ticking lines*, and the tick boxes live in the gutters.
 * A horizontally scrolled split puts the right side's boxes off-screen, so staging a line
 * would mean scrolling to find its checkbox and scrolling back to read what it says. Unified
 * keeps every box in view and loses only the side-by-side arrangement, which is the thing that
 * did not fit anyway.
 *
 * The fallback is announced, not silent — see `cramped` in {@link GitDiffView}. A toggle that
 * appears not to work is worse than one that says why.
 */
export const DIFF_SPLIT_MIN_PX = 860

/**
 * Breathing room above a change the iterator scrolled to, in pixels.
 *
 * Only reached for a run taller than the viewport, where the run is top-aligned instead of
 * centred. A few rows of the preceding context is what makes it read as a position in a file
 * rather than as a jump cut.
 */
const SCROLL_PAD = 24

/**
 * What one drawn line carries beside its text: the wire position, or none.
 *
 * `at` is the index into `hunk.lines` — the second half of a `hunk:line` mark — and it is
 * `null` on a row that must not carry one: the right column's copy of a context line, or a
 * gap line the whole-file reconstruction synthesized. A context line is one entry in the
 * unified diff and is drawn twice in the split layout, so exactly one of the two carries the
 * position; without that rule the same mark would appear twice in the DOM and "the rows
 * drawn as selected" would no longer be a set of positions. The grouping itself — which
 * lines share a run, which column they land in — is `diffRows.columnRows` now.
 */
export interface SplitCell {
  line: DiffLineView
  at: number | null
}

/**
 * A `BlameFile` off the wire into the [`BlameLookup`] the column reads, or `null` for an answer
 * that must not be drawn. (M18)
 *
 * The gate is `collapseRuns`, and it is borrowed rather than reimplemented on purpose. It is the
 * one function in the app that knows whether a run set is a valid cover of `[1, lines]`, and its
 * header states at length why a broken one must produce **no column at all** rather than a
 * repaired one: a hole paints a column that is silently one line off for everything below it,
 * every row still carries a plausible oid, and there is no artefact, no gap and no error to
 * notice. Calling it here means the diff column and the editor's gutter refuse the same answers.
 *
 * It is also where the age ramp comes from. `collapseRuns` allocates one marker per line and this
 * keeps only one number per *run* out of them, which looks wasteful and is the cheaper mistake:
 * re-deriving the buckets from `AGE_BUCKETS` here would be a second implementation of the ramp,
 * and the failure it would eventually produce — the same line tinted differently in the editor
 * and in a diff of that editor's file — is the kind nobody reports as a bug. The array is
 * short-lived and this runs once per fetched blame, not once per paint.
 *
 * Exported so `gitDiffSmoke.tsx` builds its fixture through the same function the pane uses; a
 * fixture that hand-wrote a `BlameLookup` would render beautifully and prove nothing about what
 * `git_blame` actually answers.
 */
export function blameLookup(file: BlameFile, now: number): BlameLookup | null {
  const markers = collapseRuns(file, now)
  // Empty means either "refused" or "an empty file". Neither has a column, so they need not be
  // told apart — the same conclusion `collapseRuns`' own header reaches.
  if (markers.length === 0) return null
  return {
    lines: file.lines,
    runs: file.runs,
    commits: file.commits,
    // One bucket per run, read off the run's first line. Unreachable `??`: `collapseRuns` only
    // returns markers for a set it has accepted as a cover, so every `start` is in range. The
    // fallback is the uncommitted tint rather than a valid age, so an impossible run can never
    // paint as "this is very old" — the one reading a wrong number here would be believed.
    buckets: file.runs.map((run) => markers[run.start - 1]?.bucket ?? UNCOMMITTED_BUCKET),
  }
}

const SIDES: ReadonlyArray<{ side: DiffSide; label: string; title: string }> = [
  { side: 'unstaged', label: 'Unstaged', title: 'index → working tree. Staging acts on this.' },
  { side: 'staged', label: 'Staged', title: 'HEAD → index. Unstaging acts on this.' },
  { side: 'combined', label: 'All', title: 'HEAD → working tree. What a commit selects from.' },
]

// --- the view ---------------------------------------------------------------------------

/**
 * The half of a diff this view *draws*, as opposed to the half it acts on.
 *
 * Satisfied by both `FileDiff` (the working tree, staging ticks and all) and `RevisionDiff`
 * (two frozen revisions, read-only). Declared structurally rather than by picking one of them,
 * because the alternatives are both worse: fabricating a `FileDiff` from a `RevisionDiff` would
 * mean inventing a `rev` and a `partialOk` for a diff that has neither — a `partialOk: false`
 * that reads as "this file cannot be partially staged" when the truth is "there is no index to
 * stage into" — and widening `FileDiff` on the wire would put two meaningless fields on every
 * revision diff, which is exactly what `RevisionDiff`'s own doc comment refuses.
 */
export interface DrawnDiff {
  readonly path: string
  readonly oldPath: string | null
  readonly status: FileState
  readonly binary: boolean
  readonly hunks: readonly DiffHunkView[]
  /**
   * Both sides whole, for the IDEA-style whole-file rendering, or `null` where the backend
   * withheld them — see `FileDiff::old_text` in `cide-ipc`. The view treats `null` as "draw
   * the hunks alone", which is every diff this pane ever drew before M25.
   */
  readonly oldText: string | null
  readonly newText: string | null
  /** An existing side was over the byte cap; the one case worth a sentence in the pane. */
  readonly textsOmitted: boolean
}

/** What both arms carry. Everything here is about *painting* the diff. */
interface GitDiffViewCommon {
  /** `null` while the fetch is in flight or after it failed; the chrome still renders. */
  diff: DrawnDiff | null
  /** Shown in the header before the first diff arrives. */
  path: string
  collapsed: ReadonlySet<number>
  busy: boolean
  /** One line under the header: a refusal, or why the selection was dropped. */
  note: string | null
  /** Why there is no diff, when there is none. */
  reason: string | null
  /**
   * Unified rows or two columns. The *stored preference*, not necessarily what is drawn — a
   * pane narrower than {@link DIFF_SPLIT_MIN_PX} draws unified whatever this says.
   *
   * Optional, and defaults to `'unified'`, so a fixture that predates the split view renders
   * exactly what it always did. `ui/src/panes/gitDiffSmoke.tsx` is that fixture.
   */
  view?: DiffView | undefined
  /** Absent ⇒ the layout toggle is disabled, with a reason. */
  onView?: ((view: DiffView) => void) | undefined
  /**
   * The blame of the diff's **new side**. Three states, and the third is why this is optional
   * *and* nullable rather than one or the other. (M18)
   *
   * - **absent** — the column is off. Nothing is drawn, no grid track is reserved, and the
   *   toggle reads un-pressed. `exactOptionalPropertyTypes` is what makes this state real
   *   rather than a synonym for `null`.
   * - **`null`** — the column is on and there is no answer yet: the fetch is in flight, or it
   *   failed, or the run set was refused by {@link blameLookup}. The toggle reads pressed and
   *   still nothing is drawn.
   * - **a lookup** — the column is on and has an answer.
   *
   * A third `blameOn` prop was the obvious alternative and lost: two props for one piece of
   * state is two props that can disagree, and the disagreement — pressed with no column, or a
   * column with an un-pressed toggle — is exactly what the reader would have to debug. The cost
   * is that the rows reflow when a blame lands. Reserving the track ahead of the answer would
   * avoid the reflow and would leave a dead 22-character margin for every diff whose blame
   * *cannot* be answered, which is not a rare case here: a diff of a newly added file has no
   * blame at all, and that is the diff most likely to be open.
   *
   * Always the new side. See `diffBlame.blameFor` for why the old one is refused.
   */
  blame?: BlameLookup | null | undefined
  /**
   * Turn the column on or off. Absent ⇒ the toggle is disabled, with the reason in its title.
   *
   * The argument is the state being asked for, not a "toggle" — a `() => void` would make the
   * handler's meaning depend on the view's idea of the current state, and the view's idea comes
   * from a prop the caller set.
   */
  onBlame?: ((on: boolean) => void) | undefined
  /**
   * Both sides tokenized, or `null` for a diff drawn in one colour.
   *
   * Deliberately **not** the three-state shape `blame` has. That prop tells "off" apart from
   * "asked for, no answer yet" because a toggle has to draw the middle state; colour has no
   * toggle, and both of its absent states spell the same markup, so one nullable optional
   * carries it. Optional so every existing call site and `panes/gitDiffSmoke.tsx` compile
   * unchanged — the rule `view` and `expandedGaps` already follow.
   *
   * Plain data, never a grammar: the tokenizer reaches `@codemirror/language`, and this
   * component is SSR-bundled and run under node by `check-diff-render.mjs`. The wiring below
   * does that work behind a dynamic `import()` and hands the answer down as arrays of strings.
   */
  tokens?: DiffTokens | null | undefined
  /**
   * Whether this diff is the one in front.
   *
   * Only the changes iterator reads it: `TabContent` never unmounts an inactive tab, so several
   * diff tabs hold live claims at once and the slot has to know which of them the user is
   * looking at. Defaults to `true` — the log tool window mounts only its active history tab, and
   * the SSR fixture has no tabs at all.
   */
  visible?: boolean | undefined
  /**
   * Which change is current, as an index into the changed runs, or `-1` for none yet.
   *
   * State in the *wiring* rather than here, which is the shape `MergePaneView` already uses, and
   * for two reasons. The fixture can drive it — with `useState` in here the SSR render would
   * always be `-1` and `check:diff-render` could prove nothing about the marker at all. And
   * staleness gets an owner: `GitDiff` keys on `${repo} ${path}` and does not remount on a
   * refetch, so a diff that shrank under an open pane has to reset this where the marks and the
   * opened gaps are already reset.
   */
  currentChange?: number | undefined
  /** Absent ⇒ the stepper is drawn disabled, with the reason in its title. */
  onCurrentChange?: ((index: number) => void) | undefined
  onCollapse: (hunk: number) => void
  /**
   * Gaps the user has opened in a folded whole-file view. Optional, like `view`, so every
   * fixture and call site that predates the whole-file rendering keeps compiling — and an
   * absent set is an empty one, which for a file under `WHOLE_FILE_COLLAPSE_ABOVE` lines is
   * also the only one there is.
   */
  expandedGaps?: ReadonlySet<number> | undefined
  /** Absent ⇒ fold rows draw disabled. The wiring owns the set; this is one gap opening. */
  onExpandGap?: ((gap: number) => void) | undefined
  /**
   * Fold unchanged stretches at any file size (`diffRows.presentSegments`' `foldAlways`).
   * Only the GitLab review sets it; absent keeps the working tree's whole-file rule.
   */
  foldUnchanged?: boolean | undefined
}

/**
 * The working-tree arm: a diff you can stage from.
 *
 * `readOnly` is optional here and `true` on the other arm, so every existing call site — and
 * the smoke fixture — keeps compiling unchanged while still landing in this arm.
 */
export interface GitDiffStagingProps extends GitDiffViewCommon {
  readOnly?: false | undefined
  diff: FileDiff | null
  side: DiffSide
  marks: Marks
  /** Lines of this file already held for the next commit, if any. */
  held: number | null
  onSide: (side: DiffSide) => void
  onMarks: (marks: Marks) => void
  /** Apply these marks: stage, unstage, or hold for the commit — whatever the side means. */
  onApply: (marks: Marks) => void
  onDropHeld: () => void
}

/**
 * The revision arm: two frozen revisions of one file, and nothing to do to them. (M18)
 *
 * It carries **no** `side`, `onApply`, `onMarks` or `onDropHeld`, and their absence is the
 * point of making this a union rather than a boolean beside a bag of optional handlers. With a
 * `readOnly: boolean` and optional callbacks, "read-only means those handlers do not exist" is
 * a convention, and the first caller to pass an `onApply` alongside `readOnly` compiles fine
 * and gets a handler that silently never runs — the listed-and-inert failure this codebase
 * keeps re-shipping. Here it is a type error.
 *
 * `opFor(side)` stays a **total** function over the three `DiffSide`s and is simply unreachable
 * from this arm. A fourth `DiffSide` variant was the other way to spell this and was rejected:
 * `DiffSide` is `Copy`, appears in five Rust signatures, and would force `opFor` to answer
 * "which staging operation does *frozen history* support" — to which the honest answer is that
 * the question does not apply, which a total function cannot say.
 */
export interface GitDiffReadOnlyProps extends GitDiffViewCommon {
  /** Content anchored immediately below a source line, outside the code row's grid. */
  renderAfterLine?: ((oldLine: number | null, newLine: number | null, side: 'old' | 'new' | 'both') => ReactNode) | undefined
  /** Review integrations can attach an inline comment to either side of a displayed row. */
  onReviewLine?: ((side: 'old' | 'new', line: number) => void) | undefined
  reviewLine?: { side: 'old' | 'new'; line: number } | undefined
  readOnly: true
  diff: RevisionDiff | null
  /**
   * The pair, already spelled for display — a short oid, or the word for a side that is not a
   * commit. Resolved by the caller because only it knows what the sides *resolved to*: a
   * `RevSide::FirstParent` is a relative side, and the oid it names is in the answer, not in
   * the request.
   *
   * `from` is `null` when the old side has no commit at all — the file did not exist there, or
   * the side is the working tree — which the header draws as an addition rather than as a pair.
   */
  revisions: { from: string | null; to: string }
}

export type GitDiffViewProps = GitDiffStagingProps | GitDiffReadOnlyProps

/** Shared by every read-only render, so the empty case allocates nothing per frame. */
const NO_MARKS: Marks = new Set<string>()

/** The default for {@link GitDiffViewCommon.expandedGaps} — one instance, for the same reason. */
const NO_GAPS: ReadonlySet<number> = new Set<number>()

/**
 * The diff, its gutters, and — on the working-tree arm — the three buttons. Pure.
 *
 * Every "is this row selected" answer comes from `highlightedKeys`, computed once for the
 * whole render — never from `marks.has(...)` inline. That is the difference between a
 * component that agrees with the wire selection and one that merely usually does: the wire
 * selection is derived from the same function, so a row painted here and a position sent to
 * Rust cannot come apart without one of them throwing.
 *
 * Taken as one `props` object rather than destructured in the signature, which is a departure
 * from this file's style and is what makes the union work: TypeScript narrows a discriminated
 * union through the *reference*, so `props.readOnly === true` is what turns `props.marks` into
 * a compile error on the read-only arm. Destructuring in the parameter list throws that away
 * and would need every arm-specific field to become optional — which is the boolean-plus-
 * optional-handlers shape this union exists to avoid.
 */
export function GitDiffView(props: GitDiffViewProps): ReactNode {
  const {
    diff,
    path,
    collapsed,
    busy,
    note,
    reason,
    view = 'unified',
    onView,
    onBlame,
    onCollapse,
    expandedGaps = NO_GAPS,
    onExpandGap,
    foldUnchanged = false,
  } = props
  /**
   * The staging half of the union, or `null` on a revision diff.
   *
   * Resolved once, here, so that every control below asks the same question in the same way.
   * Reading `props.readOnly` at each site would work and would put the discriminator in nine
   * places; one `staging === null` is also what makes the withheld controls legible as a set.
   */
  const staging = props.readOnly === true ? null : props
  /** The pair to draw in the header, or `null` when this is a working-tree diff. */
  const revisions = props.readOnly === true ? props.revisions : null
  const marks: Marks = staging?.marks ?? NO_MARKS
  /**
   * The column's two questions, kept apart. See {@link GitDiffViewCommon.blame}.
   *
   * `blameOn` is what the toggle reflects and `blame` is what there is to draw; they differ for
   * exactly as long as a fetch is in flight, and for ever on a diff whose blame was refused.
   */
  const blameOn = props.blame !== undefined
  const blame = props.blame ?? null
  // Beside `blame` rather than in the destructure above, because the two are the same kind of
  // thing: an optional the view draws when it is there and ignores when it is not.
  const tokens = props.tokens ?? null
  const onCurrentChange = props.onCurrentChange
  const visible = props.visible ?? true
  const painted =
    staging === null || staging.diff === null
      ? new Set<string>()
      : highlightedKeys(staging.marks, staging.diff)

  /*
   * How wide the pane is, so side-by-side can refuse to draw itself in a sliver.
   *
   * Measured here rather than passed in, and that does not make this component impure in the
   * sense the module comment means: it still talks to no store and no IPC, and
   * `renderToStaticMarkup` runs no effects, so under node this is the initial state and
   * nothing else. The alternative — a `width` prop — would put the measurement in the wiring
   * component, which does not render the element being measured; a `ResizeObserver` needs the
   * node, and the node is here.
   *
   * A callback ref rather than `useRef`, because the two returns below mount different roots:
   * the "nothing to show" branch and the diff branch are separate elements, and a plain ref
   * with an empty dependency list would observe whichever existed at mount and never notice
   * the swap.
   *
   * Starts wide. A pane that is in fact narrow shows one split frame and settles; starting
   * narrow would make every *wide* pane flash unified first, which is the commoner case and
   * the more visible flicker.
   */
  const [root, setRoot] = useState<HTMLElement | null>(null)
  const [wide, setWide] = useState(true)
  /** This pane's identity in `resizeGesture`'s queue. An object, so it cannot collide. */
  const settleKey = useRef({})
  useEffect(() => {
    if (root === null || typeof ResizeObserver === 'undefined') return
    const key = settleKey.current
    const observer = new ResizeObserver((entries) => {
      const width = entries[0]?.contentRect.width ?? root.clientWidth
      /*
       * Deferred to the end of a resize gesture. Crossing `DIFF_SPLIT_MIN_PX` swaps the whole
       * body between split and unified — a full re-render and relayout of the diff — and doing
       * that in the middle of a drag means doing it twice for a pointer that wandered over the
       * threshold and back. See `@/layout/resizeGesture`.
       */
      whenResizeSettles(key, () => setWide(width >= DIFF_SPLIT_MIN_PX))
    })
    observer.observe(root)
    return () => {
      observer.disconnect()
      cancelResizeSettle(key)
    }
  }, [root])

  /** What is actually drawn, and whether the preference had to be overruled to get there. */
  const layout: DiffView = view === 'split' && wide ? 'split' : 'unified'
  const cramped = view === 'split' && !wide

  /*
   * Where the reader had got to, so parking a tab does not lose their place. (M38)
   *
   * The rows go while the tab is behind another one — see the `parked` body below for why —
   * and a scroller with no content has nowhere to be scrolled to, so the browser cannot keep
   * this for us. One entry per scroller because the split layout has two and they are scrolled
   * independently by the reader (the sync only *maps* one onto the other).
   *
   * A ref and not state: nothing renders from it, and a scroll event that re-rendered the whole
   * row list would be the opposite of the point.
   */
  const scrollMemo = useRef({ unified: 0, old: 0, new: 0 })
  const rememberScroll = useCallback((event: UIEvent<HTMLElement>): void => {
    const el = event.currentTarget
    const side = el.dataset['side']
    scrollMemo.current[side === 'old' ? 'old' : side === 'new' ? 'new' : 'unified'] = el.scrollTop
  }, [])
  /*
   * Spending it, on the frame the rows come back.
   *
   * Found through `root` rather than through `leftCol`/`rightCol`, and that is not a stylistic
   * choice: those are callback-ref *state*, set during the commit that mounts the columns, so
   * on this pass they are still `null` and a layout effect reading them would restore nothing.
   * `root` is already set — the pane's own element is drawn whether the body is parked or not,
   * which is also why parking does not tear down `setRoot`.
   *
   * Keyed on `visible` alone. A `layout` flip has its own correct answer (`currentAnchor`'s
   * effect re-runs and re-centres the change being read), and this must not fight it.
   */
  useLayoutEffect(() => {
    if (!visible || root === null) return
    const memo = scrollMemo.current
    for (const el of root.querySelectorAll<HTMLElement>('[data-audit="gitDiffScroller"]')) {
      const side = el.dataset['side']
      const want = memo[side === 'old' ? 'old' : side === 'new' ? 'new' : 'unified']
      if (want > 0) el.scrollTop = want
    }
  }, [visible, root])

  /*
   * Whether the tick boxes are drawn at all.
   *
   * `staging === null` withholds them from the whole revision arm: there is no index to
   * stage two commits into, so a box there would be a control whose only possible outcome is
   * a refusal from Rust. Computed up here, before the no-diff return, because the split
   * model below is a hook and hooks cannot sit past a conditional return.
   */
  const selectable =
    staging !== null &&
    staging.diff !== null &&
    staging.diff.partialOk &&
    (diff?.hunks.length ?? 0) > 0

  /*
   * The whole file, reconstructed — or `null`, which means every rendering below falls back
   * to the hunks alone, exactly as this pane drew before M25. `diffRows.wholeFileSegments`
   * validates the text against the hunks and refuses any disagreement, so a `null` here is
   * "cannot be trusted whole", never an error.
   */
  const whole = useMemo(
    () =>
      diff === null || diff.hunks.length === 0
        ? null
        : wholeFileSegments(diff.hunks, diff.newText),
    [diff],
  )
  const totalLines = useMemo(
    () => (diff?.newText == null ? 0 : splitLines(diff.newText).length),
    [diff],
  )
  const segments: DisplaySegment[] | null = useMemo(
    () =>
      whole === null ? null : presentSegments(whole, totalLines, expandedGaps, foldUnchanged),
    [whole, totalLines, expandedGaps, foldUnchanged],
  )

  /*
   * The split view's two columns and the run table that aligns them.
   *
   * The fallback arm (no whole-file model) feeds the same renderer from the hunks alone:
   * bars carry the `@@` header — the only landmark between discontinuous line numbers — and
   * honour the collapsed set, as the unified fallback does. The whole-file arm draws bars
   * only where there is a staging affordance to hold; a read-only whole file has its line
   * numbers for landmarks and a bar would say nothing they do not.
   */
  /*
   * Built whatever the layout, because the **changes iterator** reads the run table too and a
   * unified diff has changes to walk exactly as a split one does. (M25)
   *
   * Hoisting it out of the `layout === 'split'` test is safe, and the reason is worth stating
   * rather than assumed: `bar` rows are `shared` runs, so drawing them or not cannot change
   * *which* runs are changes — only their row indices, and the unified rendering never uses a
   * row index (it addresses rows by `hunk:at`). The `collapsed` set does change the answer, and
   * it changes it **correctly**: `columnRows` folds a collapsed hunk's lines away and the
   * unified fallback renders `{!shut && …}`, so the anchors are exactly the changes currently in
   * the DOM. That is what makes "Next change never scrolls to something invisible" true by
   * construction rather than by a filter somebody has to remember to apply.
   */
  const model = useMemo(() => {
    if (diff === null || diff.hunks.length === 0) return null
    return segments === null
      ? columnRows(hunkSegments(diff.hunks), { bars: true, collapsed })
      /*
       * **No hunk bars in the split whole-file view.** (M25)
       *
       * They were `bars: selectable`, and what that drew was a row per hunk in *both* columns
       * carrying a tri-state box on the left and nothing at all on the right — a checkbox facing
       * a blank line, twice per hunk, in a view whose whole job is to put the two documents side
       * by side. Reported in those words and it is the right call: the bar's landmark value is
       * the `@@` header, and a whole-file rendering has none to show (the line numbers are
       * continuous and on screen), so all that was left of it was the affordance.
       *
       * The fallback above keeps its bars, and must: with only the hunks drawn the headers are
       * the only landmark between discontinuous line numbers. Its controls moved to the new side
       * with the per-line boxes — see `hunkBar`.
       */
      : columnRows(segments, { bars: false })
  }, [diff, segments, collapsed, selectable])
  /** The same table, when it is what the split layout is drawing from. Rendering is unchanged. */
  const split = layout === 'split' ? model : null

  /*
   * The changes to walk, and which one is current. (M25)
   *
   * `changeAnchors` is a fourth consumer of the run table above, which is the property
   * `columnRows`' comment claims for the existing three: the two columns and `diffSync` cannot
   * disagree about where a run begins, and now neither can the iterator.
   */
  const anchors = useMemo(() => (model === null ? [] : changeAnchors(model)), [model])
  // Clamped on read rather than written back during render: the wiring owns the number, and a
  // diff that shrank under an open pane must not make this component set state while rendering.
  const currentChange =
    props.currentChange === undefined ? -1 : Math.min(props.currentChange, anchors.length - 1)
  const currentAnchor = currentChange < 0 ? null : (anchors[currentChange] ?? null)
  /*
   * Every row of the current change, as `hunk:at` keys.
   *
   * Through `mark()` — the same function `painted` uses — so the two spellings of "which rows
   * does this position set contain" cannot drift. Every row of the run and not merely its first:
   * an eight-line paired edit is one change and has to read as one block.
   */
  const currentKeys = useMemo(() => {
    if (model === null || currentAnchor === null) return new Set<string>()
    return new Set(changePositions(model, currentAnchor).map((p) => mark(p.hunk, p.at)))
  }, [model, currentAnchor])

  /*
   * Keeping the two columns in step.
   *
   * # Why two scrollers now, when this pane spent a comment arguing for one
   *
   * The split view used to be one grid scroller with hatched filler cells wherever a side
   * had no line — alignment by construction, no sync to write. The fillers are what lost:
   * a column of empty cells beside an added block reads as blank lines that are not there,
   * and IDEA's answer — the left column simply flows on, with a thin marker at the insertion
   * point — needs the columns to be different heights, which one scroller cannot draw.
   *
   * `@codemirror/merge`'s `MergeView` still loses too, even though the whole documents it
   * needs exist since M25: it aligns by inserting spacer blocks, which is the same empty
   * space with different plumbing, and this pane's tick boxes, wire positions and blame are
   * plain DOM keyed by `hunk:line` — rebuilding those as CodeMirror extensions buys nothing
   * but the rebuild.
   *
   * The scroll-echo loop that one-scroller design feared is real and answered: writing the
   * other column's `scrollTop` fires that column's own scroll event, *asynchronously*, so a
   * time-released flag is down before the echo lands — `MergePane.tsx` documents the failure
   * at length and its Set-of-marks guard is reused here verbatim in spirit. The mapping is
   * `diffSync.mapScroll` over anchors measured once per render and after a *settled* resize
   * (`check:resize`'s rule), never per scroll frame.
   */
  const [connector, setConnector] = useState<SVGSVGElement | null>(null)
  const [leftCol, setLeftCol] = useState<HTMLElement | null>(null)
  const [rightCol, setRightCol] = useState<HTMLElement | null>(null)
  const echoes = useRef<Set<'left' | 'right'>>(new Set())
  const syncKey = useRef({})
  useEffect(() => {
    if (leftCol === null || rightCol === null || split === null) return
    const spans = rowSpans(split.runs)
    const key = syncKey.current
    const echoSet = echoes.current
    let geometry: SyncGeometry | null = null

    /** Pixel top of every row (markers skipped — they are height 0), plus an end sentinel. */
    const rowTops = (column: HTMLElement): number[] | null => {
      const content = column.firstElementChild
      if (!(content instanceof HTMLElement)) return null
      const tops: number[] = []
      for (const child of Array.from(content.children)) {
        if (!(child instanceof HTMLElement)) continue
        if (child.dataset['audit'] === 'gitDiffInsertMark' || child.dataset['audit'] === 'gitDiffAnnotation') continue
        tops.push(child.offsetTop)
      }
      tops.push(column.scrollHeight)
      return tops
    }
    const measure = (): void => {
      const left = rowTops(leftCol)
      const right = rowTops(rightCol)
      if (left === null || right === null) {
        geometry = null
        return
      }
      const anchors: Array<{ a: number; b: number }> = [{ a: 0, b: 0 }]
      for (const span of spans) {
        const a1 = left[span.leftFrom]
        const b1 = right[span.rightFrom]
        const a2 = left[span.leftTo]
        const b2 = right[span.rightTo]
        if (a1 === undefined || b1 === undefined || a2 === undefined || b2 === undefined) continue
        anchors.push({ a: a1, b: b1 }, { a: a2, b: b2 })
      }
      anchors.push({ a: leftCol.scrollHeight, b: rightCol.scrollHeight })
      geometry = {
        anchors,
        aMax: Math.max(0, leftCol.scrollHeight - leftCol.clientHeight),
        bMax: Math.max(0, rightCol.scrollHeight - rightCol.clientHeight),
      }
    }
    measure()

    // Content height moves without the column resizing — a blame landing changes wrapping,
    // a fold opens — so the observer watches the inner content, and the re-measure is
    // deferred to a settled size like every other expensive resize reaction.
    const observer =
      typeof ResizeObserver === 'undefined'
        ? null
        : new ResizeObserver(() => whenResizeSettles(key, measure))
    const leftContent = leftCol.firstElementChild
    const rightContent = rightCol.firstElementChild
    if (observer !== null && leftContent !== null) observer.observe(leftContent)
    if (observer !== null && rightContent !== null) observer.observe(rightContent)

    const follow = (fromName: 'left' | 'right'): void => {
      // Our own doing: consume the mark and stop. This is the whole loop guard.
      if (echoSet.delete(fromName)) return
      if (geometry === null) return
      const from = fromName === 'left' ? leftCol : rightCol
      const to = fromName === 'left' ? rightCol : leftCol
      const want = mapScroll(geometry, fromName === 'left' ? 'a' : 'b', from.scrollTop)
      // Clamped before compared: a write clamped to the value already there fires no event,
      // and a mark laid for it would linger and swallow the next real scroll.
      if (Math.abs(want - to.scrollTop) < 1) return
      echoSet.add(fromName === 'left' ? 'right' : 'left')
      to.scrollTop = want
    }
    const onLeft = (): void => follow('left')
    const onRight = (): void => follow('right')
    leftCol.addEventListener('scroll', onLeft, { passive: true })
    rightCol.addEventListener('scroll', onRight, { passive: true })
    return () => {
      leftCol.removeEventListener('scroll', onLeft)
      rightCol.removeEventListener('scroll', onRight)
      observer?.disconnect()
      cancelResizeSettle(key)
      echoSet.clear()
    }
  }, [leftCol, rightCol, split, blame])

  /*
   * Drawing the connector's ribbons. (M25)
   *
   * A second effect rather than a branch inside the sync above, and the split is by *what
   * triggers it*: the sync reacts to a scroll by writing the other column, which is a one-shot
   * correction, while this has to redraw on every frame of every scroll of either column and on
   * every relayout. Folding them together would mean the sync's echo guard — whose whole job is
   * to make the *second* of a pair of events do nothing — swallowing half the redraws.
   *
   * It measures with the same rule `rowTops` uses (skip the zero-height insertion markers, and
   * carry one extra entry for the content height) because it is indexing the same `ColumnRow`
   * arrays. That rule is stated twice in this file and both statements name each other; a third
   * copy would be the one that drifts.
   */
  useEffect(() => {
    if (connector === null || leftCol === null || rightCol === null || split === null) return
    const runs = split.runs
    let raf = 0
    let last = ''

    const topsOf = (column: HTMLElement): number[] | null => {
      const content = column.firstElementChild
      if (!(content instanceof HTMLElement)) return null
      const tops: number[] = []
      for (const child of Array.from(content.children)) {
        if (!(child instanceof HTMLElement)) continue
        if (child.dataset['audit'] === 'gitDiffInsertMark' || child.dataset['audit'] === 'gitDiffAnnotation') continue
        tops.push(child.offsetTop)
      }
      tops.push(content.offsetHeight)
      return tops
    }

    /**
     * The measurement, held between frames.
     *
     * **`offsetTop` does not move when a scroller scrolls**, and that is the whole justification:
     * every number `topsOf` collects is a position inside the column's content, so re-reading
     * them per frame was measuring something that could not have changed. What it cost was real
     * — `Array.from(content.children)` plus an `offsetTop` per row is a forced synchronous layout
     * over the whole column, twice, and this runs on every animation frame of every scroll of
     * either side. On a diff of a few thousand rows that is the whole frame budget, and the
     * symptom is a scrollbar that stutters rather than anything visibly wrong.
     *
     * `null` means "not measured yet". It is cleared by the observer below and by nothing else,
     * which is the invariant to keep: anything that can move a row must invalidate here.
     */
    let geometry: { left: number[]; right: number[]; width: number; height: number } | null = null
    const measure = (): void => {
      const left = topsOf(leftCol)
      const right = topsOf(rightCol)
      geometry =
        left === null || right === null
          ? null
          : {
              left,
              right,
              // The connector's own box is measured here too, for the same reason: `clientWidth`
              // is a layout read, and it changes when the pane resizes — which is exactly when
              // the observer fires.
              width: connector.clientWidth,
              height: connector.clientHeight,
            }
    }

    const paint = (): void => {
      raf = 0
      if (geometry === null) measure()
      if (geometry === null) return
      const { left: leftTops, right: rightTops, width, height } = geometry
      const shapes = connectorShapes(
        runs,
        leftTops,
        rightTops,
        leftCol.scrollTop,
        rightCol.scrollTop,
        width,
        height,
      )
      /*
       * Rebuilt as one markup string and compared with the last one before touching the DOM.
       *
       * This runs on a scroll frame. A diff of a few hundred changes would otherwise be a few
       * hundred `setAttribute` calls sixty times a second while somebody drags a scrollbar, and
       * the great majority of those frames move no shape at all — the columns scroll in step, so
       * a ribbon only moves relative to the gutter when the two sides disagree about how far.
       * That is also why `connectorShapes` rounds its coordinates.
       */
      const markup = shapes
        .map((shape) => `<path d="${shape.d}" class="${styles.ribbon ?? ''}" data-tone="${shape.kind}"/>`)
        .join('')
      if (markup === last) return
      last = markup
      connector.innerHTML = markup
    }

    const schedule = (): void => {
      if (raf === 0) raf = requestAnimationFrame(paint)
    }
    /** A size changed, so the held measurement is stale. The only thing that may clear it. */
    const remeasure = (): void => {
      geometry = null
      schedule()
    }

    paint()
    leftCol.addEventListener('scroll', schedule, { passive: true })
    rightCol.addEventListener('scroll', schedule, { passive: true })
    // The columns' content, not the columns: a fold opening changes the content's height without
    // the scroller's box moving at all, and that is precisely when every ribbon below it moves.
    const observer =
      typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(() => remeasure())
    for (const column of [leftCol, rightCol, connector]) {
      const target = column === connector ? connector : column.firstElementChild
      if (observer !== null && target !== null) observer.observe(target)
    }
    return () => {
      if (raf !== 0) cancelAnimationFrame(raf)
      leftCol.removeEventListener('scroll', schedule)
      rightCol.removeEventListener('scroll', schedule)
      observer?.disconnect()
      connector.innerHTML = ''
    }
  }, [connector, leftCol, rightCol, split, blame])

  /*
   * Scrolling the current change into view. (M25)
   *
   * # One scroller is written, never two
   *
   * `follow` above treats a programmatic `scrollTop` write exactly as it treats a person's: it
   * finds no mark in `echoes`, maps through `mapScroll`, and moves the *other* column itself.
   * That is what we want, and it is why nothing here touches the echo set or `diffSync` at all.
   *
   * Writing **both** columns would be the bug. Two writes are two scroll events, each of which
   * moves the other column, and the second write is then overwritten by the first one's
   * follow-up — so the reader lands somewhere neither call asked for. The other column is
   * *derived*, through the only code that knows both geometries.
   *
   * # Found by attribute, never by index
   *
   * The columns interleave zero-height `.insertMark` divs between rows — which is why `rowTops`
   * skips them — so a child index is not a row index, and reproducing that skip in a second
   * place is how the two come to disagree. `data-at` is the one identity all three renderings
   * share (split columns, whole-file unified, hunks-only fallback), and `ColumnRow`'s contract
   * guarantees every position occurs exactly once across both columns, so the query is
   * unambiguous within the pane.
   *
   * # `scrollTop`, never `scrollIntoView`
   *
   * `scrollIntoView` walks *ancestor* scrollers as well, which in this app can move the tab
   * content and the window under a reader who asked for the next change in one pane.
   */
  useEffect(() => {
    if (root === null || currentAnchor === null) return
    const first = root.querySelector<HTMLElement>(
      `[data-at="${currentAnchor.first.hunk}:${currentAnchor.first.at}"]`,
    )
    if (first === null) return
    const scroller = first.closest<HTMLElement>('[data-audit="gitDiffScroller"]')
    if (scroller === null) return
    const last =
      root.querySelector<HTMLElement>(
        `[data-at="${currentAnchor.last.hunk}:${currentAnchor.last.at}"]`,
      ) ?? first
    /*
     * Rects rather than `offsetTop`, unlike the sync's `rowTops` above.
     *
     * That one can use `offsetTop` because `.column` is `position: relative` and is therefore the
     * offset parent of its own rows — a fact `check:diff-render` pins in the stylesheet. The
     * unified `.body` carries no such rule, so a row's `offsetTop` there is measured against
     * whatever happens to be positioned further up the tree. Rects are relative to the viewport
     * and need no such assumption, and this runs once per keypress rather than per frame.
     */
    const box = scroller.getBoundingClientRect()
    const firstRect = first.getBoundingClientRect()
    const lastRect = last.getBoundingClientRect()
    const top = firstRect.top - box.top + scroller.scrollTop
    const height = Math.max(lastRect.bottom - firstRect.top, firstRect.height)
    const view = scroller.clientHeight
    // Centred, matching MergePane's `y: 'center'` so the two surfaces feel the same — except for
    // a run taller than the viewport, which is the case centring gets wrong: it would put the
    // start of a 200-line block off the top of the screen.
    const want = height >= view - SCROLL_PAD * 2 ? top - SCROLL_PAD : top - (view - height) / 2
    scroller.scrollTop = Math.max(0, Math.min(want, scroller.scrollHeight - view))
    // `model` and `blame` are in the list because both change every row's geometry; the effect
    // and not the click handler, so `data-current` is committed and the scroll lands together.
  }, [root, currentAnchor, layout, model, blame])

  /*
   * Announcing this diff to `navigate.nextChange` / `navigate.prevChange`. (M25)
   *
   * The `ChangeNav` handed over must keep one identity for the life of the claim — it is what
   * `release` removes — so the live answers ride a ref that is written during render. That is
   * this file's existing idiom; see `marksRef` below.
   */
  const visibleRef = useRef(visible)
  visibleRef.current = visible
  const navImpl = useRef<ChangeNav>({ step: () => false, cursor: () => NO_CURSOR })
  const navStable = useRef<ChangeNav>({
    step: (delta) => navImpl.current.step(delta),
    cursor: () => navImpl.current.cursor(),
  })
  navImpl.current = {
    step: (delta) => {
      const next = stepIndex(anchors.length, currentChange, delta)
      if (next === null || onCurrentChange === undefined) return false
      onCurrentChange(next)
      return true
    },
    cursor: () => ({ index: currentChange, count: anchors.length }),
  }
  const navRef = useRef<ChangeNavSlot | null>(null)
  useEffect(() => {
    const slot = claimChangeNav(navStable.current, visibleRef.current)
    navRef.current = slot
    return () => {
      slot.release()
      navRef.current = null
    }
    // Once per mount. `visible` moves the claim through the effect below rather than by
    // retaking it, so that a tab switch does not drop and re-add a slot the dispatcher may be
    // reading between the two.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
  useEffect(() => {
    if (visible) navRef.current?.focus()
  }, [visible])

  /*
   * Next / previous change. (M25)
   *
   * The same gesture `navigate.nextChange` fires, and the same clamp: disabled at the ends
   * rather than wrapping, so the control states where the walk stops instead of leaving the
   * reader to discover it by being teleported. `MergePane`'s block stepper is the same pair of
   * chevrons for the same reason.
   *
   * Drawn even when there is nothing to walk — a control that appears and disappears as diffs
   * are opened reads as a bug rather than as a refusal — with the reason in the title.
   */
  const stepChange = (delta: 1 | -1): void => {
    const next = stepIndex(anchors.length, currentChange, delta)
    if (next !== null) onCurrentChange?.(next)
  }
  const changeStepper = (
    <div className={styles.sides} role="group" aria-label="Changes">
      {([-1, 1] as const).map((delta) => (
        <button
          key={delta}
          type="button"
          className={styles.side}
          data-audit="gitDiffStep"
          data-delta={delta === 1 ? 'next' : 'prev'}
          disabled={onCurrentChange === undefined || stepIndex(anchors.length, currentChange, delta) === null}
          title={
            anchors.length === 0
              ? 'This diff has no changes to step through.'
              : delta === 1
                ? `Next change (${Math.max(currentChange + 1, 0)} of ${anchors.length})`
                : `Previous change (${Math.max(currentChange + 1, 0)} of ${anchors.length})`
          }
          onClick={() => stepChange(delta)}
        >
          <Icon name={delta === 1 ? 'chevron-down' : 'chevron-up'} size={0} />
        </button>
      ))}
    </div>
  )

  const layoutSwitcher = (
    <div className={styles.sides} role="group" aria-label="Diff layout">
      {(['unified', 'split'] as const).map((mode) => (
        <button
          key={mode}
          type="button"
          title={
            mode === 'unified'
              ? 'One column, deletions and additions interleaved.'
              : cramped
                ? `This pane is under ${DIFF_SPLIT_MIN_PX}px wide, so it is drawn unified.` +
                  ' Widen it, or detach the tab, to get the two sides.'
                : 'The two sides beside each other, scrolled in step.'
          }
          aria-pressed={view === mode}
          data-audit="gitDiffLayout"
          data-overruled={mode === 'split' && cramped ? 'true' : 'false'}
          disabled={onView === undefined}
          className={view === mode ? `${styles.side} ${styles.sideOn}` : styles.side}
          onClick={() => onView?.(mode)}
        >
          {mode === 'split' ? 'Split' : 'Unified'}
        </button>
      ))}
    </div>
  )

  /*
   * Blame. A third segmented control, of one segment, beside the layout switcher. (M18)
   *
   * **Off by default**, and that is a decision rather than an oversight: the column is 22
   * characters of the monospace face — the same width the editor's gutter takes, so a file open
   * in both does not have two different margins — and it comes out of the text column in every
   * hunk of every diff. This pane's job is getting a selection into the index, and the question
   * it is built around is "what changed", not "who changed it". A reader who wants the second
   * question asks for it, once, per tab.
   *
   * Drawn even when it cannot be used, disabled, with the reason in the title. The alternative —
   * hiding it on the staged side — would leave a control that appears and disappears as the side
   * switcher is clicked, which reads as a bug rather than as a refusal.
   */
  const blameRefused = staging === null ? null : blameRefusal(staging.side)
  const blameSwitcher = (
    <div className={styles.sides} role="group" aria-label="Blame">
      <button
        type="button"
        title={
          blameRefused ??
          (onBlame === undefined
            ? 'This diff cannot be annotated.'
            : 'Who last changed each line of the new side. The old side is never annotated —' +
              ' that needs a second blame at the other revision.')
        }
        aria-pressed={blameOn}
        data-audit="gitDiffBlameToggle"
        /*
         * Refused *and* unhandled, not either. Withholding `onBlame` is the wiring's job and it
         * does it; asserting the rule here too is what makes "the staged side is not blamable"
         * unrepresentable rather than merely observed — a caller that passed a handler for it
         * would otherwise get a working button and a column that lies by a line or two.
         */
        disabled={onBlame === undefined || blameRefused !== null}
        className={blameOn ? `${styles.side} ${styles.sideOn}` : styles.side}
        onClick={() => onBlame?.(!blameOn)}
      >
        Blame
      </button>
    </div>
  )

  /*
   * The header is drawn whether or not there is a diff, and the side switcher with it.
   *
   * `git_diff_file` answers `NoSuchChange` for a file with nothing on the side being asked
   * about — a file with no staged changes, viewed on `Staged`, is the ordinary case — and a
   * failure view without the switcher is a pane the user cannot get out of except by closing
   * the tab and opening it again from the panel.
   *
   * On the revision arm there is no switcher, because there is nothing to switch: the pair is
   * the tab's identity (see `cmd::file::shows_revision_diff`), and a control that re-pointed it
   * would be re-pointing the tab. What takes its place is the pair itself, written out — a diff
   * of two commits with nothing on screen saying *which* two is a diff the reader cannot check.
   */
  const header = (
    <header className={styles.header}>
      <span className={styles.path}>{diff?.path ?? path}</span>
      {diff?.oldPath != null && (
        <>
          <span className={styles.arrow}>
            <Icon name="arrow-left-right" size={0} />
          </span>
          <span className={styles.path}>{diff.oldPath}</span>
        </>
      )}
      {diff !== null && <span className={styles.status}>{diff.status}</span>}
      {/* One right-aligned group, so a long path clips against both switchers rather than
          having the free space split between two `margin-left: auto` siblings. */}
      <div className={styles.controls}>
        {changeStepper}
        {layoutSwitcher}
        {blameSwitcher}
        {revisions !== null ? (
          /*
           * The pair, oldest first, in the direction the diff reads: `9f8e7d6 → a1b2c3d`.
           *
           * A `<span>` group and not buttons, and that is the honest shape rather than a
           * missing feature: nothing here is clickable because nothing about this tab can be
           * changed from it. `from === null` is the side that has no commit — a file added in
           * this revision, or a comparison against the working tree — and it says so in words
           * instead of drawing an arrow out of nothing.
           */
          <span
            className={styles.status}
            data-audit="gitDiffRevisions"
            title="This diff is between two revisions and cannot be staged from."
          >
            {revisions.from === null ? `added in ${revisions.to}` : `${revisions.from} → ${revisions.to}`}
          </span>
        ) : (
          <div className={styles.sides} role="group" aria-label="Diff side">
            {SIDES.map((entry) => (
              <button
                key={entry.side}
                type="button"
                title={entry.title}
                aria-pressed={staging?.side === entry.side}
                className={
                  staging?.side === entry.side ? `${styles.side} ${styles.sideOn}` : styles.side
                }
                data-audit="gitDiffSide"
                onClick={() => staging?.onSide(entry.side)}
              >
                {entry.label}
              </button>
            ))}
          </div>
        )}
      </div>
    </header>
  )

  if (diff === null) {
    return (
      <div className={styles.pane}
        data-audit="gitDiffPane"
        ref={setRoot}
        // Settles the one tie `visible` cannot: a git diff tab in front and the log
        // tool window open on a revision diff are both mounted and both claim, and
        // without this the slot would keep whichever mounted last however long the
        // reader worked in the other. A pointer press is the cheapest true answer to
        // "which of them am I in".
        onPointerDown={() => navRef.current?.focus()}>
        {header}
        <div className={styles.notice}>
          {reason === null ? (
            'Reading the diff…'
          ) : (
            <>
              <div>Nothing to show on this side.</div>
              <div className={styles.noticeWhy} data-audit="gitDiffWhy">
                {reason}
              </div>
            </>
          )}
        </div>
      </div>
    )
  }

  const selection =
    staging === null || staging.diff === null ? null : toSelection(staging.marks, staging.diff)
  const totalChanges = diff.hunks.reduce(
    (n, hunk) => n + hunk.lines.filter((l) => l.origin !== 'context').length,
    0,
  )
  const op = staging === null ? null : opFor(staging.side)
  const actionLabel =
    op === 'stage' ? 'Stage selection' : op === 'unstage' ? 'Unstage selection' : 'Use for commit'
  const held = staging?.held ?? null

  /**
   * One line of one side.
   *
   * Shared by both layouts, which is what keeps the rule this pane exists for true in the new
   * one: the `data-at` / `data-selected` pair is written in exactly one place, from the same
   * `painted` set, so the split view cannot drift into painting a row the unified view would
   * not — and `check-diff-render.mjs`'s regex over `data-audit="gitDiffRow"` reads either.
   *
   * `at === null` is the mirrored half of a context row: same text, no position, no box. It
   * carries no `data-at`, so a mark is never in the document twice.
   *
   * The blame cell goes between the tick box and the line numbers, and `which` decides whether
   * it is an annotation or a spacer — see below.
   */
  const cell = (
    key: string,
    hunkIndex: number,
    entry: SplitCell,
    which: 'old' | 'new' | 'both',
    className: string,
  ): ReactNode => {
    const { line, at } = entry
    const change = line.origin !== 'context'
    const on = at !== null && painted.has(mark(hunkIndex, at))
    const numbered = which === 'new' ? line.newLineno : line.oldLineno
    /*
     * Which commit wrote this line — on the new side only.
     *
     * `which === 'old'` is the split layout's left column, and it has no blame cell and no
     * blame track at all. The reason is the same one `blameFor` gives for refusing
     * `oldLineno`: the left column is a *different document*, and annotating it needs a
     * second blame at the other revision. The columns are independent grids now, so the
     * missing track costs nothing to alignment — the scroll sync's anchors are measured off
     * the rendered rows, whatever their widths.
     */
    const annotated = blame !== null && which !== 'old' ? blameFor(blame, line) : null
    /*
     * The runs to draw this line with, or `null` for "draw it plain".
     *
     * `lineTokens` compares the row's content against the line its tokens spell and refuses on
     * any difference — see `diffTokens.ts` for why that guard is the whole point of the module.
     * A miss here costs this row its colour and nothing else.
     */
    const coloured = lineTokens(tokens, line)
    /*
     * Whether *this column* draws a tick box. (M25)
     *
     * The left column does not, and that is a deliberate narrowing rather than an oversight.
     * Two boxes facing each other across the gutter read as two independent selections of one
     * file, which is the opposite of what the model does — a mark is a `hunk:line` position and
     * the sides are two views of one set. One column of boxes says that; two invite the
     * question of what ticking both would mean.
     *
     * **The cost is real and is stated rather than discovered: a deletion cannot be ticked in
     * the split layout.** A deletion exists only on the old side, so its position rides the left
     * column and there is now no box on it — the row still *paints* as selected when it is, so
     * the split view shows the selection faithfully, it just cannot originate one there.
     * Unified is the layout that stages, which is also where the hunk boxes live; split is the
     * one that reads. Requested in exactly those terms.
     */
    const reviewNumber = (side: 'old' | 'new', number: number | null) => {
      const handler = props.readOnly === true ? props.onReviewLine : undefined
      return handler && number !== null
        ? <button type="button" className={styles.reviewLine}
            data-review-side={side} data-review-line={number}
            aria-label={`Comment on ${side} line ${number}`}
            aria-pressed={props.readOnly === true && props.reviewLine?.side === side && props.reviewLine.line === number}
            onClick={() => handler(side, number)}>{number}</button>
        : number ?? ''
    }
    const boxSide = selectable && which !== 'old'
    const annotation = props.readOnly === true ? props.renderAfterLine?.(line.oldLineno, line.newLineno, which) : null
    return (
      <Fragment key={key}>
      <div
        className={`${className} ${rowClass(line.origin)}`}
        {...(at === null
          ? {}
          : {
              'data-audit': 'gitDiffRow',
              'data-at': `${hunkIndex}:${at}`,
              'data-selected': on ? 'true' : 'false',
              // **Appended last, and it has to stay last.** `gitDiffSmoke.tsx`'s row regex
              // matches these three attributes in order; inserting anywhere above would leave
              // it matching nothing and every positional digest silently empty.
              'data-current': currentKeys.has(mark(hunkIndex, at)) ? 'true' : 'false',
            })}
      >
        {/*
          * The line number, before the tick box on the new side. (M25)
          *
          * Three orders, one per column, because the two split columns are read from opposite
          * directions: the left column's number sits at its right edge and the right column's at
          * its left, so the two meet either side of the connector. The unified row is unchanged
          * — it has both numbers, side by side, and no gutter to meet across.
          *
          * Ordering matters and is not cosmetic: a grid places its children **by order**, so
          * these fragments and the per-side track lists in the stylesheet are one decision
          * written in two files, and `check:diff-render` reads the order back off the markup.
          */}
        {which === 'new' && <span className={styles.lineno}>{reviewNumber('new', numbered)}</span>}
        {boxSide && (
          <button
            type="button"
            role="checkbox"
            aria-checked={on}
            aria-label={`Select line ${numbered ?? line.newLineno ?? line.oldLineno ?? ''}`}
            className={styles.lineBox}
            data-audit="gitDiffLineBox"
            disabled={!change || at === null}
            onClick={() => {
              // `staging` is non-null whenever `selectable` is, but the compiler is asked
              // rather than told: a `!` here would be the one place this union could be
              // subverted, and the whole point of the union is that it cannot be.
              if (at === null || staging === null || staging.diff === null) return
              staging.onMarks(toggleLine(staging.marks, staging.diff, hunkIndex, at))
            }}
          >
            {on ? <Icon name="check" size={0} /> : null}
          </button>
        )}
        {blame !== null && which !== 'old' && (
            <span
              className={styles.blame}
              data-audit="gitDiffBlame"
              /*
               * The oid, empty when there is none — a deletion, or a line the blame does not
               * cover. An attribute rather than only text because the text is clipped to the
               * column and an em dash is not an oid: this is what a check reads, and what a
               * future "open this commit" click would.
               */
              data-blame={annotated?.oid ?? ''}
              /*
               * The age band, straight from `blameModel`'s ramp, or `''` for a row with no
               * attribution at all. `-1` is an uncommitted line, which is a different fact from
               * an absent one and gets a different tint.
               */
              data-age={annotated === null ? '' : `${annotated.bucket}`}
              /*
               * The full label, because the cell clips at 22 characters and a clipped author is
               * the one thing a reader wants back. Unlike the editor's gutter this column has no
               * hover card of its own, so there is no second tooltip for WebKitGTK's to race —
               * see `blameModel.BlameMarker.title` for why the editor cannot use `title`.
               */
              {...(annotated === null
                ? {}
                : {
                    title: annotated.uncommitted
                      ? 'Not in HEAD — this line exists only in the working copy.'
                      : `${annotated.oid} ${annotated.author}`,
                  })}
            >
              {annotated === null
                ? ''
                : annotated.uncommitted
                  ? '—'
                  : `${annotated.oid} ${annotated.author}`}
            </span>
        )}
        {which === 'both' && (
          <>
            <span className={styles.lineno}>{reviewNumber('old', line.oldLineno)}</span>
            <span className={styles.lineno}>{reviewNumber('new', line.newLineno)}</span>
          </>
        )}
        <span className={styles.sign}>
          {line.origin === 'addition' ? '+' : line.origin === 'deletion' ? '-' : ' '}
        </span>
        <span className={styles.text}>
          {/*
            * Token spans, or the bare string exactly as this pane drew it before M25.
            *
            * Three things this deliberately does not disturb, each a silent bug if it did.
            * **Grid placement**: `.row`/`.half` are grids and a grid lays out only its *direct*
            * children, so these spans — inside `.text`, the last track — leave the four
            * `data-boxed`/`data-blamed` column lists alone. **The scroll anchors**: the spans are
            * inline inside a `pre-wrap` box, so they change no box and no line breaking, and the
            * sync's measured row tops are unmoved. **Selection and copy-out**: the `::selection`
            * rules are descendant selectors, and adjacent inline spans introduce no whitespace,
            * so a dragged selection still copies the characters that are on screen.
            *
            * A run with no role gets a `Fragment` and no element at all, so it keeps the pane's
            * own `var(--text)`. Not `--tk-fg`: this is chrome that contains code — its line
            * numbers, sign column and blame cell are all in the chrome family — and letting an
            * imported scheme's ink fight them is a worse answer than leaving the body text be.
            */}
          {coloured === null
            ? line.content
            : coloured.map((token, index) =>
                token.cls === null ? (
                  <Fragment key={index}>{token.text}</Fragment>
                ) : (
                  <span key={index} className={token.cls}>
                    {token.text}
                  </span>
                ),
              )}
          {line.noNewline && <span className={styles.noNewline}> ⏎ no newline at end of file</span>}
        </span>
        {/*
          * The left column numbers on its **right** edge, IDEA's arrangement, so the two
          * columns' numbers meet either side of the connector instead of sitting at the two
          * outer margins of the pane with the code between them. A grid places by order, so
          * this is a real reordering of the children and `.column[data-side='old'] .half`
          * carries the matching track list.
          */}
        {which === 'old' && <span className={styles.lineno}>{reviewNumber('old', numbered)}</span>}
      </div>
      {annotation && <div className={styles.reviewAnnotation} data-audit="gitDiffAnnotation">{annotation}</div>}
      </Fragment>
    )
  }

  /** The tri-state hunk box, one markup for the sticky header and the slim bar. */
  const hunkBox = (hunkIndex: number): ReactNode => {
    const state =
      staging === null || staging.diff === null
        ? 'none'
        : hunkState(staging.marks, staging.diff, hunkIndex)
    return (
      <button
        type="button"
        role="checkbox"
        aria-checked={state === 'all' ? true : state === 'some' ? 'mixed' : false}
        aria-label={`Select hunk ${hunkIndex + 1}`}
        className={styles.box}
        data-audit="gitDiffHunkBox"
        data-state={state}
        onClick={() => {
          if (staging === null || staging.diff === null) return
          staging.onMarks(toggleHunk(staging.marks, staging.diff, hunkIndex))
        }}
      >
        {state === 'all' ? (
          <Icon name="check" size={0} />
        ) : state === 'some' ? (
          <Icon name="minus" size={0} />
        ) : null}
      </button>
    )
  }

  const hunkActionButton = (hunkIndex: number): ReactNode => (
    <button
      type="button"
      className={styles.hunkAction}
      disabled={busy}
      // The whole hunk, whatever is ticked — the gesture people expect from a hunk header,
      // and the shortest path to `Selection::Hunks`. The same `hunkMarks` the checkbox
      // uses, so the two cannot disagree about what "this hunk" means.
      onClick={() => {
        if (staging === null || staging.diff === null) return
        staging.onApply(new Set(hunkMarks(staging.diff, hunkIndex)))
      }}
    >
      {op === 'stage' ? 'Stage hunk' : 'Unstage hunk'}
    </button>
  )

  /**
   * The slim per-hunk strip the non-sticky layouts draw: the whole-file unified view (the
   * staging affordances have nowhere else to live once the `@@` headers are gone) and both
   * arms of the split view. In the split, `which` keeps the controls in the left column
   * only — a box per column would be two checkboxes for one hunk — while the bar itself
   * renders in both so the columns keep equal heights across it. The fallback split (no
   * whole-file model) is the one place the `@@` text survives: there the line numbers jump
   * between hunks and the header is the only landmark saying by how much.
   */
  const hunkBar = (hunkIndex: number, which: 'old' | 'new' | 'both', key: string): ReactNode => {
    const withHeader = segments === null
    // The new side, following the per-line boxes across in M25. `which === 'both'` is the
    // unified row, which is one column and keeps them.
    const controls = selectable && which !== 'old'
    const shut = collapsed.has(hunkIndex)
    return (
      <div key={key} className={styles.hunkBar} data-audit="gitDiffHunkBar">
        {controls && hunkBox(hunkIndex)}
        {withHeader ? (
          <button
            type="button"
            className={styles.hunkTitle}
            aria-expanded={!shut}
            onClick={() => onCollapse(hunkIndex)}
          >
            <span className={styles.caret}>
              <Icon name={shut ? 'chevron-right' : 'chevron-down'} size={1} />
            </span>
            {diff.hunks[hunkIndex]?.header ?? ''}
          </button>
        ) : (
          <span className={styles.hunkFill} />
        )}
        {controls && op !== 'commit' && hunkActionButton(hunkIndex)}
      </div>
    )
  }

  /** A folded gap: one full-width row saying what is hidden, click to open. */
  const foldRow = (gap: number, count: number, key: string): ReactNode => (
    <button
      key={key}
      type="button"
      className={styles.fold}
      data-audit="gitDiffExpand"
      data-count={`${count}`}
      aria-label={`Expand ${count} unchanged lines`}
      disabled={onExpandGap === undefined}
      onClick={() => onExpandGap?.(gap)}
    >
      ⋯ {count} unchanged lines
    </button>
  )

  /**
   * One column of the split view. The insertion markers are interleaved between rows at the
   * boundaries `diffSync.insertMarkers` names — zero-height, so they cost the measured
   * anchors nothing — and `beforeRow` may equal the row count for an insertion at end of
   * file, which is why the last marker is looked up separately.
   */
  const column = (side: 'old' | 'new', rows: readonly ColumnRow[]): ReactNode => {
    if (split === null) return null
    const colName = side === 'old' ? 'left' : 'right'
    const tones = new Map<number, 'add' | 'del'>()
    for (const marker of insertMarkers(split.runs)) {
      if (marker.column === colName) tones.set(marker.beforeRow, marker.tone)
    }
    const markEl = (tone: 'add' | 'del', key: string): ReactNode => (
      <div
        key={key}
        className={styles.insertMark}
        data-audit="gitDiffInsertMark"
        data-tone={tone}
        aria-hidden="true"
      />
    )
    const children: ReactNode[] = []
    rows.forEach((row, index) => {
      const tone = tones.get(index)
      if (tone !== undefined) children.push(markEl(tone, `m${index}`))
      if (row.kind === 'line') {
        children.push(cell(`r${index}`, row.hunk, { line: row.line, at: row.at }, side, styles.half ?? ''))
      } else if (row.kind === 'bar') {
        children.push(hunkBar(row.hunk, side, `r${index}`))
      } else {
        children.push(foldRow(row.gap, row.count, `r${index}`))
      }
    })
    const end = tones.get(rows.length)
    if (end !== undefined) children.push(markEl(end, `m${rows.length}`))
    return (
      <div
        className={styles.column}
        data-side={side}
        // What the changes iterator writes `scrollTop` on. Marked rather than found by class,
        // because the unified body is a different element with the same job.
        data-audit="gitDiffScroller"
        // The new side only, matching `boxSide` in `cell()`. A grid places by order, so a track
        // reserved for a cell this column does not render would move every later cell across —
        // the failure the four track lists in the stylesheet were written for.
        data-boxed={selectable && side === 'new' ? 'true' : 'false'}
        data-blamed={side === 'new' && blame !== null ? 'true' : 'false'}
        ref={side === 'old' ? setLeftCol : setRightCol}
        // A React handler beside the sync effect's own `scroll` listener, rather than a third
        // job for that listener: the effect is torn down while the tab is parked, and the last
        // position before it was parked is exactly the thing that has to survive.
        onScroll={rememberScroll}
      >
        <div className={styles.columnContent}>{children}</div>
      </div>
    )
  }

  return (
    <div className={styles.pane}
        data-audit="gitDiffPane"
        ref={setRoot}
        // Settles the one tie `visible` cannot: a git diff tab in front and the log
        // tool window open on a revision diff are both mounted and both claim, and
        // without this the slot would keep whichever mounted last however long the
        // reader worked in the other. A pointer press is the cheapest true answer to
        // "which of them am I in".
        onPointerDown={() => navRef.current?.focus()}>
      {header}

      {note !== null && (
        <p className={styles.note} data-audit="gitDiffNote">
          {note}
        </p>
      )}
      {staging !== null && staging.diff !== null && !staging.diff.partialOk && (
        <p className={styles.note} data-audit="gitDiffWhole">
          This file can only be staged whole — it is binary, a submodule, a symlink, a deletion
          or a rename. Tick it in the panel instead.
        </p>
      )}
      {diff.textsOmitted && (
        <p className={styles.note} data-audit="gitDiffTruncated">
          This file is too large to show whole — showing the changes alone.
        </p>
      )}
      {staging !== null && held !== null && (
        <p className={styles.note} data-audit="gitDiffHeld">
          {held} line{held === 1 ? '' : 's'} of this file are held for the next commit.{' '}
          <button type="button" className={styles.link} onClick={staging.onDropHeld}>
            Commit the whole file instead
          </button>
        </p>
      )}

      {!visible ? (
        /*
         * **A tab nobody is looking at draws no rows.** (M38)
         *
         * `TabContent` keeps every tab of the active project mounted and hides all but one with
         * `visibility: hidden` — deliberately, and its own header says why — which means every
         * hidden tab is still *laid out at full size*. This pane has no virtualisation on
         * purpose (`diffRows.ts` argues for that: find-in-page, and a selection dragged across
         * hunks), and below `WHOLE_FILE_COLLAPSE_ABOVE` it draws every line of the file, twice
         * in the split layout. So a 2,000-line diff is on the order of ten thousand
         * `display: grid` rows with `pre-wrap` text, and six such tabs left sixty thousand of
         * them in the document being laid out for the rest of the session. That is the reported
         * *"several opened tabs with diff makes cide very laggy"*, and it is a cost the whole
         * window pays — layout is global, so the terminals and editors slow down with it.
         *
         * What is given up is that revealing a diff tab is a render rather than a repaint. It is
         * one tab's render, on a component that already re-renders whole every time its file
         * moves under it, against a permanent cost multiplied by tab count. The reading position
         * is the part that would genuinely be lost, so it is kept explicitly — see
         * `scrollMemo` and the layout effect that spends it.
         *
         * **The memos above are deliberately still computed.** `whole`, `segments`, `model` and
         * `anchors` are keyed on `diff`/`collapsed`/`expandedGaps`, none of which move for a
         * hidden tab, so they cost nothing per event — and `navImpl.cursor()` reads
         * `anchors.length`, so zeroing them would make a hidden pane lie to `changeNav` about
         * how many changes it has.
         */
        <div className={styles.parked} data-audit="gitDiffParked" />
      ) : layout === 'split' && split !== null ? (
        /*
         * Side by side: two independently scrolling columns, each drawing only its own
         * side's rows — no filler cells where the other side has a block, only a thin
         * `.insertMark` at the insertion point. The columns are kept in step by the sync
         * effect above; see its comment for why this replaced the one-grid layout, and
         * `diffRows.columnRows` for the row/run model both columns and the sync read.
         */
        <div className={styles.splitBody} data-audit="gitDiffSplit">
          {column('old', split.left)}
          {/*
            * The gutter between the columns, and the ribbons in it. (M25)
            *
            * A third grid track rather than an overlay, so it takes its own width out of the
            * layout once instead of covering two columns that then have to reserve space for
            * it. The paths are drawn by the effect above, which is also the only thing that
            * knows where either column is scrolled to.
            */}
          <div className={styles.connector} data-audit="gitDiffConnector" aria-hidden="true">
            <svg ref={setConnector} className={styles.connectorSvg} preserveAspectRatio="none" />
          </div>
          {column('new', split.right)}
        </div>
      ) : (
        <div
          className={styles.body}
          data-audit="gitDiffScroller"
          onScroll={rememberScroll}
        >
          {diff.hunks.length === 0 && (
            <p className={styles.notice}>No text changes on this side.</p>
          )}
          {segments !== null ? (
            /*
             * The whole file, unified. Hunk rows go through the same `cell()` at the same
             * `hunk:line` positions as ever; gap rows carry no position (`at: null`), so
             * nothing about the selection contract moved. No `@@` headers — the line
             * numbers are on screen — but the staging arm keeps a slim bar per hunk,
             * because the tri-state box and "Stage hunk" have nowhere else to live.
             *
             * `data-boxed`/`data-blamed` are the row grid's column list, stated once for
             * the file — same rule as the fallback below, which see.
             */
            <div
              className={styles.lines}
              data-audit="gitDiffWholeFile"
              data-boxed={selectable ? 'true' : 'false'}
              data-blamed={blame !== null ? 'true' : 'false'}
            >
              {segments.map((segment) => {
                if (segment.kind === 'gap') {
                  return (
                    <div
                      key={`g${segment.index}`}
                      data-audit="gitDiffGap"
                      data-count={`${segment.lines.length}`}
                    >
                      {segment.lines.map((line, index) =>
                        cell(`${index}`, -1, { line, at: null }, 'both', styles.row ?? ''),
                      )}
                    </div>
                  )
                }
                if (segment.kind === 'fold') {
                  return foldRow(segment.gap, segment.count, `f${segment.gap}`)
                }
                return (
                  <Fragment key={`h${segment.hunk.index}`}>
                    {selectable && hunkBar(segment.hunk.index, 'both', `hb${segment.hunk.index}`)}
                    {segment.hunk.lines.map((line, index) =>
                      cell(`${index}`, segment.hunk.index, { line, at: index }, 'both', styles.row ?? ''),
                    )}
                  </Fragment>
                )
              })}
            </div>
          ) : (
            diff.hunks.map((hunk) => {
              const shut = collapsed.has(hunk.index)
              return (
                <section key={hunk.index} className={styles.hunk}>
                  <div className={styles.hunkHeader}>
                    {selectable && hunkBox(hunk.index)}
                    <button
                      type="button"
                      className={styles.hunkTitle}
                      aria-expanded={!shut}
                      onClick={() => onCollapse(hunk.index)}
                    >
                      <span className={styles.caret}>
                        <Icon name={shut ? 'chevron-right' : 'chevron-down'} size={1} />
                      </span>
                      {hunk.header}
                    </button>
                    {selectable && op !== 'commit' && hunkActionButton(hunk.index)}
                  </div>

                  {/*
                    * `data-boxed` and `data-blamed` are the row grid's column list, stated once
                    * per hunk instead of once per row.
                    *
                    * A row's cells are placed by *order*, so a template with a track for a cell
                    * that was not rendered puts every later cell one column to the left. That is
                    * not hypothetical: the read-only arm draws no tick box, and until the blame
                    * column needed a fifth track nothing had noticed that its line numbers were
                    * sitting in the box's 16px and its text in the sign's 2ch. The stylesheet
                    * spells out all four combinations; the flags are what pick one.
                    */}
                  {!shut && (
                    <div
                      className={styles.lines}
                      data-boxed={selectable ? 'true' : 'false'}
                      data-blamed={blame !== null ? 'true' : 'false'}
                    >
                      {hunk.lines.map((line, index) =>
                        cell(`${index}`, hunk.index, { line, at: index }, 'both', styles.row ?? ''),
                      )}
                    </div>
                  )}
                </section>
              )
            })
          )}
        </div>
      )}

      {/*
        * The footer is the staging half and is withheld entirely on the revision arm.
        *
        * Not "drawn disabled with a reason", which is this application's usual treatment and is
        * wrong here: a greyed *Stage selection* under a diff of two 2019 commits implies the
        * gesture exists somewhere and is unavailable *now*, when in fact it does not apply to
        * this kind of document at all. The same argument the file tree makes for a group row
        * returning a one-item menu rather than fifteen greyed ones.
        */}
      {staging !== null && (
        <footer className={styles.actions}>
          <span className={styles.count} data-audit="gitDiffCount">
            {painted.size} of {totalChanges} line{totalChanges === 1 ? '' : 's'} selected
            {selection !== null && <span className={styles.kind}> · {selection.kind}</span>}
          </span>
          <button
            type="button"
            className={styles.button}
            disabled={!selectable || busy}
            onClick={() => staging.diff !== null && staging.onMarks(selectEverything(staging.diff))}
          >
            Select all
          </button>
          <button
            type="button"
            className={styles.button}
            disabled={painted.size === 0 || busy}
            onClick={() => staging.onMarks(new Set<string>())}
          >
            Clear
          </button>
          <button
            type="button"
            className={`${styles.button} ${styles.primary}`}
            data-audit="gitDiffApply"
            disabled={selection === null || busy}
            onClick={() => staging.onApply(marks)}
          >
            {actionLabel}
          </button>
        </footer>
      )}
    </div>
  )
}


// --- the wiring -------------------------------------------------------------------------

export interface GitDiffPaneProps {
  project: ProjectId
  /**
   * The tab's spec. A `git` or a `gitRevision` origin renders here; a Claude one belongs to
   * `DiffPane`, which is answering a blocked agent turn rather than showing a document.
   */
  spec: DiffSpec
  /**
   * Whether this tab is the one on screen — the flag `TabContent` hands `renderTree`.
   *
   * Optional, and **not** the pane's only source for it. Omitted, the pane works the same
   * answer out of the workspace mirror; see `diffTabs.diffTabOnScreen` for why it does not
   * simply assume "visible" instead. Passed, the host wins: a host that renders this pane
   * outside the tab stack knows something the mirror does not.
   */
  visible?: boolean
}

export function GitDiffPane({ project, spec, visible }: GitDiffPaneProps): ReactNode {
  if (spec.origin.kind === 'gitRevision') {
    /*
     * Two frozen revisions of one file. (M18)
     *
     * Keyed on all four of `(repo, path, new, old)` — the same key
     * `cmd::file::shows_revision_diff` opens the tab under, and for the same reason. A tab
     * retargeted from one comparison to another is showing a *different document*, and a key
     * that left the sides out would carry the previous pair's fetched hunks and collapsed set
     * into it. `JSON.stringify` because a `RevSide` is a tagged object, not a string.
     */
    const { repo, path, new: next, old: prev } = spec.origin
    return (
      <RevisionDiffPane
        key={`${repo} ${path} ${JSON.stringify(next)} ${JSON.stringify(prev)}`}
        project={project}
        repo={repo}
        path={path}
        next={next}
        prev={prev}
        {...(visible === undefined ? {} : { visible })}
      />
    )
  }
  if (spec.origin.kind !== 'git') {
    // Not reachable through `tab_open_diff`, which only ever writes a git origin. Said out
    // loud rather than rendered blank so a mis-wired host sees why nothing is here.
    return <div className={styles.notice}>This tab is not a git diff.</div>
  }
  const { repo, path, side } = spec.origin
  // Keyed on the file, so a tab that comes to show a different diff re-derives everything
  // rather than painting the previous file's selection against the new file's line numbers.
  return (
    <GitDiff
      key={`${repo} ${path}`}
      project={project}
      repo={repo}
      path={path}
      from={side}
      visible={visible}
    />
  )
}

/**
 * Both sides of a diff, tokenized, once per (path, texts).
 *
 * Takes a [`DrawnDiff`] so the working-tree pane and the revision pane share one implementation,
 * which is the whole reason that interface exists.
 *
 * # Why the tokenizer arrives through `import()`
 *
 * `panes/diffHighlight.ts` reaches `editor/languages.ts` and `editor/markdown/fenceTokens.ts`,
 * and through them `@codemirror/language` and `@lezer/highlight`. `check-diff-render.mjs`
 * SSR-bundles `GitDiffView` and runs it under node to prove that what this pane highlights is
 * what it stages, and `editor/diffViewMode.ts` records at length why that bundle is kept free of
 * the editor's dependencies. Those packages do import cleanly under node today — `check:markdown`
 * already runs `fenceTokens.ts` there — so this is not a fix for a crash; it is the difference
 * between a property that holds and one that is *asserted* to hold, and the check now greps the
 * emitted bundle rather than trusting this comment. It also keeps the grammar chunks out of the
 * download until somebody opens a diff.
 *
 * # Why the previous answer is not cleared first
 *
 * A refetch — an agent writing the file, a `nonce` bump from an unrelated path — replaces `diff`
 * several times a second. Clearing to `null` on each one would blink the whole diff to plain.
 * Keeping the old tokens is safe because `lineTokens` checks every row's content against the line
 * its tokens spell: a row the previous text no longer describes draws plain, and one it still
 * describes draws runs that are, by construction, that exact string's.
 */
export function useDiffTokens(diff: DrawnDiff | null): DiffTokens | null {
  const [tokens, setTokens] = useState<DiffTokens | null>(null)
  const path = diff?.path ?? null
  const oldPath = diff?.oldPath ?? null
  const oldText = diff?.oldText ?? null
  const newText = diff?.newText ?? null

  useEffect(() => {
    if (path === null || (oldText === null && newText === null)) return
    let disposed = false
    void import('./diffHighlight')
      .then((mod) => mod.diffTokens(path, oldPath, oldText, newText))
      .then((built) => {
        if (!disposed) setTokens(built)
      })
      .catch((e: unknown) => {
        // Colour is the one thing in this pane nothing else depends on, so a chunk that will not
        // load costs the colour and nothing else — the same trade `loadGrammar` makes for a
        // grammar. Logged rather than swallowed, because "the diff is never coloured" is
        // otherwise an absence with no symptom to search for.
        void diag.log(`git diff pane: highlighting ${path} failed: ${String(e)}`)
      })
    return () => {
      disposed = true
    }
    // The texts, not the `diff` object: `Object.is` on a string is a value comparison, so a
    // refetch that returned the same bytes re-tokenizes nothing. `oldPath` is in the key because
    // a rename can change the extension, and the old side is then a different language.
  }, [path, oldPath, oldText, newText])

  return tokens
}

interface GitDiffProps {
  project: ProjectId
  repo: RepoId
  path: string
  /** The side the tab was opened on. The pane owns it from here. */
  from: DiffSide
  /** The host's answer, when it has one. `undefined` means "work it out". */
  visible: boolean | undefined
}

/*
 * Memoised, and this is the boundary that decides whether N open diff tabs cost N. (M38)
 *
 * `App.tsx`'s `WorkspaceContent` is the only memo above here, and its props are snapshot-derived
 * — so it re-renders exactly once per *accepted workspace mutation*, which is the right answer
 * for the pane grid and the wrong one for this. Activating a tab, focusing a pane, releasing a
 * splitter and binding a session are all mutations, and each of them re-reconciled every mounted
 * diff tab's entire row list: tens of thousands of elements rebuilt to produce identical output,
 * several times a second while somebody clicks around.
 *
 * The memo *holds* because every prop here is a primitive — `GitDiffPane` destructures
 * `spec.origin` above precisely so that the object off the workspace snapshot, whose identity
 * changes on every broadcast, stops at that boundary. Keep it that way: a prop added here that is
 * not a string, number or boolean silently reopens this.
 */
const GitDiff = memo(function GitDiff({
  project,
  repo,
  path,
  from,
  visible: told,
}: GitDiffProps): ReactNode {
  /**
   * Whether the tab drawing this diff is the one in front, worked out rather than told.
   *
   * `told` wins when a host passes it — it knows things the workspace does not, such as
   * rendering this pane outside the tab stack — but it is not the only source, because a
   * deferral that has to be switched on is a deferral that is off: the shell renders
   * `<GitDiffPane project spec />` with no flag, and every tab claiming to be visible makes
   * the whole hidden-tab path below dead code that reads as if it were working.
   *
   * Starts `true` and is corrected by the first `cide://workspace-changed`. That direction is
   * the safe one — a tab wrongly believed visible costs a redundant `git_diff_file`, one
   * wrongly believed hidden stops following its file — and activating a tab *is* a workspace
   * mutation, so the first tab switch in the window settles it for every diff tab at once.
   */
  const [onScreen, setOnScreen] = useState(true)
  const visible = told ?? onScreen
  const [side, setSide] = useState<DiffSide>(from)
  const [diff, setDiff] = useState<FileDiff | null>(null)
  const [reason, setReason] = useState<string | null>(null)
  const [marks, setMarks] = useState<Marks>(() => new Set<string>())
  const [collapsed, setCollapsed] = useState<ReadonlySet<number>>(() => new Set<number>())
  /** Gaps opened in a folded whole-file view. Cleared with the marks: same staleness rule. */
  const [expandedGaps, setExpandedGaps] = useState<ReadonlySet<number>>(() => new Set<number>())
  /** Which change the iterator is on, or `-1` for none yet. See `GitDiffViewCommon`. */
  const [currentChange, setCurrentChange] = useState(-1)
  const [note, setNote] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  /** Bumped to re-run the fetch; a git mutation anywhere invalidates this view. */
  const [nonce, setNonce] = useState(0)
  /** Something invalidated this view while the tab was behind another one. See below. */
  const [stale, setStale] = useState(false)

  // Read through refs inside the fetch so a re-render caused by a tick cannot re-run it, and
  // so the staleness check can see the marks without depending on them.
  const marksRef = useRef<Marks>(marks)
  marksRef.current = marks
  const revRef = useRef<string | null>(null)
  // The invalidation listeners below are subscribed once per file and must not be torn down
  // and rebuilt every time the user switches tabs, so visibility reaches them through a ref.
  const visibleRef = useRef(visible)
  visibleRef.current = visible

  /*
   * Unified or split, from `Settings.editor.diffView`.
   *
   * Read through `@/editor/diffViewMode` rather than `@/settings/useSettings` — that hook
   * reaches `@/store/workspace`, which reaches xterm, and this pane is SSR-rendered under node
   * by `check-diff-render.mjs`. Same trade as the `onWorkspaceChanged` subscription below, and
   * the module says so at length.
   */
  const diffView = useSyncExternalStore(subscribeDiffView, getDiffView, getServerDiffView)

  const partials = useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot)
  const held = useMemo(
    () => partials.find((p) => p.repo === repo && p.path === path)?.lines ?? null,
    [partials, repo, path],
  )

  /*
   * The blame column. (M18)
   *
   * # Why this is per-pane state and not `editor/blameStore`
   *
   * That store is the right home for the *editor's* column and the wrong one for this, on two
   * counts that are both about its key. It is `(project, absolute path)` — `toggleBlame` resolves
   * the repository with `git_locate`, which needs an absolute path — and a diff tab holds a
   * repo-relative path beside a `RepoId`, because that is how `DiffOrigin::Git` spells it and how
   * `git_diff_file` wants it. Rebuilding the absolute form means `repoRoots`, whose own header
   * records that the map is *empty* until some `ChangesTree` has arrived: a window restored with
   * the sidebar on Files has never seen one, so the Blame button would work or not depending on
   * whether the git panel had been opened this session. That is the "works by the route you
   * tested, fails by the route you did not" shape.
   *
   * The second count is worse and applies even with the root in hand. `RevisionDiffPane` blames
   * at `new_rev`, and the store's key has no room for a revision — so one file annotated at HEAD
   * in an editor and at `a1b2c3d` in a revision tab would be one entry with two meanings, and
   * whichever fetch landed last would answer for both. In a column whose entire claim is *this
   * commit wrote this line*, that is a per-line falsehood rather than a stale view.
   *
   * So the pane owns it, both arms the same way, and pays a `git_blame` per tab that asks. What
   * is shared with the editor is the part that must not fork: `collapseRuns` decides which
   * answers are drawable and what a line's age tint is — see {@link blameLookup}.
   */
  const [blameOn, setBlameOn] = useState(false)
  const [blameFile, setBlameFile] = useState<BlameFile | null>(null)
  /** `null` when this side's new text can be blamed at all. See `diffBlame.blameRefusal`. */
  const blameRefused = blameRefusal(side)

  useEffect(() => {
    if (!blameOn || blameRefused !== null) {
      // Off, or on a side that cannot be answered. Dropped rather than kept: a megabyte of runs
      // for a column nobody is looking at buys one fast re-open and costs memory for the rest of
      // the session, which is the trade `blameStore.toggleBlame` makes the same way.
      setBlameFile(null)
      return
    }
    let disposed = false
    /*
     * No `contents`. The editor sends its buffer when the tab is dirty because the gutter sits
     * beside live text; here the document being annotated is the diff's new side, which for both
     * blamable sides *is* the working file — the same bytes `git_diff_file` just read. Sending a
     * buffer this pane does not have would be inventing one.
     */
    historyApi
      .blame(project, repo, path)
      .then((fresh) => {
        if (!disposed) setBlameFile(fresh)
      })
      .catch((e: unknown) => {
        if (disposed) return
        setBlameFile(null)
        // A `GitError` is `{kind, detail}`, so `String(e)` is `[object Object]` — the bug
        // `check:branches` exists for. The ordinary refusals here are real answers rather than
        // faults: a file added in this diff is `NotTracked` at HEAD, and a generated one is
        // `FileTooLarge`. The toggle stays pressed, as the editor's does, so the note explains a
        // state the user asked for instead of a button that undid itself.
        //
        // This *knew* about the tagged shape and still printed `[object Object]`, which is why
        // it is now `explain` rather than a second hand-rolled unwrap: it reached for `detail`
        // and stringified it, and `detail` is itself an object — `{path}` for `notTracked`,
        // `{path, bytes, limit}` for `fileTooLarge`. Both of those are exactly the cases this
        // comment says are the common ones, so the one refusal a reader was likely to meet was
        // the one it could not describe.
        const detail = explain(e)
        setNote(`No blame for this file — ${detail}`)
        void diag.log(`git diff pane: blame ${path} failed: ${detail}`)
      })
    return () => {
      disposed = true
    }
    // `nonce` is in the list on purpose. It is what the diff itself re-reads on, and a column
    // beside a diff that moved under it is the per-line falsehood this whole feature has to
    // avoid — an agent's edit shifts the new side's line numbers, and the old runs still cover
    // every one of them. The cost is a `git blame` per invalidation, which is why the column is
    // off until asked for.
  }, [blameOn, blameRefused, project, repo, path, nonce])

  /*
   * Sampled **once** per answer and passed in, so every line of one paint is bucketed against one
   * instant — `collapseRuns` says why at length. Seconds, because that is the unit the whole
   * blame wire is in.
   */
  const blame = useMemo(
    () => (blameFile === null ? null : blameLookup(blameFile, Math.floor(Date.now() / 1000))),
    [blameFile],
  )
  const tokens = useDiffTokens(diff)

  /**
   * The last refusal written to the log, so a diff that is gone writes one line and not one a
   * second. See the `catch` below.
   */
  const loggedRef = useRef<string | null>(null)

  useEffect(() => {
    /*
     * **A tab nobody is looking at does not fetch, not even its first time.** (M38)
     *
     * The deferral below covers every *re*-fetch, and this covers the one it could not: the
     * mount. A workspace restored with six diff tabs opened six `git_diff_file` calls at once,
     * each of which pays `repo::find` — see its own comment for what that used to cost — and
     * brings back both whole file texts, which are then tokenized whole on this thread. Five of
     * those six were for tabs behind the one in front.
     *
     * `visible` is deliberately **not** in the dependency list: it moves on every tab switch,
     * and a dependency on it would refetch a diff nobody asked to refresh every time it came
     * back. The reveal effect below is the resume path — it bumps `nonce`, which is in the list
     * — so the fetch happens exactly once, when the tab is first looked at.
     */
    if (!visible) {
      setStale(true)
      return
    }
    let disposed = false
    gitApi
      .diffFile(project, repo, path, side)
      .then((fresh) => {
        if (disposed) return
        setDiff(fresh)
        setReason(null)
        // Re-arms the log below: a file that goes, comes back and goes again is two events and
        // deserves two lines. Only a refusal repeating itself is silenced.
        loggedRef.current = null
        /*
         * A moved diff drops the selection.
         *
         * The positions in `marks` index *this* diff. When the file changes underneath —
         * Claude edits it, a build regenerates it, the user saves — the new diff has
         * different hunks and the same indices name different lines. Carrying them over is
         * precisely the silent wrong answer `FileDiff.rev` exists to catch, so they go, and
         * the pane says so. Re-deriving them by content matching would be a guess on the one
         * code path where a guess destroys work.
         */
        if (revRef.current !== null && revRef.current !== fresh.rev) {
          if (marksRef.current.size > 0) {
            setNote('The file changed while it was open, so the selection was cleared.')
          }
          setMarks(new Set<string>())
          // The gap indices are positions in the previous reconstruction; a moved file has
          // different gaps, and index 3 of the new set is not what the user opened.
          setExpandedGaps(new Set<number>())
          // And the walk, for the same reason: change 3 of the new diff is not the edit the
          // reader was looking at. Back to "nothing current", not to the first change — a
          // scroll nobody asked for is worse than a stepper that starts again.
          setCurrentChange(-1)
        }
        /*
         * A diff **opens on its first change**, and only on the way in. (M31)
         *
         * > *"when opening diff - it should point to line where first diff exists, currently
         * > diff always starts from 1 line."*
         *
         * Nothing scrolled before, by omission rather than by decision: `currentChange` starts
         * at `-1` ("nothing walked yet"), and the scroll effect in the view bails on a null
         * anchor, so every diff opened at the top of the file and the reader either scrolled
         * or pressed the stepper to get to the point. `changeAnchors` skips `shared` runs, so
         * `anchors[0]` *is* the first real difference, and the effect already centres it.
         *
         * `revRef.current === null` is the discriminator and it is the exact one: this is a
         * first fetch for this pane, so there is no reading position to preserve. That is what
         * separates it from the refetch a few lines up, whose comment argues — correctly, and
         * this must not undo it — that moving the reader after the file changed underneath
         * them is worse than leaving the stepper where it was. Opening and being interrupted
         * are opposite cases and they now get opposite answers.
         *
         * Safe when there is nothing to jump to: the view clamps with
         * `Math.min(currentChange, anchors.length - 1)`, so a diff with no anchors clamps
         * straight back to `-1` and nothing scrolls.
         */
        if (revRef.current === null) setCurrentChange(0)
        /*
         * Whether this answer is a *different diff* from the one already on screen. (M31)
         *
         * Computed before `revRef` is advanced, and it is the condition the collapse below
         * needs. Most fetches are not a new diff at all: `bump` re-reads on every git mutation
         * in the project, so staging a neighbouring file, or committing one, re-fetches this
         * pane and gets back the identical bytes.
         */
        const moved = revRef.current !== fresh.rev
        revRef.current = fresh.rev
        /*
         * The collapse-by-default guard is the *fallback's*; the whole-file view bounds its
         * rows by folding gaps instead, and pre-collapsed hunks there would fight it.
         *
         * Gated on `moved`, which it was not before, and the ungated version had two faults
         * that shared one cause — it ran on *every* fetch, including the many that bring back
         * a byte-identical diff:
         *
         *   * **It re-collapsed hunks the reader had opened.** On any diff past
         *     `COLLAPSE_ABOVE`, expanding a hunk and then staging anything else in the project
         *     folded it up again, with no gesture in between that could explain it.
         *   * **It cost a full re-measure each time.** `new Set(...)` is a fresh identity even
         *     when it holds the same numbers, so `collapsed` changed, `model` was recomputed,
         *     and the three geometry effects re-ran — each walking every row in both columns
         *     reading `offsetTop`, which is a forced synchronous layout over thousands of
         *     nodes. That is a real part of *"cide starts to lag"* while committing with a
         *     large diff open, and nothing about it was visible on screen.
         *
         * A diff that genuinely moved still collapses, which is the behaviour this is for: the
         * hunk indices are new, so what the reader had opened is not addressable any more —
         * the same argument `expandedGaps` is cleared under, a few lines up.
         */
        if (moved && wholeFileSegments(fresh.hunks, fresh.newText) === null) {
          const rows = fresh.hunks.reduce((n, hunk) => n + hunk.lines.length, 0)
          if (rows > COLLAPSE_ABOVE) setCollapsed(new Set(fresh.hunks.map((h) => h.index)))
        }
      })
      .catch((e: unknown) => {
        if (disposed) return
        /*
         * The refusal, as a sentence. (M31)
         *
         * This read `e instanceof Error ? e.message : String(e)` and printed **`[object
         * Object]`** on the commonest path there is. A `GitError` is a tagged enum on the wire
         * — `#[serde(tag = "kind", content = "detail")]` — so a rejection is a plain object
         * with no `message`, and `String()` of one says nothing at all. `client.ts` names this
         * exact hazard and `chrome/branchModel.ts::explain` is the sanctioned answer; this file
         * had simply never used it. What the reader saw after committing a file whose diff was
         * open was a notice headed *"Nothing to show on this side."* over `[object Object]`,
         * and the `diag.log` line beside it recorded the same non-answer.
         *
         * `noSuchChange` is then special-cased, because `explain`'s wording for it is written
         * for the *staging* caller — "it changed again while that was in flight. Refresh and
         * try once more" — which is a sentence about a race, and describes a failure. Here it
         * is not a failure and there is nothing to retry: the file genuinely has nothing left
         * on this side, and by far the likeliest reason is that the reader just committed it.
         * The pane knows which of the two it is and `explain` cannot, so the context supplies
         * the sentence and the tag stays shared.
         */
        const detail =
          kindOf(e) === 'noSuchChange'
            ? `${path} has no changes on this side any more — it may have just been committed.`
            : explain(e)
        // The ordinary end of a diff's life: the change was committed, reverted or staged
        // away. Not a dialog — the side simply has nothing to show, and the switcher above is
        // still there to look at another one.
        setDiff(null)
        setReason(detail)
        /*
         * **Once per distinct refusal, not once per fetch.** (M38)
         *
         * `diag_log` is a *non-async* `#[tauri::command]`, so every one of these is a
         * main-thread round trip and a file append — `ipc/consoleBridge.ts` states the rule it
         * follows from: a diagnostic for a lag report must not itself be a cause of lag. And a
         * gone diff is the shape that breaks it, because it fails *for ever*: the tab keeps its
         * subscriptions, so it refetches on every `cide://git-status` in the project and logs
         * the same sentence again, for the life of the tab, times however many such tabs are
         * open. That is exactly the "several diff tabs, especially ones whose diff is gone"
         * report.
         *
         * The ref and not `reason`, because `reason` is also what the notice draws and a
         * comparison there would couple the two: the line is about what *changed*, and the
         * notice is about what *is*.
         */
        if (loggedRef.current !== detail) {
          loggedRef.current = detail
          void diag.log(`git diff pane: ${path} on ${side} failed: ${detail}`)
        }
      })
    return () => {
      disposed = true
    }
    // `visible` is read and deliberately not depended on; see the head of the effect. Listing it
    // would be two bugs at once: hiding a tab would mark it stale (so revealing it would re-read
    // a diff nothing had touched), and revealing one would fetch twice — once for the dependency
    // and once for the `nonce` the reveal effect below bumps.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project, repo, path, side, nonce])

  /*
   * What invalidates this view, and what does not.
   *
   * A mutation cide made anywhere invalidates it — including one made in the panel beside it,
   * or in another window. Tool events cover the case with no cide mutation at all: an agent
   * writing the file this tab is showing.
   *
   * # Both filters are load-bearing
   *
   * `cide://session-tool` is emitted with `AppHandle::emit`, which reaches **every window**,
   * and it names a session rather than a project. So this handler hears every tool call every
   * Claude in the app makes, in any project. `TabContent` keeps every tab of the project
   * mounted at once — that is what makes switching tabs free — so an unfiltered bump meant N
   * open diff tabs each issuing a `git_diff_file` on every tool call anywhere, and all but at
   * most one of those tabs was behind another one.
   *
   * The event names the paths it touched, which is the filter: a tool call is only this tab's
   * business if it wrote *this tab's file*. Project scoping falls out of the same comparison
   * rather than needing a `project` field on the event, because an absolute path under another
   * project's root is not this file — see `repoRoots.touchesFile`.
   *
   * `cide://git-status` already carries a project and stays filtered on it alone. It is not
   * narrowed to the path as well: it fires once per cide mutation rather than once per tool
   * call, so it is not the storm, and staging a *neighbouring* file does move this view — the
   * `held` line and the side switcher both describe an index that just changed.
   *
   * # Hidden tabs defer rather than skip
   *
   * A hidden tab records that it is stale and fetches when it is next revealed, once, however
   * many events went by. It deliberately does not refetch on *every* reveal: the whole reason
   * `TabContent` keeps tabs mounted is that coming back to one is instant, and a tab that
   * re-reads a 4,000-line diff every time it is looked at would trade the storm for a stutter
   * on a gesture people make constantly. So the cost is paid exactly when the file it is
   * showing actually moved, and a tab nobody touched draws from the state it already has.
   *
   * "Hidden" is `onScreen` above, which follows `cide://workspace-changed` rather than waiting
   * to be told. That is not tidiness: with the answer coming only from a prop, a host that
   * renders `<GitDiffPane project spec />` — which is what the shell does today — leaves every
   * tab claiming to be visible, so this whole section is dead and the only filter that ever
   * fired was the path one above. A deferral that has to be switched on is a deferral that is
   * off.
   *
   * The one thing this gives up is a hidden tab that is *watched* rather than looked at —
   * split off into its own window, say. Not reachable today, and it is worth being exact
   * about why, because `onScreen` reads the project's `activeTab` and a second window has an
   * active tab of its own that the workspace does not record: a `pane:` window renders
   * `DetachedPaneWindow`, never a tab, and `App` draws diff tabs only from the shell's
   * `TabContent`. A host that does put this pane in a second window has to pass `visible`,
   * which is the case that prop is kept for.
   */
  useEffect(() => {
    let gone = false
    let timer: number | null = null
    const unlisten: Array<() => void> = []
    const bump = () => {
      if (timer !== null) window.clearTimeout(timer)
      // One edit reports several paths and a `git_status` broadcast follows every mutation, so
      // the triggers arrive together; coalescing them is one fetch instead of three. Which of
      // the two this becomes is decided when the timer fires, not when it is set: a tab
      // revealed inside the window fetches straight away rather than waiting to be revealed
      // again.
      timer = window.setTimeout(() => {
        if (visibleRef.current) setNonce((n) => n + 1)
        else setStale(true)
      }, 120)
    }
    const track = (p: Promise<() => void>) => {
      void p
        .then((fn) => {
          if (gone) fn()
          else unlisten.push(fn)
        })
        .catch((e: unknown) => diag.log(`git diff pane: events unavailable: ${String(e)}`))
    }
    track(
      events.onGitStatus((forProject, tree) => {
        // Noted whoever it is for: the roots in another project's tree are still true, and a
        // tab in this window may be showing one of those repos after a project switch.
        noteRepoRoots(tree)
        if (forProject === project) bump()
      }),
    )
    track(
      events.onSessionTool((_session, paths) => {
        if (touchesFile(paths, repoRoot(repo), path)) bump()
      }),
    )
    /*
     * Git moving underneath us, from outside cide. (M18)
     *
     * `cide://git-status` is emitted by cide's **own** mutations and by nothing else, so before
     * this a `git commit`, `git checkout` or `git stash` typed into a bash pane left this diff —
     * and its blame column — showing the previous HEAD, indefinitely. The log and the editor's
     * blame gutter both read `FsChange.git` and refresh; this pane was the one surface left that
     * did not, which made it the odd one out rather than merely stale.
     *
     * `gitRefsMoved` is the same predicate the log uses, imported rather than re-derived: an
     * index-only burst — a `git add`, or a `git status` in a loop rewriting `.git/index` — moves
     * no ref and must not re-walk anything. It is shared precisely so the two surfaces cannot
     * come to different conclusions about what "git changed" means.
     *
     * No second debounce: `bump` is already throttled at 120 ms above, and `cide-fs`'s coalescer
     * has held the burst until the tree went quiet before it ever reaches here.
     */
    track(
      events.onFsChanged((forProject, change) => {
        if (forProject === project && gitRefsMoved(change)) bump()
      }),
    )
    /*
     * Which tab is in front is workspace state, so it arrives here the way every other piece of
     * workspace state does. Subscribed beside the other two rather than through the store hook
     * that already follows this event: `store/workspace` reaches xterm through `paneHosts`, and
     * importing it here would put a terminal in the SSR bundle `check-diff-render.mjs` renders
     * this pane's view from.
     *
     * **Only when nobody told us.** (M38) The shell passes `visible` now, so for every tab in
     * the app this subscription would be a listener whose handler runs `diffTabOnScreen` — a
     * filter over the project's whole tab list — once per pane per accepted mutation, to reach
     * an answer the prop has already given. The mirror stays for hosts that pass nothing, which
     * is the case `diffTabs.ts`' header is written about; it is not the road anything takes
     * today, and it must not cost anything when it is not taken.
     */
    if (told === undefined) {
      track(
        events.onWorkspaceChanged((ws) => {
          setOnScreen(diffTabOnScreen(ws.projects[project], repo, path))
        }),
      )
    }
    return () => {
      gone = true
      if (timer !== null) window.clearTimeout(timer)
      for (const fn of unlisten) fn()
    }
    // `told === undefined` and not `told`: whether the mirror is consulted is a fact about the
    // *host*, fixed for the life of this pane, while `told` itself moves on every tab switch and
    // would tear down and rebuild all four listeners each time.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project, repo, path, told === undefined])

  // Reveal is where a deferred fetch is spent. The flag is cleared by the reveal and not by
  // the fetch's outcome: a file staged away answers `NoSuchChange` for as long as it stays
  // that way, and a flag that only cleared on success would re-arm every render into a loop.
  useEffect(() => {
    if (!visible || !stale) return
    setStale(false)
    setNonce((n) => n + 1)
  }, [visible, stale])

  const switchSide = useCallback((next: DiffSide) => {
    // Positions do not transfer between sides; see the module comment.
    setMarks(new Set<string>())
    setNote(null)
    // Clearing this also re-arms the open-on-first-change seed in the fetch above, and that is
    // wanted rather than incidental: picking the other side is a request to read a diff the
    // reader has not seen, so it lands on its first change exactly as opening one does. The
    // stepper's position would not transfer anyway — the two sides have different anchors.
    revRef.current = null
    setSide(next)
  }, [])

  const run = useCallback(async (what: string, call: () => Promise<unknown>) => {
    setBusy(true)
    try {
      await call()
      setNote(null)
      setMarks(new Set<string>())
      setNonce((n) => n + 1)
    } catch (e: unknown) {
      // `explain`, not `String(e)`: every refusal here is a tagged `GitError`, so the old
      // spelling turned the two sentences below into `[object Object]` — and these are the two
      // that most need reading, because they are about work that was *not* applied.
      const detail = explain(e)
      // Every refusal lands here, and the two that matter read plainly: `staleSelection`
      // means the file moved and *nothing was applied*; `partialRefused` means this file can
      // only go through the index whole.
      setNote(`${what} refused — ${detail}`)
      void diag.log(`git diff pane: ${what} failed: ${detail}`)
    } finally {
      setBusy(false)
    }
  }, [])

  const apply = useCallback(
    (chosen: Marks) => {
      if (diff === null) return
      const selection = pathSelection(chosen, diff)
      if (selection === null) return
      if (opFor(side) === 'stage') {
        void run('stage', () => gitApi.stage(project, repo, [selection]))
        return
      }
      if (opFor(side) === 'unstage') {
        void run('unstage', () => gitApi.unstage(project, repo, [selection]))
        return
      }
      /*
       * Commit does not happen here. The panel owns the message box and the changelist, so
       * this hands the selection over and the next Commit uses it — see `partialStore`.
       *
       * "Everything" is stored as *nothing*: a `whole` selection is what a ticked row already
       * means, and an entry for it would put the file in the panel's "will be committed in
       * part" warning while committing all of it — a warning that is false is worse than no
       * warning. It would also attach a rev to a selection that names no positions, so a
       * later unrelated edit would have the commit refused for no reason at all.
       */
      if (selection.selection.kind === 'whole') {
        clearPartial(repo, diff.path)
        setNote('The whole file will be committed; nothing is held back.')
        return
      }
      setPartial({
        repo,
        path: diff.path,
        side: diff.side,
        rev: diff.rev,
        selection: selection.selection,
        lines: countLines(diff, selection.selection),
      })
      setNote(null)
    },
    [diff, side, project, repo, run],
  )

  return (
    <GitDiffView
      diff={diff}
      path={path}
      side={side}
      marks={marks}
      collapsed={collapsed}
      busy={busy}
      note={note}
      reason={reason}
      held={held}
      view={diffView.view}
      // Disabled until the first `settings.get` lands: a patch built before the real
      // `EditorSettings` are known would send this module's guesses for `fontSize` and the
      // rest. It is one round trip at window start.
      {...(diffView.writable ? { onView: setDiffView } : {})}
      // Absent when off, `null` while the answer is in flight or refused. Spelling "off" as an
      // absent prop rather than as `blame={null}` is what lets the view tell the two apart with
      // no third prop — see `GitDiffViewCommon.blame`.
      tokens={tokens}
      currentChange={currentChange}
      onCurrentChange={setCurrentChange}
      visible={visible}
      {...(blameOn ? { blame } : {})}
      // Withheld on the staged side, which is the same rule the view's disabled title states.
      {...(blameRefused === null ? { onBlame: setBlameOn } : {})}
      onSide={switchSide}
      onMarks={setMarks}
      onCollapse={(hunk) =>
        setCollapsed((prev) => {
          const next = new Set(prev)
          if (!next.delete(hunk)) next.add(hunk)
          return next
        })
      }
      expandedGaps={expandedGaps}
      onExpandGap={(gap) => setExpandedGaps((prev) => new Set(prev).add(gap))}
      onApply={apply}
      onDropHeld={() => clearPartial(repo, path)}
    />
  )
})

// --- the revision diff (M18) --------------------------------------------------------------

/**
 * How one resolved side is spelled in the header.
 *
 * Seven characters of the oid, the same cut `cmd::file::rev_label` makes for the tab title, so
 * a tab reading `main.rs @ a1b2c3d` and a header reading `9f8e7d6 → a1b2c3d` name the same
 * commit in the same alphabet. The two non-commit sides get a word rather than a short oid: a
 * `RevSide::WorkingTree` has no commit at all, and printing the oid it happens to sit on would
 * claim the diff is against a commit when it is against whatever is on disk this second.
 *
 * The **resolved** oid is preferred over the requested side, which is the whole reason
 * `RevisionDiff` echoes `new_oid`/`old_oid` back: a `FirstParent` request answers with the
 * parent it actually found, and a header reading `parent → a1b2c3d` would be unquotable in a
 * bug report and unusable in a `git show`.
 */
function revLabel(side: RevSide, oid: string | null): string | null {
  if (oid !== null) return oid.slice(0, 7)
  if (side.kind === 'workingTree') return 'working tree'
  // No commit on this side and it is not the working tree: the file did not exist there. The
  // header draws this as "added in …" rather than as a pair — see `GitDiffReadOnlyProps`.
  return null
}

export interface RevisionDiffPaneProps {
  project: ProjectId
  repo: RepoId
  path: string
  next: RevSide
  prev: RevSide
  /**
   * Whether this diff is the one in front, for the changes iterator's slot.
   *
   * Defaults to `true`, which is right for the git tool window: `ToolWindowHost` mounts only the
   * active history tab, so a mounted one is by definition the one being looked at. The tab route
   * through `GitDiffPane` passes the host's own answer.
   */
  visible?: boolean
}

/**
 * The wiring for a read-only revision diff.
 *
 * **Exported since M21**, because the git tool window's History tab needs exactly this and
 * nothing else: one file, one pair of revisions, read-only, with the unified/split toggle. That
 * tab was opened by *Show history for this file* and then drew the whole commit's file list
 * beside it — every path the commit touched, when the reader had already named the one they
 * cared about. Reusing this rather than building a second one is what keeps the layout toggle,
 * the blame column and the `oldPath` rename handling from existing twice.
 *
 * Its own component rather than a branch inside {@link GitDiff}, and the split is the same one
 * the view's union makes: that component holds a `side`, a `Marks` set, a `partialStore`
 * subscription, a `rev` guard and four staging callbacks, and every one of them is meaningless
 * here. Folding the two together would mean nine `if (readOnly)` branches through a hook body,
 * which is how a stage call ends up reachable from a document that cannot be staged.
 *
 * # What it deliberately does *not* do
 *
 * **No `git-status` subscription for a frozen pair.** `GitDiff` spends fifty lines on
 * invalidation because a working-tree diff goes stale constantly — an agent writes the file, a
 * build regenerates it, the user saves. Two commits diff to the same bytes for ever, so a tab
 * showing one is correct from its first frame until it is closed, and a refetch would be a round
 * trip that cannot change a pixel.
 *
 * The exception is a [`RevSide::WorkingTree`] side, which is exactly why that is a variant a
 * caller can match on rather than a magic oid. When one is present the same two events are
 * subscribed, with the same 120 ms coalescing, because then the diff *is* moving.
 *
 * # And when it is moving, it defers exactly as `GitDiff` does (M38)
 *
 * This paragraph used to say there was no visibility deferral here either, and the omission was
 * not free. `git_diff_revision` builds a diff of the **whole repository** and then filters to
 * one path (`cide_git::revision::build_diff` sets no pathspec, and runs rename detection over
 * every delta), and with a working-tree side it recurses untracked directories on the way. So a
 * commit-against-working-tree tab sitting behind another one re-ran all of that on every git
 * mutation in the project, for nothing. The machinery is `GitDiff`'s, restated rather than
 * shared because the two panes hold different state: a `visibleRef` so the listeners are not
 * rebuilt on every tab switch, a `stale` flag so however many events go by cost one fetch, and
 * `diffTabs.revisionTabOnScreen` so the answer does not depend on a host remembering to pass a
 * prop.
 */
/**
 * The `memo` comparator, because two of this pane's props are objects.
 *
 * `next`/`prev` are `RevSide`s off the tab's spec, so they are fresh literals on every workspace
 * broadcast and the default shallow compare would never hold — which is the whole cost the memo
 * is there to remove; `GitDiff`'s says what that cost is. `sameSide` and not `JSON.stringify`:
 * `diffTabs.ts` records why, and it is the same reason here, since one side of any comparison
 * came through serde.
 */
function sameRevisionQuestion(a: RevisionDiffPaneProps, b: RevisionDiffPaneProps): boolean {
  return (
    a.project === b.project &&
    a.repo === b.repo &&
    a.path === b.path &&
    a.visible === b.visible &&
    sameSide(a.next, b.next) &&
    sameSide(a.prev, b.prev)
  )
}

export const RevisionDiffPane = memo(function RevisionDiffPane({
  project,
  repo,
  path,
  next,
  prev,
  visible: told,
}: RevisionDiffPaneProps): ReactNode {
  /**
   * Whether the tab drawing this comparison is the one in front, worked out rather than told.
   *
   * The whole argument is `GitDiff`'s and `diffTabs.ts`' — a host's answer wins where it has
   * one, and the workspace mirror answers where it does not, because a deferral that has to be
   * switched on is a deferral that is off. `PaneBody` does pass the flag today; this is what
   * keeps that from being the only thing standing between a background tab and a whole-repo
   * diff per git event.
   */
  const [onScreen, setOnScreen] = useState(true)
  const visible = told ?? onScreen
  // The invalidation listeners are subscribed once per comparison and must not be torn down and
  // rebuilt on every tab switch, so visibility reaches them through a ref.
  const visibleRef = useRef(visible)
  visibleRef.current = visible
  /** Something invalidated this view while the tab was behind another one. */
  const [stale, setStale] = useState(false)
  /** The last refusal written to the log. See the `catch` below. */
  const loggedRef = useRef<string | null>(null)
  // The shared unified/split preference. See the `view` prop below for why this pane needs it.
  const diffView = useSyncExternalStore(subscribeDiffView, getDiffView, getServerDiffView)
  const [diff, setDiff] = useState<RevisionDiff | null>(null)
  const [reason, setReason] = useState<string | null>(null)
  const [collapsed, setCollapsed] = useState<ReadonlySet<number>>(() => new Set<number>())
  /** Gaps opened in a folded whole-file view. Reset by each fetch — the indices are its. */
  const [expandedGaps, setExpandedGaps] = useState<ReadonlySet<number>>(() => new Set<number>())
  /** Which change the iterator is on, or `-1` for none yet. See `GitDiffViewCommon`. */
  const [currentChange, setCurrentChange] = useState(-1)
  /**
   * Whether this pane has ever had a diff in it.
   *
   * `GitDiff` gets this fact for free from `revRef`, which it keeps for the staleness check;
   * this arm has no such ref because two revisions diff to the same bytes for ever and there
   * is nothing here to go stale. So the one bit is kept on its own: it is what tells opening
   * the pane apart from refetching it, and those two want opposite scroll behaviour.
   */
  const openedRef = useRef(false)
  /** Bumped to re-run the fetch. Only a moving side can bump it — see the module note above. */
  const [nonce, setNonce] = useState(0)
  /** One line under the header. On this arm it only ever carries a blame refusal. */
  const [note, setNote] = useState<string | null>(null)

  /*
   * Whether either side is the working tree, and therefore whether this diff can move at all.
   *
   * Computed from the props rather than from the answer: the answer arrives a round trip later,
   * and a listener that only attaches once the first fetch lands would miss every change made
   * while it was in flight — which on a file an agent is writing is the interesting window.
   */
  const moving = next.kind === 'workingTree' || prev.kind === 'workingTree'

  useEffect(() => {
    // A tab nobody is looking at does not fetch — not even its first time, and here that first
    // time is a whole-repository diff. `visible` is read and deliberately not depended on; the
    // reveal effect below is the resume path. `GitDiff`'s fetch states the argument in full.
    if (!visible) {
      setStale(true)
      return
    }
    let disposed = false
    gitLog
      .diff(project, repo, path, next, prev)
      .then((fresh) => {
        if (disposed) return
        setDiff(fresh)
        setReason(null)
        // Re-arms the log below; see `GitDiff`'s success arm.
        loggedRef.current = null
        setExpandedGaps(new Set<number>())
        // The walk: on the way *in* it lands on the first change, for the reason `GitDiff`'s
        // fetch states at length — a diff that opens at line 1 makes the reader hunt for the
        // point of it. On a refetch it goes back to "nothing current" instead, because change
        // 3 of the new diff is not the edit the reader was on.
        setCurrentChange(openedRef.current ? -1 : 0)
        openedRef.current = true
        // Fallback only, as in `GitDiff`: the whole-file view folds gaps instead.
        if (wholeFileSegments(fresh.hunks, fresh.newText) === null) {
          const rows = fresh.hunks.reduce((n, hunk) => n + hunk.lines.length, 0)
          if (rows > COLLAPSE_ABOVE) setCollapsed(new Set(fresh.hunks.map((h) => h.index)))
        }
      })
      .catch((e: unknown) => {
        if (disposed) return
        // `explain` for the same reason `GitDiff`'s fetch gives at length: a `GitError` has no
        // `message`, so this printed `[object Object]` under the notice. No `noSuchChange`
        // special case here — on a *revision* diff that tag means the commit did not touch the
        // path, which is a statement about history rather than about something the reader just
        // did, and `explain`'s wording is right for it.
        const detail = explain(e)
        // The ordinary ends of a revision diff's life: the file did not exist at one of the
        // revisions, or the oid no longer resolves because the branch was rebased under the
        // tab. Not a dialog — the pane says what it knows and stays put.
        setDiff(null)
        setReason(detail)
        // Once per distinct refusal, for the reason `GitDiff`'s catch spells out: `diag_log` is
        // a main-thread round trip, and a refusal that repeats on every git event repeats this
        // line with it.
        if (loggedRef.current !== detail) {
          loggedRef.current = detail
          void diag.log(`revision diff pane: ${path} failed: ${detail}`)
        }
      })
    return () => {
      disposed = true
    }
    // `next`/`prev` are object literals off the tab's spec and change identity on every
    // workspace snapshot, so they are keyed by content: the component is already remounted on a
    // real change (see `GitDiffPane`'s `key`), and depending on the objects would refetch on
    // every unrelated broadcast. `visible` is read and omitted for the reason `GitDiff`'s fetch
    // states: listing it would mark a hidden tab stale and fetch twice on every reveal.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project, repo, path, JSON.stringify(next), JSON.stringify(prev), nonce])

  useEffect(() => {
    let gone = false
    let timer: number | null = null
    const unlisten: Array<() => void> = []
    const bump = () => {
      if (timer !== null) window.clearTimeout(timer)
      // Which of the two this becomes is decided when the timer fires, not when it is set —
      // `GitDiff`'s `bump` says why, and this is the same coalescing over the same triggers.
      timer = window.setTimeout(() => {
        if (visibleRef.current) setNonce((n) => n + 1)
        else setStale(true)
      }, 120)
    }
    const track = (p: Promise<() => void>) => {
      void p
        .then((fn) => {
          if (gone) fn()
          else unlisten.push(fn)
        })
        .catch((e: unknown) => diag.log(`revision diff pane: events unavailable: ${String(e)}`))
    }
    if (moving) {
      track(
        events.onGitStatus((forProject, tree) => {
          noteRepoRoots(tree)
          if (forProject === project) bump()
        }),
      )
      track(
        events.onSessionTool((_session, paths) => {
          // The same path filter `GitDiff` uses, and load-bearing for the same reason:
          // `cide://session-tool` reaches every window and names a session, not a project, so an
          // unfiltered handler refetches on every tool call every agent in the app makes.
          if (touchesFile(paths, repoRoot(repo), path)) bump()
        }),
      )
    }
    /*
     * Subscribed whether or not the diff is moving, because this is not about invalidation: it
     * is what tells the pane to draw its rows at all. A frozen pair never refetches and still
     * has to know when it is behind another tab — see `GitDiffView`'s hidden-tab body.
     *
     * And only when nobody told us, for the reason `GitDiff`'s copy gives — with one more here:
     * `LogTab` renders this pane in the **tool window**, which is not a tab at all, so the
     * mirror could answer "hidden" about the thing the reader is looking at. That host says
     * `visible` explicitly and this listener is then never created.
     *
     * Beside the other two rather than through `store/workspace`, again for `GitDiff`'s reason:
     * that module reaches xterm through `paneHosts`, and this file is SSR-bundled under node by
     * `check-diff-render.mjs`.
     */
    if (told === undefined) {
      track(
        events.onWorkspaceChanged((ws) => {
          setOnScreen(revisionTabOnScreen(ws.projects[project], repo, path, next, prev))
        }),
      )
    }
    return () => {
      gone = true
      if (timer !== null) window.clearTimeout(timer)
      for (const fn of unlisten) fn()
    }
    // `next`/`prev` by content, as the fetch above keys them and for the same reason;
    // `told === undefined` and not `told`, as `GitDiff`'s copy of this list explains.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    moving,
    project,
    repo,
    path,
    JSON.stringify(next),
    JSON.stringify(prev),
    told === undefined,
  ])

  // Reveal is where a deferred fetch is spent, and the flag is cleared by the reveal rather than
  // by the fetch's outcome — `GitDiff`'s copy of this effect argues why the other way round is a
  // loop.
  useEffect(() => {
    if (!visible || !stale) return
    setStale(false)
    setNonce((n) => n + 1)
  }, [visible, stale])

  /*
   * The blame column, and **this arm shows it too**. (M18)
   *
   * The decision is worth stating because the opposite one is defensible: this pane can do
   * nothing with the answer — there is no staging here and no commit to open yet — so the column
   * is pure reading. That is precisely the argument for it. A revision diff is where somebody is
   * *reading history*, which is the question a blame answers; the staging arm, where the column
   * is a 22-character tax on the gesture the pane exists for, is the one with the case against.
   * It also costs less here than there: two revisions diff to the same bytes for ever, so the
   * answer is fetched once and never invalidated, while a working-tree blame is re-walked on
   * every invalidation of the diff beside it.
   *
   * # And it blames `new_rev`, not HEAD
   *
   * `BlameRequest.newest` is the whole reason this is possible: `cide_git::blame` documents that
   * it blames the file **as it was at that commit**, which is exactly the document the new side
   * of this diff is. Blaming HEAD instead would number a different file — a commit from 2019
   * against today's line numbers — and, once again, every row would still carry a plausible oid.
   *
   * The exception is a `RevSide::WorkingTree` new side, which has no commit; there `newest` stays
   * `null` and the blame is of the working file, which is what that side *is*. That is the reason
   * the side is matched on rather than the resolved oid being tested: a `newOid` of `null` also
   * means "the file does not exist at the new revision", which is a diff with no new-side lines
   * at all and nothing to annotate.
   */
  const [blameOn, setBlameOn] = useState(false)
  const [blameFile, setBlameFile] = useState<BlameFile | null>(null)
  const workingSide = next.kind === 'workingTree'
  const newest = workingSide ? null : (diff?.newOid ?? null)
  const blamable = diff !== null && (workingSide || newest !== null)

  useEffect(() => {
    if (!blameOn || !blamable) {
      setBlameFile(null)
      return
    }
    let disposed = false
    historyApi
      .blame(project, repo, path, null, { ...BLAME_DEFAULT, newest })
      .then((fresh) => {
        if (!disposed) setBlameFile(fresh)
      })
      .catch((e: unknown) => {
        if (disposed) return
        setBlameFile(null)
        // The same unwrap-that-did-not-unwrap as the working-tree arm above: `detail` is an
        // object for every refusal this can actually get, so stringifying it printed
        // `[object Object]`.
        const detail = explain(e)
        setNote(`No blame for this file — ${detail}`)
        void diag.log(`revision diff pane: blame ${path} failed: ${detail}`)
      })
    return () => {
      disposed = true
    }
    // `nonce` only moves when a side is the working tree, which is also the only case where this
    // answer can go stale — see the component header. For a frozen pair this list never changes
    // after the diff lands, so the blame is fetched exactly once.
  }, [blameOn, blamable, newest, project, repo, path, nonce])

  const blame = useMemo(
    () => (blameFile === null ? null : blameLookup(blameFile, Math.floor(Date.now() / 1000))),
    [blameFile],
  )
  const tokens = useDiffTokens(diff)

  return (
    <GitDiffView
      readOnly
      diff={diff}
      path={path}
      collapsed={collapsed}
      // Nothing here is ever busy: there is no mutation to be in flight. Passed as the constant
      // rather than removed from the common half, because `busy` also disables the layout
      // toggle, and a read-only diff still switches between unified and split.
      busy={false}
      note={note}
      reason={reason}
      /*
       * The layout toggle, wired exactly as the working-tree pane wires it.
       *
       * > *"when opening diff from git panel - i'm not able to set split view of the diff - but
       * > i should be able to do that. Split button just grey and not clickable"*
       *
       * The comment three lines above already said a read-only diff still switches between
       * unified and split — and then neither `view` nor `onView` was passed, so `GitDiffView`
       * fell back to `'unified'` and drew the control `disabled`, permanently. Reading a
       * historical diff side by side is if anything the *commoner* want: there is nothing to
       * stage, so the two columns are the whole point of opening it.
       *
       * The same store and the same `writable` gate as the other pane. The mode is one editor
       * setting, so a revision diff and a working-tree diff must not each remember their own —
       * and `writable` stays false until `settings.get` lands, because a patch built before the
       * real `EditorSettings` are known would send guesses for `fontSize` and the rest.
       */
      view={diffView.view}
      {...(diffView.writable ? { onView: setDiffView } : {})}
      tokens={tokens}
      currentChange={currentChange}
      onCurrentChange={setCurrentChange}
      visible={visible}
      {...(blameOn ? { blame } : {})}
      /*
       * Offered until the answer proves there is nothing to offer. A diff whose new side is a
       * commit the file does not exist at has no new-side lines at all — every row is a deletion
       * — so the column could only ever be empty, and a toggle that turns nothing on is the
       * listed-and-inert state. Before the diff lands the question cannot be asked, so the
       * control stays live and the fetch waits for it.
       */
      {...(diff === null || blamable ? { onBlame: setBlameOn } : {})}
      revisions={{
        from: diff === null ? null : revLabel(prev, diff.oldOid),
        // Before the answer arrives there is nothing resolved to show, so the requested side is
        // the best available label and `?` is the honest stand-in for an unresolved one.
        to: (diff === null ? null : revLabel(next, diff.newOid)) ?? '?',
      }}
      onCollapse={(hunk) =>
        setCollapsed((prev_) => {
          const nextSet = new Set(prev_)
          if (!nextSet.delete(hunk)) nextSet.add(hunk)
          return nextSet
        })
      }
      expandedGaps={expandedGaps}
      onExpandGap={(gap) => setExpandedGaps((prev_) => new Set(prev_).add(gap))}
    />
  )
}, sameRevisionQuestion)
