/**
 * What xterm's hidden textarea is allowed to contain, and who writes a keystroke to the child.
 *
 * # The defect this exists to close, read out of `node_modules/@xterm/xterm/src`
 *
 * Five different pieces of xterm can write to the child on the typing path. Two of them
 * derive their payload from `textarea.value` measured against a *base* captured earlier, and
 * every bug reported against this app has been one of those bases going stale:
 *
 * | # | site | payload |
 * |---|------|---------|
 * | E1 | `CoreBrowserTerminal._keyDown` (line 1092) | `evaluateKeyboardEvent(...).key` — from the event, never wrong |
 * | E2 | `CoreBrowserTerminal._inputEvent` (line 1206) | `ev.data` — from the event, never wrong |
 * | E3 | `CompositionHelper._handleAnyTextareaChanges` (lines 186-207) | `newValue.replace(oldValue, '')`, `oldValue` captured at keydown |
 * | E4 | `CompositionHelper._finalizeComposition` (lines 128-178) | `textarea.value.substring(start)`, `start` captured at `compositionstart` |
 * | E5 | `CoreBrowserTerminal._keyPress` (line 1157) | `String.fromCharCode(ev.charCode)` — from the event, never wrong |
 *
 * E5 is the one that is easy to miss and it is not decorative: `_keyDown` has an early
 * `return true` for an unmodified capital letter (`ev.key.length === 1`, charCode 65-90) that
 * neither writes nor cancels, and E5 is what writes those. Because `_keyPress` calls
 * `cancel(event)` *without* force and `cancelEvents` defaults to false, the browser also
 * inserts the character into the textarea — so E5 and this module's `input` handler would
 * both deliver the same keystroke. `xterm.ts` returns false from the custom key handler for
 * `keypress`, which is what restores the invariant below to "exactly one of them writes".
 *
 * **Nothing in xterm 6 empties that textarea during ordinary typing.** The only three writers
 * of `''` are `_handleTextAreaBlur` (line 292), `_keyDown` for Ctrl+C and Enter *only*
 * (line 1087), and `Clipboard.paste` (line 55). Backspace never gets there: `_keyDown`
 * encodes DEL and calls `cancel(event, true)`, so the child is told to delete but the
 * textarea is not edited. So the textarea accumulates every character typed for the whole
 * lifetime of the pane, and every base above is measured against a growing string.
 *
 * ## How each reported symptom arises
 *
 * The user's three rounds, in order, all from the table above:
 *
 * * **Round 3, the decisive one.** Textarea holds a stale `\pwd`; the user types `pwd` and
 *   the child receives `\pwdp` `\pwdpw` `\pwdpwd` — one emission per keystroke, each the
 *   *entire* textarea. That is E4 with `start === 0`. `_compositionPosition.start` is
 *   initialised to `0` in the constructor and assigned in exactly one place,
 *   `compositionstart()`. An IME that commits with no preedit fires `compositionend` with no
 *   matching `compositionstart` — so `start` is never re-based, stays `0`, and
 *   `substring(0)` is the whole buffer. The stale `\pwd` proves the same thing from the
 *   other end: it could only survive minutes of backspacing because nothing ever emptied
 *   the textarea.
 * * **Round 2, `ppwpwd` = `p` + `pw` + `pwd`.** The identical mechanism with an
 *   empty-at-session-start textarea. It is the same bug as round 3 and not a different one.
 * * **Round 1, `pwwdwd`.** Before the `imeFiltered` bail in `xterm.ts`, a keyCode-229 keydown
 *   reached `CompositionHelper.keydown` (line 110), which armed E3 as well. Two emitters, one
 *   of them a `setTimeout(0)` diff racing the input method's commit, which is why round 1's
 *   duplicates came in ragged chunks (`p`, `w`, `wd`, `wd`) instead of round 3's clean
 *   whole-buffer pattern.
 * * **And why round 2 changed the symptom instead of curing it.** E3's timeout stores its
 *   diff in `_dataAlreadySent` (line 195), and E4 adds `_dataAlreadySent.length` to its start
 *   position (line 161). While E3 was alive it was quietly re-basing E4 by one character per
 *   keystroke, which masked the whole-buffer emission. Disarming E3 removed the mask.
 *
 * # The fix, and why the claim/redeem router that used to live here is gone
 *
 * The predecessor of this module counted "claims" — a keydown claimed a later `insertText`,
 * which was then swallowed as a duplicate — on the theory that an input method re-delivers
 * keystrokes xterm has already sent. That theory is unsupported: WebKit reports `keyCode 229`
 * precisely for keys the IM *filtered*, and for those `_keyDown` never emits at all, so E1
 * and the textarea emitters are mutually exclusive by construction. The claim machinery was
 * guarding a case that cannot happen, it shipped twice, and it changed the symptom twice.
 * It is deleted rather than tuned.
 *
 * What replaces it is one invariant:
 *
 * > **The textarea is empty at the start of every keystroke, and between one keystroke and
 * > the next it holds nothing but that keystroke's own text.**
 *
 * Hold that and every base in the table is correct without anyone having to know which
 * emitter will fire:
 *
 * * E4's `start` is `0` *and the textarea holds only the composed text*, so `substring(0)`
 *   is exactly that text — right whether or not `compositionstart` ever fired.
 * * E3's `oldValue` is `''`, so `newValue.replace('', '')` returns `newValue`, which is
 *   exactly the one new character. (`String.replace` with an empty pattern is not the
 *   identity-with-deletion it looks like; with a *non-empty* base that is not a substring it
 *   returns `newValue` unchanged, which is the whole-buffer emit above. An empty base is the
 *   one value for which "returns `newValue`" is the right answer.)
 * * E3 can never take its `newValue.length < oldValue.length` branch and emit a bare `DEL`,
 *   because the base is `''` and nothing is shorter than that. Backspace on an empty
 *   textarea also produces no `deleteContentBackward` at all — there is nothing to delete —
 *   so that branch is unreachable from both sides.
 * * E1, E2 and E5 read the event, not the textarea, and were always right about *what* to
 *   write. Only E5 was ever wrong about *whether* to, and `xterm.ts` disarms it.
 *
 * That leaves only *how many* emitters fire, which is what the small state machine below
 * decides. It has two bits — is a composition open, and is a commit being read out of the
 * textarea right now — and no timers, no TTL, and no counting.
 *
 * Deliberately pure — no DOM types, no imports — so `ui/scripts/check-input-emitters.mjs`
 * can compile it with nothing but `tsc` and drive it through a model of all four emitters.
 * A bug nobody can reproduce on demand needs a test more than most, and there is no DOM test
 * environment in this repo.
 */

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
 * What the host must do about the DOM event it just reported.
 *
 * A value rather than the guard touching the DOM itself, because that is what makes the
 * whole decision testable from `node` with no browser: `inputHost.ts` is then a dozen lines
 * of "apply this", and everything that can be wrong is in a pure function.
 */
