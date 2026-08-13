/**
 * How many files are uncommitted in this project, kept fresh whatever the sidebar is showing.
 *
 * # Why this store had to exist
 *
 * The rail's git badge was wired to `auditMode()` — the `?audit=1` layout-audit flag — and to
 * nothing else. `App.tsx` passed `gitDirty={auditMode()}`, with a comment explaining that the
 * audit "has to ask for" a dirty tree because the badge only renders on one. In a normal
 * launch it therefore **never rendered, in any repository state**: another instance of this
 * project's recurring defect, a surface built correctly and reachable from nothing.
 *
 * Adding a number to it needs a source, and none of the three that existed could serve:
 *
 *   * `useGitPanel`'s tree has the right data in React state that **does not exist** while the
 *     sidebar is on Files or shut. `keys/dispatch.ts` already documents that constraint for
 *     `git.commit` and `git.refresh`.
 *   * `sidebar/gitStatusStore` is global but its subscriptions live inside `Explorer`, so it
 *     goes stale the moment the sidebar leaves Files — and its `TreeStatusMap` is not
 *     countable anyway: `cide_git::tree_status` puts a rollup mark on every *ancestor
 *     directory* of every change and does not recurse untracked directories, so its key count
 *     is the changeset's directory depth plus one entry for a whole `target/`.
 *   * `chrome/BranchSelector`'s `useBranches` is the wrong data — and the right shape. It is
 *     an always-live module store subscribed from chrome that is always mounted, which is
 *     exactly what this is, so this is modelled on it deliberately.
 *
 * # Freshness, and the one round trip that is not made
 *
 * Three triggers, each of which the other two miss (the argument is `gitStatusStore`'s and is
 * spelled out in full there):
 *
 *   * `cide://git-status` — every mutation cide made, in any window. **It carries the whole
 *     recomputed tree**, so it is adopted rather than answered with a walk of our own.
 *   * `cide://fs-changed` — the only signal for a `git add` or a `git commit` run in a bash
 *     pane, and for a plain edit. Needs a walk.
 *   * `cide://session-tool` — arrives ahead of the watcher when Claude edits, which is what
 *     keeps the badge inside the panel's own 200 ms acceptance window.
 *
 * And when the panel is open, no walk happens here at all: `useGitPanel.absorb` hands its tree
 * to {@link noteChangeCount} in passing, the same way it already hands roots to
 * `noteRepoRoots`. Two `git_status` walks of a large repository per keystroke-burst, to
 * produce one number that was already in the first one, is not a cost worth paying.
 */
import { useEffect } from 'react'
import { create } from 'zustand'
import {
  events,
  git as gitApi,
  isDegraded,
  pendingCommand,
  type ChangesTree,
  type ProjectId,
} from '@/ipc/client'
import { countChangedFiles, normalizeStatus } from '@/sidebar/GitPanel/model'

/**
 * How long a burst of triggers is held before one walk runs.
 *
 * The same 120 ms and the same throttle-not-debounce as `gitStatusStore`, for the same
 * reason: a debounce that restarts on every trigger can be starved indefinitely by a steady
 * stream of writes — a `cargo watch` loop, a formatter on save in a big tree — and a badge
 * that freezes for as long as anything is happening is worse than one that updates 120 ms
 * after the first change of each burst.
 */
const COALESCE_MS = 120

const EMPTY: ChangesTree = { repos: [] }

interface GitCountStore {
  project: ProjectId | null
  /**
   * Changed files across every repository, or `null` for "not walked yet".
   *
   * `null` is not `0` and the difference is the whole point: `0` says the tree is clean, and
   * saying that before anything has been looked at makes the badge flash on every launch.
   * `model.badgeText` renders both as nothing, for two different reasons.
   */
  count: number | null
  /** True once `git_status` has failed, i.e. it is not registered in this build. */
  degraded: boolean

  /** Point the store at a project, or at nothing. */
  attach: (project: ProjectId | null) => Promise<void>
  /** Re-read the count now. */
  refresh: () => Promise<void>
  /** Re-read at most once per `COALESCE_MS`, on a trigger. */
  schedule: () => void
}

/**
 * Bumped on every `attach` **and every `refresh`**, so a response tagged with an older
 * generation is dropped. Two races, one counter — the reasoning is `gitStatusStore`'s, and it
 * applies here unchanged: a walk in flight when the user switches projects would install the
 * old project's number over the new project's badge, and two overlapping walks of the *same*
 * project can land out of order and leave a count computed before the edits the user just
 * made, with nothing to correct it until the next event.
 */
let generation = 0
let timer: ReturnType<typeof setTimeout> | null = null

