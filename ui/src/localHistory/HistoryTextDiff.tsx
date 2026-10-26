import { useEffect, useRef, useState } from 'react'
import { EditorState } from '@codemirror/state'
import { Decoration, EditorView, lineNumbers } from '@codemirror/view'
import { presentableDiff, unifiedMergeView } from '@codemirror/merge'
import { tooLargeToDiff } from '@/panes/DiffPane'
import { polarityExtension, polaritySlot, watchPolarity } from '@/editor/cmPolarity'
import type { HistoryComparison } from '@/ipc/generated'
import { Banner } from '@/kit/components/Feedback'
import diffStyles from '@/panes/DiffPane.module.css'
import styles from './LocalHistoryPanel.module.css'
/** The existing unified merge surface and theme, with edits and merge controls disabled. */
export function HistoryTextDiff({ comparison }: { comparison: HistoryComparison }) {
  const host = useRef<HTMLDivElement>(null)
  const [error, setError] = useState<string | null>(null)
  const large = !comparison.right.unavailable && tooLargeToDiff(comparison.left.text ?? '', comparison.right.text ?? '')
  useEffect(() => {
    if (host.current === null || comparison.left.unavailable) return
    setError(null)
    const polarity = polaritySlot()
    try {
      if (large) {
        const a = EditorState.create({ doc: comparison.left.text ?? '' }).doc.toString()
        const b = EditorState.create({ doc: comparison.right.text ?? '' }).doc.toString()
        const changes = presentableDiff(a, b, { scanLimit: 500, timeout: 100 })
        const make = (text: string, side: 'a' | 'b') => new EditorView({ parent: host.current!, doc: text, extensions: [
          polarityExtension(polarity), lineNumbers(), EditorState.readOnly.of(true), EditorView.editable.of(false), EditorState.transactionFilter.of((tr) => tr.docChanged ? [] : tr),
          EditorView.editorAttributes.of({ class: `cm-merge-${side}` }),
          EditorView.decorations.of(Decoration.set(changes.flatMap((c) => {
            const from = side === 'a' ? c.fromA : c.fromB; const to = side === 'a' ? c.toA : c.toB
            return from < to ? [Decoration.mark({ class: 'cm-changedText' }).range(from, to)] : []
          }), true)),
        ] })
        const left = make(a, 'a'); const right = make(b, 'b')
        const stopLeft = watchPolarity(left, polarity); const stopRight = watchPolarity(right, polarity)
        return () => { stopLeft(); stopRight(); left.destroy(); right.destroy() }
      }
      const view = new EditorView({ parent: host.current, doc: comparison.right.unavailable ? comparison.left.text ?? '' : comparison.right.text ?? '', extensions: [
        polarityExtension(polarity), lineNumbers(), EditorView.lineWrapping,
        EditorState.readOnly.of(true), EditorView.editable.of(false), EditorState.transactionFilter.of((tr) => tr.docChanged ? [] : tr),
        ...(comparison.right.unavailable ? [] : [unifiedMergeView({ original: comparison.left.text ?? '', mergeControls: false, highlightChanges: true, gutter: true, collapseUnchanged: { margin: 3, minSize: 4 }, diffConfig: { scanLimit: 500, timeout: 100 } })]),
      ] })
      const stopPolarity = watchPolarity(view, polarity)
      return () => { stopPolarity(); view.destroy() }
    } catch (why) { setError(String(why)) }
  }, [comparison, large])
  return <>{large && <Banner>Large comparison: changed ranges are highlighted in two read-only documents.</Banner>}{error && <Banner tone="bad">The comparison could not be drawn: {error}</Banner>}<div className={`${diffStyles.body} ${large ? styles.largeDiff : ''}`} ref={host} aria-label="Read-only revision diff" /></>
}
