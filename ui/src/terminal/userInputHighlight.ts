import type { IDecoration, IMarker } from '@xterm/xterm'
import type { TerminalHandle } from './xterm'
import { concreteColor } from './xterm'
import { inputBlocks, type SubmittedInput } from './userInputModel'
import { useTerminalFind } from './findStore'
import styles from './userInputHighlight.module.css'

interface RowPaint { marker: IMarker; decorations: IDecoration[]; signature: string }
interface RailPaint { first: IMarker; last: IMarker; start: number; end: number; element: HTMLElement }

/** Keep decorations attached to their buffer markers. Scrolling only positions the rails
 * and DOM row grounds; unrelated writes keep the same decorations. Search owns the cell fill
 * while its bar is open. Foreground colours remain the CLI's ANSI colours; only the row
 * background and rail identify user input. PTY/clipboard text and selection stay intact. */
export function highlightUserInputs(handle: TerminalHandle, paneId: string,
  harness: 'claude' | 'codex', initial: readonly SubmittedInput[]) {
  const term = handle.term
  let inputs = initial
  let disposed = false
  let frame: number | null = null
  let rows: RowPaint[] = []
  let blocks: RailPaint[] = []
  let rails: HTMLElement | null = null
  let grounds = new Set<HTMLElement>()
  let searching = false

  const removeRow = (row: RowPaint): void => { row.decorations.forEach((d) => d.dispose()); row.marker.dispose() }
  const clear = (): void => {
    rows.forEach(removeRow)
    rows = []
    blocks = []
    rails?.remove()
    rails = null
    grounds.forEach((row) => row.style.removeProperty('background-color'))
    grounds.clear()
  }
  const position = (): void => {
    if (disposed) return
    const buffer = term.buffer.active
    const screen = term.element?.querySelector<HTMLElement>('.xterm-screen')
    if (!screen) return
    const height = screen.clientHeight / term.rows
    for (const block of blocks) {
      if (!block.first.isDisposed) block.start = block.first.line
      if (!block.last.isDisposed) block.end = block.last.line
      block.element.style.top = `${(block.start - buffer.viewportY) * height}px`
      block.element.style.height = `${(block.end - block.start + 1) * height}px`
    }
    // DOM xterm omits trailing blank cells before consulting cell decorations. Its row
    // elements are reused on scroll, so assign grounds after rendering and clear only rows
    // that no longer show a user message. WebGL paints blank cells with decorations itself.
    const domRows = screen.querySelector('.xterm-rows')?.children
    const next = new Set<HTMLElement>()
    if (!searching && domRows) {
      for (const row of rows) {
        if (row.marker.isDisposed) continue
        const element = domRows[row.marker.line - buffer.viewportY]
        if (element instanceof HTMLElement) next.add(element)
      }
    }
    for (const row of grounds) if (!next.has(row)) row.style.removeProperty('background-color')
    for (const row of next) if (!grounds.has(row)) row.style.backgroundColor = 'var(--user-input-bg)'
    grounds = next
  }
  const paint = (): void => {
    if (frame !== null) { cancelAnimationFrame(frame); frame = null }
    if (disposed) return
    const buffer = term.buffer.active
    const lines = Array.from({ length: buffer.length }, (_, index) => {
      const line = buffer.getLine(index)
      return { text: line?.translateToString(false) ?? '', wrapped: line?.isWrapped ?? false }
    })
    const cursorRow = buffer.baseY + buffer.cursorY
    const style = getComputedStyle(document.documentElement)
    const fill = concreteColor(style.getPropertyValue('--user-input-bg').trim())
    searching = useTerminalFind.getState().open[paneId] !== undefined
    const screen = term.element?.querySelector<HTMLElement>('.xterm-screen')
    // Both xterm cell renderers support decorations on the alt buffer, but its decoration
    // DOM hides there. Keep our non-interactive rail layer attached across ordinary paints.
    if (screen && rails?.parentElement !== screen) {
      rails?.remove()
      rails = document.createElement('div')
      rails.className = styles.rails ?? ''
      rails.dataset.userInputRails = 'true'
      rails.setAttribute('aria-hidden', 'true')
      screen.appendChild(rails)
    }
    const previous = new Map(rows.filter((row) => !row.marker.isDisposed).map((row) => [row.marker.line, row]))
    // ED can dispose xterm markers even when the next TUI grid has the same prompt. Keep
    // rails by their last row position so recreating those markers does not remove the rail.
    const previousBlocks = new Map(blocks.map((block) => [block.first.isDisposed ? block.start : block.first.line, block]))
    const nextRows: RowPaint[] = []
    const nextBlocks: RailPaint[] = []
    for (const block of inputBlocks(lines, inputs, harness, cursorRow)) {
      const blockRows: RowPaint[] = []
      for (let line = block.start; line <= block.end; line++) {
        // Enabled highlighting owns the prompt's background, including a CLI's own fill.
        // Disposing its decorations restores the untouched native SGR background. Search
        // temporarily owns the fill while open; foregrounds always remain the CLI's.
        const runs = searching ? [] : [{ x: 0, width: term.cols }]
        const signature = JSON.stringify([term.cols, searching, fill])
        let row = previous.get(line)
        if (row && row.signature !== signature) { removeRow(row); row = undefined }
        if (!row) {
          const marker = term.registerMarker(line - cursorRow)
          if (!marker) continue
          const decorations: IDecoration[] = []
          for (const run of runs) {
            const decoration = term.registerDecoration({ marker, ...run, layer: 'bottom', backgroundColor: fill })
            if (decoration) decorations.push(decoration)
          }
          row = { marker, decorations, signature }
        }
        previous.delete(line)
        nextRows.push(row)
        blockRows.push(row)
      }
      const first = blockRows[0]?.marker, last = blockRows.at(-1)?.marker
      if (!rails || !first || !last) continue
      let rail = previousBlocks.get(block.start)
      if (rail) {
        previousBlocks.delete(block.start)
        rail.first = first; rail.last = last; rail.start = block.start; rail.end = block.end
      }
      else {
        const element = document.createElement('div')
        element.className = styles.rail ?? ''
        rails.appendChild(element)
        rail = { first, last, start: block.start, end: block.end, element }
      }
      nextBlocks.push(rail)
    }
    previous.forEach(removeRow)
    previousBlocks.forEach((block) => block.element.remove())
    rows = nextRows
    blocks = nextBlocks
    position()
  }
  const schedule = (): void => {
    if (!disposed && frame === null) frame = requestAnimationFrame(paint)
  }
  // Parsed writes reconcile before xterm's next paint, including a TUI wheel redraw. A
  // native scroll never clears or re-registers cell decorations. Rendering only positions
  // our DOM overlays, so decoration-triggered renders cannot create a feedback loop.
  const subs = [term.onWriteParsed(paint), term.onResize(paint), term.onScroll(position),
    term.onRender(position), term.buffer.onBufferChange(() => { clear(); paint() })]
  const theme = new MutationObserver(schedule)
  theme.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme', 'data-accent', 'style'] })
  const stopFind = useTerminalFind.subscribe((state, previous) => {
    if (state.open[paneId] !== previous.open[paneId]) schedule()
  })
  schedule()
  return {
    update(next: readonly SubmittedInput[]): void { inputs = next; schedule() },
    dispose(): void {
      disposed = true
      if (frame !== null) cancelAnimationFrame(frame)
      subs.forEach((sub) => sub.dispose())
      theme.disconnect()
      stopFind()
      clear()
    },
  }
}
