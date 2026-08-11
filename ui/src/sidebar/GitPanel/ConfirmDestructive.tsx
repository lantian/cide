/**
 * The confirmation in front of anything in this panel that throws work away.
 *
 * Reverting a changelist is one click from a lost afternoon: `git_rollback` checks out HEAD
 * over the tracked files and *deletes* the untracked ones, `cmd/git.rs` will not ask first —
 * *"a confirmation the backend cannot show is not a safeguard"* — and git has no undo for
 * either. `CloseConfirm` is the house precedent and this follows its three rules:
 *
 * 1. **It names what would be lost**, every path, not a count. The user is about to lose
 *    *specific* files, and "12 files" is precisely the thing they cannot check before
 *    clicking. This is why `ConfirmState.files` is a list and not a number.
 * 2. **It only appears when something is at risk.** `useGitPanel` builds no state for an empty
 *    group, so the menu item is disabled rather than opening a dialog about nothing.
 * 3. **Cancel is the default.** Escape cancels, initial focus is on Cancel, and the
 *    destructive button is the plain one while the accent-filled button is the safe one — so
 *    a reflexive Enter, the keystroke already in flight when the dialog appeared, backs out.
 *
 * It takes a `run` rather than a specific verb because the panel has four of them (revert a
 * changelist, revert a file, delete unversioned files, drop a shelf entry) and they differ
 * only in their wording and their command.
 */
import { useEffect, useRef } from 'react'
import { OverlayCard } from '@/overlays/ModalShell'
import { splitPath } from './model'
import type { ConfirmState } from './types'
import styles from './ConfirmDestructive.module.css'

export interface ConfirmDestructiveProps {
  state: ConfirmState
  onCancel: () => void
  /** Go ahead. The caller runs `state.run` and closes; this component does neither. */
  onConfirm: () => void
}

export function ConfirmDestructive({ state, onCancel, onConfirm }: ConfirmDestructiveProps) {
  const cancel = useRef<HTMLButtonElement>(null)

  useEffect(() => {
    cancel.current?.focus()
  }, [])

  const onKeyDown = (ev: React.KeyboardEvent) => {
    if (ev.key === 'Escape') {
      ev.stopPropagation()
      onCancel()
    }
  }

  return (
    <OverlayCard label={state.title} onDismiss={onCancel}>
      <div className={styles.dialog} onKeyDown={onKeyDown} data-audit="gitConfirm">
        <div className={styles.head}>
          <h2 className={styles.title} data-audit="gitConfirmTitle">
            {state.title}
          </h2>
          <p className={styles.body}>{state.body}</p>
        </div>

        <ul className={styles.list} data-audit="gitConfirmList">
          {state.files.map((path) => {
            const { name, dir } = splitPath(path)
            return (
              <li key={path} className={styles.row} title={path}>
                {/* `−` rather than the tab strip's `●`: what is about to happen to these rows
                    is removal, and reusing the dirty dot would say "unsaved" instead. */}
                <span className={styles.mark} aria-hidden="true">
                  −
                </span>
                <span className={styles.name}>{name}</span>
                <span className={styles.where}>{dir}</span>
              </li>
            )
          })}
        </ul>

        <div className={styles.footer}>
          <button
            type="button"
            className={`${styles.button} ${styles.buttonDanger}`}
            data-audit="gitConfirmRun"
            onClick={onConfirm}
          >
            {state.confirmLabel}
          </button>
          <button
            ref={cancel}
            type="button"
            className={`${styles.button} ${styles.buttonPrimary}`}
            data-audit="gitConfirmCancel"
            onClick={onCancel}
          >
            Cancel
          </button>
        </div>
      </div>
    </OverlayCard>
  )
}
