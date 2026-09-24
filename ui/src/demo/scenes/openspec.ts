import type { SpecArtifactText, SpecBoard, SpecChange, TaskBoard } from '../../ipc/generated'
import { PROPOSAL, SPEC_BOARD, SPEC_CHANGE } from '../data/openspec'
import { click, showPanel } from '../drive'
import type { Scene } from '../scenes'
import { leaf, pane, tab, wire } from '../world'

/**
 * OpenSpec: the sidebar board — four changes and cide's capabilities — beside the change the
 * coalescer work is tracked under, opened as its own page with its requirement deltas, task list
 * and artifacts. The page is a tab kind drawn over
 * the pane tree instead of inside it.
 */
export const openspec: Scene = {
  setup: (world, handlers) => {
    // An `openSpec` tab draws a page over its tree, but the tree still holds one process-less
    // editor pane — `cmd/spec.rs` opens it exactly so.
    const host = pane('editor', 'openspec')
    world.open(tab({ kind: 'openSpec', subject: { kind: 'change', change: SPEC_CHANGE.name } }, leaf(host.id), [host]))
    handlers.set('spec_board', (): SpecBoard => SPEC_BOARD)
    handlers.set('spec_change', (): SpecChange => SPEC_CHANGE)
    handlers.set('spec_artifact', (): SpecArtifactText => PROPOSAL)
    // A read tracker, even an empty one: while it is unread every *Start work* is greyed out,
    // which on a screenshot reads as a broken panel.
    handlers.set('tasks_board', (): TaskBoard => ({ kind: 'ready', tasks: [], rev: wire(1) }))
  },
  drive: async () => {
    await showPanel('openspec')
    // Fold the rendered proposal so the requirement deltas — the part only OpenSpec has — come
    // up into the picture; the folded heading still says the prose is there.
    await click('[data-block="proposal"] > summary')
  },
}
