/**
 * When a terminal has taken bytes and stopped painting them.
 *
 * # The failure this describes
 *
 * xterm's `RenderService` pauses itself. It puts an `IntersectionObserver` on `.xterm-screen`
 * and, whenever that element reports non-intersecting, sets `_isPaused` — after which
 * `refreshRows` returns early and records `_needsFullRefresh` instead. **The buffer keeps
 * taking writes the whole time**, so the terminal's state stays correct; what stops is the
 * picture. The flag is cleared in exactly one place: the same observer reporting intersecting
 * again. There is no timeout, no public API that clears it, and no other path back.
 *
 * That is fine when the pause and the resume are two halves of one DOM change — a host parked
 * into `layout/paneHosts.ts`'s detached parking div and mounted again, which is why
 * `mountHost` already asks for a frame on the way back in. It is not fine when the thing that
 * made the element non-intersecting was never a DOM change at all: a window that was
 * minimised, moved to another virtual desktop, or occluded stops WebKit's rendering update,
 * and the resume notification the flag is waiting for is not guaranteed to arrive when it
 * starts again. The pane is then frozen for the rest of the session, showing the frame it was
 * on, over a buffer that is still perfectly up to date.
 *
 * The reported shape of it: *"bash panel — on long running tasks it just stops outputting, and
 * when I click fullscreen it shows me that nothing is doing"*. Maximising repairs it because
 * maximising changes the pane's grid tracks, and a layout change is what forces WebKit to
 * recompute the intersection and notice the element was visible all along. Nothing else the
 * user can do to a pane will.
 *
 * # The second way a frame goes missing: a resize nobody answered
 *
 * The rule started as *bytes parsed, no frame*, which is one debt and not the only one. A
 * **geometry change** owes a frame too: `panes/sessionSink.ts`'s `syncSize` fits the terminal
 * to a new box and pushes the size at the child, and everything the user is looking at has
 * moved whether or not another byte ever arrives. On an idle pane no byte does — so with
 * `lastParsedAt` as the only debt, a resize that failed to repaint was a state nothing in the
 * system could notice, for the rest of the session.
 *
 * That is the *maximise* report, which is the same sentence as above with the sign flipped:
 * *"switching claude panel to fullscreen makes screen empty until i scroll in it"*. Maximising
 * is the one gesture that changes a pane's size without changing its DOM, so it is both the
 * gesture that repairs the pause above and a gesture that can leave a frame owed with no
 * further output to arm anything.
 *
 * [`RenderStallInput.lastResizedAt`] is therefore a second debt, sitting beside the first, and
 * the two are not guarded identically — see [`RenderStallInput.atBottom`].
 *
 * # Why the rule is a pure function in its own file
 *
 * This project has no JS test runner: `ui/scripts/check-*.mjs` *are* the suite, and each one
 * compiles a deliberately import-free module with the TypeScript in `node_modules` and asserts
 * on the output (`ui/scripts/check-render-stall.mjs`). A rule that lived inside `paneHosts.ts`
 * would need a DOM, a live xterm and a webview to exercise, which is to say it would never be
 * exercised — and the whole point of the guards below is that they are the difference between
 * a repair that fires only in the broken state and one that flickers a healthy pane.
 *
 * Keep this file import-free.
 */

/**
 * How long a frame may be owed before the renderer is presumed stuck.
 *
 * A healthy terminal paints within an animation frame of parsing, so anything past a handful
 * of frames is already anomalous — and the number is nowhere near that, on purpose.
 *
 * **It has to clear xterm's synchronized-output window, which is the one legitimate way for a
 * terminal to take bytes and deliberately paint nothing.** A program that opens DEC mode 2026
 * (`CSI ? 2026 h`) tells the terminal to buffer rows until it closes the block, and
 * `RenderService`'s `SynchronizedOutputHandler` gives that a one-second safety timeout before
 * forcing the frame out. A threshold under a second would therefore call a TUI mid-frame
 * "stalled" — the Claude console being the obvious one — and repair a pane that was working
 * exactly as designed. 1500 ms sits clear of that timeout with room for the frame that follows
 * it.
 *
 * The cost of waiting is that a genuinely frozen pane stays frozen a second and a half longer,
 * against a bug whose current lifetime is "until the user thinks to maximise the pane".
 */
export const STALL_MS = 1_500

/**
 * The floor between two repairs of the same pane.
 *
 * A repair that did not work must not become a loop — the pane would then be nudged on every
 * burst of output for as long as the run lasts. If the first one did not free the renderer,
 * the second one a few seconds later is diagnosis, not persistence.
 */
