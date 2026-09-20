/**
 * The Tasks panel's named states, as plain props. (M18)
 *
 * # Why these exist
 *
 * Two of them cannot be arranged on demand at all. An `unreadable` board needs a
 * `.cide/tasks.json` that does not parse — most often a half-resolved merge conflict — and it
 * is the state with the strictest rule in the whole panel: **only** Reveal and Retry, because
 * an app that "recovers" from an unparseable tracker by overwriting it has destroyed the user's
 * data to fix its own display. A status outside the four is the other: it comes from a cide
 * newer than this one, or from a hand edit, and the panel must group it somewhere visible
 * rather than let the task fall out of a tracker somebody is relying on.
 *
 * # Two families of story, because the card is a modal
 *
 * `TASKS_STORIES` are `TasksPanelViewProps` — the list, in every board state. `CARD_STORIES` are
 * `TaskDetailProps` — the open task's card, in every read/edit state. `COMPOSE_STORIES` are
 * `TaskComposeProps` — the new-task dialog, empty and filled in. (M21) They are separate because
 * the card is now mounted by `TaskDetailHost` beside the view rather than inside it, and because
 * the thing that mounts it (`TaskDetailModal`, an `OverlayCard`) portals to `document.body`,
 * which `react-dom/server` refuses to render at all. `TaskDetail` is the whole card minus that
 * one wrapper, so every field, every mode and every control stays inside the render gate; what
 * the wrapper adds is checked as source in `check-agents-render.mjs`.
 *
 * # `list` and `list-with-live-run` are the same board
 *
 * Deliberately: the *only* difference between them is the runs array. That is what makes
 * "the chip's class set differs" a statement about the cross-link rather than about two
 * unrelated fixtures — an exited agent must not look like a working one, and the two stories
 * are the two sides of exactly that claim.
 *
 * # Milliseconds are `number`
 *
 * `rev`, `createdUnixMs`, `updatedUnixMs` and `atUnixMs` are `u64` on the wire and therefore
 * `bigint` in `generated.ts`; `adapt.ts` converts them. Everything here is a plain `number`,
 * matching `model.ts`. Mixing the two in a comparison throws a `TypeError` rather than
 * coercing, and a render that throws unmounts the tree under React 19 — so the cost of getting
 * this wrong is a blank window rather than a wrong date.
 */
import type { ProjectId } from '@/ipc/client'
import {
  EMPTY_DRAFT,
  assigneeHint,
  linkTargetOptions,
  linkableTargets,
  taskLinks,
  type ArmedDelete,
  type Board,
  type CommentView,
  type FieldEdit,
  type LinkTargetOption,
  type RunRef,
  type TaskDraft,
  type TaskStatus,
  type TaskDetailView,
  type TaskView,
  type AttachmentPreview,
  type AttachmentView,
  type StagedAttachment,
} from './model'
import { mentionOptions, type MentionOption } from './mentionModel'
import type { TasksPanelViewProps } from './TasksPanel'
import type { TaskDetailPendingProps, TaskDetailProps } from './TaskDetail'
import type { TaskComposeProps } from './TaskCompose'

/** A fixed instant, so a digest of a story is the same on two runs. */
export const NOW_MS = 1_764_005_000_000

export const PROJECT = 'ffffffff-0000-4000-8000-00000000cafe' as unknown as ProjectId

/** The path the `absent` and `unreadable` screens print, and that the render check greps for. */
export const TASKS_PATH = '/home/dev/work/thing/.cide/tasks.json'

const ROLES: Readonly<Record<string, string>> = {
  developer: 'Developer',
  qa: 'QA',
}

/**
 * `id` is derived from the timestamp rather than minted, so a story renders byte-identically on
 * every run — `check-agents-render.mjs` compares a digest, and a uuid per call would make every
 * comparison fail for a reason that has nothing to do with the panel.
 */
function comment(over: Partial<CommentView> & Pick<CommentView, 'text' | 'atMs'>): CommentView {
  return {
    id: `c-${over.atMs}`,
    author: { kind: 'user' },
    editedMs: null,
    // No files, which is most comments — and the value that keeps every existing story's
    // digest byte-identical to what it was before M39.
    attachments: [],
    ...over,
  }
}

/*
 * Out of order on purpose.
 *
 * `Task::comments` is documented as arriving oldest first, so `commentOrder` is a defence
 * rather than a transformation — and the thing it defends against is real: this file is
 * committed, and a merge or a hand edit can interleave two agents' lines. A log that is not in
 * time order reads as a different conversation, so the fixture arrives scrambled and the render
 * check asserts the panel prints it straight.
 */
const COMMENTS: readonly CommentView[] = [
  comment({
    text: 'Retried once; the second attempt got through.',
    atMs: NOW_MS - 600_000,
    author: { kind: 'agent', agent: 'developer', label: 'Developer' },
  }),
  comment({ text: 'The bar should offer a retry, not re-dispatch on its own.', atMs: NOW_MS - 3_600_000 }),
  comment({
    text: 'Assigned to developer.\n\nTwo lines, and the break matters.',
    atMs: NOW_MS - 1_800_000,
    author: { kind: 'orchestrator' },
  }),
]

/**
 * A board **row**: what `cide://tasks-changed` carries since M68.
 *
 * The content fields moved to [`detail`], and the split is the fixture's half of what makes the
 * card unrepresentable without content — a story that wants a body or a log has to ask for one, so
 * there is no way to write a card story that silently renders `No comments yet.` over a task the
 * fixture meant to give a conversation.
 */
function task(over: Partial<TaskView> & Pick<TaskView, 'id' | 'title'>): TaskView {
  return {
    status: 'todo',
    // No change, which is most tasks — and the value that keeps every existing story's digest
    // byte-identical to what it was before M28. `check:agents-render` compares them.
    change: null,
    // And no conversation, the same claim: the work of a task that went to a role, or nowhere
    // yet, is not with a Claude session and the card draws no session row.
    session: null,
    // No links either, the same claim for M30: a card handed no `links` prop draws no Links
    // section at all, so every pre-M30 story's digest stands.
    links: [],
    agent: null,
    // Zero of each, the same claim for M68: a row with no conversation and no files. `detail`
    // recomputes both from the content it is given, so a card story cannot end up claiming three
    // comments over a log of two.
    commentCount: 0,
    attachmentCount: 0,
    // The user, by default, because that is what the panel's own New task button produces. The
    // stories that need the other answer say so; see `BARE`, which is the card's half of the pair
    // that pins the creator to the author enum rather than to a name.
    createdBy: { kind: 'user' },
    createdMs: NOW_MS - 86_400_000,
    updatedMs: NOW_MS - 600_000,
    ...over,
  }
}

