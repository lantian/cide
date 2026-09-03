/**
 * "Open the Log tab with this filter" — from outside React. (M37)
 *
 * The sibling of `requestLogReveal` in `LogTab.tsx`, and the same design for the same two reasons
 * written out there: the Log tab is usually not mounted when the request is made and sometimes
 * already is, so the request is *parked* and consumed by whichever render sees it first —
 * consumed, not observed, and only by the render that spent it. The caller opens the tool window;
 * parking cannot fail, and a refused `tool_window_activate` is the caller's sentence to show.
 *
 * # Why this is its own module and not beside `requestLogReveal`
 *
 * The producer is `keys/dispatch.ts` — the pull's notice — and `LogTab.tsx` imports
 * `openTagDialog` *from* `keys/dispatch.ts`. A `requestLogFilter` exported from `LogTab.tsx` would
 * close that cycle: harmless for two functions called at runtime, and a trap for the first
 * module-level value either side adds. This file imports nothing of cide's but two types, so it can
 * sit on both sides of that edge.
 */
import { create } from 'zustand'
import type { ProjectId } from '@/ipc/generated'
import type { LogFilter } from './logModel'

/** One outstanding "show me this filter". */
export interface FilterRequest {
  readonly project: ProjectId
  readonly filter: LogFilter
  /** `Date.now()` at the request, for the TTL. */
  readonly at: number
  /** Two requests for the same filter are two gestures — see `RevealRequest.nonce`. */
  readonly nonce: number
}

/**
 * How long a parked request stays live. The same window as `LogTab.tsx`'s `REVEAL_TTL_MS`, for the
 * same reason: it has to cover the tool window opening and the tab mounting, and it has to rule
 * out a request parked for a tab that never opened firing on some unrelated visit later.
 */
export const FILTER_REQUEST_TTL_MS = 30_000

interface FilterRequestStore {
  pending: FilterRequest | null
  request: (next: FilterRequest) => void
  clear: (spent: FilterRequest) => void
}

export const useFilterRequests = create<FilterRequestStore>((set, get) => ({
  pending: null,
  request(next) {
    set({ pending: next })
  },
  clear(spent) {
    // Only the request that was consumed — a blind clear would drop a newer one that arrived
    // between the consumer deciding and calling back.
    if (get().pending !== spent) return
    set({ pending: null })
  },
}))

let filterNonce = 0

export function requestLogFilter(project: ProjectId, filter: LogFilter): void {
  filterNonce += 1
  useFilterRequests.getState().request({ project, filter, at: Date.now(), nonce: filterNonce })
}
