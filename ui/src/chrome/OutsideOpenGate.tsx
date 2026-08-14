/**
 * Draws the out-of-project confirmation, wherever the ctrl+click was made.
 *
 * Three lines of component and one reason for existing: it is rendered in **both** branches of
 * `App.tsx` — the shell window and the `pane:<uuid>` window — next to `<Failures/>`, which is
 * the other thing that had to be in both for the same reason. A detached pane's output names
 * paths exactly as a docked one does, and a dialog wired only into the shell tree would leave
 * that ctrl+click asking nobody and opening nothing.
 *
 * It owns no rules. `terminal/outsideOpen.ts` decides whether there is a question and what it
 * says, `chrome/outsideOpenStore.ts` holds it, and `ConfirmDestructive` draws it — Cancel
 * focused and accented, Escape cancelling, every path named in full.
 */
import { ConfirmDestructive } from './ConfirmDestructive'
import { useOutsideOpen } from './outsideOpenStore'

export function OutsideOpenGate() {
  const pending = useOutsideOpen((s) => s.pending)
  if (pending === null) return null
  return (
    <ConfirmDestructive
      state={{
        title: pending.ask.title,
        body: pending.ask.body,
        files: pending.ask.files,
        confirmLabel: pending.ask.confirmLabel,
        mark: pending.ask.mark,
        // `ConfirmDestructive` calls neither of these itself — the caller runs and closes — so
        // the store's `confirm` is the whole of it and this is never invoked.
        run: () => {},
      }}
      onCancel={() => useOutsideOpen.getState().dismiss()}
      onConfirm={() => useOutsideOpen.getState().confirm()}
    />
  )
}