/**
 * A row **with its content**: what `task_get` answers and what the card takes. (M68)
 *
 * The counts are **derived here** rather than defaulted, and that is what keeps the fixture honest.
 * `cide_tasks::row_of` derives them in the app; a story that set them by hand would be free to
 * claim three comments over a log of two, and since no surface draws the number yet, nothing in
 * either check would notice. Deriving them means the fixture cannot say something the app never
 * would.
 */
function detail(
  over: Partial<TaskDetailView> & Pick<TaskDetailView, 'id' | 'title'>,
): TaskDetailView {
  const body = over.body ?? ''
  const comments = over.comments ?? []
  const attachments = over.attachments ?? []
  const history = over.history ?? []
  const files =
    attachments.length + comments.reduce((n, c) => n + c.attachments.length, 0)
  return {
    ...task(over),
    commentCount: comments.length,
    attachmentCount: files,
    body,
    comments,
    attachments,
    history,
  }
}

const T14 = detail({
  id: 't-14',
  title: 'Add the retry bar',
  body: 'A frozen run may have lost its turn. Offer a retry rather than re-dispatching.',
  status: 'doing',
  agent: 'developer',
  comments: COMMENTS,
  /*
   * Out of order on purpose, `COMMENTS`' trick for the other log: the wire promises oldest
   * first, a merge can break the promise, and the render check asserts the card prints the
   * transitions straight anyway. The pair of authors is the point of the feature — the second
   * hop is the orchestrator's automatic `Todo → Doing` on dispatch, and the row must say so.
   */
  history: [
    { from: 'doing', to: 'review', by: { kind: 'user' }, atMs: NOW_MS - 900_000 },
    { from: 'todo', to: 'doing', by: { kind: 'orchestrator' }, atMs: NOW_MS - 7_200_000 },
    { from: 'review', to: 'doing', by: { kind: 'user' }, atMs: NOW_MS - 700_000 },
  ],
})

const T15 = detail({
  id: 't-15',
  title: 'Sweep the phase table',
  status: 'review',
  agent: 'developer',
  // A subagent decomposed this one into existence. The third arm of the author union, so all
  // three are somewhere in the fixtures rather than only the two the card stories draw.
  createdBy: { kind: 'agent', agent: 'developer', label: 'Developer' },
})

const T16 = detail({ id: 't-16', title: 'Write check-agents' })

const T17 = detail({
  id: 't-17',
  title: 'Teach the watcher about .cide/',
  status: 'done',
  agent: 'qa',
})

/*
 * A status from a cide newer than this one, and one that is a prototype key.
 *
 * Cast rather than written as a `TaskStatus`, because that is the situation: the wire type
 * claims one and a hand-edited JSON file says otherwise. `'constructor'` is the member an
 * `?? fallback` does not catch — the table lookup returns `Object.prototype.constructor`, a
 * function, which React refuses as a child and which `className` stringifies into the whole
 * source text of `Object`.
 */
const ROGUE = detail({
  id: 't-18',
  title: 'A task from the future',
  status: 'constructor' as unknown as TaskStatus,
})

/*
 * The markdown card's task — `card-markdown`'s whole fixture. (M27)
 *
 * Kept off the list stories on purpose: T14's plain body is pinned byte-for-byte by the
 * `fieldValues` assertion, and rewriting it would touch every story at once. This one exists
 * so the render check can say "the fixture wrote `**strong**` and the markup contains
 * `<strong>`" about a card whose other properties nothing else asserts on.
 */
const RICH = detail({
  id: 't-21',
  title: 'Render tracker text as markdown',
  status: 'doing',
  agent: 'developer',
  body: [
    '## Scope',
    '',
    'Bodies and comments render **strong**, *emphasis* and `code` spans.',
    '',
    '- the body at rest',
    '- every comment',
    '',
    '```rust',
    'fn main() {}',
    '```',
    '',
    'See [the markdown pane](https://example.invalid/preview) for the full grammar.',
  ].join('\n'),
  comments: [
    comment({
      text: 'Done — `TaskMarkdown.tsx` renders both:\n\n1. parse with `blocks.ts`\n2. render as elements',
      atMs: NOW_MS - 500_000,
      author: { kind: 'agent', agent: 'developer', label: 'Developer' },
    }),
  ],
})

/*
 * Recency, as a fixture. (See `groups`'s doc for the ordering argument.)
 *
 * Three Todo tasks, in the file in the order they were created — which is the order an
 * append-only tracker accretes, oldest activity first. A panel drawing the array as written
 * would print exactly the reverse of what the check asserts, so the story is a statement about
 * the sort rather than about the fixture. All three in one group, so the assertion cannot be
 * satisfied by `GROUP_ORDER` doing the work.
 */
const T31 = detail({ id: 't-31', title: 'Quiet since this morning', updatedMs: NOW_MS - 7_200_000 })
const T32 = detail({ id: 't-32', title: 'Stirred an hour ago', updatedMs: NOW_MS - 3_600_000 })
const T33 = detail({ id: 't-33', title: 'Touched a minute ago', updatedMs: NOW_MS - 60_000 })

const REV = 42

function ready(tasks: readonly TaskView[], rev = REV): Board {
  return { kind: 'ready', tasks, rev }
}

/**
 * An arming that is still good: `t-14`, on the rev every `ready()` board below carries.
 *
 * Written as a constant beside [`STALE_ARM`] so the two stories differ in **one field**, which
 * is what makes "the board moved and the arming lapsed" a statement about `armedDelete` rather
 * than about two unrelated fixtures. Same trick as `list` / `list-with-live-run`.
 */
const ARM: ArmedDelete = { task: 't-14', rev: REV }

/**
 * The same arming, one rev behind — a user who armed a delete and then had the board change
 * underneath them, which is ordinary rather than rare: two cide windows and every dispatched
 * agent write this file.
 */
const STALE_ARM: ArmedDelete = { task: 't-14', rev: REV - 1 }

/** A live run against `t-14`, and the only difference between two of the stories below. */
const LIVE: readonly RunRef[] = [
  {
    run: 'r-0001',
    task: 't-14',
    agentLabel: 'Developer',
    phase: 'running',
    session: 's-0001',
    openable: true,
  },
]

/**
 * Every handler wired to a no-op, on **every** story — including the ones that must render
 * nothing.
 *
 * A check asserting "the unreadable board offers no writing control" against a story that was
 * given no `onCreateTask` asserts nothing at all. The gate being tested is `canWrite`, and it
 * can only be seen doing its work when the alternative was available.
 */
const HANDLERS = {
  onSelectTask: () => {},
  onCreateTask: () => {},
  onFilter: () => {},
  onQuery: () => {},
  /*
   * Both halves of the delete, on every story. The unarmed assertions — "no story shows the
   * confirming label unless it was armed" — are worth nothing against a fixture that was never
   * given `onDelete`: `DeleteControl` draws nothing at all without it, so the check would be
   * asserting the absence of a control that could not have appeared.
   */
  onDeleteArm: () => {},
  onDelete: () => {},
  onReveal: () => {},
  onRetry: () => {},
} as const