export const REPAIR_COOLDOWN_MS = 5_000

/** Everything the rule needs, all of it observable from the host and its terminal. */
export interface RenderStallInput {
  /** A monotonic clock — `performance.now()` at the call site. */
  readonly now: number
  /** When bytes were last *parsed* into this terminal, or `null` if none ever have been. */
  readonly lastParsedAt: number | null
  /** When this terminal last painted a frame (xterm's `onRender`), or `null`. */
  readonly lastRenderedAt: number | null
  /** When this pane was last repaired, or `null` if it never has been. */
  readonly lastRepairAt: number | null
  /** Whether the host's element sits in a live slot. */
  readonly mounted: boolean
  /**
   * Whether the element has a real box in a visible document.
   *
   * A pane with no box, or one in a document the compositor is not drawing, is *correctly*
   * not painting; repairing it would be a nudge for nothing and, worse, would teach the
   * watchdog to fire on the one state the pause exists to serve.
   */
  readonly onScreen: boolean
  /**
   * Whether the viewport is at the bottom of the scrollback.
   *
   * This is the guard that keeps a scrolled-back pane out of the rule, and it is not
   * cosmetic. xterm only repaints rows that are on screen, so output landing below a viewport
   * the user has scrolled away from is *meant* to paint nothing — parsed bytes and no frame
   * is the correct behaviour there, and it is indistinguishable from the failure without this
   * flag. Reading somebody's scrollback would otherwise nudge the pane for as long as
   * they read.
   *
   * **It vetoes the bytes debt only, never the resize one.** The argument above is entirely
   * about *where the bytes landed*: rows below the fold. A geometry change has no such
   * excuse — it moves every row the user can see, whatever they have scrolled to — and the
   * repair does not scroll, so answering it while somebody reads their scrollback costs them
   * nothing. Vetoing both with this flag would have made the maximise case unreachable in
   * exactly the state it was reported from: a pane whose viewport is *the thing that is
   * wrong*.
   */
  readonly atBottom: boolean
  /**
   * When this terminal was last fitted to a new cell geometry, or `null` if it never has been.
   *
   * Stamped by `layout/paneHosts.ts`'s `noteResized`, which `syncSize` calls only on the path
   * where `cols`/`rows` actually changed — a `ResizeObserver` fires per frame of a drag and
   * most of those frames are the same geometry, so stamping every callback would arm a timer
   * per frame for a terminal that owes nothing.
   */
  readonly lastResizedAt: number | null
}

/**
 * Whether an event that should have produced a frame has gone unanswered.
 *
 * "Owes" is an event stamp running ahead of `lastRenderedAt`: the bytes are in the buffer, or
 * the terminal has been fitted to a new size, and nothing has drawn since. A terminal that has
 * had neither owes nothing, and one whose last frame came after both is simply up to date.
 */
function owedSince(input: RenderStallInput): number | null {
  const { lastParsedAt, lastRenderedAt, lastResizedAt } = input
  const unanswered = (at: number | null): boolean =>
    at !== null && (lastRenderedAt === null || lastRenderedAt < at)
  // The bytes debt carries the scrollback veto; the resize debt does not. See `atBottom`.
  const bytes = input.atBottom && unanswered(lastParsedAt) ? lastParsedAt : null
  const resize = unanswered(lastResizedAt) ? lastResizedAt : null
  if (bytes === null) return resize
  if (resize === null) return bytes
  // The *earliest* unanswered event, because the question the threshold asks is how long a
  // frame has been owed, not how recently something asked for one. Taking the later stamp
  // would let a steady trickle of output keep pushing the deadline out in front of a renderer
  // that has already been dead for a minute — which is the same mistake `checkStall`'s
  // re-arm branch in `layout/paneHosts.ts` exists to undo.
  return Math.min(bytes, resize)
}

/**
 * Whether this terminal owes a frame it has not painted, for long enough to be a fault.
 *
 * The three guards come first and are each a state in which painting nothing is *correct*:
 * a parked host, an element with no box in a document the compositor is drawing, and — for
 * bytes alone — a viewport the user has scrolled away from.
 */
export function isRenderStalled(input: RenderStallInput): boolean {
  if (!input.mounted || !input.onScreen) return false
  const owed = owedSince(input)
  if (owed === null) return false
  return input.now - owed >= STALL_MS
}

/** Whether to actually act on a stall now, or leave it to the cooldown. */
export function shouldRepairRender(input: RenderStallInput): boolean {
  if (!isRenderStalled(input)) return false
  const { lastRepairAt } = input
  return lastRepairAt === null || input.now - lastRepairAt >= REPAIR_COOLDOWN_MS
}
