/**
 * The Sessions tab's pure core: what a row is called and doing, which rows a search keeps, how
 * long ago, and how many sessions wait for the user. (M134, reworked M199)
 *
 * **Import-free**, `model.ts`'s rule and for its reason: `check-sessions.mjs` compiles this file
 * (and `sessionsTree.ts`, which imports only types from here) with a bare `tsc` and imports the
 * result under node. So the wire's `SessionRow` is restated structurally as `SessionView`
 * (`sessionsStore.ts` adapts it), and the kinds and activities are frozen lists the check pins
 * against `cide_ipc::sessions` — a variant arriving from Rust with no label fails there, not on
 * screen as a blank glyph.
 *
 * # The two searches, and why they are OR'd
 *
 * The text box always searches what the journal holds — cide's label, the `/rename` name, title,
 * first prompt, task, role, branch, directory, kind — on every keystroke, here. *Search
 * transcripts* adds what the conversations *said*, which only Rust can read and which arrives
 * later as `hits`. A row is kept when either finds it: turning transcripts on must only ever add
 * rows, or the toggle would read as a different search rather than a deeper one.
 *
 * The kind chips and the role select went in M199. "Only the subagents", "only the developer's
 * runs" are still one word in the box — the kind's label and the role are in the haystack — and
 * two controls the user had to clear before a search found anything were the price of keeping
 * them.
 *
 * # What a row is doing, and the one thing laid over Rust's answer
 *
 * `activity` is Rust's (`cide_core::sessions::activity`: one table over run states and hook
 * states). The panel adds exactly one fact Rust's listing would hold stale: **a question**. A
 * conversation that asked the user something — a request in *Waiting for you* naming it, or the
 * board question on its run's task — reads *Asked you a question* while it is live, and the
 * answer clears it the moment the request store moves, not at the next re-list.
 */

/** `cide_ipc::sessions::SessionKind`, camelCased. */
export const SESSION_KINDS = [
  'console',
  'consoleSplit',
  'tab',
  'worker',
  'planner',
  'reviewer',
  'subagent',
  'mrReview',
  'openSpec',
] as const

export type SessionKindName = (typeof SESSION_KINDS)[number]

/** `cide_ipc::sessions::SessionActivity`, camelCased — Rust's answer, before the question. */
export const SESSION_ACTIVITIES = [
  'starting',
  'working',
  'idle',
  'awaitingInput',
  'permission',
  'paused',
  'blocked',
  'failed',
  'finished',
  'stopped',
] as const

export type SessionActivityName = (typeof SESSION_ACTIVITIES)[number]

/** What a row's glyph says: Rust's activity, or a question laid over it. */
export type SessionStatus = SessionActivityName | 'question'

const KIND_LABEL: Readonly<Record<SessionKindName, string>> = {
  console: 'Console',
  consoleSplit: 'Console split',
  tab: 'Tab',
  worker: 'Worker tab',
  planner: 'Planner',
  reviewer: 'Reviewer',
  subagent: 'Subagent',
  mrReview: 'MR review',
  openSpec: 'OpenSpec',
}

/**
 * A kind's label. `Object.hasOwn`, never a bare index: a kind this build does not know — or
 * `'constructor'` — must read as itself, not as `Object.prototype.constructor` stringified into a
 * row (`check-agents.mjs`' `ROGUE` lesson).
 */
export function kindLabel(kind: string): string {
  return Object.hasOwn(KIND_LABEL, kind) ? KIND_LABEL[kind as SessionKindName] : kind
}

const HARNESS_LABEL: Readonly<Record<string, string>> = {
  claude: 'Claude',
  opencode: 'opencode',
  qwen: 'Qwen',
  codex: 'Codex',
  mimo: 'MiMo',
}

export function harnessLabel(harness: string): string {
  return Object.hasOwn(HARNESS_LABEL, harness) ? (HARNESS_LABEL[harness] ?? harness) : harness
}

/** The Subagents tab's tones (`model.ts`' `Tone`); a row's glyph takes the class of the same name. */
export type StatusTone = 'idle' | 'busy' | 'attention' | 'paused' | 'done' | 'error'

export interface StatusLook {
  /** A vendored icon name. */
  glyph: string
  tone: StatusTone
  label: string
}

