/**
 * One tab of the git tool window: the commit list, wired. (M19)
 *
 * The Log tab and a per-file History tab are the same component — they differ by `path`, which
 * is the same field the backend query differs by. `LogView.tsx` is the pure half.
 *
 * # Why the state is local and not a store
 *
 * A page of commits belongs to *this tab*: two History tabs are two different questions, and the
 * Log tab is a third. There is no cross-window fact here — nothing else in the app reads a log —
 * so the workspace mirror has nothing to say about it, and a module-level store would need a key
 * per tab and a lifetime rule to go with it. `useState` keyed by the tab's own identity is the
 * smaller thing that is also correct.
 *
 * # The generation counter
 *
 * Every fetch is tagged, and an answer whose tag is not the current one is dropped. Without it,
 * switching repositories or typing a filter mid-flight paints the *previous* question's answer
 * over the current one, which is `chrome/gitCountStore.ts`'s recorded bug. There is no
 * cancellation to lean on: `spawn_blocking` cannot be interrupted, so a superseded walk finishes
 * and its result has to be discarded on this side.
 *
 * # Two filters, one rule, and why the typed value and the asked value are separate state
 *
 * `typed` is what the boxes show and changes on the keystroke. `applied` is what the wire was
 * last asked and changes [`FILTER_DEBOUNCE_MS`] later. The list is drawn from the rows of
 * `applied`, narrowed locally by `typed` while the two disagree — so the list reacts on the
 * keystroke and is replaced by the authoritative answer when it lands.
 *
 * The narrow is deliberately **not** applied once the two agree. The backend matches the whole
 * message and `CommitRow` carries only the summary, so re-running the local rule over the wire's
 * own answer would hide the rows it found by their body: a commit would appear for one frame and
 * then be filtered out by the pass whose entire purpose is to make the box feel fast.
 * `logModel::matchesText` documents both directions of that gap.
 *
 * # Three things reach this file from outside, and each has its own note below
 *
 * * **`FsChange.git`** — the log refreshes itself when a `git commit` in a bash pane moves a ref.
 *   See the *git moved underneath* effect and [`GIT_REFRESH_MS`].
 * * **A reveal request** — "select this commit", from a click on a blame line. See
 *   [`requestLogReveal`], which is the seam `panes/EditorPane.tsx` used to stand in for with a
 *   notice naming the oid.
 *
 * # The commit actions (M19)
 *
 * A right-click on a row is the only way into `git_revert`, `git_cherry_pick`, `git_reset`,
 * `git_tag_create`, `git_checkout_detached` and *branch from here*. All six shipped tested
 * against the real `git` and reachable from nothing; the menu is the seam that ends that.
 *
 * Four rules hold this half together, and each of them is a bug if it is broken:
 *
 * * **`chrome/logActions.ts` owns every sentence.** Not one of the dialog titles, bodies, mode
 *   labels or completion notes is written here. That module has no DOM and no imports, so
 *   `check-log-actions.mjs` drives the *wording* directly — and wording is exactly the half of a
 *   confirmation that no Rust test can see.
 * * **`useContextMenu` lives here and not in `LogView`.** The hook reads the window's live keymap
 *   out of `@/store/workspace`, which imports `layout/paneHosts`, which calls
 *   `document.createElement` at module scope. `check-log-render.mjs` server-renders `LogView`;
 *   the view therefore takes a [`LogRowMenu`] handle, exactly as `GitPanelView` takes `treeMenu`.
 * * **The dialog is local state, and this component runs it.** `ConfirmDestructive` calls neither
 *   `onCancel` nor `run` itself — there is no global confirmation store and `GitPanel.tsx` and
 *   `FileTree.tsx` both hold their own `ConfirmState | null`. See [`LogConfirm`].
 * * **Nothing refreshes the list afterwards, on purpose.** Every mutating command in
 *   `cmd/git.rs` ends with `refreshed(…)` *and* moves a ref on disk, so the `cide://fs-changed`
 *   subscription below sees `.git/refs/…` change and re-fetches page one through the same
 *   throttle a `git commit` in a bash pane goes through. A second refresh here would be a second
 *   walk over the same history, 300 ms before the first one.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { create } from 'zustand'
import {
  branch,
  commitActions,
  events,
  file,
  git,
  gitLog,
  logQuery,
  logWalk,
  revisionDiff,
  toolWindow,
} from '@/ipc/client'
import type {
  BranchList,
  CheckoutMode,
  CommitDetail,
  CommitPage,
  CommitRow,
  GraphRow,
  LogScope,
  ProjectId,
  RepoId,
  RepoInfo,
  RevSide,
  RevisionRange,
  ToolTabId,
} from '@/ipc/client'
import { useContextMenu, type MenuEntry } from '@/menus'
import { useIconTheme } from '@/icons'
import type { IconTheme } from '@/icons/iconFor'
import { fileMenu, type FileMenuId } from './fileMenu'
import { toggleCollapsed } from './fileRows'
import { RevisionDiffPane } from '@/panes/GitDiffPane'
import { LOG_SPLIT_DEFAULT, LOG_SPLIT_HISTORY } from '@/toolwindow/logSplit'
import { explain, refusalOf } from '@/chrome/branchModel'
import { panelHostPresent, requestAmend } from '@/chrome/panelRequests'
import { NO_CLIPBOARD } from '@/chrome/menuModel'
import { notify } from '@/chrome/notices'
import { ConfirmDestructive, type ConfirmChoice } from '@/chrome/ConfirmDestructive'
import {
  MAINLINE_TITLE,
  SHELVE_FIRST_DEFAULT,
  SHELVE_FIRST_LABEL,
  detachConfirm,
  detachNote,
  forceTagConfirm,
  mainlineBody,
  mainlineChoices,
  replayNote,
  resetBody,
  resetChoices,
  resetNote,
  resetTitle,
  shelveFirstOffered,
  tagNote,
} from '@/chrome/logActions'
import {
  commitMenu,
  errorField,
  mainlineOf,
  pickedId,
  replayRequestFor,
  resetKindOf,
  resetRequestFor,
  tagRequestFor,
} from './logMenu'
import { openTagDialog, registerTagDialog } from '@/keys/dispatch'
import { OverlayCard } from '@/overlays/ModalShell'
import { gestureOf, logFileClick } from '@/sidebar/clickSemantics'
import { copyText } from '@/sidebar/copyText'
import { useWorkspace } from '@/store/workspace'
import {
  applyLocalText,
  comparePair,
  CROSS_REPO_COMPARE,
  exactly,
  filterOverrides,
  FILTER_DEBOUNCE_MS,
  findCommitFilter,
  gitRefsMoved,
  graphOffReason,
  isFiltered,
  logStatus,
  mergePage,
  NO_FILTER,
  NO_REVEAL,
  NO_SELECTION,
  RANGE_LOADING,
  sameFilter,
  selectRow,
  shortenOid,
  swapPair,
  WORKING_TREE_LABEL,
  type ComparePair,
  type LogFilter,
  type LogReveal,
  type LogSelection,
  type SelectMods,
} from './logModel'
import { ChangedFileList, LogView, RangeDetails } from './LogView'
import styles from './LogView.module.css'

/* ------------------------------------------------------------------------------------------
 * The reveal seam
 * --------------------------------------------------------------------------------------- */

/** One outstanding "show me this commit". */
interface RevealRequest {
  readonly project: ProjectId
  /**
   * The repository the oid lives in.
   *
   * Required, not `null`-for-unknown. The only caller — a blame gutter — already has it (the
   * blame it is drawing was fetched against one repository), and the alternative is a loop of
   * `git_commit_detail` calls across every root of a monorepo, each of which is a real object
   * lookup, to discover something the caller was holding all along.
   */
  readonly repo: RepoId
  readonly oid: string
  /**
   * The **repo-relative** file the reveal came from, or `null`.
   *
   * > *"when clicking the blame item - currently git commit is opened - this ok, but we also
   * > should select the file in right panel of git panel"*
   *
   * A blame click is about one line of one file, so landing on the commit and leaving the reader
   * to find that file among its forty is half a gesture. The caller already has the path — it is
   * `git_locate`'s answer, which the same click needed to find the repository — so carrying it
   * costs a string and asking for it later would cost a lookup the caller already did.
   *
   * `null` for a reveal that is genuinely only about a commit, which is what the palette would
   * send if it ever grew one.
   */
  readonly file: string | null
  /** `Date.now()` at the request, for the TTL. See [`REVEAL_TTL_MS`]. */
  readonly at: number
  /**
   * Distinguishes two requests for the same commit.
   *
   * Clicking the same blame line twice is two gestures and must scroll twice: the user may have
   * scrolled the list away in between. Without this the second request would be `Object.is`-equal
   * to the first as far as zustand is concerned in the case that matters — same project, same
   * repo, same oid, same millisecond — and nothing would re-render.
   */
  readonly nonce: number
}

/**
 * How long a parked request stays live.
 *
 * `editor/revealRequest.ts` has the argument and it applies here word for word: *"an entry parked
 * for a file that never opened would fire minutes later when the user opens that same file by
 * hand… for a search they have forgotten. A reveal is the answer to a click, and it goes stale
 * with the click."* The window here has to cover a tool window opening and its first page landing
 * — a `git_log` over a large repository — so it is generous rather than tight; what it rules out
 * is the reveal firing on some unrelated visit to the Log tab an hour later.
 */
const REVEAL_TTL_MS = 30_000

/**
 * How long a burst of ref moves is held before one walk runs.
 *
 * The same interval and the same *throttle-not-debounce* as `chrome/gitCountStore.ts` and
 * `sidebar/gitStatusStore.ts`, for the reason both of them write down: a debounce that restarts
 * on every trigger is starved indefinitely by a steady writer, and a panel that freezes for as
 * long as anything is happening is worse than one that answers 120 ms after the first change of
 * each burst.
 *
 * It was 120 ms on the reasoning that `cide-fs` had already done the coalescing — it holds a
 * burst until the tree is quiet for 300 ms — and that this only had to absorb the *pair*: a
 * mutation cide makes produces `cide://git-status` and then `cide://fs-changed`, and a rebase or
 * a fetch writes several watched files in quick succession after the watcher has flushed once.
 *
 * That reasoning assumed the tree goes quiet. On this repository it does not: several agents
 * write to it continuously, so the watcher flushes on its `max_wait` instead, every burst is
 * `truncated`, and `gitRefsMoved` cannot tell a dropped path list from a moved ref — so it says
 * yes. The predicate that exists to keep a `git add` from restarting a frontier walk is
 * therefore defeated by exactly the workload it was written for.
 *
 * Half a second, and the cost of the change is that a `git commit` typed into a bash pane shows
 * up an eighth of a second later than it used to, which is not perceptible. The reload itself is
 * no longer visible at all — the list keeps its rows and its selection and swaps in place — so
 * what remains is backend work, bounded by this window and cancelled when superseded.
 */
const GIT_REFRESH_MS = 500

interface RevealStore {
  pending: RevealRequest | null
  request: (next: RevealRequest) => void
  clear: (spent: RevealRequest) => void
}

const useRevealRequests = create<RevealStore>((set, get) => ({
  pending: null,
  request(next) {
    set({ pending: next })
  },
  clear(spent) {
    // Only the request that was actually consumed. A blind `set({pending: null})` would drop a
    // *newer* request that arrived between the consumer deciding and it calling back — the user
    // clicking a second blame line while the first was still fetching.
    if (get().pending !== spent) return
    set({ pending: null })
  },
}))

let revealNonce = 0

/**
 * "Open the Log tab and select this commit" — from outside React.
 *
 * # Why a parked request and not a call into the panel
 *
 * The same two constraints `chrome/focusRequests.ts` sets out, and this is modelled on it
 * deliberately:
 *
 *   * **The Log tab is usually not mounted.** The caller opens the tool window in the same turn,
 *     and that only mounts `LogTab` on the next React render — so anything that reached for the
 *     component synchronously would find nothing on the common path.
 *   * **And sometimes it already is.** When the tool window is open on the Log tab, revealing
 *     mounts nothing at all, so a mount-only hook would silently do nothing on the second click.
 *
 * So the request is parked and consumed by whichever render sees it first. It is *consumed* and
 * not merely observed — see `focusRequests.ts` on why a pulse counter has a bug a spent flag does
 * not.
 *
 * # Why the caller opens the tool window and this does not
 *
 * Separation of two failures. `tool_window_set_layout` and `tool_window_activate` can each be
 * refused, and a refusal is a sentence the *caller* has to show; parking a request cannot fail at
 * all. Folding the two together would make this an async function whose failure mode is a reveal
 * that never happens with nothing on screen — the shape being fixed, not a new one.
 */
export function requestLogReveal(
  project: ProjectId,
  repo: RepoId,
  oid: string,
  file: string | null = null,
): void {
  revealNonce += 1
  useRevealRequests
    .getState()
    .request({ project, repo, oid, file, at: Date.now(), nonce: revealNonce })
}

/* ------------------------------------------------------------------------------------------
 * Is a revision diff open right now?
 * --------------------------------------------------------------------------------------- */

/* ------------------------------------------------------------------------------------------
 * The two local overlays
 * --------------------------------------------------------------------------------------- */

/**
 * A `ConfirmDestructive` waiting for an answer.
 *
 * The **wording half** of a `ConfirmState` — everything `chrome/logActions.ts` can decide —
 * plus the two things it cannot: what running the act does, and whether this act's chosen mode
 * wants the one checkbox. Every field above `optionFor` is spread straight out of that module,
 * which is why `detachConfirm(row, blockers)` and `forceTagConfirm(…)` can be used with a single
 * `...` and no sentence is written twice.
 *
 * `run` takes the answers rather than reading them, because `ConfirmDestructive` is controlled:
 * the radio and the checkbox live in this component's state and the component only paints them.
 */
