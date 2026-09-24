/**
 * The confirmation shown before a close that would destroy something.
 *
 * It exists because closing a file tab with unsaved edits used to discard them in silence.
 * Rust is the guard — `close_tab` and `close_project` refuse without an explicit `force`,
 * and answer with `CoreError::UnsavedChanges` naming every file at stake — and this is the
 * thing that turns that refusal into a decision the user can make.
 *
 * Three rules shape it, and each one is a way this kind of dialog usually fails:
 *
 * 1. **It names what would be lost.** Not "3 items": `main.rs`, and the directory it is in,
 *    because two open `mod.rs` are the ordinary case in a Rust tree. The data comes from
 *    Rust with the paths already in it, so there is nothing to look up and nothing to be
 *    stale about.
 * 2. **It does not appear when nothing is at risk.** `atRisk` decides, and the caller is
 *    expected to consult it (or simply to mount this only on a refusal). A dialog that
 *    shows up on every close is one users learn to dismiss unread.
 * 3. **Cancel is the default.** Escape cancels, the initial focus is on Cancel, and Cancel is
 *    the kit's primary. The destructive button is the kit's black `danger`: the accent-filled
 *    button is the safe one, and the loss never wears the accent.
 *
 * NOT mounted here — see the note at the bottom of this file for where it goes.
 */
import { useEffect, useRef } from 'react'
import { Modal } from '@/overlays/ModalShell'
import { Button } from '@/kit/components/Button'
import { Dialog } from '@/kit/components/Overlay'
import { Dot } from '@/kit/components/Status'
import { PathGroup, PathList, PathRow } from '@/kit/components/Surface'
import type { SessionSummary, UnsavedTab } from '@/ipc/client'
import {
  atRisk,
  confirmBody,
  confirmLabel,
  confirmTitle,
  sessionRow,
  unsavedRow,
  type CloseScope,
} from './closeConfirmModel'

export interface CloseConfirmProps {
  /** What the user asked to close. Only the wording depends on it. */
  scope: CloseScope
  /** The dirty file tabs. From `CoreError::UnsavedChanges`, or `QuitDecision.unsaved`. */
  unsaved: UnsavedTab[]
  /** Claude sessions mid-turn. From `QuitDecision.blocking`; empty for a tab close. */
  sessions: SessionSummary[]
  /** Go back. Nothing closes, nothing is written. */
  onCancel: () => void
  /** Proceed and lose the above. The caller re-issues its command with `force: true`. */
  onDiscard: () => void
  /**
   * Write the unsaved buffers, then close. Omit when the caller cannot offer it.
   *
   * Optional because saving is not this component's to do: the text lives in the editor's
   * CodeMirror state, in this window, and only the mounting code knows how to reach it.
   * A dialog that offered a Save button it could not honour would be worse than one that
   * does not offer it.
   */
  onSaveAll?: (() => void) | undefined
}

export function CloseConfirm({
  scope,
  unsaved,
  sessions,
  onCancel,
  onDiscard,
  onSaveAll,
}: CloseConfirmProps) {
  const cancel = useRef<HTMLButtonElement>(null)

  // Focus the safe button, not the destructive one. A dialog that opens with Discard
  // focused turns a reflexive Enter — the keystroke that was in flight when it appeared —
  // into the data loss it was put there to prevent.
  useEffect(() => {
    cancel.current?.focus()
  }, [])

  // Escape cancels, from anywhere in the dialog. Captured on the card rather than on the
  // window: a keydown listener on `document` would fire for the terminal underneath too,
  // and two overlays open at once would both answer it.
  const onKeyDown = (ev: React.KeyboardEvent) => {
    if (ev.key === 'Escape') {
      ev.stopPropagation()
      onCancel()
    }
  }

  const risk = { unsaved, sessions }
  const title = confirmTitle(scope, risk)

  // The prominent button is Cancel and holds the focus; the loss is the kit's black `danger`.
  // `Save all` is the recovery and sits in the footer's left slot, away from the two close
  // actions, so a user reaching for it cannot hit Discard by being 4px off.
  return (
    <Modal onDismiss={onCancel}>
      {/* The handler sits on the dialog rather than on `document`: focus is already in here
          (the effect above put it on Cancel), and a document listener would also answer for
          the terminal underneath and for any other overlay that is open. */}
      <Dialog
        title={title}
        lead={confirmBody(scope, risk)}
        width="narrow"
        flush
        onKeyDown={onKeyDown}
        data-audit="closeConfirm"
        footNote={
          onSaveAll !== undefined &&
          unsaved.length > 0 && (
            <Button onClick={onSaveAll} data-audit="closeConfirmSave">
              Save all and close
            </Button>
          )
        }
        actions={
          <>
            <Button variant="danger" onClick={onDiscard} data-audit="closeConfirmDiscard">
              {confirmLabel(risk)}
            </Button>
            <Button ref={cancel} variant="primary" onClick={onCancel} data-audit="closeConfirmCancel">
              Cancel
            </Button>
          </>
        }
      >
        <PathList data-audit="closeConfirmList">
          {unsaved.length > 0 && sessions.length > 0 && <PathGroup>Unsaved files</PathGroup>}
          {unsaved.map((tab) => {
            const row = unsavedRow(tab)
            // The accent dot is the tab strip's dirty mark, so the row and the tab match.
            return (
              <PathRow
                key={tab.tab}
                mark={<Dot tone="accent" label="Unsaved" />}
                name={row.name}
                where={row.where}
                full={tab.path}
              />
            )
          })}

          {unsaved.length > 0 && sessions.length > 0 && <PathGroup>Sessions mid-turn</PathGroup>}
          {sessions.map((session) => {
            const row = sessionRow(session)
            // Amber: a session mid-turn is *waiting* on something, not lost yet.
            return (
              <PathRow
                key={session.session}
                mark={<Dot tone="yellow" label="Mid-turn" />}
                name={row.name}
                where={row.where}
              />
            )
          })}
        </PathList>
      </Dialog>
    </Modal>
  )
}

export { atRisk }
export type { CloseScope }

/*
 * How to mount this.
 *
 * It is deliberately not mounted in `App.tsx` here — that file is being edited in parallel.
 * The intended wiring, in one place:
 *
 *   const pending = useCloseConfirm((s) => s.pending)
 *   ...
 *   {pending && (
 *     <CloseConfirm
 *       scope={pending.scope}
 *       unsaved={pending.unsaved}
 *       sessions={pending.sessions}
 *       onCancel={() => useCloseConfirm.getState().dismiss()}
 *       onDiscard={() => void useCloseConfirm.getState().confirm()}
 *     />
 *   )}
 *
 * beside the existing `<OverlayHost />`, inside the themed subtree. `useCloseConfirm` is
 * `chrome/closeConfirmStore.ts`: the workspace store's `closeTab` / `closeProject` already
 * catch `unsavedChanges` and park the refusal there, so no call site needs to change and
 * `confirm()` is what re-issues the command with `force: true`.
 */
