/**
 * The one door every explicit navigation goes through — and the Back/Forward that walks it.
 *
 * # Why a seam rather than a call at each site
 *
 * There were eight places that moved the caret to another file (the Explorer, a search hit, a
 * Problems row, the file picker, open-in-split, Go to symbol, the File Structure popup, Go to
 * definition, a Ctrl+click on a path in terminal output), and every one of them did the same two
 * things in the same order for the same reason. Adding "and remember where we came from" to
 * eight independent call sites is a rule enforced by memory, and the ninth navigation gesture
 * somebody adds next year is the one that forgets. So they all call [`jumpTo`] instead, and
 * `check-editor.mjs` asserts that no `requestReveal(` survives outside this module and
 * `goToDefinition.ts`.
 *
 * # What is here and what is deliberately not
 *
 * No decisions. The stack arithmetic — the merge rule, the forward-tail truncation, the cap, the
 * refresh-from-the-live-caret, and since M16 the rule that says which pointer clicks are
 * navigations — is all in `navHistory.ts`, which is import-free and is compiled and driven by
 * `ui/scripts/check-editor.mjs`. What is left here is the store, the clock and the IPC: the parts
 * a headless check could not run anyway.
 *
 * Three functions write to the store, and the split is about *when the caret can be read*, not
 * about taste: [`jumpTo`] for a navigation that is about to happen, [`pendingJump`] for one that
 * might still be refused, and [`recordClick`] for one that has already happened. Each carries the
 * ordering its own call site gets wrong.
 *
 * # Across windows: stated, not left to be discovered
 *
 * **The history is per JavaScript realm, and a detached pane window has its own.** That is not a
 * design so much as a consequence: a `pane:<uuid>` window is a separate webview with its own
 * module instances and no shared memory, which has already caused two bugs in this repository —
 * `App.tsx`'s `openTerminalPath` carries a KNOWN GAP comment saying exactly this about
 * `revealRequest`'s module-level map. Here it means:
 *
 * * A detached window renders a terminal or a refusal and **never an editor**
 *   (`windows/DetachedPaneWindow.tsx`, and the invariant stated in `openBuffers.ts`), so its
 *   history is always empty and Back there is refused with a sentence rather than silently
 *   walking the *shell* window's stack, which is not the window the user is looking at.
 * * [`jumpTo`] records nothing in a non-shell realm, because the tab it opens mounts in the
 *   shell window's realm — the same asymmetry the KNOWN GAP describes.
 * * If an editor is ever allowed to detach, the two realms get two divergent stacks. The only
 *   real fix is a Rust-owned history, which costs an event per jump to every window; that is
 *   written in `README.md`'s honest-record column rather than half-built here.
 *
 * The history is also **session-scoped**: it does not survive a window reload or a relaunch, and
 * that is deliberate. A back-stack restored from last week is a claim about where you have been
 * that is not true. Per-file view memory is the durable half, and it is a different store for
 * exactly that reason.
 */
import { file as fileApi, type ProjectId } from '@/ipc/client'
import { useWorkspace } from '@/store/workspace'
import { focusedCaret } from './caretTrack'
import { requestReveal, type RevealTarget } from './revealRequest'
import {
  EMPTY_HISTORY,
  UNKNOWN_LINE,
  abandon,
  record,
  walk,
  type NavDirection,
  type NavHistory,
} from './navHistory'
import type { FilePosition } from './position'

/**
 * One stack per project, per window.
 *
 * Per project because a shell window hosts several at once — stacked windows are the default —
 * and Back must not walk out of the project the user is looking at and into another one's files.
 * `Map` rather than a field on the workspace mirror because the mirror is replaced wholesale on
 * every `cide://workspace-changed`, and because putting a caret-level history in the tree would
 * mean a full broadcast to every window plus a queued disk write per jump.
 */
const histories = new Map<ProjectId, NavHistory>()

