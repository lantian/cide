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
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactNode,
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
import { diffTabOnScreen } from './diffTabs'
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
 * One row of a side-by-side hunk: what is on the left, what is on the right, either may be
 * absent.
 *
 * `at` is the index into `hunk.lines` — the second half of a `hunk:line` mark — and it is
 * `null` on the mirrored half of a context row. A context line is one entry in the unified
 * diff and is drawn twice here, so exactly one of the two cells carries the position; without
 * that rule the same mark would appear twice in the DOM and "the rows drawn as selected" would
 * no longer be a set of positions.
 */
export interface SplitCell {
  line: DiffLineView
  at: number | null
}

export interface SplitRow {
  left: SplitCell | null
  right: SplitCell | null
}

/**
 * Pair a hunk's unified rows into two columns.
 *
 * A run of deletions immediately followed by a run of additions is one edit, so the two runs
 * are zipped: the first deletion faces the first addition, and whichever run is shorter leaves
 * blanks at the bottom. That is what makes a side-by-side diff readable — the changed line and
 * what it became are on the same row — and it is the whole content of "side by side"; anything
 * finer (matching by similarity rather than by position) is a diff algorithm, and the diff has
 * already been computed by libgit2.
 *
 * A deletion *after* an addition starts a new pair group rather than joining the one before
 * it. `git` emits `-` before `+` within an edit, so `+` then `-` means two separate edits that
 * happen to be adjacent, and zipping across the boundary would face a line against a line from
 * a different change.
 *
 * Pure and exported so it can be reasoned about on its own; it is also what
 * `ui/scripts/check-diff-render.mjs` would assert against if the split view ever grows
 * fixtures of its own.
 */
