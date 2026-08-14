/**
 * Watch where a buffer is looking, and say so at most once a frame. (M12)
 *
 * The producer half of per-file view memory. It answers one question — *which line is at the
 * top of this editor, and where is the caret* — and hands the answer to a callback. Everything
 * it *decides* (whether the answer is news, how it is clamped, whether it may be applied) is in
 * `position.ts`, which is import-free and is run by `ui/scripts/check-editor.mjs`; what is left
 * here is the part that genuinely needs a laid-out DOM.
 *
 * # Why a scroll listener and not `update()` alone
 *
 * **`viewportChanged` is not enough, and this is measured rather than assumed.** `minimap.ts`
 * carries the same note beside the same listener: CodeMirror renders a margin above and below
 * the visible range, so a scroll *inside* that margin changes neither the viewport nor the
 * geometry, and `update()` is never called — while the first visible line, which is the whole
 * of what this module reports, has just moved. A `ViewUpdate`-only tracker would follow long
 * scrolls and silently ignore short ones, which is the worst of both: it would look like it
 * worked.
 *
 * # Why one `requestAnimationFrame`
 *
 * A scroll fires continuously — a flung wheel gesture is hundreds of events — and reading
 * geometry is a layout read. Coalescing into one frame is the first rung of the ladder written
 * out in `crates/cide-app/src/positions_state.rs`: this stage costs a `getBoundingClientRect`
 * per frame and crosses nothing; the caller's debounce is the stage that costs an IPC call, and
 * the Rust store's is the one that costs a disk write.
 */
import { EditorView, ViewPlugin, type ViewUpdate } from '@codemirror/view'
import type { Extension } from '@codemirror/state'
import { topVisibleLine, worthNoting, type FileView } from './position'

/**
 * The first visible line of `view`, 1-based.
 *
 * By hit-testing the top-left of the scroller rather than by arithmetic on `scrollTop`.
 * `scrollTop` looks simpler and is a coordinate space away from being right: CodeMirror's
 * height map is measured from the top of the *document*, `.cm-content` carries its own padding,
 * and the find bar is a panel that may or may not sit inside the scrolled box depending on the
 * version. `posAtCoords` asks the layout the question in the layout's own terms.
 *
 * `precise: false` — the overload that always answers, clamping to the nearest position rather
 * than returning `null` for a point in the gutter or past the last line. There is no useful
 * "don't know" here: this runs on every frame of a scroll and a null would simply be a frame
 * that reported nothing.
 *
 * The x is taken from `contentDOM`, not from the scroller: the scroller's left edge is inside
 * the line-number gutter, and a point in the gutter is not a document position.
 *
 * The line under that pixel may be only *partly* visible, so the answer is corrected by
 * `topVisibleLine` — which is where the reasoning and the two measurements live, because it is
 * arithmetic and this function is a pair of `getBoundingClientRect` calls.
 */
function firstVisibleLine(view: EditorView): number {
  const scroller = view.scrollDOM.getBoundingClientRect()
  const content = view.contentDOM.getBoundingClientRect()
  const pos = view.posAtCoords({ x: content.left + 1, y: scroller.top + 1 }, false)
  const line = view.state.doc.lineAt(pos)
  const block = view.lineBlockAt(line.from)
  return topVisibleLine(
    line.number,
    // `documentTop` already includes the document's own padding (`contentDOM` rect + paddingTop,
    // in `@codemirror/view`), and `block.top` is measured from there — so the sum is this line's
    // top edge in the same screen space as `scroller.top`.
    view.documentTop + block.top,
    // A *wrapped* line is taller than one row, and half of it is the right threshold for it too:
    // the question is whether enough of this line is on screen to be the one you are reading.
    block.height,
    scroller.top,
    view.state.doc.lines,
  )
}

/** Where this buffer is looking, right now. */
function observe(view: EditorView, path: string): FileView {
  const head = view.state.selection.main.head
  const line = view.state.doc.lineAt(head)
  return {
    path,
    // 1-based line and a 1-based UTF-16 column, because `head - line.from` is already a UTF-16
    // offset — that is what a CodeMirror document position is. See `position.ts`.
    line: line.number,
    column: head - line.from + 1,
    topLine: firstVisibleLine(view),
  }
}

/**
 * Report this buffer's view whenever it changes, coalesced to one frame.
 *
 * `path` is fixed for the life of the view: `EditorSurface` rebuilds the whole `EditorView` when
 * the file changes, so a path that could change under a live plugin would be a plugin outliving
 * the editor it belongs to.
 */
export function viewTracker(path: string, publish: (at: FileView) => void): Extension {
  return ViewPlugin.define((view) => new ViewTracker(view, path, publish))
}

class ViewTracker {
  private frame = 0
  /** The last value handed to `publish`, so an unchanged view costs nothing downstream. */
  private last: FileView | null = null

  private readonly onScroll = (): void => {
    this.schedule()
  }

  constructor(
    private readonly view: EditorView,
    private readonly path: string,
    private readonly publish: (at: FileView) => void,
  ) {
    view.scrollDOM.addEventListener('scroll', this.onScroll, { passive: true })
    /*
     * Nothing is published at construction, deliberately.
     *
     * A freshly built editor is at line 1 for one frame, and `EditorSurface` restores the
     * remembered position in a dispatch immediately afterwards. Publishing here would report
     * that line 1 to the caller, whose 500 ms debounce would then be racing the restore — and
     * on a slow mount the note would win and *overwrite the stored position with the top of the
     * file*, which is the exact data loss this whole feature exists to prevent. The restore's
     * own dispatch produces an `update`, so the first real report happens a frame later anyway.
     */
  }

  update(update: ViewUpdate): void {
    // `geometryChanged` is in the list because a pane resize moves which line is at the top
    // without moving the caret or the scroll offset by a pixel.
    if (
      update.selectionSet ||
      update.docChanged ||
      update.viewportChanged ||
      update.geometryChanged
    ) {
      this.schedule()
    }
  }

  destroy(): void {
    this.view.scrollDOM.removeEventListener('scroll', this.onScroll)
    if (this.frame !== 0) cancelAnimationFrame(this.frame)
    this.frame = 0
  }

  private schedule(): void {
    if (this.frame !== 0) return
    this.frame = requestAnimationFrame(() => {
      this.frame = 0
      // The view can be destroyed between the frame being requested and it running — a tab
      // closed mid-scroll. `destroy` cancels, so this is belt to that brace; reading geometry
      // off a detached DOM answers zeroes, which would report line 1 for a buffer nobody is
      // looking at any more.
      if (this.view.dom.isConnected === false) return
      const at = observe(this.view, this.path)
      if (!worthNoting(this.last, at)) return
      this.last = at
      this.publish(at)
    })
  }
}