export interface Reaction {
  /** `stopPropagation()`, so xterm's own listener on the textarea never sees this event. */
  readonly stop: boolean
  /** Hand this to `term.input()`, xterm's "as if typed" path. `null` writes nothing. */
  readonly emit: string | null
  /** Empty the textarea now, synchronously, before this event finishes dispatching. */
  readonly clear: boolean
  /**
   * Call [`InputGuard.commitSettled`] from a macrotask (`setTimeout(…, 0)`).
   *
   * Only ever returned from [`InputGuard.compositionend`], and the host must schedule it
   * from a **bubble-phase** listener: `_finalizeComposition` queues its own `setTimeout(0)`
   * from xterm's listener on the textarea, and ours has to run after that one so the clear
   * cannot empty the textarea out from under the read that delivers the composed text.
   */
  readonly defer: boolean
}

const NOTHING: Reaction = { stop: false, emit: null, clear: false, defer: false }
/** Ours: nothing else may see it, but nothing is written and the textarea is left alone. */
const SWALLOW: Reaction = { stop: true, emit: null, clear: false, defer: false }

/**
 * Whether this keydown is the input method's placeholder rather than a real key.
 *
 * All three spellings occur: WebKitGTK reports `keyCode 229` for a key the IM has taken,
 * Chromium-derived paths report `key === 'Process'`, and a keydown raised while a
 * composition is open carries `isComposing`. Any of them means the character has *not* been
 * handled at keydown and something on the textarea path is the only delivery there will be.
 *
 * Used by `xterm.ts` to return false from `attachCustomKeyEventHandler`, which stops
 * `_keyDown` at line 1026 — before `_compositionHelper.keydown(event)` at line 1032, and so
 * before the `keyCode === 229` branch (line 110) that arms E3. That bail is load-bearing:
 * with the textarea kept empty E3 would now emit the *correct* character, but it would be a
 * second emitter for the same keystroke.
 */
export function imeFiltered(k: KeyFacts): boolean {
  return k.isComposing || k.keyCode === 229 || k.key === 'Process'
}

/**
 * The text an `input` event is carrying, or `null` if this app must not write it.
 *
 * Exported for the check script, which asserts the exclusions one at a time.
 */
export function insertedText(ev: InputFacts): string | null {
  if (ev.data === null || ev.data === '') return null
  // Deletions, `historyUndo`, `formatBold`, … . A deletion cannot happen against the empty
  // textarea this module maintains, and if one arrives anyway it means something put text
  // there that we did not put there — writing a DEL at the child on that basis is how
  // `_handleAnyTextareaChanges` produces its phantom backspaces.
  if (!ev.inputType.startsWith('insert')) return null
  // xterm's own `paste` handler already wrote this, with bracketed-paste markers around it
  // (`Clipboard.handlePasteEvent` → `paste`, lines 43-56). It calls `stopPropagation` but
  // *not* `preventDefault`, so the browser goes on to insert the pasted text into the
  // textarea and this `input` event fires behind the write that already happened. Emitting
  // `ev.data` here would paste twice, the second time unbracketed.
  if (ev.inputType === 'insertFromPaste' || ev.inputType === 'insertFromDrop') return null
  return ev.data
}

