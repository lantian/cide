/**
 * What a mouse press inside a terminal pane means, and which buffer cell it landed on.
 *
 * Three pure functions and no imports, for the reason `pathMatch.ts` next door states and this
 * module is the proof of: **the rule that shipped broken lived inside a DOM callback**, which is
 * the one place `ui/scripts/check-paths.mjs` cannot compile. `pathLinks.ts` keeps the DOM, the
 * IPC and the xterm handles; every decision it makes about a press is made here.
 *
 * # The bug this module exists because of
 *
 * The ctrl+click gate used to read:
 *
 * ```ts
 * swallowed = false
 * if (ev.button !== 0) return
 * if (!ev.ctrlKey && !ev.metaKey) return
 * const target = hovered
 * if (target === null) return          // <-- returns BEFORE preventDefault
 * ev.preventDefault(); ev.stopPropagation(); …
 * ```
 *
 * The last early return is the defect. `hovered` is only set once xterm's `Linkifier` has fired a
 * `mousemove` **and** the async existence probe behind it has answered, and in a Claude Code pane
 * neither is reliable: the alt-screen TUI repaints the transcript under a stationary pointer, so
 * no `mousemove` ever fires, and on a fresh line there is a real window between the first move and
 * the probe's reply. When the gate returned early, xterm's own always-on `mousedown` handler ran,
 * wrote an SGR mouse report to the pty with the Ctrl bit set, and the child acted on it.
 *
 * What the child does with it is the visible symptom. Claude Code claims ctrl+click and alt+click
 * for its own hyperlink opener — `(button & 24) !== 0`, i.e. Ctrl(16) or Alt(8) — and for a
 * `file:` target that opener is
 * `dbus-send … org.freedesktop.FileManager1.ShowItems`, which opens the desktop **file manager**
 * at the containing directory with the item selected. So a ctrl+click on a path in a Claude pane
 * opened Dolphin on the parent folder. Nothing in cide's own source could be grepped for it: the
 * fork happens three processes away, in a child cide had *delegated the gesture to* by not
 * swallowing it.
 *
 * # Decision: a ctrl/meta+left press inside a terminal pane is cide's, always
 *
 * [`pressVerdict`] takes no hover state and cannot. A press cide has claimed is stopped in capture
 * before xterm sees it, so no mouse report is written and no `mouseup` listener is even
 * registered (xterm adds the document-level `mouseup` *inside* its `mousedown` handler, so
 * swallowing the press is sufficient to swallow the whole gesture). What happens next — a file
 * opens, a directory is revealed, or a sentence says why neither can — is decided *afterwards*,
 * against the buffer, at press time.
 *
 * Resolving at press time rather than trusting `hovered` also fixes the mirror-image bug in the
 * same three lines: a **stale** `hovered` survives a repaint, so the old gate could act on a
 * candidate from a line that had since been overwritten.
 *
 * # What is deliberately NOT claimed
 *
 * * **Alt+click.** Claude Code claims it too, and it means something to the child — its own
 *   block-selection anchor. cide has never bound alt+click to anything, so swallowing it would be
 *   taking a gesture away from the program the user is talking to in order to do nothing with it.
 *   The alt+click half of the file-manager problem is answered instead by
 *   `xterm.ts`'s XTVERSION reply, which makes Claude Code stand down from *both* chords — see
 *   there.
 * * **Any button but the left one.** Middle-click paste and the pane's own context menu are
 *   untouched, which is what the old `ev.button !== 0` line already got right.
 * * **A plain left click.** It focuses the pane, and under a mouse-tracking TUI it is a click the
 *   child is entitled to. `pathLinks.ts` answers a plain click on a link with one sentence naming
 *   the gesture that opens it.
 */

/** What the capture-phase `mousedown` listener should do with a press. */
export type PressVerdict =
  /** Not ours. Let it reach xterm, and through xterm the child. */
  | 'ignore'
  /**
   * Ours. Stop it in capture, then work out what it pointed at.
   *
   * **Unconditional on purpose.** Returning `ignore` because nothing has resolved yet is what
   * forwarded the press to `claude`, and a gesture forwarded is a gesture delegated.
   */
  | 'claim'

/** The parts of a `MouseEvent` the verdict depends on. Structural, so a test needs no DOM. */
export interface PressLike {
  readonly button: number
  readonly ctrlKey: boolean
  readonly metaKey: boolean
}

/**
 * Whether cide takes this press.
 *
 * `metaKey` alongside `ctrlKey` because macOS spells the same gesture with Cmd, and because the
 * keymap's own `mod` normalisation already treats the two as one — a rule that disagreed with the
 * keymap about which modifier opens a link would be a second answer to the same question.
 */
