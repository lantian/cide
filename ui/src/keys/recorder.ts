/**
 * The keystroke recorder, as arithmetic: what one keypress means while Settings → Keymap is
 * waiting for a chord.
 *
 * ```
 * click Edit on a row     → the recorder is armed and owns every keystroke
 * press Ctrl+Shift+O      → the chip reads ⌃⇧O
 * press Ctrl+Alt+O        → the chip reads ⌃⌥O — a fumbled chord replaces, it does not append
 * click “+ second stroke” → the next press extends, so ⌃K ⌃S is recordable
 * Enter                   → save;  Escape → cancel, nothing written
 * ```
 *
 * Pure functions over strings, with no DOM, no store, no React and no `@/` import — the same
 * shape and for the same reason as `keys/switcher.ts`: `ui/scripts/check-keymap.mjs` compiles
 * this module standalone and drives every rule below, and `ui/scripts/check-key-gate.mjs`
 * compiles it beside the gate to sweep the whole chord space with a recorder armed. The live
 * half — the zustand state, the arm/disarm, the claim the gate consults — is
 * `keys/recorderStore.ts`.
 *
 * # Why the recorder has to own the keyboard outright
 *
 * `keys/gate.ts` installs a **window capture** listener, so it resolves a chord before the
 * event reaches its target. A recorder that listened for its own `keydown` on a dialog — the
 * way `overlays/GoToLine.tsx` handles Escape — would never see Ctrl+P at all: the gate would
 * have opened the file picker on top of the popup asking the user to press a shortcut. So the
 * recorder is a [`KeyGateHost.capture`](./gate.ts), the gate's one stateful claim on a stroke,
 * ranked above the keymap, and while it is armed **every** stroke is consumed. That is not
 * over-reach: a chord you are recording must never also run.
 *
 * # What that costs, named rather than discovered
 *
 * Commit and cancel have to be keystrokes too, or a keyboard-only user can start a rebind and
 * never finish one — the Save button cannot be reached with Tab, because Tab is data. So bare
 * **Enter** saves and bare **Escape** cancels, and those two chords are therefore not
 * recordable from this screen. VS Code makes the same trade with the same two keys.
 *
 * It costs less here than it looks. Every modified spelling is still recordable — `shift+enter`,
 * `ctrl+escape` — and a *bare* Enter or Escape is a binding this app must not have anyway: the
 * window capture gate would swallow it in every text field, every rename box and every terminal
 * in every window. A user who genuinely wants one can still write it in `keymap.json` by hand,
 * which is the escape hatch the whole file is.
 *
 * Bare modifiers never arrive here at all — `gate.ts` answers `PASS` for them before consulting
 * the capture — so there is deliberately no live "⌃⇧…" preview while the user is still holding
 * keys down. Adding one would mean a second keydown/keyup listener beside the gate, and the
 * only thing it would buy is a shimmer before the chip that appears anyway.
 */
import { chipLabel, parseStroke } from './chords'

/**
 * How many strokes one binding may hold.
 *
 * A UI cap, not a format one: `parse_chord` splits on whitespace and `prefixesOf` handles
 * arbitrary depth, so three-stroke sequences already work everywhere. Nothing in this codebase
 * produces or consumes one, and a recorder that let a user build `ctrl+k ctrl+x ctrl+s` would
 * be shipping the only path that could.
 */
export const MAX_STROKES = 2

/** What has been captured so far, and whether the next press adds to it. */
export interface Recording {
  /** Canonical strokes, in order. Empty until the first press. */
  readonly strokes: readonly string[]
  /**
   * True only while the user has asked for a second stroke.
   *
   * The flag is what makes a fumble harmless. Appending unconditionally — the obvious
   * implementation — turns "I meant ⌃⇧O and hit ⌃⇧P first" into the two-stroke sequence
   * `⌃⇧P ⌃⇧O`, silently, because both presses were data. So the default is *replace*, and a
   * sequence is something the user asks for by name and gets exactly one stroke of.
   */
  readonly extending: boolean
}

