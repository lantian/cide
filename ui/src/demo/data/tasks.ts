/**
 * The demo project's task board: the PTY-backpressure story from `data/git.ts`, broken into the
 * tasks an orchestrator would have filed for it, plus the ordinary tail of a tracker that has
 * been in use for a while.
 *
 * Wire shapes (`TaskBoard`, `TaskDetail`), not `TasksPanel/fixture.ts`'s view props: the demo
 * answers `tasks_board` and `task_get`, and the store runs its own adapter over them — so the
 * picture goes through the same conversion a real board does.
 *
 * Timestamps are relative to the moment the page loads rather than fixed: the panel's clock is
 * `Date.now()`, and a fixed instant would read "10 months ago" on every row.
 */
import type {
  LinkType,
  TaskAuthor,
  TaskBoard,
  TaskComment,
  TaskDetail,
  TaskLink,
  TaskRow,
  TaskStatus,
  TaskStatusChange,
} from '../../ipc/generated'
import { wire } from '../world'

export const NOW = Date.now()
const MIN = 60_000
const HOUR = 60 * MIN
const DAY = 24 * HOUR
export const ago = (ms: number) => wire(NOW - ms)

export const USER: TaskAuthor = { kind: 'user' }
export const ORCH: TaskAuthor = { kind: 'orchestrator' }
export const agent = (id: string, label: string): TaskAuthor => ({ kind: 'agent', agent: id, label })

const link = (kind: LinkType, target: string, at = 2 * HOUR): TaskLink => ({
  link: kind,
  target,
  deleted: false,
  atUnixMs: ago(at),
})

function row(
  id: string,
  title: string,
  status: TaskStatus,
  over: Partial<TaskRow> & { age?: number; touched?: number } = {},
): TaskRow {
  const { age = 3 * HOUR, touched = 20 * MIN, ...rest } = over
  return {
    id,
    title,
    status,
    agent: null,
    links: [],
    createdBy: ORCH,
    createdUnixMs: ago(age),
    updatedUnixMs: ago(touched),
    commentCount: 0,
    attachmentCount: 0,
    ...rest,
  }
}

/** The epic every other open task hangs off. */
export const EPIC = 't-40'

export const TASKS: TaskRow[] = [
  // Noticed along the way, not yet triaged: the inbox.
  row('t-49', 'Terminal flickers when a pane is resized mid-flush', 'inbox', {
    createdBy: agent('tester', 'Tester'),
    age: 25 * MIN,
    touched: 25 * MIN,
    links: [link('related', 't-41', 25 * MIN)],
  }),
  row('t-50', 'cide-headless could print coalescer stats', 'inbox', {
    createdBy: agent('coder', 'Coder'),
    age: 12 * MIN,
    touched: 12 * MIN,
  }),

  row(EPIC, 'PTY backpressure: a slow webview must not grow memory', 'doing', {
    createdBy: USER,
    age: 5 * HOUR,
    touched: 8 * MIN,
    commentCount: 3,
  }),
  row('t-41', 'Split the PTY coalescer out of session.rs', 'doing', {
    agent: 'coder',
    links: [link('subtaskOf', EPIC)],
    age: 4 * HOUR,
    touched: 2 * MIN,
    commentCount: 4,
  }),
  row('t-42', 'Test: a stalled sink keeps the ring bounded', 'doing', {
    agent: 'tester',
    links: [link('subtaskOf', EPIC), link('blockedBy', 't-41')],
    age: 4 * HOUR,
    touched: 6 * MIN,
    commentCount: 2,
  }),
  row('t-43', 'Bound the frame channel and spill to scrollback', 'todo', {
    agent: 'coder',
    links: [link('subtaskOf', EPIC), link('blockedBy', 't-41')],
    age: 4 * HOUR,
    touched: 40 * MIN,
    commentCount: 1,
  }),
  row('t-44', 'architecture.md: rewrite the PTY coalescing section', 'todo', {
    agent: 'docs',
    links: [link('subtaskOf', EPIC), link('blockedBy', 't-43')],
    age: 4 * HOUR,
    touched: 55 * MIN,
  }),
  row('t-45', 'check:render-stall should replay a 10 MB burst', 'todo', {
    agent: 'tester',
    links: [link('related', 't-42')],
    age: 3 * HOUR,
    touched: HOUR,
  }),
  row('t-46', 'Audit every early return in ide.rs::pump for openDiff', 'todo', {
    agent: 'security-auditor',
    age: 2 * DAY,
    touched: 5 * HOUR,
    createdBy: USER,
  }),
  row('t-47', 'Journal entry for M103', 'todo', { agent: 'docs', createdBy: USER, age: DAY, touched: 3 * HOUR }),

  row('t-37', 'Coalesce small frames through webview.eval on the GTK loop', 'review', {
    agent: 'coder',
    age: DAY,
    touched: 18 * MIN,
    commentCount: 5,
  }),
  row('t-38', 'Stop re-rendering for unchanged rosters and log rows', 'review', {
    agent: 'reviewer',
    age: DAY,
    touched: 32 * MIN,
    commentCount: 2,
  }),

  row('t-39', 'Coalescer returns Pressure::Hold past the ack window', 'done', {
    agent: 'coder',
    links: [link('subtaskOf', EPIC)],
    age: 4 * HOUR,
    touched: 70 * MIN,
    commentCount: 2,
  }),
  row('t-31', 'Measure the largest frame burst in real sessions', 'done', {
    agent: 'tester',
    links: [link('subtaskOf', EPIC)],
    age: 5 * HOUR,
    touched: 3 * HOUR,
    commentCount: 1,
  }),
  row('t-36', 'Refit off-screen panes after a resize settles', 'done', { agent: 'coder', age: 2 * DAY, touched: 6 * HOUR, commentCount: 3 }),
  row('t-35', 'Share unchanged workspace subtrees between revisions', 'done', { agent: 'coder', age: 2 * DAY, touched: 9 * HOUR }),
  row('t-34', 'Language-server teardown off the main thread', 'done', { agent: 'coder', age: 3 * DAY, touched: DAY }),
  row('t-33', 'Document diag_echo_bytes in docs/checks.md', 'done', { agent: 'docs', age: 3 * DAY, touched: DAY }),
  row('t-32', 'check:selectors flags an inline array selector', 'done', { agent: 'tester', age: 4 * DAY, touched: 2 * DAY }),
]

