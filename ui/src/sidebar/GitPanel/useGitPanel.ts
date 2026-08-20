/**
 * The git panel's state and its only route to Rust.
 *
 * # The guard
 *
 * Every call goes through `guarded`. A git command can fail for reasons that are nobody's bug
 * (a repo mid-rebase, an index lock held by a `git` in a bash pane, a submodule nobody
 * initialised), and none of those may take the window down. React 19 unmounts the whole tree
 * on an unhandled throw out of a render or an effect, so a bare `await` in here is a blank
 * window rather than a broken panel.
 *
 * Failures land in `unavailable` as one dim line under the toolbar, plus a full line on the
 * app's stderr. They are never thrown, never retried in a loop, never silent.
 *
 * # Repo ids
 *
 * Every command here takes a `RepoId`, which is a uuid derived from the canonical work tree —
 * **not** a path. `cmd/git.rs::repo_root` resolves it through `cide_git::repo::find`, so a
 * path passed where an id belongs is a `NoSuchRepo` on every button in the panel. The ids come
 * out of `RepoChanges.repo.id` and travel through row ids and `CommitUnit`s untouched.
 *
 * # The external-staging guard
 *
 * `CommitRequest.force` waives the index check in `cide_git::commit`. It defaults to `false`
 * here and is only ever `true` for a repo where the user pressed **Overwrite** on the guard
 * bar. That default is the whole point: bash panes inside cide are exactly where someone runs
 * `git add`, and a panel that always waived the check would silently clobber it.
 *
 * # Freshness
 *
 * M10's acceptance test is "Claude edits a file → the panel updates within 200 ms with no
 * manual refresh". `cide://session-tool` names the touched paths and arrives ahead of any
 * filesystem watcher, so it is one trigger; `cide://git-status` is the other, and carries the
 * new tree rather than asking for it. Both land on the same coalescer.
 */
import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import {
  diag,
  events,
  git as gitApi,
  gitDiff as gitDiffApi,
  type ChangesTree,
  type DiffSide,
  type FileDiff,
  type PathSelection,
  type ProjectId,
  type RepoId,
} from '@/ipc/client'
import { noteChangeCount } from '@/chrome/gitCountStore'
import { explain } from '@/chrome/branchModel'
import { requestFocus } from '@/chrome/focusRequests'
import {
  amendPending,
  amendPendingServer,
  claimAmend,
  planAmend,
  subscribeAmendRequest,
  type AmendRequest,
} from '@/chrome/panelRequests'
import { diffPaneAvailable } from './diffHost'
import {
  clearAllPartials,
  commitSelections,
  getServerSnapshot as partialServerSnapshot,
  getSnapshot as partialSnapshot,
  liveKey,
  pruneTo,
  subscribe as subscribePartials,
  type PartialEntry,
} from './partialStore'
import { noteRepoRoots } from './repoRoots'
import {
  allFiles,
  allGroups,
  allRepos,
  arrivals,
  buildRows,
  changelistsOf,
  commitUnits,
  defaultExpanded,
  defaultSelection,
  findChangelistId,
  flatFiles,
  groupOf,
  inRepo,
  isIgnoredGroupRow,
  normalizeStatus,
  pruneSelection,
  repoOf,
  selectedFiles,
  toggleRow,
  toggleRows,
  viewOf,
  type Row,
} from './model'
import {
  carriedIds,
  collapseTo,
  keySelect,
  pressSelect,
  pruneRowSelection,
  releaseSelect,
  selectAll,
  NO_ROWS,
  type RowSelection,
  type SelectMods,
} from './rowSelection'
import { storyFromQuery, type GitStory } from './fixture'
import type { ConfirmState } from '@/chrome/ConfirmDestructive'
import type {
  ChangeEntry,
  ChangelistDialogState,
  DiffOpenMode,
  ShelfRow,
  StatusView,
} from './types'

/**
 * Whole-file selections for a list of paths.
 *
 * The panel ticks files; `cide-git` accepts a `PathSelection` per path so the same commands
 * serve per-hunk and per-line staging. `whole` is what a ticked checkbox means, and `rev:
 * null` skips the staleness check, which is only correct for `Whole` — see `PathSelection`.
 *
 * Still here, and still the right answer for `unstage`. That command resolves its selections
 * against `DiffSide::Staged`, and the diff pane's stored selections are made against
 * `Combined` (see `partialStore`), so honouring one here would send positions in one diff as
 * positions in another. `commit` and `shelve` do resolve against `Combined` and go through
 * `commitSelections` instead — that is the seam where the hunk gutter reaches the commit.
 */
function wholeFiles(paths: string[]): PathSelection[] {
  return paths.map((path) => ({ path, selection: { kind: 'whole' }, rev: null }))
}

const EMPTY: StatusView = { repos: [] }

/**
 * The repository's name, or `''` when there is only one and naming it would be noise.
 *
 * The dialogs show it because a changelist belongs to exactly one repository's sidecar, and a
 * monorepo user with four roots open otherwise has to guess which one *New changelist* meant.
 */
function repoLabel(view: StatusView, repo: RepoId): string {
  if (allRepos(view).length < 2) return ''
  return repoOf(view, repo)?.name ?? repo
}

/** `1 file` / `4 files`, so the dialogs and the menu labels agree on the wording. */
function files(n: number): string {
  return `${n} ${n === 1 ? 'file' : 'files'}`
}

/**
 * The `shownFor` key for "add to git, then file" — one key for a gesture that is two calls.
 *
 * Keyed as one operation on purpose: the panel shows a single line, only the operation that
 * wrote it may clear it (see `note`), and the two halves of this gesture must not be able to
 * clear each other's message. A failed add followed by a successful anything-else would
 * otherwise wipe the only record that the files never made it into git.
 */
const TRACK_NOTE = 'git add and file'

/** Coalescing window for refresh bursts. One edit reports several paths. */
const REFRESH_DEBOUNCE_MS = 60

/**
 * The commit an amend is aimed at, once something has named one.
 *
 * The repository travels with the oid because a workspace has several and a commit is only the
 * tip of one of them. `commit` walks its `CommitUnit`s repo by repo and sends the oid **only**
 * to the repository it came from; the others get `null` and keep the checkbox's own
 * amend-whatever-HEAD-is meaning, which is the honest answer for a repo nobody named a commit in.
 */
export interface AmendTarget {
  repo: RepoId
  /** The full oid — what goes on the wire as `CommitRequest.amendOf`. */
  oid: string
  /** The log's abbreviation, for the checkbox label and the confirmation. */
  shortOid: string
}

export interface GitPanelModel {
  view: StatusView
  rows: Row[]
  shelf: readonly ShelfRow[]
  /**
   * The **ticks** — the file rows a commit would take. Not the row selection; the two are
   * different concepts and the header of `rowSelection.ts` is the table that says how.
   */
  selected: ReadonlySet<string>
  /**
   * The **row selection** — which rows a gesture is about.
   *
   * Held here rather than inside `ChangesTree` for three reasons, and only the first is
   * tidiness. The context menu builds its scope in `GitPanelHost`, one level *above* the view,
   * so anything the menu and the drag must agree on has to be visible from this hook. The tree
   * unmounts every time the Shelf tab is opened. And a selection down in the component could
   * not be pruned against a refresh, which happens several times a second while an agent
   * edits.
   */
  selection: RowSelection
  /**
   * The row selection resolved to what a gesture carries — see `rowSelection.ts::carriedIds`.
   * Memoized here because `grab` is called once per drag *and* once per context menu open.
   */
  carried: ReadonlySet<string>
  /**
   * The row the user is pointing at, by id — the cursor, which is neither a tick nor the
   * selection. An id rather than an index; see the header of `ChangesTree`.
   */
  current: string | null
  expanded: ReadonlySet<string>
  /** The `ChangeEntry`s behind the ticks, in row order — what the footer counts. */
  picked: ChangeEntry[]
  loading: boolean
  /** Set when a git call failed. One dim line, not a dialog. */
  unavailable: string | null
  /** A command is in flight; the commit buttons are disabled while it is. */
  busy: string | null
  /** Repos whose index moved under us and whose bar has not been answered yet. */
  diverged: RepoId[]
  message: string
  amend: boolean
  /**
   * The commit the *log* asked to amend, or `null` for the checkbox's own meaning.
   *
   * `null` is the ordinary state and it is not a missing value: the Amend checkbox is a
   * statement about HEAD, whatever HEAD happens to be when Commit is pressed, and that is the
   * only thing it can be — the panel is drawn from a status walk and has no commit list to name
   * a row in. The log does have one, so its *Amend…* arrives with an oid, and from then until
   * the box is committed or the tick is cleared this commit is the one being rewritten.
   *
   * Surfaced on the model, and drawn on the checkbox, for the reason the partial-selection
   * warning is drawn: it changes what the Commit button writes and nothing else in the panel
   * would show it. A hidden modifier on the button that rewrites history is not acceptable.
   */
  amendOf: AmendTarget | null
  /**
   * The repository Commit would reword, or `null` if this click is not a reword.
   *
   * Non-null means: Amend is ticked, nothing is selected, one repository is named, and its HEAD
   * exists. It is the single answer behind both halves of the reword — `model.ts::canCommit`
   * lights the button from it and `commitUnits` mints the file-less unit from it — because two
   * derivations of one fact disagree eventually, and the way they disagree here is a live
   * button that commits nothing.
   */
  rewordRepo: RepoId | null
  /**
   * IDEA's "use Git staging area instead" mode — the toolbar's ◉ toggle.
   *
   * Read out of the payload, not held locally: it is a per-repo setting written to the
   * changelists sidecar by `git_set_use_staging_area`, so a local mirror would disagree with
   * the backend the moment another window flipped it.
   */
  stagingArea: boolean
  /**
   * Files the diff pane has held part of for the next commit.
   *
   * Surfaced because it changes what Commit does without anything in this panel showing it:
   * a ticked row whose file has a stored selection commits some of its lines and leaves the
   * rest. A hidden modifier on the button that writes history is not acceptable, so the
   * panel draws a line naming the count with a way to drop it.
   */
  partials: readonly PartialEntry[]
  /** True when the panel is showing a fixture rather than a repository. */
  story: boolean
  /** The changelist chooser, or `null`. Create, rename and move are all this one dialog. */
  dialog: ChangelistDialogState | null
  /**
   * A pending destructive confirmation, or `null`.
   *
   * Held here rather than in the view so that the *model* owns the rule that revert never runs
   * without one: an action that opened a dialog from inside a component could be bypassed by
   * any other caller of `rollback`.
   */
  confirm: ConfirmState | null
}