interface LogConfirm {
  readonly title: string
  readonly body: string
  readonly files: readonly string[]
  readonly confirmLabel: string
  readonly mark?: string | undefined
  /** The modes this act has. Absent for an act with one — see [`ConfirmState.choices`]. */
  readonly choices?: readonly ConfirmChoice[] | undefined
  /**
   * The checkbox's label for the mode currently selected, or `null` for no checkbox.
   *
   * A function of the selection and not a constant, because reset's *Shelve my changes first*
   * only makes sense on `--hard` with a dirty tree: `--soft` and `--mixed` leave the working
   * tree alone, so offering it there would be a control that never does anything — and a
   * control the user has learned does nothing is one they stop reading before the mode where
   * it matters. `logActions::shelveFirstOffered` is the rule.
   */
  readonly optionFor?: ((picked: string | null) => string | null) | undefined
  readonly run: (picked: string | null, option: boolean) => void
}

/**
 * A name prompt waiting for an answer: *New branch from here…* or *Tag…*.
 *
 * Two fields, and everything else is derived at render. That is deliberate: the tag prompt can
 * be opened from `keys/dispatch.ts` by a command that knows nothing about this page, and a state
 * object that had captured the repository, the short oid or the row would be capturing the page
 * that was loaded at the moment the palette was used — which a filter keystroke replaces.
 */
interface NamePrompt {
  readonly kind: 'branch' | 'tag'
  /**
   * The commit, as a full oid — or the literal `HEAD`, which is what `git.tag.new` from the
   * palette means and what `TagRequest.target` accepts as a revspec.
   */
  readonly target: string
}

/** How much of an oid the prompt shows when the commit is not on the loaded page. */
const PROMPT_OID_WIDTH = 8

/**
 * The row a menu gesture landed on, or `null`.
 *
 * Two routes, and they answer differently on purpose.
 *
 * A **pointer** right-click resolves the row under it by walking up to the nearest
 * `[data-row-id]` — the same lookup `sidebar/GitPanel/GitPanelHost.tsx` does, and the difference
 * between a menu that acts on what you right-clicked and one that acts on whatever was last
 * hovered. Finding none means the empty space below the last row, and the answer is `null`: a
 * menu about the selected commit, opened by a click that was nowhere near it, would be acting on
 * something the user was not pointing at.
 *
 * A **keyboard** invocation — Shift+F10, the Menu key — arrives as a `contextmenu` event at 0,0,
 * and `useContextMenu` anchors it to the focused element. Here that is the listbox and never a
 * row: the rows are `role="option"` and the scroller is the single tab stop. So there is nothing
 * under the pointer to read and the *selection* is what the keyboard is pointing at. Without
 * this arm the whole menu would be unreachable without a mouse.
 */
function rowFor(
  rows: readonly CommitRow[],
  selected: string | null,
  target: HTMLElement | null,
  x: number,
  y: number,
): CommitRow | null {
  const id = target?.closest<HTMLElement>('[data-row-id]')?.dataset['rowId']
  if (id !== undefined) return rows.find((r) => `${r.repo}:${r.oid}` === id) ?? null
  if (x !== 0 || y !== 0) return null
  return rows.find((r) => r.oid === selected) ?? null
}

/**
 * Everything the details pane needs in order to draw a comparison.
 *
 * Module-level rather than inline in the component, so the one derivation that produces it has a
 * named shape to be checked against: the two gestures that fill it — a two-row selection and
 * *Compare with working tree* — differ in exactly the five fields below and in nothing else.
 */
interface CompareView {
  readonly repo: RepoId
  /** The right-hand side. `WorkingTree` only ever appears here — see [`swappable`]. */
  readonly next: RevSide
  readonly prev: RevSide
  /** What the header calls each side: a full oid, which is abbreviated, or a name that is not. */
  readonly newerLabel: string
  readonly olderLabel: string
  /**
   * Whether ⇄ Swap is offered.
   *
   * `false` for the working tree, and derived rather than disabled: `RevSide::WorkingTree` is
   * legal only as the newer side and `cide_git::revision` refuses the other arrangement by name,
   * so a swap control there would be a control whose only outcome is a typed error.
   */
  readonly swappable: boolean
  /** A sentence in the file list's place, or `null` to go and fetch one. */
  readonly refusal: string | null
}

/**
 * A selection made by something that is not a pointer: a reveal, a resolved revspec.
 *
 * A named constant rather than `{ ctrl: false, shift: false }` written at each call site, because
 * the two booleans are not decoration — `logModel::selectRow` reads them as the whole gesture, and
 * a `true` slipped into one of them at one of these call sites would silently turn a reveal into
 * a second comparison endpoint.
 */
const NO_MODS: SelectMods = { ctrl: false, shift: false }

/** A stable empty list, so an empty log does not hand `useMemo` a new array every render. */
const NO_ORDER: readonly string[] = []

export interface LogTabProps {
  project: ProjectId
  /**
   * Which tool-window tab this is, for the cancellation registry.
   *
   * **Per tab and not per project**, which is the whole reason `LogRegistry` is keyed the way it
   * is: the Log tab and any number of History tabs are live at once with different questions, and
   * one job per project would make opening a History tab cancel the Log's walk.
   *
   * It must be **stable across remounts**, or two remounts look like two tabs and neither cancels
   * the other. `ToolWindowHost` satisfies that by construction — a history tab's id is persisted
   * in `workspace.json`, and the Log tab, which has no id at all, gets a fixed sentinel.
   */
  tab: ToolTabId
  /** `null` on the Log tab: walk every repository the project has. */
  repo: RepoId | null
  /** Repo-relative. `null` on the Log tab. */
  path: string | null
}

