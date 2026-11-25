/**
 * The Agents panel's Sessions tab: the project's journal as the webview mirrors it, the search
 * the user is typing, and which row the tree has selected. (M134, reworked M199)
 *
 * A mirror, per ADR 0002 — Rust's `sessions_state` owns the journal and the groups, and this
 * holds the last `sessions_list` answer for one project. It is re-asked:
 *
 * - on `cide://sessions-changed` for that project — a journal write, a grouping edit in another
 *   window, or (M199) a pane opened onto or closed off one of its sessions;
 * - on `cide://agents-changed` — a run's state is joined in at list time and is not a journal
 *   change, so a run ending would otherwise leave its row reading live;
 * - on `cide://session-state` (M199) for a session one of its rows is live in, **when that
 *   session's state actually moved** — the row's glyph is that state, and a turn ending or a child
 *   exiting has no other road to the tab.
 *
 * # Attached by the app, not by the panel (M199)
 *
 * The Agents rail button's yellow count and the Sessions tab's pill both read these rows, and the
 * rail is on screen while the panel is shut. So `App.tsx` attaches this for the active project,
 * the way it attaches `userRequestsStore`; until M199 only the visible tab did.
 *
 * The filter and the selection live here rather than in the view for `agentsTabStore`'s reason:
 * the panel is unmounted whenever another sidebar panel shows, and a search typed, left and come
 * back to is a search the user expects to find still typed.
 *
 * # The transcript search
 *
 * Debounced and tokened. Each keystroke with *Search transcripts* on restarts a 300 ms timer; the
 * search that fires carries a token one higher than the last, and Rust stops reading for any
 * older token and answers `cancelled` — which is dropped here, as is any answer for a query or a
 * project the store has since left.
 *
 * # "Seen" is wired here, not in the panel (M205)
 *
 * A finished turn the user has dealt with reads `idle` (`sessionsSeen.ts`), and both roads to
 * that are subscribed in `follow`, from the app's first `attach`: the pane table's changes
 * (`awaiting.ts::subscribeAwaiting`, re-laid over the rows without a re-list) and a click or a
 * keystroke in a pane (`onAcknowledged`). Until M205 the second lived in `AgentsPanelHost`,
 * which is unmounted whenever another sidebar panel shows — while `App.tsx` keeps these rows
 * attached precisely so the rail can count them with the panel shut. A click into a finished
 * pane then cleared its marker and left its row, and the rail's number, saying *waiting*.
 *
 * The seen marks are kept per project, so switching to another project and back no longer raises
 * every turn the user had already read; and a listing answer that a newer one has overtaken is
 * dropped — two `sessions_list` calls run side by side on Rust's blocking pool, and the slower,
 * older one used to land last and set a row back to a state its session had left.
 */
import { create } from 'zustand'

import {
  agentEvents,
  events,
  sessionJournal,
  type ProjectId,
  type SessionListing,
  type SessionRow,
} from '@/ipc/client'
import type { SessionEdit } from '@/ipc/generated'
import { errorText } from '@/ipc/errorText'
import { applySeen, canMarkSeen } from '@/sidebar/AgentsPanel/sessionsSeen'
import { isSettled, onAcknowledged, subscribeAwaiting } from '@/panes/awaiting'
import type {
  GroupView,
  SessionActivityName,
  SessionKindName,
  SessionView,
} from '@/sidebar/AgentsPanel/sessionsModel'

/** The wire row → the view's plain shape. The only place that knows both. */
export function adaptSession(row: SessionRow): SessionView {
  const r = row.record
  return {
    planningPolicy: r.planningPolicy,
    planningPhase: r.planningPhase,
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
    liveSession: row.liveSession,
    run: r.run ?? null,
    pane: row.pane,
    searchable: row.searchable,
    activity: row.activity as SessionActivityName,
    group: row.group,
    label: row.label,
  }
}

export type SessionsSub = 'opened' | 'closed'