export interface GitPanelActions {
  refresh: () => void
  /** Re-read every repo's shelf. Called when the Shelf tab opens, and after it changes. */
  refreshShelf: () => void
  /**
   * Tick or untick one row's subtree. **Never touches the row selection** — a checkbox is a
   * statement about a commit, and a click on one that also moved the selection would make
   * ticking a file silently change what the next drag carries.
   */
  toggleCheck: (row: Row) => void
  /** Tick or untick every selected row at once — the Space key. Still no selection change. */
  toggleCheckSelected: () => void
  /** Move the cursor, and nothing else. What `onFocus` on a row calls. */
  setCurrent: (id: string) => void
  /**
   * A left press on a row. Returns `true` when the collapse it implies has been **deferred**
   * to the release — see `rowSelection.ts::PressPlan`.
   */
  pressRow: (id: string, mods: SelectMods) => boolean
  /** The release of a deferred press, when the gesture did not turn out to be a drag. */
  releaseRow: (id: string) => void
  /** An arrow/Home/End that has already chosen its destination row. */
  keyToRow: (id: string, mods: SelectMods) => void
  /** Ctrl+A on the tree: every selectable row on screen. */
  selectAllRows: () => void
  /** Escape on the tree: back to the row the cursor is on. */
  collapseSelection: () => void
  toggleExpand: (row: Row) => void
  setAllExpanded: (open: boolean) => void
  setMessage: (text: string) => void
  setAmend: (on: boolean) => void
  setStagingArea: (on: boolean) => void
  commit: (push: boolean) => void
  /** Take the ticked paths out of the index. NOT a rollback — see `Toolbar`. */
  unstage: () => void
  /**
   * One file into the index, out of it, or back to HEAD — the context menu's three verbs.
   *
   * They act on the file they are given and on nothing else. The toolbar's `unstage` above
   * acts on the *ticks*, which is a different set and usually a larger one; a menu item that
   * quietly took the ticks with it would rewrite the index for files the user never pointed
   * at. `rollbackFile` destroys uncommitted work — see `git.rollback`.
   */
  stageFile: (repo: RepoId, path: string) => void
  unstageFile: (repo: RepoId, path: string) => void
  /** Opens the confirmation. Nothing is destroyed until it is answered. */
  rollbackFile: (repo: RepoId, path: string) => void

  // --- changelists -------------------------------------------------------------------------

  /**
   * Open the chooser on `New changelist`. `repo` is optional only because the toolbar button
   * has no row under it; with more than one repository open the toolbar disables itself and
   * the gesture comes from a repository row's menu instead, which does know.
   */
  newChangelist: (repo?: RepoId) => void
  renameChangelist: (repo: RepoId, id: string) => void
  /** Its paths fall back to the default list — no work is lost, so this does not confirm. */
  deleteChangelist: (repo: RepoId, id: string) => void
  setActiveChangelist: (repo: RepoId, id: string) => void
  /**
   * Open the chooser on `Move to changelist` for these repo-relative paths.
   *
   * `track` says the paths are unversioned: picking a list **adds them to git** and then
   * files them, which the dialog says in words before anything runs. The menu passes it from
   * the row's group kind, exactly as a drag passes it through `dragDrop.ts`'s `track`
   * outcome, so the two routes cannot end up doing different things to the repository.
   */
  moveToChangelist: (repo: RepoId, paths: string[], track: boolean) => void
  /**
   * File these paths into that changelist. No dialog — the target was already named by the
   * gesture, which is what a drop onto a changelist row is.
   *
   * The `id` is the **raw** changelist id, never the `cl:`-prefixed group id — see
   * `changelistIdOf`. The rules deciding whether a drop may happen at all live in
   * `dragDrop.ts`, so this is deliberately unguarded past the empty check: a caller that has
   * a target and a non-empty path list has already been told the move is legal.
   */
  movePaths: (repo: RepoId, changelist: string, paths: string[]) => void
  /**
   * Add these unversioned paths to git and file them into that changelist — one gesture, two
   * calls, and no dialog. What a drop out of `Unversioned Files` runs.
   *
   * Separate from `movePaths` because it writes to the repository: it is a `git add`, and
   * ADR 0004's index-is-derived model makes that a decision with consequences rather than a
   * detail. The implementation carries them.
   */
  trackPaths: (repo: RepoId, changelist: string, paths: string[]) => void
  /** Throw away everything in one group. **Opens the confirmation**, which names every file. */
  revertGroup: (repo: RepoId, group: string) => void
  /**
   * Same, for an explicit list of paths.
   *
   * `untracked` is not a flavour of wording, it is what the command *does*: `stage::rollback`
   * finds nothing in HEAD for an untracked path and deletes it from disk instead. Pass it for
   * any set drawn from the unversioned list, or the dialog promises a restore before running a
   * delete — see `revertGroup`, which has split on the same fact since the group menu shipped.
   */
  revertFiles: (repo: RepoId, paths: string[], untracked?: boolean) => void
  /** Shelve a whole group under its own name — the group menu's `Shelve Changelist`. */
  shelveGroup: (repo: RepoId, group: string) => void
  dismissDialog: () => void
  /** The chooser's list rows: move the pending paths into an existing changelist. */
  pickChangelist: (id: string) => void
  /** The chooser's name field: create (and, in `move`, move), or rename. */
  submitChangelistName: (name: string) => void
  dismissConfirm: () => void
  /** Answer the confirmation with "yes". The only thing that runs a `ConfirmState.run`. */
  runConfirm: () => void

  shelve: () => void
  unshelve: (row: ShelfRow) => void
  /** IDEA's *Unshelve and keep*: apply the patch and leave it on the shelf. */
  unshelveKeep: (row: ShelfRow) => void
  /** Remove a shelf entry without applying it. **Opens the confirmation.** */
  dropShelf: (row: ShelfRow) => void
  /** Guard bar, left button: adopt git's index and drop our ticks for that repo. */
  reloadIndex: (repo: RepoId) => void
  /** Guard bar, right button: keep our ticks and let the next commit rewrite the index. */
  overwriteIndex: (repo: RepoId) => void
  /** Forget every held partial selection: the next commit takes whole files again. */
  clearPartials: () => void
  /**
   * A file row was activated: double-clicked, Enter, or single-clicked with a diff already up.
   *
   * `mode` tells the two apart, because they are two different commands — see
   * {@link DiffOpenMode}. It is optional so a caller who only ever means "open properly" (the
   * keyboard, a host wiring the tree itself) says nothing and gets that.
   */
  openDiff: (row: Row, mode?: DiffOpenMode) => void
  /**
   * The toolbar's ◫ — the diff of the first ticked file.
   *
   * Reads the tree rather than the rendered rows, because a collapsed group has ticked files
   * and no rows at all: the button is enabled off `picked`, so searching the rows left it
   * enabled and inert whenever the user had collapsed the changelist.
   */
  showSelectedDiff: () => void
}

export interface GitPanelOptions {
  /**
   * Where a double-clicked file's diff goes.
   *
   * The panel fetches the `FileDiff` and hands it over; it does not own a pane and cannot open
   * one. Absent, the diff is fetched and reported as unavailable rather than silently dropped
   * — which is what the panel did before, and it made ◫ and double-click look broken.
   */
  onOpenDiff?: ((diff: FileDiff, repo: RepoId) => void) | undefined
}

