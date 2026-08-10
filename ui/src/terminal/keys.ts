/**
 * Chords a terminal must encode itself, because xterm.js would otherwise send something the
 * application on the other end cannot tell apart from a different chord.
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
