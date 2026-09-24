/**
 * *Files Merged with Conflicts* — IDEA's list, with a decision per row. (M20)
 *
 * Three answers per file, which are IDEA's three: **Accept ⟨ours⟩**, **Accept ⟨theirs⟩**, and
 * *Merge…*, which opens the three-pane resolver as a tab. Plus the two that act on all of them —
 * *Resolve simple* and *Abort* — and *Continue*, which is enabled only when nothing is left.
 *
 * # The side labels are the file's, not the words "yours" and "theirs"
 *
 * During a **rebase** git swaps stage 2 and stage 3: `HEAD` is the branch being rebased *onto*
 * and your own commit is the incoming side. A dialog that hardcoded *Accept Yours* over stage 2
 * would hand the user the other branch's work under their own name, which is the worst kind of
 * wrong — it looks like it worked. `cide_git::conflict::side_labels` decides the wording and
 * `MergeState.ours` / `.theirs` carry it; this only draws it.
 *
 * # Why it is a dialog when `MergeBar` is a bar
 *
 * `GuardBar`'s header argues that the staging guard must not be modal, because the honest
 * default there is *do nothing* and a modal would interrupt a commit message being typed. Both
 * halves of that argument fail here. A conflict has no honest do-nothing state — nothing else in
 * git will work until it is answered — and the gesture that produced it may well have been made
 * from a terminal pane with the sidebar shut, where the bar is not on screen at all.
 *
 * So this is the one merge surface that appears on its own. The bar remains the standing one:
 * dismiss this and the work is still findable, which is why *Later* is offered and Escape works.
 */
import { Modal } from '@/overlays/ModalShell'
import { Button } from '@/kit/components/Button'
import { Dialog } from '@/kit/components/Overlay'
import { PathList, PathRow } from '@/kit/components/Surface'
import { branch as branchApi, file as fileApi, type ConflictSide } from '@/ipc/client'
import { explain } from './branchModel'
import { notify } from './notices'
import { afterResolve, useConflicts } from './conflictsStore'
import { Icon } from '@/icons/Icon'

import styles from './ConflictsDialog.module.css'

export function ConflictsDialog() {
  const pending = useConflicts((s) => s.pending)
  if (pending === null) return null

  const { project, repo, repoName, state } = pending
  const left = state.entries.filter((e) => !e.resolved)
  const rebasing = state.operation === 'rebase'

  /**
   * Re-read after every verb, so the list redraws from git rather than from hope.
   *
   * Through `afterResolve` once nothing is left, which is what commits the merge when the last
   * row is answered here rather than in the three-pane tab — the two routes to the same
   * situation must not end differently.
   */
  const reread = async () => {
    const next = await branchApi.conflicts(project, repo).catch(() => null)
    if (next !== null && next.entries.every((e) => e.resolved)) {
      await afterResolve(project, repo, repoName)
      return
    }
    useConflicts.getState().update(next)
  }

  const take = (path: string, side: ConflictSide) => {
    void branchApi
      .conflictTake(project, repo, path, side)
      .then(reread)
      .catch((error: unknown) => notify(explain(error), { kind: 'error' }))
  }

  const title = `${
    rebasing ? `Rebasing ${state.ours} onto ${state.theirs}` : `Merging ${state.theirs} into ${state.ours}`
  }${repoName !== '' ? ` — ${repoName}` : ''}`

  return (
    <Modal onDismiss={() => useConflicts.getState().close()}>
      <Dialog
        title={title}
        lead={
          left.length === 0
            ? 'Every file has been answered. Continue to finish.'
            : `${left.length} of ${state.entries.length} file${state.entries.length === 1 ? '' : 's'} still need a decision. Take one side whole, or open the three-pane merge to combine them.`
        }
        width="picker"
        flush
        data-audit="conflictsDialog"
        footNote={
          state.step !== undefined && state.step !== null
            ? `Step ${state.step.done} of ${state.step.total}`
            : undefined
        }
        actions={
          <>
            <Button
              onClick={() => {
                void branchApi
                  .mergeAbort(project, repo)
                  .then(() => useConflicts.getState().close())
                  .catch((error: unknown) => notify(explain(error), { kind: 'error' }))
              }}
              title="Put the working tree back exactly as it was before this started"
            >
              Abort
            </Button>
            <Button
              onClick={() => useConflicts.getState().close()}
              title="Leave the conflict as it is; the Git panel keeps the controls"
            >
              Later
            </Button>
            <Button
              variant="primary"
              disabled={left.length > 0}
              data-audit="conflictsContinue"
              onClick={() => {
                void branchApi
                  .mergeContinue(project, repo)
                  .then((outcome) => {
                    const next = outcome.state ?? null
                    useConflicts.getState().update(next)
                    if (next === null) notify('Merge finished', { kind: 'ok' })
                  })
                  .catch((error: unknown) => notify(explain(error), { kind: 'error' }))
              }}
              title={
                left.length > 0
                  ? `${left.length} file${left.length === 1 ? '' : 's'} still to answer`
                  : rebasing
                    ? 'Commit this step and go on to the next'
                    : 'Commit the merge'
              }
            >
              Continue
            </Button>
          </>
        }
      >
        <PathList data-audit="conflictsList">
          {state.entries.map((entry) => {
            const cut = entry.path.lastIndexOf('/')
            return (
              <PathRow
                key={entry.path}
                full={entry.path}
                data-audit="conflictRow"
                name={cut === -1 ? entry.path : entry.path.slice(cut + 1)}
                where={cut === -1 ? '' : entry.path.slice(0, cut)}
                mark={
                  <span className={entry.resolved ? styles.markDone : styles.mark}>
                    <Icon name={entry.resolved ? 'check' : 'swords'} size={1} />
                  </span>
                }
                trailing={
                  entry.resolved ? (
                    'resolved'
                  ) : (
                    <>
                      <Button size="sm" onClick={() => take(entry.path, 'ours')}>
                        {`Accept ${state.ours}`}
                      </Button>
                      <Button size="sm" onClick={() => take(entry.path, 'theirs')}>
                        {`Accept ${state.theirs}`}
                      </Button>
                      <Button
                        size="sm"
                        disabled={entry.binary}
                        title={
                          entry.binary
                            ? 'This file is binary — take one side whole'
                            : 'Open the three-pane merge'
                        }
                        onClick={() => {
                          void fileApi.openMergeTab(project, repo, entry.path).catch(() => {})
                          useConflicts.getState().close()
                        }}
                      >
                        Merge…
                      </Button>
                    </>
                  )
                }
              />
            )
          })}
        </PathList>
      </Dialog>
    </Modal>
  )
}
