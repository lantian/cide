/**
 * Which task codes in a line of terminal output may be offered as links. (M60)
 *
 * `t-503` is what `cide_tasks::next_id` mints, and an agent prints it constantly — in a
 * `claude` pane's prose, in an opencode run's tool lines, in a shell's `git log`. This module is
 * the whole rule: what counts as a code, and whether a code may be underlined at all. Both
 * halves are here, and import-free, so `check:task-links` can compile and drive them standalone.
 * The second half especially: a gate that lived inside the provider's `provideLinks` would be a
 * rule in a DOM callback, which is the one place no check can reach.
 *
 * # The matcher is deliberately narrower than the file format
 *
 * `cide_tasks::well_formed_id` accepts any `[A-Za-z0-9_-]+`, so a hand-edited `.cide/tasks.json`
 * may legally hold `spike` or `bug`. Those are not linked and must not be: a matcher that looked
 * up every word of output against the board would light up prose, and would do it differently in
 * every project. Only the shape Rust mints is recognised.
 */

/**
 * How much of one logical line is scanned.
 *
 * A wrapped line can be tens of thousands of characters — a `cat` of a minified file is one
 * logical line — and this runs on a hover, inside a frame.
 */
export const MAX_LINE = 4096

export interface TaskCode {
  /** The id exactly as the line spells it. This is the string the board is looked up by. */
  readonly id: string
  /** 0-based offset of the `t`. */
  readonly start: number
  /** 0-based, exclusive. */
  readonly end: number
}

/**
 * `t-` and one to nine digits, bounded on both sides.
 *
 * The lookbehind refuses a code welded onto a word or sitting in a path — `rust-1.89`, `gpt-5`,
 * `port-5030`, `commit-t-503`, `/tmp/t-503`, `src/t-503`. `\p{L}\p{N}` rather than `A-Za-z0-9`,
 * so `Ωt-503` is refused too; a code inside a non-ASCII word is still inside a word.
 *
 * The first lookahead is what makes the digit run **maximal**, and it is the assertion this
 * pattern cannot do without. Greed alone is not enough at a cap: without it `t-5034` reads as
 * `t-503` and `t-1234567890` reads as `t-123456789` — each a link to a real and *different*
 * task, which opens the wrong card and looks entirely correct doing it. `t-5034` is a perfectly
 * good id and must match as itself; ten digits must match nothing at all.
 *
 * The second lookahead refuses a trailing `.` **only when a word character follows it**, so
 * `t-503.rs` and `t-503.md` are out while `Finished t-503.` and `t-503...` are in. A flat `.`
 * refusal would have lost the single commonest shape in agent prose — a code at the end of a
 * sentence. A trailing `/` keeps the flat refusal, because `t-503/` is `ls`-shaped whatever
 * follows it.
 *
 * Lowercase only. `T-503` would be refused by the board lookup anyway, so this is not a
 * correctness rule — it is about not underlining what cide cannot open, and about not implying
 * that `T-503` names some other kind of thing.
 */
const TASK_CODE = /(?<![\p{L}\p{N}_\-./])t-\d{1,9}(?![\p{L}\p{N}_\-/])(?!\.[\p{L}\p{N}_])/gu

/** Every task code in one logical line, in order. Offsets are into the string as given. */
export function matchTaskCodes(line: string): readonly TaskCode[] {
  const truncated = line.length > MAX_LINE
  const text = truncated ? line.slice(0, MAX_LINE) : line
  const found: TaskCode[] = []
  TASK_CODE.lastIndex = 0
  for (let m = TASK_CODE.exec(text); m !== null; m = TASK_CODE.exec(text)) {
    const start = m.index
    const end = start + m[0].length
    /*
     * A match that ends exactly at the cap on a longer line is dropped.
     *
     * Slicing `t-5034` at the cap leaves `t-503`, and the lookahead cannot see what the slice
     * removed — so the maximality rule above, which is the one thing standing between this
     * module and a link to somebody else's task, does not hold at this one offset. Dropping is
     * the only honest answer: there is no way to tell a code that ends at 4096 from one that
     * was cut there.
     */
    if (truncated && end === MAX_LINE) continue
    found.push({ id: m[0], start, end })
  }
  return found
}

/**
 * The task board, as narrowly as this module needs it.
 *
 * Structural rather than imported: `TasksPanel/model.ts`'s `Board` is the real type, and the two
 * files may not import one another — both are compiled standalone by their own checks. A `Board`
 * is assignable to this.
 */
export interface BoardLike {
  readonly kind: string
  readonly tasks?: readonly { readonly id: string }[]
}

/**
 * `Board`'s ready arm, spelled here because that type cannot be imported.
 *
 * `check:task-links` pins this literal against `model.ts`, because a rename on either side is a
 * link that silently stops being offered — no error, no log, just output that stopped being
 * clickable.
 */
export const READY = 'ready'

export interface TaskLinkCtx {
  /** The project this pane's output belongs to, or `null`/`''` for a pane that has none. */
  readonly paneProject: string | null
  /** The project `tasksStore` is attached to. `null` in a detached-pane window. */
  readonly boardProject: string | null
  readonly board: BoardLike
}

/**
 * The codes in this line that may be underlined. Empty is the common answer and the safe one.
 *
 * # No board, no link — never an underline that would then refuse
 *
 * A link that takes a click and does nothing is the failure this project has found sixteen
 * times, and `PathLinkEnv.open`'s `?? null` is the same rule one file over. So every doubt here
 * withholds the link rather than offering one and apologising later:
 *
 * - A **detached-pane window** attaches `tasksStore` to `null` (`App.tsx`) and mounts no
 *   `TaskDetailHost` at all, so `boardProject` is `null` there for the window's whole life.
 * - A pane whose project is not the one the board is attached to would resolve its code against
 *   the wrong project's tracker. Every project can have a `t-503`.
 * - A tracker that is absent or unreadable has nothing to open; the Tasks panel says why.
 * - A code that is not on the board is not a link, which is a decision rather than a fallback:
 *   an unknown `t-999` in output stays plain text. The board is already in memory, so this costs
 *   nothing — unlike a path, which has to be probed before it can be believed.
 */
export function offeredTaskCodes(line: string, ctx: TaskLinkCtx): readonly TaskCode[] {
  if (ctx.paneProject === null || ctx.paneProject === '') return []
  if (ctx.boardProject === null || ctx.boardProject === '') return []
  if (ctx.paneProject !== ctx.boardProject) return []
  if (ctx.board.kind !== READY) return []
  const tasks = ctx.board.tasks
  if (tasks === undefined) return []
  /*
   * `some` over the list, never an index or a `Record` lookup — `openTask`'s rule in a new
   * place. An id of `constructor` has to find nothing rather than a function, and the matcher
   * not being able to produce one today is not a reason to write the lookup that could.
   */
  return matchTaskCodes(line).filter((code) => tasks.some((task) => task.id === code.id))
}
