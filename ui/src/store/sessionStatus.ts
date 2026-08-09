/**
 * Live statusline figures, keyed by session.
 *
 * The formatting lives in `statusFormat.ts`, which imports nothing: it is the part with
 * real edge cases (absent fields, a fresh session that has genuinely used zero tokens, a
 * CLI release that renames something) and keeping it dependency-free is what lets it be
 * checked without a bundler or a DOM.
 */
import { create } from 'zustand'
import type { StatusPayload } from './statusFormat'

export * from './statusFormat'

interface StatusStore {

  /** Keyed by session id. A window shows whichever session its focused pane holds. */
  bySession: Record<string, StatusPayload>
  set: (session: string, status: StatusPayload) => void
  clear: (session: string) => void
}

export const useSessionStatus = create<StatusStore>((set) => ({
  bySession: {},
  set: (session, status) =>
    set((s) => ({ bySession: { ...s.bySession, [session]: status } })),
  clear: (session) =>
    set((s) => {
      const next = { ...s.bySession }
      delete next[session]
      return { bySession: next }
    }),
}))
