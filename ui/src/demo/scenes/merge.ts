import type { ChangesTree, ConflictFile, MergeState } from '../../ipc/generated'
import type { Scene } from '../scenes'
import { click } from '../drive'
import { leaf, pane, tab } from '../world'
import { CONFLICT, CONFLICT_PATH, MERGE, MERGE_STATUS, REPO } from '../data/merge'

/**
 * The three-pane conflict resolver on `session.rs`, mid-merge of master into the branch: the
 * branch's side, the result and master's side, with four conflicting blocks marked and the
 * non-conflicting ones ready to apply. `git_status` and `git_conflicts` answer mid-merge too,
 * so the rail's Git badge and the panel, if opened, agree with the resolver.
 */
export const merge: Scene = {
  setup: (world, handlers) => {
    const editor = pane('editor', 'session.rs — merge')
    world.open(tab({ kind: 'merge', repo: REPO.id, path: CONFLICT_PATH, dirty: false }, leaf(editor.id), [editor]))
    handlers.set('git_repos', () => [REPO])
    handlers.set('git_status', (): ChangesTree => MERGE_STATUS)
    handlers.set('git_conflicts', (): MergeState => MERGE)
    handlers.set('git_conflict_read', (): ConflictFile => CONFLICT)
    handlers.set('git_shelf_list', () => [])
    handlers.set('git_stash_list', () => [])
    // The resolver marks its tab dirty as it builds the result; nothing here keeps the flag.
    handlers.set('tab_set_dirty', () => ({ rev: Number(world.boot.workspace.rev) }))
  },
  drive: async () => {
    // The sidebar hidden, by clicking the lit rail icon as a person would: the resolver draws
    // three editors side by side and every column the sidebar takes is a wrapped line in all
    // three. (A narrower Git panel was tried; its merge bar clips below ~400px.)
    await click('[data-audit="railIcon"][aria-selected="true"]')
  },
}
