/**
 * The agent checkouts under `.cide/worktrees/` this project has, and the **one** of them the
 * Explorer, the Git panel and the Log are pointed at. (M207)
 *
 * # One choice, three surfaces
 *
 * Until M207 this said "One list, two choices": the Git panel and the Log chose independently,
 * keyed by surface, on the argument that a user reviewing an agent's history in the Log while
 * committing their own work in the panel was the ordinary case. The Log's own picker was later
 * removed as redundant, and then the user's call (2026-11-26) was the opposite rule: choosing a
 * checkout in the Explorer — the file tree — or in the Git panel moves both, and the Log's
 * *Current branch* follows it. Looking at an agent's checkout is one act, and three surfaces
 * that each had to be pointed at it separately made it three.
 *
 * So the choice is keyed by project only, and it lives in a zustand store rather than in each
 * hook's state: the Explorer and the bottom Log are mounted at the same time, and a choice made
 * in one has to re-scope the other in the same frame.
 *
 * Module scope, per window, never on disk — the `lastTicks` bargain in `useGitPanel`: the
 * sidebar panels unmount on every rail switch and the Log on every tab switch, and a choice that
 * reset each time would be one nobody could rely on; surviving a restart would reopen a tree on a
 * checkout that `Integrate` may long since have removed.
 *
 * # The list is shared too
 *
 * Not only the choice. A chosen checkout that is no longer listed drops every surface back to the
 * project (`keptChoice`), and that judgement is only sound against one list: with a list per
 * consumer, the Log's — loaded a minute ago — would not yet hold the checkout the user just
 * picked in the Explorer, and would drop the fresh choice for everyone.
 *
 * # When the list is re-read
 *
 * While at least one surface holds the project (a refcount; the last to unmount releases the
 * listeners): on the first hold, on every `cide://git-status`, and on every untagged
 * `cide://fs-changed`. A new checkout creates its `cide/<name>` branch, which is a ref write the
 * project's watcher reports; one being removed is noticed the same way. A *tagged* burst is a
 * checkout's own watcher and cannot add or remove one (`projectHears`).
 */
import { useCallback, useEffect } from 'react'
import { create } from 'zustand'
import { events, git as gitApi, type ProjectId, type WorktreeInfo } from '@/ipc/client'
import { shareEqual } from '@/store/shareEqual'
import { keptChoice, projectHears } from './worktreeScope'

/** Re-reading the list is a `worktree list` per root; a save burst must not ask twenty times. */
const RELOAD_COALESCE_MS = 400

const NONE: readonly WorktreeInfo[] = []

interface Entry {
  worktrees: readonly WorktreeInfo[]
  /** The chosen checkout's path — `WorktreeInfo.repo.root` — or `null` for the project's own. */
  chosen: string | null
  projectBranch: string | null
}

const EMPTY: Entry = { worktrees: NONE, chosen: null, projectBranch: null }

interface WorktreeStore {
  byProject: Readonly<Record<string, Entry>>
  choose: (project: ProjectId, worktree: string | null) => void
  listed: (project: ProjectId, list: readonly WorktreeInfo[] | null, projectBranch: string | null) => void
}

export const useWorktreeStore = create<WorktreeStore>((set, get) => ({
  byProject: {},
  choose(project, worktree) {
    const entry = get().byProject[project] ?? EMPTY
    if (entry.chosen === worktree) return
    set({ byProject: { ...get().byProject, [project]: { ...entry, chosen: worktree } } })
  },
  listed(project, list, projectBranch) {
    const entry = get().byProject[project] ?? EMPTY
    // A fresh array off the wire every time, and this reloads on every watcher burst. Sharing it
    // is what stops an unchanged list re-rendering every surface that holds this hook.
    const worktrees = shareEqual(entry.worktrees, list ?? entry.worktrees)
    const chosen = keptChoice(entry.chosen, list)
    if (worktrees === entry.worktrees && chosen === entry.chosen && projectBranch === entry.projectBranch) return
    set({ byProject: { ...get().byProject, [project]: { worktrees, chosen, projectBranch } } })
  },
}))