/**
 * The card's handlers, on **every** card story, for [`HANDLERS`]'s reason.
 *
 * `onEditing` matters most here. Without it the pencil would still render — it is not gated on a
 * handler — but a check asserting "at rest there is no control in a field" would be asserting it
 * of a card that could not have opened one anyway.
 */
/**
 * Every handler the host passes, and the two that were missing are the point. (M21)
 *
 * `onEditComment` and `onDeleteComment` are optional on `TaskDetailProps` — the card draws each
 * control only when its handler is there, which is `TaskLine`'s `rowTag` rule applied to an
 * action — and the stories were built before either existed. The consequence was that **no
 * story rendered the per-comment controls at all**, so the render gate had nothing to say about
 * a pair of buttons that were on screen in the running app: exactly the shape of hole that lets
 * a control ship unreachable. They are here so the markup is checked, and the check asserts one
 * of each per comment.
 */
const CARD_HANDLERS = {
  onEditing: () => {},
  onClose: () => {},
  onSetTitle: () => {},
  onSetStatus: () => {},
  onSetAssignee: () => {},
  onSetBody: () => {},
  onAddComment: () => {},
  onEditComment: () => {},
  onDeleteComment: () => {},
  onDeleteArm: () => {},
  onDelete: () => {},
  onOpenRun: () => {},
  onPauseRun: () => {},
  onResumeRun: () => {},
  /*
   * On every card story, so "no session row" is never true merely because nobody passed a
   * handler. The row is gated on `spec.session`, and its Open control on the pane still being
   * there — both of which are facts about the story, which is where the assertions want them.
   */
  onOpenSession: () => {},
} as const

/* ------------------------------------------------------------------- typed links (M30) */

/**
 * The linked card's task: three kinds outgoing, one of them dangling.
 *
 * `t-99` is on nobody's board — deleted, or living on a branch not pulled yet — and the chip
 * must draw *marked* rather than vanish: a reference that silently disappeared is the
 * task-leaves-the-tracker failure, one edge over.
 */
const LINKED = detail({
  id: 't-40',
  title: 'Ship the panel',
  links: [
    { kind: 'blockedBy', target: 't-14' },
    { kind: 'subtaskOf', target: 't-16' },
    { kind: 'related', target: 't-17' },
    { kind: 'blockedBy', target: 't-99' },
  ],
})

/**
 * The other end: a task whose own stored edge names `t-40`. Its chip on `t-40`'s card —
 * "Blocks t-41" — is **derived**, which is the claim the story exists for: each edge is stored
 * once, on its canonical side, and the other reading is computed from the board.
 */
const BLOCKED_BY_LINKED = detail({
  id: 't-41',
  title: 'Write the release note',
  links: [{ kind: 'blockedBy', target: 't-40' }],
})

const LINK_BOARD: readonly TaskView[] = [T14, T16, T17, LINKED, BLOCKED_BY_LINKED]

/**
 * The link handlers, separate from `CARD_HANDLERS` on purpose: a card handed no `links` prop
 * draws no Links section at all — that absence is the pre-M30 digest claim — so only the link
 * stories carry these.
 */
const LINK_HANDLERS = {
  onLinkAdd: () => {},
  onLink: () => {},
  onUnlink: () => {},
  onOpenTask: () => {},
} as const

/** Through the real derivations, so the stories exercise `taskLinks`' actual answers. */
const LINK_CHIPS = taskLinks(LINKED, LINK_BOARD)
const LINK_TARGETS = linkableTargets('t-40', LINK_BOARD).map((candidate) => ({
  id: candidate.id,
  title: candidate.title,
  status: candidate.status,
}))

function story(over: Partial<TasksPanelViewProps>): TasksPanelViewProps {
  return { project: PROJECT, roles: ROLES, ...HANDLERS, ...over }
}

function card(over: Partial<TaskDetailProps> & Pick<TaskDetailProps, 'task'>): TaskDetailProps {
  return { runs: [], roles: ROLES, nowMs: NOW_MS, ...CARD_HANDLERS, ...over }
}

function compose(draft: TaskDraft, over: Partial<TaskComposeProps> = {}): TaskComposeProps {
  return {
    draft,
    roles: ROLES,
    onDraft: () => {},
    onCreate: () => {},
    onCancel: () => {},
    ...over,
  }
}

/**
 * A task that implements an OpenSpec change.
 *
 * Its whole job is `change`, and its pair is every other card story, where `change` is `null` and
 * the prop is absent — which is the optionality claim: with no change the card's markup is what
 * it was before M28.
 */
/**
 * A change as the card has it once the read lands — 3 of 9 steps, valid, one requirement.
 *
 * `action` is what `OpenSpecPanel/model.ts::primaryAction` answers for that state, restated here
 * rather than computed: this fixture belongs to the *card*, and a story that called into the
 * panel's model would be testing two modules at once and would go green the day either changed.
 */
const SPEC_CARD = {
  change: 'add-dark-mode',
  done: 3,
  total: 9,
  valid: true,
  issues: 0,
  tasks: [
    { done: true, description: 'Add the theme tokens' },
    { done: false, description: 'Wire the palette switch' },
  ],
  deltas: [
    {
      spec: 'dark-mode',
      op: 'added',
      requirements: [
        {
          target: 'd0.r0',
          issues: [],
          name: 'Theme switching',
          text: 'The app SHALL switch themes.',
          scenarios: [{ title: 'Picks dark', body: '- **WHEN** a\n- **THEN** b' }],
          block: '### Requirement: Theme switching',
        },
      ],
    },
  ],
  artifacts: [{ id: 'tasks', path: '/repo/openspec/changes/add-dark-mode/tasks.md' }],
  action: {
    id: 'approve' as const,
    label: 'Approve & dispatch',
    hint: 'Choose where the work happens.',
    enabled: true,
    reason: '',
  },
  /** Nothing has been dispatched anywhere: no session row, and the approve button stands. */
  session: null,
  /** In flight. `card-spec-archived` is the same change once `openspec archive` has moved it. */
  archived: null,
}

/**
 * The same change, handed to a conversation. (M28)
 *
 * `action.id` is `none` because `primaryAction` refuses the approve road once a session is set —
 * so this story is also the assertion that the two move together. A card that kept the button
 * *and* drew this row would be offering to dispatch work that is already being done, which is
 * exactly what shipped: choosing *New Claude session* started the session, typed the task in, and
 * left **Approve & dispatch** sitting over it.
 */
