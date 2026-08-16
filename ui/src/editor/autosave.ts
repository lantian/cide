/**
 * Autosave: **when** a buffer may be written without anybody asking. (M15)
 *
 * All of the policy, none of the mechanism. The timers, the blur listener and the write itself
 * are in `EditorSurface.tsx` and `EditorPane.tsx`; every *decision* is [`shouldAutosave`], which
 * is a pure function over a record of facts so that `check-editor.mjs` can drive the whole truth
 * table under node.
 *
 * That split is not tidiness. This feature writes the user's files on a timer, its refusals are
 * the difference between a working IDE and one that silently destroys work, and a rule inside a
 * `useEffect` is a rule no check script can compile — which is where six shipped bugs in this
 * project have already hidden. Pure and **import-free**, the same arrangement as
 * `openBuffers.ts`; do not add an import.
 *
 * # What autosave refuses, and why each one
 *
 * | fact | refuses | because |
 * | --- | --- | --- |
 * | `enabled` off | always | the setting, `settings.editor.autosave` |
 * | not `dirty` | always | nothing to write, and a write is a `didSave`, which is a `cargo check` |
 * | `readOnly` | always | External Libraries sources; the buffer cannot even be typed into |
 * | `conflict` | always | the bar is the user's unanswered question and autosave must not answer it for them |
 * | `agentDiff` | always | **`openDiff` blocks the agent's turn** — see below |
 * | focus still inside the editor | on blur | the find bar is a CodeMirror panel inside `view.dom` |
 * | an overlay or context menu is open | on blur | Ctrl+P is not leaving the file |
 *
 * ## The agent diff, which is the one that costs work rather than a keystroke
 *
 * `cide-app/src/ide.rs` states it: *"`openDiff` blocks an agent turn. The CLI sends it and
 * waits; nothing else happens in that turn."* The collision is the ordinary gesture and not an
 * edge case — the user has unsaved edits in `foo.rs`, Claude proposes a diff of `foo.rs`, and
 * the user **clicks the diff tab to look at it**. That click is a tab switch, a tab switch is a
 * blur, and a blur is a save: `foo.rs` is written underneath a proposal the agent computed
 * against the old bytes. Accepting it then makes Claude's Edit tool either fail its string match
 * or overwrite the edits that were just saved.
 *
 * An explicit Ctrl+S is still allowed in that state. That is the user deciding, and this whole
 * module is about what happens when nobody decided anything.
 *
 * ## Window deactivation is a save, and it is checked first
 *
 * IDEA saves on frame deactivation and it is the behaviour people expect. It has to be tested
 * **before** the chrome exclusions, because when the OS window loses focus `document.activeElement`
 * does not move — so `focusInsideEditor` is still true, and testing it first would make
 * alt-tabbing away from a focused editor the one case that never saves.
 */

/** Idle time with no document change before a dirty buffer is written. */
export const AUTOSAVE_IDLE_MS = 60_000

/**
 * The longest a dirty buffer goes unwritten while somebody keeps typing.
 *
 * A straight 60 s debounce is literally what was asked for and is starved by continuous input:
 * someone typing steadily for twenty minutes never autosaves, because the timer restarts on
 * every keystroke. This project has written that hazard down twice already — `docSync.ts` and
 * `gitCountStore.ts` both carry the note — so the debounce gets a ceiling measured from the
 * moment the buffer went dirty, and the first of the two to fire wins.
 */
export const AUTOSAVE_CEILING_MS = 300_000

/** Which timer or event is asking. */
export type AutosaveReason = 'blur' | 'idle'

/**
 * Everything the decision needs, as values.
 *
 * A record rather than seven parameters, because six of the seven are booleans and a call site
 * that transposes two booleans compiles perfectly and refuses the wrong thing.
 */
