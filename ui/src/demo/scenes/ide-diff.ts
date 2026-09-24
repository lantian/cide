import type { Settings } from '../../ipc/generated'
import type { Scene } from '../scenes'
import { leaf, pane, tab } from '../world'
import { click } from '../drive'
import { COALESCE_PATH, ORIGINAL, PROPOSED, REQUEST_ID } from '../data/ide-diff'

/**
 * Claude Code's IDE integration: an agent's `openDiff` on `coalesce.rs`, shown in cide's own
 * editor with the Accept / Reject bar, while the turn that proposed it waits.
 *
 * A tab of its own with one diff pane, because that is what `ide.rs::open_diff_tab` builds — not
 * a split beside the Claude pane. A split cannot be drawn honestly: `PaneBody` renders every pane
 * of a `claudeMcp` diff tab as the diff, a Claude pane included, so the "beside" picture would be
 * two diffs. The pinned Claude tab stays in the strip next to it, which is the relationship.
 *
 * Split rather than the stored default (unified), and with the sidebar shut so each side has the
 * width of a line. Unified is the layout with the per-hunk Keep change / Keep original buttons,
 * but those draw white-on-white in the light theme (see the report), and split is the picture
 * that reads as before-and-after at a glance anyway.
 */
export const ideDiff: Scene = {
  setup: (world, handlers) => {
    world.boot.workspace.settings.editor.diffView = 'split'
    const diff = pane('diff', 'coalesce.rs')
    world.open(
      tab(
        {
          kind: 'diff',
          spec: {
            title: 'coalesce.rs',
            oldPath: COALESCE_PATH,
            newPath: COALESCE_PATH,
            origin: { kind: 'claudeMcp', requestId: REQUEST_ID },
          },
          preview: false,
        },
        leaf(diff.id),
        [diff],
      ),
    )
    // The diff pane reads its mode from `settings_get`; unanswered, the toggle draws disabled.
    handlers.set('settings_get', (): Settings => world.boot.workspace.settings)
    handlers.set('claude_diff_content', (): { original: string; proposed: string } => ({ original: ORIGINAL, proposed: PROPOSED }))
  },
  // Clicking the open panel's rail icon shuts the sidebar, as it does for a person.
  drive: async () => {
    await click('[data-audit="railIcon"][aria-selected="true"]')
  },
}
