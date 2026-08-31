/**
 * The log line whose whole event is on screen — one at a time, in whichever window asked.
 *
 * A store rather than local state for `outsideOpenStore.ts`'s reasons, both of which apply
 * unchanged: the gesture starts in xterm's `linkHandler`, which lives outside React entirely,
 * and **both window kinds have to draw the answer** — a detached pane renders log lines exactly
 * as a docked one does, and a card wired only into the shell tree would leave that click doing
 * nothing at all.
 *
 * Not part of `store/workspace.ts`: that mirrors Rust-owned durable state, and which line
 * somebody is reading is neither durable nor Rust's.
 *
 * Unlike the outside-open gate this one **replaces** rather than drops. That gate is a question
 * whose whole job is *read this path before answering*, so swapping it under the reader would be
 * the one unforgivable thing; this is a viewer, and clicking a second line while the first is
 * open means "show me that one instead" in every application that has ever had one.
 */
import { create } from 'zustand'
import type { LogLineDetail } from '@/ipc/client'

export interface PendingLogDetail {
  /** Null while the lookup is in flight, so the card can open immediately and fill in. */
  detail: LogLineDetail | null
  /** True once the answer came back empty: the line has aged out of the session's ring. */
  gone: boolean
}

interface LogDetailStore {
  pending: PendingLogDetail | null
  open: () => void
  fill: (detail: LogLineDetail | null) => void
  dismiss: () => void
}

export const useLogDetail = create<LogDetailStore>((set) => ({
  pending: null,
  open: () => set({ pending: { detail: null, gone: false } }),
  // Guarded on the card still being open: a dismiss that lands while the lookup is in flight
  // must not be undone by the answer arriving a moment later.
  fill: (detail) =>
    set((state) =>
      state.pending === null ? state : { pending: { detail, gone: detail === null } },
    ),
  dismiss: () => set({ pending: null }),
}))

/** Open the card, from outside React. `terminal/xterm.ts`'s link handler is the only caller. */
export function showLogDetail(load: () => Promise<LogLineDetail | null>): void {
  useLogDetail.getState().open()
  void load()
    .then((detail) => useLogDetail.getState().fill(detail))
    // A rejected lookup is indistinguishable from an evicted line as far as the reader is
    // concerned, and the card says the same thing for both. The alternative — a notice on top
    // of an open card — is two pieces of chrome for one failure.
    .catch(() => useLogDetail.getState().fill(null))
}
