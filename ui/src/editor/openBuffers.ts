/**
 * Every editor currently mounted, by tab, so something outside the pane tree can save it.
 *
 * # Why a registry and not state
 *
 * The text lives in a CodeMirror `EditorState` inside `EditorSurface`, which is where it has
 * to live: lifting it into React would re-render the editor on every keystroke, and lifting
 * it into the workspace store would put a five-megabyte rope through an IPC broadcast. So
 * the buffer is unreachable from anywhere above the pane — which is exactly the problem when
 * a close confirmation wants to offer *Save and close*.
 *
 * This is the same shape as `layout/paneHosts.ts`, and for the same reason: a live DOM/editor
 * instance outlives its position in the React tree, so the thing that owns it is a module and
 * not a component.
 *
 * # Only offer Save when it can be honoured
 *
 * `saveAll` reports which tabs it could not save. A dialog that offers a Save button and then
 * silently loses a file it could not write is worse than one that only offers Discard — see
 * `CloseConfirm`, which omits the button entirely when the caller passes no handler.
 */

/** Writes the buffer to disk. Resolves once the write has succeeded. */
type Saver = () => Promise<void>

const savers = new Map<string, Saver>()

/** Called by `EditorPane` on mount. Replacing an entry is normal: a tab can remount. */
export function registerBuffer(tab: string, save: Saver): void {
  savers.set(tab, save)
}

export function unregisterBuffer(tab: string): void {
  savers.delete(tab)
}

/** Whether every one of these tabs has a live editor that could save it. */
export function canSaveAll(tabs: readonly string[]): boolean {
  return tabs.length > 0 && tabs.every((t) => savers.has(t))
}

/**
 * Save the given tabs, and report the ones that failed.
 *
 * Sequential rather than concurrent. These are writes to the user's files: a failure part-way
 * through a `Promise.all` leaves an indeterminate set written with no way to say which, and
 * the whole point of the return value is to be able to say which.
 */
export async function saveAll(tabs: readonly string[]): Promise<{ failed: string[] }> {
  const failed: string[] = []
  for (const tab of tabs) {
    const save = savers.get(tab)
    if (!save) {
      failed.push(tab)
      continue
    }
    try {
      await save()
    } catch {
      failed.push(tab)
    }
  }
  return { failed }
}

/** For tests and diagnostics. */
export function registeredBuffers(): string[] {
  return [...savers.keys()]
}
