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
  Task as WireTask,
  TaskAuthor as WireAuthor,
  TaskBoard as WireBoard,
  TaskComment as WireComment,
} from '@/ipc/client'
import type { Board, CommentAuthor, CommentView, TaskView } from './model'

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

function comment(from: WireComment): CommentView {
  return {
    id: from.id,
    author: author(from.author),
    text: from.text,
    atMs: ms(from.atUnixMs),
    editedMs: from.editedAtUnixMs === null ? null : ms(from.editedAtUnixMs),
  }
}

function task(from: WireTask): TaskView {
  return {
    id: from.id,
    title: from.title,
    body: from.body,
    status: from.status,
    // The role this task is **for**. `AgentId | null` on the wire and `string | null` here:
    // `model.ts` cannot name `AgentId`, and the distinction it would draw is not one the panel
    // makes — `agentChip` looks the id up in a plain map and falls back to printing it.
    agent: from.agent,
    /*
     * Tombstones are dropped here, so nothing above this line knows they exist. (M21)
     *
     * A deleted comment stays *in the file* — one merely removed comes back on the next merge
     * with a stale copy, which is a delete that does not delete, and `TaskComment::deleted` has
     * the argument. That is a storage concern and the panel has no use for it, so the boundary
     * is this function. Filtering in the view instead would leave `CommentView` carrying a flag
     * every consumer had to remember, and the first one to forget would draw an empty log entry
     * where a comment used to be.
     */
    comments: from.comments.filter((c) => !c.deleted).map(comment),
    createdMs: ms(from.createdUnixMs),
    updatedMs: ms(from.updatedUnixMs),
  }
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
      return { kind: 'ready', tasks: from.tasks.map(task), rev: ms(from.rev) }
  }
}
