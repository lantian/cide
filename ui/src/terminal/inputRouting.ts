/**
 * Who is allowed to turn one keystroke into one byte sequence.
 *
 * # The defect this exists to close
 *
 * xterm 6 has two independent emitters on the ordinary typing path, and with our options
 * only one of them is guarded. Read out of the shipped bundle
 * (`node_modules/@xterm/xterm/lib/xterm.js`), not from memory:
 *
 * ```
 * _keyDown(e) {
 *   this._keyDownHandled = false; this._keyDownSeen = true;
 *   …
 *   this.coreService.triggerDataEvent(result.key, true);          // emitter 1
 *   !screenReaderMode || e.altKey || e.ctrlKey
 *     ? this.cancel(e, true)                                      // ← we take this branch
 *     : void (this._keyDownHandled = true);                       // ← never reached
 * }
 * _keyUp(e) { this._keyDownSeen = false; … }
 * _inputEvent(e) {
 *   if (e.data && e.inputType === 'insertText'
 *       && (!e.composed || !this._keyDownSeen) && !screenReaderMode) {
 *     if (this._keyPressHandled) return false;
 *     this.coreService.triggerDataEvent(e.data, true);            // emitter 2
 *   }
 * }
 * ```
 *
 * `screenReaderMode` is off, so `_keyDownHandled` is never set and — because `cancel()`
 * suppresses the `keypress` that would have set it — neither is `_keyPressHandled`. An
 * `input` event is always `composed: true`, so emitter 2's guard collapses to a single
 * term: **"emit unless a key is physically down right now."**
 *
 * `preventDefault()` on keydown stops the *browser's* own insertion. It does not stop an
 * input method: under ibus on Wayland the commit arrives as a `zwp_text_input_v3`
 * `commit_string`, asynchronously, and WebKitGTK inserts it into the helper textarea
 * whenever it lands. If it lands after `keyup` — a race, decided per keystroke by
 * scheduling — `_keyDownSeen` is already false and the same character is emitted twice.
 * Three keys, some of them doubled, in no fixed pattern: `pwwdwd`.
 *
 * # The rule
 *
 * A keystroke is *claimed* at keydown by whoever handled it — xterm's own `_keyDown`, or
 * this app's key gate, or the Shift+Enter re-encoding. A later `insertText` that redeems a
 * claim is the input method re-delivering a keystroke that has already been sent, and is
 * dropped. An `insertText` with no claim behind it — an emoji picker, a CJK commit, a
 * synthetic insertion — was never emitted by anybody, and *this* is the path that has to
 * emit it.
 *
 * Claims rather than a boolean, because the commit for key *n* can arrive after the keydown
 * for key *n+1*: what has to be preserved is the **count**, not the pairing. Claims expire
 * ([`CLAIM_TTL_MS`]) so a keystroke whose input method never commits cannot swallow an
 * unrelated insertion minutes later.
 *
 * # Why the obvious fixes were rejected
 *
 * * `screenReaderMode: true` does disable emitter 2, but it also flips the keydown
 *   cancellation branch above and drags in xterm's accessibility DOM — a much larger
 *   behaviour change than the bug.
 * * An `input` listener that unconditionally calls `term.input(data)` re-emits every
 *   character `_keyDown` already sent, i.e. it *creates* the doubling on the ordinary path.
 *   The listener has to decide, not adopt.
 *
 * Deliberately pure — no DOM types, no imports — so `ui/scripts/check-input-emitters.mjs`
 * can compile and run it with nothing but `tsc`. A bug nobody can reproduce needs a test
 * more than most, and there is no DOM test environment in this repo.
 */

/** What the host should do with an `input` event. */
export type InputDecision =
  /** Already sent at keydown. Stop the event and write nothing. */
  | 'swallow'
  /** Nobody has sent this. Feed it to the terminal as if it had been typed. */
  | 'emit'
  /** Not ours: composition text, deletions, paste. Let xterm handle it. */
  | 'defer'

/**
 * How long a keydown's claim on a later `insertText` stays valid.
 *
 * Long enough for an input method round trip through the compositor (observed in the
 * low tens of milliseconds; a loaded machine is worse), short enough that a claim can never
 * still be standing when the user reaches for the emoji picker. The failure mode at each end
 * is asymmetric and that is what picks the number: too short duplicates a character, too
 * long drops one. Both are bad, but a dropped character from a *deliberate* second insertion
 * within half a second of typing is a case that does not occur, while duplication from a
 * slow commit is the bug being fixed.
 */
export const CLAIM_TTL_MS = 500

/** The parts of a `KeyboardEvent` this decision needs. */
export interface KeyFacts {
  key: string
  keyCode: number
  ctrlKey: boolean
  altKey: boolean
  metaKey: boolean
  isComposing: boolean
}