/** Where the caret is now, as the shared position type. `null` when no editor holds the slot. */
function livePosition(): FilePosition | null {
  const caret = focusedCaret()
  if (caret === null) return null
  return { path: caret.path, line: caret.line, column: caret.column }
}

/** Only the shell window keeps a history; see the module header. */
function isShellRealm(): boolean {
  return useWorkspace.getState().boot?.role.kind === 'shell'
}

export interface JumpTarget {
  readonly path: string
  /**
   * 1-based, or [`UNKNOWN_LINE`] for "open this file wherever it remembers".
   *
   * The second is what the Explorer, the file picker and open-in-split pass: they name a file
   * and not a place in it, so the per-file view memory decides, and nothing here may move the
   * caret or the restore would be overwritten by a jump to line 1.
   */
  readonly line: number
  /** 1-based, UTF-16 units. */
  readonly column: number
  /** 1-based and exclusive. Omitted is a bare caret, which is what a jump usually means. */
  readonly endColumn?: number | undefined
  /** Take the keyboard as well as the caret. See `RevealTarget.focus`. */
  readonly focus?: boolean | undefined
  /** Put the destination in the middle of the viewport. See `RevealTarget.align`. */
  readonly align?: 'center' | undefined
}

/**
 * Record the jump and ask for the caret to land on it.
 *
 * **This deliberately does not open the file.** The callers that need a tab already call
 * `file.open` themselves and then hydrate the mirror, and the order — reveal *first*, open
 * second — is load-bearing: the editor for the destination usually does not exist yet, so the
 * request is parked and spent by the mount the open causes. Reversed, the tab opens at line 1
 * and the caret never moves, which is precisely the case the whole mechanism exists to serve.
 * Folding the open in here would mean this module deciding *which* project a path belongs to,
 * which is a fact about the workspace it has no business holding.
 *
 * `project` may be `null` — a pane outside any project. The reveal still happens; only the
 * recording is skipped, because a history is keyed by project and there is nothing to key it on.
 */
export function jumpTo(project: ProjectId | null, to: JumpTarget, record_ = true): void {
  /*
   * `record_ = false` is for re-issuing a jump the user made once.
   *
   * The out-of-project confirmation is the case that needs it: the reveal has a 10 s TTL and
   * reading a dialog can take longer, so the retry has to re-park the reveal — but re-parking
   * used to re-record too, and the Back stack ended up holding the same origin/destination
   * pair twice. Pressing Back then appeared to do nothing, because the entry it arrived at was
   * identical to the one it left. `record`/`near` in `navHistory.ts` merge only against the
   * previous entry and never across files, so the duplicate survived.
   *
   * A parameter rather than a separate `revealJump` export because the two must not drift on
   * the reveal *flags* — the whole reason `jumpTo` exists is that three call sites got the
   * order and the flags subtly different.
   */
  if (record_ && project !== null && isShellRealm()) {
    pruneHistories()
    const history = histories.get(project) ?? EMPTY_HISTORY
    histories.set(
      project,
      record(history, livePosition(), { path: to.path, line: to.line, column: to.column }),
    )
  }

  // "Somewhere in this file" reveals nothing: the file is being opened without a destination,
  // and a `requestReveal` here would land the caret on line 1 and *overwrite the per-file view
  // memory's restore* — turning the one feature into a bug in the other.
  if (to.line === UNKNOWN_LINE) return

  const target: RevealTarget = {
    line: to.line,
    column: to.column,
    // A bare caret, not a selection: `revealRange` collapses an empty range to the anchor, and
    // that is what IDEA does on a jump — it puts you *at* the place, it does not highlight it.
    endColumn: to.endColumn ?? to.column,
    ...(to.focus === undefined ? {} : { focus: to.focus }),
    ...(to.align === undefined ? {} : { align: to.align }),
  }
  requestReveal(to.path, target)
}

