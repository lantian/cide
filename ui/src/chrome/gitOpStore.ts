/**
 * Which git operations are in flight, for the status bar's indicator. (M65)
 *
 * # Why a module store and not `useState` anywhere
 *
 * The producers are outside React's tree in every case. `chrome/pushRun.ts` is called from
 * `keys/dispatch.ts`, from the branch popup and from the Git panel; the branch popup's own
 * roads run inside promise chains that outlive the popup being dismissed. That is
 * `chrome/notices.ts`'s argument for being a module, and `chrome/gitCountStore.ts`'s for being
 * one: "an always-live module store subscribed from chrome that is always mounted".
 *
 * Deliberately **not** part of `store/workspace.ts`. That store mirrors Rust-owned durable state
 * and holds nothing local, and nothing here survives a reload — `chrome/pushStore.ts` states the
 * rule in the same words.
 *
 * # Scope is a project, because that is where the outcome lands
 *
 * `chrome/notices.ts` scopes a toast to the window *and* the project: in `Stacked` window mode
 * one window holds every open project, so "a toast is the outcome of something done **in** a
 * project". This indicator is the same fact earlier in time and takes the same scope — every
 * gesture carries its `ProjectId` and the bar draws only the active project's.
 *
 * The consequence worth naming: a push started from a **detached-pane** window shows no
 * indicator, because `App.tsx` returns before the status bar for that window role. Its toast
 * still lands there, exactly as before.
 */
import { create } from 'zustand'
import type { ProjectId } from '@/ipc/client'
import type { GitOpKind, RunningOp } from './gitOpModel'

interface GitOpStore {
  /** Oldest first, which `nextId` below is what guarantees. */
  running: readonly RunningOp[]
}

export const useGitOps = create<GitOpStore>(() => ({ running: [] }))

/**
 * Monotonic, and never reset.
 *
 * It is the identity a `settle` closes over, so two gestures of the same kind on the same
 * project cannot be confused for one another — and because it only ever rises, appending to
 * `running` keeps the list in oldest-first order for free. `gitOpModel.gitOpLabel` reads that
 * order as a rule rather than as an accident, so nothing here may sort or splice out of order.
 */
let nextId = 0

/**
 * Begin a gesture, and hand back the function that reports one unit of it finished.
 *
 * `total` is how many answers this gesture is waiting for — the number of repositories a push
 * was fanned out to, and 1 for everything else.
 *
 * # `settle` is idempotent, and that is load-bearing
 *
 * It is called from promise callbacks, and the cheapest correct way to call it on both outcomes
 * is `then(settle, settle)` — which would run it twice if a promise could ever do both. It
 * cannot, but the guard costs a boolean and the failure it prevents is a gesture that removes
 * itself one unit early and leaves the bar clean while a push is still running.
 *
 * # There is no timeout
 *
 * Deliberately. A push over a slow link genuinely runs for minutes — `git_push`'s own doc says
 * so — and a timer that cleared the indicator would be making a claim the app cannot support:
 * that the push had ended. The indicator's promise is *this is still running*, and the only
 * thing that knows when that stops being true is the promise it was given.
 */
export function beginGitOp(project: ProjectId, kind: GitOpKind, total = 1): () => void {
  // A gesture with nothing to wait for is not a gesture. Guarding here rather than at the call
  // sites keeps `pushPass`'s `previews.length` honest for the empty selection it already returns
  // on, and stops a zero-total row sitting in `running` for ever because nothing will ever
  // settle it.
  if (total <= 0) return () => {}

  const id = nextId
  nextId += 1
  useGitOps.setState((s) => ({ running: [...s.running, { id, project, kind, done: 0, total }] }))

  let spent = 0
  return () => {
    if (spent >= total) return
    spent += 1
    useGitOps.setState((s) => ({
      running: s.running.flatMap((op) => {
        if (op.id !== id) return [op]
        const done = op.done + 1
        return done >= op.total ? [] : [{ ...op, done }]
      }),
    }))
  }
}

/**
 * The one-unit case: track `work` and hand back **`work` itself**.
 *
 * # Why this returns the original promise and attaches a side branch
 *
 * Both of the obvious spellings reintroduce the bug this helper exists to keep out of four call
 * sites, and `chrome/pushRun.ts` carries the long version of the argument.
 *
 * `work.finally(settle)` returns a *new* promise that rejects whenever `work` does. Nothing is
 * attached to it, so it fires `unhandledrejection` — which `chrome/Failures.tsx` turns into a
 * toast — **in addition to** the one the caller's own error path raises. The two do not collapse
 * in `notices.admit`, which keys on the text: the caller's says a sentence, and the stray one
 * says the bare word `push`, because `describe` falls through a `GitError` with no `message` to
 * its `kind`.
 *
 * `work.then(v => { settle(); return v }, e => { settle(); throw e })` is worse in a quieter way:
 * it moves the rejection onto a new promise, so whether the failure is ever reported at all
 * depends on the caller remembering to use the return value rather than the original.
 *
 * So: `work.then(settle, settle)` is a side branch that fulfils on both outcomes and can never
 * reject, and the value handed back is the untouched `work`, whose error path is exactly what it
 * was before this call was added.
 */
export function trackGitOp<T>(
  project: ProjectId,
  kind: GitOpKind,
  work: Promise<T>,
): Promise<T> {
  const settle = beginGitOp(project, kind)
  void work.then(settle, settle)
  return work
}
