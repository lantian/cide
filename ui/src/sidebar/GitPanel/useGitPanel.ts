/**
 * The git panel's state and its only route to Rust.
 *
 * # The guard
 *
 * Every call goes through `guarded`. None of the eight `git_*` handlers exists yet, so on
 * this branch *all* of them reject with Tauri's "command not found" — and the panel still
 * has to paint, because it is the thing under review. The rule that follows is worth
 * keeping after the backend lands: a git command can fail for reasons that are nobody's
 * bug (a repo mid-rebase, an index lock held by a `git` in a bash pane, a submodule nobody
 * initialised), and none of those may take the window down. React 19 unmounts the whole
 * tree on an unhandled throw out of a render or an effect, so a bare `await` in here is a
 * blank window rather than a broken panel.
 *
 * Failures land in `unavailable` as one dim line under the toolbar, plus a full line on
 * the app's stderr. They are never thrown, never retried in a loop, never silent.
 *
 * # Freshness
 *
 * M10's acceptance test is "Claude edits a file → the panel updates within 200 ms with no
 * manual refresh". `cide://session-tool` names the touched paths and arrives ahead of any
 * filesystem watcher, so it is the trigger, and the debounce below is well inside that
 * budget. It is not sufficient on its own — a `sed -i` in a bash pane goes through no tool
 * — so when the Rust side emits a watcher event it belongs on this same coalescer, not on
 * a polling timer somewhere else.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { diag, events, git as gitApi, type ProjectId } from '@/ipc/client'
import {
  allFiles,
  allGroups,
  buildRows,
  commitUnits,
  defaultExpanded,
  defaultSelection,
  inRepo,
  normalizeStatus,
  pruneSelection,
  selectedFiles,
  toggleRow,
  type Row,
} from './model'
import { storyFromQuery, type GitStory } from './fixture'
import type { ChangeFile, ChangesTree, ShelfEntry } from './types'

const EMPTY: ChangesTree = { repos: [] }

/** Coalescing window for refresh bursts. One edit reports several paths. */
const REFRESH_DEBOUNCE_MS = 60

export interface GitPanelModel {
  tree: ChangesTree
  rows: Row[]
  shelf: readonly ShelfEntry[]
  selected: ReadonlySet<string>
  expanded: ReadonlySet<string>
  /** The `ChangeFile`s behind the ticks, in row order — what the footer counts. */
  picked: ChangeFile[]
  loading: boolean
  /** Set when a git call failed. One dim line, not a dialog. */
  unavailable: string | null
  /** A command is in flight; the commit buttons are disabled while it is. */
  busy: string | null
  /** Repos whose index moved under us and whose bar has not been answered yet. */
  diverged: string[]
  message: string
  amend: boolean
  /** IDEA's "use Git staging area instead" mode — the toolbar's ◉ toggle. */
  stagingArea: boolean
  /** True when the panel is showing a fixture rather than a repository. */
  story: boolean
}

export interface GitPanelActions {
  refresh: () => void
  toggleCheck: (row: Row) => void
  toggleExpand: (row: Row) => void
  setAllExpanded: (open: boolean) => void
  setMessage: (text: string) => void
  setAmend: (on: boolean) => void
  setStagingArea: (on: boolean) => void
  commit: (push: boolean) => void
  /** Take the ticked paths out of the index. NOT a rollback — see `Toolbar`. */
  unstage: () => void
  shelve: () => void
  /** Guard bar, left button: drop our ticks and take git's view of this repo. */
  reloadIndex: (repo: string) => void
  /** Guard bar, right button: keep our ticks and let the commit rewrite the index. */
  overwriteIndex: (repo: string) => void
  openDiff: (row: Row) => void
}

