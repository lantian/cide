/**
 * xterm.js instances and the WebGL renderer pool.
 *
 * # Why there is a pool at all
 *
 * WebKitGTK caps the number of concurrent WebGL contexts (roughly 8-16, engine- and
 * driver-dependent). A 2x2 Claude grid times several open projects, plus shell panes, goes
 * past that, and the failure mode is a context that silently never paints. Worse, the
 * healthy and unhealthy paths are indistinguishable from JS: context creation succeeds
 * even on a software rasterizer, and `WEBGL_debug_renderer_info` is masked — on Linux it
 * reports "Apple GPU" regardless of hardware. So we cannot probe; we can only budget.
 *
 * The pool grants WebGL to the most recently focused [`MAX_WEBGL`] terminals and leaves
 * the rest on the DOM renderer. `addon-canvas` was removed in xterm 6, so DOM is the only
 * fallback there is.
 */
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { WebglAddon } from '@xterm/addon-webgl'
import { UnicodeGraphemesAddon } from '@xterm/addon-unicode-graphemes'
import { terminalKeyGate } from '@/keys/gate'
import { ClipboardAddon } from '@xterm/addon-clipboard'
import '@xterm/xterm/css/xterm.css'

/** How many terminals may hold a WebGL context at once. */
export const MAX_WEBGL = 5

export interface TerminalHandle {
  term: Terminal
  fit: FitAddon
  /** Cell metrics, needed so the PTY gets real pixel dimensions rather than zeroes. */
  cellSize(): { width: number; height: number }
  dispose(): void
}

const webglOrder: TerminalHandle[] = []
const webglAddons = new WeakMap<Terminal, WebglAddon>()

function readTheme(): Record<string, string> {
  const s = getComputedStyle(document.documentElement)
  const v = (name: string) => s.getPropertyValue(name).trim()
  return {
    background: v('--panel'),
    foreground: v('--text'),
    cursor: v('--accent'),
    cursorAccent: v('--panel'),
    selectionBackground: v('--sel'),
    black: v('--panel-2'),
    red: v('--red'),
    green: v('--green'),
    yellow: v('--yellow'),
    blue: v('--blue'),
    magenta: v('--purple'),
    cyan: v('--cyan'),
    white: v('--text'),
    brightBlack: v('--faint'),
    brightRed: v('--red'),
    brightGreen: v('--green'),
    brightYellow: v('--yellow'),
    brightBlue: v('--blue'),
    brightMagenta: v('--purple'),
    brightCyan: v('--cyan'),
    brightWhite: v('--text-hi'),
  }
}

export function createTerminal(): TerminalHandle {
  const style = getComputedStyle(document.documentElement)

  const term = new Terminal({
    // Required by the unicode addon's provider registration.
    allowProposedApi: true,
    fontFamily: style.getPropertyValue('--font-mono').trim() || 'monospace',
    fontSize: 12,
    lineHeight: 1.35,
    cursorBlink: true,
    cursorStyle: 'block',
    scrollback: 5000,
    // The Rust core holds the authoritative screen mirror, so xterm's own scrollback is a
    // convenience rather than the source of truth for reattach.
    convertEol: false,
    theme: readTheme(),
  })

  const fit = new FitAddon()
  term.loadAddon(fit)
  term.loadAddon(new ClipboardAddon())

  // Not optional. Without grapheme-aware widths every wide glyph is measured one cell
  // narrow, and a fullscreen TUI drawn with box characters shears down the right edge —
  // which is exactly what the Claude Code console is.
  term.loadAddon(new UnicodeGraphemesAddon())
  term.unicode.activeVersion = '15-graphemes'

  /*
   * Entry point 1 of the key gate.
   *
   * This runs BEFORE xterm processes the key, and returning false is the only thing that
   * stops `^P` being written to the PTY. A window listener — entry point 2, installed in
   * `App` — is neither sufficient nor correct on its own here: by the time a keydown
   * bubbles to the window, xterm has already forwarded the byte, and Ctrl+P and Ctrl+K
   * both mean something to readline. So the decision has to precede byte forwarding, which
   * is why the gate has two entry points and a test asserting they resolve every chord
   * identically (`ui/scripts/check-key-gate.mjs`).
   */
  term.attachCustomKeyEventHandler(terminalKeyGate)

  const handle: TerminalHandle = {
    term,
    fit,
    cellSize() {
      // xterm does not expose cell metrics publicly; derive them from the rendered
      // dimensions, which is stable once the terminal has opened and fitted.
      const el = term.element
      if (!el || term.cols === 0 || term.rows === 0) return { width: 8, height: 17 }
      const screen = el.querySelector('.xterm-screen') as HTMLElement | null
      const w = screen?.clientWidth ?? el.clientWidth
      const h = screen?.clientHeight ?? el.clientHeight
      return {
        width: Math.max(1, Math.round(w / term.cols)),
        height: Math.max(1, Math.round(h / term.rows)),
      }
    },
    dispose() {
      releaseWebgl(handle)
      term.dispose()
    },
  }

  return handle
}

/**
 * Grant this terminal a WebGL context, evicting the least recently promoted one if the
 * budget is spent.
 *
 * Call after `term.open()` — the addon needs a rendered element.
 */
export function promoteWebgl(handle: TerminalHandle): void {
  if (webglAddons.has(handle.term)) {
    // Already promoted; just refresh its recency.
    const i = webglOrder.indexOf(handle)
    if (i >= 0) webglOrder.splice(i, 1)
    webglOrder.push(handle)
    return
  }

  while (webglOrder.length >= MAX_WEBGL) {
    const victim = webglOrder.shift()
    if (victim) releaseWebgl(victim)
  }

  try {
    const addon = new WebglAddon()
    // A lost context is not recoverable in place: drop back to the DOM renderer rather
    // than leaving a pane that looks alive but never repaints.
    addon.onContextLoss(() => releaseWebgl(handle))
    handle.term.loadAddon(addon)
    webglAddons.set(handle.term, addon)
    webglOrder.push(handle)
  } catch {
    // DOM renderer. Slower, but correct, and on some WebKitGTK/driver combinations it is
    // the only thing that paints at all.
  }
}

export function releaseWebgl(handle: TerminalHandle): void {
  const addon = webglAddons.get(handle.term)
  if (!addon) return
  webglAddons.delete(handle.term)
  const i = webglOrder.indexOf(handle)
  if (i >= 0) webglOrder.splice(i, 1)
  try {
    addon.dispose()
  } catch {
    // Disposing an already-lost context throws; nothing to do about it.
  }
}

/** Repaint every live terminal with the current token values, after a theme switch. */
export function retheme(handles: Iterable<TerminalHandle>): void {
  const theme = readTheme()
  for (const h of handles) h.term.options.theme = theme
}
