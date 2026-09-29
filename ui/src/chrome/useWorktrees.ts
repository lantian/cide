/**
 * The agent checkouts under `.cide/worktrees/` a git surface can be pointed at, and which one
 * each surface has chosen.
 *
 * # One list, two choices
 *
 * The Git panel and the Log each carry a `WorktreeSelect`, and they choose *independently*: a
 * user reviewing an agent's history in the Log while committing their own work in the panel is
 * the ordinary case, and a shared choice would yank the panel onto the agent's checkout the
 * moment the Log was pointed at it. So the selection is keyed by surface as well as project.
 *
 * Module scope, per window, never on disk — the `lastTicks` bargain in `useGitPanel`: the panel
 * unmounts on every rail switch and the Log on every tab switch, and a choice that reset each
 * time would be one nobody could rely on; surviving a restart would reopen a panel on a checkout
 * that `Integrate` may long since have removed.
 *
 * # When the list is re-read
 *
 * On mount, on every `cide://git-status` and on every `cide://fs-changed` for the project. A new
 * checkout creates its `cide/<name>` branch, which is a ref write the watcher reports; one being
 * removed is noticed the same way. A chosen checkout that is no longer listed drops the surface
 * back to the project's own — every command would refuse its id from then on anyway.
 */
import { useCallback, useEffect, useRef, useState } from 'react'
import { events, git as gitApi, type ProjectId, type WorktreeInfo } from '@/ipc/client'

/** Which surface is choosing. The choices are independent; see the header. */
export type WorktreeSurface = 'panel' | 'log'

const chosen = new Map<string, string | null>()
const keyOf = (surface: WorktreeSurface, project: ProjectId) => `${surface}\u0000${project}`

/** Re-reading the list is a `worktree list` per root; a save burst must not ask twenty times. */
const RELOAD_COALESCE_MS = 400

const NONE: readonly WorktreeInfo[] = []

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

export function useWorktrees(surface: WorktreeSurface, project: ProjectId | null): Worktrees {
  const [worktrees, setWorktrees] = useState<readonly WorktreeInfo[]>(NONE)
  const [projectBranch, setProjectBranch] = useState<string | null>(null)
  const [worktree, setWorktree] = useState<string | null>(() =>
    project === null ? null : (chosen.get(keyOf(surface, project)) ?? null),
  )
  // Whether the list has been answered at least once for this project. Until it has, a remembered
  // choice cannot be judged stale — an empty initial list would drop every choice on every mount.
  const [loaded, setLoaded] = useState(false)

  useEffect(() => {
    setWorktree(project === null ? null : (chosen.get(keyOf(surface, project)) ?? null))
    setWorktrees(NONE)
    setProjectBranch(null)
    setLoaded(false)
  }, [surface, project])

  const generation = useRef(0)
  const reload = useCallback(async () => {
    if (project === null) return
    generation.current += 1
    const mine = generation.current
    const list = await gitApi.worktrees(project).catch(() => null)
    /*
     * The project's branch only matters for a single-repository project — with several the
     * first row cannot name one branch and says "Project" instead, the same rule the header's
     * readout had. `repos` is the cheap call; `branchInfo` is one `HEAD` read.
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
    if (generation.current !== mine) return
    setWorktrees(list ?? NONE)
    setProjectBranch(head)
    setLoaded(list !== null)
  }, [project])

  useEffect(() => {
    if (project === null) return
    void reload()
    let pending: number | null = null
    const schedule = () => {
      if (pending !== null) return
      pending = window.setTimeout(() => {
        pending = null
        void reload()
      }, RELOAD_COALESCE_MS)
    }
    // `gone` for the reason `useGitPanel` spells out: `listen` resolves a tick after the effect,
    // and a handle that arrives after unmount must be released rather than kept firing.
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
    track(events.onFsChanged((forProject) => forProject === project && schedule()))
    return () => {
      gone = true
      if (pending !== null) window.clearTimeout(pending)
      for (const fn of unlisten) fn()
    }
  }, [project, reload])

  const choose = useCallback(
    (next: string | null) => {
      if (project !== null) chosen.set(keyOf(surface, project), next)
      setWorktree(next)
    },
    [surface, project],
  )

  const current = worktree === null ? null : (worktrees.find((w) => w.repo.root === worktree) ?? null)

  // A chosen checkout that has gone — integrated and removed, or deleted by hand — falls back to
  // the project's own rather than leaving a surface every command now refuses.
  useEffect(() => {
    if (loaded && worktree !== null && current === null) choose(null)
  }, [loaded, worktree, current, choose])

  return { worktrees, worktree, current, projectBranch, choose }
}
