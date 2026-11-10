/**
 * What waits for the user across the Agents panel — the one count the Sessions tab's pill, the
 * Waiting tab's pill and the Agents rail button's amber number are all read from. (M199)
 *
 * Three stores feed it: the session rows (`sessionsStore`, attached by the app for the active
 * project), the requests in *Waiting for you* (`userRequestsStore`) and the board's questions and
 * reviews (`tasksStore`). `sessionsModel.waitingSummary` does the arithmetic, counting a
 * session's question once although it is both a waiting session and a Waiting-tab item.
 *
 * # A derived store rather than a hook over three stores
 *
 * `App.tsx` is the whole shell and draws the rail, and it selects **numbers**, never a board or a
 * roster — a board is a new object on every `tasks-changed`, and selecting one would re-render
 * the shell for every comment a run posts. A hook reading the three stores' objects to add them up
 * would be exactly that. So this module subscribes to the three itself, recomputes on any change,
 * and republishes only when a figure (or the set of questioned conversations) actually moved; a
 * component reads it through `useSyncExternalStore` and re-renders on that alone.
 */
import { useSyncExternalStore } from 'react'

import { useSessions } from '@/sidebar/sessionsStore'
import { useTasks } from '@/sidebar/tasksStore'
import { useUserRequests } from '@/sidebar/userRequestsStore'
import { waitingFor } from '@/sidebar/TasksPanel/model'
import { questionedIds, waitingSummary } from '@/sidebar/AgentsPanel/sessionsModel'
import { waitingDecisionCount } from '@/sidebar/AgentsPanel/waitingModel'

export interface WaitingCounts {
  /** Everything, each question once — the rail's amber number. */
  total: number
  /** Live sessions stopped on the user — the Sessions tab's pill. */
  sessions: number
  /** The Waiting tab's own count. */
  decisions: number
  /** Conversations with an open question, for the rows' "Asked you a question". */
  questioned: ReadonlySet<string>
}

const NONE: ReadonlySet<string> = new Set()
let snapshot: WaitingCounts = { total: 0, sessions: 0, decisions: 0, questioned: NONE }
let questionedKey = ''
const listeners = new Set<() => void>()

function recompute(): void {
  const rows = useSessions.getState().rows ?? []
  const board = useTasks.getState().board
  const requests = useUserRequests.getState().view?.requests ?? []
  const pending = requests.filter((r) => !r.delivered)
  const waiting = waitingFor(board.kind === 'ready' ? board.tasks : [])
  const decisions = waitingDecisionCount(pending, [...waiting.questions, ...waiting.review])
  const questioned = questionedIds(
    rows,
    pending.map((r) => ({ origin: r.origin.id, open: r.response == null })),
    new Set(waiting.questions.map((t) => t.id)),
  )
  const { sessions, total } = waitingSummary(rows, questioned, decisions)
  const key = [...questioned].sort().join('\n')
  if (
    total === snapshot.total &&
    sessions === snapshot.sessions &&
    decisions === snapshot.decisions &&
    key === questionedKey
  )
    return
  questionedKey = key
  snapshot = { total, sessions, decisions, questioned: questioned.size === 0 ? NONE : questioned }
  for (const listener of listeners) listener()
}

let wired = false
/** Subscribe to the three stores once, from the first reader — never at import. */
function wire(): void {
  if (wired) return
  wired = true
  useSessions.subscribe(recompute)
  useTasks.subscribe(recompute)
  useUserRequests.subscribe(recompute)
  recompute()
}

function subscribe(listener: () => void): () => void {
  wire()
  listeners.add(listener)
  return () => listeners.delete(listener)
}

const read = () => snapshot

/** The counts, re-rendering the caller only when one of them moves. */
export function useWaitingCounts(): WaitingCounts {
  return useSyncExternalStore(subscribe, read, read)
}