export function pressVerdict(ev: PressLike): PressVerdict {
  if (ev.button !== 0) return 'ignore'
  if (!ev.ctrlKey && !ev.metaKey) return 'ignore'
  return 'claim'
}

/** A rectangle in client coordinates — a `DOMRect`, minus the parts nothing here reads. */
export interface Rect {
  readonly left: number
  readonly top: number
  readonly width: number
  readonly height: number
}

/** The terminal's grid, and where the viewport currently sits in the buffer. */
export interface Grid {
  readonly cols: number
  readonly rows: number
  /** `buffer.active.viewportY` — the 0-based buffer index of the viewport's first row. */
  readonly viewportY: number
}

/**
 * A buffer position in the coordinates xterm's `ILink.range` uses: **1-based, both axes**.
 *
 * 1-based rather than converted at the last moment because the only thing this is ever compared
 * against is a link range, and two coordinate conventions in one comparison is how an off-by-one
 * becomes a link that opens from every column except its first.
 */
export interface Cell {
  readonly x: number
  readonly y: number
}

/**
 * Which buffer cell a pointer at `point` is over, or `null` if it is not over the grid at all.
 *
 * Derived from the *screen* element's rect divided by the grid, rather than from a measured cell
 * size: `TerminalHandle.cellSize()` rounds to whole pixels for layout purposes, and 80 columns of
 * accumulated rounding drift is several columns by the right-hand edge of a wide pane. The
 * division here is exact by construction — the screen element is precisely `cols × rows` cells.
 *
 * xterm's own `MouseService.getCoords` does the same division. It is not importable (nothing in
 * the public surface exposes it), so this is the transcription, and it is checked against a real
 * link range by `check-paths.mjs` rather than trusted.
 */
export function cellFromPoint(
  point: { readonly clientX: number; readonly clientY: number },
  rect: Rect,
  grid: Grid,
): Cell | null {
  if (rect.width <= 0 || rect.height <= 0 || grid.cols <= 0 || grid.rows <= 0) return null
  const col = Math.floor(((point.clientX - rect.left) / rect.width) * grid.cols)
  const row = Math.floor(((point.clientY - rect.top) / rect.height) * grid.rows)
  // Outside the grid is `null`, not a clamp. A press on the scrollbar or in the padding below the
  // last row is not a press on the last row's text, and clamping would silently make it one.
  if (col < 0 || col >= grid.cols) return null
  if (row < 0 || row >= grid.rows) return null
  return { x: col + 1, y: grid.viewportY + row + 1 }
}

/** An xterm link range: 1-based, `end.x` inclusive. The shape `ILink.range` has. */
export interface LinkRange {
  readonly start: { readonly x: number; readonly y: number }
  readonly end: { readonly x: number; readonly y: number }
}

/**
 * Is `cell` inside `range`?
 *
 * Transcribed from `Linkifier._linkAtPosition` in `@xterm/xterm`, **deliberately rather than
 * approximated**, and that is the property worth stating: the predicate that decides which link a
 * press opens is now the same one that decided which link got underlined on hover. Anything
 * simpler — "same line, x between start and end" — disagrees with the underline on a link that
 * wrapped across the right-hand edge, which for a long absolute path in a narrow pane is the
 * common case rather than the exotic one.
 *
 * The wrap arms read oddly on their own and are correct: on the *first* row of a wrapped link
 * everything from `start.x` rightwards is inside it, on the *last* row everything up to `end.x`
 * is, and on a middle row the whole row is.
 */
export function linkAtCell(range: LinkRange, cell: Cell): boolean {
  if (cell.y < range.start.y || cell.y > range.end.y) return false
  const sameLine = range.start.y === range.end.y
  const wrappedFromLeft = range.start.y < cell.y
  const wrappedToRight = range.end.y > cell.y
  return (
    (sameLine && range.start.x <= cell.x && range.end.x >= cell.x) ||
    (wrappedFromLeft && range.end.x >= cell.x) ||
    (wrappedToRight && range.start.x <= cell.x) ||
    (wrappedFromLeft && wrappedToRight)
  )
}

/*
 * Runtime values only, no imports: `check-paths.mjs` compiles this file alone with a bare `tsc`
 * and loads the emitted `.js` in node. The same note is at the foot of `pathMatch.ts`,
 * `chrome/notices.ts` and `chrome/branchModel.ts`, and it is the whole reason the rules above are
 * not where they were written.
 */