/**
 * Activity → glyph, tone and words. **The same glyph and tone as the run phase it corresponds
 * to** in `model.ts`'s `PHASE_GLYPH`/`PHASE_TONE` — `check-sessions.mjs` pins each pair — so a run
 * reads the same on the Sessions tab as on the Subagents tab. The words may differ: "Working"
 * says what a console is doing better than "Running", which is what a run is.
 *
 * Every *attention* status is something the user owes, and only those are counted as waiting.
 */
const STATUS: Readonly<Record<SessionStatus, StatusLook>> = {
  starting: { glyph: 'circle-dashed', tone: 'busy', label: 'Starting' },
  working: { glyph: 'loader-circle', tone: 'busy', label: 'Working' },
  idle: { glyph: 'circle-dot', tone: 'idle', label: 'Idle' },
  awaitingInput: { glyph: 'circle-alert', tone: 'attention', label: 'Waiting for your input' },
  question: { glyph: 'circle-alert', tone: 'attention', label: 'Asked you a question' },
  permission: { glyph: 'circle-alert', tone: 'attention', label: 'Wants a permission' },
  paused: { glyph: 'circle-pause', tone: 'paused', label: 'Paused' },
  blocked: { glyph: 'circle-alert', tone: 'error', label: 'Blocked' },
  failed: { glyph: 'circle-x', tone: 'error', label: 'Failed' },
  finished: { glyph: 'circle-check', tone: 'done', label: 'Finished' },
  stopped: { glyph: 'circle-minus', tone: 'paused', label: 'Stopped' },
}

/** The glyph a status this build does not know draws — `model.ts`' unknown phase. */
const UNKNOWN_STATUS: StatusLook = { glyph: 'circle-slash', tone: 'idle', label: 'Unknown' }

export function statusLook(status: string): StatusLook {
  return Object.hasOwn(STATUS, status) ? STATUS[status as SessionStatus] : UNKNOWN_STATUS
}

/** Whether a status is one the user owes something on — the waiting counts. */
export function isWaiting(status: string): boolean {
  return statusLook(status).tone === 'attention'
}

/** One row, as the view draws it. */
export interface SessionView {
  planningPolicy?: 'required' | 'skip' | null
  planningPhase?: 'planning' | 'approving' | 'implementing' | null
  /** The conversation id — the row's key, and what Open, a group and the search name it by. */
  id: string
  kind: SessionKindName
  harness: string
  title: string
  /** The `/rename` name. */
  name: string | null
  prompt: string | null
  task: string | null
  taskTitle: string | null
  agent: string | null
  branch: string | null
  cwd: string
  startedMs: number
  lastSeenMs: number
  /** A child is alive for it now. */
  live: boolean
  /** That child's handle — the key `cide://session-state` is joined on. */
  liveSession: string | null
  /** The run it is, when it is a run's. */
  run: string | null
  /** A pane of the project shows it: Open reveals instead of resuming. */
  pane: string | null
  /** Its transcript can be searched (claude, qwen, codex). */
  searchable: boolean
  activity: SessionActivityName
  /** The group it is filed in. */
  group: string | null
  /** cide's own name for it. */
  label: string | null
}

/** A user-made folder. `cide_ipc::sessions::SessionGroup`. */
export interface GroupView {
  id: string
  name: string
  parent: string | null
  collapsed: boolean
}

/** Live with no pane: a run, a tab closed with its child kept, any background harness. */
export function isBackground(v: SessionView): boolean {
  return v.live && v.pane === null
}

/**
 * A title that is only "project : harness" — what an unrenamed console is called, and what made
 * a list of fifty of them unreadable. The first prompt says more.
 */
const GENERIC_TITLE = / : (claude|codex|opencode|qwen|mimo)$/i

/**
 * What a row is called: cide's label, the `/rename` name, the title unless it is generic, the
 * first prompt, the generic title, and the id when there is nothing else.
 */
export function displayName(v: SessionView): string {
  for (const s of [v.label, v.name]) if (s !== null && s.trim() !== '') return s
  const title = v.title.trim()
  if (title !== '' && !GENERIC_TITLE.test(title)) return title
  if (v.prompt !== null && v.prompt.trim() !== '') return v.prompt
  return title !== '' ? title : v.id
}

/** Whether [`displayName`] had nothing better than a generic title — drawn dimmer. */
export function hasOnlyGenericName(v: SessionView): boolean {
  return displayName(v) === v.title.trim() && GENERIC_TITLE.test(v.title.trim())
}

