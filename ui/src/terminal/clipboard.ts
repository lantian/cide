/**
 * Copy and paste *in a terminal*: one implementation, three callers.
 *
 * The key path (`xterm.ts`), the pane's own context menu (`TerminalPane.tsx`) and the
 * `terminal.paste` command (`keys/dispatch.ts`) all come through here. They used to be three
 * bodies doing measurably different things — the menu's Paste wrote raw bytes at the pty, the
 * command wrote raw bytes at the pty, and the key did nothing at all — which is how a menu item
 * and a keystroke with the same name end up pasting differently into the same pane.
 *
 * # Why `term.paste` and not `session.write`
 *
 * Both of the previous implementations sent the clipboard's text straight down the pty with
 * `session_write`, and the comment above one of them argued that bracketed paste was
 * unreachable because DECSET 2004 "lives in the vt100 mirror on the Rust side and is not on the
 * wire". **That premise is wrong for xterm.** ADR 0003 hands xterm the VT stream: every byte
 * the child prints goes through `term.write`, including the screen mirror replayed on attach
 * (and `cide_pty::Terminal::screen_state` re-emits the bracketed-paste and mouse modes
 * precisely so a rehydrated pane is not left in the wrong one). So
 * `coreService.decPrivateModes.bracketedPasteMode` is accurate per pane, and
 * `Clipboard.paste` — which `Terminal.paste` calls — wraps in `ESC[200~ … ESC[201~` *only* when
 * the child asked for it, normalises `\r\n` to `\r`, and leaves through the same `onData` the
 * pane already writes to the pty.
 *
 * What that buys is not tidiness. Without the wrapper a multi-line paste into `claude` submits
 * at the first newline, and a multi-line paste into `bash` *executes* each line as it arrives —
 * the second of which nobody had written down.
 *
 * # Why the Tauri plugin and not `navigator.clipboard`
 *
 * Settled twice already in this repository and not re-litigated here: the async Clipboard API
 * needs a secure context and a transient activation that a keyboard-driven path does not have
 * in WebKitGTK, and WebKit refuses `document.execCommand('paste')` from page script outright.
 * `editor/clipboard.ts` wraps the plugin and — the part that matters — tells "the clipboard is
 * empty" apart from "this window's capability file lost a line", so a copy that cannot work
 * says why instead of being a control that silently does nothing.
 */
import { notify } from '@/chrome/notices'
import { copyUnavailable, pasteUnavailable, readClipboard, writeClipboard } from '@/editor/clipboard'

/**
 * The three things this module needs from a terminal.
 *
 * Structural rather than xterm's `Terminal`, so a caller can hand over whatever it holds and so
 * nothing here depends on the addon surface. Every implementor is a real xterm instance.
 */
export interface ClipboardTerminal {
  getSelection(): string
  clearSelection(): void
  paste(data: string): void
}

/**
 * Put this terminal's selection on the system clipboard, and drop the selection.
 *
 * Clearing is not cosmetic. A copy writes nothing to the pty, so xterm's `onUserInput` — which
 * is what normally clears a selection — never fires, and the selection would still be there
 * when the user pressed Ctrl+C again meaning *interrupt*. Clearing is what makes the second
 * press an interrupt, which is what every other terminal does.
 *
 * Answers whether anything was copied, so a caller that has a second gesture to offer can tell
 * "nothing was selected" from "the clipboard refused".
 */
export async function copyTerminalSelection(term: ClipboardTerminal): Promise<boolean> {
  const text = term.getSelection()
  if (text === '') return false

  if (!(await writeClipboard(text))) {
    // Loudly. A copy that silently fails is the dead-control pattern this project keeps
    // finding: the user pastes somewhere else and gets whatever was on the clipboard before.
    notify(copyUnavailable() ?? 'The clipboard could not be written.', { kind: 'error' })
    return false
  }

  term.clearSelection()
  return true
}

/**
 * Paste the system clipboard into this terminal, as if it had been typed.
 *
 * A silent return for an empty clipboard — pasting nothing is an ordinary thing to do by
 * accident and does not deserve a notice — and a notice for a clipboard this window is not
 * *allowed* to read, which is a different fact and one only the person reading the report can
 * act on.
 */
export async function pasteIntoTerminal(term: ClipboardTerminal): Promise<void> {
  const text = await readClipboard()
  if (text === null || text === '') {
    const why = pasteUnavailable()
    if (why !== null) notify(why, { kind: 'error' })
    return
  }
  term.paste(text)
}