/**
 * Write the entry for a pointer click that has **already** moved the caret. (M16)
 *
 * The third recording door, and the narrow one: no reveal, no open, no flags. [`jumpTo`] exists
 * because eight gestures had to *ask* for a caret move and then remember to record it;
 * `navRecorder.ts` is the opposite shape — the caret is already where the user pointed, so
 * issuing a `requestReveal` here would dispatch a selection into the view that just produced one,
 * for no change. The whole of the decision is `navHistory.ts::recordsClick`, which the caller has
 * already run.
 *
 * # Why the origin is a parameter and not read here
 *
 * The same ordering hazard [`pendingJump`] was written for, arriving from the other side.
 * `origin` is the caret **before** this click, and the only place that is still knowable is
 * inside the `ViewPlugin`'s `update()`: CodeMirror runs plugin updates before update listeners,
 * and `EditorSurface`'s listener is what hands `caretTrack`'s slot to the editor being clicked
 * into. Read the caret here — one listener later — and the origin *is* the destination, `near`
 * merges the two into one entry, and the feature is a silent no-op with nothing on screen to say
 * so. Taking it as an argument means this function cannot get that wrong.
 */
export function recordClick(
  project: ProjectId | null,
  origin: FilePosition | null,
  to: FilePosition,
): void {
  if (project === null || !isShellRealm()) return
  pruneHistories()
  const history = histories.get(project) ?? EMPTY_HISTORY
  histories.set(project, record(history, origin, to))
}

/**
 * Prepare the history entry for a jump that has **not happened yet**, and hand back the commit.
 *
 * The other half of `jumpTo(…, record_ = false)`, and it exists for one call site: opening a
 * path a *terminal* named. That open can be refused — the path is out of every project root, it
 * is a FIFO, it is 2 GB, it is gone — and `App.tsx` used to write the history entry before
 * asking, so a path the user then **declined** in the out-of-project dialog stayed on the Back
 * stack. Pressing Back walked into it through a command that enforces nothing, and the refusal
 * the user had just given was bypassed by a mouse button.
 *
 * `navHistory.ts`'s own recording rule is that an entry is a place the user has *been*; a refused
 * open is not one. So the reveal is still parked before the call — it has a 10 s TTL and reading
 * a dialog takes longer, which is why the two halves are separable at all — and the entry is
 * written from the `.then()`.
 *
 * # Why this is a closure and not `recordJump(project, to)` called from the `.then()`
 *
 * **The origin has to be read before the IPC, and the entry written after it.** `record` needs
 * where the user *was*, and that is `livePosition()` — the top of `caretTrack`'s claim stack.
 * A mounting editor *claims* that stack, and `terminal_open_path` broadcasts
 * `cide://workspace-changed` from inside the workspace lock, before it returns: by the time the
 * invoke promise settles, the new editor can already have mounted and taken the slot. Reading
 * the caret there would make the origin the *destination*, `near` would merge the two into one
 * entry, and Back from a terminal ctrl+click would land nowhere — the feature quietly reduced to
 * a no-op, with no error anywhere.
 *
 * Splitting it as "export the caret reader too, and remember to call it first" was the
 * alternative. It loses for the reason this module exists at all: an ordering rule spread over
 * two call sites is enforced by memory. Here the closure *is* the ordering.
 */
export function pendingJump(project: ProjectId | null, to: FilePosition): () => void {
  if (project === null || !isShellRealm()) return () => {}
  const origin = livePosition()
  return () => {
    pruneHistories()
    const history = histories.get(project) ?? EMPTY_HISTORY
    histories.set(project, record(history, origin, to))
  }
}

