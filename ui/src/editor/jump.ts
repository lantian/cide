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
 * refresh-from-the-live-caret — is all in `navHistory.ts`, which is import-free and is compiled
 * and driven by `ui/scripts/check-editor.mjs`. What is left here is the store, the clock and the
 * IPC: the parts a headless check could not run anyway.
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
 * is wanted. Two consequences, accepted rather than worked around: Back can grow the tab strip,
 * and an entry for a file deleted since produces a tab showing *"This file could not be
 * opened"*. That notice is the honest report; the alternative is a Back that silently skips.
 */
export function navigate(direction: NavDirection): WalkRefusal | null {
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
   * Uncaught, deliberately. `chrome/Failures.tsx` listens for `unhandledrejection`, so a
   * `.catch(() => {})` here is precisely what would stop a failed open being reported — the
   * reasoning written out at length in `goToDefinition.ts`.
   */
  void fileApi.open(project, to.path).then(() => {
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
     */
    if (to.line !== UNKNOWN_LINE) requestAnimationFrame(reveal)
  })
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
