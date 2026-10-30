/**
 * Local History's gestures, for every surface that offers them.
 *
 * Its own module for `chrome/filePropertiesOpen.ts`'s reason: `keys/dispatch.ts` is imported by
 * the menus, so a menu reaching back into it would close an ESM cycle. Everything calls here.
 */
import { create } from 'zustand'
import { notify, notifyFailure } from '@/chrome/notices'
import { saveTab } from '@/editor/openBuffers'
import { clipboardComparison, localHistory, type ProjectId } from '@/ipc/client'
import { copyText } from '@/sidebar/copyText'
import { useWorkspace } from '@/store/workspace'

/** Open a history tab. `path === null` is the whole project. */
export function openLocalHistory(project: ProjectId, path: string | null, folder: boolean): void {
  void localHistory.open(project, path, folder).catch(notifyFailure)
}

function basename(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1)
}

/**
 * Save this window's unsaved edits to a path, so they become a version instead of being lost.
 *
 * Rust cannot do this: the text is in the webview. What it does get is a guarantee in the other
 * direction, because a restore saves whatever is on disk first. A dirty buffer in another window
 * is not reached by this, and is the one case left to the conflict bar.
 */
async function saveOpenBuffers(project: ProjectId, path: string): Promise<void> {
  const tabs = useWorkspace.getState().boot?.workspace.projects[project]?.tabs ?? []
  for (const tab of tabs) {
    if (tab.kind.kind === 'file' && tab.kind.path === path && tab.kind.dirty) {
      await saveTab(tab.id)
    }
  }
}

/** Make a file what a version says. Reversible: what it replaced is kept as a version. */
export async function restoreVersion(
  project: ProjectId,
  path: string,
  version: number,
): Promise<boolean> {
  try {
    await saveOpenBuffers(project, path)
    const outcome = await localHistory.restore(project, path, version)
    notify(
      outcome === 'recreated'
        ? `Restored ${basename(path)}`
        : `Restored this version of ${basename(path)}`,
      { kind: 'ok', hint: 'What it replaced is kept in Local History.' },
    )
    return true
  } catch (error) {
    notifyFailure(error)
    return false
  }
}

/** Open a version against the live file, whose per-block arrows are the partial restore. */
export async function compareVersion(
  project: ProjectId,
  path: string,
  version: number,
  label: string,
): Promise<void> {
  try {
    const text = await localHistory.text(project, version)
    await clipboardComparison.open(project, path, text, label)
  } catch (error) {
    notifyFailure(error)
  }
}

export function copyPatch(
  project: ProjectId,
  path: string,
  from: number,
  to: number | null,
): void {
  void localHistory
    .patch(
      project,
      path,
      { kind: 'version', id: from },
      to === null ? { kind: 'current' } : { kind: 'version', id: to },
    )
    .then(copyText)
    .then((copied) => {
      notify(copied ? 'Patch copied' : 'Could not copy the patch', {
        kind: copied ? 'ok' : 'warn',
      })
    })
    .catch(notifyFailure)
}

interface PutLabelStore {
  /** The project being labelled, or `null` while no dialog is up. */
  project: ProjectId | null
  open: (project: ProjectId) => void
  close: () => void
}

/** A store for `filePropertiesStore.ts`' reason: the palette's gesture is outside React. */
export const usePutLabel = create<PutLabelStore>((set) => ({
  project: null,
  open: (project) => set({ project }),
  close: () => set({ project: null }),
}))

export function requestPutLabel(project: ProjectId): void {
  usePutLabel.getState().open(project)
}
