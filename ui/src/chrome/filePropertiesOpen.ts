/**
 * The one way to raise a properties card. (M70)
 *
 * Its own module for `chrome/pushRun.ts`'s reason, which is structural rather than stylistic:
 * `keys/dispatch.ts` already imports from `chrome/menuModel.ts` and `chrome/BranchSelector.tsx`,
 * so a menu that reached back into `dispatch.ts` for the opener would close an ESM cycle. Every
 * caller — the palette's `case`, the file tree's row, the tab strip's row — imports this
 * instead, and nothing imports `dispatch.ts`.
 *
 * It is one line today. `openPushDialog` was one line too, until three surfaces needed it and
 * the question became which of them asked first; `PushDialog`'s own note records what that
 * split cost. Having the seam before there are three callers is cheaper than finding it after.
 */
import { requestProperties } from './filePropertiesStore'

/**
 * Show the properties of `path` in `project`.
 *
 * `path` must be **absolute**: every caller has one (the tree row, the tab's `TabKind::File`,
 * the focused editor) and resolving a relative one here would need a root this module has no
 * reason to know.
 */
export function openFileProperties(project: string, path: string): void {
  requestProperties({ project, path })
}
