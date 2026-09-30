/**
 * The Sessions tab's pure core: what a row is called, which rows a filter keeps, how long ago.
 * (M134)
 *
 * **Import-free**, `model.ts`'s rule and for its reason: `check-sessions.mjs` compiles this file
 * alone with a bare `tsc` and imports the result under node. So the wire's `SessionRow` is
 * restated structurally as `SessionView` (`sessionsStore.ts` adapts it), and the kinds are a
 * frozen list the check pins against `cide_ipc::sessions::SessionKind` — a tenth kind arriving from
 * Rust with no label and no place in the filter row fails there, not on screen as a blank chip.
 *
 * # The two searches, and why they are OR'd
 *
 * The text box always searches what the journal holds — title, `/rename` name, first prompt, task,
 * role, branch, directory — on every keystroke, here. *Search transcripts* adds what the
 * conversations *said*, which only Rust can read and which arrives later as `hits`. A row is kept
 * when either finds it: turning transcripts on must only ever add rows, or the toggle would read
 * as a different search rather than a deeper one.
 */

/** `cide_ipc::sessions::SessionKind`, camelCased, in the filter row's order. */
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
 * chip (`check-agents.mjs`' `ROGUE` lesson).
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

/** One row, as the view draws it. */
export interface SessionView {
  /** The conversation id — the row's key, and what Open and the search name it by. */
  id: string
  kind: SessionKindName
  harness: string
  title: string
  /** The `/rename` name, drawn instead of `title` when there is one. */
  name: string | null
  prompt: string | null
  task: string | null
  taskTitle: string | null
  agent: string | null
  branch: string | null
  cwd: string
  startedMs: number
  lastSeenMs: number
  live: boolean
  /** A pane of the project shows it: Open reveals instead of resuming. */
  pane: string | null
  /** Its transcript can be searched (claude, qwen, codex). */
  searchable: boolean
}

/** What a row is called: its name, its title, or its id when it has neither. */
export function displayTitle(v: SessionView): string {
  if (v.name !== null && v.name.trim() !== '') return v.name
  if (v.title.trim() !== '') return v.title
  return v.id
}

export interface SessionFilter {
  query: string
  /** The kinds to keep. Empty keeps every kind — the "All" chip. */
  kinds: ReadonlySet<SessionKindName>
  /**
   * The one subagent role to keep — `Developer`, `Artist` — or `null` for every row. Its own
   * control and not a word in the search box: "which runs did the Developer do" is the question a
   * project with fifty runs is asked most, and a role name typed as text also matches every prompt
   * and task title that mentions it.
   */
  agent: string | null
}

/** Every text the journal holds about a row, lowercased and joined, for the text search. */
function haystack(v: SessionView): string {
  return [v.name, v.title, v.prompt, v.task, v.taskTitle, v.agent, v.branch, v.cwd, v.id, kindLabel(v.kind)]
    .filter((s): s is string => s !== null)
    .join('\n')
    .toLowerCase()
}

/**
 * Whether the journal's text matches `query`: every whitespace-separated word, anywhere. Words and
 * not the phrase, because the fields are separate — "coder t-14" should find the coder's run on
 * t-14, whose role and task are two fields.
 */
export function matchesText(v: SessionView, query: string): boolean {
  const words = query.toLowerCase().split(/\s+/).filter((w) => w !== '')
  if (words.length === 0) return true
  const text = haystack(v)
  return words.every((w) => text.includes(w))
}

/**
 * The rows a filter keeps, in the order given (newest first, from Rust).
 *
 * `hits` is the transcript search's answer — conversation id → snippet — or `null` when it is off
 * or has not answered. See the header for why a hit keeps a row the text search would drop.
 */
export function filterSessions(
  views: readonly SessionView[],
  filter: SessionFilter,
  hits: Readonly<Record<string, string>> | null,
): SessionView[] {
  return views.filter(
    (v) =>
      (filter.kinds.size === 0 || filter.kinds.has(v.kind)) &&
      (filter.agent === null || v.agent === filter.agent) &&
      (matchesText(v, filter.query) || (hits !== null && Object.hasOwn(hits, v.id))),
  )
}

/** How many rows of each kind there are, for the filter chips — only kinds present, in order. */
export function kindCounts(views: readonly SessionView[]): { kind: SessionKindName; count: number }[] {
  const counts = new Map<SessionKindName, number>()
  for (const v of views) counts.set(v.kind, (counts.get(v.kind) ?? 0) + 1)
  return SESSION_KINDS.filter((k) => counts.has(k)).map((kind) => ({ kind, count: counts.get(kind) ?? 0 }))
}

/**
 * The roles that ran in this project, for the role filter: each with its run count, most runs
 * first, then by name. Only rows that carry a role — consoles and tabs have none.
 */
export function agentCounts(views: readonly SessionView[]): { agent: string; count: number }[] {
  const counts = new Map<string, number>()
  for (const v of views) if (v.agent !== null && v.agent !== '') counts.set(v.agent, (counts.get(v.agent) ?? 0) + 1)
  return [...counts]
    .map(([agent, count]) => ({ agent, count }))
    .sort((a, b) => b.count - a.count || a.agent.localeCompare(b.agent))
}

const MINUTE = 60_000
const HOUR = 60 * MINUTE
const DAY = 24 * HOUR

/**
 * When a row was last seen, relative to `nowMs`. Coarse on purpose — "3h ago", not "3h 12m" — a
 * row is found by roughly when, and the tab re-renders on the panel's clock.
 */
export function ago(nowMs: number, thenMs: number): string {
  const d = Math.max(0, nowMs - thenMs)
  if (d < MINUTE) return 'just now'
  if (d < HOUR) return `${Math.floor(d / MINUTE)}m ago`
  if (d < DAY) return `${Math.floor(d / HOUR)}h ago`
  if (d < 30 * DAY) return `${Math.floor(d / DAY)}d ago`
  return new Date(thenMs).toISOString().slice(0, 10)
}
