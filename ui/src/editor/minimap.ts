/**
 * The 96px canvas minimap.
 *
 * Hand-written because there is no alternative. `@replit/codemirror-minimap` — the one
 * published CM6 minimap — was last released 2023-12-12 against `@codemirror/view ^6.21.3`;
 * the installed view is 6.43.8, and a package that has not seen a release in that window
 * is not something to hang a milestone on.
 *
 * # What it draws
 *
 * One 3px row per line, per the mock: a 2px bar inside a 3px pitch, indented by the line's
 * own indentation and as wide as its trimmed content, coloured by the line's leading
 * syntax token. The caret's line is `--tk-caret`. The visible range is a `--tk-sel` rectangle,
 * and dragging it scrolls the buffer.
 *
 * # Two decisions worth naming
 *
 * **The map slides rather than compressing.** A 3px pitch fits about 250 lines in a normal
 * pane; a file longer than that cannot be shown whole without shrinking the bars, which the
 * mock fixes at 3px. So past that point the map shows a window of the document that tracks
 * the scroll position, the way VS Code's does. The alternative — a variable pitch — makes a
 * 10,000-line file a grey smear and makes the bar height a function of file length, which
 * is a worse answer to "3px bars".
 *
 * **Colour is resolved, not referenced.** Canvas cannot read `var(--purple)`, so the
 * palette is read out of the DOM with `getComputedStyle` and cached. A theme switch has to
 * invalidate that cache without rebuilding the editor — the app toggles themes live, with
 * terminals running — so the plugin watches `data-theme` on the document element and
 * repaints. That is the whole of the theme story: no remount, no state rebuild.
 *
 * # Where the arithmetic lives
 *
 * `minimapGeometry.ts`, and it is checked by `scripts/check-editor.mjs`. What is left here
 * is the part that needs a canvas, a scroller and a parse tree; the row pitch, the bar
 * extent, the viewport rectangle and the click-to-line mapping are functions of numbers and
 * are tested as such.
 */
import { syntaxTree } from '@codemirror/language'
import { highlightTree } from '@lezer/highlight'
import { EditorView, ViewPlugin, type PluginValue, type ViewUpdate } from '@codemirror/view'
import { PLAIN_TOKEN, TOKEN_VAR_BY_CLASS, cideHighlightStyle } from './highlight'
import {
  cancelResizeSettle,
  isResizeGesturing,
  whenResizeSettles,
} from '@/layout/resizeGesture'
import {
  LINE_SCAN_LIMIT,
  MINIMAP_WIDTH,
  barRect,
  firstMapLine,
  lastMapLine,
  lineAtOffset,
  lineMetrics,
  mapRows,
  viewportRect,
} from './minimapGeometry'

interface Palette {
  /** Resolved colour per highlight class, plus `''` for text with no class. */
  byClass: Map<string, string>
  plain: string
  caret: string
  viewportFill: string
  viewportStroke: string
}

/**
 * What a token resolves to when it resolves to nothing.
 *
 * One neutral grey for every role, and deliberately *not* each token's real value written
 * out again: a per-role fallback table is a second palette, and the day someone adjusts
 * `tokens.css` it becomes a palette that is wrong in a way only the minimap shows. This
 * should never be reached — `EditorView` attaches its DOM to the parent before it builds
 * its plugins, so the element is in the document and inheriting `:root` by the time the
 * constructor reads it — and if it ever is, a grey map is a legible one.
 */
const UNRESOLVED = '#888'

/**
 * Read the design tokens off a live element.
 *
 * `getComputedStyle` rather than a hardcoded table: the tokens are defined in
 * `tokens.css` under `[data-theme]`, and duplicating their values here would give the app
 * two palettes that drift the first time one is adjusted.
 */
function readPalette(el: HTMLElement): Palette {
  const style = getComputedStyle(el)
  const read = (name: string): string => {
    const value = style.getPropertyValue(name).trim()
    return value.length > 0 ? value : UNRESOLVED
  }

  const byClass = new Map<string, string>()
  for (const [cls, token] of TOKEN_VAR_BY_CLASS) byClass.set(cls, read(token))

  return {
    byClass,
    plain: read(PLAIN_TOKEN),
    // `--tk-caret`, not `--accent`: an imported colour scheme owns the caret, and a map whose
    // caret line did not move with it would be pointing at a different row's colour.
    caret: read('--tk-caret'),
    // The rectangle is a wash over the bars rather than a block on top of them, so the
    // shape of the file stays legible inside the part being looked at.
    viewportFill: read('--tk-sel'),
    viewportStroke: read('--border'),
  }
}

/** A row of the map: where the bar starts, how long it is, and what colour. */
interface Row {
  indent: number
  length: number
  colour: string
}

class Minimap implements PluginValue {
  private readonly canvas: HTMLCanvasElement
  private readonly ctx: CanvasRenderingContext2D | null
  private readonly themeWatch: MutationObserver
  private palette: Palette
  private frame = 0
  /** Set while a pointer is down on the canvas, so moves scroll instead of selecting. */
  private dragging = false
  private pointer: number | null = null