export const BOARD: TaskBoard = { kind: 'ready', tasks: TASKS, rev: wire(318) }

export const TITLES: Record<string, string> = Object.fromEntries(TASKS.map((t) => [t.id, t.title]))

let commentSeq = 0
const comment = (author: TaskAuthor, at: number, text: string): TaskComment => ({
  id: `c-${(++commentSeq).toString().padStart(4, '0')}`,
  author,
  text,
  atUnixMs: ago(at),
  editedAtUnixMs: null,
  deleted: false,
  attachments: [],
})

const moved = (from: TaskStatus, to: TaskStatus, by: TaskAuthor, at: number): TaskStatusChange => ({
  from,
  to,
  by,
  atUnixMs: ago(at),
})

const T41_BODY = `Frames are batched inside \`session.rs\` today, tangled with the child's lifecycle. Move the batching into \`coalesce.rs\` so backpressure (t-43) has one place to live.

- \`Coalescer\` owns the 4 ms window, the 16 KiB cap and the flush
- Small payloads still go through \`webview.eval\` — coalescing is correctness, not tuning
- Done when \`check:render-stall\` and \`check:attach\` pass`

const T41_COMMENTS: TaskComment[] = [
  comment(ORCH, 4 * HOUR, 'Filed from t-40. Assigned to @coder; @tester picks up t-42 once the module boundary exists.'),
  comment(
    agent('coder', 'Coder'),
    90 * MIN,
    'Moved `Coalescer` into `coalesce.rs` (218 lines) and left `session.rs` calling `push`/`flush`. The exit path needed care: the final flush has to happen before `SessionExit` is emitted, or the last frame lands after the pane reads "exited". Covered by `flushes_before_exit`.',
  ),
  comment(
    USER,
    40 * MIN,
    'Keep the 4 ms window a `const` for now — making it a setting is t-50 territory, not this task.',
  ),
  comment(
    agent('coder', 'Coder'),
    6 * MIN,
    '`cargo test -p cide-pty`: 41 passed. Running `check:render-stall` next; attached the frame-count comparison against master.',
  ),
]

export const T41: TaskDetail = {
  row: TASKS.find((t) => t.id === 't-41') as TaskRow,
  body: T41_BODY,
  comments: T41_COMMENTS,
  attachments: [],
  history: [moved('todo', 'doing', agent('coder', 'Coder'), 3 * HOUR)],
}

/** Every other task's card: its row, a plain body, no thread. Only t-41 is opened on purpose. */
export function detailOf(id: string): TaskDetail | null {
  if (id === 't-41') return T41
  const found = TASKS.find((t) => t.id === id)
  if (!found) return null
  return { row: found, body: `${found.title}.`, comments: [], attachments: [], history: [] }
}
