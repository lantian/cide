/**
 * What the Find usages popup is showing, and what it is waiting for.
 *
 * # Why a store rather than props
 *
 * The gesture starts in two places React cannot hand props from: `editor/ctrlLink.ts`'s `mousedown`
 * handler, which lives inside a CodeMirror extension, and `keys/dispatch.ts`, which runs from a
 * window key listener. Both are outside the component tree entirely. That is the same predicament
 * `chrome/BranchSelector`'s popup is in, and it has the same answer — `OverlayHost` names the kind,
 * the component reads its rows from here.
 *
 * # Why the generation counter is not optional
 *
 * A references search takes up to twenty seconds, and every way out of the popup — Escape, the
 * scrim, picking a row, a second ⌥F7 — has to make the answer that is still in flight *stop being
 * relevant*. Comparing a captured generation on arrival is what does that. Dismissal alone would
 * leave a late answer free to re-open a popup over whatever the user did next, which is the
 * "modal that appears from nowhere" bug that a plain `open` flag cannot avoid.
 *
 * `cancel` is the other half and they are not interchangeable: the generation stops *us* caring,
 * `diagnostics.usagesCancel` stops the *server* working. Only doing the first leaves rust-analyzer
 * searching a whole workspace for a list nobody will ever read.
 */
import { create } from 'zustand'
import { diagnostics as diagnosticsApi, type ProjectId, type Usage } from '@/ipc/client'

interface UsagesStore {
  /** The project the search belongs to, and what a cancel is aimed at. */
  project: ProjectId | null
  /** The identifier, when the caller knew it. Heads the popup and the empty-result notice. */
  name: string | null
  /** True between the request going out and the answer landing. */
  searching: boolean
  rows: readonly Usage[]
  /** The server offered more than the cap; the popup says so rather than lying about a count. */
  truncated: boolean
  /** The server's own sentence, when it could not be asked. */
  failed: string | null
  /**
   * Bumped by every start and every dismissal. An answer whose captured value no longer matches
   * is dropped on arrival.
   */
  generation: number
}

export const useUsages = create<UsagesStore>(() => ({
  project: null,
  name: null,
  searching: false,
  rows: [],
  truncated: false,
  failed: null,
  generation: 0,
}))

/**
 * Start a search. Returns the generation to compare against when the answer lands.
 *
 * Clears the rows, deliberately. Leaving the previous search's list up while a new one runs looks
 * like an answer and is not, and the user has just told us it was the wrong question.
 */
export function beginUsages(project: ProjectId, name: string | null): number {
  const generation = useUsages.getState().generation + 1
  useUsages.setState({
    project,
    name,
    searching: true,
    rows: [],
    truncated: false,
    failed: null,
    generation,
  })
  return generation
}

/** The answer arrived. */
export function showUsages(rows: readonly Usage[], truncated: boolean): void {
  useUsages.setState({ searching: false, rows, truncated, failed: null })
}

/** Nobody could be asked, or the wait ran out. The sentence is the server's own. */
export function failUsages(reason: string): void {
  useUsages.setState({ searching: false, rows: [], truncated: false, failed: reason })
}

/**
 * Stop caring, and tell the server to stop too.
 *
 * Called on every exit from the popup, including the ones where the answer has already landed —
 * `usagesCancel` is a no-op when nothing is outstanding, and being cheap enough not to have to
 * check is the design. The generation bump is what makes a late answer inert.
 */
export function cancelUsages(): void {
  const { project, generation } = useUsages.getState()
  useUsages.setState({ searching: false, generation: generation + 1 })
  if (project !== null) {
    // Deliberately swallowed. A cancel that fails has nothing the user can do about it, and a
    // toast reading "the cancellation failed" over a popup they have already dismissed is noise
    // about a key they just pressed.
    void diagnosticsApi.usagesCancel(project).catch(() => {})
  }
}

/** Is this answer still the one being waited for? */
export function isCurrentUsages(generation: number): boolean {
  return useUsages.getState().generation === generation
}
