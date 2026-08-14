/**
 * The live half of the keystroke recorder: what is being rebound, and the claim on the
 * keyboard that makes recording possible at all.
 *
 * `keys/recorder.ts` is the arithmetic and knows nothing about commands or IPC. This is what
 * plugs it in — the gate's `capture` on one side, Settings → Keymap on the other — and it is
 * deliberately the same pair of files, in the same shape, as `keys/switcher.ts` /
 * `keys/switcherStore.ts`. The gate has exactly one `capture` hook, so the two claims are
 * composed in `keys/useKeyGate.ts` rather than each installing something of its own.
 *
 * # Why the state lives here and not in the component
 *
 * `settings/KeymapSection.tsx` draws the popup and nothing else. If it owned the claim, then a
 * render that had not committed yet — or a Settings tab hidden by `visibility: hidden`, which
 * is how `TabContent` hides an inactive tab — would leave the gate consulting a claim whose UI
 * nobody can see, swallowing every keystroke in the window with no popup to explain why. The
 * same argument `switcherStore.ts` makes for arming its release watcher from the store, and
 * the reason `KeymapSection` calls [`stopRecording`] from its unmount cleanup.
 *
 * # The two things arming has to clear first
 *
 * * **An open switcher walk.** Holding Ctrl and clicking *Edit* is an ordinary thing to do by
 *   accident, and the walk's keyup latch is not the gate's — it would still be listening, and
 *   the release of that Ctrl would activate another tab out from under the popup.
 * * **An armed chord prefix.** `gate.ts` only consults the capture when `pendingSequence` is
 *   `null`, so a `ctrl+k` typed before the click would eat the first recorded stroke and the
 *   user would be looking at a box that ignored their keystroke.
 */
import { create } from 'zustand'
import { currentKeyGate } from './gate'
import { cancelWalk } from './switcherStore'
import { EMPTY, canSave, capture, extend, sequenceOf, type Recording } from './recorder'

/** What one recording is for. Captured when the popup opens, and read again at the commit. */
export interface RecordTarget {
  /** The command id being bound. */
  readonly command: string
  /** Its title, for the popup's heading. */
  readonly title: string
  /**
   * The `when` of the binding being replaced, copied **verbatim** from the row.
   *
   * Never re-derived, and never `Command::when`. A removal matches on (key, command, `when`),
   * so a context string that is reconstructed rather than copied removes nothing and reports
   * nothing the user will see; and a command's palette clause is a different thing from a
   * binding's keyboard clause — writing one into the other silently scopes a chord the user
   * asked to be unconditional.
   */
  readonly when: string | null
  /** The chord this command answers to today, for the popup's "was" line. `null` if unbound. */
  readonly current: string | null
  /** Write it. Called with the canonical key sequence, once, after the popup has closed. */
  readonly save: (key: string) => void
}

interface RecorderStore {
  /** What is being recorded, or `null` when the popup is closed. */
  target: RecordTarget | null
  /** The strokes captured so far. Meaningless while `target` is `null`. */
  recording: Recording
}

export const useRecorder = create<RecorderStore>(() => ({ target: null, recording: EMPTY }))

/** Open the recorder for one row. */
export function startRecording(target: RecordTarget): void {
  // Both of these are about *other* keyboard state that would otherwise outlive the click.
  // See the module note; neither is defensive tidying.
  cancelWalk()
  currentKeyGate()?.reset()
  useRecorder.setState({ target, recording: EMPTY })
}

/** Close, writing nothing. Escape, a click on the scrim, and the section unmounting. */
export function stopRecording(): void {
  useRecorder.setState({ target: null, recording: EMPTY })
}

/** Ask for a second stroke, so the next press extends instead of replacing. */
export function extendRecording(): void {
  useRecorder.setState((s) => ({ recording: extend(s.recording) }))
}

/**
 * Save what was recorded.
 *
 * Closes first, like every other popup in the app: the write is a round trip, and leaving the
 * box up over a dimmed screen while it happens reads as a click that did nothing.
 */
export function commitRecording(): void {
  const { target, recording } = useRecorder.getState()
  if (target === null || !canSave(recording)) return
  const key = sequenceOf(recording)
  stopRecording()
  target.save(key)
}

/**
 * The gate's `capture` hook for the recorder half.
 *
 * Returns `true` for **every** stroke while the popup is up — the modal claim. A chord being
 * recorded must not also run, and there is no stroke for which "let the keymap have it" is the
 * right answer here: that is what would open the file picker on top of a box asking the user to
 * press Ctrl+P. Closed, it is a `null` check and a `false`; this runs on every keystroke in the
 * app.
 */
export function recorderCapture(stroke: string): boolean {
  const { target, recording } = useRecorder.getState()
  if (target === null) return false

  const action = capture(recording, stroke)
  if (action.kind === 'record') useRecorder.setState({ recording: action.next })
  else if (action.kind === 'cancel') stopRecording()
  else if (action.kind === 'commit') commitRecording()
  return true
}
