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
 *
 * # Ctrl+V in a Claude pane, and how image paste survives cide claiming the chord
 *
 * The reported bug: *"CTRL+V not working (not paste) text in claude session panel in cases
 * when claude asks, for example plan contains: 1. Yes … 2. Yes … 3. Tell Claude what to
 * change. And i can not paste via CTRL+V into p3. But context menu Paste is working."*
 *
 * cide used to hand `^V` straight to the child in a Claude pane and paste nothing itself. That
 * was deliberate — Claude Code binds ctrl+v and shells out to `xclip -selection clipboard -t
 * image/png -o` / `wl-paste`, which is the **only** way an image reaches a prompt, and cide
 * cannot reimplement it. But it only holds where the CLI is listening for that chord. A
 * numbered-choice prompt, a permission question, a "tell Claude what to change" field: none of
 * those install the paste handler, so the byte was swallowed three processes down and the
 * keystroke did nothing at all, with no way to tell that from a clipboard that was empty.
 *
 * ## The design, and why it is not "ask the clipboard whether it holds an image"
 *
 * That was the obvious shape — probe for an image, forward `^V` when there is one, paste text
 * otherwise — and `tauri-plugin-clipboard-manager` does have `read_image`. It loses on three
 * counts, all measured in `2.3.2`'s own source rather than assumed:
 *
 *  * **The permission is not granted.** `crates/cide-app/capabilities/*.json` list
 *    `clipboard-manager:allow-read-text` and `allow-write-text` and nothing else, so the probe
 *    would reject in every window until a capability file changed — and a capability file
 *    quietly losing a line is the exact failure `editor/clipboard.ts` exists to report on.
 *  * **It is not a probe, it is a decode.** `commands::read_image` calls
 *    `clipboard.read_image()?.to_owned()` and pushes the result into the webview's resource
 *    table, returning a `ResourceId`. Freeing it needs `core:resources:allow-close`, which is
 *    also not granted — so every Ctrl+V would leak a full RGBA bitmap (a 4K screenshot is
 *    ~33 MB) into the window for as long as it lives.
 *  * **It answers a question we do not need answered.** The fallback is to hand the keystroke
 *    to Claude, and Claude then runs the real detection with `xclip`/`wl-paste`. So the only
 *    thing this module has to know is whether the clipboard has **text** — and `read_text`,
 *    which is granted and cheap, answers exactly that.
 *
 * So [`pasteIntoTerminal`] reads the text. Text there: paste it, through `term.paste`, which is
 * strictly better than what the CLI's own text fallback does because it arrives wrapped in
 * bracketed paste. No text there — an image, or an empty clipboard, or a permission this window
 * has lost — and in a Claude pane the `^V` byte is written at the child *after the fact*, so
 * the CLI's handler runs and image paste is exactly as it was.
 *
 * The keystroke is therefore always intercepted and sometimes re-emitted, rather than being
 * conditionally passed through. It has to be that way round: xterm's custom key handler is
 * synchronous and reading the clipboard is not, so by the time the answer arrives the choice
 * between "return true" and "return false" has already been made. Re-emitting costs one IPC
 * round trip of latency on the image path and nothing on the text path.
 *
 * ## What this trades away, named rather than discovered
 *
 * A clipboard holding **both** an image and text now pastes the text where it used to paste the
 * image. Screenshot tools (Spectacle, Flameshot, GNOME Screenshot) offer `image/png` alone, so
 * the ordinary image-paste gesture is untouched; a file copied in a file manager offers
 * `text/uri-list` and usually `text/plain`, and Claude Code's own handler tries image first and
 * falls back to text, so that case lands on the same text either way. The residual is a source
 * that offers both and means the image. Ctrl+Shift+V is deliberately unclaimed (see `keys.ts`)
 * and is where a "paste the other thing" gesture would go if anyone ever asks for one.
 */
import { notify } from '@/chrome/notices'
import { diag } from '@/ipc/client'
import { copyUnavailable, pasteUnavailable, readClipboard, writeClipboard } from '@/editor/clipboard'
import type { TerminalPaneKind } from './keys'

/**
 * The four things this module needs from a terminal.
 *
 * Structural rather than xterm's `Terminal`, so a caller can hand over whatever it holds and so
 * nothing here depends on the addon surface. Every implementor is a real xterm instance.
 */
export interface ClipboardTerminal {
  getSelection(): string
  clearSelection(): void
  paste(data: string): void
  /**
   * `Terminal.input` — write these bytes at the child *as if the user had typed them*.
   *
   * Here rather than `session.write` for the same reason `terminalKeyBytes`' caller uses it:
   * the bytes leave through the `onData` the pane already listens on, so there is no second
   * path to the pty, and typing's side effects (scroll to bottom, drop the selection) still
   * happen. Used by exactly one branch below — handing `^V` back to the Claude CLI — and
   * declared here rather than reached for through a cast so that branch is visible in the
   * type.
   */
  input(data: string): void
}

/**
 * `^V`, the byte xterm would have sent if this module had not claimed the keystroke.
 *
 * Module-private and written out as a constant so the one place that re-emits it and anyone
 * grepping for `\x16` land on the same line — an export nothing imports is the shape this
 * project keeps finding wired to nothing. In `bash` this is readline's `quoted-insert`, which is why it is only
 * ever sent to a Claude pane; in Claude Code it is the chord the CLI binds to its own paste,
 * image included.
 */
const CTRL_V = '\x16'

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
 * **The one implementation.** Ctrl+V (`xterm.ts`), the pane's context menu
 * (`TerminalPane.tsx`) and the `terminal.paste` command (`keys/dispatch.ts`) all end here, and
 * that is what stops the three from doing three different things — which is exactly what the
 * reported bug was, once: the menu pasted and the keystroke did nothing.
 *
 * `kind` decides only what happens when there is **no text to paste**, and the module header
 * carries the whole argument. In a Claude pane the `^V` byte goes to the child, because the CLI
 * is the only thing in the stack that can paste an image and it detects one itself. Anywhere
 * else there is no such handler — `bash` binds `^V` to readline's `quoted-insert`, which
 * swallows it and takes the *next* keystroke literally, which is a worse outcome than nothing —
 * so the empty case stays silent.
 *
 * Silent for an empty clipboard either way: pasting nothing is an ordinary thing to do by
 * accident and does not deserve a notice. A clipboard this window is not *allowed* to read is a
 * different fact and one only the person reading the report can act on, so it is said out loud
 * — but as a `diag` line in a Claude pane rather than a toast, because the `^V` that follows
 * may well succeed where cide could not: the CLI reads the clipboard from its own subprocess
 * and is not subject to this window's Tauri capabilities at all.
 */
export async function pasteIntoTerminal(
  term: ClipboardTerminal,
  kind: TerminalPaneKind,
): Promise<void> {
  const text = await readClipboard()
  if (text !== null && text !== '') {
    term.paste(text)
    return
  }

  const why = pasteUnavailable()
  if (kind === 'claude') {
    if (why !== null) void diag.log(`terminal paste: ${why}; handing ^V to the Claude CLI`)
    term.input(CTRL_V)
    return
  }
  if (why !== null) notify(why, { kind: 'error' })
}
