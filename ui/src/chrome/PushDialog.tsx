/**
 * *Push Commits* — IDEA's dialog, with cide's multi-root tree. (M31)
 *
 * One row per git repository in the project, each with a checkbox and the commits it would send.
 * `ConflictsDialog.tsx` is the template — `OverlayCard`, head / list / foot, Escape on the card —
 * and not `ConfirmDestructive`, whose list is a flat `readonly string[]`: this one is two levels
 * deep and each root carries its own answer.
 *
 * It owns no rules. `chrome/pushModel.ts` decides what every sentence says, `chrome/pushStore.ts`
 * holds the question, and `keys/dispatch.ts`'s `pushPass` is what actually sends.
 *
 * # The three things this component must keep doing
 *
 * **Cancel is focused and accented.** `ConfirmDestructive`'s rule 3, and this dialog is opened by
 * a keystroke — the Enter that was already in flight has to back out.
 *
 * **Force starts unticked on every open**, including a reopen after a dismissal. Carrying it
 * would mean somebody who ticked it, cancelled, and pushed again overwriting a remote they never
 * saw the warning for. The `useEffect` keyed on `pending` is what guarantees it.
 *
 * **Force is disabled unless it applies.** `forceApplies` is false when no ticked row has
 * diverged, and a checkbox that reads the same whether it is about to overwrite a colleague's
 * work or do nothing at all is one people learn to tick.
 */
import { useEffect, useRef, useState } from 'react'
import { OverlayCard } from '@/overlays/ModalShell'
import { Icon } from '@/icons/Icon'
import {
  blockedNote,
  confirmLabel,
  defaultChecked,
  forceApplies,
  forceNote,
  outgoing,
  outgoingTotal,
  pushSummary,
  pushable,
  targetLabel,
} from './pushModel'
import { usePush } from './pushStore'

import styles from './PushDialog.module.css'

export function PushDialog() {
  const pending = usePush((s) => s.pending)
  const [checked, setChecked] = useState<readonly string[]>([])
  const [collapsed, setCollapsed] = useState<readonly string[]>([])
  const [force, setForce] = useState(false)
  const cancel = useRef<HTMLButtonElement>(null)

  // Reset on every new question, force above all — see the header.
  useEffect(() => {
    setChecked(pending === null ? [] : defaultChecked(pending.previews))
    setCollapsed([])
    setForce(false)
  }, [pending])

  /*
   * Focus lands on Cancel once per question, and never again.
   *
   * A `ref` callback calling `focus()` would look equivalent and is not: React runs it on every
   * render, so ticking the Force checkbox would move focus back to Cancel and the next space
   * would dismiss the dialog. `ConfirmDestructive` uses the same effect for the same reason.
   */
  useEffect(() => {
    cancel.current?.focus()
  }, [pending])

  if (pending === null) return null
  const { previews } = pending

  const total = outgoingTotal(previews, checked)
  const canForce = forceApplies(previews, checked)
  // Ticked and then made irrelevant by unticking the diverged row: the flag stays in state so
  // re-ticking that row restores the user's answer, but it must not travel on the wire.
  const forcing = force && canForce
  const single = previews.length === 1

  const toggle = (id: string) => {
    setChecked((now) => (now.includes(id) ? now.filter((x) => x !== id) : [...now, id]))
  }
  const fold = (id: string) => {
    setCollapsed((now) => (now.includes(id) ? now.filter((x) => x !== id) : [...now, id]))
  }

  return (
    <OverlayCard label="Push commits" onDismiss={() => usePush.getState().dismiss()}>
      <div
        onKeyDown={(ev) => {
          if (ev.key === 'Escape') {
            ev.stopPropagation()
            usePush.getState().dismiss()
          }
        }}
        data-audit="pushDialog"
      >
        <div className={styles.head}>
          <h2 className={styles.title}>Push commits</h2>
          <p className={styles.body} data-audit="pushSummary">
            {pushSummary(previews, checked)}
          </p>
        </div>

        <ul className={styles.list} data-audit="pushList">
          {previews.map((preview) => {
            const id = preview.repo.id
            const can = pushable(preview)
            const open = !collapsed.includes(id) && preview.commits.length > 0
            const block = blockedNote(preview)
            const warn = forceNote(preview)
            return (
              <li key={id} className={styles.repo} data-audit="pushRepo" data-repo={id}>
                <div className={styles.repoRow}>
                  <button
                    type="button"
                    className={styles.twisty}
                    disabled={preview.commits.length === 0}
                    aria-label={open ? 'Collapse' : 'Expand'}
                    aria-expanded={open}
                    onClick={() => fold(id)}
                  >
                    <Icon name={open ? 'chevron-down' : 'chevron-right'} size={1} />
                  </button>
                  <input
                    type="checkbox"
                    className={styles.tick}
                    checked={can && checked.includes(id)}
                    disabled={!can}
                    aria-label={`Push ${preview.repo.name}`}
                    data-audit="pushRepoTick"
                    onChange={() => toggle(id)}
                  />
                  {!single && <span className={styles.repoName}>{preview.repo.name}</span>}
                  <span className={styles.target} title={preview.refspec}>
                    {targetLabel(preview)}
                  </span>
                  {can && (
                    <span className={styles.count}>
                      {outgoing(preview) === 1 ? '1 commit' : `${outgoing(preview)} commits`}
                    </span>
                  )}
                </div>

                {block !== '' && <div className={styles.note}>{block}</div>}
                {/* Only while Force is ticked. The sentence is about what forcing would do, and
                    on a row nobody is forcing it would be a warning about nothing. */}
                {warn !== '' && forcing && (
                  <div className={`${styles.note} ${styles.noteWarn}`} data-audit="pushForceNote">
                    {warn}
                  </div>
                )}

                {open && (
                  <ul className={styles.commits}>
                    {preview.commits.map((commit) => (
                      <li
                        key={commit.shortOid}
                        className={styles.commit}
                        data-audit="pushCommit"
                        title={commit.summary}
                      >
                        <span className={styles.oid}>{commit.shortOid}</span>
                        <span className={styles.summary}>{commit.summary}</span>
                        <span className={styles.author}>{commit.author}</span>
                      </li>
                    ))}
                    {preview.more > 0 && (
                      <li className={styles.more}>{`…and ${preview.more} more`}</li>
                    )}
                  </ul>
                )}
              </li>
            )
          })}
        </ul>

        <div className={styles.foot}>
          <label
            className={`${styles.force} ${canForce ? '' : styles.forceOff}`}
            data-audit="pushForce"
            title={
              canForce
                ? 'Overwrite the remote branch, refusing if it moved since your last fetch'
                : 'Nothing selected would be rejected, so there is nothing to force'
            }
          >
            <input
              type="checkbox"
              checked={forcing}
              disabled={!canForce}
              onChange={(ev) => setForce(ev.target.checked)}
            />
            Force push (with lease)
          </label>
          <button
            type="button"
            className={`${styles.action} ${forcing ? styles.danger : ''}`}
            disabled={total === 0}
            data-audit="pushConfirm"
            onClick={() => usePush.getState().confirm(checked, forcing)}
          >
            {confirmLabel(total, forcing)}
          </button>
          <button
            type="button"
            className={`${styles.action} ${styles.primary}`}
            data-audit="pushCancel"
            ref={cancel}
            onClick={() => usePush.getState().dismiss()}
          >
            Cancel
          </button>
        </div>
      </div>
    </OverlayCard>
  )
}
