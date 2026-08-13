/**
 * "You are about to do this to these files. Are you sure?"
 *
 * The house dialog for a gesture that removes things, wherever it is made. It lived in
 * `sidebar/GitPanel/` and moved here when the file tree needed the same dialog: the reasons
 * the two panels want it are different, the dialog is identical, and two copies of a
 * safeguard is how one of them quietly stops matching the other.
 *
 * `CloseConfirm` beside it is the original precedent and all three of them follow its rules:
 *
 * 1. **It names what would be lost**, every path, not a count. The user is about to act on
 *    *specific* files, and "12 files" is precisely the thing they cannot check before
 *    clicking. This is why [`ConfirmState.files`] is a list and not a number.
 * 2. **It only appears when something is at risk.** `useGitPanel` builds no state for an empty
 *    group, so the menu item is disabled rather than opening a dialog about nothing.
 * 3. **Cancel is the default.** Escape cancels, initial focus is on Cancel, and the
 *    destructive button is the plain one while the accent-filled button is the safe one — so
 *    a reflexive Enter, the keystroke already in flight when the dialog appeared, backs out.
 *
 * It takes a `run` rather than a specific verb because there are six of them now, and they
 * differ only in their wording and their command.
 *
 * # The two callers, and why they are not the same argument
 *
 * * **The Git panel**, for reverting a changelist or a file, deleting unversioned files, and
 *   dropping a shelf entry. `git_rollback` checks out HEAD over the tracked files and
 *   *deletes* the untracked ones, `cmd/git.rs` will not ask first — *"a confirmation the
 *   backend cannot show is not a safeguard"* — and git has no undo for any of it. The dialog
 *   is there because the act is **irreversible**.
 * * **The file tree**, for *Move to Trash*. That act is reversible: `fs_delete` moves to the
 *   freedesktop trash and never unlinks, which is exactly why the context-menu item shipped
 *   with no confirmation at all and was right to. The dialog appeared when the gesture got a
 *   **bare Delete key**, one finger from the arrow keys that same handler uses to move the
 *   selection — the cost of a misfire went from "impossible by accident" to "trivially
 *   likely", and the recovery went from nothing to a desktop trash the user has to go and
 *   find. The dialog is there because the act became **easy to trigger by accident**.
 *
 * Both reasons produce the same dialog; only the body text differs, which is what the body
 * text is for. What they must not do is diverge in their *shape* — one that named the paths
 * and one that counted them would teach the user to read one and skim the other.
 */
import { useEffect, useRef } from 'react'
import { OverlayCard } from '@/overlays/ModalShell'
import { basename, dirname } from '@/overlays/format'
import styles from './ConfirmDestructive.module.css'

/**
 * A confirmation for something a user cannot take back easily.
 *
 * `files` is every path at stake and the dialog names all of them. A count is not enough: the
 * user is about to act on *specific* files, and "12 files" is not something anyone can check
 * before clicking. Same rule `CloseConfirm` follows, for the same reason.
 */
export interface ConfirmState {
  title: string
  body: string
  files: readonly string[]
  /** The word on the destructive button, e.g. `Revert 4 files`. */
  confirmLabel: string
  run: () => void
}

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
      <div className={styles.dialog} onKeyDown={onKeyDown} data-audit="confirmDestructive">
        <div className={styles.head}>
          <h2 className={styles.title} data-audit="confirmDestructiveTitle">
            {state.title}
          </h2>
          <p className={styles.body}>{state.body}</p>
        </div>

        <ul className={styles.list} data-audit="confirmDestructiveList">
          {state.files.map((path) => {
            // `basename`/`dirname` rather than the Git panel's `splitPath`, which does the
            // same arithmetic: this file no longer lives in that panel, and a chrome component
            // reaching back into a feature folder for a two-line helper is the import that
            // makes a shared component un-shareable again.
            const name = basename(path)
            const dir = dirname(path)
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
            data-audit="confirmDestructiveRun"
            onClick={onConfirm}
          >
            {state.confirmLabel}
          </button>
          <button
            ref={cancel}
            type="button"
            className={`${styles.button} ${styles.buttonPrimary}`}
            data-audit="confirmDestructiveCancel"
            onClick={onCancel}
          >
            Cancel
          </button>
        </div>
      </div>
    </OverlayCard>
  )
}