/**
 * One terminal's textarea discipline.
 *
 * Per terminal, not per app: two panes have two textareas and two independent input methods'
 * worth of state.
 */
export class InputGuard {
  /** Between `compositionstart` and `compositionend`: the textarea holds live preedit. */
  private composing = false
  /**
   * Between `compositionend` and the macrotask after it: `_finalizeComposition` has a
   * `setTimeout(0)` in flight that is going to read the composed text out of the textarea.
   *
   * While this is set the textarea is *not* touched and this app writes nothing — E4 owns
   * the delivery. It is what makes both orderings of the final `input` event correct: whether
   * the browser fires it before `compositionend` (caught by `composing`) or after it (caught
   * by this), exactly one write reaches the child.
   *
   * The window is one macrotask wide and it is not quite airtight: if the event loop runs a
   * whole keystroke *between* `_finalizeComposition`'s timer and the settle timer queued
   * behind it, that keystroke's `input` event is swallowed as part of the commit that has
   * already been read, and its character is dropped. Sub-millisecond, and it needs the user
   * to type inside it. The airtight alternative — settle on the first `onData` after
   * `compositionend`, which *is* E4's emission — lost because E4 emits nothing at all for an
   * empty composition (`if (input.length > 0)`, line 172), so the window would sometimes
   * never close and the pane would go deaf.
   */
  private commitPending = false

  /**
   * Capture phase, on an ancestor of the textarea, so this runs before xterm's own keydown
   * listener — and therefore before E3 snapshots `oldValue` and before any `compositionstart`
   * this key raises records `start`. Both bases are then taken against an empty textarea,
   * which is the entire fix.
   *
   * The one thing lost by clearing here: on Linux, `onLinuxMouseSelection` (line 528) parks a
   * mouse selection in this same textarea so middle-click can paste it, and the next keystroke
   * now wipes it. Accepted — typing already clears the terminal selection through
   * `onUserInput`, so the selection the middle click would have pasted is gone from the screen
   * anyway. The alternative, skipping the clear while the textarea looks like a parked
   * selection, cannot tell that state from ordinary residue after the first keystroke and
   * would reopen the accumulation this exists to stop.
   */
  keydown(_k: KeyFacts): Reaction {
    // Never while a composition is open (the preedit lives in there and the IM is tracking
    // it) and never while a commit is being read out of it.
    if (this.composing || this.commitPending) return NOTHING
    return { stop: false, emit: null, clear: true, defer: false }
  }

  /** Observational: the textarea is already empty, courtesy of the keydown that raised this. */
  compositionstart(): Reaction {
    this.composing = true
    return NOTHING
  }

  /** Bubble phase, after xterm's listener — see [`Reaction.defer`]. */
  compositionend(): Reaction {
    this.composing = false
    this.commitPending = true
    return { stop: false, emit: null, clear: false, defer: true }
  }

  /** The macrotask [`Reaction.defer`] asked for: E4 has read the textarea, so empty it. */
  commitSettled(): Reaction {
    this.commitPending = false
    // Unless a *new* composition opened inside the window. Its `compositionstart` recorded
    // `_compositionPosition.start` against the textarea as it stood then, so emptying it now
    // would leave E4 reading `substring(start)` past the end of a shorter string and the next
    // commit would arrive truncated or not at all. The residue is harmless — that base is
    // already correct for it, and the next keydown outside a composition clears it.
    if (this.composing) return NOTHING
    return { stop: false, emit: null, clear: true, defer: false }
  }

  /**
   * Every `input` event is stopped, without exception.
   *
   * `_inputEvent` (E2) is the emitter that "emits unless a key is physically down right now"
   * — an `input` event is always `composed: true`, so with `screenReaderMode` off its guard
   * (line 1196) collapses to `!this._keyDownSeen`, i.e. it fires or not depending on whether
   * the IM's commit beat `keyup`. A per-keystroke coin flip is not something to reason
   * about; it is something to disconnect. The decision of what to write is made here or by
   * `CompositionHelper`, never by a race.
   */
  input(ev: InputFacts): Reaction {
    // The composed text is in the textarea and `_finalizeComposition` is going to deliver it.
    // Emitting here as well is the doubling; clearing here instead is the dropped character.
    if (this.composing || this.commitPending || ev.isComposing) return SWALLOW

    const text = insertedText(ev)
    // Cleared whether or not anything was written: residue is what every one of these bugs
    // was made of. This is also the ordinary IME path — a keyCode-229 keydown that `xterm.ts`
    // bailed out of, whose commit arrives here as `insertText` with no composition events
    // around it at all.
    return { stop: true, emit: text, clear: true, defer: false }
  }

  /** A blur, a re-parent, a pane leaving the tree: no keystroke is in flight any more. */
  reset(): void {
    this.composing = false
    this.commitPending = false
  }

  /** For the dev probe and the check script. */
  state(): { composing: boolean; commitPending: boolean } {
    return { composing: this.composing, commitPending: this.commitPending }
  }
}