// --- the refcounted loader ------------------------------------------------------------------

interface Holder {
  count: number
  release: () => void
}
const holders = new Map<ProjectId, Holder>()
const generations = new Map<ProjectId, number>()

async function reload(project: ProjectId): Promise<void> {
  const mine = (generations.get(project) ?? 0) + 1
  generations.set(project, mine)
  const list = await gitApi.worktrees(project).catch(() => null)
  /*
   * The project's branch only matters for a single-repository project — with several the first
   * row cannot name one branch and says "Project" instead, the same rule the header's readout
   * had. `repos` is the cheap call; `branchInfo` is one `HEAD` read.
   */
  const repos = await gitApi.repos(project).catch(() => null)
  const only = repos !== null && repos.length === 1 ? repos[0] : undefined
  const head =
    only === undefined
      ? null
      : await gitApi
          .branchInfo(project, only.id)
          .then((info) => info.head)
          .catch(() => null)
  if (generations.get(project) !== mine) return
  useWorktreeStore.getState().listed(project, list, head)
}

function hold(project: ProjectId): () => void {
  const held = holders.get(project)
  if (held !== undefined) {
    held.count += 1
    return () => drop(project)
  }
  void reload(project)
  let pending: number | null = null
  const schedule = () => {
    if (pending !== null) return
    pending = window.setTimeout(() => {
      pending = null
      void reload(project)
    }, RELOAD_COALESCE_MS)
  }
  // `gone` for the reason `useGitPanel` spells out: `listen` resolves a tick after the hold, and
  // a handle that arrives after release must be released rather than kept firing.
  let gone = false
  const unlisten: Array<() => void> = []
  const track = (p: Promise<() => void>) => {
    void p
      .then((fn) => {
        if (gone) fn()
        else unlisten.push(fn)
      })
      .catch(() => undefined)
  }
  track(events.onGitStatus((forProject) => forProject === project && schedule()))
  track(
    events.onFsChanged(
      (forProject, _change, tag) => forProject === project && projectHears(tag) && schedule(),
    ),
  )
  holders.set(project, {
    count: 1,
    release: () => {
      gone = true
      if (pending !== null) window.clearTimeout(pending)
      for (const fn of unlisten) fn()
    },
  })
  return () => drop(project)
}

function drop(project: ProjectId): void {
  const held = holders.get(project)
  if (held === undefined) return
  held.count -= 1
  if (held.count > 0) return
  holders.delete(project)
  held.release()
}

// --- the hook -------------------------------------------------------------------------------

export interface Worktrees {
  /** The agent checkouts, empty for a project with none (or no repository). */
  worktrees: readonly WorktreeInfo[]
  /** The chosen checkout's path — `WorktreeInfo.repo.root` — or `null` for the project's own. */
  worktree: string | null
  /** The chosen checkout's entry, or `null`. */
  current: WorktreeInfo | null
  /** The branch the project has checked out, for the selector's first row. */
  projectBranch: string | null
  choose: (worktree: string | null) => void
}

export function useWorktrees(project: ProjectId | null): Worktrees {
  const entry = useWorktreeStore((s) => (project === null ? EMPTY : (s.byProject[project] ?? EMPTY)))
  useEffect(() => (project === null ? undefined : hold(project)), [project])
  const choose = useCallback(
    (next: string | null) => {
      if (project !== null) useWorktreeStore.getState().choose(project, next)
    },
    [project],
  )
  const current =
    entry.chosen === null ? null : (entry.worktrees.find((w) => w.repo.root === entry.chosen) ?? null)
  return {
    worktrees: entry.worktrees,
    worktree: entry.chosen,
    current,
    projectBranch: entry.projectBranch,
    choose,
  }
}