/**
 * Drop the stacks of projects that are no longer open.
 *
 * `histories` is keyed by `ProjectId` and nothing ever removed from it, so closing a project left
 * its entries — up to `NAV_CAP` positions each — alive for the life of the window. The same leak
 * `ClosedTabs::forget_project` exists to prevent in Rust, and the same reason it matters: a
 * reopened directory gets a **new** id, so the old stack is unreachable as well as stale.
 *
 * Swept from the recording paths rather than pushed from the store's `closeProject`, and that is
 * deliberate. `store/workspace.ts` is what `jump.ts` imports; importing back would make the two
 * modules circular, and the alternative — a call in `App.tsx` beside every `closeProject` — is a
 * rule enforced by memory across three call sites, which is the arrangement this file's own
 * header was written to get rid of. Sweeping costs one `Map` walk on a gesture that is already
 * doing IPC, and it cannot be forgotten.
 */
function pruneHistories(): void {
  if (histories.size === 0) return
  const open = useWorkspace.getState().boot?.workspace.projects
  if (open === undefined) return
  for (const project of histories.keys()) {
    if (!(project in open)) histories.delete(project)
  }
}

/** Why a Back or Forward did nothing, in a sentence the user can read. */
export type WalkRefusal = string

/**
 * Walk the history one step and go there. Answers `null` on success, a sentence on refusal.
 *
 * The refusal is a value rather than a silence because a mouse button that does nothing is
 * indistinguishable from a mouse button wired to nothing — which is this project's most-repeated
 * defect, and the reason `dispatch.ts` has an `unmet` at all.
 *
 * Every walk opens the destination file. An entry holds a *path*, not a tab, so Back after
 * closing a file reopens it — which is IDEA's behaviour and one of the main reasons the gesture
 * is wanted. Back can therefore grow the tab strip, which is accepted rather than worked around.
 *
 * # Reopening goes through `file.reopen`, not `file.open`
 *
 * `tab_open_file` mints one fresh single-pane Editor tab and knows nothing about how the tab
 * left. That was wrong in two ways, and `cmd::file::tab_reopen_file` documents both at length:
 * a file tab's pane tree can hold a **live Claude session** (splitting a File tab defaults to
 * `NewClaude`), and opening a fresh tab while that file's record sat on the closed-tab stack
 * silently poisoned the next Ctrl+Shift+T — it popped the record, found the tab open, and
 * consumed it for nothing.
 *
 * So the walk asks the command that consults that stack. It **peeks**, and takes the record only
 * when it actually reinserts the tab: a Back that merely activates an already-open tab leaves
 * the record alone, because it is still the right answer for a Ctrl+Shift+T after the user
 * closes that tab again. The cost of the alternative, stated because it was the tempting one:
 * ignoring the stack entirely keeps Back simple and loses the split, the pane ids and the
 * session binding every time — and leaves the same record to be spent invisibly later.
 *
 * # And a deleted file is a sentence, not a broken tab
 *
 * An entry for a file removed since used to produce a permanent tab reading *"This file could
 * not be opened / No such file or directory (os error 2)"*, with the path nowhere in it. The
 * command stats first and answers `gone`; the entry is then dropped from the history, so a
 * second press does not offer the same dead file again.
 *
 * Asynchronous because of that answer: the refusal for a gone file is only known after the round
 * trip. `dispatch.ts` awaits it and notifies, exactly as it does for the synchronous refusals.
 */