export function useGitPanel(
  project: ProjectId | null,
  options: GitPanelOptions = {},
): GitPanelModel & GitPanelActions {
  const { onOpenDiff } = options
  // Read once. A story is a property of how the window was opened; re-reading the URL each
  // render would let a navigation swap the panel's data source mid-session.
  const [story] = useState<GitStory | null>(() => storyFromQuery())
  const initial = useMemo(() => (story ? normalizeStatus(story.status) : EMPTY), [story])

  const [view, setView] = useState<StatusView>(initial)
  const [shelf, setShelf] = useState<readonly ShelfRow[]>(() => story?.shelf ?? [])
  const [selected, setSelected] = useState<ReadonlySet<string>>(() => defaultSelection(initial))
  /*
   * The row selection and the cursor, both empty to begin with.
   *
   * Nothing is selected when the panel first paints, and that is the point: `defaultSelection`
   * above *ticks* the active changelist, because "Claude edited a file, commit it" should be
   * one click. Selecting those rows as well would mean the first drag in a session carried a
   * changelist nobody had pointed at — which is precisely the bug that made the ticks the
   * wrong thing to widen a drag by. The two sets start life disagreeing, deliberately.
   */
  const [selection, setSelection] = useState<RowSelection>(NO_ROWS)
  const [current, setCurrentRow] = useState<string | null>(null)
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => defaultExpanded(initial))
  const [loading, setLoading] = useState(false)
  const [unavailable, setUnavailable] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [message, setMessage] = useState('')
  const [amend, setAmendFlag] = useState(false)
  const [amendOf, setAmendOf] = useState<AmendTarget | null>(null)
  const [dialog, setDialog] = useState<ChangelistDialogState | null>(null)
  const [confirm, setConfirm] = useState<ConfirmState | null>(null)
  /** Repos where the user answered the guard bar. Cleared when the divergence clears. */
  const [answered, setAnswered] = useState<ReadonlySet<RepoId>>(new Set())
  /** Repos where the answer was "overwrite": the next commit is sent with `force: true`. */
  const overwritten = useRef<Set<RepoId>>(new Set())
  /** What the previous payload contained, so genuinely new rows can be treated as new. */
  const seenFiles = useRef<Set<string>>(new Set(allFiles(initial)))
  const seenGroups = useRef<Set<string>>(allGroups(initial))

  const rows = useMemo(() => buildRows(view, expanded), [view, expanded])
  const picked = useMemo(() => selectedFiles(view, selected), [view, selected])
  const carried = useMemo(() => carriedIds(rows, selection), [rows, selection])

  // Written by the diff pane, which is in another subtree of this same window — a module
  // store rather than a prop because the nearest common ancestor is the shell. It reaches no
  // *further* than this window; see `partialStore`'s header for why that is a limit and not a
  // bug. `getSnapshot` returns a cached array; a fresh one per call would re-render forever.
  const partials = useSyncExternalStore(
    subscribePartials,
    partialSnapshot,
    // The panel is server-rendered by `check:render`; React refuses a store read there
    // without this third argument.
    partialServerSnapshot,
  )

  /**
   * The current view, for callbacks that must not be rebuilt when it changes.
   *
   * `loadShelf` iterates the repositories and is called from an effect keyed on the Shelf tab.
   * If it closed over `view` it would get a new identity on every payload, the effect would
   * re-run, `setShelf` would render again, and the panel would spin: a render loop for as long
   * as the Shelf tab is open. Reading through a ref keeps the callback stable.
   */
  const viewRef = useRef(view)
  viewRef.current = view

  /**
   * What is in the commit box, for the amend effect below.
   *
   * Through a ref for the same reason `viewRef` exists, one failure sharper: the effect that
   * claims an amend request has to know whether the user has a draft, and putting `message` in
   * its dependency array would re-run it on **every keystroke** in the box. It would early-return
   * each time — there is nothing pending — but a per-keystroke effect over a module store is the
   * kind of thing that stops being harmless the moment somebody adds a second statement to it.
   */
  const messageRef = useRef(message)
  messageRef.current = message

  /**
   * Staging-area mode, as every repository in the workspace reports it.
   *
   * `every`, not `some`: the toggle is drawn pressed only when it is true everywhere, because
   * a half-pressed button over a workspace where one root is in staging-area mode and another
   * is not would claim something that is false for half the tree.
   */
  const stagingArea = useMemo(() => {
    const repos = allRepos(view)
    return repos.length > 0 && repos.every((r) => r.useStagingArea)
  }, [view])

  /**
   * Whether the next `git_status` should walk ignored files.
   *
   * Only once the user has actually opened an Ignored group. `include_ignored` is the
   * expensive half of a status walk on a checkout with a big `target/`, and asking for it
   * always would make every refresh — several a second while an agent edits — pay for rows
   * nobody is looking at.
   */
  const includeIgnored = useMemo(
    () => [...expanded].some((id) => isIgnoredGroupRow(id)),
    [expanded],
  )
  // Read inside `refresh` through a ref so that opening the Ignored group does not rebuild
  // `refresh`, whose identity drives the mount effect and the event subscription.
  const includeIgnoredRef = useRef(includeIgnored)
  includeIgnoredRef.current = includeIgnored

  /**
   * Which operation the line under the toolbar is currently about.
   *
   * Only that operation may clear it. Every write path ends in `await refresh()`, so a
   * blanket "any success clears the line" would wipe *why the commit failed* a few
   * milliseconds after showing it — the one message in this panel worth reading, replaced
   * by nothing at all because the status read that followed it went fine.
   */
  const shownFor = useRef<string | null>(null)

  const note = useCallback((what: string, line: string | null) => {
    if (line === null && shownFor.current !== what) return
    shownFor.current = line === null ? null : what
    setUnavailable(line)
  }, [])

  /**
   * Run a git call, or record why it could not run.
   *
   * Returns `undefined` on failure so callers branch on a value rather than on a `catch`.
   * That keeps every call site one line and makes an unguarded one visible in review.
   */
  const guarded = useCallback(
    async <T,>(what: string, call: () => Promise<T>) => {
      try {
        const value = await call()
        note(what, null)
        return value
      } catch (e) {
        /*
         * Through `explain`, not `String(e)`.
         *
         * Every command in `cmd/git.rs` returns `Result<_, GitError>`, so what arrives here is
         * the *serialised tagged object* and not an `Error`: `String({kind: 'notHead', …})` is
         * `[object Object]`, which is how a panel ends up reporting nothing at all in the one
         * place it had something to say. That is the bug `check:branches` was written for, one
         * surface over, and this panel had it on every git call it makes.
         *
         * It matters most for the arm this line was changed for. `require_amend_head` refuses an
         * amend whose target is no longer the tip, and `notHead`'s sentence names the commit that
         * *is* — "a1b2c3d is not the last commit — e5f6a7b is" — which is the whole difference
         * between a refusal the user can act on and a panel that says `[object Object]` under a
         * ticked Amend box. `explain` falls back to `error.message`/`String(error)` for anything
         * that is not a recognisable wire error, so nothing that used to read well reads worse.
         */
        const detail = explain(e)
        note(what, `${what} unavailable — ${detail}`)
        // Also to the app's stderr: the panel shows one line, and the reason a command is
        // missing is usually longer than one line.
        void diag.log(`git panel: ${what} failed: ${detail}`)
        return undefined
      }
    },
    [note],
  )

  /**
   * Adopt a new payload without losing what the user was doing.
   *
   * Selection and expansion survive a refresh — this repaints several times a second while
   * an agent edits, and a tree that re-ticked itself or sprang open under the pointer would
   * be unusable. Rows that are genuinely new are ticked if they landed in the active
   * changelist, which is what makes "Claude edited a file, commit it" one click; groups
   * that are genuinely new open unless they are the ignored group.
   *
   * `authoritative` says whether `next` is git's answer or a placeholder. The panel empties
   * itself on a failed `git status` and when there is no project, and those emptyings are not
   * evidence about anything — see the `pruneTo` call below, which is the one thing here that
   * destroys state the user cannot get back by waiting.
   */
  const adopt = useCallback((next: StatusView, authoritative = true) => {
    const live = allFiles(next)
    // A partial selection outlives the panel that made it (it lives in a module store shared
    // with the diff pane), so the payload that says a file is gone is also the only signal
    // that its stored positions are meaningless. Left behind, they would silently apply to
    // the *next* change to that path — a set of line numbers from a file that was committed
    // an hour ago.
    //
    // Only against a real answer, though. `refresh` adopts `EMPTY` when `git status` fails —
    // an index.lock held by a bash pane is enough — and pruning against that reads the
    // failure as "every file is gone" and drops every held selection. The panel's warning
    // line would go with it, so the next Commit would quietly write whole files where the
    // user had held lines back: the same silent wrong answer the rev check exists to stop,
    // arrived at from the other end.
    if (authoritative) {
      pruneTo(new Set(flatFiles(next).map((f) => liveKey(f.repo, f.entry.path))))
    }
    // What is genuinely new is decided *here*, before the two `seen` refs are replaced, and
    // never inside a state updater. React runs an updater eagerly only while the fiber has no
    // other update pending, and `refresh` always leaves one (`setLoading(false)` runs one line
    // earlier), so an updater that read `seenFiles.current` ran during the following render —
    // after the assignments below — and found every id already seen. Nothing was ever ticked
    // and no group was ever opened: the panel painted its rows and then sat there with every
    // changelist shut and Commit disabled. See `model.ts::arrivals`.
    const fresh = arrivals(next, seenFiles.current, seenGroups.current)
    const groups = allGroups(next)
    seenFiles.current = new Set(live)
    seenGroups.current = groups
    setSelected((prev) => {
      const kept = pruneSelection(live, prev)
      for (const id of fresh.files) kept.add(id)
      return kept
    })
    /*
     * The row selection is pruned and never *added* to, which is the difference between it and
     * the ticks one line up. A file that arrives in the active changelist is ticked, because a
     * commit is what the panel is for; selecting it as well would move the drag's load and the
     * menu's scope under the user's hand while an agent edits, several times a second.
     *
     * `groups` is `allGroups`, which already enumerates every expandable row id — repositories,
     * changelists *and* directory rows — so a selected folder survives here without a second
     * walk. Only against an authoritative payload, for the same reason `pruneTo` above is:
     * `refresh` adopts `EMPTY` when `git status` fails, and reading that as "every row is gone"
     * would clear a selection the user spent four ctrl-clicks building because a bash pane held
     * an index.lock for a moment.
     */
    if (authoritative) {
      const alive = new Set([...live, ...groups])
      setSelection((prev) => pruneRowSelection(alive, prev))
    }
    setExpanded((prev) => {
      // Identity is the bailout: this runs on every payload while an agent edits, and a new
      // Set each time would re-render the whole tree for a refresh that changed nothing.
      if (fresh.groups.length === 0) return prev
      const merged = new Set(prev)
      for (const id of fresh.groups) merged.add(id)
      return merged
    })
    setView(next)
    // A repo that stopped diverging drops its answer, so the next real divergence raises
    // the bar again instead of being suppressed by an answer to an older one.
    const stillDiverged = new Set(
      allRepos(next).flatMap((r) => (r.indexChangedExternally ? [r.id] : [])),
    )
    // …and it drops its *waiver* with it. `overwritten` used to be cleared only by a
    // successful commit, so "Overwrite" answered at 10:00 and never committed was still
    // armed at 14:00 — by which time the bar had come and gone. The next real `git add` in
    // a bash pane would then raise a fresh bar the user had not answered, and a commit sent
    // `force: true` anyway and clobbered it. That is the exact silent clobber this guard
    // exists to prevent, so the waiver dies with the divergence it answered.
    for (const id of overwritten.current) {
      if (!stillDiverged.has(id)) overwritten.current.delete(id)
    }
    setAnswered((prev) => {
      const still = new Set([...prev].filter((id) => stillDiverged.has(id)))
      return still.size === prev.size ? prev : still
    })
  }, [])

  /**
   * `viewOf`, plus the two facts in a tree that outlive the panel.
   *
   * Every `ChangesTree` names the absolute work tree of every repo in it, and the diff pane
   * needs exactly that to tell a tool call that touched *its* file from one that touched a
   * file at the same relative path in another project. The panel is where trees arrive first
   * — usually well before any diff tab exists, since a tab is opened by double-clicking a row
   * here — so noting it in passing costs nothing and saves the pane a `git_status` of its own.
   * See `repoRoots.ts`.
   *
   * The activity rail's badge is the second, and it is the same trade for the same reason.
   * `chrome/gitCountStore` is always live because the badge has to be right with the sidebar
   * on Files or shut — but while this panel *is* open, both would otherwise walk the work tree
   * on every burst, twice, to produce a number the first walk already contained. See
   * `noteChangeCount`.
   *
   * Not folded into `viewOf`: `model.ts` is compiled on its own by `check-git-tree.mjs`, and a
   * side effect in a pure mapping is not the sort of thing that stays in one place.
   */
  const absorb = useCallback(
    (tree: ChangesTree): StatusView => {
      noteRepoRoots(tree)
      if (project !== null) noteChangeCount(project, tree)
      return viewOf(tree)
    },
    [project],
  )

  const refresh = useCallback(async () => {
    if (story) return
    if (project === null) {
      adopt(EMPTY, false)
      return
    }
    setLoading(true)
    const raw = await guarded('git status', () =>
      gitApi.status(project, includeIgnoredRef.current),
    )
    setLoading(false)
    // `undefined` is the guard's failure signal; `{ repos: [] }` is a legitimate answer.
    // Keeping the previous tree after a failure would show changes that may no longer
    // exist, so a failure empties the panel.
    if (raw === undefined) adopt(EMPTY, false)
    else adopt(absorb(raw))
  }, [project, story, guarded, adopt, absorb])

  // One timer, shared by the mount refresh and by every event that invalidates the tree.
  const pending = useRef<number | null>(null)
  const schedule = useCallback(() => {
    if (pending.current !== null) window.clearTimeout(pending.current)
    pending.current = window.setTimeout(() => {
      pending.current = null
      void refresh()
    }, REFRESH_DEBOUNCE_MS)
  }, [refresh])

  useEffect(() => {
    void refresh()
    return () => {
      if (pending.current !== null) window.clearTimeout(pending.current)
    }
  }, [refresh])

  // Opening the Ignored group changes the *question*, not just its timing, so it re-asks
  // immediately rather than waiting for the next edit to trigger a refresh.
  const askedIgnored = useRef(includeIgnored)
  useEffect(() => {
    if (askedIgnored.current === includeIgnored) return
    askedIgnored.current = includeIgnored
    if (includeIgnored) schedule()
  }, [includeIgnored, schedule])

  useEffect(() => {
    if (story) return
    // `gone` rather than just holding the unlisten in a variable: `listen` resolves a tick
    // or two after the effect runs, so a panel unmounted in between (the sidebar switching
    // back to Files) would never see the handle and would leave a listener firing
    // `git_status` on every tool event for the rest of the session, one more per remount.
    let gone = false
    const unlisten: Array<() => void> = []
    const track = (p: Promise<() => void>, what: string) => {
      void p
        .then((fn) => {
          if (gone) fn()
          else unlisten.push(fn)
        })
        .catch((e) => diag.log(`git panel: ${what} events unavailable: ${String(e)}`))
    }
    track(events.onSessionTool(() => schedule()), 'tool')
    // A mutation cide made anywhere — including in another window — arrives with the new
    // tree already computed, so this costs no round trip.
    //
    // `cmd/git.rs::refreshed` always broadcasts with `include_ignored: false`, so an open
    // Ignored group empties for as long as it takes the mutation's own trailing `refresh()`
    // to answer. Adopting the broadcast anyway is still right: it is the only signal another
    // window's commit produces, and briefly missing the ignored rows is a smaller lie than
    // showing files that were just committed away.
    track(
      events.onGitStatus((forProject, tree) => {
        if (project !== null && forProject === project) adopt(absorb(tree))
      }),
      'git-status',
    )
    return () => {
      gone = true
      for (const fn of unlisten) fn()
    }
  }, [schedule, story, project, adopt, absorb])

  const toggleCheck = useCallback((row: Row) => setSelected((prev) => toggleRow(row, prev)), [])

  /*
   * Space over a multi-row selection ticks all of it.
   *
   * Delegated to `model.ts::toggleRows`, and the delegation is the fix rather than a tidy-up.
   *
   * This was a fold — one `toggleRow` per selected row — on the reasoning that `toggleRow`
   * owns the rule that a partly-ticked subtree *clears* rather than completes, so folding it
   * avoided a second copy of that rule. The reasoning was right and the fold did not achieve
   * it: a row's `files` is its whole subtree and a selection spanning a folder contains the
   * folder *and* its children, so every file was toggled twice and came back unchanged. What
   * survived was the inverse of the rule — any tick anywhere left the subtree fully ticked —
   * which in this panel means silently re-ticking a file the user excluded from the commit.
   *
   * `toggleRows` unions the subtrees and applies the rule once, so there is genuinely one copy
   * now. It also lives where `check-git-tree.mjs` can compile and run it; this hook is the one
   * place the check cannot reach, which is why the defect was invisible to all 29 gates.
   *
   * It does not touch the selection, and the selection does not touch the ticks. That is the
   * whole distinction this feature rests on.
   */
  const toggleCheckSelected = useCallback(() => {
    setSelected((prev) => toggleRows(rows, selection.ids, prev))
  }, [rows, selection])

  const setCurrent = useCallback((id: string) => setCurrentRow(id), [])

  /**
   * A left press. The cursor always moves; what happens to the selection is `pressSelect`'s.
   *
   * Returns whether the collapse was deferred so the component can hand the same id back on
   * mouseup — the state that says "a press is pending" belongs to the gesture, which is the
   * component's, not to the model.
   */
  const pressRow = useCallback(
    (id: string, mods: SelectMods) => {
      const plan = pressSelect(rows, selection, id, mods)
      setCurrentRow(id)
      if (!plan.deferred) setSelection(plan.next)
      return plan.deferred
    },
    [rows, selection],
  )

  const releaseRow = useCallback(
    (id: string) => setSelection((prev) => releaseSelect(rows, prev, id)),
    [rows],
  )

  const keyToRow = useCallback(
    (id: string, mods: SelectMods) => {
      setCurrentRow(id)
      setSelection((prev) => keySelect(rows, prev, id, mods))
    },
    [rows],
  )

  const selectAllRows = useCallback(() => setSelection(selectAll(rows)), [rows])

  const collapseSelection = useCallback(() => setSelection(collapseTo(current)), [current])

  const toggleExpand = useCallback((row: Row) => {
    if (!row.expandable) return
    setExpanded((prev) => {
      const next = new Set(prev)
      if (!next.delete(row.id)) next.add(row.id)
      return next
    })
  }, [])

  const setAllExpanded = useCallback(
    (open: boolean) => setExpanded(open ? allGroups(view) : new Set<string>()),
    [view],
  )

  /**
   * Ticking `Amend` **by hand** does not prefill the message, and clears any commit the log
   * named.
   *
   * The prefill half is unchanged and the argument for it is unchanged: it used to fill the box
   * from a `headMessage` field the panel invented; `RepoChanges` carries no such thing and
   * inventing one here would mean a second round trip per repo on every refresh for a string
   * that is only read when a checkbox is ticked. The box is left alone rather than filled with a
   * guess — `git commit --amend` keeps the old message when none is given, so an empty box
   * amends without rewriting the subject.
   *
   * The second half is new and it is a safeguard rather than tidiness. `amendOf` is set only by
   * the git log's *Amend…*, which names one commit; the moment the user touches this checkbox
   * themselves they have gone back to the panel's own meaning — amend whatever HEAD is — and a
   * leftover oid would either send a guard value for a commit nobody named or, worse, survive an
   * untick and a re-tick and arm a rewrite of a commit the user had just cancelled out of.
   * Cleared on **both** edges for that reason: unticking abandons the request, and re-ticking is
   * a fresh statement about HEAD.
   */
  const setAmend = useCallback((on: boolean) => {
    setAmendFlag(on)
    setAmendOf(null)
  }, [])

  /**
   * Adopt an amend request: tick the box, fill it in, and remember which commit.
   *
   * All three together, always. Two of the three is the half-finished control the log's item was
   * disabled rather than become — ticking Amend without the message rewrites HEAD's subject with
   * whatever happens to be in the box, and filling the message without the tick writes a *new*
   * commit carrying the old one's words.
   */
  const adoptAmend = useCallback(
    (request: AmendRequest) => {
      setMessage(request.message)
      setAmendFlag(true)
      // `as RepoId` because `panelRequests.ts` is import-free — it cannot name the alias, and
      // says so. The two are the same type today (`export type RepoId = string`); the cast is
      // the one documented place where that equivalence is relied on, so a `RepoId` that ever
      // becomes a branded type fails here rather than silently everywhere.
      setAmendOf({ repo: request.repo as RepoId, oid: request.oid, shortOid: request.shortOid })
      /*
       * And bring the commit box into view.
       *
       * Not a flourish, and not only the caret. `CommitBox` is mounted **under the Commit tab
       * only**, so an amend adopted while the panel is on Shelf would fill a message box nobody
       * can see — the item would appear to do nothing, which is the state it was disabled to
       * avoid. `GitPanel.tsx` watches this same request and switches the tab; `CommitBox`
       * consumes it one render later. `git.commit` asks for the identical thing for the
       * identical reason.
       */
      requestFocus('commitMessage')
    },
    [],
  )

  /**
   * The git log asked for an amend. (M19)
   *
   * `chrome/panelRequests.ts` parks the request and reveals this panel; this is the far end. The
   * store is read through `useSyncExternalStore` rather than a prop because the two ends are in
   * different subtrees of one window — the log is a tool window at the bottom, this is the
   * sidebar — and the nearest common React ancestor is the shell itself. Same shape and same
   * reason as `partialStore` above.
   *
   * A boolean snapshot, claimed inside the effect. The value is never rendered: it is read,
   * decided upon and spent, and a claim is the only thing that stops a request from firing again
   * on an unrelated mount later. The panel unmounts every time the user clicks another icon in
   * the activity rail, and a flag that survived would mean that merely *looking* at this panel
   * ticked Amend and replaced whatever was in the box.
   *
   * Four ways this ends, and only the first is the happy one:
   *
   *   * the box is empty — adopt, silently. The user asked for this by name;
   *   * the box holds a draft — ask, through the panel's own `ConfirmDestructive`. `planAmend`
   *     owns that decision and the sentence, so a check script can run it;
   *   * the request names another project — refuse on the panel's one dim line. Not silence: the
   *     user made a gesture in the log and something has to answer it;
   *   * the claim comes back `null` — expired, or already spent by an earlier run of this same
   *     effect under StrictMode. Silent, and that is the one case where silence is right: there
   *     is no gesture behind it to answer.
   */
  const amendWanted = useSyncExternalStore(
    subscribeAmendRequest,
    amendPending,
    // The panel is server-rendered by `check:render`; React refuses a store read there without
    // this third argument, exactly as it does for the partials above.
    amendPendingServer,
  )
  useEffect(() => {
    if (!amendWanted) return
    const request = claimAmend()
    // Stale, or claimed by an earlier run of this effect. `claimAmend` spends an expired request
    // rather than leaving it for the next mount to examine — see its header — so `null` here is
    // ordinary and not an error worth a line on the panel.
    if (request === null) return
    if (project === null || request.project !== project) {
      /*
       * A request about a project this panel is not showing.
       *
       * Possible with two projects open: the sidebar follows the active project and a log tool
       * window belongs to the one it was opened in. Refused rather than adopted, because
       * prefilling a commit message from a repository that is not on screen would arm a rewrite
       * of a HEAD the user cannot see — and refused *out loud*, because the alternative is the
       * menu item doing nothing at all, which is the state it was disabled to avoid.
       */
      note('git amend', 'Amend is for another project — open its window to amend that commit.')
      return
    }
    // A previous refusal is answered by this request. `note` only lets the operation that wrote
    // the line clear it, so without this the sentence above would be the one thing in the panel
    // with no way out: nothing else is keyed `git amend`, so no later success would ever wipe it.
    note('git amend', null)
    const plan = planAmend(messageRef.current, request)
    if (plan.kind === 'adopt') {
      adoptAmend(request)
      return
    }
    setConfirm({
      title: plan.title,
      body: plan.body,
      /*
       * Empty, and this is the one caller of `ConfirmDestructive` where that is right.
       *
       * Rule 1 of that component is "name what would be lost, every path, not a count" — and
       * what would be lost here is not a path. The list runs every entry through
       * `basename`/`dirname`, so a draft reading `fix: handle a/b paths` would be drawn as the
       * file `b paths` in the directory `fix: handle a`: the dialog misquoting the very text it
       * is asking permission to destroy. So the draft is named in the body instead, quoted whole
       * where it fits, which keeps the rule and drops only the formatting.
       */
      files: [],
      confirmLabel: plan.confirmLabel,
      run: () => adoptAmend(request),
    })
  }, [amendWanted, project, adoptAmend, note])

  /**
   * Which repository a reword is about, or `null` if this is not one.
   *
   * `amendOf.repo` when the log named a commit — the route this exists for. Otherwise the bare
   * Amend checkbox, which means "HEAD", and only answers in a single-root project: with several
   * roots there are several HEADs and picking one would rewrite whichever sorted first. In that
   * case it stays `null`, `units` stays empty and Commit does nothing — which is why
   * `model.ts::canCommit` is not the only gate, and why this is computed beside it rather than
   * inside it.
   */
  const rewordRepo = useMemo(() => {
    if (!amend) return null
    const all = allRepos(view)
    const named = amendOf !== null ? amendOf.repo : all.length === 1 ? (all[0]?.id ?? null) : null
    if (named === null) return null
    // An unborn branch has no commit to reword. Checked against the *named* repository rather
    // than `canAmend`'s "some repository has a HEAD", which is the right question for the
    // checkbox and the wrong one here: in a monorepo with one fresh root, `canAmend` is true
    // while the root being reworded may be the empty one.
    return all.some((r) => r.id === named && !r.branch.unborn) ? named : null
  }, [amend, amendOf, view])

  /** The ticks, split by repo, with the changelist named when they all came from one. */
  const units = useMemo(
    () => commitUnits(view, selected, rewordRepo),
    [view, selected, rewordRepo],
  )

  /** Repos whose index moved under us and whose bar has not been answered yet. */
  const diverged = useMemo(
    () =>
      allRepos(view).flatMap((r) =>
        r.indexChangedExternally && !answered.has(r.id) ? [r.id] : [],
      ),
    [view, answered],
  )

  const commit = useCallback(
    (push: boolean) => {
      if (project === null || units.length === 0) return
      void (async () => {
        setBusy(push ? 'Committing and pushing…' : 'Committing…')
        let allOk = true
        for (const unit of units) {
          /*
           * An unanswered guard bar stops the commit here rather than at the backend.
           *
           * `cide_git::commit` refuses on its own when the index moved, so this is belt and
           * braces — but it is the half that can say *which* repo and what to do about it,
           * and it costs no round trip.
           */
          if (diverged.includes(unit.repo)) {
            const label = repoOf(view, unit.repo)?.name ?? unit.repo
            note(
              'git commit',
              `git commit refused — ${label}'s index changed outside cide; `
                + 'answer Reload or Overwrite first',
            )
            allOk = false
            break
          }
          const outcome = await guarded('git commit', () =>
            gitApi.commit(project, unit.repo, {
              message,
              amend,
              /*
               * The commit the log named, for the repository it named it in — and `null`
               * everywhere else.
               *
               * `null` used to be unconditional here, with a comment saying this checkbox is
               * *about* HEAD and cannot name anything else. That was true of the checkbox and
               * has stopped being true of one path into it: the git log's *Amend…* is drawn on
               * a row, so it does name a commit, and it arrives through
               * `chrome/panelRequests.ts` with the oid. Sending it is not a formality —
               * `cide_git::commit::require_amend_head` refuses anything but the tip with
               * `GitError::NotHead`, and HEAD moves: another window commits, or a `git commit`
               * is typed into a bash pane, between the menu opening and Commit being pressed.
               * Without the oid that becomes a silent rewrite of the wrong commit.
               *
               * Per repository, because a `CommitUnit` is per repository and an oid is the tip
               * of exactly one of them. A monorepo commit that amends across two roots sends
               * the oid to the root it came from and `null` to the others, which is the
               * checkbox's own meaning and the only honest answer for a repo nobody named a
               * commit in.
               */
              amendOf: amend && amendOf?.repo === unit.repo ? amendOf.oid : null,
              changelist: unit.changelist,
              // The whole file, unless the diff pane left a partial selection for it. This
              // is the one line that makes per-hunk staging reach a commit; `commitSelections`
              // is where the rule lives that only a `combined` selection may be honoured,
              // because that is the side `cide_git::commit` re-derives.
              selections: commitSelections(unit.repo, unit.paths),
              // `false` unless the user pressed Overwrite on this repo's bar. This is the
              // waiver for `cide_git::commit`'s `require_index_unchanged`, and defaulting it
              // to `true` — which is what the panel used to do — turns the guard off for
              // every commit rather than for the one the user asked to waive.
              force: overwritten.current.has(unit.repo),
            }),
          )
          if (outcome === undefined) {
            allOk = false
            break
          }
          if (push) {
            const ok = await guarded('git push', () => gitApi.push(project, unit.repo, null, null))
            if (ok === undefined) {
              allOk = false
              break
            }
          }
          overwritten.current.delete(unit.repo)
        }
        setBusy(null)
        // The box is cleared only on success. A failed commit that also loses the message
        // is two problems, and the message is the one the user cannot reconstruct.
        if (allOk) {
          setMessage('')
          setAmendFlag(false)
          // With the tick. The commit the log named no longer exists under that oid — an amend
          // writes a new object — so keeping it would arm the *next* commit against a target
          // that is already gone, and `require_amend_head` would refuse it with a sentence
          // about a commit the user has forgotten asking about.
          setAmendOf(null)
        }
        await refresh()
      })()
    },
    [project, units, view, diverged, message, amend, amendOf, guarded, note, refresh],
  )

  const unstage = useCallback(() => {
    if (project === null || units.length === 0) return
    void (async () => {
      setBusy('Unstaging…')
      for (const unit of units) {
        await guarded('git unstage', () =>
          gitApi.unstage(project, unit.repo, wholeFiles(unit.paths)),
        )
      }
      setBusy(null)
      await refresh()
    })()
  }, [project, units, guarded, refresh])

  /**
   * One file, staged / unstaged / rolled back — the context menu's three verbs.
   *
   * Deliberately **not** routed through `units`, which is the ticked selection. The menu acts
   * on the row that was right-clicked, and that row is very often not ticked: a menu whose
   * *Stage* silently staged four other files because they happened to have ticks would be the
   * worst kind of surprise in a panel that writes to the index.
   *
   * One helper for all three because the three differ only in the command and the word on the
   * busy line. `wholeFiles` is right for each of them: these are whole-file verbs, and a held
   * partial selection belongs to the diff pane and to `commit`, which resolves it against
   * `Combined` — see the note on `wholeFiles`.
   */
  const fileVerb = useCallback(
    (
      what: string,
      busyLabel: string,
      call: (
        project: ProjectId,
        repo: RepoId,
        selections: PathSelection[],
      ) => Promise<ChangesTree>,
    ) =>
      (repo: RepoId, path: string) => {
        if (project === null) return
        void (async () => {
          setBusy(busyLabel)
          await guarded(what, () => call(project, repo, wholeFiles([path])))
          setBusy(null)
          await refresh()
        })()
      },
    [project, guarded, refresh],
  )

  const stageFile = useMemo(() => fileVerb('git stage', 'Staging…', gitApi.stage), [fileVerb])
  const unstageFile = useMemo(
    () => fileVerb('git unstage', 'Unstaging…', gitApi.unstage),
    [fileVerb],
  )
  /**
   * The rollback itself, once the confirmation has been answered.
   *
   * Deliberately private. Everything that reaches this panel's most destructive command goes
   * through `revertFiles`/`revertGroup`, which put a dialog in front of it — `git_rollback`
   * checks HEAD out over the tracked paths and *deletes* the untracked ones, and `cmd/git.rs`
   * will not ask ("a confirmation the backend cannot show is not a safeguard"). Keeping the
   * unguarded version out of `GitPanelActions` is what stops the next caller from skipping it.
   */
  const doRollback = useCallback(
    (repo: RepoId, paths: string[]) => {
      if (project === null || paths.length === 0) return
      void (async () => {
        setBusy('Reverting…')
        await guarded('git rollback', () => gitApi.rollback(project, repo, wholeFiles(paths)))
        setBusy(null)
        await refresh()
      })()
    },
    [project, guarded, refresh],
  )

  // --- changelists ---------------------------------------------------------------------------

  /**
   * Run a changelist mutation and adopt the tree it answers with.
   *
   * Every `git_changelist_*` handler returns the fresh `ChangesTree` rather than an
   * acknowledgement, precisely so the panel does not have to ask again — see the header of
   * `cmd/git.rs`. Calling `refresh()` after one would be a second full status walk of every
   * root, and the frame in between shows a tree that is visibly wrong.
   */
  const mutate = useCallback(
    (what: string, busyLabel: string, call: (p: ProjectId) => Promise<ChangesTree>) => {
      if (project === null) return
      void (async () => {
        setBusy(busyLabel)
        const tree = await guarded(what, () => call(project))
        setBusy(null)
        // A failure has already been reported by `guarded`; re-reading status is how the panel
        // gets back to something true rather than to whatever it had before the attempt.
        if (tree === undefined) await refresh()
        else adopt(absorb(tree))
      })()
    },
    [project, guarded, refresh, adopt, absorb],
  )

  const newChangelist = useCallback(
    (repo?: RepoId) => {
      const target = repo ?? allRepos(view)[0]?.id
      if (target === undefined) return
      setDialog({
        mode: 'create',
        repo: target,
        repoName: repoLabel(view, target),
        lists: changelistsOf(view, target),
        id: null,
        name: '',
        paths: [],
        track: false,
      })
    },
    [view],
  )

  const renameChangelist = useCallback(
    (repo: RepoId, id: string) => {
      const lists = changelistsOf(view, repo)
      setDialog({
        mode: 'rename',
        repo,
        repoName: repoLabel(view, repo),
        lists,
        id,
        name: lists.find((l) => l.id === id)?.name ?? '',
        paths: [],
        track: false,
      })
    },
    [view],
  )

  const moveToChangelist = useCallback(
    /**
     * `track` says these paths are unversioned, so picking a list adds them to git first —
     * the context menu's route to `dragDrop.ts`'s `track` outcome. It is a required argument
     * rather than one defaulting to `false`, because every call site knows the answer from
     * the row it was opened on and a default would let a new one silently promise a plain
     * re-filing over files git has never seen.
     */
    (repo: RepoId, paths: string[], track: boolean) => {
      if (paths.length === 0) return
      // The list the paths are in *now*, when they all share one — the dialog disables that
      // row, because moving a file to where it already is is a gesture that appears to work
      // and changes nothing.
      //
      // Never for an unversioned set. `ChangeEntry.changelist` comes from `Sidecar::owner_of`,
      // which answers *the active list* for anything nobody has filed — so an untracked path
      // claims to be in `Changes` while being drawn under `Unversioned Files` and being in no
      // list at all. Trusting it here greyed out the active changelist, which is the one row
      // most of these files are headed for. Same fact `dropOutcome`'s `track` branch is built
      // around.
      const here = track
        ? new Set<string>()
        : new Set(
            flatFiles(view)
              .filter((f) => f.repo === repo && paths.includes(f.entry.path))
              .map((f) => f.changelist),
          )
      setDialog({
        mode: 'move',
        repo,
        repoName: repoLabel(view, repo),
        lists: changelistsOf(view, repo),
        id: here.size === 1 ? ([...here][0] ?? null) : null,
        name: '',
        paths,
        track,
      })
    },
    [view],
  )

  /**
   * The drop half of drag and drop: file these paths, no dialog.
   *
   * Through `mutate`, so the tree the command answers with is adopted directly — a `refresh()`
   * here would be a second full status walk of every root, and the frame in between shows the
   * files back where they came from, which reads as the drop having failed.
   */
  const movePaths = useCallback(
    (repo: RepoId, changelist: string, paths: string[]) => {
      // An empty list is what `dropOutcome` returns for a drop onto the list the files are
      // already in. Sending it would be a round trip and a busy line for a no-op.
      if (paths.length === 0) return
      mutate('git changelist move', 'Moving…', (p) =>
        gitApi.changelist.movePaths(p, repo, changelist, paths),
      )
    },
    [mutate],
  )

  /**
   * The other drop: add unversioned paths to git, *then* file them.
   *
   * > *"i should be able to move files from Unversioned Files to any of change list via drag
   * > and drop and via context ... but when dragging it should stage them automatically."*
   *
   * # Why this is a `git add` and not something lighter
   *
   * A changelist assignment for an untracked path is a write that undoes itself.
   * `status::repo_changes` builds its `live` set from every entry that is neither `Untracked`
   * nor `Ignored` and hands it to `Sidecar::reconcile`, which drops every assignment outside
   * it — so filing an untracked path succeeds in the sidecar and is gone by the next status
   * walk, roughly 150 ms later. `dropOutcome` used to refuse the drag for exactly that reason.
   * The path has to *leave* the untracked state for the assignment to survive, and the only
   * thing that does that is putting it in the index.
   *
   * `git add -N` (intent-to-add) was the lighter option and it loses twice. It does not buy
   * the thing that matters — `commit::rebuild_index` resets the index to HEAD before writing
   * the changelist anyway, so an intent-to-add entry is no more durable than a full one — and
   * `cide-git` has no way to make one: every IA entry in its tests is created by forking the
   * `git` binary (`an_intent_to_add_entry_stages_by_hunk`), because git2 offers it only by
   * hand-setting `IndexEntry` extended flags, which is not a thing to invent on the one code
   * path in this project where a mistake destroys uncommitted work. `stage::stage` is the call
   * every other staging gesture in the panel already goes through, property-tested against
   * real `git apply --cached`, and for a whole untracked file it does what `git add` does —
   * `assert_matches_git_add` is the test that says so.
   *
   * # What it costs, per ADR 0004
   *
   * The file is now in `.git/index`, and the index is a derived artifact here: committing a
   * *different* changelist runs `rebuild_index`, which resets the index to HEAD and writes
   * only that list's selections. The newly added file's index entry goes with it, the path is
   * untracked again, and `reconcile` then drops the changelist assignment it just earned — so
   * it reappears under `Unversioned Files`. That is the changelists-are-the-truth model doing
   * exactly what the ADR says it does, not a bug in this path, and it is why the ghost says
   * *"Add N files to git"* rather than *"track N files"*: the promise made is the one that is
   * kept. Committing the list the files were filed into commits them, which is the case the
   * gesture is actually for.
   *
   * The staging does **not** raise the external-index guard bar: `stage::stage` ends in
   * `changelist::record_index`, so the fingerprint the guard compares against is cide's own
   * new one. A drag that accused the user of staging outside cide would be absurd.
   *
   * # Two calls, and what happens when the first one fails
   *
   * There is no single command for this, and it is deliberately *not* dressed as one that
   * cannot fail halfway. If the add fails nothing else is attempted — the move would be a
   * write the next status walk erases — and the line says nothing was added. If the add
   * succeeds and the move fails, the line says that too, naming both halves, because the
   * files are then in git and in the *active* changelist rather than the one they were
   * dropped on, and a message about a failed "move" alone would leave the user with no idea
   * their repository had changed. Silence in either case is the failure this panel has had
   * before.
   */
  const trackPaths = useCallback(
    (repo: RepoId, changelist: string, paths: string[]) => {
      if (project === null || paths.length === 0) return
      void (async () => {
        // One busy line for the whole gesture. It is one operation to the person who dropped
        // the files, and a label that flipped from `Adding to git…` to `Moving…` halfway would
        // be reporting cide's plumbing rather than what they asked for.
        setBusy('Adding to git…')
        let added = false
        try {
          await gitApi.stage(project, repo, wholeFiles(paths))
          added = true
          const moved = await gitApi.changelist.movePaths(project, repo, changelist, paths)
          note(TRACK_NOTE, null)
          setBusy(null)
          // The command's own answer, adopted directly — see `mutate` for why a `refresh()`
          // here would show a frame of the files back where they came from.
          //
          // There is still one frame in between, and it is honest: `git_stage` broadcasts its
          // own tree, in which the files are added to git and sit in the *active* changelist
          // (`Sidecar::owner_of` answers the active list for anything unfiled). That is what
          // the repository actually contains at that instant. Collapsing the two calls into
          // one command would remove the frame and is the only thing that would; it is not
          // worth a new entry in the IPC contract for a flicker that shows the truth.
          adopt(absorb(moved))
          return
        } catch (e) {
          const detail = e instanceof Error ? e.message : String(e)
          note(
            TRACK_NOTE,
            added
              ? `${files(paths.length)} were added to git, but the move failed — ${detail}`
              : `nothing was added to git — ${detail}`,
          )
          void diag.log(`git panel: add to git and file failed: ${detail}`)
        }
        setBusy(null)
        // Whatever happened, the tree on screen is now a guess. A status walk is how the panel
        // gets back to something true — including, after a half-applied gesture, showing the
        // files where they actually ended up.
        await refresh()
      })()
    },
    [project, note, refresh, adopt, absorb],
  )

  const dismissDialog = useCallback(() => setDialog(null), [])

  const deleteChangelist = useCallback(
    (repo: RepoId, id: string) =>
      mutate('git changelist delete', 'Deleting changelist…', (p) =>
        gitApi.changelist.delete(p, repo, id),
      ),
    [mutate],
  )

  const setActiveChangelist = useCallback(
    (repo: RepoId, id: string) =>
      mutate('git changelist active', 'Switching changelist…', (p) =>
        gitApi.changelist.setActive(p, repo, id),
      ),
    [mutate],
  )

  const pickChangelist = useCallback(
    (id: string) => {
      if (dialog === null || dialog.mode !== 'move') return
      const { repo, paths, track } = dialog
      setDialog(null)
      // The same two operations the drop has, chosen the same way. The chooser is the
      // keyboard's route to a gesture the pointer makes by dragging, and a route that ran a
      // plain move over unversioned paths would write an assignment the next status walk
      // erases — the silent no-op this whole item is about.
      if (track) {
        trackPaths(repo, id, [...paths])
        return
      }
      mutate('git changelist move', 'Moving…', (p) =>
        gitApi.changelist.movePaths(p, repo, id, [...paths]),
      )
    },
    [dialog, mutate, trackPaths],
  )

  /**
   * The chooser's name field: create, rename, or create-and-move.
   *
   * The last one is two round trips and cannot be one: `git_changelist_create` answers with
   * the new `ChangesTree`, not with the id it minted, and that id is a slug of the name with a
   * collision suffix the frontend must not try to reproduce. `findChangelistId` reads the id
   * back out of the answer, which is exact. A dedicated `create_with_paths` command would make
   * it atomic; it is not worth a new entry in the IPC contract for a window in which the only
   * thing that can go wrong is that the list exists and the files did not move — visibly, in
   * the tree, with the list right there to drop them on.
   */
  const submitChangelistName = useCallback(
    (name: string) => {
      if (dialog === null || project === null) return
      const { mode, repo, id, paths, track } = dialog
      setDialog(null)
      if (mode === 'rename') {
        if (id === null) return
        mutate('git changelist rename', 'Renaming…', (p) =>
          gitApi.changelist.rename(p, repo, id, name),
        )
        return
      }
      void (async () => {
        setBusy(mode === 'move' ? 'Moving…' : 'Creating changelist…')
        const created = await guarded('git changelist create', () =>
          gitApi.changelist.create(project, repo, name),
        )
        if (created === undefined) {
          setBusy(null)
          await refresh()
          return
        }
        if (mode !== 'move' || paths.length === 0) {
          setBusy(null)
          adopt(absorb(created))
          return
        }
        const fresh = findChangelistId(created, repo, name)
        if (fresh === null) {
          setBusy(null)
          // The list was created but cannot be found by name, so the files stayed put. Said
          // out loud: a silent half-move is the failure this whole path exists to avoid.
          note(
            'git changelist move',
            `“${name}” was created but the files did not move — move them from the menu`,
          )
          adopt(absorb(created))
          return
        }
        // Create-and-move over unversioned paths is create-and-*add-and*-move. `trackPaths`
        // owns both halves and their failure wording; duplicating the add here would be a
        // second place for the two routes to disagree about what a drop into a brand new
        // changelist does.
        if (track) {
          setBusy(null)
          adopt(absorb(created))
          trackPaths(repo, fresh, [...paths])
          return
        }
        const moved = await guarded('git changelist move', () =>
          gitApi.changelist.movePaths(project, repo, fresh, [...paths]),
        )
        setBusy(null)
        if (moved === undefined) await refresh()
        else adopt(absorb(moved))
      })()
    },
    [dialog, project, mutate, guarded, refresh, adopt, absorb, note, trackPaths],
  )

  // --- reverting, which is the one thing here with no undo ------------------------------------

  const revertFiles = useCallback(
    (repo: RepoId, paths: string[], untracked = false) => {
      if (paths.length === 0) return
      setConfirm({
        // The same split `revertGroup` makes, and for the same reason: git has nothing to
        // restore an untracked path from, so `stage::rollback` removes the file. A dialog
        // saying "goes back to its last committed state" over a delete is the one wording in
        // this panel that could cost work the user cannot get back.
        title: untracked
          ? `Delete ${files(paths.length)}?`
          : paths.length === 1
            ? 'Revert this file?'
            : `Revert ${files(paths.length)}?`,
        body: untracked
          ? 'Git is not tracking these files, so there is nothing to restore them from. '
            + 'They are deleted from disk.'
          : 'These files go back to their last committed state. Uncommitted work in them is '
            + 'thrown away, and git has no undo for it.',
        files: paths,
        confirmLabel: untracked
          ? `Delete ${files(paths.length)}`
          : `Revert ${files(paths.length)}`,
        run: () => doRollback(repo, paths),
      })
    },
    [doRollback],
  )

  /** **Opens the confirmation.** The menu marks it `danger` and the dialog names the file. */
  const rollbackFile = useCallback(
    (repo: RepoId, path: string) => revertFiles(repo, [path]),
    [revertFiles],
  )

  /**
   * *"i should be able to revert the group"*.
   *
   * The confirmation names every file rather than counting them, because the user is about to
   * lose *specific* work — see `ConfirmDestructive`. The wording splits on the group's kind:
   * an unversioned file has no committed state to go back to, so `stage::rollback` deletes it
   * from disk, and calling that "revert" would be a lie about what the button does.
   */
  const revertGroup = useCallback(
    (repo: RepoId, group: string) => {
      const found = groupOf(view, repo, group)
      if (found === undefined || found.entries.length === 0) return
      const paths = found.entries.map((e) => e.path)
      const untracked = found.kind === 'unversioned'
      setConfirm({
        title: untracked
          ? `Delete ${files(paths.length)} in “${found.name}”?`
          : `Revert “${found.name}”?`,
        body: untracked
          ? 'Git is not tracking these files, so there is nothing to restore them from. '
            + 'They are deleted from disk.'
          : `Every change in this changelist goes back to its last committed state. `
            + 'Uncommitted work in these files is thrown away, and git has no undo for it.',
        files: paths,
        confirmLabel: untracked
          ? `Delete ${files(paths.length)}`
          : `Revert ${files(paths.length)}`,
        run: () => doRollback(repo, paths),
      })
    },
    [view, doRollback],
  )

  const dismissConfirm = useCallback(() => setConfirm(null), [])

  const runConfirm = useCallback(() => {
    // Cleared first, then run — and the run is *outside* the updater on purpose. React calls a
    // state updater twice under StrictMode and may replay it at will, so a `run()` in there
    // would roll back two changelists for one click. The same trap `useContextMenu::close`
    // documents for its focus restore.
    const pending = confirm
    setConfirm(null)
    pending?.run()
  }, [confirm])

  /**
   * Read every repo's shelf.
   *
   * Called when the Shelf tab opens, after anything shelves, and from the panel's own ↻ —
   * which cannot know which tab is showing, and whose whole meaning is "re-read everything".
   * Never on a timer and never from a status refresh, because it is one round trip per
   * repository and the Shelf tab is its only reader. The panel used to hold a `useState` that
   * nothing ever wrote to, so the Shelf tab showed the fixture or nothing at all, forever.
   */
  const loadShelf = useCallback(async () => {
    if (story || project === null) return
    const out: ShelfRow[] = []
    for (const repo of allRepos(viewRef.current)) {
      const entries = await guarded('git shelf', () => gitApi.shelf.list(project, repo.id))
      for (const entry of entries ?? []) {
        out.push({ repo: repo.id, key: `${repo.id}/${entry.id}`, entry })
      }
    }
    setShelf(out)
  }, [project, story, guarded])

  const refreshShelf = useCallback(() => void loadShelf(), [loadShelf])

  const shelve = useCallback(() => {
    if (project === null || units.length === 0) return
    void (async () => {
      setBusy('Shelving…')
      const name = message.trim() === '' ? 'Shelved changes' : message.trim()
      for (const unit of units) {
        // `shelf::shelve` resolves against `Combined`, the same side as commit, so a stored
        // partial selection is meaningful here too: shelving half a file is exactly what the
        // shelf is for.
        await guarded('git shelve', () =>
          gitApi.shelf.shelve(project, unit.repo, name, commitSelections(unit.repo, unit.paths)),
        )
      }
      setBusy(null)
      await refresh()
      await loadShelf()
    })()
  }, [project, units, message, guarded, refresh, loadShelf])

  /**
   * Shelve one whole group under its own name — the group menu's `Shelve Changelist`.
   *
   * Not routed through `units`, which is the ticked selection: the menu acts on the group that
   * was right-clicked, and a changelist is very often not the one that is ticked. The name is
   * the changelist's own rather than the commit message, because a shelf entry called after
   * the list it came from is the one a user can find again.
   *
   * `commitSelections` rather than `wholeFiles`, so a partial selection held by the diff pane
   * is honoured here too — shelving half a file is exactly what the shelf is for, and
   * `shelf::shelve` resolves against `Combined`, the same side those selections were made on.
   */
  const shelveGroup = useCallback(
    (repo: RepoId, group: string) => {
      if (project === null) return
      const found = groupOf(view, repo, group)
      if (found === undefined || found.entries.length === 0) return
      const paths = found.entries.map((e) => e.path)
      void (async () => {
        setBusy('Shelving…')
        await guarded('git shelve', () =>
          gitApi.shelf.shelve(project, repo, found.name, commitSelections(repo, paths)),
        )
        setBusy(null)
        await refresh()
        await loadShelf()
      })()
    },
    [project, view, guarded, refresh, loadShelf],
  )

  /**
   * Put a shelf entry back. `keep` is IDEA's *Unshelve and keep* — useful for applying the
   * same change to two branches, and the reason `git_unshelve` takes the flag at all.
   */
  const unshelveWith = useCallback(
    (row: ShelfRow, keep: boolean) => {
      if (project === null) return
      void (async () => {
        setBusy(keep ? 'Unshelving (keeping)…' : 'Unshelving…')
        await guarded('git unshelve', () =>
          gitApi.shelf.unshelve(project, row.repo, row.entry.id, keep),
        )
        setBusy(null)
        await refresh()
        await loadShelf()
      })()
    },
    [project, guarded, refresh, loadShelf],
  )

  const unshelve = useCallback((row: ShelfRow) => unshelveWith(row, false), [unshelveWith])
  const unshelveKeep = useCallback((row: ShelfRow) => unshelveWith(row, true), [unshelveWith])

  /**
   * Delete a shelf entry without applying it.
   *
   * Confirmed, and by the same dialog as a revert: the patch file is the only copy of that
   * work — `shelve` rolled the working tree back after writing it — so dropping it is exactly
   * as final as reverting, and the entry's own file list is what is at stake.
   */
  const dropShelf = useCallback(
    (row: ShelfRow) => {
      if (project === null) return
      setConfirm({
        title: `Delete the shelf entry “${row.entry.name}”?`,
        body:
          'The patch is the only copy of this work — shelving took it out of the working '
          + 'tree. Deleting it cannot be undone.',
        files: row.entry.files,
        confirmLabel: 'Delete shelf entry',
        run: () => {
          void (async () => {
            setBusy('Deleting shelf entry…')
            await guarded('git shelf drop', () =>
              gitApi.shelf.drop(project, row.repo, row.entry.id),
            )
            setBusy(null)
            await loadShelf()
          })()
        },
      })
    },
    [project, guarded, loadShelf],
  )

  /**
   * IDEA's ◉. Applied to every repository in the workspace, because it is one button.
   *
   * The setting lives in each repo's changelists sidecar, so this is a command rather than a
   * piece of local state — which is also why `stagingArea` above is read back out of the next
   * payload instead of being set optimistically here.
   */
  const setStagingArea = useCallback(
    (on: boolean) => {
      if (project === null) return
      void (async () => {
        for (const repo of allRepos(view)) {
          await guarded('git staging mode', () =>
            gitApi.setUseStagingArea(project, repo.id, on),
          )
        }
        await refresh()
      })()
    },
    [project, view, guarded, refresh],
  )

  const reloadIndex = useCallback(
    (repo: RepoId) => {
      overwritten.current.delete(repo)
      setAnswered((prev) => new Set(prev).add(repo))
      // "Reload" means git's view wins: forget this repo's ticks and let the next payload
      // re-apply its defaults, exactly as if the panel had just opened on it.
      seenFiles.current = new Set([...seenFiles.current].filter((id) => !inRepo(id, repo)))
      setSelected((prev) => new Set([...prev].filter((id) => !inRepo(id, repo))))
      void (async () => {
        if (project === null) {
          await refresh()
          return
        }
        // `git_adopt_index` is what actually clears the divergence: it records the index as
        // it now stands, so the next status stops reporting `indexChangedExternally`. A local
        // refresh alone — what this used to do — left the bar up forever.
        const tree = await guarded('git reload index', () => gitApi.adoptIndex(project, repo))
        if (tree === undefined) await refresh()
        else adopt(absorb(tree))
      })()
    },
    [project, guarded, refresh, adopt, absorb],
  )

  const overwriteIndex = useCallback((repo: RepoId) => {
    overwritten.current.add(repo)
    setAnswered((prev) => new Set(prev).add(repo))
  }, [])

  /**
   * Open one file's diff.
   *
   * Takes the repo and the entry rather than a `Row`, because the toolbar's ◫ acts on the
   * *selection* and the selection outlives the rows: `buildRows` emits no file rows for a
   * collapsed group, so a search of the row list found nothing whenever the user had collapsed
   * the changelist — an enabled button that did nothing, which is the class of bug this panel
   * already had too much of. `flatFiles` reads the tree, exactly as commit does.
   *
   * # Why this no longer fetches
   *
   * It used to call `git_diff_file` and hand the `FileDiff` to `onOpenDiff` — and with no
   * host wired, which was every build, the diff was fetched and dropped. Now it opens a
   * *tab*: `tab_open_diff` records the key (repo, path, side) and `GitDiffPane` fetches when
   * it mounts. That is one round trip instead of two, it survives a restart, and it is what
   * makes the diff reachable without the sidebar's host knowing anything about git.
   *
   * `onOpenDiff` is kept for a host that wants the payload itself — the layout audit, or a
   * future preview strip. When it is set it takes precedence and no tab is opened, so a host
   * cannot end up with both.
   *
   * # `mode`, and why the sidebar does not decide which tab
   *
   * `'open'` is the double-click (and Enter, and the toolbar's ◫): open a tab and keep it.
   * `'retarget'` is a single click while a diff is already up — "change current diff to
   * selected file" — and it re-points one tab instead of adding one.
   *
   * The choice between them comes from `gitTreeClick`, which already made it: the click rules
   * decided `open` was true and *why*, and until now both reasons went to the same command.
   * They are two commands because they are two operations; picking *which* tab a retarget
   * lands on is deliberately not done here, because a tab is workspace state that a second
   * window can also be looking at. See `cmd::file::tab_retarget_diff`.
   */
  const showDiff = useCallback(
    (repo: RepoId, entry: ChangeEntry, mode: DiffOpenMode = 'open') => {
      if (project === null) return
      // `combined` is HEAD→working tree, which is what a changelist row *is*, and the side a
      // commit selects from. In staging-area mode the index is the truth, so the staged side
      // is the honest one. Either way the pane can switch, and switching clears the selection
      // because positions do not transfer between sides.
      const side: DiffSide = stagingArea ? 'staged' : 'combined'
      void (async () => {
        if (onOpenDiff !== undefined) {
          const diff = await guarded('git diff', () =>
            gitApi.diffFile(project, repo, entry.path, side),
          )
          if (diff !== undefined) onOpenDiff(diff, repo)
          return
        }
        if (!diffPaneAvailable()) {
          // Nothing in this build can draw a diff tab, so opening one would leave the user
          // with a blank pane and no explanation. Said out loud instead — the same refusal
          // the panel used to give when `onOpenDiff` was absent, for the same reason.
          note('git diff', `no diff view is wired up — ${entry.path} was not opened`)
          void diag.log('git panel: GitDiffPane is not linked; no diff tab was opened')
          return
        }
        // No `hydrate()` afterwards, and no import of the workspace store: `tab_open_diff`
        // goes through `WorkspaceState::update`, which broadcasts `cide://workspace-changed`
        // to every window, and the store's own subscription applies it. Reaching for the
        // store here would also drag the terminal stack into this module's import graph —
        // `store/workspace` → `layout/paneHosts` → xterm, which touches `self` at import
        // time and breaks `check:render`'s server-side render of this very panel.
        await guarded('git diff', () =>
          mode === 'retarget'
            ? gitDiffApi.retargetTab(project, repo, entry.path, side, entry.origPath)
            : gitDiffApi.openTab(project, repo, entry.path, side, entry.origPath),
        )
      })()
    },
    [project, guarded, note, stagingArea, onOpenDiff],
  )

  /**
   * A file row was activated. `mode` is the gesture's meaning, not the caller's preference —
   * `ChangesTree` derives it from the same `RowAction` that decided to call this at all.
   *
   * Defaulting to `'open'` so a caller that predates the split — Enter, a host wiring the
   * tree itself — keeps the behaviour it had. The conservative direction: `'open'` at worst
   * costs a tab, `'retarget'` at worst replaces one the user was reading.
   */
  const openDiff = useCallback(
    (row: Row, mode: DiffOpenMode = 'open') => {
      if (row.kind !== 'file' || row.entry === undefined) return
      showDiff(row.repo, row.entry, mode)
    },
    [showDiff],
  )

  /** The toolbar's ◫: the first ticked file, whether or not its group is rendered. */
  const showSelectedDiff = useCallback(() => {
    const first = flatFiles(view).find((f) => selected.has(f.id))
    if (first !== undefined) showDiff(first.repo, first.entry)
  }, [view, selected, showDiff])

  return {
    view,
    rows,
    shelf,
    selected,
    selection,
    carried,
    current,
    expanded,
    picked,
    loading,
    unavailable,
    busy,
    diverged,
    message,
    amend,
    /** Non-null exactly when Commit would reword — see the `useMemo` that computes it. */
    rewordRepo,
    amendOf,
    stagingArea,
    partials,
    story: story !== null,
    dialog,
    confirm,
    refresh: () => {
      void refresh()
      void loadShelf()
    },
    refreshShelf,
    toggleCheck,
    toggleCheckSelected,
    setCurrent,
    pressRow,
    releaseRow,
    keyToRow,
    selectAllRows,
    collapseSelection,
    toggleExpand,
    setAllExpanded,
    setMessage,
    setAmend,
    setStagingArea,
    commit,
    unstage,
    stageFile,
    unstageFile,
    rollbackFile,
    newChangelist,
    renameChangelist,
    deleteChangelist,
    setActiveChangelist,
    moveToChangelist,
    movePaths,
    trackPaths,
    revertGroup,
    revertFiles,
    shelveGroup,
    dismissDialog,
    pickChangelist,
    submitChangelistName,
    dismissConfirm,
    runConfirm,
    shelve,
    unshelve,
    unshelveKeep,
    dropShelf,
    reloadIndex,
    overwriteIndex,
    clearPartials: clearAllPartials,
    openDiff,
    showSelectedDiff,
  }
}
