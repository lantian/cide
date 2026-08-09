/**
 * The file tree's per-path git status, and what keeps it fresh.
 *
 * Held apart from `treeStore` on purpose. The two are fetched independently so that the
 * tree's **first paint never waits on git**: `fs_tree_count` and `fs_tree_rows` land and the
 * rows draw with an empty map, then `git_tree_status` lands and the tags appear. A repository
 * whose `git status` takes two seconds shows an untagged tree and then tags — never an empty
 * pane. Folding the status into `TreeRow` would have made that impossible by construction;
 * see `crates/cide-git/src/tree_status.rs` for the full comparison.
 *
 * # Staying correct
 *
 * A commit, a `git add` in a bash pane, and a Claude edit all change status, and none of them
 * is a scroll. So this refreshes on events, never on a timer and never per window:
 *
 *   * `cide://git-status` — every mutation cide itself made.
 *   * `cide://fs-changed` — everything else. The watcher explicitly watches `HEAD`, `index`
 *     and the refs (see `cide_fs::filter::GIT_WATCHED`) and flags such a burst with `git`,
 *     which is how an external `git add` or `git commit` arrives. A plain file edit matters
 *     too — writing a file is what turns it from clean to modified — so both kinds refresh.
 *
 * `Explorer` owns the subscriptions; this owns the coalescing.
 */
import { create } from 'zustand'
import {
  git as gitApi,
  isDegraded,
  pendingCommand,
  type ProjectId,
  type TreeStatusMap,
} from '@/ipc/client'
import { NO_STATUS } from './treeStatus'

/**
 * How long a burst of triggers is held before one refresh runs.
 *
 * The watcher already debounces the filesystem at 300 ms, but a single user gesture still
 * produces several triggers here: `git_commit` broadcasts `cide://git-status` and rewrites
 * `.git/index` and `HEAD`, which the watcher then reports as well. Without this, one commit
 * would be three full status walks of the repository.
 *
 * Trailing, not leading: the point is to run *after* the burst, against the state git has
 * actually settled into.
 */
const COALESCE_MS = 120

interface GitStatusStore {
  project: ProjectId | null
  /** Absolute path → status, plus the truncation flag. Never null; see `NO_STATUS`. */
  status: TreeStatusMap
  /** True once `git_tree_status` has failed, i.e. it is not registered in this build. */
  degraded: boolean

  /** Point the store at a project, or at nothing. Clears the map first. */
  attach: (project: ProjectId | null) => Promise<void>
  /** Re-read the map now. */
  refresh: () => Promise<void>
  /** Re-read the map once, after the current burst of triggers has stopped. */
  schedule: () => void
}

/**
 * Bumped on every `attach`. A response tagged with an older generation is dropped — without
 * it, a status map that was in flight when the user switched projects lands afterwards and
 * tags the new project's tree with the old project's paths. They would almost all miss, which
 * is the worst version of the bug: a tree that is subtly, silently under-tagged.
 */
let generation = 0
let timer: ReturnType<typeof setTimeout> | null = null

export const useGitStatus = create<GitStatusStore>((set, get) => ({
  project: null,
  status: NO_STATUS,
  degraded: false,

  async attach(project) {
    generation += 1
    if (timer !== null) {
      clearTimeout(timer)
      timer = null
    }
    // Cleared rather than left in place while the new map is fetched: the old project's tags
    // over the new project's rows is a wrong answer, and no tags is a true one.
    set({ project, status: NO_STATUS })
    if (project === null) return
    await get().refresh()
  },

  async refresh() {
    const { project } = get()
    if (project === null) return
    const mine = generation

    const status = await pendingCommand(
      'git_tree_status',
      () => gitApi.treeStatus(project),
      NO_STATUS,
    )
    // Two guards, because two different things can have happened while the walk ran: the
    // project was swapped (`generation`), or a second refresh was attached to a newer one.
    if (generation !== mine || get().project !== project) return
    set({ status, degraded: isDegraded('git_tree_status') })
  },

  schedule() {
    if (timer !== null) return
    timer = setTimeout(() => {
      timer = null
      void get().refresh()
    }, COALESCE_MS)
  },
}))
