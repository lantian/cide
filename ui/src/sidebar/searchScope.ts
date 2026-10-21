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
import { openSearch, useSearchSessions, EMPTY_DRAFT } from '@/search/store'
import { useWorkspace } from '@/store/workspace'
import { activeProjectIdOf, activeProjectOf } from '@/keys/target'
import { isSyntheticPath } from './groupRows'
import { rootOf } from './rowPaths'
import { scopeDirOf, scopeLabel } from './SearchModel'
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

/** Set the active project's folder filter before opening its modal. Drafts are per project,
 * so switching projects cannot carry a previous project's folder into this search. */
export function applyScope(scope: string): void {
  const project = activeProjectIdOf(useWorkspace.getState().boot)
  const state = useSearchSessions.getState()
  const draft = state.drafts[project ?? ''] ?? EMPTY_DRAFT
  state.patchDraft(project, { descriptor: { ...draft.descriptor, query: { ...draft.descriptor.query, scope } } })
}

/** Open Text search with the requested folder and focus its query input. */
export function searchInFolder(scope: string): boolean {
  if (activeProjectIdOf(useWorkspace.getState().boot) === null) return false
  applyScope(scope)
  openSearch('text')
  requestFocus('search')
  return true
}
