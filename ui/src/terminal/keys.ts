/**
 * Chords a terminal must resolve itself: the ones xterm.js would otherwise encode wrongly,
 * and the two clipboard chords that are the terminal's own rather than the key gate's.
 *
 * # Shift+Enter
 *
 * There is no such thing as "Shift+Enter" on a PTY. A terminal sends `\r` for Enter, and by
 * default xterm.js sends exactly `\r` whether or not Shift was held — DEC's key encoding has
 * no modifier bits for the Return key. So a TUI reading raw bytes cannot distinguish the
 * two, and M0's "working Shift+Enter" — a newline inside a Claude prompt rather than a
 * submit — is simply unreachable without an agreed-upon encoding.
 *
 * Two encodings exist and only one of them is the one Claude Code understands:
 *
 * * `CSI 13;2u` (`\x1b[13;2u`), the modifyOtherKeys / kitty-keyboard form. Correct in the
 *   abstract, but it requires the application to have *requested* that mode; sent
 *   unsolicited to a program that did not, it arrives as a visible `[13;2u` in the prompt.
 * * `ESC CR` (`\x1b\r`) — what `claude /terminal-setup` writes into VS Code's
 *   `terminal.integrated.sendKeybindingsToShell` bindings, and therefore what the Claude TUI
 *   is known to parse as "newline, do not submit". No mode negotiation, nothing to request.
 *
 * We send `ESC CR`. The plan names it, and being bit-identical to what the vendor's own
 * setup command produces is worth more here than being theoretically tidier: this is a
 * hand-shake with one specific program, not a general terminal feature.
 *
 * # Ctrl+C and Ctrl+V, and why they are decided here and not in the keymap
 *
 * Neither chord is in `cide_core::keymap::defaults()`, and a Rust test now keeps it that way.
 * The obvious design — bind them with `when: "terminalFocused"` — is wrong, and the reason is
 * already written down in `sidebar/FileTree.tsx`: **these chords are focus-scoped, not
 * context-scoped.** `terminalFocused` is derived from `tab.tree.focused`, a value Rust owns,
 * and it stays true while the caret is in the file tree, the search box, the commit message, a
 * rename field or a picker. A global binding is resolved by the window *capture* listener, so
 * it would `stopPropagation` the keystroke before any of those saw it — killing the file
 * tree's own Ctrl+C/X/V and the native copy and paste of every text input in the app.
 *
 * This module is reached from one place only: the composed handler in `xterm.ts`, which xterm
 * consults for a keydown delivered to *that terminal's* textarea. That location is focus-scoped
 * by construction. And it is reached only after `terminalKeyGate` has passed the stroke
 * through, which is the same contract [`terminalKeyBytes`] already has — a chord the user has
 * bound in `keymap.json` stays bound, and binding `ctrl+c` yourself still wins.
 *
 * ## The rules, and what each one is protecting
 *
 * * **Ctrl+C copies only when there is a selection.** With no selection it returns `null` and
 *   xterm sends `ETX` exactly as it always has — same bytes, same `onUserInput` side effects.
 *   Ctrl+C is the primary escape from a runaway command and from a mid-turn `claude`, so the
 *   empty-selection branch is the one that must never change. A copy writes nothing to the pty,
 *   so `onUserInput` does not fire and the selection would survive; the caller clears it, which
 *   is what makes "select, copy, interrupt" work without reaching for the mouse.
 * * **Ctrl+V pastes in a shell pane and is left alone in a Claude pane.** This is the reported
 *   asymmetry, and its cause is entirely on the child's side. `bash` binds `\x16` to readline's
 *   `quoted-insert`, which swallows it and takes the next keystroke literally — "nothing
 *   happens". Claude Code binds ctrl+v itself and reads the *system* clipboard out of a
 *   subprocess (`xclip -selection clipboard -t image/png -o … || wl-paste …`, with a
 *   `text/plain` fallback), which is how an image reaches a prompt at all. Claiming the chord
 *   there would stop `^V` reaching the CLI and take image paste away — the one thing cide
 *   cannot reimplement, since the Tauri clipboard plugin only reads text.
 *
 * ## Why this matches on `key` where the key gate matches on `code`
 *
 * `keys/chords.ts` resolves a *binding* by physical key, because a binding is a gesture on a
 * keyboard. This rule is not a binding: it shadows the bytes xterm would otherwise produce
 * itself, and xterm decides that from `keyCode` (`Keyboard.ts`, the `ctrlKey && !shiftKey`
 * branch), which follows the character the layout produces. Matching `code` here would let the
 * two disagree on a non-QWERTY layout: the physical C key on Dvorak produces `j`, so we would
 * copy on the keystroke xterm was about to send `^J` for. Matching `key` keeps the pair exact —
 * and where a layout makes them differ anyway, the mismatch costs a copy that does not happen
 * rather than an interrupt that does not happen.
 *
 * Ctrl only, with no `meta` spelling: nothing rewrites this module for macOS the way
 * `keymap::platform_layer` rewrites the keymap. A macOS port starts here.
 *
 * # Why this is a pure function
 *
 * Keeping the decision separate from the xterm wiring is what makes it testable without a
 * DOM, and what keeps the interaction with the key gate visible in one place —
 * `createTerminal` asks the gate first and only reaches this when the gate passed the
 * keystroke through. See `xterm.ts`.
 */

/**
 * The parts of a `KeyboardEvent` a chord is resolved from.
 *
 * Structural rather than `KeyboardEvent` so a test can pass a plain object and so this
 * module needs no DOM lib types. A real `KeyboardEvent` satisfies it.
 */