export function useGitPanel(project: ProjectId | null): GitPanelModel & GitPanelActions {
  // Read once. A story is a property of how the window was opened; re-reading the URL each
  // render would let a navigation swap the panel's data source mid-session.
  const [story] = useState<GitStory | null>(() => storyFromQuery())
  const initial = story?.status ?? EMPTY

  const [tree, setTree] = useState<ChangesTree>(initial)
  const [shelf] = useState<readonly ShelfEntry[]>(story?.shelf ?? [])
  const [selected, setSelected] = useState<ReadonlySet<string>>(() => defaultSelection(initial))
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => defaultExpanded(initial))
  const [loading, setLoading] = useState(false)
  const [unavailable, setUnavailable] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [message, setMessage] = useState('')
  const [amend, setAmendFlag] = useState(false)
  const [stagingArea, setStagingArea] = useState(false)
  /** Repos where the user answered the guard bar. Cleared when the divergence clears. */
  const [answered, setAnswered] = useState<ReadonlySet<string>>(new Set())
  /** Repos where the answer was "overwrite": commit without an index expectation. */
  const overwritten = useRef<Set<string>>(new Set())
  /** What the previous payload contained, so genuinely new rows can be treated as new. */
  const seenFiles = useRef<Set<string>>(new Set(allFiles(initial)))
  const seenGroups = useRef<Set<string>>(allGroups(initial))

  const rows = useMemo(() => buildRows(tree, expanded), [tree, expanded])
  const picked = useMemo(() => selectedFiles(tree, selected), [tree, selected])

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
   */
  const adopt = useCallback((next: ChangesTree) => {
    const live = allFiles(next)
    const defaults = defaultSelection(next)
    setSelected((prev) => {
      const kept = pruneSelection(live, prev)
      for (const id of live) {
        if (!seenFiles.current.has(id) && defaults.has(id)) kept.add(id)
      }
      return kept
    })
    setExpanded((prev) => {
      const merged = new Set(prev)
      for (const id of defaultExpanded(next)) {
        if (!seenGroups.current.has(id)) merged.add(id)
      }
      return merged
    })
    seenFiles.current = new Set(live)
    seenGroups.current = allGroups(next)
    setTree(next)
    // A repo that stopped diverging drops its answer, so the next real divergence raises
    // the bar again instead of being suppressed by an answer to an older one.
    const stillDiverged = new Set(
      next.repos.flatMap((r) => (r.indexDiverged === true ? [r.root] : [])),
    )
    // …and it drops its *waiver* with it. `overwritten` used to be cleared only by a
    // successful commit, so "Overwrite" answered at 10:00 and never committed was still
    // armed at 14:00 — by which time the bar had come and gone. The next real `git add` in
    // a bash pane would then raise a fresh bar the user had not answered, and a commit sent
    // `expectIndex: null` anyway and clobbered it. That is the exact silent clobber this
    // guard exists to prevent, so the waiver dies with the divergence it answered.
    for (const root of overwritten.current) {
      if (!stillDiverged.has(root)) overwritten.current.delete(root)
    }
    setAnswered((prev) => {
      const still = new Set([...prev].filter((root) => stillDiverged.has(root)))
      return still.size === prev.size ? prev : still
    })
  }, [])

  const refresh = useCallback(async () => {
    if (story) return
    if (project === null) {
      adopt(EMPTY)
      return
    }
    setLoading(true)
    const raw = await guarded('git status', () => gitApi.status(project))
    setLoading(false)
    // `undefined` is the guard's failure signal; `{ repos: [] }` is a legitimate answer.
    // Keeping the previous tree after a failure would show changes that may no longer
    // exist, so a failure empties the panel.
    adopt(raw === undefined ? EMPTY : normalizeStatus(raw))
  }, [project, story, guarded, adopt])

  // One timer, shared by the mount refresh and by every tool event.
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

  useEffect(() => {
    if (story) return
    // `gone` rather than just holding the unlisten in a variable: `listen` resolves a tick
    // or two after the effect runs, so a panel unmounted in between (the sidebar switching
    // back to Files) would never see the handle and would leave a listener firing
    // `git_status` on every tool event for the rest of the session, one more per remount.
    let gone = false
    let unlisten: (() => void) | null = null
    void events
      .onSessionTool(() => schedule())
      .then((fn) => {
        if (gone) fn()
        else unlisten = fn
      })
      .catch((e) => diag.log(`git panel: tool events unavailable: ${String(e)}`))
    return () => {
      gone = true
      unlisten?.()
    }
  }, [schedule, story])

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
    (open: boolean) => setExpanded(open ? allGroups(tree) : new Set<string>()),
    [tree],
  )

  /**
   * Ticking `Amend` with an empty box prefills HEAD's message.
   *
   * Only when empty: replacing something already typed is a way to lose a message that
   * cannot be got back.
   */
  const setAmend = useCallback(
    (on: boolean) => {
      setAmendFlag(on)
      if (!on) return
      const head = tree.repos.find((r) => r.headMessage !== undefined)?.headMessage
      if (head !== undefined) setMessage((m) => (m.trim() === '' ? head : m))
    },
    [tree],
  )

  /** The ticks, split by repo, with the changelist named when they all came from one. */
  const units = useMemo(() => commitUnits(tree, selected), [tree, selected])

  /** Repos whose index moved under us and whose bar has not been answered yet. */
  const diverged = useMemo(
    () =>
      tree.repos.flatMap((r) => (r.indexDiverged === true && !answered.has(r.root) ? [r.root] : [])),
    [tree, answered],
  )

  const commit = useCallback(
    (push: boolean) => {
      if (project === null || units.length === 0) return
      void (async () => {
        setBusy(push ? 'Committing and pushing…' : 'Committing…')
        let allOk = true
        for (const unit of units) {
          const repo = tree.repos.find((r) => r.root === unit.repo)
          /*
           * An unanswered guard bar stops the commit here rather than at the backend.
           *
           * `expectIndex: null` means "waive the check and overwrite", and it is *also*
           * what an absent `indexToken` produces — so a backend that forgets to populate
           * the token would turn the guard off for every commit rather than for the one
           * the user waived. Refusing locally makes the outcome the same whether or not
           * the token is there: while the bar is up and unanswered, no commit is sent.
           */
          if (diverged.includes(unit.repo)) {
            const label = repo?.label ?? unit.repo
            note(
              'git commit',
              `git commit refused — ${label}'s index changed outside cide; `
                + 'answer Reload or Overwrite first',
            )
            allOk = false
            break
          }
          // `null` waives the index check. Only reached when the user chose "Overwrite" on
          // the bar, or when this repo never diverged in the first place.
          const expect = overwritten.current.has(unit.repo) ? null : (repo?.indexToken ?? null)
          const oid = await guarded('git commit', () =>
            gitApi.commit(project, unit.repo, message, amend, unit.changelist, unit.paths, expect),
          )
          if (oid === undefined) {
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
    [project, units, tree, diverged, message, amend, guarded, note, refresh],
  )

  const unstage = useCallback(() => {
    if (project === null || units.length === 0) return
    void (async () => {
      setBusy('Unstaging…')
      for (const unit of units) {
        await guarded('git unstage', () => gitApi.stagePaths(project, unit.repo, unit.paths, false))
      }
      setBusy(null)
      await refresh()
    })()
  }, [project, units, guarded, refresh])

  const shelve = useCallback(() => {
    if (project === null || units.length === 0) return
    void (async () => {
      setBusy('Shelving…')
      const name = message.trim() === '' ? 'Shelved changes' : message.trim()
      for (const unit of units) {
        await guarded('git shelve', () => gitApi.shelve(project, unit.repo, unit.paths, name))
      }
      setBusy(null)
      await refresh()
    })()
  }, [project, units, message, guarded, refresh])

  const reloadIndex = useCallback(
    (repo: string) => {
      overwritten.current.delete(repo)
      setAnswered((prev) => new Set(prev).add(repo))
      // "Reload" means git's view wins: forget this repo's ticks and let the next payload
      // re-apply its defaults, exactly as if the panel had just opened on it.
      seenFiles.current = new Set([...seenFiles.current].filter((id) => !inRepo(id, repo)))
      setSelected((prev) => new Set([...prev].filter((id) => !inRepo(id, repo))))
      void refresh()
    },
    [refresh],
  )

  const overwriteIndex = useCallback((repo: string) => {
    overwritten.current.add(repo)
    setAnswered((prev) => new Set(prev).add(repo))
  }, [])

  const openDiff = useCallback(
    (row: Row) => {
      const file = row.file
      if (project === null || row.kind !== 'file' || file === undefined) return
      void guarded('git diff', () =>
        gitApi.diffFile(project, row.repo, file.path, stagingArea ? 'staged' : 'worktree'),
      )
    },
    [project, guarded, stagingArea],
  )

  return {
    tree,
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
    story: story !== null,
    refresh: () => void refresh(),
    toggleCheck,
    toggleExpand,
    setAllExpanded,
    setMessage,
    setAmend,
    setStagingArea,
    commit,
    unstage,
    shelve,
    reloadIndex,
    overwriteIndex,
    openDiff,
  }
}