const SPEC_CARD_SESSION = {
  ...SPEC_CARD,
  action: {
    id: 'none' as const,
    label: '',
    hint: 'This change’s work is with a conversation. Open it to see where it has got to.',
    enabled: false,
    reason:
      'Its work is with a Claude conversation — open this change’s task to see where it has got to.',
  },
  session: {
    id: '11111111-2222-4333-8444-555555555555',
    label: 'Conversation 1',
    open: true,
    awaiting: false,
  },
}

/** The three kinds of place a run can happen. See `specCard.ts::DispatchTarget`. */
const TARGETS = [
  { kind: 'role' as const, id: 'developer', label: 'Developer', detail: 'Runs unattended.' },
  {
    kind: 'session' as const,
    id: '11111111-2222-4333-8444-555555555555',
    label: 'Conversation 1',
    detail: 'Already open — the task is typed into it.',
  },
  { kind: 'fresh' as const, id: '', label: 'New Claude session', detail: 'Adds a pane.' },
]

const SPEC_TASK = detail({
  id: 't-21',
  title: 'Add the dark theme',
  status: 'doing',
  agent: 'developer',
  change: 'add-dark-mode',
})

/**
 * A task with nothing in any of the three editable fields, and the reason it is a story.
 *
 * `restText` is total and never returns an empty string, and *that* is the property this fixture
 * exists to render: a field whose value is absent still draws a row with a placeholder in it. A
 * card that collapsed those rows would be indistinguishable on screen from one that failed to
 * render them.
 */
const BARE = detail({
  id: 't-19',
  title: '',
  body: '',
  status: 'todo',
  agent: null,
  /*
   * Created by the orchestrator, which is the second half of the creator assertion. (M21)
   *
   * `card` is the user's — *Created by You* — and this one is not, over a task that is
   * *unassigned*: the pair is what proves the head names the **creator** and not the assignee,
   * which is the one way that line can be quietly wrong. A card whose creator followed `agent`
   * would print nothing here and pass every other assertion in the story.
   */
  createdBy: { kind: 'orchestrator' },
})

/** A title mid-edit: the draft differs from the task's, which is what makes it *dirty*. */
const TITLE_DRAFT: FieldEdit = { field: 'title', draft: 'Add the retry bar, with a reason' }

/** An assignee editor that has been opened and not yet changed — open, and clean. */
const ASSIGNEE_EDIT: FieldEdit = { field: 'assignee', draft: 'developer' }

/** A body mid-edit. */
const BODY_DRAFT: FieldEdit = { field: 'body', draft: 'Rewritten, and not yet saved.' }

export type TasksStoryName =
  | 'no-project'
  | 'board-unknown'
  | 'absent'
  | 'unreadable'
  | 'empty'
  | 'list'
  | 'list-with-live-run'
  | 'list-selected'
  | 'rogue-status'
  | 'list-recent-first'
  | 'list-delete-armed'
  | 'list-delete-stale'
  | 'list-filtered'
  | 'list-filtered-rogue'
  | 'list-filter-no-match'
  | 'list-searched'
  | 'list-search-no-match'
  | 'list-search-pending'

/**
 * The card before `task_get` has answered. (M68)
 *
 * Its own tiny table because the pending card takes a **row**, not a `TaskDetailProps` — which is
 * the point of it: the one component in the card's file that may be handed a row, precisely because
 * it draws no content field at all.
 *
 * `task()` and not `detail()`, deliberately. A `detail()` here would hand the story a body and a
 * log it is forbidden to draw, and the check could then pass while the component quietly started
 * drawing them.
 */
export type PendingStoryName = 'card-pending' | 'card-pending-long-title'

export const PENDING_STORIES: Record<PendingStoryName, TaskDetailPendingProps> = {
  'card-pending': {
    task: task({ id: 't-14', title: 'Add the retry bar', status: 'doing' }),
    onClose: () => {},
  },
  /* A title long enough to wrap, because the pending card reserves a minimum height and a wrapped
     title must eat into it rather than push the note out of the box. */
  'card-pending-long-title': {
    task: task({
      id: 't-704',
      title:
        'Nomad infantry: Scavenger and Raider models — establish the silhouette, then the walk cycle',
      status: 'todo',
    }),
    onClose: () => {},
  },
}