export interface TerminalKeyEvent {
  /** xterm's custom key handler is called for keydown, keypress *and* keyup. */
  type: string
  key: string
  shiftKey: boolean
  ctrlKey: boolean
  altKey: boolean
  metaKey: boolean
}

/**
 * Which of the two programs a terminal pane is hosting.
 *
 * The only thing the clipboard rule needs to know about a pane, and it is a real distinction
 * rather than a convenience: `claude` handles Ctrl+V itself and `bash` does not. Anything that
 * is not the Claude CLI is `'shell'` — a pane running some other program is a pane where the
 * chord is unclaimed, which is the same situation `bash` is in.
 */
export type TerminalPaneKind = 'claude' | 'shell'

/** What the rule is decided from: the pane, and what is selected in it right now. */
export interface ClipboardFacts {
  /** `term.getSelection()`. The string, not the terminal — the fact, not the object. */
  selection: string
  kind: TerminalPaneKind
}

/**
 * What this keystroke means for the clipboard, or `null` for "not ours".
 *
 * `null` is the load-bearing value: it means the caller returns `true` and *xterm* handles the
 * key, which for Ctrl+C is the interrupt and for Ctrl+V in a Claude pane is `^V` reaching the
 * CLI's own paste. Every branch that is not a copy or a paste has to reach it.
 */
export type ClipboardAction = { kind: 'copy'; text: string } | { kind: 'paste' } | null

/**
 * ESC CR — Shift+Enter as `/terminal-setup` encodes it.
 *
 * Exported so a test can name the constant rather than repeat the escape, and so anyone
 * grepping for `\x1b\r` finds the one place it is written.
 */
export const SHIFT_ENTER = '\x1b\r'

/**
 * The bytes this keystroke should put on the PTY *instead of* whatever xterm would send, or
 * `null` to let xterm handle it normally.
 *
 * Only genuine keydowns resolve: xterm offers keypress and keyup to the same handler, and
 * synthesising on all three would send the sequence three times for one press.
 */
export function terminalKeyBytes(ev: TerminalKeyEvent): string | null {
  if (ev.type !== 'keydown') return null

  // Plain Shift+Enter only. Ctrl+Shift+Enter and friends are left alone deliberately —
  // they are unclaimed, and claiming them here would silently shadow a future keybinding
  // for chords no one has asked this module to encode.
  if (ev.key === 'Enter' && ev.shiftKey && !ev.ctrlKey && !ev.altKey && !ev.metaKey) {
    return SHIFT_ENTER
  }

  return null
}

/**
 * Exactly the modifier shape xterm's own ctrl branch requires.
 *
 * `Keyboard.ts` encodes a control character only for `ctrlKey && !shiftKey && !altKey &&
 * !metaKey`, so any other combination is a keystroke xterm was *not* going to turn into a byte
 * — and shadowing one would be this module inventing a chord rather than shadowing one. It is
 * also what leaves Ctrl+Shift+C and Ctrl+Shift+V free: they are inert in a terminal today, and
 * they stay that way until somebody asks for them.
 */
function plainCtrl(ev: TerminalKeyEvent): boolean {
  return ev.ctrlKey && !ev.shiftKey && !ev.altKey && !ev.metaKey
}

/** Whether this keystroke is the letter `name`, as the layout produced it. See the header. */
function letter(ev: TerminalKeyEvent, name: string): boolean {
  return ev.key.length === 1 && ev.key.toLowerCase() === name
}

/**
 * Whether Ctrl+C / Ctrl+V should reach the clipboard instead of the child.
 *
 * Pure, and driven by facts rather than by a `Terminal`, so `check-terminal-keys.mjs` can run
 * every branch — including the one that must never change, which is that an empty selection
 * yields `null` and the interrupt goes through untouched.
 */
export function terminalClipboardAction(
  ev: TerminalKeyEvent,
  facts: ClipboardFacts,
): ClipboardAction {
  // keydown only. xterm offers keypress and keyup to the same handler, and a copy on all
  // three would write the clipboard three times for one press — and, worse, would return
  // `false` for the keyup of an interrupt.
  if (ev.type !== 'keydown') return null
  if (!plainCtrl(ev)) return null

  if (letter(ev, 'c')) {
    // The interrupt, whenever nothing is selected. This is the branch the feature is allowed
    // to cost nothing: `null` means xterm sends `ETX` with its own side effects (scroll to
    // bottom, clear selection), which is what a mid-turn `claude` and a runaway command are
    // stopped with. Re-implementing the interrupt as a write from a handler was the
    // alternative and it loses twice over: it changes the one keystroke that must never
    // regress, and it sends nothing at all in a pane that has no session id yet.
    //
    // `.trim()`, not `=== ''`, and the difference is the whole safety of this branch. xterm's
    // `getSelection()` is coordinate-derived and never consults `hasSelection()`, so a
    // one-pixel drag across a row boundary while clicking to focus the pane returns `"\n"`,
    // and a double-click that misses a word by one cell returns the run of spaces beside it.
    // Both are selections the user never meant to make, and against `=== ''` both turn the
    // next Ctrl+C — the one aimed at a mid-turn `claude` — into a silent clipboard write.
    //
    // The trade is deliberate and one-sided: the cost of trimming is that a deliberate
    // selection of nothing but whitespace copies nothing, which no one does on purpose; the
    // cost of not trimming is an interrupt that does not happen, which is the one keystroke
    // in this app that must never regress.
    return facts.selection.trim() === '' ? null : { kind: 'copy', text: facts.selection }
  }

  if (letter(ev, 'v')) {
    // Claude's own, deliberately. See the header: taking `^V` here would take image paste
    // with it, and a text-only paste is a strictly worse version of what already works.
    return facts.kind === 'claude' ? null : { kind: 'paste' }
  }

  return null
}
