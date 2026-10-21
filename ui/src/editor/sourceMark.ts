import { Compartment } from '@codemirror/state'
import { Decoration, EditorView } from '@codemirror/view'

export interface SourceMark { line: number; column: number; endColumn: number }
/** Search highlights are decorations, not the user's selection. Polling must not move a caret. */
export function markSource(view: EditorView, slot: Compartment, at: SourceMark | null): void {
  if (!at) { view.dispatch({ effects: slot.reconfigure([]) }); return }
  const line = view.state.doc.line(Math.max(1, Math.min(at.line, view.state.doc.lines)))
  const from = Math.min(line.to, line.from + Math.max(0, at.column - 1))
  const to = Math.min(line.to, Math.max(from, line.from + at.endColumn - 1))
  const ranges = [Decoration.line({ class: 'cide-search-line' }).range(line.from)]
  if (to > from) ranges.push(Decoration.mark({ class: 'cide-search-match' }).range(from, to))
  view.dispatch({ effects: [slot.reconfigure(EditorView.decorations.of(Decoration.set(ranges, true))), EditorView.scrollIntoView(from, { y: 'center' })] })
}