interface SessionsStore {
  project: ProjectId | null
  /** `null` until the first answer for `project`. */
  rows: SessionView[] | null
  groups: readonly GroupView[]
  error: string | null
  query: string
  /** *Search transcripts* is on. */
  transcripts: boolean
  /** Conversation id → snippet, for the current query; `null` while off or not yet answered. */
  hits: Readonly<Record<string, string>> | null
  /** A transcript search is in flight. */
  searching: boolean
  /** The last transcript search stopped at its file budget. */
  capped: boolean
  /** Which of the two lists shows. Kept across a project switch: it is how the user reads. */
  sub: SessionsSub
  /** The selected row — a conversation or a group id — or `null`. */
  selected: string | null
  /** The row being renamed in place, or `null`. */
  renaming: string | null
  /**
   * The user has looked at this conversation's finished turn: read it as idle instead of
   * "Waiting for your input" (`sessionsSeen.ts`). A no-op for anything that is not waiting on
   * nothing but being read.
   */
  markSeen: (id: string) => void
  /** Follow `project`: reset for a new one, and re-ask either way. */
  attach: (project: ProjectId | null) => void
  refresh: () => void
  setQuery: (query: string) => void
  setTranscripts: (on: boolean) => void
  setSub: (sub: SessionsSub) => void
  /** Not `select`: `check:task-links` pins that the Agents host never calls a `.select(` —
   *  the board's task selection has one road, and a lookalike name here would hide a second. */
  selectRow: (id: string | null) => void
  setRenaming: (id: string | null) => void
  /**
   * Send one grouping edit and adopt the listing it answers. Rejects with Rust's refusal, which
   * the caller puts on screen — a gesture that silently did nothing is this panel's worst failure.
   */
  organize: (edit: SessionEdit) => Promise<void>
}

let token = 0
let searchTimer: ReturnType<typeof setTimeout> | undefined
/*
 * Above `create`, and it has to be: zustand runs the initializer below the moment the store is
 * created, at module load, so a `const` it reads that is declared further down is still in its
 * temporal dead zone — a ReferenceError at import, and the whole shell (which imports this store)
 * renders nothing. It shipped that way once in M199.
 */
const NO_GROUPS: readonly GroupView[] = []
/** The rows as Rust answered, before "seen" is laid over them (`sessionsSeen.ts`). */
let rawRows: SessionView[] = []
/**
 * Each project's seen marks (M205). Kept across `attach` — one set reset on every project switch
 * re-raised every finished turn the user had read — and each pruned only against its own
 * project's listing, which is the one list its ids can be in.
 */
const seenByProject = new Map<ProjectId, ReadonlySet<string>>()
const NO_SEEN: ReadonlySet<string> = new Set()
/** Listing order (M205): each ask takes the next number, and an answer older than one adopted is dropped. */
let listAsked = 0
let listAdopted = 0

/**
 * The rows to draw for `project`: the raw rows with seen, and settled in the pane table, laid
 * over them. Stores the pruned set as a side effect, so a mark whose turn has moved on is gone
 * before the next turn can end.
 */
function overlay(project: ProjectId): SessionView[] {
  const before = seenByProject.get(project) ?? NO_SEEN
  const shown = applySeen(rawRows, before, isSettled)
  if (shown.seen !== before) {
    if (shown.seen.size === 0) seenByProject.delete(project)
    else seenByProject.set(project, shown.seen)
  }
  return shown.rows
}

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

  const adopt = (project: ProjectId, listing: SessionListing, asked: number) => {
    // A late answer for a project the store has since left is dropped — and so is one a newer
    // listing has already overtaken (see the header).
    if (get().project !== project || asked < listAdopted) return
    listAdopted = asked
    rawRows = listing.rows.map(adaptSession)
    remember(rawRows)
    set({ rows: overlay(project), groups: listing.groups, error: null })
  }

  return {
    project: null,
    rows: null,
    groups: NO_GROUPS,
    error: null,
    query: '',
    transcripts: false,
    hits: null,
    searching: false,
    capped: false,
    sub: 'opened',
    selected: null,
    renaming: null,
    attach: (project) => {
      follow()
      if (project !== get().project) {
        // Another project's search hits, selection and rename name its conversations, not these.
        // Its seen marks stay in `seenByProject`, for when the user comes back (M205).
        rawRows = []
        set({
          project,
          rows: null,
          groups: NO_GROUPS,
          error: null,
          hits: null,
          searching: false,
          capped: false,
          selected: null,
          renaming: null,
        })
        searchSoon()
      }
      get().refresh()
    },
    refresh: () => {
      const project = get().project
      if (project === null) return
      listAsked += 1
      const asked = listAsked
      void sessionJournal
        .list(project)
        .then((listing) => adopt(project, listing, asked))
        .catch((e: unknown) => {
          if (get().project === project) set({ error: errorText(e) })
        })
    },
    setQuery: (query) => {
      set({ query })
      searchSoon()
    },
    setTranscripts: (on) => {
      set({ transcripts: on })
      searchSoon()
    },
    setSub: (sub) => set({ sub, renaming: null }),
    selectRow: (id) => set({ selected: id }),
    markSeen: (id) => {
      const project = get().project
      if (project === null || !canMarkSeen(rawRows, id)) return
      // Already marked: nothing to draw. `acknowledge` runs on every keystroke into a terminal,
      // and typing the next prompt into a finished pane would otherwise publish fresh rows per key.
      if (seenByProject.get(project)?.has(id) === true) return
      const next = new Set(seenByProject.get(project) ?? NO_SEEN)
      next.add(id)
      seenByProject.set(project, next)
      set({ rows: overlay(project) })
    },
    setRenaming: (id) => set({ renaming: id }),
    organize: (edit) => {
      const project = get().project
      if (project === null) return Promise.resolve()
      listAsked += 1
      const asked = listAsked
      // An answer overtaken by a newer listing is dropped like any other; the edit is still on
      // screen, because `sessions_organize` announces `sessions-changed` to every window,
      // this one included, and that re-list is newer still.
      return sessionJournal.organize(project, edit).then((listing) => adopt(project, listing, asked))
    },
  }
})