/** The parts of an `InputEvent` this decision needs. */
export interface InputFacts {
  inputType: string
  data: string | null
  isComposing: boolean
}

/**
 * Whether this keydown is the input method's placeholder rather than a real key.
 *
 * All three spellings occur: WebKitGTK reports `keyCode 229` for a key the IM has taken,
 * Chromium-derived paths report `key === 'Process'`, and a keydown raised while a
 * composition is open carries `isComposing`. Any of them means the character has *not* been
 * handled at keydown and the `input` event that follows is the only delivery there will be.
 */
export function imeFiltered(k: KeyFacts): boolean {
  return k.isComposing || k.keyCode === 229 || k.key === 'Process'
}

/**
 * Whether this keydown is one that an input method could later re-deliver as `insertText`.
 *
 * Only single-character keys with no Ctrl/Alt/Meta. Everything else — Enter, arrows, chords
 * — reaches the child as a control sequence that no IM commits, so claiming for them would
 * leave stale claims lying around for the *next* real insertion to redeem. `key.length === 1`
 * is xterm's own test for the same thing in `_keyDown`.
 */
export function producesText(k: KeyFacts): boolean {
  return !imeFiltered(k) && k.key.length === 1 && !k.ctrlKey && !k.altKey && !k.metaKey
}

/**
 * One terminal's view of who has emitted what.
 *
 * Per terminal, not per app: two panes have two textareas and two independent input methods'
 * worth of state, and a shared counter would let a keystroke in one pane swallow a commit in
 * the other.
 */
export class InputRouter {
  /** Timestamps of keystrokes already sent, oldest first. */
  private claims: number[] = []

  /**
   * Record that this keydown has been dealt with — by xterm, or by the gate, or by us.
   *
   * Called for *both* outcomes of the key handler on purpose. A key the gate swallowed was
   * still consumed by this application; if the IM re-delivers it, forwarding it to the child
   * would defeat the binding the user configured, which is the same bug in the other
   * direction.
   */
  keydown(k: KeyFacts, at: number): void {
    if (!producesText(k)) return
    this.expire(at)
    this.claims.push(at)
  }

  /** Decide what the host should do with an `input` event. */
  input(ev: InputFacts, at: number): InputDecision {
    // `insertCompositionText`, `insertFromPaste`, `deleteContentBackward`… are xterm's to
    // handle: composition flows through `compositionstart`/`update`/`end`, and paste through
    // its own paste handler. Touching them here would break real CJK input to fix ASCII.
    if (ev.inputType !== 'insertText') return 'defer'
    if (ev.isComposing) return 'defer'
    if (ev.data === null || ev.data === '') return 'defer'

    this.expire(at)
    if (this.claims.length > 0) {
      this.claims.shift()
      return 'swallow'
    }
    return 'emit'
  }

  /** Drop the standing claims — a blur, a reset, a re-parent. */
  clear(): void {
    this.claims.length = 0
  }

  /** For the dev probe and the check script. */
  pending(): number {
    return this.claims.length
  }

  private expire(at: number): void {
    while (this.claims.length > 0 && at - (this.claims[0] as number) > CLAIM_TTL_MS) {
      this.claims.shift()
    }
  }
}

/**
 * Replay a whole event sequence and report how many emits it produced.
 *
 * The check script's entry point, and the only way to state the property that matters:
 * *n* characters typed produce *n* writes, whatever order the input method delivers them in.
 * Written here rather than in the script so the thing under test is the shipped code path.
 */
export type ProbeEvent =
  | { at: number; kind: 'keydown'; key: KeyFacts }
  | { at: number; kind: 'input'; input: InputFacts }

export function replay(events: readonly ProbeEvent[]): {
  decisions: InputDecision[]
  emits: number
  swallows: number
  /** What actually reaches the child: one per keydown xterm sends, plus each `emit`. */
  writes: number
} {
  const router = new InputRouter()
  const decisions: InputDecision[] = []
  let writes = 0
  for (const ev of events) {
    if (ev.kind === 'keydown') {
      // xterm's `_keyDown` writes the character for a plain text key it is handed; an
      // IME-filtered keydown is returned false before it gets that far. That write is not
      // this module's to make, but it is the other half of the count, so the model has to
      // include it or the property is untestable.
      if (producesText(ev.key)) writes += 1
      router.keydown(ev.key, ev.at)
      continue
    }
    const d = router.input(ev.input, ev.at)
    decisions.push(d)
    if (d === 'emit') writes += 1
  }
  return {
    decisions,
    emits: decisions.filter((d) => d === 'emit').length,
    swallows: decisions.filter((d) => d === 'swallow').length,
    writes,
  }
}
