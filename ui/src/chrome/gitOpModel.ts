/**
 * *Something is happening to this repository* — the rules behind the status bar's git indicator.
 * (M65)
 *
 * # Why this exists at all
 *
 * There was nothing between pressing Push and the toast. `chrome/pushStore.ts`'s `confirm` nulls
 * `pending` — so the dialog unmounts — and calls `pushRun.pushPass`, which fires one `invoke` per
 * repository and returns `void`. `git_push` is a single `spawn_blocking` round trip whose own doc
 * says it "can sit on a network round trip for minutes". So a slow push read as: the dialog
 * vanished, nothing happened, and some minutes later a toast appeared. Fetch, Pull and Merge in
 * the branch popup were the same silence — they move `useBranches.busy`, but the only thing that
 * has ever done is grey out buttons in a popup that is shut by then.
 *
 * # Why the numbers here are counts and never a percentage
 *
 * Because there is no percentage, and reaching for one is a bigger change than it looks.
 * `cide_git::push`'s only `RemoteCallbacks` registers `sideband_progress` and
 * `push_update_reference` — no `push_transfer_progress` — and that route is taken only for local
 * remotes anyway, since `route()` hands a real remote to the `git` binary through `.output()`,
 * which returns when the whole push is over. `cide-git` also tried the sideband stream as a UI
 * source once and rejected it, with a test pinning the decision by name
 * (`a_fetch_reports_a_sentence_and_not_a_progress_meter`).
 *
 * So the honest claim is *this is running*, plus a real `done/total` when one gesture covers
 * several repositories — which is a count of answers received, not a guess at bytes.
 *
 * **Zero imports, not even type-only ones.** `chrome/pushModel.ts`'s rule, for its reason: a
 * value import is what stops `ui/scripts/check-push.mjs` compiling this file on its own. The
 * shapes below are declared structurally; `ProjectId` is `string` on the wire and is spelled that
 * way here rather than imported.
 */

/** The four operations worth an indicator. See [`GIT_OP_VERB`] for why checkout is not one. */
export type GitOpKind = 'push' | 'pull' | 'fetch' | 'merge'

/**
 * One gesture in flight.
 *
 * A *gesture*, not a request: `git.push` names no repository and so acts on every one in the
 * project, which is why `total` exists and is usually 1.
 */
export interface RunningOp {
  /** Monotonic, and the reason the list can be read as oldest-first. */
  id: number
  /** The `ProjectId` this was started for. Scoping is the whole of [`gitOpLabel`]'s filter. */
  project: string
  kind: GitOpKind
  /** Units **settled** — fulfilled or rejected. A three-repository push opens at 0. */
  done: number
  /** Units this gesture set out to do. */
  total: number
}

/**
 * The present participle, per operation.
 *
 * A `Record` over the union rather than a `switch` with a default, so adding a kind is a compile
 * error here instead of a gesture that silently draws the word `Working`.
 *
 * **Checkout is deliberately absent**, and so are branch create, rename and delete. They are
 * local and finish in milliseconds; an indicator that flickered on every one of them would teach
 * people to stop looking at it, which costs exactly the slow push this was built for.
 */
const GIT_OP_VERB: Record<GitOpKind, string> = {
  push: 'Pushing',
  pull: 'Pulling',
  fetch: 'Fetching',
  merge: 'Merging',
}

/** This project's gestures, oldest first. `id` is monotonic, so insertion order is that order. */
function mine(running: readonly RunningOp[], project: string): RunningOp[] {
  return running.filter((op) => op.project === project)
}

/**
 * What the bar says: `Pushing…`, or `Pushing 1/3…` when one gesture covers several repositories.
 *
 * `''` when this project has nothing running, which is what the component renders as nothing at
 * all — see `GitOpIndicator.tsx` for why it returns `null` rather than drawing an empty box.
 *
 * # The oldest gesture wins, and the others are only in the tooltip
 *
 * Two gestures can overlap — push a project, switch repository, fetch — and none of the obvious
 * answers is honest. Summing `done`/`total` across kinds produces a fraction whose numerator and
 * denominator count different things. Showing the newest makes the label jump backwards the
 * moment a second gesture starts, which reads as the first one having failed. So the label
 * describes one gesture, the oldest, and [`gitOpTitle`] is where the rest become visible.
 */
export function gitOpLabel(running: readonly RunningOp[], project: string): string {
  const first = mine(running, project)[0]
  if (first === undefined) return ''
  const verb = GIT_OP_VERB[first.kind]
  return first.total > 1 ? `${verb} ${first.done}/${first.total}…` : `${verb}…`
}

/**
 * The tooltip: every running gesture, spelled out.
 *
 * This is the one surface that admits to a second concurrent gesture, and it says `repositories`
 * in full because a tooltip has the room the 26px bar does not.
 */
export function gitOpTitle(running: readonly RunningOp[], project: string): string {
  const ops = mine(running, project)
  if (ops.length === 0) return ''
  return ops
    .map((op) => {
      const verb = GIT_OP_VERB[op.kind]
      return op.total > 1 ? `${verb} — ${op.done} of ${op.total} repositories done` : verb
    })
    .join(' · ')
}