/*
 * Each live row's handle and the state it was last drawn with, so a `cide://session-state` frame
 * re-lists only for this project's sessions and only when the state moved. The hook server
 * reports a session `busy` on every tool call of a turn; re-listing on each would be a listing per
 * tool call for nothing the row draws.
 */
const liveHandles = new Set<string>()
const lastStates = new Map<string, string>()
function remember(rows: readonly SessionView[]): void {
  liveHandles.clear()
  for (const r of rows) if (r.liveSession !== null) liveHandles.add(r.liveSession)
}

let refreshTimer: ReturnType<typeof setTimeout> | undefined
/**
 * Coalesced — a dispatch can move the journal and the roster within one frame — and a **throttle,
 * not a debounce**: it re-lists 200 ms after the first cue and ignores cues until then. A
 * debounce restarted by every cue would never fire through a busy minute of roster and session
 * traffic, and the rows would sit on the state they had when the traffic began.
 */
function refreshSoon(): void {
  if (refreshTimer !== undefined) return
  refreshTimer = setTimeout(() => {
    refreshTimer = undefined
    useSessions.getState().refresh()
  }, 200)
}

/**
 * Re-lay the overlay after the pane table moved, without a re-list (M205). Published only when a
 * row's drawn activity actually changed: the table notifies on every transition of every session
 * in the app, and a fresh `rows` array re-renders the panel and recounts the rail.
 */
function reshow(): void {
  const { project, rows } = useSessions.getState()
  if (project === null || rows === null) return
  const next = overlay(project)
  if (next.length === rows.length && next.every((r, i) => r.id === rows[i]?.id && r.activity === rows[i]?.activity)) return
  useSessions.setState({ rows: next })
}

let wired = false
/**
 * Subscribe once, from the first `attach`: a store that subscribed at import time would do so in
 * every realm that merely imports a type from here — `followMilestones`' rule.
 */
function follow(): void {
  if (wired) return
  wired = true
  // "Seen" (M205, the header): the pane table's answer, and a click or keystroke in a pane —
  // both here, app-wide, never in the panel. The listener marks every row the child is live in;
  // `markSeen` refuses any that is not waiting on nothing but being read. It is still needed
  // beside the table: `acknowledge` returns early for a session the table never raised, and
  // the row's waiting is Rust's hook state, which the table may not have heard.
  subscribeAwaiting(reshow)
  onAcknowledged((handle) => {
    const store = useSessions.getState()
    for (const r of rawRows) if (r.liveSession === handle) store.markSeen(r.id)
  })
  void sessionJournal.onChanged((project) => {
    if (project === useSessions.getState().project) refreshSoon()
  })
  void agentEvents.onChanged((project) => {
    if (project === useSessions.getState().project) refreshSoon()
  })
  void events.onSessionState((session, state) => {
    const key = state.state === 'exited' ? `exited:${state.code}` : state.state
    if (lastStates.get(session) === key) return
    lastStates.set(session, key)
    if (state.state === 'exited') lastStates.delete(session)
    if (liveHandles.has(session)) refreshSoon()
  })
}
