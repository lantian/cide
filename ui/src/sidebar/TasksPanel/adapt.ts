/**
 * The wire's `TaskBoard` into the panel's `Board`. (M18)
 *
 * # Why there are two of them at all
 *
 * `model.ts` **imports nothing** — that is what lets `ui/scripts/check-agents.mjs` compile it
 * alone with a bare `tsc` and drive it under node, which together with the render check is this
 * panel's whole test story. So it cannot import `generated.ts` and restates the shapes
 * structurally instead. This module is the one place the two meet, and it is the only file in
 * the directory allowed to import both. `ProblemsPanel/adapt.ts` does the identical job for
 * diagnostics and this follows it.
 *
 * # What actually differs, and why none of it is laziness
 *
 * * **`bigint` → `number`.** `rev`, `createdUnixMs`, `updatedUnixMs` and `atUnixMs` are `u64` in
 *   Rust, and ts-rs renders a `u64` as **`bigint`**. Every one of them is a `number` in
 *   `model.ts`, and that file's header argues why: mixing a `bigint` with a `number` in a
 *   comparison throws a `TypeError` rather than coercing, so a single stray `bigint` reaching
 *   `newerBoard` or `commentOrder` would throw inside a render — and React 19 unmounts the whole
 *   tree on a throw out of a render, which is a blank window rather than a wrong date.
 *
 *   `Number()` on a millisecond timestamp is **exact**: a `number` holds every integer up to
 *   2^53 - 1, which as Unix milliseconds is the year 287396. `rev` is a counter on one file and
 *   is not going to reach 9 quadrillion either. Nothing is lost and nothing is rounded; the
 *   conversion is total.
 *
 *   It is also total in the *other* direction, which is worth knowing before somebody "fixes" it
 *   with a `typeof` branch: the value that actually arrives over Tauri's IPC is a JSON number,
 *   because serde writes a `u64` as one, so `bigint` is what the bindings *say* rather than what
 *   the wire carries. `Number()` is then the identity function. Converting unconditionally is
 *   what makes this module correct under either reading — and the type is the thing to trust,
 *   because the day a payload does arrive as a `bigint` the comparison in `newerBoard` throws.
 *
 * * **`T | null` on the wire against the model's own optionality.** Every outbound optional in
 *   `cide-ipc` is `T | null` rather than an absent field, because `#[ts(optional)]` changes only
 *   the emitted TypeScript and not what serde writes. Under `exactOptionalPropertyTypes` those
 *   are genuinely different types, so each one is restated rather than passed through — see
 *   `agent` below, which is the only nullable field the board actually has.
 *
 * Everything else is a structural copy, and that is the point: this is a conversion, not a
 * second model. The moment it grows a *decision* — a default, a fallback, a piece of wording —
 * that decision belongs in `model.ts`, where the check script can reach it.
 *
 * # Field by field, never `as`
 *
 * The objects are built out explicitly instead of being cast or spread. A cast would compile
 * today and go on compiling after a Rust field was renamed, which is the exact drift
 * `cargo xtask codegen --check` exists to catch one layer down; writing the fields out means the
 * rename fails *here*, in the seam that is supposed to know about it.
 */
import type {
  TaskAttachment as WireAttachment,
  TaskAuthor as WireAuthor,
  TaskBoard as WireBoard,
  TaskComment as WireComment,
  TaskDetail as WireDetail,
  TaskRow as WireRow,
  TaskStatusChange as WireStatusChange,
} from '@/ipc/client'
import type {
  AttachmentView,
  Board,
  CommentAuthor,
  CommentView,
  StatusChangeView,
  TaskDetailView,
  TaskView,
} from './model'

/**
 * `bigint` → `number`, in one place so the argument above is made once.
 *
 * Exact for every value either side of this seam carries; see the header.
 */
function ms(value: bigint): number {
  return Number(value)
}

/**
 * Who wrote a comment.
 *
 * Restated arm by arm rather than passed through. The two unions are structurally identical
 * today, and a `switch` is what makes a variant added in Rust — or a field added to `agent` —
 * a compile error here instead of a value that silently keeps the old shape.
 */
function author(from: WireAuthor): CommentAuthor {
  switch (from.kind) {
    case 'user':
      return { kind: 'user' }
    case 'orchestrator':
      return { kind: 'orchestrator' }
    case 'agent':
      return { kind: 'agent', agent: from.agent, label: from.label }
  }
}

/** One file. (M39) The same `bigint` and author conversions a comment gets. */
function attachment(from: WireAttachment): AttachmentView {
  return {
    id: from.id,
    name: from.name,
    bytes: ms(from.bytes),
    kind: from.kind,
    addedBy: author(from.addedBy),
    addedMs: ms(from.addedUnixMs),
  }
}

/**
 * Live attachments only. (M39) Tombstones are dropped here for exactly the comments' reason
 * below: the record stays in the file so a merge cannot resurrect a detached file, and no
 * renderer has any use for one.
 */
function attachments(from: readonly WireAttachment[]): AttachmentView[] {
  return from.filter((a) => !a.deleted).map(attachment)
}

function comment(from: WireComment): CommentView {
  return {
    id: from.id,
    author: author(from.author),
    text: from.text,
    atMs: ms(from.atUnixMs),
    editedMs: from.editedAtUnixMs === null ? null : ms(from.editedAtUnixMs),
    attachments: attachments(from.attachments),
  }
}