export const TASKS_STORIES: Record<TasksStoryName, TasksPanelViewProps> = {
  'no-project': story({ project: null, board: ready([T14]) }),

  /*
   * Nobody has looked yet. The assertion is negative — no button, and none of `absent`'s
   * sentences — because drawing "there is no tracker in this project" one frame before the
   * tracker arrives is a confident claim about a file nothing has read.
   */
  'board-unknown': story({ board: { kind: 'unknown' } }),

  absent: story({
    board: {
      kind: 'absent',
      hint: 'Nothing has created .cide/tasks.json in this project yet.',
      path: TASKS_PATH,
    },
  }),

  unreadable: story({
    board: {
      kind: 'unreadable',
      path: TASKS_PATH,
      error: 'expected value at line 12 column 3 — the file still contains conflict markers',
    },
  }),

  empty: story({ board: ready([]) }),

  list: story({ board: ready([T14, T15, T16, T17]) }),

  /* The same board, one run. See the file header. */
  'list-with-live-run': story({ board: ready([T14, T15, T16, T17]), runs: LIVE }),

  /*
   * The same board as `list`, with `t-14`'s card open.
   *
   * The whole point is that it is **still the same list**: the card is a modal mounted beside
   * this view, so four rows, the filter row and the New task button are all still on screen
   * behind its scrim. The one thing that differs is the mark on the row the card came from —
   * without it a modal over four near-identical rows makes the user close it to find out which
   * one they opened.
   */
  'list-selected': story({ board: ready([T14, T15, T16, T17]), selected: 't-14' }),

  /*
   * A status this build has never heard of, beside three it has.
   *
   * The rogue task must land in `todo` — visible and actionable, in the group that claims the
   * least — rather than vanishing, and the header count must still include it. That is
   * `check-problems.mjs`'s regression restated: an unrecognised value used to zero every
   * counter, so the panel claimed a clean workspace above rows the user could see.
   */
  'rogue-status': story({ board: ready([T14, T15, T16, ROGUE]) }),

  /*
   * The recency order, in markup. The file holds T31 → T32 → T33, oldest activity first, and
   * the check asserts the rows come out reversed — see the constants' comment for why the
   * story is a statement about the sort and not about the fixture.
   */
  'list-recent-first': story({ board: ready([T31, T32, T33]) }),

  /* ------------------------------------------------------------------ the delete gesture */

  /*
   * `list`, with `t-14`'s delete armed. The list is otherwise identical, which is what makes
   * the pair of assertions — unarmed carries no destructive label, armed does — a statement
   * about the arming rather than about the screen.
   */
  'list-delete-armed': story({ board: ready([T14, T15, T16, T17]), deleteArmed: ARM }),

  /*
   * **The one that has to draw nothing.** The same arming against a board that has moved on by
   * one rev. `armedDelete` refuses it, so the row shows its ordinary × again and the confirming
   * press is not on screen — a second click cannot land on a board the user never agreed to.
   */
  'list-delete-stale': story({ board: ready([T14, T15, T16, T17]), deleteArmed: STALE_ARM }),

  /* ------------------------------------------------------------------------- the filter */

  /*
   * The board of `list`, narrowed to Doing. One task survives; the other three must be absent
   * from the markup entirely rather than merely dimmed, and the header figure must still count
   * all four — see `metaFigure`'s doc for why it does not follow the filter.
   */
  'list-filtered': story({ board: ready([T14, T15, T16, T17]), filter: 'doing' }),

  /*
   * A status this build cannot read, under the filter it is *drawn* in.
   *
   * `groups` puts a rogue status in Todo. If the filter matched on the raw value instead of on
   * `groupOf`, filtering to Todo would hide a task the unfiltered list had just shown under
   * Todo — the tracker losing a row, arrived at from the filter's side.
   */
  'list-filtered-rogue': story({ board: ready([T14, T15, T16, ROGUE]), filter: 'todo' }),

  /*
   * A tracker with tasks in it and none in the chosen group.
   *
   * The screen must not be the empty tracker's: *"No tasks yet"* over three real tasks is how a
   * user concludes their board is gone. The filter row stays drawn, because a filter that hid
   * its own control would leave no way back.
   */
  'list-filter-no-match': story({ board: ready([T14, T15, T16]), filter: 'done' }),

  /* ------------------------------------------------------------------------- the search */

  /*
   * The board of `list`, narrowed by text. One task survives (t-14, whose title and body both say
   * `retry`); the other three must be absent from the markup entirely, and the header figure must
   * still count all four, for the filter's reason.
   *
   * **`matched` beside `query`, since M68.** The narrowing is `task_search`'s answer now — the
   * board carries no bodies, so the rule runs in Rust and `cide-tasks` pins the rule itself
   * (case-insensitivity included; it used to be pinned here by the capital `Retry`). What this
   * story pins is the *markup* consequence, which is the half Rust cannot see: a narrowed list
   * draws fewer rows and an undiminished count.
   */
  'list-searched': story({
    board: ready([T14, T15, T16, T17]),
    query: 'Retry',
    matched: new Set(['t-14']),
  }),

  /*
   * A tracker with tasks in it and a query none of them contain.
   *
   * The screen must not be the empty tracker's, and it must not be the *filter's* either: the
   * way out of this one is clearing what was typed, so the sentence names the query and the
   * actions offer Clear search. The box itself stays drawn — a search that hid its own control
   * on a miss would leave no way to clear it.
   */
  'list-search-no-match': story({
    board: ready([T14, T15, T16]),
    query: 'quaternion',
    // An **empty set**, not `null`: the search ran and matched nothing. `null` would be "the
    // answer is not in yet", which draws no sentence at all — see `list-search-pending`.
    matched: new Set<string>(),
  }),

  /*
   * A query typed whose answer has not landed. (M68)
   *
   * The state between a keystroke and `task_search` returning, and it has to be its own story
   * because the tempting collapse — read "no answer yet" as "nothing matched" — prints
   * *No tasks match "quaternion"* about a search that has not run, over a board that may well
   * contain it. One frame is plenty: it is the frame the user is looking at while they type.
   *
   * So the whole board is drawn, unnarrowed, and no empty screen appears. The box keeps the text.
   */
  'list-search-pending': story({
    // `list`'s own board, so the render check can assert the row list is *byte-identical* to the
    // unnarrowed one — a stronger statement of "narrows nothing" than any count.
    board: ready([T14, T15, T16, T17]),
    query: 'quaternion',
    matched: null,
  }),
}

/* ============================================================================== the card */

/**
 * The card's named states — `TaskDetail`, which is [`TaskDetailModal`] minus the one wrapper a
 * server render cannot follow.
 *
 * The pairs are what carry the assertions, the same trick `list` / `list-with-live-run` uses:
 *
 *  - **`card` against `card-editing-title`.** One board, one task, one prop apart. At rest the
 *    card must contain **no control in any field**; with `editing` set, exactly one field must
 *    have one and the other two must still be text. "One field at a time" is not a claim that
 *    can be made about a single story.
 *  - **`card` against `card-bare`.** The same card over a task whose title, body and assignee
 *    are all empty. `restText` is total and never empty, so every field still draws a row — a
 *    card that collapsed them would look like one that failed to render.
 *  - **`card-editing-assignee` against the other two editors.** Its editor is a `<select>` and
 *    it draws **no Save**, because choosing is the commit; the other two draw one. A uniform
 *    Save beside a select would be a button with nothing left to do.
 */
export type CardStoryName =
  | 'card'
  | 'card-spec-reading'
  | 'card-spec-failed'
  | 'card-spec-session'
  | 'card-spec-session-awaiting'
  | 'card-spec-session-closed'
  | 'card-spec-ready'
  | 'card-spec-picking'
  | 'card-spec-accepting'
  | 'card-spec-archive-only'
  | 'card-spec-archived'
  | 'card-spec-archived-session'
  | 'card-propose'
  | 'card-bare'
  | 'card-with-live-run'
  | 'card-editing-title'
  | 'card-editing-assignee'
  | 'card-assignee-no-roles'
  | 'card-editing-body'
  | 'card-delete-armed'
  | 'card-markdown'
  | 'card-linked'
  | 'card-link-adding'
  | 'card-attachments'
  | 'card-attachments-readonly'
  | 'card-attach-empty'
  | 'card-composer-staged'

/* ---------------------------------------------------------------- attachment fixtures (M39) */

function attachment(
  over: Partial<AttachmentView> & Pick<AttachmentView, 'id' | 'name' | 'kind'>,
): AttachmentView {
  return {
    bytes: 24_576,
    addedBy: { kind: 'user' },
    addedMs: NOW_MS - 300_000,
    ...over,
  }
}

const ATTACHED = detail({
  id: 't-23',
  title: 'Match the mock',
  body: 'The header is 2px too tall against the design. Screenshots attached.',
  status: 'doing',
  agent: 'developer',
  attachments: [
    attachment({ id: 'a-ready', name: 'mock.png', kind: 'image', bytes: 184_320 }),
    attachment({ id: 'a-pending', name: 'actual.png', kind: 'image' }),
    attachment({ id: 'a-refused', name: 'not-really.png', kind: 'image' }),
    attachment({ id: 'a-log', name: 'build.log', kind: 'file', bytes: 1_532 }),
  ],
  comments: [
    comment({
      text: 'Fixed; the header now measures as the mock.',
      atMs: NOW_MS - 120_000,
      author: { kind: 'agent', agent: 'developer', label: 'Developer' },
      attachments: [
        attachment({
          id: 'a-after',
          name: 'after.png',
          kind: 'image',
          addedBy: { kind: 'agent', agent: 'developer', label: 'Developer' },
          addedMs: NOW_MS - 120_000,
        }),
      ],
    }),
  ],
})