  constructor(private readonly view: EditorView) {
    this.canvas = document.createElement('canvas')
    this.canvas.className = 'cm-cide-minimap'
    this.ctx = this.canvas.getContext('2d')
    this.palette = readPalette(view.dom)

    this.canvas.addEventListener('pointerdown', this.onPointerDown)
    this.canvas.addEventListener('pointermove', this.onPointerMove)
    this.canvas.addEventListener('pointerup', this.onPointerUp)
    this.canvas.addEventListener('pointercancel', this.onPointerUp)
    // Scrolling is not reliably a `ViewUpdate`. CodeMirror renders a margin above and below
    // the visible range, so a scroll inside that margin changes neither the viewport nor the
    // geometry and `update()` below is never called — while both things this canvas draws
    // from the scroll position, the `--tk-sel` rectangle and (past ~250 lines) which slice of
    // the document is shown at all, have just moved. Without this the map jumps a screenful
    // at a time instead of tracking.
    view.scrollDOM.addEventListener('scroll', this.onScroll, { passive: true })
    view.dom.appendChild(this.canvas)

    // The tokens live on `<html data-theme>`, so that is what is watched. Watching the
    // editor's own element would see nothing: nothing about it changes when the theme does.
    this.themeWatch = new MutationObserver(() => {
      this.palette = readPalette(this.view.dom)
      this.schedule()
    })
    this.themeWatch.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ['data-theme'],
    })

    this.schedule()
  }

  update(update: ViewUpdate): void {
    // `focusChanged` is in the list because the caret row is drawn in `--accent` and the
    // selection is only meaningful while the editor has focus.
    if (
      update.docChanged ||
      update.viewportChanged ||
      update.geometryChanged ||
      update.selectionSet ||
      update.focusChanged
    ) {
      this.schedule()
    }
  }

  destroy(): void {
    if (this.frame !== 0) cancelAnimationFrame(this.frame)
    cancelResizeSettle(this)
    this.themeWatch.disconnect()
    this.view.scrollDOM.removeEventListener('scroll', this.onScroll)
    this.canvas.removeEventListener('pointerdown', this.onPointerDown)
    this.canvas.removeEventListener('pointermove', this.onPointerMove)
    this.canvas.removeEventListener('pointerup', this.onPointerUp)
    this.canvas.removeEventListener('pointercancel', this.onPointerUp)
    this.canvas.remove()
  }

  /**
   * Coalesce repaints onto one frame.
   *
   * A burst of transactions — a paste, a multi-cursor edit, a scroll that fires several
   * updates — would otherwise redraw the canvas once each. Reading DOM geometry inside the
   * frame is also what keeps this out of CodeMirror's measure cycle.
   */
  private readonly onScroll = (): void => {
    this.schedule()
  }

  private schedule(): void {
    if (this.frame !== 0) return
    /*
     * A resize gesture is the one case where a frame is not the right unit.
     *
     * `geometryChanged` fires on every frame of a splitter drag, and `draw` reads
     * `scrollHeight`, `clientHeight` and `devicePixelRatio` and then repaints the whole canvas
     * — per editor, and `layout/TabContent.module.css` keeps every tab's editors mounted and
     * laid out at full size. None of those intermediate pictures is looked at: the map is a
     * picture of a box whose height is still moving. So the work waits for the gesture to end,
     * keyed on this instance so several updates during the drag still draw once.
     */
    if (isResizeGesturing()) {
      whenResizeSettles(this, () => this.draw())
      return
    }
    this.frame = requestAnimationFrame(() => {
      this.frame = 0
      this.draw()
    })
  }

  /** The first document line the map shows, given how far the buffer is scrolled. */
  private firstMapLine(rows: number): number {
    const scroller = this.view.scrollDOM
    return firstMapLine(
      this.view.state.doc.lines,
      rows,
      scroller.scrollTop,
      scroller.scrollHeight - scroller.clientHeight,
    )
  }

  private draw(): void {
    const ctx = this.ctx
    if (ctx === null) return

    const height = this.view.dom.clientHeight
    const dpr = window.devicePixelRatio || 1
    if (height <= 0) return

    // Sized in device pixels and scaled back, so a 2px bar is 2 CSS pixels on a HiDPI
    // screen rather than a blurry 1.
    const pixelWidth = Math.round(MINIMAP_WIDTH * dpr)
    const pixelHeight = Math.round(height * dpr)
    if (this.canvas.width !== pixelWidth || this.canvas.height !== pixelHeight) {
      this.canvas.width = pixelWidth
      this.canvas.height = pixelHeight
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
    ctx.clearRect(0, 0, MINIMAP_WIDTH, height)

    const state = this.view.state
    const rows = mapRows(height)
    const first = this.firstMapLine(rows)
    const last = lastMapLine(state.doc.lines, rows, first)
    const caretLine = state.doc.lineAt(state.selection.main.head).number

    for (let n = first; n <= last; n++) {
      const row = this.rowFor(n)
      if (row.length === 0) continue
      const bar = barRect(row.indent, row.length, n - first)
      ctx.fillStyle = n === caretLine ? this.palette.caret : row.colour
      ctx.fillRect(bar.x, bar.y, bar.width, bar.height)
    }

    // The viewport rectangle, painted over the bars.
    const rect = viewportRect(
      first,
      last,
      state.doc.lineAt(this.view.viewport.from).number,
      state.doc.lineAt(this.view.viewport.to).number,
    )
    if (rect !== null) {
      ctx.globalAlpha = 0.35
      ctx.fillStyle = this.palette.viewportFill
      ctx.fillRect(0, rect.top, MINIMAP_WIDTH, rect.height)
      ctx.globalAlpha = 1
      ctx.strokeStyle = this.palette.viewportStroke
      ctx.lineWidth = 1
      // Half-pixel offsets, or a 1px stroke straddles the boundary and paints 2px of grey.
      ctx.strokeRect(0.5, rect.top + 0.5, MINIMAP_WIDTH - 1, rect.height - 1)
    }
  }

  /**
   * Indentation, content length and colour for one document line.
   *
   * The colour is the first *coloured* token on the line, which is what makes a block of
   * comments read as a block and an attribute stand out above a function. Taking the
   * majority token instead was tried and is worse: it turns almost every line the colour of
   * whatever punctuation and identifiers dominate it, which is to say the same colour.
   */
  private rowFor(number: number): Row {
    const line = this.view.state.doc.line(number)
    const metrics = lineMetrics(line.text, this.view.state.tabSize)
    if (metrics.length === 0) return { indent: 0, length: 0, colour: this.palette.plain }

    // `syntaxTree` returns whatever has been parsed so far and never blocks. For a large
    // file the parser has not reached most of the document, so most rows come back with no
    // classes and are drawn in the plain colour — the degradation the milestone asks for,
    // and the reason `ensureSyntaxTree`, which *would* block, is not used here.
    let colour = this.palette.plain
    let decided = false
    const tree = syntaxTree(this.view.state)
    const scanTo = Math.min(line.to, line.from + LINE_SCAN_LIMIT)
    if (tree.length >= line.from) {
      highlightTree(
        tree,
        cideHighlightStyle,
        (from, to, classes) => {
          if (decided) return
          // Whitespace carries a class from its enclosing node often enough to matter; a
          // run that is only spaces would otherwise decide the row's colour.
          if (this.view.state.doc.sliceString(from, to).trim().length === 0) return
          for (const cls of classes.split(' ')) {
            const found = this.palette.byClass.get(cls)
            if (found !== undefined) {
              colour = found
              decided = true
              return
            }
          }
        },
        line.from,
        scanTo,
      )
    }

    return { indent: metrics.indent, length: metrics.length, colour }
  }

  // --- dragging ------------------------------------------------------------------------

  private readonly onPointerDown = (event: PointerEvent): void => {
    this.dragging = true
    this.pointer = event.pointerId
    this.canvas.setPointerCapture(event.pointerId)
    this.scrollTo(event)
    event.preventDefault()
  }

  private readonly onPointerMove = (event: PointerEvent): void => {
    if (!this.dragging) return
    this.scrollTo(event)
    event.preventDefault()
  }

  private readonly onPointerUp = (event: PointerEvent): void => {
    if (this.pointer !== null && this.canvas.hasPointerCapture(this.pointer)) {
      this.canvas.releasePointerCapture(this.pointer)
    }
    this.dragging = false
    this.pointer = null
    event.preventDefault()
  }

  /**
   * Centre the buffer on the line under the pointer.
   *
   * Centring rather than putting the line at the top: a click near the bottom of the map
   * should show that part of the file, and scrolling it to the top would put it out of
   * view whenever the document ends before a screenful later.
   */
  private scrollTo(event: PointerEvent): void {
    const rect = this.canvas.getBoundingClientRect()
    if (rect.height <= 0) return
    const rows = mapRows(rect.height)
    const first = this.firstMapLine(rows)
    const target = lineAtOffset(event.clientY - rect.top, first, this.view.state.doc.lines)
    const pos = this.view.state.doc.line(target).from
    this.view.dispatch({ effects: EditorView.scrollIntoView(pos, { y: 'center' }) })
  }
}

/**
 * The minimap extension.
 *
 * The gap the canvas sits in is reserved by `EditorSurface.module.css`, which pads
 * `.cm-scroller` by `MINIMAP_WIDTH`, rather than by a theme spec here — the same file owns
 * the `--border-soft` rule the mock puts to the map's left, and splitting one visual
 * element between a stylesheet and a JS theme is how the two end up disagreeing by a pixel.
 */
export function minimap() {
  return ViewPlugin.define((view) => new Minimap(view))
}