export interface AutosaveFacts {
  /** `settings.editor.autosave`. */
  readonly enabled: boolean
  /** The buffer differs from what was last written. */
  readonly dirty: boolean
  /** `file_read` cleared `writable` — a dependency cache or a toolchain source. */
  readonly readOnly: boolean
  /** The "this file changed on disk" bar is up and unanswered. */
  readonly conflict: boolean
  /** A `claudeMcp` diff tab in this project is showing this path. See the module header. */
  readonly agentDiff: boolean
  /** `document.hasFocus()` — false means the OS window was deactivated. */
  readonly windowFocused: boolean
  /** Focus is still somewhere inside `view.dom`: the find bar, or any future in-editor panel. */
  readonly focusInsideEditor: boolean
  /** The command palette, the file picker, Go to line, Find usages… */
  readonly overlayOpen: boolean
  readonly contextMenuOpen: boolean
}

/**
 * May this buffer be written, unasked, right now?
 *
 * Ordered so that the *unconditional* refusals come first and read as a list — a reader
 * checking "does autosave respect the conflict bar" should find one line, not a condition
 * spread across two branches.
 *
 * Skipping is never a loss. The idle timer and the next real blur both still fire, so a save
 * refused because the palette was open happens a moment later, when it is closed.
 */
export function shouldAutosave(reason: AutosaveReason, f: AutosaveFacts): boolean {
  if (!f.enabled) return false
  // Nothing to write. Checked *before* the save rather than inside it, because a save is a
  // `didSave`, and `didSave` makes rust-analyzer re-run flycheck over the workspace — so an
  // unguarded blur save would be a `cargo check` per alt-tab. With the gate here, the first
  // autosave cleans the buffer and every blur after it is free.
  if (!f.dirty) return false
  // A read-only buffer can never become dirty, so this is unreachable in practice — and it is
  // stated anyway, because "unreachable" is a property of two other modules agreeing and this
  // one must not depend on that. External Libraries sources are read-only *by design*: writing
  // one corrupts a crate every project on the machine compiles against.
  if (f.readOnly) return false
  // The bar is a question the user has not answered. Answering it for them — in the direction
  // that discards whatever changed the file — is the single worst thing this feature could do.
  if (f.conflict) return false
  // An agent turn is blocked on a diff of this very file. See the module header.
  if (f.agentDiff) return false

  // An idle timer is not a gesture, so nothing about where the caret is can excuse it: the user
  // stopped typing a minute ago, and where they stopped is not information.
  if (reason === 'idle') return true

  // The window went away. **First**, deliberately — see the module header — because
  // `document.activeElement` does not move when the OS deactivates a window, so every check
  // below would still describe the editor and would veto the one save IDEA is famous for.
  if (!f.windowFocused) return true
  // The find bar is a CodeMirror panel inside `view.dom`, so opening it blurs the content and
  // would otherwise be a save. One test covers it and everything else that ever lives in there.
  if (f.focusInsideEditor) return false
  // Ctrl+P, the palette, Go to line, a right-click. None of them is leaving the file, and all of
  // them take focus. `overlayOpen()` and `contextMenuOpen()` are already this application's
  // vocabulary for the question — `cide_core::commands` lists both as *host* context flags.
  if (f.overlayOpen || f.contextMenuOpen) return false
  return true
}

/**
 * When the next autosave is owed, given when the last edit happened and when the buffer first
 * went dirty. `null` when nothing is pending.
 *
 * Extracted so the "debounce, with a ceiling" arithmetic is drivable rather than being two
 * `setTimeout`s in a closure. The caller arms a timer for this many milliseconds; expiry itself
 * is still the timer's, because unlike the key gate's prefix there is no keystroke afterwards to
 * compare timestamps at.
 */
export function autosaveDelay(
  now: number,
  lastEditAt: number,
  dirtySince: number,
): number | null {
  if (!Number.isFinite(lastEditAt) || !Number.isFinite(dirtySince)) return null
  const idle = lastEditAt + AUTOSAVE_IDLE_MS - now
  const ceiling = dirtySince + AUTOSAVE_CEILING_MS - now
  // Never negative: a delay in the past is a timer that fires on the next tick, which is what
  // `setTimeout` does with a negative value anyway — but saying so here keeps the caller from
  // having to know that.
  return Math.max(0, Math.min(idle, ceiling))
}