/** Nothing captured yet. */
export const EMPTY: Recording = { strokes: [], extending: false }

/** What one keystroke means to an armed recorder. Every one of them is consumed. */
export type RecorderAction =
  /** Captured. `next` is the whole recording, not the stroke. */
  | { kind: 'record'; next: Recording }
  /** Bare Enter with something recorded: write it. */
  | { kind: 'commit' }
  /** Bare Escape: close, write nothing. */
  | { kind: 'cancel' }
  /** Consumed and nothing changed — bare Enter with an empty recording. */
  | { kind: 'wait' }

/**
 * Fold one stroke into a recording.
 *
 * Total, and deliberately forgiving at the cap: a press past [`MAX_STROKES`] starts over rather
 * than being ignored. Ignoring it is the version that feels broken — the user has pressed a key
 * in a box that is asking for keys and nothing moved — and there is no state a restart can lose
 * that a further keystroke cannot rebuild.
 */
export function record(current: Recording, stroke: string): Recording {
  const extending =
    current.extending && current.strokes.length > 0 && current.strokes.length < MAX_STROKES
  return {
    strokes: extending ? [...current.strokes, stroke] : [stroke],
    extending: false,
  }
}

/**
 * Ask for one more stroke. The "+ second stroke" button, as a function.
 *
 * A no-op with nothing recorded (there is nothing to extend) and at the cap, so the button can
 * be pressed twice without producing a state the recorder cannot represent.
 */
export function extend(current: Recording): Recording {
  if (!canExtend(current)) return current
  return { ...current, extending: true }
}

/** Is there room for another stroke, and something to attach it to? The button's `disabled`. */
export function canExtend(current: Recording): boolean {
  return current.strokes.length > 0 && current.strokes.length < MAX_STROKES
}

/** The key string a binding would carry: strokes joined by the single space Rust splits on. */
export function sequenceOf(current: Recording): string {
  return current.strokes.join(' ')
}

/** Is there anything to save? */
export function canSave(current: Recording): boolean {
  return current.strokes.length > 0
}

/**
 * What the popup shows: the chip text, plus a trailing ellipsis while a second stroke is owed.
 *
 * `⌃K …` is the shape `KeyGateHost.onPending` was written for, and using it here keeps the two
 * readouts — "a prefix is armed" and "a prefix is being recorded" — spelled the same way.
 */
export function chipFor(current: Recording): string {
  const chip = current.strokes.length === 0 ? '' : chipLabel(sequenceOf(current))
  if (!current.extending) return chip
  return chip === '' ? '…' : `${chip} …`
}

/**
 * What an armed recorder wants done with one keystroke.
 *
 * The order of the rules is the design, and the first two are the ones that cost something:
 *
 * 1. **Bare Escape cancels.** Unmodified only — `shift+escape` is a chord somebody may want,
 *    and a real keyboard reports `ctrl+escape` when Escape is pressed with Ctrl down.
 * 2. **Bare Enter commits**, and does nothing at all when nothing has been recorded, so the
 *    first thing a user presses cannot save an empty binding.
 * 3. **Everything else is data.** Including Tab, Space, F-keys and every modified spelling of
 *    Enter and Escape.
 */
export function capture(current: Recording, stroke: string): RecorderAction {
  const chord = parseStroke(stroke)
  // Unreachable from the gate, which only ever hands over a stroke it rendered itself. Answered
  // rather than thrown because this is a capture: throwing here would propagate out of a
  // `keydown` handler and leave the recorder armed with no way to close it.
  if (chord === null) return { kind: 'wait' }

  const bare = !chord.ctrl && !chord.alt && !chord.shift && !chord.meta
  if (bare && chord.key === 'escape') return { kind: 'cancel' }
  if (bare && chord.key === 'enter') {
    return canSave(current) ? { kind: 'commit' } : { kind: 'wait' }
  }
  return { kind: 'record', next: record(current, stroke) }
}