export function splitHunk(hunk: DiffHunkView): SplitRow[] {
  const out: SplitRow[] = []
  let dels: SplitCell[] = []
  let adds: SplitCell[] = []

  const flush = (): void => {
    for (let i = 0; i < Math.max(dels.length, adds.length); i++) {
      out.push({ left: dels[i] ?? null, right: adds[i] ?? null })
    }
    dels = []
    adds = []
  }

  hunk.lines.forEach((line, at) => {
    if (line.origin === 'deletion') {
      if (adds.length > 0) flush()
      dels.push({ line, at })
    } else if (line.origin === 'addition') {
      adds.push({ line, at })
    } else {
      flush()
      // The position rides on the left cell; the right one is the same text, unnumbered.
      out.push({ left: { line, at }, right: { line, at: null } })
    }
  })
  flush()
  return out
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
  onCollapse: (hunk: number) => void
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
                : 'The two sides beside each other, in one scroller.'
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
      <div className={styles.pane} data-audit="gitDiffPane" ref={setRoot}>
        {header}
        <div className={styles.notice}>
          {reason === null ? (
            'Reading the diff…'
          ) : (
            <>
              <div>Nothing to show on this side.</div>
              <div className={styles.noticeWhy}>{reason}</div>
            </>
          )}
        </div>
      </div>
    )
  }

  /*
   * Whether the tick boxes are drawn at all.
   *
   * `staging === null` is the new clause and it withholds them from the whole revision arm:
   * there is no index to stage two commits into, so a box there would be a control whose only
   * possible outcome is a refusal from Rust.
   */
  const selectable =
    staging !== null && staging.diff !== null && staging.diff.partialOk && diff.hunks.length > 0
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
    entry: SplitCell | null,
    which: 'old' | 'new' | 'both',
    className: string,
  ): ReactNode => {
    if (entry === null) {
      // The other side has a line here and this one does not. Kept in the flow rather than
      // omitted so the two columns stay in step row for row — an absent cell would slide
      // everything below it up by one and face the wrong lines against each other.
      return <div key={key} className={`${className} ${styles.filler ?? ''}`} aria-hidden="true" />
    }
    const { line, at } = entry
    const change = line.origin !== 'context'
    const on = at !== null && painted.has(mark(hunkIndex, at))
    const numbered = which === 'new' ? line.newLineno : line.oldLineno
    /*
     * Which commit wrote this line — on the new side only.
     *
     * `which === 'old'` is the split layout's left half, and it gets a spacer instead of a cell.
     * The reason is the same one `blameFor` gives for refusing `oldLineno`: the left column is a
     * *different document*, and annotating it needs a second blame at the other revision. The
     * spacer is not decoration — it is the `entry === null` argument one level down. Both halves
     * are grids with the same template, so a column present in one and absent in the other would
     * slide the left side's code out of line with the right's for every row of the diff.
     */
    const annotated = blame !== null && which !== 'old' ? blameFor(blame, line) : null
    return (
      <div
        key={key}
        className={`${className} ${rowClass(line.origin)}`}
        {...(at === null
          ? {}
          : {
              'data-audit': 'gitDiffRow',
              'data-at': `${hunkIndex}:${at}`,
              'data-selected': on ? 'true' : 'false',
            })}
      >
        {selectable && (
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
        {blame !== null &&
          (which === 'old' ? (
            <span className={styles.blameGap} data-audit="gitDiffBlameGap" aria-hidden="true" />
          ) : (
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
          ))}
        {which === 'both' ? (
          <>
            <span className={styles.lineno}>{line.oldLineno ?? ''}</span>
            <span className={styles.lineno}>{line.newLineno ?? ''}</span>
          </>
        ) : (
          <span className={styles.lineno}>{numbered ?? ''}</span>
        )}
        <span className={styles.sign}>
          {line.origin === 'addition' ? '+' : line.origin === 'deletion' ? '-' : ' '}
        </span>
        <span className={styles.text}>
          {line.content}
          {line.noNewline && <span className={styles.noNewline}> ⏎ no newline at end of file</span>}
        </span>
      </div>
    )
  }

  return (
    <div className={styles.pane} data-audit="gitDiffPane" ref={setRoot}>
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
      {staging !== null && held !== null && (
        <p className={styles.note} data-audit="gitDiffHeld">
          {held} line{held === 1 ? '' : 's'} of this file are held for the next commit.{' '}
          <button type="button" className={styles.link} onClick={staging.onDropHeld}>
            Commit the whole file instead
          </button>
        </p>
      )}

      <div className={styles.body}>
        {diff.hunks.length === 0 && <p className={styles.notice}>No text changes on this side.</p>}
        {diff.hunks.map((hunk) => {
          const state =
            staging === null || staging.diff === null
              ? 'none'
              : hunkState(staging.marks, staging.diff, hunk.index)
          const shut = collapsed.has(hunk.index)
          return (
            <section key={hunk.index} className={styles.hunk}>
              <div className={styles.hunkHeader}>
                {selectable && (
                  <button
                    type="button"
                    role="checkbox"
                    aria-checked={state === 'all' ? true : state === 'some' ? 'mixed' : false}
                    aria-label={`Select hunk ${hunk.index + 1}`}
                    className={styles.box}
                    data-audit="gitDiffHunkBox"
                    data-state={state}
                    onClick={() => {
                      if (staging === null || staging.diff === null) return
                      staging.onMarks(toggleHunk(staging.marks, staging.diff, hunk.index))
                    }}
                  >
                    {state === 'all' ? (
                      <Icon name="check" size={0} />
                    ) : state === 'some' ? (
                      <Icon name="minus" size={0} />
                    ) : null}
                  </button>
                )}
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
                {selectable && op !== 'commit' && (
                  <button
                    type="button"
                    className={styles.hunkAction}
                    disabled={busy}
                    // The whole hunk, whatever is ticked — the gesture people expect from a
                    // hunk header, and the shortest path to `Selection::Hunks`. The same
                    // `hunkMarks` the checkbox uses, so the two cannot disagree about what
                    // "this hunk" means.
                    onClick={() => {
                      if (staging === null || staging.diff === null) return
                      staging.onApply(new Set(hunkMarks(staging.diff, hunk.index)))
                    }}
                  >
                    {op === 'stage' ? 'Stage hunk' : 'Unstage hunk'}
                  </button>
                )}
              </div>

              {/*
                * `data-boxed` and `data-blamed` are the row grid's column list, stated once per
                * hunk instead of once per row.
                *
                * A row's cells are placed by *order*, so a template with a track for a cell that
                * was not rendered puts every later cell one column to the left. That is not
                * hypothetical: the read-only arm draws no tick box, and until the blame column
                * needed a fifth track nothing had noticed that its line numbers were sitting in
                * the box's 16px and its text in the sign's 2ch. The stylesheet spells out all
                * four combinations; the flags are what pick one.
                */}
              {!shut && layout === 'unified' && (
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

              {/*
                * Side by side. **One scroller, two columns** — the two sides are cells of the
                * same grid row, so they are aligned by layout and scroll together because
                * there is only one thing scrolling. That is the whole reason this is not two
                * synchronized editors and not `@codemirror/merge`'s `MergeView`: a scroll
                * listener writing the other pane's `scrollTop` writes a scroll event back, and
                * the guard flag that stops the loop is the bug people spend an afternoon on.
                * Here there is no loop to guard.
                *
                * `MergeView` lost for a second, harder reason. It diffs two whole *documents*,
                * and `git_diff_file` hands this pane a patch: hunks with a few lines of context
                * and nothing between them. The documents it would need do not exist here, and
                * its chunks are its own — mapping a `MergeView` chunk back onto a `hunk:line`
                * position is exactly the index arithmetic that stages the line next to the one
                * the user ticked. `DiffPane` uses `MergeView` and is right to: it has both
                * documents in full.
                */}
              {!shut && layout === 'split' && (
                <div
                  className={styles.lines}
                  data-boxed={selectable ? 'true' : 'false'}
                  data-blamed={blame !== null ? 'true' : 'false'}
                >
                  {splitHunk(hunk).map((row, index) => (
                    <div key={index} className={styles.splitRow}>
                      {cell('l', hunk.index, row.left, 'old', styles.half ?? '')}
                      {cell('r', hunk.index, row.right, 'new', styles.half ?? '')}
                    </div>
                  ))}
                </div>
              )}
            </section>
          )
        })}
      </div>

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

interface GitDiffProps {
  project: ProjectId
  repo: RepoId
  path: string
  /** The side the tab was opened on. The pane owns it from here. */
  from: DiffSide
  /** The host's answer, when it has one. `undefined` means "work it out". */
  visible: boolean | undefined
}

function GitDiff({ project, repo, path, from, visible: told }: GitDiffProps): ReactNode {
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
        const detail =
          typeof e === 'object' && e !== null && 'detail' in e
            ? String((e as { detail: unknown }).detail)
            : e instanceof Error
              ? e.message
              : String(e)
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

  useEffect(() => {
    let disposed = false
    gitApi
      .diffFile(project, repo, path, side)
      .then((fresh) => {
        if (disposed) return
        setDiff(fresh)
        setReason(null)
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
        }
        revRef.current = fresh.rev
        const rows = fresh.hunks.reduce((n, hunk) => n + hunk.lines.length, 0)
        if (rows > COLLAPSE_ABOVE) setCollapsed(new Set(fresh.hunks.map((h) => h.index)))
      })
      .catch((e: unknown) => {
        if (disposed) return
        const detail = e instanceof Error ? e.message : String(e)
        // `NoSuchChange` is the ordinary end of a diff's life: the change was committed,
        // reverted or staged away. Not a dialog — the side simply has nothing to show, and
        // the switcher above is still there to look at another one.
        setDiff(null)
        setReason(detail)
        void diag.log(`git diff pane: ${path} on ${side} failed: ${detail}`)
      })
    return () => {
      disposed = true
    }
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
    // Which tab is in front is workspace state, so it arrives here the way every other piece
    // of workspace state does. Subscribed beside the other two rather than through the store
    // hook that already follows this event: `store/workspace` reaches xterm through
    // `paneHosts`, and importing it here would put a terminal in the SSR bundle
    // `check-diff-render.mjs` renders this pane's view from.
    track(
      events.onWorkspaceChanged((ws) => {
        setOnScreen(diffTabOnScreen(ws.projects[project], repo, path))
      }),
    )
    return () => {
      gone = true
      if (timer !== null) window.clearTimeout(timer)
      for (const fn of unlisten) fn()
    }
  }, [project, repo, path])

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
      const detail = e instanceof Error ? e.message : String(e)
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
      onApply={apply}
      onDropHeld={() => clearPartial(repo, path)}
    />
  )
}

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
 * **No visibility deferral and no `git-status` subscription for a frozen pair.** `GitDiff`
 * spends fifty lines on both because a working-tree diff goes stale constantly — an agent
 * writes the file, a build regenerates it, the user saves. Two commits diff to the same bytes
 * for ever, so a tab showing one is correct from its first frame until it is closed, and a
 * refetch would be a round trip that cannot change a pixel.
 *
 * The exception is a [`RevSide::WorkingTree`] side, which is exactly why that is a variant a
 * caller can match on rather than a magic oid. When one is present the same two events are
 * subscribed, with the same 120 ms coalescing, because then the diff *is* moving.
 */
export function RevisionDiffPane({
  project,
  repo,
  path,
  next,
  prev,
}: RevisionDiffPaneProps): ReactNode {
  // The shared unified/split preference. See the `view` prop below for why this pane needs it.
  const diffView = useSyncExternalStore(subscribeDiffView, getDiffView, getServerDiffView)
  const [diff, setDiff] = useState<RevisionDiff | null>(null)
  const [reason, setReason] = useState<string | null>(null)
  const [collapsed, setCollapsed] = useState<ReadonlySet<number>>(() => new Set<number>())
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
    let disposed = false
    gitLog
      .diff(project, repo, path, next, prev)
      .then((fresh) => {
        if (disposed) return
        setDiff(fresh)
        setReason(null)
        const rows = fresh.hunks.reduce((n, hunk) => n + hunk.lines.length, 0)
        if (rows > COLLAPSE_ABOVE) setCollapsed(new Set(fresh.hunks.map((h) => h.index)))
      })
      .catch((e: unknown) => {
        if (disposed) return
        const detail = e instanceof Error ? e.message : String(e)
        // The ordinary ends of a revision diff's life: the file did not exist at one of the
        // revisions, or the oid no longer resolves because the branch was rebased under the
        // tab. Not a dialog — the pane says what it knows and stays put.
        setDiff(null)
        setReason(detail)
        void diag.log(`revision diff pane: ${path} failed: ${detail}`)
      })
    return () => {
      disposed = true
    }
    // `next`/`prev` are object literals off the tab's spec and change identity on every
    // workspace snapshot, so they are keyed by content: the component is already remounted on a
    // real change (see `GitDiffPane`'s `key`), and depending on the objects would refetch on
    // every unrelated broadcast.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project, repo, path, JSON.stringify(next), JSON.stringify(prev), nonce])

  useEffect(() => {
    if (!moving) return
    let gone = false
    let timer: number | null = null
    const unlisten: Array<() => void> = []
    const bump = () => {
      if (timer !== null) window.clearTimeout(timer)
      timer = window.setTimeout(() => setNonce((n) => n + 1), 120)
    }
    const track = (p: Promise<() => void>) => {
      void p
        .then((fn) => {
          if (gone) fn()
          else unlisten.push(fn)
        })
        .catch((e: unknown) => diag.log(`revision diff pane: events unavailable: ${String(e)}`))
    }
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
    return () => {
      gone = true
      if (timer !== null) window.clearTimeout(timer)
      for (const fn of unlisten) fn()
    }
  }, [moving, project, repo, path])

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
        const detail =
          typeof e === 'object' && e !== null && 'detail' in e
            ? String((e as { detail: unknown }).detail)
            : e instanceof Error
              ? e.message
              : String(e)
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
    />
  )
}
