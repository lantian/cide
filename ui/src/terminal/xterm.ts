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
import { terminalKeyBytes } from './keys'
import { imeFiltered, InputGuard } from './inputRouting'
import {
  paletteSignature,
  terminalPalette,
  TERMINAL_MIN_CONTRAST,
  unresolvedSlots,
} from '@/settings/theme'
import { ClipboardAddon } from '@xterm/addon-clipboard'
import '@xterm/xterm/css/xterm.css'

/** How many terminals may hold a WebGL context at once. */
export const MAX_WEBGL = 5

export interface TerminalHandle {
  term: Terminal
  fit: FitAddon
  /**
   * Whether a composition is open, and whether a commit is being read out of the textarea.
   *
   * Driven entirely by the listeners `openTerminal` installs (`inputHost.ts`). See
   * `inputRouting.ts` — this is the state that keeps xterm's four emitters from writing
   * anything but the character that was typed, exactly once.
   */
  input: InputGuard
  /** Cell metrics, needed so the PTY gets real pixel dimensions rather than zeroes. */
  cellSize(): { width: number; height: number }
  dispose(): void
}

const webglOrder: TerminalHandle[] = []
const webglAddons = new WeakMap<Terminal, WebglAddon>()

/**
 * The colours each terminal is currently wearing, as a [`paletteSignature`].
 *
 * On the terminal rather than a single module-level "current theme", because the two are
 * not the same claim. A terminal built before the first `data-theme` write, or one whose
 * host was evicted and rebuilt, has whatever the palette was when it was constructed — and
 * that is the terminal a switch most needs to reach. A `WeakMap` so a disposed terminal
 * takes its entry with it; `TerminalHandle` is not a stable key across an eviction.
 */
const appliedPalette = new WeakMap<Terminal, string>()

/**
 * The token values a terminal paints with, resolved against a live computed style.
 *
 * The style is passed in rather than read here so a caller that needs other tokens from the
 * same element — `createTerminal` also wants `--font-mono` — pays for one lookup.
 */
function readTheme(style: CSSStyleDeclaration): Record<string, string> {
  return terminalPalette((name) => style.getPropertyValue(name).trim())
}

/**
 * A `:root` token as a number, with the fallback the token file states.
 *
 * `getPropertyValue` returns `'12.5px'` or `''` — the latter in a harness that never loaded
 * `tokens.css` — so the unit is stripped and a bad parse falls back rather than handing
 * xterm a `NaN`, which it rejects at construction and takes the whole terminal with it.
 */
function metric(style: CSSStyleDeclaration, name: string, fallback: number): number {
  const parsed = Number.parseFloat(style.getPropertyValue(name))
  return Number.isFinite(parsed) ? parsed : fallback
}