export const useGitCount = create<GitCountStore>((set, get) => ({
  project: null,
  count: null,
  degraded: false,

  async attach(project) {
    if (project === get().project) return
    generation += 1
    if (timer !== null) {
      clearTimeout(timer)
      timer = null
    }
    // Back to `null`, not to `0`. The previous project's number over the new project's badge
    // is a wrong answer; "not known yet" is a true one, and it draws nothing either way.
    set({ project, count: null })
    if (project === null) return
    await get().refresh()
  },

  async refresh() {
    const { project } = get()
    if (project === null) return
    generation += 1
    const mine = generation

    // `includeIgnored: false`, always. The count excludes ignored files by rule
    // (`model.counts`), so asking for them would be paying for the expensive half of a status
    // walk — every path under a big `target/` — to throw the answer away.
    const tree = await pendingCommand('git_status', () => gitApi.status(project, false), EMPTY)
    if (generation !== mine || get().project !== project) return
    set({ count: countChangedFiles(normalizeStatus(tree)), degraded: isDegraded('git_status') })
  },

  schedule() {
    if (timer !== null) return
    timer = setTimeout(() => {
      timer = null
      void get().refresh()
    }, COALESCE_MS)
  },
}))

/**
 * Adopt a tree somebody else already fetched.
 *
 * Called by `useGitPanel.absorb` on every payload it sees — its own `git_status`, the
 * `cide://git-status` broadcast, and every mutation's own reply — so an open panel keeps the
 * badge exact at the cost of one fold over an array. The alternative was a second `git_status`
 * per refresh while the panel is open, walking a large work tree twice to compute a number
 * that was already in the first walk.
 *
 * The generation is bumped so that a walk this store started, and which is still in flight,
 * cannot land afterwards and overwrite a fresher number with an older one.
 *
 * A tree for a project this store is not pointed at is dropped rather than adopted: a
 * detached-pane window's panel and the shell can be looking at different projects.
 */
export function noteChangeCount(project: ProjectId, tree: ChangesTree): void {
  if (useGitCount.getState().project !== project) return
  generation += 1
  useGitCount.setState({ count: countChangedFiles(normalizeStatus(tree)) })
}

/**
 * Keep the store pointed at this window's project and fresh against git, and hand back the
 * number.
 *
 * Called from `App.tsx` rather than from `ActivityRail`, on purpose. Every chrome component in
 * this app is a pure render target that reads nothing from a store — that is what lets the
 * layout audit drive each of them with fixed props, and `chrome/auditFixture.ts` exists
 * because of it. A rail that subscribed would be the first exception and the audit would lose
 * its handle on it.
 *
 * `null` for the project is how a window with no rail opts out: a detached-pane window returns
 * before the rail is rendered and has nothing to draw a badge on, so walking its repositories
 * would be work for nobody. Passing `null` rather than skipping the call keeps the hook order
 * unconditional, which is the rule React actually enforces.
 */
export function useGitChangeCount(project: ProjectId | null): number | null {
  const count = useGitCount((s) => s.count)

  useEffect(() => {
    void useGitCount.getState().attach(project)
  }, [project])

  useEffect(() => {
    if (project === null) return
    // Carries the whole recomputed tree, so this costs no round trip at all.
    const stop = events.onGitStatus((changed: ProjectId, tree: ChangesTree) => {
      if (changed === useGitCount.getState().project) noteChangeCount(changed, tree)
    })
    return () => void stop.then((off: () => void) => off())
  }, [project])

  useEffect(() => {
    if (project === null) return
    // The watcher's view. This is the only signal for a `git add` or a `git commit` run in a
    // bash pane, and for a plain edit — neither of which broadcasts `cide://git-status`.
    const stop = events.onFsChanged((changed: ProjectId) => {
      if (changed === useGitCount.getState().project) useGitCount.getState().schedule()
    })
    return () => void stop.then((off: () => void) => off())
  }, [project])

  useEffect(() => {
    if (project === null) return
    /*
     * Claude's tool calls, which arrive ahead of the watcher and are what make the badge move
     * while an agent is working rather than 300 ms after it stops.
     *
     * The event names a session, not a project, and this store holds one project — so it
     * refreshes on any session's tools. That is deliberate rather than sloppy: the walk is
     * coalesced and generation-guarded, the wrong-project case costs one `git_status` of a
     * tree that has not changed, and the alternative — resolving session → project here —
     * would put a second copy of the workspace mirror's job in a badge.
     */
    const stop = events.onSessionTool(() => useGitCount.getState().schedule())
    return () => void stop.then((off: () => void) => off())
  }, [project])

  return count
}
