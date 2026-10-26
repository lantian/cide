import { clipboardComparison, file, type ProjectId } from '@/ipc/client'
import { notify } from '@/chrome/notices'
import { imageKindFor } from '@/panes/imageKinds'
import { drawingKindFor } from '@/panes/excalidrawKinds'
import { pasteUnavailable, readClipboard } from './clipboard'
import { flushDocument } from './sharedDocuments'

export function canCompareClipboard(path: string): boolean {
  return imageKindFor(path) === null && drawingKindFor(path) === null
}

/** Captures the clicked path. Selection, active tab and clipboard are never inferred later. */
export async function compareToClipboard(project: ProjectId, path: string): Promise<void> {
  try {
    if (!canCompareClipboard(path)) {
      notify('Compare to Clipboard is available for text files.', { kind: 'warn' })
      return
    }
    const text = await readClipboard()
    if (text === null) {
      notify(pasteUnavailable() ?? 'The clipboard does not contain text.', { kind: 'warn' })
      return
    }
    await flushDocument(path)
    await file.read(path) // Reject binary, missing and oversized files before creating a tab.
    await clipboardComparison.open(project, path, text)
  } catch (error) {
    notify(`Could not compare to clipboard: ${String(error)}`, { kind: 'error' })
  }
}