export function createTerminal(): TerminalHandle {
  const style = getComputedStyle(document.documentElement)
  const theme = readTheme(style)

  const term = new Terminal({
    // Required by the unicode addon's provider registration.
    allowProposedApi: true,
    fontFamily: style.getPropertyValue('--font-mono').trim() || 'monospace',
    /*
     * The mono scale is `tokens.css`'s, not this file's.
     *
     * These were `12` and `1.35`, while `EditorSurface.module.css` set the editor to the
     * mock's 12.5px — so a terminal beside an editor in the same tab drew the same typeface
     * half a pixel smaller, which reads as two different fonts rather than as two sizes.
     *
     * `lineHeight` is a multiplier on the **measured glyph box**, not on the font size:
     * xterm computes `cell = floor(ceil(fontSize × boxEm × dpr) × lineHeight)`. So it cannot
     * be `--lh-code / --fs-code`; the arithmetic that turns 21px of leading into 1.27 is
     * written out beside the tokens, which is also where it has to be redone if the size
     * moves.
     */
    fontSize: metric(style, '--fs-term', 12.5),
    lineHeight: metric(style, '--term-line-height', 1.27),
    /*
     * The floor for a foreground against whatever background actually resolves in its cell.
     *
     * This is the half of terminal legibility a palette cannot reach, and the reported bug is
     * squarely in it. `crates/cide-app/src/cmd/session.rs` sets `COLORTERM=truecolor`, so the
     * Claude Code TUI — whose theme defaults to `dark` and whose `dark` theme sets
     * `text: rgb(255,255,255)` — emits SGR `38;2;255;255;255`. That is a 24-bit literal: it
     * touches no ANSI slot, so **no mapping in `settings/theme.ts` can affect it**, and on this
     * app's white `--panel` it is 1.00:1. Its diff rows survived only because Claude Code paints
     * those with a background of its own (`rgb(34,92,43)`, 7.97:1); everything between and around
     * them — prose, context lines, the whole unchanged part of a diff — was white on white. That
     * is the screenshot: readable coloured bars, blank page.
     *
     * The general shape of it is two programs wanting opposite answers from one colour. One sets
     * a foreground alone and means "readable on your background"; one sets a foreground *and* a
     * background and means "this exact pair". A palette can only answer the first, because it is
     * chosen before either background is known. So the second is answered here, per cell, where
     * both are known: xterm's `ThemeService` resolves the cell's real background — including a
     * program's own SGR 48 and including the selection — and `color.ensureContrastRatio` moves
     * the foreground toward whichever end reaches the ratio (`Color.ts:288`), lightening it over
     * a dark bar and darkening it over a light one.
     *
     * Set to the same constant `check-theme.mjs` gates the palette with, so on the backgrounds
     * this app paints it is *nearly* inert: every ink slot clears the floor on `--panel` and on
     * `--sel` by that gate. Nearly, and the gap is worth naming rather than rounding off. The four
     * exemptions that gate grants are precisely the pairs this repaints — dark ANSI 0 (#0e0e10 on
     * #151518, 1.06:1) comes out #636364 and dark ANSI 8 (#5c5c66, 2.76:1) comes out #6d6d76 the
     * moment either is used as a *foreground*, and light ANSI 15 on `--sel` likewise.
     *
     * That is accepted, not overlooked. Colour 0 is sunk into the background so it works as a
     * fill, and a fill is a background, which is never corrected — so the sink survives exactly
     * where it is meant to be used and stops being invisible where it is not. The alternative that
     * lost was raising those two tokens to clear the floor outright, which would have made the
     * gate and this option agree at the cost of the one property they exist for.
     *
     * Both renderers honour it — `DomRendererRowFactory._applyMinimumContrast` and, via
     * `CharAtlasUtils.configEquals`, the WebGL texture atlas. The (bg, fg) contrast cache carries
     * most of the cost, but **not** for a cell that has a background override: dim, selected and
     * decorated cells set `bgOverride` and skip the cache read entirely
     * (`DomRendererRowFactory.ts:488`). Those pay one `ensureContrastRatio` per *merged span* per
     * row render — per span rather than per cell only because the merge branch `continue`s before
     * reaching this at all.
     *
     * Two things it deliberately does not fix. Powerline separators and box/block glyphs are
     * excluded upstream (`RendererUtils.treatGlyphAsBackgroundColor`) because they are drawn as
     * fills, not as text. And backgrounds are never adjusted at all — which is why the light
     * theme also had to stop painting ANSI 7 and 15 near-black; see `styles/tokens.css`.
     */
    minimumContrastRatio: TERMINAL_MIN_CONTRAST,
    cursorBlink: true,
    cursorStyle: 'block',
    scrollback: 5000,
    // The Rust core holds the authoritative screen mirror, so xterm's own scrollback is a
    // convenience rather than the source of truth for reattach.
    convertEol: false,
    theme,
  })
  // Recorded so `retheme` can tell a terminal that already has these colours from one built
  // under a different palette. A terminal created *after* a switch reads the tokens here and
  // needs no repaint; one created before does, and only the values distinguish them.
  appliedPalette.set(term, paletteSignature(theme))

  const fit = new FitAddon()
  term.loadAddon(fit)
  term.loadAddon(new ClipboardAddon())

  // Not optional. Without grapheme-aware widths every wide glyph is measured one cell
  // narrow, and a fullscreen TUI drawn with box characters shears down the right edge —
  // which is exactly what the Claude Code console is.
  term.loadAddon(new UnicodeGraphemesAddon())
  term.unicode.activeVersion = '15-graphemes'

  /*
   * Entry point 1 of the key gate, plus the chords the terminal has to encode itself.
   *
   * The gate runs BEFORE xterm processes the key, and returning false is the only thing that
   * stops `^P` being written to the PTY. A window listener — entry point 2, installed in
   * `App` — is neither sufficient nor correct on its own here: by the time a keydown
   * bubbles to the window, xterm has already forwarded the byte, and Ctrl+P and Ctrl+K
   * both mean something to readline. So the decision has to precede byte forwarding, which
   * is why the gate has two entry points and a test asserting they resolve every chord
   * identically (`ui/scripts/check-key-gate.mjs`).
   *
   * `attachCustomKeyEventHandler` takes exactly one function and a second call replaces the
   * first, so Shift+Enter cannot be a handler of its own — it has to be composed here, and
   * the composition order is the contract: **the gate decides first**. A chord the user has
   * bound stays bound; only a keystroke the gate passes through is eligible to be re-encoded.
   * Doing it the other way round would make a `shift+enter` binding in `keymap.json`
   * unreachable inside terminals with nothing on screen to say why.
   */
  const input = new InputGuard()

  term.attachCustomKeyEventHandler((ev) => {
    if (ev.type === 'keydown') {
      /*
       * An input method has taken this key: it has *not* been handled here, and something on
       * the textarea path is the only delivery there will be.
       *
       * Returning false stops `_keyDown` at line 1026, before
       * `this._compositionHelper.keydown(event)` at line 1032 — and so before the
       * `keyCode === 229` branch (`CompositionHelper.keydown`, line 110) that arms
       * `_handleAnyTextareaChanges`. That still matters now that the textarea is kept empty:
       * against an empty base that diff would emit the *correct* character, but it would be a
       * second emitter for a keystroke `inputHost.ts` is already delivering.
       *
       * Deliberately WITHOUT `preventDefault`, which is the opposite of every other false
       * return in this file: the textarea has to keep receiving the commit, because the
       * capture-phase listeners in `openTerminal` are what turn it into one byte sequence.
       * Cancelling here would make the character vanish instead of doubling.
       */
      if (imeFiltered(ev)) return false
    }

    /*
     * `_keyPress` is a fifth emitter, and it is the one that breaks the invariant everything
     * else here rests on: *either* `_keyDown` writes the key and cancels the event, *or* the
     * event is left alone and the textarea path delivers it. Read out of the shipped bundle,
     * `_keyDown` has an early `return true` for `ev.key.length === 1 && charCode 65..90` with
     * no modifiers — every capital letter — that neither writes nor cancels. `_keyPress` then
     * writes it (`triggerDataEvent(String.fromCharCode(charCode))`) and calls `cancel(event)`
     * *without* force, which is a no-op because `cancelEvents` defaults to false. So the
     * browser also inserts the character into the textarea, an `input` event follows, and
     * `InputGuard.input` writes it a second time: one Shift+P, `PP` at the child.
     *
     * Returning false stops `_keyPress` at its own guard, before the write, and leaves the
     * default action intact so the character still lands in the textarea. `preventDefault`
     * would be the obvious-looking alternative and it is the wrong one: it suppresses the
     * insertion, and then nothing delivers the character at all.
     *
     * Above the gate rather than below it because the gate already returns pass-through for
     * every non-keydown (`decide`, `ev.type !== 'keydown'`) and `terminalKeyBytes` resolves
     * only keydowns — so no chord resolution is skipped by short-circuiting here.
     */
    if (ev.type === 'keypress') return false

    if (!terminalKeyGate(ev)) return false

    const bytes = terminalKeyBytes(ev)
    if (bytes === null) return true

    // `input` rather than reaching for the PTY directly: it is xterm's own "as if typed"
    // path, so the bytes go out through the same `onData` the pane already listens on, and
    // the side effects of typing — scroll to bottom, clear the selection — still happen.
    // Returning false is what stops xterm also sending its own bare `\r` behind us.
    ev.preventDefault()
    term.input(bytes, true)
    return false
  })

  const handle: TerminalHandle = {
    term,
    fit,
    input,
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
/**
 * Whether this window asked for the WebGL renderer. **It is off unless asked for.**
 *
 * Read from the URL, the same channel every other launch instrument uses, because the choice
 * has to be known before the first `term.open()` and a webview cannot ask Rust anything that
 * early. Set by `CIDE_RENDERER=webgl` — see `crates/cide-app/src/windows.rs`, where the
 * upstream defect that made DOM the default is written down in full.
 *
 * Computed once. It cannot change without a relaunch, and reading `location` per pane promotion
 * would put a URL parse on the path this function exists to keep cheap.
 */
const webglRequested: boolean = (() => {
  try {
    return new URLSearchParams(window.location.search).get('renderer') === 'webgl'
  } catch {
    // No `location` at all — an SSR bundle under a check script. Not requested, then.
    return false
  }
})()

export function promoteWebgl(handle: TerminalHandle): void {
  // Checked before the pool, so a DOM window never takes a context it would only have to give
  // back. Returning here leaves the terminal on the DOM renderer, which is what xterm falls
  // back to when no renderer addon is loaded — the same state `releaseWebgl` and the `catch`
  // below leave it in, so this adds no new code path.
  if (!webglRequested) return

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

/**
 * Repaint every live terminal with the current token values, after a theme switch.
 *
 * # Why one assignment is enough, including for WebGL
 *
 * Read out of the xterm 6 and `addon-webgl` 0.19 sources in `node_modules` rather than
 * assumed, because the WebGL renderer keeps a glyph texture atlas keyed by colour and a
 * stale atlas is precisely the "some terminals did not switch" symptom:
 *
 * `term.options.theme = …` hits the `OptionsService` setter, which fires `onOptionChange`
 * on reference inequality — `readTheme` returns a fresh object every call, so it always
 * fires. `ThemeService` listens for that key, recomputes the colour set, clears its
 * contrast caches and fires `onChangeColors`. `WebglRenderer._handleColorChange` then calls
 * `_refreshCharAtlas`, and `acquireTextureAtlas` keys its cache on the resolved foreground,
 * background and all 256 ansi colours (`CharAtlasUtils.configEquals`) — so a changed
 * palette releases the old atlas and builds a new one — and clears the render model.
 * `RenderService` separately schedules a full refresh off the same event.
 *
 * So the atlas needs no manual `clearTextureAtlas`, and the addon must *not* be disposed
 * and re-created: that would hand back and re-take one of the `MAX_WEBGL` contexts this
 * pool exists to budget, on every theme switch, for no change in what is drawn.
 *
 * # Parked hosts
 *
 * `liveHosts()` yields hosts sitting in the parking div as well as mounted ones, and they
 * are repainted here rather than on the way back in. xterm carries it: while its
 * IntersectionObserver says the screen is hidden, `RenderService._fullRefresh` records
 * `_needsFullRefresh` instead of drawing, and `_handleIntersectionChange` flushes it when the
 * element becomes visible again. A parked host with no measurable size also leaves
 * `WebglRenderer._isAttached` false, and `renderRows` re-acquires the atlas on the first
 * frame after it is connected and measurable. A host that skipped this would re-dock wearing
 * the old palette.
 *
 * A host that was *evicted* has no terminal at all; the one rebuilt in its place reads the
 * tokens in `createTerminal` and is correct without going through here.
 */
export function retheme(handles: Iterable<TerminalHandle>): void {
  // Read once for the whole pass, and read *now*: the caller has just written `data-theme`,
  // and touching a computed style flushes the pending recalculation, so this observes the
  // palette that was switched to rather than the one a frame ago.
  const theme = readTheme(getComputedStyle(document.documentElement))
  const signature = paletteSignature(theme)
  warnUnresolved(theme, signature)

  for (const h of handles) {
    // Skipped only when this terminal already holds these exact colours. The switch is
    // cheap but not free — a full model clear and an atlas rebuild each — and `retheme` is
    // reachable twice for one change while `App`'s effect and the store subscription
    // overlap.
    if (appliedPalette.get(h.term) === signature) continue
    appliedPalette.set(h.term, signature)
    h.term.options.theme = theme
  }
}

/** Signature already reported, so a repaint per snapshot cannot become a log flood. */
let warnedPalette = ''

/**
 * Say something when a token is missing, because nothing else will.
 *
 * xterm's `parseColor` swallows an unparseable value and substitutes its own default, so a
 * palette that forgot `--panel` gives that terminal a stock black background under a white
 * theme with no error anywhere. `check-theme.mjs` catches this before it ships; this is for
 * the case that gets past it.
 */
function warnUnresolved(theme: Record<string, string>, signature: string): void {
  if (warnedPalette === signature) return
  warnedPalette = signature
  const missing = unresolvedSlots(theme)
  if (missing.length > 0) {
    console.warn(
      `[cide] terminal palette unresolved: ${missing.join(', ')} — xterm will substitute its own colours`,
    )
  }
}

/**
 * Repaint every live terminal at the current font tokens.
 *
 * The sibling of {@link retheme}, and it exists for the same reason: a terminal holds resolved
 * numbers, not CSS variables, so a token that changes after construction reaches the editor
 * through the cascade and reaches a terminal through nothing at all. That is why the two
 * Settings font-size controls appeared to do nothing — the editor half would have followed a
 * token had one been written, and this half needed a call that did not exist.
 *
 * Three things have to happen in order and none is optional:
 *
 * 1. The options change, which is what resizes the cell.
 * 2. `fit()` recomputes how many cells the pane now holds — a bigger font means fewer columns
 *    in the same box, and without this the terminal keeps its old grid and clips.
 * 3. The child is told, or it keeps writing at the old width and every wrapped line is wrong.
 *    This is a real `SIGWINCH`, which is the same thing a window resize does.
 *
 * Step 3 is the caller's, because this module does not know about sessions; `onResized` is
 * handed each handle that actually changed so the caller can push geometry for exactly those.
 */
export function refont(handles: Iterable<TerminalHandle>, onResized: (h: TerminalHandle) => void): void {
  const style = getComputedStyle(document.documentElement)
  const family = style.getPropertyValue('--font-mono').trim() || 'monospace'
  const size = metric(style, '--fs-term', 12.5)
  const lineHeight = metric(style, '--term-line-height', 1.27)

  for (const handle of handles) {
    const term = handle.term
    // Skipped when nothing moved. Assigning an unchanged `fontSize` still makes xterm drop its
    // character atlas and re-measure, which on a pane full of output is a visible hitch — and
    // this runs on every settings change, including the ones that are not about fonts.
    if (term.options.fontSize === size && term.options.lineHeight === lineHeight && term.options.fontFamily === family) {
      continue
    }
    term.options.fontFamily = family
    term.options.fontSize = size
    term.options.lineHeight = lineHeight
    try {
      handle.fit.fit()
    } catch {
      // An unlaid-out pane fits to nonsense; the next `ResizeObserver` callback corrects it.
    }
    onResized(handle)
  }
}
