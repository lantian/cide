/**
 * "Search in this folder" — the one place that gesture is spelled.
 *
 * Two callers, and they must not be two implementations: `keys/dispatch.ts`'s `sidebar.search`
 * arm, which seeds the box when Ctrl+Shift+F is pressed with the file tree focused, and the
 * tree's own *Search in Folder* context-menu item. A chord and a menu row that name the same
 * act have to do the same thing — the argument `keys/dispatch.ts`'s header makes about there
 * being one dispatcher for both the palette and the keyboard, in miniature.
 *
 * Not in `SearchModel.ts`, which is import-free so a check script can compile it standalone;
 * everything below reaches into three stores. The *rules* live there — `scopeLabel` and
 * `scopeDirOf` — and this is the wiring that reads the tree and writes the panel.
 */
import { requestFocus } from '@/chrome/focusRequests'
import { requestPanel } from '@/chrome/panelRequests'
import { useWorkspace } from '@/store/workspace'
import { activeProjectIdOf, activeProjectOf } from '@/keys/target'
import { isSyntheticPath } from './groupRows'
import { rootOf } from './rowPaths'
import { scopeDirOf, scopeLabel } from './SearchModel'
import { useSearch } from './SearchStore'
import { useFileTree } from './treeStore'

/** The active project's roots, in the shape [`scopeLabel`] wants. */
function roots(): { path: string; label: string }[] {
  const project = activeProjectOf(useWorkspace.getState().boot)
  return project === null ? [] : project.roots.map((r) => ({ path: r.path, label: r.label }))
}

/**
 * The string the scope box should hold for an absolute path, or `null` when there is none.
 *
 * `null` for the two populations that cannot be searched, and they are refused here rather than
 * sent to the backend to be refused with a sentence: a heading is not a place, and a path under
 * no project root is outside every walk the search ever starts.
 */
export function scopeFor(path: string, isDir: boolean): string | null {
  // A `cide://group/…` sentinel — the *External Libraries*, *Scratches* and *Project Notes*
  // headings. Tested by shape, the same way `mutationRefusal` tests it, so a synthetic scheme
  // a later group invents is covered the day it exists.
  if (isSyntheticPath(path)) return null
  const dir = scopeDirOf(path, isDir)
  const owned = roots()
  if (rootOf(dir, owned.map((r) => r.path)) === null) return null
  return scopeLabel(dir, owned)
}

/**
 * The folder the file tree's cursor stands for, or `null`.
 *
 * The **cursor** (`selected`), not the selection set: a scope is one place, and a search
 * narrowed to five folders is not a thing this backend can express. That is also what *Reveal
 * in File Manager* and *Show File History* take, and for the reason they both give — a verb
 * about a single place should not silently pick one of five.
 *
 * The row is consulted for its `kind` so a *file* becomes its parent directory immediately,
 * while the caret is still in the tree and the box is about to be drawn. The tree windows its
 * rows, so a cursor scrolled out of the chunk cache answers `undefined` — in which case the
 * path is handed over as it is, which is correct rather than merely tolerable, because
 * `cide_search::content::resolve_scope` applies the same file-to-parent rule at the other end.
 * The only cost of the miss is that the box reads `src/log.rs` for one round trip.
 */
export function scopeFromTree(): string | null {
  const tree = useFileTree.getState()
  const path = tree.selected
  if (path === null) return null
  const row = tree.rowAt(tree.selectedIndex)
  // Only when the row really is the cursor's: `selectedIndex` is advisory, and a stale one
  // would answer with a *different* row's kind, which is how a file would be scoped to itself.
  const isDir = row?.path === path ? row.kind === 'dir' : false
  return scopeFor(path, isDir)
}

/**
 * Put `scope` in the panel's folder box, for the project the tree is showing.
 *
 * # Why this attaches first
 *
 * `SearchStore.attach` clears both narrowing boxes when the project it is handed is not the one
 * it was pointed at — a path means nothing in another project, and a scope silently carried
 * across is a search of a fraction of the new tree that looks exactly like a search of all of
 * it. That is right, and it is also a trap for this function: the panel is usually **not
 * mounted** when the gesture is made, so its `attach` runs one React commit *later* and would
 * throw away the scope that had just been written for it. The gesture would appear to do
 * nothing, and only for a user who had switched project since they last opened the panel —
 * which is the kind of bug that gets reported as "sometimes it works".
 *
 * Attaching here closes it: by the time the panel mounts and attaches again the store is
 * already pointed at this project, so its call is a no-op as far as the boxes are concerned.
 * `attach` is deliberately not early-returning on the same project, so the second call costs a
 * cleared result list and nothing else — and the poll it schedules is cancelled by the
 * `setQuery` below before its timer can fire.
 */
export function applyScope(scope: string): void {
  const project = activeProjectIdOf(useWorkspace.getState().boot)
  if (project !== null) useSearch.getState().attach(project)
  useSearch.getState().setQuery({ scope })
}

/**
 * Point the search panel at one folder and put the caret in its query box.
 *
 * The caret goes to the **pattern**, not to the folder: the folder has just been filled in for
 * you, and the pattern is what you came to type.
 *
 * `false` when this window has no sidebar to reveal — the caller says so rather than assuming
 * the gesture landed. See `chrome/panelRequests.ts`.
 */
export function searchInFolder(scope: string): boolean {
  // Written before the reveal, so the panel's first render already has it. The order is
  // invisible today because `showPanel` goes through a `useState` setter, and it is written
  // this way for the same reason `requestAmend` parks before revealing.
  applyScope(scope)
  if (!requestPanel('search')) return false
  requestFocus('search')
  return true
}
