/** Native macOS Edit-menu events can arrive without a keydown reaching xterm. */
export interface TerminalClipboardHandlers {
  copy(): void
  paste(): void
}

/**
 * Own clipboard events only inside the focused terminal on macOS. Capture on the host,
 * above xterm's textarea and element listeners, so native paste uses the same bracketed-paste
 * and image fallback as the keyboard path instead of being pasted a second time by xterm.
 * The caller handles async failures and registers the returned removal with host cleanup.
 */
export function attachTerminalClipboard(
  host: HTMLElement,
  macos: boolean,
  handlers: TerminalClipboardHandlers,
): () => void {
  if (!macos) return () => {}

  const onClipboard = (event: Event): void => {
    if (event.defaultPrevented || !host.contains(host.ownerDocument.activeElement)) return
    event.preventDefault()
    event.stopPropagation()
    if (event.type === 'copy') handlers.copy()
    else handlers.paste()
  }

  host.addEventListener('copy', onClipboard, true)
  host.addEventListener('paste', onClipboard, true)
  return () => {
    host.removeEventListener('copy', onClipboard, true)
    host.removeEventListener('paste', onClipboard, true)
  }
}