export async function navigate(direction: NavDirection): Promise<WalkRefusal | null> {
  const boot = useWorkspace.getState().boot
  if (boot?.role.kind !== 'shell') {
    return 'this window has no editor history — a detached pane keeps its own, and it is empty'
  }
  const project = boot.role.active
  if (project === null) return 'no open project'

  const history = histories.get(project) ?? EMPTY_HISTORY
  const stepped = walk(history, direction, livePosition())
  if (stepped === null) {
    return direction === 'back'
      ? 'nothing to go back to yet — Back walks the jumps you have made, not the keys you have pressed'
      : 'already at the most recent place'
  }
  histories.set(project, stepped.history)

  const { to } = stepped
  /*
   * `focus: true` and `align: 'center'`, matching Go to line and Go to symbol.
   *
   * This is a place the user named — with a mouse button, but named — so the keyboard has to
   * follow the caret, and a destination flush against the bottom edge of the viewport shows the
   * line with none of the code around it. A thumb-button press over a *terminal* is the common
   * case, and without `focus` the next keystroke would still go to that terminal.
   *
   * Skipped for an entry with no position: opening the file and letting the view memory place it
   * is exactly what the click that recorded the entry did, so Back reproduces it rather than
   * inventing a line the user never saw.
   */
  const reveal = (): void => {
    requestReveal(to.path, {
      line: to.line,
      column: to.column,
      endColumn: to.column,
      focus: true,
      align: 'center',
    })
  }
  if (to.line !== UNKNOWN_LINE) reveal()
  /*
   * Awaited rather than fired and forgotten, because `gone` is an answer this function has to
   * report. Not caught: `chrome/Failures.tsx` listens for `unhandledrejection`, so swallowing a
   * genuine rejection here is precisely what would stop a failed open being reported — the
   * reasoning written out at length in `goToDefinition.ts`. A rejection therefore propagates to
   * the caller's `void`, exactly as before.
   */
  const outcome = await fileApi.reopen(project, to.path)
  if (outcome.kind === 'gone') {
    /*
     * The walk is undone and the dead entry dropped, so the next press does not walk into the
     * same wall — and so the cursor is left where the user actually is rather than on a place
     * they never reached. `abandon` is where that arithmetic lives and why it takes the
     * direction; this function only decides *when*.
     *
     * Read back out of the map rather than using `stepped.history`: this function has awaited,
     * and a jump from another gesture could have recorded in the meantime — writing the stale
     * value back would erase it. The path check is what confirms the cursor is still standing on
     * the entry this walk landed on; if it is not, the history has moved on and is not ours to
     * rewrite.
     */
    const current = histories.get(project) ?? EMPTY_HISTORY
    if (current.entries[current.index]?.path === to.path) {
      histories.set(project, abandon(current, direction))
    }
    // The command's own copy of the path rather than the entry's. They are the same string
    // today; taking it from the answer means the sentence names what Rust actually looked for.
    return `${outcome.path} is no longer there`
  }

  useWorkspace.getState().hydrate()
  /*
   * And reveal a second time, because the first one lands in the wrong place for the most
   * common Back there is.
   *
   * The order above — reveal, then open — is deliberate and stays: a Back into a file no
   * pane has open must park the request so the mount can spend it, which is the reasoning
   * `goToDefinition.ts` writes out. But the *dominant* case is a file that is already open
   * in a BACKGROUND tab, and there the request is delivered immediately, synchronously, to
   * an `EditorSurface` sitting under `visibility: hidden` — where `view.focus()` is a no-op.
   * `TabContent` never unmounts an inactive tab, so a live receiver exists the whole time
   * and nothing parks. The open that follows activates the tab a moment later, by which
   * point the reveal has been spent and there is nothing left to re-focus: the caret is
   * placed but the keyboard is on `<body>`, and the next thing typed goes nowhere.
   *
   * A frame after the hydrate, so React has committed the visibility flip. Re-issuing is
   * cheap and idempotent — it sets the same selection and focuses the same view — which is
   * why this is a second call rather than a scheduler that tries to guess which case it is
   * in. Guessing is what would have to be right every time; this only has to be harmless
   * when it is redundant.
   *
   * A `restored` tab needs it just as much: `reinsert_tab` mounts a whole pane tree, so the
   * editor that will spend the reveal appears strictly after this point either way.
   */
  if (to.line !== UNKNOWN_LINE) requestAnimationFrame(reveal)
  return null
}

/** The history for a project, for checks and diagnostics. Never mutated through this. */
export function historyOf(project: ProjectId): NavHistory {
  return histories.get(project) ?? EMPTY_HISTORY
}

/** Testing seam: forget every stack. Never called by the app. */
export function resetHistoriesForTest(): void {
  histories.clear()
}