/* The URLs are what the asset protocol would hand back; a server render draws the `src` and
   never fetches it, which is exactly the boundary the strip is meant to respect. */
const PREVIEWS: Readonly<Record<string, AttachmentPreview>> = {
  'a-ready': { kind: 'ready', url: 'asset://localhost/repo/.cide/attachments/t-23/a-ready/mock.png' },
  'a-refused': { kind: 'refused', reason: 'not-really.png: the bytes are not an image' },
  'a-after': { kind: 'ready', url: 'asset://localhost/repo/.cide/attachments/t-23/a-after/after.png' },
}

const STAGED: readonly StagedAttachment[] = [
  { path: '/home/u/shots/Pasted image 2026-09-04 at 10.02.11.png', name: 'Pasted image 2026-09-04 at 10.02.11.png', bytes: 88_064 },
  { path: '/home/u/notes/findings.md', name: 'findings.md', bytes: null },
]

const ATTACHMENT_HANDLERS: Partial<TaskDetailProps> = {
  onPickAttachments: () => Promise.resolve([]),
  onStageClipboard: () => Promise.resolve(null),
  onAttach: () => {},
  onAttachClipboard: () => {},
  onDetachAttachment: () => {},
  onOpenAttachment: () => {},
  onRevealAttachment: () => {},
  onViewAttachment: () => {},
  onComposerStaged: () => {},
}

