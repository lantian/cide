/**
 * The conflicted files waiting to be answered — IDEA's *Files Merged with Conflicts*. (M20)
 *
 * # Why this opens itself
 *
 * A merge or rebase that stops is not something the user asked for and may not be looking at.
 * The commit panel's `MergeBar` is the *standing* surface — always there while an operation is
 * in flight, never interrupting anything — but it is only visible if the Git panel happens to be
 * open, and Ctrl+T can be pressed from a terminal pane with the sidebar shut. A conflict that
 * announced itself only in a toast would leave the user reading *"2 files to resolve"* with
 * nothing on screen that resolves anything.
 *
 * So this is the one merge surface that appears on its own, and it appears **once, at the moment
 * the conflict lands** — not on every refresh. `keys/dispatch.ts` and the bar's *Resolve
 * conflicts* button are the two callers.
 *
 * Deliberately **not** part of `store/workspace.ts`: that store mirrors Rust-owned durable state,
 * and the durable half of this is git's own `MERGE_HEAD`. What lives here is only *whether the
 * list is on screen*, which is per-window and must not be persisted — a dialog restored on
 * launch over a merge somebody finished in a terminal is worse than no dialog.
 */
import { create } from 'zustand'
import { branch as branchApi, type MergeState, type ProjectId } from '@/ipc/client'
import { explain } from './branchModel'
import { notify } from './notices'

export interface PendingConflicts {
  project: ProjectId
  repo: string
  /** The repository's name, for the title when a project has more than one. */
  repoName: string
  state: MergeState
}

interface ConflictsStore {
  /** Null whenever the list is not on screen, which is almost always. */
  pending: PendingConflicts | null
  open: (pending: PendingConflicts) => void
  /** Replace the list in place after a resolution, without closing it. */
  update: (state: MergeState | null) => void
  close: () => void
}

export const useConflicts = create<ConflictsStore>((set, get) => ({
  pending: null,
  open: (pending) => set({ pending }),
  update: (state) => {
    const held = get().pending
    if (held === null) return
    // The operation finished — the last file was answered and Continue ran, or somebody aborted
    // in a terminal. Closing is the honest response; leaving a list of files that are no longer
    // conflicted on screen is how a dialog becomes something users dismiss without reading.
    if (state === null) {
      set({ pending: null })
      return
    }
    set({ pending: { ...held, state } })
  },
  close: () => set({ pending: null }),
}))

/**
 * Show the list, from outside React.
 *
 * Dropped when one is already up rather than replacing it: the only way to make a second by hand
 * is another Ctrl+T, which is the same gesture repeated, and swapping the list under somebody
 * mid-read is what `outsideOpenStore`'s header refuses for the same reason.
 */
export function showConflicts(pending: PendingConflicts): void {
  if (useConflicts.getState().pending !== null) return
  useConflicts.getState().open(pending)
}

/**
 * What happens after one file is answered, or after the resolver is closed without answering.
 *
 * This is the thread that makes a multi-file merge a sequence rather than a series of unrelated
 * gestures. Resolve one file, its tab closes, and the list comes back with that row ticked and
 * the rest still to do — which is IDEA's flow and the one thing a merge of four files needs from
 * a tool.
 *
 * **When the last file is answered it commits.** The user asked for it in those words, and it is
 * what the state is for: a merge with every conflict resolved and nothing else to decide has one
 * remaining action, and making somebody find a button for it is ceremony. The commit is
 * announced, and it is an ordinary commit in the log — nothing here is irreversible in a way
 * `git reset --hard ORIG_HEAD` does not cover.
 *
 * Called from the resolver on both exits, so closing a tab without answering brings the list back
 * rather than leaving the user in a repository that is mid-merge with nothing on screen saying so.
 */
export async function afterResolve(
  project: ProjectId,
  repo: string,
  repoName: string,
): Promise<void> {
  const state = await branchApi.conflicts(project, repo).catch(() => null)
  // The operation is over — somebody finished it in a terminal, or aborted it. Nothing to show.
  if (state === null) {
    useConflicts.getState().close()
    return
  }

  const left = state.entries.filter((e) => !e.resolved)
  if (left.length > 0) {
    useConflicts.getState().close()
    showConflicts({ project, repo, repoName, state })
    return
  }

  try {
    const outcome = await branchApi.mergeContinue(project, repo)
    const next = outcome.state ?? null
    if (next === null) {
      useConflicts.getState().close()
      notify(`${state.operation === 'rebase' ? 'Rebase' : 'Merge'} committed`, {
        kind: 'info',
        hint: 'Every conflicted file was resolved, so it finished on its own.',
      })
      return
    }
    // A rebase stopped again on its next commit. That is not the end of anything — it is the
    // same situation one step along, so the list comes back for it.
    useConflicts.getState().close()
    showConflicts({ project, repo, repoName, state: next })
  } catch (error: unknown) {
    // The commit refused — an unresolved path cide did not know about, an identity git will not
    // accept. Say so and put the list back rather than leaving the user with nothing.
    notify(explain(error), { kind: 'error' })
    useConflicts.getState().close()
    showConflicts({ project, repo, repoName, state })
  }
}
