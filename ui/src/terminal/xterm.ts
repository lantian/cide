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
import { notify } from '@/chrome/notices'
import { showLogDetail } from '@/chrome/logDetailStore'
import { paneSession } from '@/ipc/client'
import { parseLogLink } from './logLink'
import { terminalKeyGate } from '@/keys/gate'
import {
  terminalClipboardAction,
  terminalKeyBytes,
  terminalOpensFind,
  type TerminalPaneKind,
} from './keys'
import { copyTerminalSelection, pasteIntoTerminal } from './clipboard'
import { openTerminalFind } from './findStore'
import { FIND_HIGHLIGHT_LIMIT } from './findModel'
import { SearchAddon, type ISearchOptions } from '@xterm/addon-search'
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

/**
 * The version cide reports in its XTVERSION reply — see the handler in [`createTerminal`].
 *
 * A literal, because `@xterm/xterm` exports no version constant and importing its
 * `package.json` would need a bundler resolution `check:paths` cannot reproduce. `check:paths`
 * asserts it equals the pinned dependency instead, so the drift this literal invites is caught
 * by the one gate that can see both numbers. It is a claim about what cide *is*; announcing a
 * version cide does not run would be the same lie as announcing VS Code, only smaller.
 */
export const XTERM_VERSION = '6.0.0'

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

/**
 * A string the child chose, made fit to sit in one line of a toast.
 *
 * Control characters out and a length cap, because the only thing between an OSC 8 URI and the
 * notice stack is this function, and the URI came from a program's output. It is shown and
 * never parsed, never followed, and never handed to anything that could act on it.
 */
function oneLine(text: string): string {
  const flat = text.replace(/[\u0000-\u001f\u007f]/g, ' ')
  return flat.length > 120 ? `${flat.slice(0, 119)}…` : flat
}

/**
 * Build a terminal for a pane of `kind`.
 *
 * The kind is a constructor argument rather than something set afterwards because it decides a
 * keystroke, and a terminal that existed for one frame without knowing what it was hosting is a
 * terminal that could have swallowed a `^V` the Claude CLI was waiting for. A pane's kind never
 * changes — `PaneKind` is domain state and a Claude pane does not become a shell — so there is
 * nothing to update later.
 *
 * `paneId` is here for the same reason and it is a *parameter* rather than something the caller
 * sets on the handle afterwards, deliberately: the composed key handler below needs it to open
 * this pane's find bar, and a handle that spent one frame not knowing its pane is a terminal
 * whose first Ctrl+F silently did nothing. Making it an argument is what stops that being
 * possible — the compiler asks for it at the one call site (`layout/paneHosts.ts`), where the
 * id is already in hand. A terminal is built for exactly one pane and never moves between
 * panes; hosts are keyed by pane id and outlive every mount.
 */
