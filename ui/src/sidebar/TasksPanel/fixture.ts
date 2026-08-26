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
  type ArmedDelete,
  type Board,
  type CommentView,
  type FieldEdit,
  type RunRef,
  type TaskDraft,
  type TaskStatus,
  type TaskView,
} from './model'
import { mentionOptions, type MentionOption } from './mentionModel'
import type { TasksPanelViewProps } from './TasksPanel'
import type { TaskDetailProps } from './TaskDetail'
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

function task(over: Partial<TaskView> & Pick<TaskView, 'id' | 'title'>): TaskView {
  return {
    body: '',
    status: 'todo',
    agent: null,
    comments: [],
    history: [],
    // The user, by default, because that is what the panel's own New task button produces. The
    // stories that need the other answer say so; see `BARE`, which is the card's half of the pair
    // that pins the creator to the author enum rather than to a name.
    createdBy: { kind: 'user' },
    createdMs: NOW_MS - 86_400_000,
    updatedMs: NOW_MS - 600_000,
    ...over,
  }
}

const T14 = task({
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

const T15 = task({
  id: 't-15',
  title: 'Sweep the phase table',
  status: 'review',
  agent: 'developer',
  // A subagent decomposed this one into existence. The third arm of the author union, so all
  // three are somewhere in the fixtures rather than only the two the card stories draw.
  createdBy: { kind: 'agent', agent: 'developer', label: 'Developer' },
})

const T16 = task({ id: 't-16', title: 'Write check-agents' })

const T17 = task({
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
const ROGUE = task({
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
const RICH = task({
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
const T31 = task({ id: 't-31', title: 'Quiet since this morning', updatedMs: NOW_MS - 7_200_000 })
const T32 = task({ id: 't-32', title: 'Stirred an hour ago', updatedMs: NOW_MS - 3_600_000 })
const T33 = task({ id: 't-33', title: 'Touched a minute ago', updatedMs: NOW_MS - 60_000 })

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
} as const

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
 * A task with nothing in any of the three editable fields, and the reason it is a story.
 *
 * `restText` is total and never returns an empty string, and *that* is the property this fixture
 * exists to render: a field whose value is absent still draws a row with a placeholder in it. A
 * card that collapsed those rows would be indistinguishable on screen from one that failed to
 * render them.
 */
const BARE = task({
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
   * The board of `list`, narrowed by text. `Retry` — capitalised, over a title that says
   * `retry` — so the story pins case-insensitivity in the markup, not only in the model. One
   * task survives (t-14, whose title and body both say it); the other three must be absent
   * from the markup entirely, and the header figure must still count all four, for the
   * filter's reason.
   */
  'list-searched': story({ board: ready([T14, T15, T16, T17]), query: 'Retry' }),

  /*
   * A tracker with tasks in it and a query none of them contain.
   *
   * The screen must not be the empty tracker's, and it must not be the *filter's* either: the
   * way out of this one is clearing what was typed, so the sentence names the query and the
   * actions offer Clear search. The box itself stays drawn — a search that hid its own control
   * on a miss would leave no way to clear it.
   */
  'list-search-no-match': story({ board: ready([T14, T15, T16]), query: 'quaternion' }),
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
  | 'card-bare'
  | 'card-with-live-run'
  | 'card-editing-title'
  | 'card-editing-assignee'
  | 'card-assignee-no-roles'
  | 'card-editing-body'
  | 'card-delete-armed'
  | 'card-markdown'

export const CARD_STORIES: Record<CardStoryName, TaskDetailProps> = {
  /*
   * At rest. Every field read-only, three pencils, one live status segment, and the log — from
   * the same scrambled `COMMENTS` the panel has always been asked to print straight.
   */
  card: card({ task: T14 }),

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

/** A draft with something in every field, including a status that is not the default. */
const FILLED_DRAFT: TaskDraft = {
  title: 'Teach the watcher about .cide/',
  status: 'doing',
  assignee: 'qa',
  body: 'The filter rejects dot-prefixed components before any gitignore matcher runs.',
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