export function LogTab({ project, tab, repo, path }: LogTabProps) {
  const [rows, setRows] = useState<readonly CommitRow[]>([])
  /*
   * How many rows are on screen, readable without depending on them.
   *
   * The refresh effect needs the count to re-walk to the same depth, and must not re-run when it
   * changes: `rows` in its dependency list would make every *Load more* immediately trigger a
   * refresh of everything it had just appended. A ref is the standard answer and this is the
   * standard reason for it.
   */
  const rowsRef = useRef<readonly CommitRow[]>([])
  rowsRef.current = rows
  const [graph, setGraph] = useState<readonly GraphRow[]>([])
  const [graphOff, setGraphOff] = useState<string | null>(null)
  const [page, setPage] = useState<CommitPage | null>(null)
  /**
   * Which commits are selected: one to read, or two to compare. (M20)
   *
   * One state and not a `selected` beside a `compareWith`, because the two would be able to
   * disagree — a pane showing a range whose anchor had been replaced by a plain click is a state
   * `logModel::selectRow` makes unrepresentable and two `useState`s would make routine.
   * `selection.primary` is what the rest of this file used to call `selected`, and every gesture
   * that used to set it now goes through the reducer.
   */
  const [selection, setSelection] = useState<LogSelection>(NO_SELECTION)
  const selected = selection.primary
  /** Collapse to one commit — a reveal, a failed lookup, or a plain click. */
  const selectOnly = useCallback((oid: string | null) => {
    setSelection({ primary: oid, secondary: null, swapped: false })
  }, [])
  /**
   * The repository the selected commit belongs to.
   *
   * Held beside the oid rather than looked up with `rows.find(…)`, because a revealed commit is
   * frequently **not in `rows` at all** — that is the whole `outside` case — and the file list in
   * the details pane still has to be able to open a file from it. A lookup that returns
   * `undefined` there is a double-click that silently does nothing.
   */
  const [selectedRepo, setSelectedRepo] = useState<RepoId | null>(null)
  /**
   * The repository the **second** endpoint belongs to. (M20)
   *
   * The same argument as `selectedRepo` one line up, one step further: *Compare with…* resolves a
   * revspec that need not be on the loaded page at all, so there is no row to look it up on. It
   * is also the only way to know that a merged walk's two selected rows came from two roots,
   * which is the one refusal this feature has — see `CROSS_REPO_COMPARE`.
   */
  const [secondaryRepo, setSecondaryRepo] = useState<RepoId | null>(null)
  /**
   * The commit *Compare with working tree* was invoked on, or `null`.
   *
   * Not part of `LogSelection`, and the alternative was seriously considered: a sentinel oid for
   * the working tree, so that one selection covered every comparison. It loses on two counts.
   * `RevSide::WorkingTree` is legal **only** as the newer side, so `comparePair` — whose whole job
   * is to order the two by the page — would have to special-case a value that is in no page and
   * has no date, and a ⇄ Swap would build the one pairing `cide_git::revision` refuses by name.
   * And an oid field holding something that is not an oid is the kind of value that eventually
   * reaches the wire.
   *
   * It is mutually exclusive with a two-row selection by construction rather than by priority:
   * setting it collapses the selection to the row it was invoked on, and every gesture that makes
   * a pair clears it. One derivation below reads both and there is nothing to arbitrate.
   */
  const [workingTreeOf, setWorkingTreeOf] = useState<string | null>(null)
  /** The range on screen, `null` while it is in flight or when there is no comparison. */
  const [range, setRange] = useState<RevisionRange | null>(null)
  /** Why there is no range: a refusal from Rust, or a cross-repository pair. */
  const [rangeNote, setRangeNote] = useState<string | null>(null)
  /** The *Compare with…* overlay, or `null`. Holds the repository its revspec resolves against. */
  const [compareWith, setCompareWith] = useState<{ repo: RepoId; from: string } | null>(null)
  const [detail, setDetail] = useState<CommitDetail | null>(null)
  const [reveal, setReveal] = useState<LogReveal>(NO_REVEAL)
  const [busy, setBusy] = useState(true)
  const [failed, setFailed] = useState<string | null>(null)
  const generation = useRef(0)
  /** The throttle's timer. `null` means no refresh is armed — see [`GIT_REFRESH_MS`]. */
  const gitTick = useRef<ReturnType<typeof setTimeout> | null>(null)
  /**
   * The last reveal this tab answered, kept so *Clear filters and find it* knows what to re-ask.
   *
   * A ref and not state: nothing renders from it, and putting it in state would re-render the
   * whole list on a value that only a button's `onClick` ever reads.
   */
  const lastReveal = useRef<RevealRequest | null>(null)
  // Captured once per load rather than read per render: `when` only needs to know which year is
  // "this" one, and a `Date.now()` in the render path would make every row's output depend on
  // when React happened to re-run it.
  const [now, setNow] = useState(() => Date.now())

  // `null` while the answer is outstanding, which is *not* the same as "no repositories": the
  // first is a reason to wait and the second is a sentence. Distinguishing them is what stops the
  // tab reporting "this project has no git repository" for the first frame of every open.
  const [repos, setRepos] = useState<readonly RepoInfo[] | null>(null)
  const [branchNames, setBranchNames] = useState<readonly string[]>([])

  const [typed, setTyped] = useState<LogFilter>(NO_FILTER)
  const [applied, setApplied] = useState<LogFilter>(NO_FILTER)
  // Bumped by ↻. A counter and not a boolean, because two refreshes in a row are two requests and
  // a boolean that is already `true` is the second one silently doing nothing.
  const [refreshes, setRefreshes] = useState(0)

  // --- the project's repositories and branches ---------------------------------------------
  //
  // Asked once per project, over the wire, and never derived from the workspace mirror — the
  // reason `client.ts` gives for `git.repos` is the one that matters here: `ProjectRoot` used to
  // carry a `repo` field that Rust never filled, and every consumer that believed it was wrong
  // for every project ever opened. Whether a project has a repository is a fact about the disk
  // that a `git init` in a bash pane changes.
  //
  // Two calls rather than one because they are the authorities on two different things.
  // `git.repos` is the list `git_log` itself resolves the scope against, so it is the one that
  // can decide `One` versus `Merged`; `branch.list` skips repositories it cannot read, which is
  // right for a picker and wrong for a scope.
  useEffect(() => {
    let live = true
    setRepos(null)
    setBranchNames([])
    void git
      .repos(project)
      .then((list) => {
        if (live) setRepos(list)
      })
      .catch(() => {
        // An empty list is the honest degradation: the tab then says the project has no
        // repository, which is what a failed discovery cannot be distinguished from anyway.
        if (live) setRepos([])
      })
    void branch
      .list(project)
      .then((lists) => {
        if (live) setBranchNames(namesOf(lists))
      })
      // A branch list that fails costs the picker its named options and nothing else — the log
      // still walks HEAD. Silent on purpose: a toast for a control the user has not touched.
      .catch(() => {})
    return () => {
      live = false
    }
  }, [project])

  /*
   * The scope, in two halves so that its *identity* is stable.
   *
   * The fetch effect below is keyed on `scope`, so a new object means a new question and a list
   * that starts again from the top. A single `useMemo` over `repos` would mint one when the
   * repository list lands — which a History tab does not care about at all, and which would show
   * as its rows blinking away and coming back a moment after the tab opened. `repoKey` is a
   * string, so the merged half only changes when the set of repositories really does.
   *
   * **`Merged` with one repository is not the same as `One`.** `cide_git::log::graph_off` tests
   * `matches!(scope, Merged { .. })` and turns the graph off for *any* merged scope, count
   * ignored — correctly, because a merged walk interleaves DAGs that share no ancestry. So a
   * single-root project asked for as `Merged { repos: [id] }` would silently lose its graph, and
   * an *empty* `Merged { repos: [] }` — which the app layer expands to every repository — would
   * lose it too. Picking `One` whenever there is exactly one root is what gives the ordinary
   * single-repository project its lanes back.
   */
  const repoKey = repos === null ? null : repos.map((info) => info.id).join(' ')
  const oneScope = useMemo<LogScope | null>(
    () => (repo === null ? null : { kind: 'one', repo }),
    [repo],
  )
  const mergedScope = useMemo<LogScope | null>(() => {
    if (repoKey === null || repoKey === '') return null
    const ids = repoKey.split(' ')
    const first = ids[0]
    if (ids.length === 1 && first !== undefined) return { kind: 'one', repo: first }
    return { kind: 'merged', repos: ids }
  }, [repoKey])
  const scope = oneScope ?? mergedScope

  // --- the debounce -------------------------------------------------------------------------
  //
  // One timer, restarted on every change to `typed`, so a word typed at speed costs one request
  // rather than one per letter. The branch control is applied at once: it is a discrete choice
  // rather than a keystroke, and a fifth of a second between clicking a branch and the list
  // moving reads as the click having missed.
  useEffect(() => {
    if (sameFilter(typed, applied)) return
    const immediate = typed.author === applied.author && typed.text === applied.text
    const timer = setTimeout(() => setApplied(typed), immediate ? 0 : FILTER_DEBOUNCE_MS)
    return () => clearTimeout(timer)
  }, [typed, applied])

  const load = useCallback(
    (more: boolean, keepDepth = 0) => {
      if (scope === null) return
      const tag = more ? generation.current : ++generation.current
      setBusy(true)
      if (!more) {
        setFailed(null)
        setNow(Date.now())
      }
      const cursor =
        more && page?.resume
          ? ({ kind: 'resume' as const, token: page.resume })
          : ({ kind: 'newest' as const })
      void logWalk
        .page(
          project,
          tab,
          logQuery(scope, {
            path,
            cursor,
            follow: path !== null,
            /*
             * How deep to re-walk, and it is only ever non-zero on a **soft refresh**.
             *
             * > *"'Load more' in git panel not working, it cycle me through same commits, i'm not
             * > able to list to first commit."*
             *
             * A soft refresh re-asks the same question and replaces the rows. That is right for
             * one page and silently destructive for five: it fetched the *first* page and threw
             * away everything the user had paged to. On a tree several agents are writing to,
             * `gitRefsMoved` fires constantly, so the list snapped back to the top between
             * clicks — load more, reset to fifty, load more, reset to fifty. From the outside
             * that is a list cycling through the same commits with no way to reach the bottom.
             *
             * Asking for as many rows as are already shown is the honest repair: one request,
             * the same question, the depth the user chose. Zero means "Rust's default", which is
             * what a first load and a *Load more* both want — see `logQuery`.
             *
             * One residual, stated rather than discovered: `log::LIMIT_MAX` is 1000, so a reader
             * who has paged past a thousand rows loses the excess on the next refresh. Ten
             * deliberate clicks to reach, one request to restore nine hundred and ninety-nine of
             * them, and the alternative is chaining requests inside a refresh nobody asked for.
             */
            ...(keepDepth > 0 ? { limit: keepDepth } : {}),
            // Spread last so the bar wins over the defaults, and built by the model so the
            // needle the wire is asked for and the needle `matchesText` uses cannot diverge.
            ...filterOverrides(applied),
          }),
        )
        .then((next) => {
          if (generation.current !== tag) return
          setRows((prev) => (more ? mergePage(prev, next) : next.commits))
          setGraph((prev) =>
            next.graph.kind === 'rows'
              ? more
                ? [...prev, ...next.graph.rows]
                : next.graph.rows
              : [],
          )
          setGraphOff(next.graph.kind === 'off' ? graphOffReason(next.graph.reason) : null)
          setPage(next)
        })
        .catch((error: unknown) => {
          if (generation.current !== tag) return
          setFailed(explain(error))
        })
        .finally(() => {
          if (generation.current === tag) setBusy(false)
        })
    },
    [project, scope, path, applied, page],
  )

  /**
   * What this tab is currently asking. Everything except `refreshes`.
   *
   * The effect below fires for two different reasons and they want opposite treatment, which is
   * what this separates. A **new question** — another project, scope, path or filter — must
   * throw the old answer away: it is about something else, and leaving it on screen under a new
   * heading is worse than an empty list. A **refresh** is the *same* question asked again, and
   * throwing the answer away there is what produced the reported bug: the list emptied and
   * repopulated on every burst, so it read as flickering, and the selection went with it.
   */
  const questionKey = useMemo(
    () => JSON.stringify([project, scope, path, applied]),
    [project, scope, path, applied],
  )
  const lastQuestion = useRef<string | null>(null)

  // Keyed on the question, not on the tab: a History tab that is re-pointed at another file is
  // the same component asking a different thing, and re-fetching is exactly right. `applied` is
  // in here for the same reason — a filter is a new question and must start the list again from
  // the top rather than appending to the previous answer. `refreshes` is in here too, but it is
  // the one dependency that does *not* make it a new question; see `soft`.
  useEffect(() => {
    generation.current += 1
    /*
     * The same question again: keep what is on screen while the new answer is fetched.
     *
     * `logStatus` already covers the rest of it — `loading && rows === 0` is the only path to
     * *Reading the log…*, and `rows > 0` returns no status at all — so leaving the rows in place
     * means the reload is invisible until it lands and then swaps in one paint. React reconciles
     * an unchanged row to nothing, which is why a refresh that finds no new commits now costs
     * exactly zero visible change instead of a full clear and repaint.
     *
     * This matters more than it looks because of what triggers a refresh. `gitRefsMoved` returns
     * true for any *truncated* burst — it cannot know what was dropped from the path list — and
     * a tree with several agents writing to it truncates constantly. Under that load the old
     * code cleared the list about once a second.
     */
    const soft = lastQuestion.current === questionKey
    lastQuestion.current = questionKey
    if (soft) {
      // Re-walk to the depth already on screen rather than to the first page. `rowsRef` and not
      // `rows`, because this effect must not re-run when the list grows — depending on the row
      // count here would make every *Load more* trigger a refresh of everything it just added.
      load(false, rowsRef.current.length)
      return
    }
    setRows([])
    setGraph([])
    setPage(null)
    setSelection(NO_SELECTION)
    setSelectedRepo(null)
    // The comparison goes with the selection it was made from. A range held across a new walk
    // would be a pane describing two commits that are no longer selected — and, after a branch
    // filter, no longer necessarily in the list the user is looking at.
    setSecondaryRepo(null)
    setWorkingTreeOf(null)
    setRange(null)
    setRangeNote(null)
    setDetail(null)
    // The reveal's answer is about a page that is being thrown away, so the note goes with it.
    // *Clear filters and find it* re-parks its request before changing the filter, so the answer
    // for the new page is recomputed rather than carried over — see `onFindCommit`.
    setReveal(NO_REVEAL)
    if (scope === null) return
    load(false)
    // `load` closes over `page` so that "load more" can read the resume token; depending on it
    // here would re-run this effect on every page and restart the list from the top.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project, scope, path, applied, refreshes])

  /*
   * Drop a selection whose commit is no longer in the list.
   *
   * The soft refresh above keeps the selection across a reload, which is the point of it — the
   * row you were reading is almost always still there, and losing it once a second was the
   * complaint. Almost always is not always: an amend, a rebase or a reset replaces the oid, and
   * a filter narrowing under a reveal can drop it too. A selection naming a row that is not
   * drawn is worse than none, because the details pane keeps describing a commit the list no
   * longer shows and the next Shift-click extends a range from an invisible anchor.
   *
   * Both ends, independently: a pair whose older side survived a rewrite and whose newer side
   * did not is still a usable one-commit selection, and clearing both would throw away a row
   * that is on screen and correct.
   */
  useEffect(() => {
    if (rows.length === 0) return
    const present = new Set(rows.map((r) => r.oid))
    setSelection((current) => {
      const primary = current.primary !== null && present.has(current.primary) ? current.primary : null
      const secondary =
        current.secondary !== null && present.has(current.secondary) ? current.secondary : null
      if (primary === current.primary && secondary === current.secondary) return current
      // The anchor is what a range is measured from, so a pair that lost its primary collapses
      // onto whatever is left rather than becoming a range with one end.
      return primary === null
        ? { ...current, primary: secondary, secondary: null }
        : { ...current, primary, secondary }
    })
  }, [rows])

  /*
   * Stop this tab's walk when the tab goes away.
   *
   * `ToolWindowHost.onClose` already cancels when a History tab is *closed*, and this is the
   * other half: hiding the panel, switching to another tab, switching project and closing the
   * window all unmount this component without closing anything, and each of them leaves a walk
   * over a deep repository running with nobody left to read the answer.
   *
   * Deliberately its own effect with an empty dependency list, so it runs exactly once on
   * unmount. Folding it into the fetch effect's cleanup above would fire it on every filter
   * keystroke — cancelling the walk the very next statement is about to supersede anyway, which
   * is one wasted round trip per character and, worse, a cancel racing the `page` that replaces
   * it: the registry cancels the *standing* job, and the two calls have no ordering between them.
   *
   * `tab` and `project` are read from a ref-free closure because neither can change without the
   * key in `ToolWindowHost` changing too — a remount, which is precisely this cleanup running.
   */
  useEffect(() => {
    return () => {
      void logWalk.cancel(project, tab).catch(() => {})
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // The flag a request's `finally` would have cleared, for the one case where no request is made.
  // A project with no git repository never reaches `load`, and a tab left busy shows
  // "Reading the log…" for ever — which is the worst of the empty states to be stuck in, because
  // it is the one that promises an answer is coming.
  useEffect(() => {
    if (repos !== null && repos.length === 0) setBusy(false)
  }, [repos])

  /*
   * --- git moved underneath -------------------------------------------------------------------
   *
   * A `git commit`, a `git checkout` or a rebase in a bash pane, or an agent's own commit, moves
   * the history this list is a view of. Until this subscription existed the ↻ button was the only
   * way to notice, which is the single most obvious way a read-only panel looks broken.
   *
   * # Two signals arrive for one cide-initiated mutation, and both are tolerated
   *
   * Every mutation cide makes broadcasts `cide://git-status` **immediately** and then produces a
   * `cide://fs-changed` roughly 300 ms later, when the watcher's coalescer goes quiet. This panel
   * subscribes only to the second — it is the one that also covers changes cide did not make,
   * which is the case that matters here — and the throttle below is what keeps the pair from
   * being two walks. A panel that answered both would double every refresh for no new information.
   *
   * # A throttle, not a debounce, and no second coalescer
   *
   * `cide-fs` already holds a burst until the tree is quiet (`quiet: 300ms`, `max_wait: 2s`), so
   * what arrives here is a burst and not a stream of events. What is added is a *throttle*: the
   * first trigger of a group starts one timer and later triggers inside the window are absorbed.
   * Deliberately not a debounce that restarts on every trigger — `chrome/gitCountStore.ts` and
   * `editor/docSync.ts` have both written down the trap, which is that a steady writer (a
   * `cargo watch`, a fetch loop) starves it indefinitely and the panel freezes for exactly as long
   * as something is happening.
   *
   * # And only for a ref, not for the index
   *
   * `logModel::gitRefsMoved` is the predicate, and it is a pure exported function so
   * `check-log.mjs` can drive its four cases. `FsChange.git` covers the index too, and a
   * `git add` — or a `git status` in a loop — must not restart a frontier walk.
   */
  useEffect(() => {
    const stop = events.onFsChanged((changed: ProjectId, change) => {
      if (changed !== project) return
      if (!gitRefsMoved(change)) return
      if (gitTick.current !== null) return
      gitTick.current = setTimeout(() => {
        gitTick.current = null
        // Through the same counter ↻ uses, so there is one refresh path and not two. It restarts
        // the list from the top and drops the selection with it — which is what ↻ does, and the
        // honest answer: page one after a commit is a different page, and re-selecting by oid
        // would only work for a row that is still inside it.
        setRefreshes((n) => n + 1)
      }, GIT_REFRESH_MS)
    })
    return () => {
      void stop.then((off: () => void) => off())
      if (gitTick.current !== null) {
        clearTimeout(gitTick.current)
        gitTick.current = null
      }
    }
  }, [project])

  /**
   * The loaded page's oids, newest first — what decides which of two selected commits is older.
   *
   * Derived from `rows` (the walk's answer) and not from `visible` (the same list with the
   * instant text narrow applied), because the narrow is a *display* filter that comes and goes
   * within one keystroke and the orientation of a diff must not.
   *
   * # Keyed on the joined oids, and that is a bug fix rather than a micro-optimisation
   *
   * > *"when in git log selecting several commits - right panel begin flickering"*
   *
   * `useMemo(() => rows.map(…), [rows])` looks stable and is not: a soft refresh calls
   * `setRows(next.commits)` with a **fresh array off the wire** even when the walk found exactly
   * the same commits, so `rows` changes identity, so `order` does, so `pair` does, so `compare`
   * does — and the effect that fetches the range clears it to *Reading…* and asks again. On a
   * tree several agents are writing to, that is once a second, which is what the flicker was.
   *
   * The join is what breaks the chain: same oids in the same order means the same string, so
   * `order` keeps its identity and nothing downstream re-runs. Newline-joined because an oid is
   * hex and cannot contain one, so the split is exact rather than approximately exact.
   */
  const orderKey = rows.map((r) => r.oid).join('\n')
  const order = useMemo(() => (orderKey === '' ? NO_ORDER : orderKey.split('\n')), [orderKey])

  /**
   * A click on a row, with whatever modifiers it carried.
   *
   * `logModel::selectRow` owns the rule and this owns the consequences: which repositories the
   * two endpoints came from, and whether the detail request is still the right one to make.
   *
   * The detail is fetched for the **anchor** on every gesture, including the ones that make a
   * pair. That is not waste: the anchor's detail is what the pane falls back to the moment one of
   * the two rows is deselected, and re-fetching it then would put a "Reading the commit…" flash
   * in the middle of a gesture that only removed something. `git_commit_detail` is one object
   * lookup against a warm odb.
   *
   * `rows` and not `visible`: the reducer's `order` has to be the page's own order. The local text
   * narrow drops rows without renumbering the walk, and deciding "which of these two is older"
   * against a list two keystrokes are about to replace would give a different answer for the same
   * two commits depending on what was in the filter box.
   */
  const onSelect = useCallback(
    (oid: string, mods: SelectMods) => {
      const next = selectRow(selection, oid, mods, order)
      setSelection(next)
      // A comparison against the working tree is a one-row state; any gesture that touches the
      // selection is the user leaving it.
      setWorkingTreeOf(null)
      setDetail(null)
      // A click on a row answers whatever the reveal note was saying: the user has moved on to a
      // different commit, and a sentence about the old one over the new one's detail is a lie.
      setReveal(NO_REVEAL)

      // The second endpoint's repository, resolved now while the row is in hand. `null` when the
      // gesture left only one commit selected, which is also what clears the cross-repo refusal.
      const repoOf = (want: string | null) =>
        want === null ? null : (rows.find((r) => r.oid === want)?.repo ?? null)
      setSecondaryRepo(repoOf(next.secondary))

      const anchor = next.primary
      if (anchor === null) {
        setSelectedRepo(null)
        return
      }
      const row = rows.find((r) => r.oid === anchor)
      if (!row) return
      setSelectedRepo(row.repo)
      const tag = generation.current
      void gitLog
        .detail(project, row.repo, anchor)
        .then((d) => {
          if (generation.current === tag) setDetail(d)
        })
        .catch(() => {})
    },
    [project, rows, order, selection],
  )

  /*
   * --- answering a reveal ----------------------------------------------------------------------
   *
   * Three outcomes, and all three have to be handled, because the *common* one is the second.
   * A blame line is usually old: the commit that last touched it can be thousands of rows below
   * page one, and it is certainly below a filtered list. A reveal that only knew how to select a
   * loaded row would, in the ordinary case, open a log that visibly does not contain the commit
   * it was about.
   *
   *   1. loaded → select it; `LogView` scrolls it into view.
   *   2. not loaded → fetch `git_commit_detail` for it directly and show that, with a note saying
   *      where it is not and a button that re-roots the walk at it.
   *   3. it does not resolve → say so.
   *
   * # Only the Log tab answers, and only once its first page has landed
   *
   * `path !== null` is a History tab, which is a different question — and it may well be the
   * mounted one at the instant the request is parked, since the caller's `tool_window_activate`
   * has to round-trip through Rust and back before the Log tab exists. A History tab that
   * consumed the request would spend it and leave the Log tab, mounting a moment later, with
   * nothing to do.
   *
   * `busy` is the other guard and it is the subtler one: consuming while the first page is still
   * in flight means deciding "not loaded" against an empty `rows`, which would show the
   * out-of-filter note for a commit that is about to appear at the top of the list. The request
   * stays parked instead; this effect re-runs when the page lands.
   *
   * The unsettled-filter guard is the third, and without it *Clear filters and find it* would not
   * work at all. That button re-parks a request and *then* changes the filter, and the filter
   * change is debounced — so for one render the request is outstanding while `rows` is still the
   * answer to the **previous** question. Answering there would spend the request on the page the
   * user pressed the button to get away from, and the walk that was about to start would land on
   * a tab with nothing left to reveal.
   */
  const pending = useRevealRequests((s) => s.pending)
  useEffect(() => {
    if (pending === null || pending.project !== project) return
    if (path !== null) return
    if (busy || !sameFilter(typed, applied)) return
    useRevealRequests.getState().clear(pending)
    // Stale: parked for a Log tab that never opened, and now being spent on an unrelated visit.
    if (Date.now() - pending.at > REVEAL_TTL_MS) return
    lastReveal.current = pending

    const { oid, repo } = pending
    // Parked before either branch moves the selection, because moving it is what triggers the
    // effect that reads this. Both paths end in a selection change, so one assignment covers
    // the commit-is-on-the-page case and the fetch-it-separately case alike.
    pendingFile.current = pending.file
    const row = rows.find((r) => r.oid === oid || (oid.length >= 4 && r.oid.startsWith(oid)))
    if (row !== undefined) {
      // A reveal is a *plain* selection, however the log happened to be selected before it. The
      // gesture is "show me this commit", and answering it by adding a second endpoint to a pair
      // the user set up minutes ago in another tool window would show them a range nobody asked
      // for. `NO_MODS` is the way to say that in the one vocabulary the reducer has.
      onSelect(row.oid, NO_MODS)
      // After `onSelect`, which clears the reveal: the two are one gesture and the *found* state
      // is the part `LogView` scrolls on.
      setReveal({ kind: 'found', oid: row.oid })
      return
    }

    selectOnly(oid)
    setSelectedRepo(repo)
    setSecondaryRepo(null)
    setWorkingTreeOf(null)
    setDetail(null)
    setReveal({ kind: 'looking', oid })
    const tag = generation.current
    void gitLog
      .detail(project, repo, oid)
      .then((d) => {
        if (generation.current !== tag) return
        setDetail(d)
        setReveal({ kind: 'outside', oid })
      })
      .catch(() => {
        if (generation.current !== tag) return
        // Not distinguished by `GitError` kind. Every way this fails — a bad oid, a repository
        // that no longer has the object after a gc, a refusal — reaches the user as the same
        // next move, which is "this is not the commit you can look at here".
        selectOnly(null)
        setSelectedRepo(null)
        setReveal({ kind: 'missing', oid })
      })
  }, [pending, project, path, busy, typed, applied, rows, onSelect, selectOnly])

  /**
   * *Clear filters and find it*.
   *
   * Two halves, and the order matters: the request is re-parked **first**, because setting the
   * filter runs the reset effect above, which clears the reveal and starts a new walk — and the
   * parked request is what selects the row when that walk's answer lands. Re-parking rather than
   * keeping the note alive is what makes this one mechanism instead of two: the answer for the
   * new page is computed the same way the answer for the old one was.
   */
  const onFindCommit = useCallback(() => {
    const asked = lastReveal.current
    if (asked === null) return
    requestLogReveal(asked.project, asked.repo, asked.oid)
    setTyped(findCommitFilter(asked.oid))
  }, [])


  /**
   * A click on a file in the commit's changed-file list: show it as that commit left it.
   *
   * # Only a double-click opens, and this pane does not share the git tree's rule
   *
   * > *"One click - selection, double click - open diff."*
   * > *"Still on one click it opens diff"*
   *
   * It used `gitTreeClick`, which opens on a single click whenever a diff is already on screen.
   * That is right for the git *commit* panel — reading down your own changes with the diff pane
   * beside you — and wrong here, which is what the second report above says. This list is a
   * reading surface: the diff arrives as a **tab**, and the user is looking *for* a file rather
   * than *at* each one in turn. Under the old rule the first double-click opened a tab and every
   * single click after it silently replaced that tab, so scanning the list kept throwing diffs
   * into the editor.
   *
   * `logFileClick` is therefore its own rule, in `sidebar/clickSemantics.ts` beside the one it
   * differs from, so the two are visible together and one check can run both.
   *
   * The double-fire still has to be handled: the browser sends `click` twice for a double-click,
   * with `detail` 1 and then 2. The first is now a plain select, so nothing opens until the
   * second — which is exactly the behaviour asked for.
   *
   * # The rest is unchanged from the double-click-only version
   *
   * It lands in the *revision* preview slot, which is deliberately a different slot from the
   * working-tree one: the changes tree and this list are on screen at the same time, and one slot
   * would mean clicking here throwing away the diff the user was staging from.
   * `cide_core::workspace::PreviewSlot` derives the slot from the origin and `retarget_diff`
   * refuses to move a tab across families.
   *
   * `FirstParent` and not the parent's oid: it is what "show me this commit" means, it is
   * resolved at fetch time, and it keeps a parent oid that nothing chose out of the persisted
   * layout.
   *
   * `selectedRepo` and not `rows.find(…)`: a revealed commit need not be in `rows` at all, and a
   * lookup that missed would make the file list of an out-of-page commit silently inert.
   */
  const openRevisionFile = useCallback(
    (
      repo: RepoId,
      next: RevSide,
      prev: RevSide,
      file: string,
      oldPath: string | null,
      detailCount: number,
    ) => {
      const gesture = gestureOf(detailCount)
      const action = logFileClick({ gesture, expandable: false })
      if (!action.open) return
      // `oldPath` is what makes a rename diff against the right blob: under the older side the
      // file is at its *old* path, and asking for the new one there answers "added, whole file".
      // The row already knows — `CommitFile.oldPath` and `RevisionChange.oldPath` are non-null
      // exactly for a rename — so passing it costs nothing and not passing it turns every rename
      // into a wall of green.
      /*
       * Always a kept tab now, never a retarget.
       *
       * `diffOpenMode` maps *single* to `retarget`, and a single click no longer opens anything
       * — so the only gesture reaching this line is the double-click, whose answer was `open`
       * under the old rule too. Keeping the branch would be unreachable code that looked
       * load-bearing.
       */
      void revisionDiff.openTab(project, repo, file, next, prev, oldPath).catch(() => {})
    },
    [project],
  )

  /*
   * How the details pane lists its files, and the control that changes it. (M21)
   *
   * Read from the workspace mirror rather than a `useState`, for the reason every other piece of
   * panel furniture is: the value is persisted on `Project.tool_window`, two windows can be
   * showing the same project, and a local copy would let them disagree until one of them
   * happened to re-read. The write goes straight to Rust and the answer comes back as a
   * `workspace-changed` snapshot — the same round trip the rail button and the divider make.
   *
   * The selector returns a **boolean**, which is what keeps it safe: a fresh object here would
   * re-render for ever and end at *Maximum update depth exceeded*. See `check:selectors`.
   */
  const filesAsTree = useWorkspace(
    (s) => s.boot?.workspace.projects[project]?.toolWindow.filesAsTree ?? true,
  )
  const onToggleTree = useCallback(
    (next: boolean) => {
      void toolWindow.setLayout(project, { filesAsTree: next }).catch(() => {})
    },
    [project],
  )

  const onOpenFile = useCallback(
    (file: string, oldPath: string | null, detailCount: number) => {
      if (selected === null || selectedRepo === null) return
      const next: RevSide = { kind: 'commit', oid: selected }
      const prev: RevSide = { kind: 'firstParent' }
      openRevisionFile(selectedRepo, next, prev, file, oldPath, detailCount)
    },
    [selected, selectedRepo, openRevisionFile],
  )

  /* ------------------------------------------------------------------------------------------
   * Comparing two revisions (M20)
   * --------------------------------------------------------------------------------------- */

  /** The pair the selection is holding, ordered by the page. `null` for a one-row selection. */
  const pair: ComparePair | null = useMemo(
    () => comparePair(selection, order),
    [selection, order],
  )

  /**
   * Everything the details pane needs to show a comparison, or `null` for a single commit.
   *
   * **One derivation and not two branches at the render site.** There are two gestures that put a
   * range in that pane — a two-row selection and *Compare with working tree* — and they differ in
   * exactly three values: what the newer side is, what it is called, and whether ⇄ Swap is
   * offered. Deciding that once, here, is what stops the pane growing a second header, a second
   * file list and a second click rule; `check-log-render.mjs` then has one component to render
   * and the working-tree case is the same markup with a different label in it.
   *
   * The cross-repository refusal is resolved here rather than at the fetch, because the menu
   * needs the same answer at the same moment — `commitMenu` draws *Compare* disabled with
   * `CROSS_REPO_COMPARE` on it — and two independent computations of "are these the same
   * repository" is how a disabled menu item and a pane full of files end up on screen together.
   */
  const compare: CompareView | null = useMemo(() => {
    if (selectedRepo === null) return null
    if (pair !== null) {
      // Both endpoints must be in one object database. Under a merged scope they need not be,
      // and `git_diff_revision_files` takes a single `repo` — there is no request to make.
      const sameRepo = secondaryRepo === null || secondaryRepo === selectedRepo
      return {
        repo: selectedRepo,
        next: { kind: 'commit', oid: pair.newer },
        prev: { kind: 'commit', oid: pair.older },
        newerLabel: pair.newer,
        olderLabel: pair.older,
        swappable: true,
        refusal: sameRepo ? null : CROSS_REPO_COMPARE,
      }
    }
    if (workingTreeOf !== null) {
      return {
        repo: selectedRepo,
        next: { kind: 'workingTree' },
        prev: { kind: 'commit', oid: workingTreeOf },
        newerLabel: WORKING_TREE_LABEL,
        olderLabel: workingTreeOf,
        swappable: false,
        refusal: null,
      }
    }
    return null
  }, [pair, selectedRepo, secondaryRepo, workingTreeOf])

  /**
   * Fetch the range whenever the comparison changes.
   *
   * Tagged with the same `generation` counter every other request in this file carries, and for
   * the same reason: `spawn_blocking` cannot be cancelled, so a superseded range finishes in Rust
   * and its answer has to be dropped on this side. Without it, ⇄ Swap on a large range paints the
   * *previous* orientation over the new one whenever the second request happens to land first.
   *
   * `setRange(null)` before the request rather than leaving the old list up: the header above it
   * has already changed to the new pair, and a file list from the previous one under it would be
   * a wrong answer presented as a current one. `RANGE_LOADING` fills the gap.
   */
  useEffect(() => {
    if (compare === null) {
      setRange(null)
      setRangeNote(null)
      return
    }
    setRange(null)
    setRangeNote(compare.refusal)
    if (compare.refusal !== null) return
    const tag = generation.current
    let live = true
    void gitLog
      .diffFiles(project, compare.repo, compare.next, compare.prev)
      .then((answer) => {
        if (!live || generation.current !== tag) return
        setRange(answer)
      })
      .catch((error: unknown) => {
        if (!live || generation.current !== tag) return
        // Through `explain`, like every other refusal on this page. A `GitError` is a tagged
        // object and `String(error)` on one is the literal text `[object Object]`.
        setRangeNote(explain(error))
      })
    return () => {
      live = false
    }
    // `compare` is a fresh object per render only when one of its inputs changed — it comes out
    // of a `useMemo` over the four values that decide it — so this is not a request per render.
  }, [project, compare])

  /** A click on a file in the *range's* list. Same rule, the range's two sides. */
  const onOpenRangeFile = useCallback(
    (file: string, oldPath: string | null, detailCount: number) => {
      if (compare === null) return
      openRevisionFile(compare.repo, compare.next, compare.prev, file, oldPath, detailCount)
    },
    [compare, openRevisionFile],
  )

  /** ⇄ Swap. One boolean in the selection; the effect above re-asks. */
  const onSwapSides = useCallback(() => {
    setSelection((current) => swapPair(current))
  }, [])

  /**
   * *Compare with working tree*: this commit against the files on disk.
   *
   * Collapses the selection onto the row first, which is what makes this and a two-row comparison
   * mutually exclusive states rather than two flags the derivation above would have to rank. It
   * also re-fetches the row's detail, so deselecting the working tree leaves the pane on the
   * commit the user was standing on rather than on "Reading the commit…".
   */
  const compareWorkingTree = useCallback(
    (repo: RepoId, oid: string) => {
      setSelection({ primary: oid, secondary: null, swapped: false })
      setSelectedRepo(repo)
      setSecondaryRepo(null)
      setWorkingTreeOf(oid)
      setReveal(NO_REVEAL)
      setDetail(null)
      const tag = generation.current
      void gitLog
        .detail(project, repo, oid)
        .then((d) => {
          if (generation.current === tag) setDetail(d)
        })
        .catch(() => {})
    },
    [project],
  )

  /**
   * *Compare with…* answered: a revspec, resolved to oids **before** anything stores it.
   *
   * `git_resolve_rev` is the whole point of the round trip. A tab — or a comparison — that kept
   * the string `main` would name a different tree tomorrow, which is exactly the staleness
   * `DiffSpec` refuses to carry, and it is why the command exists.
   *
   * A **range** spec (`a..b`, `a...b`) fills both ends at once. `ResolvedRev.range` says so
   * explicitly rather than being inferred from `to` being present, because the two forms produce
   * different views and the flag is the backend's own answer to which one was typed. `to` is the
   * newer end by git's own convention, so it becomes the anchor.
   *
   * Setting the second endpoint goes through `setSelection` and nothing else — the same state a
   * Ctrl+click writes — so the pane, the row highlights and the menu all reach the two-row state
   * by one path. That was the alternative that lost: a separate "compared against" field would
   * have needed its own clearing rule on every gesture that changes the selection, and the first
   * one to be forgotten would leave a stale endpoint in a header.
   */
  const applyCompareWith = useCallback(
    (repo: RepoId, anchor: string, resolved: { from: string; to: string | null; range: boolean }) => {
      setWorkingTreeOf(null)
      setSecondaryRepo(repo)
      setSelectedRepo(repo)
      setReveal(NO_REVEAL)
      setDetail(null)
      /*
       * The anchor is the **right-clicked row**, and it is passed in rather than read out of the
       * current selection.
       *
       * A right-click does not select — `rowFor` resolves the row under the pointer, which is the
       * difference between a menu that acts on what you pointed at and one that acts on whatever
       * was last clicked. So `selection.primary` here is very often some *other* commit, and using
       * it would compare the typed revision against a row the user was not looking at, with a
       * header naming two commits neither of which they chose.
       */
      const primary = resolved.range && resolved.to !== null ? resolved.to : anchor
      setSelection({ primary, secondary: resolved.from, swapped: false })
      // The anchor's own detail, so that removing one endpoint falls straight back to a commit
      // rather than to "Reading the commit…". Asked by oid and not looked up in `rows`, because a
      // range spec's newer end is frequently a revision this repository resolves and this page
      // has never drawn — which is the whole reason *Compare with…* exists.
      const tag = generation.current
      void gitLog
        .detail(project, repo, primary)
        .then((d) => {
          if (generation.current === tag) setDetail(d)
        })
        .catch(() => {})
    },
    [project],
  )

  /**
   * The one-file diff a History tab shows, or `null` for the Log tab. (M21)
   *
   * `path !== null` **is** what makes a tab a History tab — it is the field the query differs by
   * — so it is also the right test for which pane to draw, and no second flag can disagree with
   * it.
   *
   * The two revisions come from `compare` when a pair is selected, so Ctrl+clicking two commits
   * in a file's history diffs the file *between them* rather than showing the newer one's own
   * change. That is the reading a two-row selection already means everywhere else in this panel,
   * and it is the more useful one here: the whole tab is about one file across time.
   *
   * `compare.refusal` is ignored on purpose. It exists for a cross-repository pair, and a
   * History tab is one file in one repository, so the case cannot arise — reading the field here
   * would be a guard against a state the tab's own shape rules out.
   */
  const fileDiff = useMemo(() => {
    if (path === null || selectedRepo === null || selected === null) return null
    return {
      repo: selectedRepo,
      path,
      next: compare?.next ?? ({ kind: 'commit', oid: selected } as RevSide),
      prev: compare?.prev ?? ({ kind: 'firstParent' } as RevSide),
    }
  }, [path, selectedRepo, selected, compare])

  /* ------------------------------------------------------------------------------------------
   * The changed-file menu (M21)
   * --------------------------------------------------------------------------------------- */

  /**
   * Which changed file is selected, and the icon set the rows draw with. (M21)
   *
   * A path and not an index: the list is regrouped by the ⊞ toggle and refetched on every
   * selection change, so an index means the highlight lands on whatever file happens to sit at
   * that position next — a different file, silently.
   *
   * Cleared when the *commit* selection moves, because a path from the previous commit either
   * does not appear in this one or is a different change to the same file. Leaving it would
   * highlight a row the user never picked.
   */
  const [selectedFile, setSelectedFile] = useState<string | null>(null)
  /**
   * A file the *next* selection change should land on, set by a reveal that named one.
   *
   * A ref rather than state, and it exists because the two rules collide. Moving the commit
   * selection clears the file selection — a path from the previous commit is either absent or a
   * different change — and a blame reveal moves the commit selection *in order to* show one
   * file. Without somewhere to park the intent, the reveal would select the file and the effect
   * below would immediately clear it.
   *
   * Consumed exactly once: cleared as it is read, so the next ordinary click on another commit
   * resets to nothing as it always did.
   */
  const pendingFile = useRef<string | null>(null)
  useEffect(() => {
    setSelectedFile(pendingFile.current)
    pendingFile.current = null
  }, [selection])

  /**
   * Which directories are folded in the grouped listing.
   *
   * **Not cleared when the commit changes**, unlike the selection above, and the two differ for
   * a reason. A selected *path* from the previous commit is either absent from this one or a
   * different change to the same file, so keeping it would highlight a row nobody picked. A
   * folded *directory* is a standing statement about an area of the tree — someone who folded
   * `crates/cide-git/src` while reading one commit wants it folded in the next — and the stale
   * entries cost a set lookup that misses.
   *
   * Local and not persisted, unlike the grouped/flat toggle. That one is a reading the user
   * chose; this is closer to a scroll position, and a panel that reopened with three directories
   * mysteriously folded would be hiding files with no visible cause.
   */
  const [collapsedDirs, setCollapsedDirs] = useState<ReadonlySet<string>>(() => new Set())
  const onToggleDir = useCallback((dir: string) => {
    setCollapsedDirs((current) => toggleCollapsed(current, dir))
  }, [])
  /**
   * This tab's divider, in per mille.
   *
   * Read from the tab rather than the project: a History tab keeps its own, because its right
   * half is one file's diff and the Log tab's is a commit summary — see `HistoryTab::split`. The
   * selector returns a **number**, which is what keeps it safe from the re-render loop
   * `check:selectors` exists for.
   */
  const split = useWorkspace((s) => {
    const state = s.boot?.workspace.projects[project]?.toolWindow
    if (state === undefined) return LOG_SPLIT_DEFAULT
    if (state.active === null) return state.logSplit
    const tab = state.history.find((h) => h.id === state.active)
    return tab?.split ?? LOG_SPLIT_HISTORY
  })

  const iconTheme = useIconTheme()

  /**
   * The files the details pane is showing, whichever pane that is.
   *
   * The menu needs two facts the DOM does not carry: the pre-rename path, and whether the commit
   * *deleted* the file. `deleted` comes from `status` on both `CommitFile` and `RevisionChange`
   * rather than from "the file is missing on disk" — that test would be wrong for a file this
   * commit removed and a later one restored, which is the case a reader is most likely to be
   * looking at when they reach for the menu.
   *
   * One list for both panes, chosen the same way the pane itself is chosen, so the menu can never
   * describe the pane that is not on screen.
   */
  const fileRowsShown = useMemo(
    () =>
      compare !== null
        ? (range?.files ?? []).map((f) => ({
            path: f.path,
            oldPath: f.oldPath,
            deleted: f.status === 'deleted',
          }))
        : (detail?.files ?? []).map((f) => ({
            path: f.path,
            oldPath: f.oldPath,
            deleted: f.status === 'deleted',
          })),
    [compare, range, detail],
  )

  /**
   * Run one of the four file actions.
   *
   * Three of them are `openRevisionFile` with a different pair of sides, which is the whole
   * argument for routing them through one function: the sides *are* the action, and a second
   * spelling of "compare with local" is a second place for `RevSide::WorkingTree` to end up on
   * the wrong side of the pair — an arrangement `cide_git::revision` refuses by name, so the
   * mistake surfaces as a typed error rather than as a wrong diff, but only after the round trip.
   *
   * `detailCount: 2` throughout. These come from a menu, and a menu click is a deliberate request
   * for a tab — never the *retarget* that a single click on a row means. Passing the real click
   * count would make a menu item re-point whatever revision diff happened to be open.
   */
  const runFileAction = useCallback(
    (id: FileMenuId, path: string, oldPath: string | null) => {
      if (selectedRepo === null) return
      if (id === 'openFile') {
        /*
         * The working tree, not a revision: this is the one item that is not a diff.
         *
         * Straight to `file.open` and not through `runCommand`, which is a closure local to
         * `App.tsx` and reaches nothing outside it. That is the same seam `revisionDiff.openTab`
         * beside it uses, so the two file gestures in this menu travel the same way.
         *
         * The path is repo-relative and `tab_open_file` wants an absolute one, which is what the
         * repository root is for — the join `GitPanelHost` already makes for *Show file history*.
         */
        const root = repos?.find((r) => r.id === selectedRepo)?.root
        if (root === undefined) return
        void file.open(project, `${root}/${path}`).catch(() => {})
        return
      }
      if (selected === null) return
      const commit: RevSide = { kind: 'commit', oid: selected }
      const before: RevSide = { kind: 'firstParent' }
      const local: RevSide = { kind: 'workingTree' }
      /*
       * Two questions crossed with two answers — see `fileMenu.ts`'s table.
       *
       * `compareLocal` starts at the commit and ends on disk: *what happened to this file after
       * this commit*. `compareBeforeLocal` starts one commit earlier, so the commit's own change
       * is inside the answer: *what has happened since just before it*. Getting these the wrong
       * way round produces a diff that is entirely plausible and answers the other question.
       */
      const sides: Record<Exclude<FileMenuId, 'openFile'>, [RevSide, RevSide]> = {
        showDiff: [commit, before],
        compareLocal: [local, commit],
        compareBeforeLocal: [local, before],
      }
      const pairForId = sides[id]
      openRevisionFile(selectedRepo, pairForId[0], pairForId[1], path, oldPath, 2)
    },
    [project, selected, selectedRepo, repos, openRevisionFile],
  )

  /**
   * The right-click menu on a changed file.
   *
   * Resolved from the DOM target the way the commit menu is, rather than by threading an
   * `onMenu` prop through `LogView`: that file is server-rendered by `check-log-render.mjs` and
   * a menu needs the workspace store, the IPC client and `runCommand`. The row publishes
   * `data-file-path` and this walks up to it.
   */
  const fileMenuHandle = useContextMenu({
    label: 'Changed file',
    items: ({ target }) => {
      const row = target?.closest('[data-file-path]')
      const path = row?.getAttribute('data-file-path') ?? null
      if (path === null) return []
      const known = fileRowsShown.find((f) => f.path === path)
      if (known === undefined) return []
      return fileMenu({
        target: { path, oldPath: known.oldPath },
        deleted: known.deleted,
        // A range already ending at the working tree has nothing to compare *with* the working
        // tree — the answer would be an empty diff, which reads as a failed request rather than
        // as "these are the same".
        newerIsWorkingTree: compare?.next.kind === 'workingTree',
      }).map((item) => ({
        id: item.id,
        label: item.label,
        ...(item.disabledReason === undefined
          ? { run: () => runFileAction(item.id, path, known.oldPath) }
          : { disabledReason: item.disabledReason }),
      }))
    },
  })

  /* ------------------------------------------------------------------------------------------
   * The commit actions
   * --------------------------------------------------------------------------------------- */

  /**
   * The confirmation on screen, or `null`.
   *
   * Held here and not in a store, because `ConfirmDestructive` has no store: `GitPanel.tsx` and
   * `FileTree.tsx` both keep their own `ConfirmState | null` and mount the component themselves.
   * That is what makes every gesture in this feature reachable without a line in `App.tsx` — the
   * note at the foot of `GitPanelHost.tsx` says so for the panel, and it is why the dialog is
   * mounted beside `LogView` below rather than portalled from somewhere central.
   *
   * `chosen` and `optionOn` are separate state and not fields of this, because the dialog is a
   * *controlled* radio group and checkbox: the component asks the caller to hold both so the
   * answer survives to the command. See [`ConfirmState.chosen`].
   */
  const [confirm, setConfirm] = useState<LogConfirm | null>(null)
  const [chosen, setChosen] = useState<string | null>(null)
  const [optionOn, setOptionOn] = useState(SHELVE_FIRST_DEFAULT)
  /** The name prompt on screen — *New branch from here…* or *Tag…* — or `null`. */
  const [prompt, setPrompt] = useState<NamePrompt | null>(null)

  /**
   * The oid `HEAD` points at, per repository, derived from the page itself.
   *
   * Out of the rows and not out of `branch.list`, and the reason is that the two answer
   * different questions: `BranchInfo.head` is a *branch short name* (`main`) and only an
   * abbreviated oid when `HEAD` is detached, while `logActions::isHead` compares against a
   * commit. `CommitRow.refs` carries a `head`-kind chip on exactly the row `HEAD` points at,
   * per repository, computed by the same walk that produced the row — so it is both free and
   * correct in a monorepo, where one project has several tips.
   *
   * A repository whose tip is not on the loaded page — a filtered walk, a branch filter, a tip
   * below the first page — simply has no entry, and [`commitMenu`] is handed `''`. *Amend* then
   * appears on no row of that repository, which is the conservative direction: `cide_git` refuses
   * an amend of anything but the tip by name, so the only cost is a menu line that is missing
   * rather than a menu line that lies.
   */
  const headOids = useMemo(() => {
    const map = new Map<string, string>()
    for (const r of rows) {
      if (map.has(r.repo)) continue
      if (r.refs.some((chip) => chip.kind === 'head')) map.set(r.repo, r.oid)
    }
    return map
  }, [rows])

  /**
   * Report a refusal.
   *
   * Through `chrome/branchModel.ts::explain`, which has an arm for every `GitError` these six
   * commands can raise — and never `String(error)`, which for a `{kind, detail}` object is the
   * literal text `[object Object]`. That is the bug `check:branches` exists for, and it is how a
   * command that refused for a good, stated reason ends up looking like a menu item wired to
   * nothing.
   */
  const report = useCallback((error: unknown) => {
    notify(explain(error), { kind: 'error' })
  }, [])

  /*
   * The action flows, one per menu line that does something.
   *
   * Plain function declarations rather than `useCallback`s, for two reasons that both point the
   * same way. Three of them are **mutually recursive** — a replay retries itself with a mainline,
   * a detach re-raises its own dialog with the blockers, a tag retries itself with `force` — and
   * hoisted declarations make that legal without a ref to route the recursion through. And none
   * of them is a prop or a hook dependency: they are called from `items`, which `useContextMenu`
   * invokes at open time, and from dialog callbacks that were created in the same render.
   */

  /** *Revert* / *Cherry-pick*. No dialog on the way in — see [`replayRequestFor`]. */
  function runReplay(
    op: 'revert' | 'cherryPick',
    repo: RepoId,
    row: CommitRow,
    mainline: number | null,
  ): void {
    const request = replayRequestFor(row.oid, mainline)
    const call =
      op === 'revert'
        ? commitActions.revert(project, repo, request)
        : commitActions.cherryPick(project, repo, request)
    void call
      .then((outcome) => notify(replayNote(outcome), { kind: 'ok' }))
      .catch((error: unknown) => {
        /*
         * A merge, reverted without saying which side to keep.
         *
         * `mainlineChoices` turns the refusal into a picker built from the parents the error
         * carries — their summaries and authors, which is the only thing that tells the two
         * sides apart. Only on the *first* attempt: a second `MergeNeedsMainline` after a
         * mainline was sent means the number was rejected, and re-opening the same picker would
         * be a loop the user cannot leave.
         */
        const parents = mainline === null ? mainlineChoices(error) : null
        if (parents !== null) {
          setChosen(parents[0]?.id ?? null)
          setConfirm({
            title: MAINLINE_TITLE,
            body: mainlineBody(row.oid),
            files: [],
            confirmLabel: '',
            choices: parents,
            run: (picked) => {
              const side = mainlineOf(pickedId(parents, picked))
              if (side !== null) runReplay(op, repo, row, side)
            },
          })
          return
        }
        report(error)
      })
  }

  /**
   * *Reset here…* — the preview first, then the dialog built out of it.
   *
   * `git_reset_preview` is its own read and is called *before* the dialog opens rather than while
   * it closes, because the Hard row says "DISCARD 3 changed files" and has to name all three
   * while the user is still deciding. A failure here is reported and no dialog appears at all,
   * which is the honest outcome: a confirmation whose numbers could not be computed would be a
   * dialog asking the user to agree to something it cannot describe.
   */
  function askReset(repo: RepoId, row: CommitRow): void {
    void commitActions
      .resetPreview(project, repo, row.oid)
      .then((preview) => {
        const modes = resetChoices(preview)
        setChosen(modes[0]?.id ?? null)
        setOptionOn(SHELVE_FIRST_DEFAULT)
        setConfirm({
          title: resetTitle(preview),
          body: resetBody(preview),
          // Ignored while `choices` is set — each mode carries its own list, which is rule 1 of
          // `ConfirmDestructive` holding *per choice*: a single list at the top would name the
          // `--hard` casualties while `--soft` was selected.
          files: [],
          confirmLabel: '',
          choices: modes,
          optionFor: (picked) =>
            shelveFirstOffered(preview, pickedId(modes, picked)) ? SHELVE_FIRST_LABEL : null,
          run: (picked, option) => {
            const id = pickedId(modes, picked)
            // `option && shelveFirstOffered(…)` and not `option` alone: the checkbox keeps its
            // state while the radio moves, so a user who ticks it on Hard and then picks Soft
            // must not shelve — Soft touches no file, and a shelf entry nobody asked for is one
            // more thing to clean up.
            const request = resetRequestFor(
              row.oid,
              resetKindOf(id),
              option && shelveFirstOffered(preview, id),
            )
            void commitActions
              .reset(project, repo, request)
              .then((outcome) => notify(resetNote(outcome), { kind: 'ok' }))
              .catch(report)
          },
        })
      })
      .catch(report)
  }

  /** *Check out this commit (detached)* — the confirmation, with whatever is in the way named. */
  function askDetach(repo: RepoId, row: CommitRow, blockers: readonly string[]): void {
    setConfirm({
      ...detachConfirm(row, blockers),
      // The second dialog is the answer to a refusal that already listed the files, so its button
      // is the stash one and the mode goes with it. `stashAndRestore` and not `stash`, because
      // that is what `detachConfirm`'s body promises in so many words — stashed first, put back
      // afterwards, and still in `git stash list` if the restore fails.
      run: () => runDetach(repo, row, blockers.length === 0 ? 'refuse' : 'stashAndRestore'),
    })
  }

  function runDetach(repo: RepoId, row: CommitRow, mode: CheckoutMode): void {
    void commitActions
      .checkoutDetached(project, repo, row.oid, mode)
      .then((outcome) =>
        // A failed restore is a failure even though the checkout worked: the user's changes are
        // in a stash they have not been told about anywhere else, and `detachNote` returns git's
        // own message verbatim for exactly that case.
        notify(detachNote(outcome), {
          kind: outcome.restoreFailed === null ? 'ok' : 'error',
        }),
      )
      .catch((error: unknown) => {
        /*
         * `mode` is `'refuse'` on the first attempt, always — the rule `branch.checkout`
         * documents and this call inherits. The refusal carries the paths that differ between
         * the tree and the target, and re-asking with them is the only way the dialog can name
         * them; offering the stash mode up front would stash a working tree that never needed
         * moving.
         */
        const refusal = mode === 'refuse' ? refusalOf(error) : null
        if (refusal !== null && refusal.paths.length > 0) {
          askDetach(repo, row, refusal.paths)
          return
        }
        report(error)
      })
  }

  /** *Tag…*, and the force retry when the name is taken. */
  function createTag(
    repo: RepoId,
    target: string,
    name: string,
    message: string,
    force: boolean,
  ): void {
    void commitActions
      .tag(project, repo, tagRequestFor(name, target, message, force))
      .then((outcome) => notify(tagNote(outcome), { kind: 'ok' }))
      .catch((error: unknown) => {
        // `TagExists` carries where the tag points *now*, and that oid is the whole of the
        // question `forceTagConfirm` asks — "move it off there?" is answerable and "the name is
        // taken" is not. Only when `force` was not already sent: a second refusal with it set is
        // a different problem and must not re-open the same dialog.
        const existing = force ? null : errorField(error, 'tagExists', 'oid')
        if (existing !== null) {
          setConfirm({
            ...forceTagConfirm(name.trim(), existing, target),
            run: () => createTag(repo, target, name, message, true),
          })
          return
        }
        report(error)
      })
  }

  /** *New branch from here…* — created at the row and switched to. */
  function createBranch(repo: RepoId, target: string, shortOid: string, name: string): void {
    void branch
      .create(project, repo, name, target, true)
      /*
       * The one sentence in this feature that `chrome/logActions.ts` does not own, because it
       * has no note for a branch and neither does `chrome/branchModel.ts`. `BranchSelector.tsx`
       * says `Switched to ${name}` for the same command, which is right there — it is switching
       * from a list of branches — and wrong here: the commit is the entire point of *from here*,
       * and a note that omitted it could not be told from an ordinary checkout.
       */
      .then(() => notify(`Created ${name} at ${shortOid} and switched to it`, { kind: 'ok' }))
      .catch(report)
  }

  /** *Copy revision number* — the full forty, and the notice prints what went on the clipboard. */
  function copyOid(oid: string): void {
    void copyText(oid).then((done) => {
      if (done) {
        notify(`Copied ${oid}`, { kind: 'ok' })
        return
      }
      // `copyText` has already tried the `execCommand` fallback and logged the rejection, so
      // there is nothing left to try — but a copy that silently did nothing is indistinguishable
      // from a menu item wired to nothing, which is the defect this whole surface exists for.
      notify('The clipboard refused the write — nothing was copied.', { kind: 'error' })
    })
  }

  /**
   * The tag dialog, registered for the whole window.
   *
   * `keys/dispatch.ts` owns the slot and `git.tag.new` — the palette row and any binding — calls
   * through it, so this component is what makes that command work at all: before this, nothing
   * anywhere registered an opener and the command reported "Nothing in this window can ask what
   * to call the tag" every time. `null` on unmount is not tidiness — the slot is module-level and
   * per window, so a Log tab that left its opener behind would answer for a tool window that is
   * no longer on screen.
   *
   * `target` is a full oid from a row, or `null` meaning HEAD, which is the palette's case and
   * the reason `git.tag.new` is listable when `git.reset` and `git.revert` are not.
   * `TagRequest.target` takes a revspec, so `HEAD` needs nothing to have been clicked.
   *
   * Empty deps: `setPrompt` is a stable setter and the prompt resolves its repository, its short
   * oid and its project at *render* rather than closing over them. A closure over `rows` here
   * would be the stale one — the page it captured is replaced on every filter keystroke.
   */
  useEffect(() => {
    registerTagDialog((target) => setPrompt({ kind: 'tag', target: target ?? 'HEAD' }))
    return () => registerTagDialog(null)
  }, [])

  /*
   * Whether this webview has a clipboard at all, resolved here rather than inside the model.
   *
   * `chrome/TabStrip.tsx` does exactly this for *Copy path* and for the same two reasons:
   * `navigator` is a DOM global that the model may not touch, and WebKitGTK does not always
   * expose one — so the answer has to be a fact about the running window rather than an
   * assumption baked into a menu.
   */
  const clipboard = typeof navigator === 'undefined' ? undefined : navigator.clipboard

  const { onContextMenu, menu } = useContextMenu({
    label: 'Commit',
    items: ({ target, x, y }) => {
      const row = rowFor(rows, selected, target, x, y)
      // The space below the last row opens nothing. An empty box at the pointer reads as a
      // broken surface rather than as a surface with nothing to offer — `useContextMenu` treats
      // an empty list as "decline to open", which is the behaviour being asked for.
      if (row === null) return []
      const repo = row.repo
      const entries: MenuEntry[] = commitMenu({
        row,
        head: headOids.get(repo) ?? '',
        noClipboard: NO_CLIPBOARD,
        crossRepo: CROSS_REPO_COMPARE,
        /*
         * The pair, and whether the two ends can be diffed at all.
         *
         * `compare?.swappable` and not `pair !== null`: the working-tree comparison is not a
         * two-row selection and must not put *Compare* and *Swap sides* on the menu — there is
         * only one commit selected and the item would be about a pair that does not exist.
         */
        pair:
          pair === null
            ? null
            : {
                newer: pair.newer,
                older: pair.older,
                sameRepo: secondaryRepo === null || secondaryRepo === selectedRepo,
              },
        on: {
          /*
           * *Compare* re-asks for the range the pane is already showing.
           *
           * A fresh selection object with the same three fields in it, which is enough: the pair,
           * the view and the effect below it are all `useMemo`/`useEffect` over object identity,
           * so a new identity re-runs the fetch and nothing else. The previous request is dropped
           * by that effect's own cleanup rather than by a `generation` bump — bumping the shared
           * counter here would also discard an in-flight page or commit detail, which are the
           * other two things this tab can be waiting on and neither of which the user asked to
           * cancel.
           */
          compare: () => setSelection((current) => ({ ...current })),
          swapSides: onSwapSides,
          compareWorkingTree: () => compareWorkingTree(repo, row.oid),
          compareWith: () => setCompareWith({ repo, from: row.oid }),
          revert: () => runReplay('revert', repo, row, null),
          cherryPick: () => runReplay('cherryPick', repo, row, null),
          reset: () => askReset(repo, row),
          /*
           * Through the dispatcher's seam and not straight to `setPrompt`, so that the menu, the
           * palette row and any future binding all open the *same* dialog. The `false` arm cannot
           * fire while this component is mounted — the effect above is what registered the opener
           * — and it is here because a control that opens nothing and says nothing is the failure
           * `openTagDialog`'s boolean exists to make impossible.
           */
          tag: () => {
            if (!openTagDialog(row.oid)) {
              notify('Nothing in this window can ask what to call the tag.', { kind: 'error' })
            }
          },
          branch: () => setPrompt({ kind: 'branch', target: row.oid }),
          detach: () => askDetach(repo, row, []),
          /*
           * *Amend…* hands the commit to the Git panel's commit box rather than amending here.
           *
           * The box owns the message, the ticked paths and the Amend checkbox, and a second
           * amend path would be a second place where "the original author is kept" can stop
           * being true — the same argument `cide_git::commit` makes for refusing a non-HEAD
           * amend inside the existing function instead of a new one. So this fetches the message
           * and parks it; `useGitPanel` claims it and fills the box.
           *
           * Offered only when this window has a panel to reveal, so the disabled line carries a
           * direction instead of the click producing a toast. The `!landed` arm below is not
           * dead in spite of that: the host is registered in an effect and can unregister while
           * the menu is open, and the fetch puts a round trip between the check and the park.
           */
          ...(panelHostPresent()
            ? {
                amend: () => {
                  void gitLog
                    .detail(project, repo, row.oid)
                    .then((detail) => {
                      const landed = requestAmend({
                        project,
                        repo,
                        oid: row.oid,
                        shortOid: row.shortOid,
                        // The full message, summary line included — `CommitDetail.message` is
                        // documented as exactly that, and an amend that dropped the body would
                        // silently truncate every commit it touched.
                        message: detail.message,
                      })
                      if (!landed) {
                        notify('This window has no Git panel, so there is no commit box to amend in.', {
                          kind: 'error',
                        })
                      }
                    })
                    .catch((error: unknown) => {
                      notify(explain(error), { kind: 'error' })
                    })
                },
              }
            : {}),
          ...(clipboard === undefined ? {} : { copy: (oid: string) => copyOid(oid) }),
        },
      })
      return entries
    },
  })

  /** The mode the confirmation is on, resolved the way `ConfirmDestructive` resolves it. */
  const picked = pickedId(confirm?.choices ?? [], chosen)
  const optionLabel = confirm?.optionFor?.(picked) ?? null

  /*
   * What the name prompt is about, resolved from the *current* page rather than captured when it
   * opened. `git.tag.new` can raise it from the command palette with nothing but a revspec, and a
   * filter keystroke replaces the page underneath it.
   */
  const promptRow = prompt === null ? null : (rows.find((r) => r.oid === prompt.target) ?? null)
  const promptShortOid = promptRow?.shortOid ?? prompt?.target.slice(0, PROMPT_OID_WIDTH) ?? ''
  /** The repository the new ref lands in: the row's, or the project's first root as a default. */
  const promptRepo = promptRow?.repo ?? repos?.[0]?.id ?? null
  /*
   * …and whether the prompt has to *ask* which one.
   *
   * Only when the gesture named none — the palette's *Tag…*, which means HEAD and knows nothing
   * about a row — and only when there is more than one root to choose between. A tag opened from
   * a row never sees the control, because the row already answered; guessing the first root in a
   * monorepo would tag the wrong repository in silence.
   */
  const promptAsksRepo = promptRow === null && (repos?.length ?? 0) > 1

  /*
   * The instant half of the text filter.
   *
   * Only while the box and the wire disagree — see the header. Once they agree this is `rows`
   * itself and not a copy of it, which is both the cheap path and the correct one: re-running the
   * local rule over the backend's own answer would hide the rows it found by their *body*, which
   * `CommitRow` does not carry.
   */
  const visible =
    typed.text === applied.text ? rows : applyLocalText(rows, typed.text)

  return (
    <>
      <LogView
        rows={visible}
        // The gutter is parallel to the *page*, index for index. A local narrow drops rows without
        // dropping their lanes, so the two would go out of step — and a graph one row out is worse
        // than no graph, because it is wrong rather than absent. Withdrawn for the moment the
        // narrow is live; the wire answer brings it back a fifth of a second later, and the wire
        // turns it off itself (`GraphOff::Filtered`) whenever a text filter is actually applied.
        graph={visible.length === rows.length ? graph : []}
        graphOff={graphOff}
        status={logStatus({
          loading: busy,
          failed,
          noRepo: repos !== null && repos.length === 0,
          rows: visible.length,
          filtered: isFiltered(typed),
          path,
          stop: page?.stop ?? null,
        })}
        page={page}
        selected={selected}
        secondary={selection.secondary}
        busy={busy}
        now={now}
        filter={typed}
        branches={branchNames}
        repos={repos ?? []}
        reveal={reveal}
        onSelect={onSelect}
        onMore={() => load(true)}
        onFilter={setTyped}
        onRefresh={() => setRefreshes((n) => n + 1)}
        onFindCommit={onFindCommit}
        /*
         * One pane, two answers, decided in one place.
         *
         * `compare` is the derivation above and it is `null` for every ordinary selection, so the
         * single-commit pane is reached by exactly the expression it always was. The range pane
         * is `LogView`'s own `RangeDetails` rather than a component here, because
         * `check-log-render.mjs` server-renders that file and cannot see this one — a compare
         * header built in the wiring half would be a header no check could look at.
         */
        details={
          /*
           * The changed-file menu's root. `display: contents`, so the wrapper receives the
           * bubbled right-click without adding a box — the details pane is a scrolling column
           * whose children are laid out against it directly, and a real element here would
           * introduce a second scroll container between them.
           *
           * Here rather than inside `LogView` because the menu needs the store, the IPC client
           * and four actions, none of which may reach the file `check-log-render.mjs`
           * server-renders. The rows publish `data-file-path`; this resolves through it.
           */
          <div className={styles.fileMenuRoot} onContextMenu={fileMenuHandle.onContextMenu}>
            {fileDiff !== null ? (
              /*
               * A History tab shows **this file's diff**, not the commit's file list. (M21)
               *
               * > *"right side prints file tree with all changes of commit but it should show
               * > diff only for this file"*
               *
               * The tab was opened by *Show history for this file*: the reader has already named
               * the path, so listing the other thirty the commit touched answers a question they
               * did not ask and puts the one they did behind a click.
               *
               * `RevisionDiffPane` rather than a diff assembled here, which is why it grew an
               * `export`: it already fetches one file's hunks, draws them read-only, carries the
               * unified/split toggle the request asks for, and handles `oldPath` so a rename
               * diffs against the right blob. A second copy would be a second place for all four
               * to drift.
               *
               * Keyed on the whole question, so selecting another commit remounts rather than
               * re-pointing — the pane holds a fetch and collapsed-hunk state, and re-pointing
               * would show the previous file's hunks under the new commit's heading for a frame.
               */
              <RevisionDiffPane
                key={`${fileDiff.repo} ${fileDiff.path} ${JSON.stringify(fileDiff.next)} ${JSON.stringify(fileDiff.prev)}`}
                project={project}
                repo={fileDiff.repo}
                path={fileDiff.path}
                next={fileDiff.next}
                prev={fileDiff.prev}
              />
            ) : compare === null ? (
              <Details
                detail={detail}
                selected={selected}
                asTree={filesAsTree}
                theme={iconTheme}
                selectedFile={selectedFile}
                onSelectFile={setSelectedFile}
                collapsedDirs={collapsedDirs}
                onToggleDir={onToggleDir}
                onToggleTree={onToggleTree}
                onOpenFile={onOpenFile}
              />
            ) : (
              <RangeDetails
                older={compare.olderLabel}
                newer={compare.newerLabel}
                range={range}
                note={rangeNote ?? (range === null ? RANGE_LOADING : null)}
                onSwap={compare.swappable ? onSwapSides : null}
                asTree={filesAsTree}
                theme={iconTheme}
                selectedFile={selectedFile}
                onSelectFile={setSelectedFile}
                collapsedDirs={collapsedDirs}
                onToggleDir={onToggleDir}
                onToggleTree={onToggleTree}
                onOpenFile={onOpenRangeFile}
              />
            )}
            {fileMenuHandle.menu}
          </div>
        }
        split={split}
        rowMenu={{ onContextMenu, menu }}
      />
      {/*
        * Both overlays are mounted here rather than by `App.tsx`, and neither portals:
        * `OverlayCard` renders its own fixed scrim in place. That is what let this whole feature
        * land without a line in a file another session owns — the same argument
        * `GitPanel.tsx` makes for its two, one folder over.
        *
        * `ConfirmDestructive` calls neither `onCancel` nor `run`; the caller runs and closes.
        * `state.run` is therefore a no-op here and the real call is in `onConfirm` — the field
        * exists because the type requires it, and the component's own comment says why it is
        * not the channel.
        */}
      {confirm !== null && (
        <ConfirmDestructive
          state={{
            title: confirm.title,
            body: confirm.body,
            files: confirm.files,
            confirmLabel: confirm.confirmLabel,
            ...(confirm.mark === undefined ? {} : { mark: confirm.mark }),
            ...(confirm.choices === undefined
              ? {}
              : { choices: confirm.choices, chosen, onChoose: setChosen }),
            ...(optionLabel === null
              ? {}
              : { option: { label: optionLabel, checked: optionOn, onToggle: setOptionOn } }),
            run: () => {},
          }}
          onCancel={() => setConfirm(null)}
          onConfirm={() => {
            // Read before the state is cleared, and the dialog is closed *before* the command is
            // sent: every one of these round-trips, and a confirmation left on screen while it
            // does invites a second click on the same button.
            const act = confirm.run
            setConfirm(null)
            act(picked, optionOn)
          }}
        />
      )}
      {prompt !== null && (
        <NamePromptCard
          kind={prompt.kind}
          shortOid={promptShortOid}
          repos={repos ?? []}
          repo={promptRepo}
          askRepo={promptAsksRepo}
          onCancel={() => setPrompt(null)}
          onSubmit={(repo, name, message) => {
            setPrompt(null)
            if (prompt.kind === 'branch') {
              createBranch(repo, prompt.target, promptShortOid, name)
              return
            }
            createTag(repo, prompt.target, name, message, false)
          }}
        />
      )}
      {/*
        * *Compare with…*.
        *
        * Mounted here for the same reason the two above are: `OverlayCard` draws its own scrim in
        * place and portals nothing, so the whole gesture lands without a line in `App.tsx`.
        *
        * The resolve is passed in rather than called by the card, so the one place that talks to
        * `git_resolve_rev` is this file — `ipc/client.ts` is the frontend's only seam to `invoke`
        * and a component three functions down reaching for it is how that stops being greppable.
        */}
      {compareWith !== null && (
        <CompareWithCard
          shortOid={shortenOid(compareWith.from)}
          rows={visible}
          exclude={compareWith.from}
          resolve={(spec: string) => gitLog.resolve(project, compareWith.repo, spec)}
          onCancel={() => setCompareWith(null)}
          onPicked={(resolved) => {
            setCompareWith(null)
            applyCompareWith(compareWith.repo, compareWith.from, resolved)
          }}
        />
      )}
    </>
  )
}

/**
 * Every branch name the picker offers: locals first, then remote-tracking, deduplicated.
 *
 * A union across repositories, because `LogRefs::Branch` resolves *per repository* and a root
 * that lacks the name reports `NoSuchRef` rather than failing the query — which
 * `logModel::missingRefNote` turns into a sentence under the list. Hiding a name only one root
 * carries would make the branch filter useless in exactly the monorepo it exists for.
 *
 * Remote-tracking names are included because `cide_git::log::find_branch_tip` tries
 * `BranchType::Local` and then `BranchType::Remote`, so `origin/main` resolves today; a picker
 * that offered only locals would be hiding half of what the backend accepts.
 */
function namesOf(lists: readonly BranchList[]): string[] {
  const seen = new Set<string>()
  const out: string[] = []
  for (const side of [0, 1]) {
    for (const list of lists) {
      for (const entry of side === 0 ? list.local : list.remote) {
        if (seen.has(entry.name)) continue
        seen.add(entry.name)
        out.push(entry.name)
      }
    }
  }
  return out
}

function Details({
  detail,
  selected,
  asTree,
  theme,
  selectedFile,
  onSelectFile,
  collapsedDirs,
  onToggleDir,
  onToggleTree,
  onOpenFile,
}: {
  detail: CommitDetail | null
  selected: string | null
  /** Group the files by directory. Persisted — see `ToolWindowState::files_as_tree`. */
  asTree: boolean
  onToggleTree?: ((next: boolean) => void) | undefined
  /**
   * Show this file as the selected commit left it.
   *
   * Takes `MouseEvent.detail` — the click count — rather than a gesture, so the rule that turns
   * one into the other stays in one place. See `onOpenFile` in the host.
   */
  onOpenFile?: ((path: string, oldPath: string | null, detailCount: number) => void) | undefined
  theme: IconTheme
  selectedFile: string | null
  onSelectFile?: ((path: string) => void) | undefined
  collapsedDirs: ReadonlySet<string>
  onToggleDir?: ((dir: string) => void) | undefined
}) {
  if (selected === null) return <div className={styles.status}>Select a commit</div>
  if (detail === null) return <div className={styles.status}>Reading the commit…</div>
  return (
    <>
      {/*
        * `LogView`'s list, shared with the two-revision one beside it (M20, regrouped in M21).
        *
        * Shared and not copied, because the *gestures* on these rows are the conditional ones — a
        * single click re-points a revision diff only while one is already open — and a second
        * spelling of that is a second thing to keep in step with `sidebar/clickSemantics.ts`.
        * What differs here is only what is not drawn: `counts` is `null`, because this list
        * states its totals once in its own summary line and `CommitDetail.total.partial` means a
        * large merge has no per-file counts at all, so a column of `+0 −0` would be a number the
        * reader could believe.
        */}
      <ChangedFileList
        files={detail.files.map((f) => ({ path: f.path, oldPath: f.oldPath, counts: null }))}
        asTree={asTree}
        summary={
          `${detail.total.files} file${detail.total.files === 1 ? '' : 's'}`
          + (detail.total.partial ? '' : ` · +${detail.total.added} −${detail.total.deleted}`)
          + (detail.filesTruncated ? ' (list truncated)' : '')
        }
        {...(onToggleTree === undefined ? {} : { onToggleTree })}
        {...(onOpenFile === undefined ? {} : { onOpen: onOpenFile })}
        theme={theme}
        selected={selectedFile}
        collapsed={collapsedDirs}
        {...(onSelectFile === undefined ? {} : { onSelect: onSelectFile })}
        {...(onToggleDir === undefined ? {} : { onToggleDir })}
        titleFor={(f) =>
          `${f.path}\n`
          + `Double-click or Ctrl+D to open this file as ${detail.commit.shortOid} left it.\n`
          + 'Right-click for more.'
        }
      />
      {/*
        * The message **below** the file list, which is the way round that was asked for and is
        * also the way round the pane is actually read.
        *
        * The list is the thing being worked: it is clicked, arrowed through and right-clicked,
        * and it is the only part whose length is unbounded. Above a message of unknown height it
        * started at a different place on screen for every commit, so the first row moved
        * whenever the selection did — and a one-line commit and a forty-line one put the list in
        * two entirely different places. Pinning the list to the top makes the row under the
        * pointer stay under the pointer, and the message, which is read once and not
        * interacted with, takes the leftover space.
        */}
      <div className={styles.commitMessage} data-audit="logCommitMessage">
        <div>
          <strong>{detail.commit.shortOid}</strong> — {detail.committer}
        </div>
        <div className={styles.note}>{exactly(detail.commit.authored)}</div>
        <pre className={styles.messageText}>{detail.message}</pre>
      </div>
    </>
  )
}

/**
 * *New branch from here…* and *Tag…*: one field, two for a tag, and a repository when the
 * gesture did not name one.
 *
 * # Why it is here and not a component of its own
 *
 * It is the *smallest* dialog in the app — a name, a button, Escape — and the thing it must not
 * become is a second `ChangelistDialog`. What it does share with that one is the card and the
 * scrim, and those come from `OverlayCard`, which is what stops the two drifting 4px apart: the
 * file picker, the close confirmation, the changelist chooser and this all draw the same ground.
 *
 * # Why the tag prompt has a message box rather than an "annotated" checkbox
 *
 * `TagRequest.message` being present *is* the choice — `Some` makes a real tag object with a
 * tagger and a date, `None` makes a ref and nothing else — so a checkbox beside a message box
 * would be two controls that can disagree, and the disagreement is invisible until `git
 * describe` ignores the tag weeks later in CI. One box: fill it in and the tag is annotated.
 *
 * # Why the repository picker appears at all
 *
 * `git.tag.new` from the command palette means "tag HEAD" and names no repository, and a project
 * can hold several roots. With one root there is nothing to ask and the control is not drawn;
 * with several, guessing the first would tag the wrong repository in silence. A tag opened from a
 * row never sees it — the row knows which repository it came from.
 */
function NamePromptCard({
  kind,
  shortOid,
  repos,
  repo,
  askRepo,
  onCancel,
  onSubmit,
}: {
  kind: 'branch' | 'tag'
  /** The commit the new ref will point at, abbreviated — or `HEAD`. */
  shortOid: string
  repos: readonly RepoInfo[]
  /** Where the ref lands: the row's repository, or the project's first root as a default. */
  repo: RepoId | null
  /** Draw the repository picker, because the gesture named none and there is more than one. */
  askRepo: boolean
  onCancel: () => void
  onSubmit: (repo: RepoId, name: string, message: string) => void
}) {
  const [name, setName] = useState('')
  const [message, setMessage] = useState('')
  const [pickedRepo, setPickedRepo] = useState<RepoId | null>(repo)
  const field = useRef<HTMLInputElement>(null)

  // The field, not a button: the dialog exists to ask for a name and the next keystroke should
  // be the first letter of it. `useEffect` and not `useLayoutEffect` — this dialog is opened by
  // a menu click rather than by a keystroke, so there is no character already in flight.
  useEffect(() => {
    field.current?.focus()
  }, [])

  const target = pickedRepo ?? repo
  const trimmed = name.trim()
  const ready = trimmed !== '' && target !== null
  const title = kind === 'branch' ? `New branch from ${shortOid}` : `Tag ${shortOid}`
  const submitLabel = kind === 'branch' ? 'Create and switch' : 'Create tag'

  const submit = () => {
    if (ready && target !== null) onSubmit(target, trimmed, message)
  }

  // On the card and not on `document`: a window listener would also answer for the terminal
  // underneath and for any other overlay that happens to be open. Same rule `ChangelistDialog`
  // and `ConfirmDestructive` follow.
  const onKeyDown = (ev: React.KeyboardEvent) => {
    if (ev.key === 'Escape') {
      ev.stopPropagation()
      onCancel()
    }
  }

  return (
    <OverlayCard label={title} onDismiss={onCancel}>
      <div className={styles.prompt} onKeyDown={onKeyDown} data-audit="logPrompt">
        <h2 className={styles.promptTitle} data-audit="logPromptTitle">
          {title}
        </h2>
        {askRepo && (
          <select
            className={styles.promptSelect}
            data-audit="logPromptRepo"
            aria-label="Repository"
            // `target` and not `pickedRepo`: the two differ for one render if the prompt is
            // raised before `git_repos` has answered, and a `<select>` whose value matches no
            // option renders **blank** — so the control would be showing nothing while the
            // button beside it was about to act on the first root.
            value={target ?? ''}
            onChange={(ev) => setPickedRepo(ev.target.value as RepoId)}
          >
            {repos.map((info) => (
              <option key={info.id} value={info.id}>
                {info.name}
              </option>
            ))}
          </select>
        )}
        <input
          ref={field}
          className={styles.promptInput}
          data-audit="logPromptName"
          value={name}
          placeholder={kind === 'branch' ? 'Branch name' : 'Tag name'}
          aria-label={kind === 'branch' ? 'Branch name' : 'Tag name'}
          spellCheck={false}
          autoComplete="off"
          onChange={(ev) => setName(ev.target.value)}
          onKeyDown={(ev) => {
            if (ev.key !== 'Enter') return
            ev.preventDefault()
            submit()
          }}
        />
        {kind === 'tag' && (
          <input
            className={styles.promptInput}
            data-audit="logPromptMessage"
            value={message}
            placeholder="Message (leave empty for a lightweight tag)"
            aria-label="Tag message"
            spellCheck={false}
            autoComplete="off"
            onChange={(ev) => setMessage(ev.target.value)}
            onKeyDown={(ev) => {
              if (ev.key !== 'Enter') return
              ev.preventDefault()
              submit()
            }}
          />
        )}
        <div className={styles.promptFooter}>
          <button type="button" className={styles.promptButton} onClick={onCancel}>
            Cancel
          </button>
          {/* Nothing here destroys anything — a new ref is added and the old ones stay — so the
              accent-filled button is the one that acts, which is `ChangelistDialog`'s rule and
              the opposite of `ConfirmDestructive`'s. */}
          <button
            type="button"
            className={`${styles.promptButton} ${styles.promptPrimary}`}
            data-audit="logPromptSubmit"
            disabled={!ready}
            onClick={submit}
          >
            {submitLabel}
          </button>
        </div>
      </div>
    </OverlayCard>
  )
}


/**
 * *Compare with…*: pick a commit off the loaded log, or type any revision git accepts.
 *
 * # Why the typed half exists at all
 *
 * The list is bounded by whatever the walk has loaded, and the revision a user wants to compare
 * against is very often not in it: a tag from last year, `HEAD~200`, a branch tip below the
 * frontier, `origin/main` after a fetch. A picker that offered only the loaded page would answer
 * the easy half of the question and be silent about the half that made the user open a menu.
 *
 * # Why the answer is an oid and never the string
 *
 * `git_resolve_rev` runs before the value is used for anything, and that is not defensive
 * plumbing — it is the same rule `DiffSpec` enforces one layer down. A comparison that had stored
 * `main` would name a different tree tomorrow: the tab would still be open, the header would
 * still say `main`, and the diff under it would silently have become a different diff. The
 * command exists for exactly this and had no caller until now.
 *
 * A **range** spec answers both ends at once (`ResolvedRev.range`), which is why the whole
 * resolved value is handed up rather than just `from`. `v1.0..v1.1` typed here is a comparison
 * the user has fully specified, and throwing away `to` would silently reinterpret it as "compare
 * the selected commit with v1.0".
 *
 * # The field narrows the list as well as being the free-text box
 *
 * One control doing both, because they are the same question asked with different precision — and
 * because a card with a filter box *and* a revision box would need a rule for what happens when
 * they disagree. Typing `pane` narrows to the commits whose subject says so; typing `v1.2` will
 * narrow to nothing and Enter resolves it. Nothing is resolved until Enter or *Compare*, so no
 * keystroke costs a round trip.
 */
function CompareWithCard({
  shortOid,
  rows,
  exclude,
  resolve,
  onCancel,
  onPicked,
}: {
  /** The commit the menu was opened on, abbreviated — the other side of the comparison. */
  shortOid: string
  /** The loaded log, in its own order. Seeded from what is on screen, not re-fetched. */
  rows: readonly CommitRow[]
  /** The anchor's oid, kept out of the list: a commit cannot be compared with itself. */
  exclude: string
  resolve: (spec: string) => Promise<{ from: string; to: string | null; range: boolean }>
  onCancel: () => void
  onPicked: (resolved: { from: string; to: string | null; range: boolean }) => void
}) {
  const [spec, setSpec] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const field = useRef<HTMLInputElement>(null)

  // The field, not a button: the card exists to ask which revision, and the next keystroke should
  // be the first letter of it. `useEffect` and not `useLayoutEffect` — a menu click opened this,
  // so there is no character already in flight.
  useEffect(() => {
    field.current?.focus()
  }, [])

  const needle = spec.trim().toLowerCase()
  const listed = rows.filter(
    (r) =>
      r.oid !== exclude
      && (needle === ''
        || r.shortOid.startsWith(needle)
        || r.oid.startsWith(needle)
        || r.summary.toLowerCase().includes(needle)),
  )

  /*
   * A row is already an oid, so it is taken as `from` with no round trip.
   *
   * Shaped as a non-range `ResolvedRev` rather than given its own callback, so the caller has one
   * path to apply and cannot end up with two rules for "what a picked revision does".
   */
  const pickRow = (oid: string) => onPicked({ from: oid, to: null, range: false })

  const submit = () => {
    const typed = spec.trim()
    if (typed === '' || busy) return
    setBusy(true)
    setError(null)
    void resolve(typed)
      .then((resolved) => onPicked(resolved))
      .catch((raised: unknown) => {
        // The card stays open holding what was typed. `explain` has an arm for `NoSuchRef` and
        // for the ambiguous-revision refusals, and `String(error)` on a `{kind, detail}` object
        // is the literal text `[object Object]` — the bug `check:branches` exists for.
        setBusy(false)
        setError(explain(raised))
      })
  }

  const onKeyDown = (ev: React.KeyboardEvent) => {
    if (ev.key === 'Escape') {
      ev.stopPropagation()
      onCancel()
    }
  }

  const title = `Compare ${shortOid} with…`
  return (
    <OverlayCard label={title} onDismiss={onCancel}>
      <div className={styles.prompt} onKeyDown={onKeyDown} data-audit="logCompareWith">
        <h2 className={styles.promptTitle}>{title}</h2>
        <input
          ref={field}
          className={styles.promptInput}
          data-audit="logCompareSpec"
          value={spec}
          placeholder="Branch, tag or revision — main, v1.2, HEAD~3, v1.0..v1.1"
          aria-label="Revision"
          spellCheck={false}
          autoComplete="off"
          onChange={(ev) => {
            setSpec(ev.target.value)
            setError(null)
          }}
          onKeyDown={(ev) => {
            if (ev.key !== 'Enter') return
            ev.preventDefault()
            submit()
          }}
        />
        {error !== null && (
          <div className={styles.compareError} data-audit="logCompareError">
            {error}
          </div>
        )}
        <div className={styles.compareList} data-audit="logCompareList">
          {listed.map((r) => (
            <button
              key={`${r.repo}:${r.oid}`}
              type="button"
              className={styles.compareRow}
              data-audit="logCompareRow"
              title={r.oid}
              onClick={() => pickRow(r.oid)}
            >
              <span className={styles.compareOid}>{r.shortOid}</span>
              <span className={styles.compareSubject}>{r.summary}</span>
            </button>
          ))}
        </div>
        <div className={styles.promptFooter}>
          <button type="button" className={styles.promptButton} onClick={onCancel}>
            Cancel
          </button>
          <button
            type="button"
            className={`${styles.promptButton} ${styles.promptPrimary}`}
            data-audit="logCompareSubmit"
            disabled={spec.trim() === '' || busy}
            onClick={submit}
          >
            Compare
          </button>
        </div>
      </div>
    </OverlayCard>
  )
}