export function createTerminal(kind: TerminalPaneKind, paneId: string): TerminalHandle {
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
    /*
     * OSC 8 hyperlinks, and the reason this option is not optional.
     *
     * xterm 6 registers `OscLinkProvider` in the terminal's own constructor
     * (`CoreBrowserTerminal.ts:160`) — it is always on, it cannot be turned off, and it outranks
     * every provider an embedder adds. When no `linkHandler` is set, clicking one of its links
     * runs `defaultActivate` (`OscLinkProvider.ts:114`): a `confirm()` followed by
     * `window.open()` **inside this app's own webview**.
     *
     * So until this line existed, any program in any pane could emit `ESC ] 8 ;; https://… ST`
     * and a single click on the text it wrapped would navigate part of the application to a URL
     * that program chose. Terminal output is untrusted data; that was a click-to-navigate path
     * out of it, and nothing in this repository had ever named it.
     *
     * The handler refuses, and says so. Refusing *silently* was the tempting version and is the
     * defect this project keeps finding: a link that underlines, takes a click and does nothing
     * is indistinguishable from one wired to nothing. Opening it for real is a separate decision
     * with a separate threat model — it would have to go through Rust and `tauri_plugin_opener`,
     * because the JS opener command is capability-gated per window and a detached-pane window
     * deliberately has no `opener` permission, so a JS-side open would work in the shell window
     * and silently do nothing in a torn-out pane.
     *
     * `allowNonHttpProtocols` is left at its default of `false`, which is what makes
     * `OscLinkProvider` drop `file:`, `javascript:` and everything else before it ever gets
     * here. File paths in output are a different mechanism entirely — see `pathLinks.ts`, which
     * resolves them against the project and opens a tab, never a URL.
     */
    linkHandler: {
      /*
       * `allowNonHttpProtocols` is on so that `cide-log:` reaches this function at all — with
       * it at its default of `false`, `OscLinkProvider` drops every non-http URI before a
       * handler is ever asked, which is what used to make the note below the whole story.
       *
       * Turning it on does not widen what cide *opens*, because this handler opens nothing it
       * does not recognise: `file:` and `javascript:` from a program's own OSC 8 sequence land
       * in the same refusal `https:` does. xterm never navigates on its own once a
       * `linkHandler` is installed — it calls this — so the decision stays here, in one place,
       * where it can be read.
       */
      allowNonHttpProtocols: true,
      activate: (_event, uri) => {
        // A rendered JSON log line's timestamp. See `terminal/logLink.ts` for the URI and
        // `cide_app::logring` for what is on the other end of the handle.
        const target = parseLogLink(uri)
        if (target !== null) {
          showLogDetail(() => paneSession.logDetail(target.session, target.handle))
          return
        }
        notify(`cide does not open web links from terminal output: ${oneLine(uri)}`, {
          kind: 'warn',
          hint: 'Copy the address and open it in a browser.',
        })
      },
    },
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

    // The gate also rewrites a chord typed under a non-Latin layout to its US spelling here —
    // `keys/latin.ts`, from inside `terminalKeyGate` — so every `ev.key` read below this line
    // sees `c` rather than `с`, and xterm's own `keyCode` branch sees 67 rather than 0.
    if (!terminalKeyGate(ev)) return false

    /*
     * Ctrl+C / Ctrl+V, decided by `terminalClipboardAction` and by nothing else.
     *
     * Here rather than in the keymap because this handler is the app's only *focus-scoped*
     * keyboard entry point: xterm consults it for a keydown delivered to this terminal's own
     * textarea, so a rule written here cannot reach the file tree, a rename field or the commit
     * box. The window-capture gate can and would — see `keys.ts`, which carries the whole
     * argument, and `sidebar/FileTree.tsx`, where it was first written down.
     *
     * Below the gate, like `terminalKeyBytes`: a user who binds `ctrl+c` in `keymap.json` still
     * wins, and both entry points of the gate still agree about every chord, so `check:keys` is
     * unaffected by any of this.
     *
     * The action is run without being awaited and the handler returns `false` synchronously.
     * That ordering is the whole of the interception: the bytes are stopped by the return
     * value, not by the promise, so nothing is racing the pty.
     */
    const action = terminalClipboardAction(ev, { selection: term.getSelection(), kind })
    if (action !== null) {
      ev.preventDefault()
      const done =
        action.kind === 'copy' ? copyTerminalSelection(term) : pasteIntoTerminal(term, kind)
      void done.catch((error: unknown) => {
        notify(`The clipboard could not be reached: ${String(error)}`, { kind: 'error' })
      })
      return false
    }

    /*
     * Ctrl+F — this pane's find bar, decided by `terminalOpensFind` and by nothing else.
     *
     * Here for exactly the reason the two clipboard chords are, and `keys.ts` carries the whole
     * argument: this handler is the app's only *focus-scoped* keyboard entry point, so a rule
     * written here cannot reach a rename field, the commit box or CodeMirror's own Ctrl+F,
     * whereas a `keymap.json` default with `when: "terminalFocused"` would have reached all
     * three (that flag follows `tab.tree.focused`, not the caret).
     *
     * Below the gate, like everything else in this handler: a user who binds `ctrl+f` in
     * `keymap.json` still wins, and both entry points of the gate still agree about every chord.
     *
     * The pane id is the constructor's, not a lookup: see [`createTerminal`]'s own note for why
     * a terminal knows its pane from the first frame rather than being told later.
     */
    if (terminalOpensFind(ev)) {
      ev.preventDefault()
      openTerminalFind(paneId)
      return false
    }

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

  /*
   * XTVERSION (`CSI > 0 q`), answered truthfully, because a program that asks it is asking
   * whether it is talking to xterm.js — and cide is.
   *
   * This is the second half of M15's terminal-click report, and it fixes a hole cide's own
   * click gate cannot reach. Claude Code claims ctrl+click **and alt+click** for its own
   * hyperlink opener, gated on `(button & 24) !== 0` — Ctrl(16) or Alt(8) in the SGR mouse
   * encoding — and for a `file:` target that opener forks
   * `dbus-send … org.freedesktop.FileManager1.ShowItems`, which opens the desktop file manager
   * on the containing directory. cide claims ctrl+click (`clickGate.ts`) and deliberately does
   * not claim alt+click, because alt+click means something to the child; so without this line
   * an alt+click in a Claude pane would still fork a file manager, and nothing in cide's source
   * would say so.
   *
   * Claude Code already has a rule for exactly this situation. Read out of the shipped binary:
   *
   * ```js
   * function tx(){ if(YA()?.isVscodeTerm) return true
   *                if(process.env.TERM_PROGRAM==="vscode") return true
   *                return qY().xtversionName?.startsWith("xterm.js") ?? false }
   * ```
   *
   * and the click path is guarded on `!tx()`. It stands down for xterm.js-family hosts because
   * they do their own ctrl+click — which is precisely the division of labour cide wants. cide
   * failed the test only because `@xterm/xterm` 6.0.0 does not implement XTVERSION at all
   * (verified: no `XTVERSION`, no `>|` anywhere in `lib/xterm.js`), so `xtversionName` stayed
   * undefined and `tx()` was false.
   *
   * The reply shape is `DCS > | <name> ST`, which is what the CLI's own parser expects
   * (`/^\x1bP>\|(.*?)(?:\x07|\x1b\\)$/`). `prefix: '>'` keeps this clear of DECSCUSR
   * (`CSI Ps SP q`), which has an intermediate space and no prefix.
   *
   * **Not** `TERM_PROGRAM=vscode`, which was the tempting one-line alternative and is a lie:
   * that variable moves half a dozen other Claude Code behaviours (OSC 52 clipboard
   * workarounds, the modifier hint text, DECSTBM gating), and cide is not VS Code. Announcing
   * `xterm.js(6.0.0)` is simply true. It has one other effect, and it is the wanted one:
   * `tx()` also selects Claude Code's xterm.js wheel/drain profile (`useAdaptiveDrain`), which
   * is the profile written for this renderer.
   *
   * `term.input(reply, false)` — `false` for `wasUserInput`, because this is not a keystroke:
   * `true` would scroll the viewport to the bottom and clear the selection behind the user's
   * back. The bytes leave through the same `onData` the pane already listens on.
   */
  term.parser.registerCsiHandler({ prefix: '>', final: 'q' }, (params) => {
    // `CSI > 0 q` and `CSI > q` both mean XTVERSION; anything else with this prefix and final is
    // not a request this knows how to answer, and must fall through rather than be swallowed.
    const first = params[0]
    const ps = Array.isArray(first) ? first[0] : first
    if (ps !== undefined && ps !== 0) return false
    term.input(`\x1bP>|xterm.js(${XTERM_VERSION})\x1b\\`, false)
    return true
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

/**
 * The search addons in this window, keyed by the terminal they were loaded into.
 *
 * A `WeakMap` for the reason `webglAddons` above is one: a disposed terminal takes its entry
 * with it, and `TerminalHandle` is not a stable key across an eviction. Nothing here disposes
 * the addon explicitly — xterm's `AddonManager.dispose()` disposes every loaded addon when the
 * terminal goes, which is what `TerminalHandle.dispose` reaches.
 */
const searchAddons = new WeakMap<Terminal, SearchAddon>()

/**
 * This terminal's search addon, loaded on first use.
 *
 * **Lazily, and that is a decision rather than an optimisation.** `SearchAddon.activate`
 * registers an `onWriteParsed` listener that re-runs the last query 200 ms after every write
 * batch, so that the count and the highlights stay true while a child keeps printing. That is
 * exactly right for a pane somebody is searching and it is pure overhead for the other eleven
 * hosts the registry may be holding, most of which will never be searched at all. Loading on
 * demand means a window full of idle Claude panes pays nothing.
 *
 * (The listener is cheap even once loaded — it returns immediately unless a query is cached
 * *and* decorations are on — so the addon is never unloaded again. xterm's `AddonManager` has no
 * "unload and reload" that keeps the terminal's state, and a bar that is closed leaves no cached
 * query behind because `clearDecorations` drops it.)
 */
export function ensureSearch(handle: TerminalHandle): SearchAddon {
  const existing = searchAddons.get(handle.term)
  if (existing) return existing

  const addon = new SearchAddon({ highlightLimit: FIND_HIGHLIGHT_LIMIT })
  handle.term.loadAddon(addon)
  searchAddons.set(handle.term, addon)
  return addon
}

/**
 * A CSS token as a `#RRGGBB` literal, or `null` when it is not one.
 *
 * xterm's decoration options accept **only** that form — `ISearchDecorationOptions` says so and
 * its colour parser is not CSS's — so a token that resolves to `rgb(…)`, to a shorthand `#abc`,
 * or (in a harness that never loaded `tokens.css`) to the empty string has to be rejected here
 * rather than handed over. The tokens this reads are 6-digit hex in both themes today; the guard
 * is for the edit that changes one, which would otherwise show up as a search that highlights
 * nothing and reports no count, with nothing anywhere saying why.
 */
function hexToken(style: CSSStyleDeclaration, name: string, fallback: string): string {
  const value = style.getPropertyValue(name).trim()
  return /^#[0-9a-fA-F]{6}$/.test(value) ? value : fallback
}

/**
 * What a search should paint, resolved against the theme that is on screen right now.
 *
 * Read per search rather than cached: a theme switch repaints every terminal
 * (`retheme` below) and the decorations would otherwise keep the colours of whichever theme was
 * up when the bar opened. The cost is two `getPropertyValue` calls per keystroke in a find
 * field, which is nothing beside the search itself.
 *
 * **`decorations` is not optional and is not cosmetic.** `SearchAddon` fires
 * `onDidChangeResults` only when the search it just ran carried a `decorations` block
 * (`ResultTracker.fireResultsChanged` returns immediately otherwise), so switching highlighting
 * off does not buy a cheaper count — it removes the count entirely, and the bar's `3 of 12`
 * with it.
 *
 * The two `…OverviewRuler` fields are required by the type and are inert here: an overview ruler
 * is drawn only for a terminal constructed with `overviewRulerWidth`, and none is. They are
 * given the ring's and the fill's colours so that switching one on later needs no second
 * decision.
 *
 * # Why the current hit is a ring and not a loud fill, and why `--sel` is not the wash
 *
 * Both halves of this were got wrong once, in this function, and both are failures the rest of
 * the repository had already written down:
 *
 *  * **`--sel` is not a neutral wash here — it is literally this terminal's selection colour.**
 *    `settings/theme.ts::TERMINAL_SLOTS` maps `selectionBackground → --sel`, so painting matches
 *    with it makes a highlight indistinguishable from selected text; and the addon *selects* the
 *    match it moves to (`SearchAddon._selectResult` calls `terminal.select`), so the one hit that
 *    most needs to stand out was the one wearing the selection's own colour twice over.
 *    `--accent-dim` instead, which is what `EditorSurface.module.css` uses for `.cm-searchMatch`
 *    and for the same stated reason: *a hit is the app's own emphasis, not a warning* — and here,
 *    not a selection either.
 *  * **A solid `--accent` for the active match was tried in the editor and is unusable.** That
 *    file's comment carries the measurement: this decoration cannot choose the ink drawn on top
 *    of it, so a loud fill ends up carrying whatever colour the text already had at about 1.1:1.
 *    In a terminal that is *worse*, not better — the ink is an arbitrary ANSI palette a child
 *    program chose, and xterm applies a decoration's `backgroundColor` as the **cell background**
 *    with the glyph's own foreground over it (`CellColorResolver`, both renderers), not as an
 *    overlay it could tint. So the same answer the editor reached: the current hit is the same
 *    fill plus a 1px `--accent` ring. `SearchAddon` renders `activeMatchBorder` as exactly that
 *    (`_applyStyles` sets `outline: 1px solid …` on the decoration element), which is the idiom
 *    `.cm-searchMatch-selected` already uses.
 *
 * `activeMatchBackground` is still handed the fill rather than left out. Omitting it would make
 * the active decoration contribute no background at all, and the two renderers disagree about
 * whether the highlight decoration underneath then survives — a hit that flickers between two
 * colours as you walk it is a worse bug than the one this replaced.
 */
export function searchOptions(): ISearchOptions {
  const style = getComputedStyle(document.documentElement)
  const fill = hexToken(style, '--accent-dim', '#7a4432')
  const ring = hexToken(style, '--accent', '#d97757')
  return {
    decorations: {
      matchBackground: fill,
      matchOverviewRuler: fill,
      activeMatchBackground: fill,
      activeMatchBorder: ring,
      activeMatchColorOverviewRuler: ring,
    },
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