export const CARD_STORIES: Record<CardStoryName, TaskDetailProps> = {
  /*
   * At rest. Every field read-only, three pencils, one live status segment, and the log — from
   * the same scrambled `COMMENTS` the panel has always been asked to print straight.
   */
  card: card({ task: T14 }),

  /*
   * ------------------------------------------------------------- the change, before it is read
   *
   * A task linked to an OpenSpec change, with the read still in flight.
   *
   * This is a *state*, and it was drawn as an absence. Reading a change is four `openspec`
   * invocations and every one is a node process — six tenths of a second now that they run at
   * once, two and a half before — and for all of it the card drew nothing where the block would
   * be. A task with a change therefore opened looking exactly like a task without one, with
   * nothing on screen saying OpenSpec was being read, and then grew a whole section.
   *
   * `card-spec-failed` is its pair, and the pair is the point: `spec === null` on its own means
   * *still reading*, and the same value with a problem means *the read finished and there is
   * nothing*. Collapsing them is a spinner that never stops.
   */
  'card-spec-reading': card({ task: SPEC_TASK, spec: null }),

  'card-spec-failed': card({
    task: SPEC_TASK,
    spec: null,
    specProblem: 'add-dark-mode could not be read. It may have been archived.',
  }),

  /*
   * The read finished and there is a change. (M28)
   *
   * The story the two above were missing, and its absence is why a regression shipped: with no
   * fixture that ever carried a *loaded* spec, nothing in the suite had rendered the dispatch
   * bar at all, so a card that drew everything except its most consequential control passed
   * every check.
   */
  'card-spec-ready': card({
    task: SPEC_TASK,
    spec: SPEC_CARD,
    dispatchTargets: TARGETS,
    dispatchOpen: false,
  }),

  /*
   * ------------------------------------------------- handed to a conversation, three states
   *
   * `Task::session` is a **record of where the work went** and survives the pane closing, so
   * whether a conversation is still open is a fact about the workspace tree looked up at render.
   * Three states and not two, `validityLabel`'s rule: *closed* is not a quieter shade of *not
   * waiting* — it is work with nowhere to continue, and it is the only one of the three that
   * names a next step.
   */
  'card-spec-session': card({
    task: { ...SPEC_TASK, session: '11111111-2222-4333-8444-555555555555' },
    spec: SPEC_CARD_SESSION,
    dispatchTargets: TARGETS,
    dispatchOpen: false,
  }),

  'card-spec-session-awaiting': card({
    task: { ...SPEC_TASK, session: '11111111-2222-4333-8444-555555555555' },
    spec: { ...SPEC_CARD_SESSION, session: { ...SPEC_CARD_SESSION.session, awaiting: true } },
    dispatchTargets: TARGETS,
    dispatchOpen: false,
  }),

  'card-spec-session-closed': card({
    task: { ...SPEC_TASK, session: '11111111-2222-4333-8444-555555555555' },
    spec: { ...SPEC_CARD_SESSION, session: { ...SPEC_CARD_SESSION.session, open: false } },
    dispatchTargets: TARGETS,
    dispatchOpen: false,
  }),

  /* The picker open, which is what Approve does rather than assigning on the spot. */
  'card-spec-picking': card({
    task: SPEC_TASK,
    spec: SPEC_CARD,
    dispatchTargets: TARGETS,
    dispatchOpen: true,
  }),

  /*
   * The accept, mid-flight. (M28)
   *
   * `Integrate & Archive` merges the agent's branch, runs `openspec archive`, re-validates and
   * closes the task — and it shipped drawing **nothing** while all of that ran, so a card that
   * had taken the press was indistinguishable from one that had not. This story is the spinner
   * and the present participle, and the button inert so a second press cannot start a second
   * merge.
   */
  'card-spec-accepting': card({
    task: SPEC_TASK,
    spec: {
      ...SPEC_CARD,
      done: 9,
      action: {
        id: 'accept' as const,
        label: 'Integrate & Archive',
        hint: "Merges the agent's branch, then merges these requirement edits into openspec/specs/ and closes the task.",
        enabled: true,
        reason: '',
      },
    },
    specBusy: true,
  }),

  /*
   * The accept over work that is already on the user's own branch. (M31)
   *
   * The pair with `card-spec-accepting`, and the whole of what this story asserts is the two
   * words the button does **not** say. A task handed to a Claude conversation carries no role —
   * `TaskEdit::SetSession` clears `Task::agent` — so `plan_accept` finds no `cide/<role>-<task>`
   * and `spec_accept` merges nothing: the press is a plain archive. The card nonetheless read
   * *Integrate & Archive*, over a conversation that had spent the afternoon committing every
   * batch of its work onto the branch the user was standing on. It named a step it would not
   * take, and the reader's conclusion was the correct one: there is nothing to integrate.
   *
   * The session row rather than `SPEC_CARD`'s null one, because that is the shape this arises
   * in — and it is what makes the story legible as *the work is over there, and it is already
   * yours* rather than as an arbitrary relabelling.
   */
  'card-spec-archive-only': card({
    task: { ...SPEC_TASK, agent: null, session: '11111111-2222-4333-8444-555555555555' },
    spec: {
      ...SPEC_CARD_SESSION,
      done: 9,
      action: {
        id: 'accept' as const,
        label: 'Archive',
        hint: 'Merges these requirement edits into openspec/specs/ and closes the task. This work was done in your own checkout, so there is no branch to merge.',
        enabled: true,
        reason: '',
      },
    },
  }),

  /*
   * The change after it was accepted. (M28)
   *
   * The card said *"add-dark-mode could not be read. It may have been archived."* here, from the
   * moment the work landed onwards — because `openspec archive` moves the directory and no CLI
   * command reads the result, so the task that did the work lost its record at exactly the point
   * the record was worth keeping.
   *
   * cide reads the directory itself now, and the story is as much about what is **not** drawn as
   * what is: the three fields an archive cannot answer come back `0/0` and vacuously valid, which
   * rendered naively is an empty progress bar and a green *Valid* over finished, merged work. So
   * there is no bar, the badge reads `Archived`, and the summary line names the directory.
   */
  'card-spec-archived': card({
    task: { ...SPEC_TASK, status: 'done' },
    spec: {
      ...SPEC_CARD,
      done: 0,
      total: 0,
      tasks: [],
      valid: true,
      issues: 0,
      archived: '2026-08-27-add-dark-mode',
      action: {
        id: 'none' as const,
        label: '',
        hint: 'Archived as openspec/changes/archive/2026-08-27-add-dark-mode/. Its requirements are in openspec/specs/ now.',
        enabled: false,
        reason: 'this change has been archived',
      },
    },
  }),

  /*
   * An archived change whose work went to a conversation. (M31)
   *
   * The story the archive arm was missing, and its absence is exactly why a regression shipped:
   * `card-spec-archived` is built on `SPEC_CARD`, whose `session` is `null`, so nothing in the
   * suite had ever drawn the session row and the archive line together. What the user saw was
   * both at once —
   *
   *     Archived
   *     archived as 2026-08-27-add-todo-list
   *     Working                       Conversation a7d4fd80   Resume  Open  Hand it elsewhere
   *     It is working. The checklist above ticks as it goes.
   *
   * — a live-work claim under the card's own *Archived* line, a sentence about a checklist this
   * arm deliberately draws no bar for, and two controls that dispatch against a change directory
   * `openspec archive` has moved. `primaryAction` had refused this arm since M28; the row was
   * the second door onto the same gesture and had none of that reasoning.
   *
   * The pair with `card-spec-session` is the claim, and it has to be a pair: the *same* open,
   * not-awaiting conversation reads `Working` there and `Did this work` here, so the assertion
   * is about the archive and cannot pass by the row having gone quiet for some other reason.
   */
  'card-spec-archived-session': card({
    task: { ...SPEC_TASK, status: 'done', session: '11111111-2222-4333-8444-555555555555' },
    spec: {
      ...SPEC_CARD_SESSION,
      done: 0,
      total: 0,
      tasks: [],
      valid: true,
      issues: 0,
      archived: '2026-08-27-add-dark-mode',
      action: {
        id: 'none' as const,
        label: '',
        hint: 'Archived as openspec/changes/archive/2026-08-27-add-dark-mode/. Its requirements are in openspec/specs/ now.',
        enabled: false,
        reason: 'this change has been archived',
      },
    },
    dispatchTargets: TARGETS,
  }),

  /*
   * A task with no change, on a project that *could* have one. (M28)
   *
   * The replacement for the compose dialog's *New change from this task*, which called
   * `spec_propose` and scaffolded a stub — no deltas, no checklist — so the task was born linked
   * to a change `openspec validate` refuses. Writing a proposal needs the codebase, so it is a
   * conversation's job; this button starts one on it.
   *
   * The pair that matters is this story against `card`, which passes no handler and must draw no
   * such button: a project with no `openspec/`, or with no propose command installed, sees the
   * card exactly as it was before M28.
   */
  'card-propose': card({ task: T14, onProposeChange: () => {} }),

  /* No title, no body, nobody assigned. Three placeholders, three rows, nothing collapsed. */
  'card-bare': card({ task: BARE }),

  /* A run working on this task: the live-run strip, and its Open control. */
  'card-with-live-run': card({ task: T14, runs: LIVE }),

  /*
   * One field in edit, with a draft that **differs** from the task's title — so the story is
   * dirty, which is the state every "what happens to it" rule in `model.ts` is about.
   */
  'card-editing-title': card({ task: T14, editing: TITLE_DRAFT }),

  /* The select, which commits on choice and therefore draws no Save. Clean: it was just opened. */
  'card-editing-assignee': card({ task: T14, editing: ASSIGNEE_EDIT }),

  /*
   * The same editor over an empty roster — a project with subagents off. The host now derives
   * `roles` from the agents store, and this is the state the assignee dropdown shipped in for
   * one whole milestone by accident; the hint is what keeps a legitimately-short list from
   * reading as that bug, and the check asserts the sentence is on screen.
   */
  'card-assignee-no-roles': card({
    task: T14,
    editing: ASSIGNEE_EDIT,
    roles: {},
    assigneeHint: assigneeHint('disabled'),
  }),

  'card-editing-body': card({ task: T14, editing: BODY_DRAFT }),

  /*
   * The card's half of the delete gesture, so neither call site can lose its confirmation alone.
   * It stays a two-click arming inside the modal rather than becoming a second dialog.
   */
  'card-delete-armed': card({ task: T14, deleteArmed: true }),

  /*
   * A body and a comment written the way agents actually write them: headings, emphasis, code
   * spans, a fence, lists, a link. (M27)
   *
   * The check asserts two things off this story: that the syntax **renders** — a `<strong>` in
   * the markup, not two asterisks in the text — and that it renders as *elements*, which
   * server-rendered markup can show and an HTML-string renderer never would. The raw-marker
   * greps (`**`, `` ``` ``) run over the flattened text, so this fixture must not use either
   * sequence anywhere it is meant to survive as prose.
   */
  'card-markdown': card({ task: RICH }),

  /*
   * The Links section, at rest. (M30) Five chips: three outgoing (one of them dangling and
   * marked), and two derived incoming readings — `Blocks t-41` from the other task's own
   * stored edge, plus nothing for `related` because `t-17` stores no edge back. The removes:
   * the outgoing chips and the related pair carry an ✕; the derived directed chip does not,
   * because that edge belongs to the other task and the chip is the road there.
   */
  'card-linked': card({
    task: LINKED,
    links: LINK_CHIPS,
    linkTargets: LINK_TARGETS,
    ...LINK_HANDLERS,
  }),

  /* The same card with the add picker open: kind and target selects, and the gated Add. */
  'card-link-adding': card({
    task: LINKED,
    links: LINK_CHIPS,
    linkTargets: LINK_TARGETS,
    linkAdd: { kind: 'blockedBy' },
    ...LINK_HANDLERS,
  }),

  /*
   * ------------------------------------------------------------------------ attachments (M39)
   *
   * Four files on the body — one image Rust has vouched for, one it is still vouching for, one
   * it refused, and a log — and one image on a comment. The three preview states are the
   * point: each has to look like something, and a check that only ever saw `ready` would pass
   * a card that drew a refused thumbnail as a blank box. With every handler, so the paperclip,
   * the per-comment Attach and the Remove are all on screen and counted as writes.
   */
  'card-attachments': card({
    task: ATTACHED,
    previews: PREVIEWS,
    ...ATTACHMENT_HANDLERS,
  }),

  /*
   * The same task with no handlers: a build with no attachment commands, or a board that
   * cannot be written. The strip is still drawn — the files exist — and nothing on it writes.
   */
  'card-attachments-readonly': card({ task: ATTACHED, previews: PREVIEWS }),

  /* A task with nothing attached, on a host that can attach: the row is one paperclip. */
  'card-attach-empty': card({ task: T14, ...ATTACHMENT_HANDLERS }),

  /* Two files picked or pasted into the composer and not yet submitted. */
  'card-composer-staged': card({
    task: T14,
    composerStaged: STAGED,
    ...ATTACHMENT_HANDLERS,
  }),
}


