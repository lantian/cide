import { EditorState, StateEffect, type Extension } from '@codemirror/state'
import { EditorView, keymap, lineNumbers } from '@codemirror/view'
import { defaultKeymap, isolateHistory } from '@codemirror/commands'
import { MergeView, unifiedMergeView } from '@codemirror/merge'
import { syntaxHighlighting } from '@codemirror/language'
import { cideHighlightStyle } from './highlight'
import { loadLanguage } from './languages'
import { polarityExtension, polaritySlot, watchPolarity } from './cmPolarity'
import { exceedsBytes } from './byteSize'
import styles from './clipboardDiffView.module.css'

export interface ClipboardComparison { id: string; text: string; layout: 'split' | 'unified' }

export function tooLargeForClipboardDiff(a: string, b: string): boolean {
  if (a.length + b.length > 1024 * 1024) return true
  if (exceedsBytes(a + b, 1024 * 1024)) return true
  let lines = 2
  for (const text of [a, b]) for (let i = 0; i < text.length; i++) if (text.charCodeAt(i) === 10 && ++lines > 12_000) return true
  return false
}

function copyButton(): HTMLButtonElement {
  const button = document.createElement('button')
  button.type = 'button'
  button.className = styles.copy ?? ''
  button.textContent = '→'
  button.title = 'Copy this block from Clipboard to the file'
  button.setAttribute('aria-label', 'Copy from Clipboard')
  return button
}

/** The right editor is the ordinary file surface, including its shared document connection. */
export function createClipboardDiff(
  parent: HTMLElement, path: string, doc: string, extensions: Extension[],
  comparison: ClipboardComparison, readOnly: boolean,
): { view: EditorView; clipboard: EditorView | null; destroy: () => void } {
  if (readOnly) extensions = [...extensions, EditorState.readOnly.of(true), EditorView.editable.of(false)]
  const isolated = EditorState.transactionExtender.of(tr => tr.isUserEvent('revert')
    ? { annotations: isolateHistory.of('full') } : null)
  const clipboardText = EditorState.create({ doc: comparison.text }).doc.toString()
  parent.classList.add(styles.host ?? '')
  if (tooLargeForClipboardDiff(clipboardText, doc)) {
    const note = document.createElement('div')
    note.className = styles.notice ?? ''
    note.setAttribute('role', 'status')
    note.textContent = 'This comparison exceeds 12,000 lines or 1 MiB. The file editor remains available.'
    parent.append(note)
    const view = new EditorView({ parent, doc, extensions })
    return { view, clipboard: null, destroy: () => { view.destroy(); note.remove() } }
  }
  if (comparison.layout === 'unified') {
    const view = new EditorView({ parent, doc, extensions: [...extensions, isolated,
      unifiedMergeView({ original: clipboardText, highlightChanges: true, gutter: true,
        mergeControls: readOnly ? false : (type, action) => {
          // Accept rewrites the baseline; a clipboard snapshot must remain fixed.
          if (type === 'accept') { const hidden = document.createElement('span'); hidden.hidden = true; return hidden }
          const button = copyButton()
          button.textContent = 'Copy from Clipboard'
          button.addEventListener('click', action)
          return button
        },
      }),
    ] })
    return { view, clipboard: null, destroy: () => view.destroy() }
  }
  const polarity = polaritySlot()
  const merge = new MergeView({ parent,
    a: { doc: clipboardText, extensions: [lineNumbers(), polarityExtension(polarity),
      syntaxHighlighting(cideHighlightStyle), EditorState.readOnly.of(true), EditorView.editable.of(false),
      keymap.of(defaultKeymap),
      EditorView.contentAttributes.of({ 'aria-label': 'Clipboard (read only)', tabindex: '0' })] },
    b: { doc, extensions: [...extensions, isolated] },
    ...(readOnly ? {} : { revertControls: 'a-to-b' as const }),
    renderRevertControl: () => {
      const button = copyButton()
      // MergeView handles pointer mousedown; bridge native button keyboard activation too.
      button.addEventListener('click', event => {
        if (event.detail === 0) button.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))
      })
      return button
    },
    highlightChanges: true, gutter: true,
  })
  merge.a.dom.classList.add(styles.clipboard ?? '')
  const stopPolarity = watchPolarity(merge.a, polarity)
  let alive = true
  void loadLanguage(path).then(language => {
    if (alive && language) merge.a.dispatch({ effects: StateEffect.appendConfig.of(language) })
  })
  return { view: merge.b, clipboard: merge.a, destroy: () => {
    alive = false; stopPolarity(); merge.destroy()
  } }
}
