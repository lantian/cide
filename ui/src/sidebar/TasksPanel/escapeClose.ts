/**
 * Escape closes a Tasks-panel modal (the milestone, checks, proposal and check-log cards).
 *
 * `OverlayCard` has no Escape of its own, on purpose: its callers disagree about what Escape
 * means (Settings → Agents asks before it discards a dirty draft, and never discards on Escape),
 * so each card answers it itself. These cards all mean the same thing by it — whatever the scrim
 * click does, which is nothing while a write is in flight — so they share this.
 *
 * On the card rather than on `document`, as `OpenSpecPanel/ConfigDialog` does: a window listener
 * would also answer for the terminal underneath and for any other overlay open. And the card
 * takes focus when it opens, because it is portalled to `document.body` — with focus still on the
 * sidebar row that opened it, the keydown would never reach the handler, and Escape would only
 * work once the user had clicked into a field first.
 */
import { useEffect, useRef, type KeyboardEvent } from 'react'

export function useEscapeClose(dismiss: () => void) {
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    // Only when nothing inside already has focus — a card that focuses a field itself keeps it.
    const el = ref.current
    if (el !== null && !el.contains(document.activeElement)) el.focus()
  }, [])
  return {
    ref,
    tabIndex: -1,
    onKeyDown: (ev: KeyboardEvent) => {
      if (ev.key !== 'Escape') return
      // Stopped, so the window's own Escape handling does not also act on it.
      ev.stopPropagation()
      dismiss()
    },
  }
}