/**
 * The compose dialog's named states — `TaskCompose`, which is `TaskComposeModal` minus the one
 * wrapper a server render cannot follow. (M21)
 *
 * Three stories, and the pairs are again what carry the assertions:
 *
 *  - **`compose-empty` against `compose-filled`.** Same dialog, one prop apart. Empty, Create
 *    must be **present and disabled**: `draftReady` refuses a task with no title, because
 *    `cide_tasks::validate` does, and a Create that vanished until the first keystroke is harder
 *    to understand than one visibly waiting. Filled, the same button must be live. "Disabled
 *    until ready" is not a claim that can be made about a single story.
 *  - **`compose-busy` against `compose-filled`.** A ready draft with the write in flight: the
 *    dialog is still up and Create is inert. A dialog that closed on the click would take the
 *    user's paragraph with it the first time a write failed, and there is nothing on disk to
 *    recover it from.
 *
 * Every story draws four live controls. That is the assertion the whole change is about — the
 * report was *"all fields are editable and task isn't created while not press Create"* — and it
 * is the exact inverse of the card's, which must draw **none** at rest.
 */
export type ComposeStoryName =
  | 'compose-empty'
  | 'compose-filled'
  | 'compose-busy'
  | 'compose-no-roles'
  | 'compose-linking'
  | 'compose-attachments'

/** A draft with something in every field, including a status that is not the default. */
const FILLED_DRAFT: TaskDraft = {
  title: 'Teach the watcher about .cide/',
  status: 'doing',
  assignee: 'qa',
  body: 'The filter rejects dot-prefixed components before any gitignore matcher runs.',
  change: '',
  links: [],
  attachments: [],
}

export const COMPOSE_STORIES: Record<ComposeStoryName, TaskComposeProps> = {
  /* Nothing typed. Four controls, and a Create that is drawn and refuses. */
  'compose-empty': compose(EMPTY_DRAFT),

  /* Every field carrying a value, including a starting status the user chose. */
  'compose-filled': compose(FILLED_DRAFT),

  /* Create pressed, the write in flight. Still on screen, still holding the draft. */
  'compose-busy': compose(FILLED_DRAFT, { busy: true }),

  /* An empty roster: one Unassigned option, and the sentence saying why — never a bare list. */
  'compose-no-roles': compose(EMPTY_DRAFT, {
    roles: {},
    assigneeHint: assigneeHint('disabled'),
  }),

  /*
   * The links row, drawn and holding one picked edge. (M30) The row exists only because the
   * `tasks` prop is present — `compose-empty` and `compose-filled` pass none, which is the
   * pre-M30 digest claim, the `changes` row's own arrangement — and the picked link renders as
   * a chip with its remove beside it, above the two selects ready for the next pick.
   */
  'compose-linking': compose(
    { ...FILLED_DRAFT, links: [{ kind: 'blockedBy', target: 't-14' }] },
    {
      tasks: [
        { id: 't-14', title: 'Add the retry bar', status: 'doing' },
        { id: 't-16', title: 'Write check-agents', status: 'todo' },
      ],
    },
  ),

  /* Two files staged for the create, on a host that can pick. (M39) */
  'compose-attachments': compose(
    { ...FILLED_DRAFT, attachments: STAGED },
    { onPickAttachments: () => Promise.resolve([]), onStageClipboard: () => Promise.resolve(null) },
  ),
}

/**
 * The @mention popup's named states — `MentionList`, which is the open half `MentionTextarea`
 * only ever mounts from DOM events a server render cannot fire. Options come through the real
 * `mentionOptions`, so the scored order on screen is the scored order the model produces.
 */
export type MentionStoryName = 'mention-open' | 'mention-filtered'

const MENTION_ROLES: Readonly<Record<string, string>> = {
  developer: 'Developer',
  qa: 'QA',
  'code-reviewer': 'Code Reviewer',
}

export const MENTION_STORIES: Record<
  MentionStoryName,
  { options: readonly MentionOption[]; selected: number; listboxId: string }
> = {
  /* Everything on offer, second row highlighted — exactly one `aria-selected`. */
  'mention-open': {
    options: mentionOptions(MENTION_ROLES, ''),
    selected: 1,
    listboxId: 'mention-story-open',
  },
  /* A query narrowing to one role. */
  'mention-filtered': {
    options: mentionOptions(MENTION_ROLES, 'dev'),
    selected: 0,
    listboxId: 'mention-story-filtered',
  },
}

/**
 * The link-target popup's named states — `LinkTargetList`, which is the half `LinkTargetInput`
 * only ever opens from DOM events a server render cannot fire. (M30) `MENTION_STORIES`'
 * arrangement, and the options come through the real `linkTargetOptions`, so the tier order on
 * screen is the tier order the model produces.
 */
export type LinkTargetStoryName = 'link-target-open' | 'link-target-filtered'

export const LINK_TARGET_STORIES: Record<
  LinkTargetStoryName,
  { options: readonly LinkTargetOption[]; selected: number; listboxId: string }
> = {
  /* The empty query: the whole board in panel order, second row highlighted. */
  'link-target-open': {
    options: linkTargetOptions('', LINK_TARGETS),
    selected: 1,
    listboxId: 'link-target-story-open',
  },
  /* A query that reaches by the summary — `retry` finds t-14 by its title. */
  'link-target-filtered': {
    options: linkTargetOptions('retry', LINK_TARGETS),
    selected: 0,
    listboxId: 'link-target-story-filtered',
  },
}
