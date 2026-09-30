/**
 * The Agents panel's Sessions tab: the project's journal as the webview mirrors it, and the
 * search the user is typing. (M134)
 *
 * A mirror, per ADR 0002 — Rust's `sessions_state` owns the journal, and this holds the last
 * `sessions_list` answer for one project. It is re-asked on `cide://sessions-changed` for that
 * project, and on `cide://agents-changed` too: the journal learns a run's changes on the same
 * coalescer flush that sends the roster, but a row's *liveness* is joined in at list time and is
 * not a journal change, so a run ending would otherwise leave its row reading live.
 *
 * The filter lives here rather than in the view for `agentsTabStore`'s reason: the panel is
 * unmounted whenever another sidebar panel shows, and a search typed, left and come back to is
 * a search the user expects to find still typed.
 *
 * # The transcript search
 *
 * Debounced and tokened. Each keystroke with *Search transcripts* on restarts a 300 ms timer; the
 * search that fires carries a token one higher than the last, and Rust stops reading for any
 * older token and answers `cancelled` — which is dropped here, as is any answer for a query or a
 * project the store has since left.
 */
import { create } from 'zustand'

import { agentEvents, sessionJournal, type ProjectId, type SessionRow } from '@/ipc/client'
import { errorText } from '@/ipc/errorText'
import type { SessionKindName, SessionView } from '@/sidebar/AgentsPanel/sessionsModel'

/** The wire row → the view's plain shape. The only place that knows both. */
export function adaptSession(row: SessionRow): SessionView {
  const r = row.record
  return {
    id: r.id,
    kind: r.kind as SessionKindName,
    harness: r.harness,
    title: r.title,
    name: r.name ?? null,
    prompt: r.prompt ?? null,
    task: r.task ?? null,
    taskTitle: r.taskTitle ?? null,
    agent: r.agent ?? null,
    branch: r.branch ?? null,
    cwd: r.cwd,
    startedMs: Number(r.startedUnixMs),
    lastSeenMs: Number(r.lastSeenUnixMs),
    live: row.live,
    pane: row.pane,
    searchable: row.searchable,
  }
}

interface SessionsStore {
  project: ProjectId | null
  /** `null` until the first answer for `project`. */
  rows: SessionView[] | null
  error: string | null
  query: string
  kinds: ReadonlySet<SessionKindName>
  /** The subagent role picked in the role filter, or `null` for all. */
  agent: string | null
  /** *Search transcripts* is on. */
  transcripts: boolean
  /** Conversation id → snippet, for the current query; `null` while off or not yet answered. */
  hits: Readonly<Record<string, string>> | null
  /** A transcript search is in flight. */
  searching: boolean
  /** The last transcript search stopped at its file budget. */
  capped: boolean
  /** Follow `project`: reset for a new one, and re-ask either way. */
  attach: (project: ProjectId | null) => void
  refresh: () => void
  setQuery: (query: string) => void
  toggleKind: (kind: SessionKindName) => void
  clearKinds: () => void
  setAgent: (agent: string | null) => void
  setTranscripts: (on: boolean) => void
}

const NO_KINDS: ReadonlySet<SessionKindName> = new Set()

let token = 0
let searchTimer: ReturnType<typeof setTimeout> | undefined

export const useSessions = create<SessionsStore>((set, get) => {
  /** Start (or restart) the debounced transcript search for the current query. */
  const searchSoon = () => {
    if (searchTimer !== undefined) clearTimeout(searchTimer)
    const { project, query, transcripts } = get()
    if (!transcripts || project === null || query.trim() === '') {
      searchTimer = undefined
      set({ hits: null, searching: false, capped: false })
      return
    }
    set({ searching: true })
    searchTimer = setTimeout(() => {
      searchTimer = undefined
      token += 1
      const mine = token
      void sessionJournal
        .search(project, query, mine)
        .then((answer) => {
          const now = get()
          if (mine !== token || now.project !== project || now.query !== query || !now.transcripts) return
          if (answer.kind === 'cancelled') return
          const hits: Record<string, string> = {}
          for (const hit of answer.hits) hits[hit.id] = hit.snippet
          set({ hits, searching: false, capped: answer.capped })
        })
        .catch((e: unknown) => {
          if (mine === token) set({ searching: false, error: errorText(e) })
        })
    }, 300)
  }

  return {
    project: null,
    rows: null,
    error: null,
    query: '',
    kinds: NO_KINDS,
    agent: null,
    transcripts: false,
    hits: null,
    searching: false,
    capped: false,
    attach: (project) => {
      follow()
      if (project !== get().project) {
        // Another project's search is not this one's: the hits name its conversations.
        // …and its roles are not this one's: a role filter left on from another project would
        // show an empty list with nothing on screen saying why.
        set({ project, rows: null, error: null, hits: null, searching: false, capped: false, agent: null })
        searchSoon()
      }
      get().refresh()
    },
    refresh: () => {
      const project = get().project
      if (project === null) return
      void sessionJournal
        .list(project)
        .then((listing) => {
          // A late answer for a project the store has since left is dropped.
          if (get().project === project) set({ rows: listing.rows.map(adaptSession), error: null })
        })
        .catch((e: unknown) => {
          if (get().project === project) set({ error: errorText(e) })
        })
    },
    setQuery: (query) => {
      set({ query })
      searchSoon()
    },
    toggleKind: (kind) => {
      const next = new Set(get().kinds)
      if (next.has(kind)) next.delete(kind)
      else next.add(kind)
      set({ kinds: next })
    },
    clearKinds: () => set({ kinds: NO_KINDS }),
    setAgent: (agent) => set({ agent }),
    setTranscripts: (on) => {
      set({ transcripts: on })
      searchSoon()
    },
  }
})

let refreshTimer: ReturnType<typeof setTimeout> | undefined
/** Coalesced: a dispatch can move the journal and the roster within one frame. */
function refreshSoon(): void {
  if (refreshTimer !== undefined) clearTimeout(refreshTimer)
  refreshTimer = setTimeout(() => {
    refreshTimer = undefined
    useSessions.getState().refresh()
  }, 200)
}

let wired = false
/**
 * Subscribe once, from the first `attach`: a store that subscribed at import time would do so in
 * every realm that merely imports a type from here — `followMilestones`' rule.
 */
function follow(): void {
  if (wired) return
  wired = true
  void sessionJournal.onChanged((project) => {
    if (project === useSessions.getState().project) refreshSoon()
  })
  void agentEvents.onChanged((project) => {
    if (project === useSessions.getState().project) refreshSoon()
  })
}
