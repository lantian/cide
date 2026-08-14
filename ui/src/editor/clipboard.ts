/**
 * Cut, copy and paste for the code pane, and the one honest answer about paste.
 *
 * It serves the *terminal* as well now — `terminal/clipboard.ts` reaches the system clipboard
 * through these two wrappers rather than through `clipboard.readText`/`writeText` directly,
 * because the part worth sharing is not the invoke, it is the classification below: "the
 * clipboard is empty" and "this window's capability file lost a line" arrive as the same
 * rejection and mean entirely different things. The module keeps its name and its home; moving
 * it to a neutral directory would be churn over a paragraph.
 *
 * # Why paste needs a plugin at all
 *
 * WebKit refuses `document.execCommand('paste')` from page script — there is no gesture that
 * makes it work, so a hand-rolled Paste item drawn from HTML is a control that is *always*
 * dead. That is the reason `menus/model.ts` leaves the native menu switched on for text inputs
 * (`nativeInTextInputs`, default `true`) and it says so in its own comment: "adding it is the
 * route to flipping this default".
 *
 * This is that route, for the editor only. `tauri-plugin-clipboard-manager` is registered in
 * `cide-app`, so the system clipboard is reachable through `invoke` — see the `clipboard`
 * block at the end of `ipc/client.ts` for why it is invoked directly rather than through the
 * plugin's JS package. The editor's surface is already opted out of the native menu (it is not
 * a text input as far as `wantsNativeMenu` is concerned), so before this change right-clicking
 * code offered no clipboard at all.
 *
 * # Why the availability is probed rather than assumed
 *
 * `clipboard-manager:default` grants **nothing** — the plugin's own default permission set is
 * empty on the grounds that "the clipboard can be inherently dangerous". The two commands are
 * granted explicitly in `crates/cide-app/capabilities/*.json`, and a capability file is exactly
 * the kind of thing that gets copied to a new window class and quietly loses a line. If that
 * happens, Paste must say *why* it is disabled rather than being drawn live and doing nothing —
 * the failure mode this whole feature exists to avoid.
 *
 * So the answer is cached from the first real attempt, and the menu asks for it. Before the
 * first attempt the item is offered: a menu that disabled itself until it had been used once
 * would never be used once.
 */
import { clipboard, diag } from '@/ipc/client'

/**
 * What the last `read_text` told us about whether reading is allowed at all.
 *
 * `null` means "not yet asked". Deliberately **not** seeded by a probe at import time: probing
 * means reading the user's clipboard for no reason they asked for, in every window, at start.
 */
let readable: boolean | null = null

/** Same for writing, which fails independently — the two permissions are separate. */
let writable: boolean | null = null

/**
 * Why Paste is off, or `null` when it is on.
 *
 * A `disabledReason`, so it is the sentence the menu shows in place of the shortcut. Reads as
 * an answer, not as an error code: the user cannot fix a capability file, but the person they
 * report it to can, and the string names the thing to grep for.
 */
export function pasteUnavailable(): string | null {
  return readable === false
    ? 'The clipboard is not readable in this window — clipboard-manager:allow-read-text'
    : null
}

export function copyUnavailable(): string | null {
  return writable === false
    ? 'The clipboard is not writable in this window — clipboard-manager:allow-write-text'
    : null
}

/**
 * The clipboard's text, or `null` when there is none to be had.
 *
 * `read_text` rejects both for an empty clipboard and for a missing permission, and those are
 * not the same fact: the first is ordinary and the second must change what the menu offers
 * next time. They are told apart by the error text — Tauri's permission refusal names the
 * command it blocked — rather than by trying to enumerate libgit2-style error kinds we do not
 * control. A wrong guess here costs a disabled menu item with a wrong reason, never a wrong
 * edit, which is why guessing is acceptable at all.
 */
export async function readClipboard(): Promise<string | null> {
  try {
    const text = await clipboard.readText()
    readable = true
    return text
  } catch (error: unknown) {
    const detail = String(error)
    if (/not allowed|forbidden|permission/i.test(detail)) {
      readable = false
      void diag.log(`editor paste: the clipboard is not readable: ${detail}`)
      return null
    }
    // An empty or non-text clipboard. Not worth a log line every time someone pastes nothing.
    readable = true
    return null
  }
}

/** Put text on the clipboard. Resolves to whether it landed. */
export async function writeClipboard(text: string): Promise<boolean> {
  try {
    await clipboard.writeText(text)
    writable = true
    return true
  } catch (error: unknown) {
    writable = false
    void diag.log(`editor copy: the clipboard is not writable: ${String(error)}`)
    return false
  }
}
