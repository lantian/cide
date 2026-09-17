/**
 * Sending a push, once somebody has said yes. (M31)
 *
 * # Why this is not in `keys/dispatch.ts` beside `pullPass`
 *
 * That is where it started and it cannot stay there. Three surfaces raise this dialog — the
 * palette, the branch popup and the Git panel's *Commit and Push…* — and `dispatch.ts` already
 * imports `openBranchPopup` from `chrome/BranchSelector.tsx`, so the popup importing the runner
 * back out of `dispatch.ts` is a cycle. ESM tolerates one; what it does not tolerate is the
 * order-dependent `undefined` that appears the day somebody moves an import, with nothing at the
 * failure site naming the cause. `pullPass` has one caller and stays where it is.
 *
 * The *sentences* still live in `chrome/branchModel.ts` (`pushNote`, `pushReport`, `explain`) and
 * the *rules* in `chrome/pushModel.ts`. This module is only the fan-out.
 */
import { git as gitApi, type ProjectId, type PushPreview } from '@/ipc/client'
import { explain, pushReport, type RepoPush } from './branchModel'
import { notify } from './notices'
import { requestPush } from './pushStore'

/**
 * Send one push per repository, and report the lot as one notice.
 *
 * Factored out of `case 'git.push'` when the dialog arrived, exactly as `pullPass` was factored
 * out of `git.pull`: three surfaces now push — the palette, the branch popup and *Commit and
 * Push…* — and all three go through the dialog, so all three land here. One place that knows how
 * to fan out, report and fail, and therefore one place that can be wrong.
 *
 * `previews` is what the dialog was drawn from, so each attempt sends *that* row's refspec and
 * remote rather than re-resolving them: the user ticked a target they read, and re-deriving it
 * here would be a second answer to a question already answered on screen.
 *
 * # The two defects this shape exists to prevent, both fixed in M20 and both easy to reintroduce
 *
 * `Promise.all` with the value discarded — "called, and the answer dropped on the floor" — gave
 * a user who pressed the key the same silence a dead key would. And with four repositories and
 * two failures, `Promise.all` reports the first and marks the rest handled. `allSettled` over
 * the *same* promises collects the answers without disturbing the failure path, and the
 * per-attempt `.catch` is what turns a raw `GitError` — `{kind:'push', detail:{output}}`, which
 * has no `message` and whose `detail` is an object — into a sentence instead of the bare word
 * `push`.
 */
function pushPass(
  project: ProjectId,
  previews: readonly PushPreview[],
  force: boolean,
): void {
  if (previews.length === 0) return
  const attempts = previews.map((preview) =>
    gitApi.push(project, preview.repo.id, {
      remote: preview.remote,
      refspec: preview.refspec,
      // The publish case, and the only caller that has ever known it up front: the preview read
      // it off the absent remote-tracking ref before anything was sent.
      setUpstream: preview.publish,
      // `force` is the dialog's answer for the whole gesture, but it is only *sent* on the rows
      // it could apply to. A `--force-with-lease` on a fast-forward is a no-op, and sending one
      // anyway would put the flag in the reflog of pushes that never needed it.
      force: force && preview.diverged,
    }),
  )
  for (const attempt of attempts) {
    void attempt.catch((error: unknown) => {
      throw new Error(explain(error, 'push'))
    })
  }
  void Promise.allSettled(attempts).then((settled) => {
    const done: RepoPush[] = []
    settled.forEach((result, i) => {
      const preview = previews[i]
      if (result.status === 'fulfilled' && preview !== undefined) {
        done.push({ name: preview.repo.name, outcome: result.value })
      }
    })
    if (done.length === 0) return
    // Aggregated into one notice, never one per repository: `notices.admit` collapses toasts by
    // identical text, so five submodules all saying "already up to date" would show one toast
    // that silently spoke for five.
    const report = pushReport(done)
    // Stamped with the project rather than left to the ambient scope: a push is seconds of
    // network, and a report that lands after the user has switched projects would otherwise be
    // filed under — and shown only in — the project they switched *to*.
    notify(report.text, { kind: 'ok', detail: report.detail, project })
  })
}

/**
 * Read what every repository would push, then ask.
 *
 * The single road to the dialog — the palette, the branch popup and *Commit and Push…* all call
 * this, so there is one answer to *what does pushing mean here* rather than three that drift.
 *
 * A plan with nothing pushable in it still opens the dialog rather than toasting *already up to
 * date*: the rows say **why** each repository has nothing to send, and three of the reasons
 * (unborn, detached, no remote) are states the user will want to see named.
 */
export function openPushDialog(project: ProjectId, only?: readonly string[]): void {
  void gitApi
    .pushPlan(project)
    .then((plan) => {
      if (plan.length === 0) {
        throw new Error('There is no git repository in this project.')
      }
      // `only` narrows the dialog to the repositories a gesture was actually about — *Commit and
      // Push…* over the repositories that committed. Without it the dialog is the whole project,
      // which is what `git.push` has always meant and what the palette and the branch popup both
      // want. A narrowing that matched nothing would draw an empty dialog, so it falls back to
      // the whole plan rather than to a card with no rows in it.
      const narrowed = only === undefined ? plan : plan.filter((p) => only.includes(p.repo.id))
      const previews = narrowed.length === 0 ? plan : narrowed
      requestPush({
        project,
        previews,
        proceed: (repos, force) => {
          pushPass(
            project,
            previews.filter((p) => repos.includes(p.repo.id)),
            force,
          )
        },
      })
    })
    .catch((error: unknown) => {
      throw new Error(explain(error, 'push'))
    })
}