/** Every text the journal holds about a row, lowercased and joined, for the text search. */
function haystack(v: SessionView): string {
  return [
    v.label,
    v.name,
    v.title,
    v.prompt,
    v.task,
    v.taskTitle,
    v.agent,
    v.branch,
    v.cwd,
    v.id,
    v.harness,
    kindLabel(v.kind),
  ]
    .filter((s): s is string => s !== null)
    .join('\n')
    .toLowerCase()
}

/** The search's words: whitespace-separated, lowercased. Empty means "not searching". */
export function searchWords(query: string): string[] {
  return query.toLowerCase().split(/\s+/).filter((w) => w !== '')
}

/**
 * Whether the journal's text matches `query`: every word, anywhere. Words and not the phrase,
 * because the fields are separate — "developer t-14" should find the developer's run on t-14,
 * whose role and task are two fields.
 */
export function matchesText(v: SessionView, query: string): boolean {
  const words = searchWords(query)
  if (words.length === 0) return true
  const text = haystack(v)
  return words.every((w) => text.includes(w))
}

/** Whether the search keeps a row: its text, or a transcript hit. See the header. */
export function keeps(
  v: SessionView,
  query: string,
  hits: Readonly<Record<string, string>> | null,
): boolean {
  return matchesText(v, query) || (hits !== null && Object.hasOwn(hits, v.id))
}

/** A pending request, as far as the waiting counts need it. */
export interface PendingAsk {
  /** `UserRequest.origin.id`: the conversation that asked. */
  origin: string
  /** The user has not answered yet. */
  open: boolean
}

/**
 * The conversations that have asked the user something still unanswered: an open request whose
 * origin they are, or — for a run — an open board question on its task.
 */
export function questionedIds(
  views: readonly SessionView[],
  asks: readonly PendingAsk[],
  questionTasks: ReadonlySet<string>,
): Set<string> {
  const ids = new Set(asks.filter((a) => a.open).map((a) => a.origin))
  for (const v of views) if (v.task !== null && questionTasks.has(v.task)) ids.add(v.id)
  return ids
}

/** What a row's glyph says: a question while it is live and has one open, else Rust's activity. */
export function rowStatus(v: SessionView, questioned: ReadonlySet<string>): SessionStatus {
  return v.live && questioned.has(v.id) ? 'question' : v.activity
}

/**
 * The two waiting figures. `sessions` is the Sessions tab's pill: live sessions stopped on the
 * user. `total` is the Agents rail button's: those plus `waitingTotal` (the Waiting tab's own
 * count), with a session's question **counted once** — it is both a waiting session and a
 * Waiting-tab item, and a user who answers one thing should see the number drop by one.
 */
export function waitingSummary(
  views: readonly SessionView[],
  questioned: ReadonlySet<string>,
  waitingTotal: number,
): { sessions: number; total: number } {
  let sessions = 0
  let both = 0
  for (const v of views) {
    if (!isWaiting(rowStatus(v, questioned))) continue
    sessions += 1
    if (v.live && questioned.has(v.id)) both += 1
  }
  return { sessions, total: Math.max(0, waitingTotal + sessions - both) }
}

const MINUTE = 60_000
const HOUR = 60 * MINUTE
const DAY = 24 * HOUR

/**
 * When a row was last seen, relative to `nowMs`, in a row's few pixels: `now`, `4m`, `3h`, `2d`,
 * then the date. Coarse on purpose — a row is found by roughly when.
 */
export function ago(nowMs: number, thenMs: number): string {
  const d = Math.max(0, nowMs - thenMs)
  if (d < MINUTE) return 'now'
  if (d < HOUR) return `${Math.floor(d / MINUTE)}m`
  if (d < DAY) return `${Math.floor(d / HOUR)}h`
  if (d < 30 * DAY) return `${Math.floor(d / DAY)}d`
  return new Date(thenMs).toISOString().slice(0, 10)
}

export type Day = 'today' | 'yesterday' | 'earlier'

/** Which of *Closed*'s captions a row goes under: calendar days in local time, not 24h windows. */
export function dayOf(nowMs: number, thenMs: number): Day {
  const midnight = new Date(nowMs)
  midnight.setHours(0, 0, 0, 0)
  const today = midnight.getTime()
  if (thenMs >= today) return 'today'
  const yesterday = new Date(today)
  yesterday.setDate(yesterday.getDate() - 1)
  return thenMs >= yesterday.getTime() ? 'yesterday' : 'earlier'
}

export const DAY_LABEL: Readonly<Record<Day, string>> = {
  today: 'Today',
  yesterday: 'Yesterday',
  earlier: 'Earlier',
}
