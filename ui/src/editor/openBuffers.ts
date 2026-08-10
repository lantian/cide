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

/**
 * The part of the workspace mirror that answers "which tab is the user in".
 *
 * Structural rather than an import of `Bootstrap` from `@/ipc/client`, and that is the whole
 * reason this function is here rather than in `keys/dispatch.ts` where it is used: this
 * module is compiled standalone by `scripts/check-editor.mjs`, and a module that pulls in the
 * IPC client pulls in `@tauri-apps/api` and cannot be exercised outside a window. The shape
 * is still checked against the real DTO — `dispatch.ts` passes a `Bootstrap | null` straight
 * in, so a renamed field or a new `WindowRole` variant is a type error at that call site
 * rather than a mirror that quietly drifts.
 */
export interface WorkspaceFocus {
  readonly role:
    | { readonly kind: 'shell'; readonly active: string | null }
    | { readonly kind: 'detachedPane' | 'detachedTab'; readonly tab: string }
  readonly workspace: {
    readonly projects: Readonly<Record<string, { readonly activeTab: string } | undefined>>
  }
}

/**
 * The tab a *Save file* means, given the workspace mirror.
 *
 * Pure, and given the mirror rather than reading it, so it can be checked against fixtures —
 * `dispatch.ts` holds the one line that reads the live store.
 *
 * A detached window is answered from its **role**, not from its project's `activeTab`. Both
 * kinds of window share one `Workspace`: the mirror is the whole domain and not this window's
 * slice of it, so a `pane:` window that asked the project which tab was active would be told
 * whichever one the *shell* window last focused, and Ctrl+S in a torn-out editor would write
 * a different file. That branch saves nothing today — `DetachedPaneWindow` renders a terminal
 * for every pane kind, so no detached window holds an editor — and it is written this way
 * because the alternative is wrong the moment one can.
 */
export function focusedTabOf(boot: WorkspaceFocus | null): string | null {
  if (boot === null) return null
  const role = boot.role
  if (role.kind !== 'shell') return role.tab
  if (role.active === null) return null
  return boot.workspace.projects[role.active]?.activeTab ?? null
}

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
 * Save one tab's buffer, or answer `null` when nothing here can.
 *
 * `null` rather than a rejected promise or a silent no-op. The caller is the `file.save`
 * command — reached from Ctrl+S and from the palette — and "there is no editor for that tab"
 * is not a failure to report to the user; it is a reason for the command to say it did not
 * run. A rejected promise would make an ordinary Ctrl+S in a terminal look like a failed
 * write, and a resolved one would make it look like a successful one. See `keys/dispatch.ts`.
 *
 * `tab` is nullable so a caller that has no focused tab at all can ask without a branch.
 */
export function saveTab(tab: string | null): Promise<void> | null {
  if (tab === null) return null
  const save = savers.get(tab)
  if (save === undefined) return null
  try {
    return save()
  } catch (error) {
    // A saver that throws synchronously would otherwise escape into the key gate's keydown
    // handler, where an exception stops the rest of that listener — including the
    // `preventDefault` that keeps `^S` away from a PTY. Turned into a rejection so the one
    // caller that cares handles it the same way it handles a failed write.
    return Promise.reject(error instanceof Error ? error : new Error(String(error)))
  }
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
