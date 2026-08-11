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
  commitUnits,
  defaultExpanded,
  defaultSelection,
  flatFiles,
  inRepo,
  isIgnoredGroupRow,
  normalizeStatus,
  pruneSelection,
  repoOf,
  selectedFiles,
  toggleRow,
  viewOf,
  type Row,
} from './model'
import { storyFromQuery, type GitStory } from './fixture'
import type { ChangeEntry, DiffOpenMode, ShelfRow, StatusView } from './types'

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

/** Coalescing window for refresh bursts. One edit reports several paths. */
const REFRESH_DEBOUNCE_MS = 60

export interface GitPanelModel {
  view: StatusView
  rows: Row[]
  shelf: readonly ShelfRow[]
  selected: ReadonlySet<string>
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
}

export interface GitPanelActions {
  refresh: () => void
  /** Re-read every repo's shelf. Called when the Shelf tab opens, and after it changes. */
  refreshShelf: () => void
  toggleCheck: (row: Row) => void
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
  rollbackFile: (repo: RepoId, path: string) => void
  shelve: () => void
  unshelve: (row: ShelfRow) => void
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
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => defaultExpanded(initial))
  const [loading, setLoading] = useState(false)
  const [unavailable, setUnavailable] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [message, setMessage] = useState('')
  const [amend, setAmendFlag] = useState(false)
  /** Repos where the user answered the guard bar. Cleared when the divergence clears. */
  const [answered, setAnswered] = useState<ReadonlySet<RepoId>>(new Set())
  /** Repos where the answer was "overwrite": the next commit is sent with `force: true`. */
  const overwritten = useRef<Set<RepoId>>(new Set())
  /** What the previous payload contained, so genuinely new rows can be treated as new. */
  const seenFiles = useRef<Set<string>>(new Set(allFiles(initial)))
  const seenGroups = useRef<Set<string>>(allGroups(initial))

  const rows = useMemo(() => buildRows(view, expanded), [view, expanded])
  const picked = useMemo(() => selectedFiles(view, selected), [view, selected])

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
        const detail = e instanceof Error ? e.message : String(e)
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
    seenFiles.current = new Set(live)
    seenGroups.current = allGroups(next)
    setSelected((prev) => {
      const kept = pruneSelection(live, prev)
      for (const id of fresh.files) kept.add(id)
      return kept
    })
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
   * `viewOf`, plus the one fact in a tree that outlives the panel.
   *
   * Every `ChangesTree` names the absolute work tree of every repo in it, and the diff pane
   * needs exactly that to tell a tool call that touched *its* file from one that touched a
   * file at the same relative path in another project. The panel is where trees arrive first
   * — usually well before any diff tab exists, since a tab is opened by double-clicking a row
   * here — so noting it in passing costs nothing and saves the pane a `git_status` of its own.
   * See `repoRoots.ts`.
   *
   * Not folded into `viewOf`: `model.ts` is compiled on its own by `check-git-tree.mjs`, and a
   * side effect in a pure mapping is not the sort of thing that stays in one place.
   */
  const absorb = useCallback((tree: ChangesTree): StatusView => {
    noteRepoRoots(tree)
    return viewOf(tree)
  }, [])

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
   * Ticking `Amend` does not prefill the message.
   *
   * It used to, from a `headMessage` field the panel invented; `RepoChanges` carries no such
   * thing and inventing one here would mean a second round trip per repo on every refresh for
   * a string that is only read when a checkbox is ticked. The box is left alone rather than
   * filled with a guess — `git commit --amend` keeps the old message when none is given, so
   * an empty box amends without rewriting the subject.
   */
  const setAmend = useCallback((on: boolean) => setAmendFlag(on), [])

  /** The ticks, split by repo, with the changelist named when they all came from one. */
  const units = useMemo(() => commitUnits(view, selected), [view, selected])

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
        }
        await refresh()
      })()
    },
    [project, units, view, diverged, message, amend, guarded, note, refresh],
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
  /** **Destroys uncommitted work.** The menu marks it `danger` and names the file. */
  const rollbackFile = useMemo(
    () => fileVerb('git rollback', 'Rolling back…', gitApi.rollback),
    [fileVerb],
  )

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

  const unshelve = useCallback(
    (row: ShelfRow) => {
      if (project === null) return
      void (async () => {
        setBusy('Unshelving…')
        await guarded('git unshelve', () => gitApi.shelf.unshelve(project, row.repo, row.entry.id))
        setBusy(null)
        await refresh()
        await loadShelf()
      })()
    },
    [project, guarded, refresh, loadShelf],
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
    expanded,
    picked,
    loading,
    unavailable,
    busy,
    diverged,
    message,
    amend,
    stagingArea,
    partials,
    story: story !== null,
    refresh: () => {
      void refresh()
      void loadShelf()
    },
    refreshShelf,
    toggleCheck,
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
    shelve,
    unshelve,
    reloadIndex,
    overwriteIndex,
    clearPartials: clearAllPartials,
    openDiff,
    showSelectedDiff,
  }
}
