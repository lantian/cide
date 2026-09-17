/**
 * Ask for folders and open them as projects — the one road, for every surface that offers it.
 *
 * There are two callers and there will be more: the header's `+` (`ProjectMenu`) and the
 * palette's *Open project…* (`keys/dispatch.ts`). They are one function rather than two because
 * of what the body has to get right and how quietly it fails if it does not:
 *
 * * **`projectMenu.browse()`, never `@tauri-apps/plugin-dialog`'s `open()`.** The plugin's
 *   command guards `set_parent` behind `cfg(windows | macos)`, so on Linux the folder dialog
 *   is parented on nothing and opens *behind* the cide window — the app looks frozen and the
 *   picker is somewhere in the task switcher. `project_pick` builds the dialog against the
 *   window's real GTK toplevel. A second surface that spelled its own picker would look
 *   identical everywhere it was developed and be broken for every Linux user.
 * * **Cancelling is `[]`, and it is not a failure.** Reporting it raises a red toast for the
 *   ordinary act of changing your mind.
 *
 * Not a hook and not in a component: `dispatch.ts` runs outside React, and the workspace mirror
 * is a module-level store, so the mutation is reached with `getState()` exactly as every other
 * command in that file reaches it.
 */
import { projectMenu } from '@/ipc/client'
import { useWorkspace } from '@/store/workspace'

/** Open the picker and open whatever it answers with. Resolves having done nothing on cancel. */
export async function browseForProject(): Promise<void> {
  const chosen = await projectMenu.browse()
  if (chosen.length === 0) return
  await useWorkspace.getState().openProject(chosen)
}
