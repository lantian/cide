/**
 * The confirmation shown before a paste that would overwrite something.
 *
 * > *"Paste collisions - yes, should be a confirmation"*
 *
 * `CloseConfirm.tsx` beside it is the house precedent and this follows it deliberately: the
 * same `OverlayCard`, the same head/list/footer, the same colours, and the same division of
 * labour — the refusal lives in the domain and this is only the thing that lets a user lift it.
 * `fs_paste` renames on collision unless it is handed an explicit decision, so a bug in this
 * component costs a stray `main copy.rs` and never a lost file.
 *
 * Three properties are the whole design, and each is a way this dialog usually goes wrong:
 *
 * 1. **It appears before anything is written.** `fs.paste_plan` answers which names are taken
 *    without touching the disk; every question is asked; only then is `fs_paste` called. So
 *    Cancel is a command that never happens, and the dialog can say so
 *    ([`nothingWrittenYet`]). The version that asks as it writes can only apologise.
 * 2. **Replace is not the default.** Enter and the initial focus go to *Keep both*, the accent
 *    fill goes to *Keep both*, and Replace is the plain button in `--red`. A stray copy can be
 *    deleted; an overwritten file cannot be brought back.
 * 3. **Ten collisions are not ten dialogs.** The checkbox answers the rest in one go, naming
 *    how many "the rest" is.
 *
 * The decisions themselves are in `pasteConfirmModel.ts`, which `check-fs-clipboard.mjs` compiles
 * and exercises on its own. Mounted by `sidebar/FileTree.tsx` — the panel that owns the paste
 * gesture — rather than by the shell, so the feature is reachable without touching `App.tsx`.
 */
import { useEffect, useRef, useState } from 'react'
import { OverlayCard } from '@/overlays/ModalShell'
import {
  applyToRestLabel,
  askBody,
  askProgress,
  askTitle,
  canReplace,
  currentCollision,
  keepBothLabel,
  nothingWrittenYet,
  replaceLabel,
  type PasteAnswer,
  type PasteAsk,
} from './pasteConfirmModel'
import styles from './PasteConfirm.module.css'

export interface PasteConfirmProps {
  /**
   * The questions and the answers so far.
   *
   * Typed as the model's own structural `CollisionLike`, not as the generated `PasteCollision`
   * — `pasteConfirmModel.ts` is compiled on its own by the check script and cannot import the
   * bindings. The two are pinned together at `startAsk` in `FileTree.tsx`, which is handed the
   * real generated array and stops compiling if `cargo xtask codegen` renames a field.
   */
  ask: PasteAsk
  /** An answer for the collision on screen, and whether it covers the remaining ones. */
  onAnswer: (answer: PasteAnswer, applyToRest: boolean) => void
  /** Back out. Nothing has been written, so there is nothing to undo. */
  onCancel: () => void
}

export function PasteConfirm({ ask, onAnswer, onCancel }: PasteConfirmProps) {
  const keep = useRef<HTMLButtonElement>(null)
  const [applyToRest, setApplyToRest] = useState(false)

  const collision = currentCollision(ask)
  const at = ask.answers.length

  // Focus the safe answer, and re-focus it for each question. A dialog that opens with the
  // destructive button focused turns the Enter that was already in flight — the one the user
  // pressed to paste — into the data loss it was put there to prevent.
  useEffect(() => {
    keep.current?.focus()
  }, [at])

  // Each question starts with the box clear. Carrying it over would mean a user who ticked it
  // for one paste, cancelled, and pasted again silently answering for files they never saw.
  useEffect(() => {
    setApplyToRest(false)
  }, [at])

  if (collision === null) return null

  // Escape cancels, caught on the card rather than on `document`: a window listener would also
  // answer for the terminal underneath and for any other overlay that happens to be open.
  const onKeyDown = (ev: React.KeyboardEvent) => {
    if (ev.key === 'Escape') {
      ev.stopPropagation()
      onCancel()
    }
  }

  const title = askTitle(collision)
  const progress = askProgress(ask)
  const restLabel = applyToRestLabel(ask)
  const replaceable = canReplace(collision)

  return (
    <OverlayCard label={title} onDismiss={onCancel}>
      <div className={styles.dialog} onKeyDown={onKeyDown} data-audit="pasteConfirm">
        <div className={styles.head}>
          <div className={styles.titleRow}>
            <h2 className={styles.title} data-audit="pasteConfirmTitle">
              {title}
            </h2>
            {/* Which question this is, when there is more than one. A user four deep in seven
                needs to know how many are left before deciding whether to use the checkbox. */}
            {progress !== null && (
              <span className={styles.progress} data-audit="pasteConfirmProgress">
                {progress}
              </span>
            )}
          </div>
          <p className={styles.body} data-audit="pasteConfirmBody">
            {askBody(collision)}
          </p>
          <p className={styles.where} title={collision.dest}>
            {collision.dest}
          </p>
        </div>

        {/*
          * The files a merge would overwrite, by name.
          *
          * Six of them at most (Rust's `PREVIEW_SAMPLE`) — this is a list the user reads, not a
          * manifest. The count in the sentence above is the whole truth; these are here so they
          * can tell whether the twelve files are the twelve they meant.
          */}
        {collision.sample.length > 0 && (
          <ul className={styles.list} data-audit="pasteConfirmList">
            <li className={styles.groupLabel}>Would be overwritten</li>
            {collision.sample.map((rel) => (
              <li key={rel} className={styles.row}>
                <span className={styles.mark} aria-hidden="true">
                  ●
                </span>
                <span className={styles.name}>{rel}</span>
              </li>
            ))}
            {collision.replaces > collision.sample.length && (
              <li className={`${styles.row} ${styles.more}`}>
                and {collision.replaces - collision.sample.length} more
              </li>
            )}
          </ul>
        )}

        <div className={styles.footer}>
          {/* Pushed left, away from the answers: a checkbox that sits beside `Replace` is one
              the user ticks by being 4px off, and it multiplies the destructive answer. */}
          {restLabel !== null && (
            <label className={styles.applyAll}>
              <input
                type="checkbox"
                checked={applyToRest}
                onChange={(ev) => setApplyToRest(ev.target.checked)}
                data-audit="pasteConfirmApplyAll"
              />
              {restLabel}
            </label>
          )}
          {/*
            * Absent, not disabled, when Replace is refused.
            *
            * The house treatment is a disabled control with the reason on it — but the reason
            * is already the entire body of this dialog when `blocked` is set, so a greyed
            * button repeating it would be the same sentence twice with a dead control under it.
            */}
          {replaceable && (
            <button
              type="button"
              className={`${styles.button} ${styles.buttonDanger}`}
              onClick={() => onAnswer('replace', applyToRest)}
              data-audit="pasteConfirmReplace"
            >
              {replaceLabel(collision)}
            </button>
          )}
          <button
            type="button"
            className={styles.button}
            onClick={onCancel}
            data-audit="pasteConfirmCancel"
          >
            Cancel
          </button>
          <button
            ref={keep}
            type="button"
            className={`${styles.button} ${styles.buttonPrimary}`}
            onClick={() => onAnswer('keepBoth', applyToRest)}
            data-audit="pasteConfirmKeepBoth"
          >
            {keepBothLabel(collision)}
          </button>
        </div>

        <p className={styles.note} data-audit="pasteConfirmNote">
          {nothingWrittenYet()}
        </p>
      </div>
    </OverlayCard>
  )
}