/** One status transition. (M27) The same `bigint` and author conversions a comment gets. */
function statusChange(from: WireStatusChange): StatusChangeView {
  return {
    from: from.from,
    to: from.to,
    by: author(from.by),
    atMs: ms(from.atUnixMs),
  }
}

function row(from: WireRow): TaskView {
  return {
    id: from.id,
    title: from.title,
    status: from.status,
    // The role this task is **for**. `AgentId | null` on the wire and `string | null` here:
    // `model.ts` cannot name `AgentId`, and the distinction it would draw is not one the panel
    // makes — `agentChip` looks the id up in a plain map and falls back to printing it.
    agent: from.agent,
    change: from.change ?? null,
    /*
     * `== null`, which catches **both** `null` and `undefined`, and the distinction cost a bug
     * that broke every task on the board.
     *
     * `TaskRow::session` pairs `#[ts(optional)]` with `skip_serializing_if`, so *this* type does
     * omit the key — but `Task::session` never did, and for a release it sent `"session": null`
     * instead. A check for `undefined` alone therefore fell through to `String(null)`, which is
     * the seven-character string `"null"`: a truthy session id on every task in the project. The
     * card drew a session row reading *Conversation null*, and `primaryAction` — which refuses
     * the approve road once a session is set — took **Approve & dispatch off every OpenSpec task
     * there was**.
     *
     * So this stays `== null` even though the row's own encoding no longer needs it. The wire is
     * two producers wide now (a row and a `TaskDetail`'s row), the file under it is hand-editable
     * JSON, and a guard that covers both spellings cannot be wrong about either; one that covers
     * the current spelling only is one `skip_serializing_if` away from that bug again.
     */
    session: from.session == null ? null : String(from.session),
    /*
     * Link tombstones are dropped here, the comments' rule one field over. (M30) An unlinked
     * edge stays in the file so the merge can resolve re-links against stale copies
     * (`TaskLink::deleted` is a *toggle* — `cide_tasks::union_links` has the argument), and no
     * renderer has any use for one. The stamp goes with it: what remains is exactly what
     * `model.ts`'s `LinkView` restates.
     */
    links: from.links
      .filter((l) => !l.deleted)
      .map((l) => ({ kind: l.link, target: l.target })),
    // Who asked for it. Through the same `author` switch as a comment's, because it is the same
    // union — and a `createdBy` passed through raw would go on compiling after a variant was
    // added in Rust that this panel has never heard of.
    createdBy: author(from.createdBy),
    createdMs: ms(from.createdUnixMs),
    updatedMs: ms(from.updatedUnixMs),
    /*
     * Counts, not lists. (M68)
     *
     * Straight through from `cide_tasks::row_of`, which is the **only** place either number is
     * derived — and that is the whole reason `TaskDetail` nests its row instead of inlining it.
     * Recomputing them here from a detail's own comments would be a second answer to "how many
     * comments does this task have", and the two would differ exactly when a tombstone was
     * involved: a number nobody can see is wrong until it is next to the list it counts.
     */
    commentCount: Number(from.commentCount),
    attachmentCount: Number(from.attachmentCount),
  }
}

/**
 * One whole task, for the card: the row joined to the content `task_get` fetched. (M68)
 *
 * The tombstone filters live here and in `comment()` rather than in Rust, which is deliberate and
 * is the seam's oldest rule: `cide_tasks` keeps a deleted comment *in the file* so a merge with a
 * stale copy cannot resurrect it, and the panel has no use for one. Filtering in the view instead
 * would leave `CommentView` carrying a flag every consumer had to remember, and the first one to
 * forget would draw an empty log entry where a comment used to be.
 */
function detail(from: WireDetail): TaskDetailView {
  return {
    ...row(from.row),
    body: from.body,
    comments: from.comments.filter((c) => !c.deleted).map(comment),
    attachments: attachments(from.attachments),
    // The status log, unfiltered: a history row has no tombstone to drop — no `TaskEdit`
    // variant deletes one, which is the property that makes it an audit trail.
    history: from.history.map(statusChange),
  }
}

/**
 * One task's content as the card reads it, or `null` for a task that is no longer there. (M68)
 *
 * `null` in and `null` out: `task_get` answers `None` for an id that has gone, which happens for
 * real — a card can be open on a task another window has just deleted. The host closes itself on
 * it rather than drawing an error about an id the user never typed.
 */
export function adaptDetail(from: WireDetail | null): TaskDetailView | null {
  return from === null ? null : detail(from)
}

/**
 * The board as the panel reads it.
 *
 * Three arms in and three arms out — the model's fourth, `unknown`, is **not reachable from
 * here** and that is deliberate. `BOARD_UNKNOWN` means *nobody has looked*, which is a fact
 * about this window rather than about the file, and it is the store's to hold: `tasks.board`
 * answering `null` (a build with no such handler) or not having answered yet are both that
 * state, and neither is a `TaskBoard`. A conversion that could invent `unknown` would let a real
 * answer be turned into "we do not know", which is the one direction that must not exist.
 */
export function adaptBoard(from: WireBoard): Board {
  switch (from.kind) {
    case 'absent':
      return { kind: 'absent', hint: from.hint, path: from.path }
    case 'unreadable':
      return { kind: 'unreadable', path: from.path, error: from.error }
    case 'ready':
      return { kind: 'ready', tasks: from.tasks.map(row), rev: ms(from.rev) }
  }
}
