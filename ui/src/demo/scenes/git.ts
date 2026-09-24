import type { FileDiff, Settings, ShelfEntry } from '../../ipc/generated'
import type { Args } from '../fakeTauri'
import type { Scene } from '../scenes'
import { click, showPanel, sleep, until } from '../drive'
import { leaf, pane, tab } from '../world'
import { REPO } from '../data/git'
import { COMMIT_MESSAGE, DIFF_PATH, otherDiff, sessionDiff, SHELF } from '../data/git-scene'

/**
 * The commit tool window beside the diff it is about: two changelists, unversioned files, the
 * branch chip, and `session.rs` open in a git diff tab with its per-line tick boxes — the
 * IDEA-shaped half of cide, next to which the Claude grid sits one tab away.
 */
export const git: Scene = {
  setup: (world, handlers) => {
    const diff = pane('diff', 'session.rs')
    world.open(
      tab(
        {
          kind: 'diff',
          spec: {
            title: 'session.rs',
            oldPath: DIFF_PATH,
            newPath: DIFF_PATH,
            origin: { kind: 'git', repo: REPO.id, path: DIFF_PATH, side: 'combined' },
          },
          // Not a preview: the user double-clicked it open, which is the tab a screenshot of
          // someone working would show.
          preview: false,
        },
        leaf(diff.id),
        [diff],
      ),
    )
    handlers.set('git_repos', () => [REPO])
    handlers.set('git_diff_file', (a: Args): FileDiff => {
      const side = a['side'] as FileDiff['side']
      return a['path'] === DIFF_PATH ? sessionDiff(side) : otherDiff(String(a['path']), side)
    })
    handlers.set('git_shelf_list', (): ShelfEntry[] => SHELF)
    handlers.set('git_stash_list', () => [])
    handlers.set('git_conflicts', () => null)
    // The diff pane reads its layout and whitespace settings; the workspace's own are the answer.
    handlers.set('settings_get', (): Settings => world.boot.workspace.settings)
  },
  drive: async () => {
    await showPanel('git')
    await until(() => document.querySelector('[data-audit="gitRow"]') !== null)
    // Tick the active changelist, as "commit what Claude just did" is one click.
    await click('[data-audit="gitRow"] [data-audit="gitCheck"]')
    const box = document.querySelector<HTMLTextAreaElement>('[data-audit="gitMessage"]')
    if (box) {
      // React tracks the value through the native setter; assigning `.value` alone would be
      // overwritten on the next render.
      const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')?.set
      set?.call(box, COMMIT_MESSAGE)
      box.dispatchEvent(new Event('input', { bubbles: true }))
    }
    // Tick one hunk and a few single lines in another: line-level staging is the part of
    // this window a screenshot has to show, and "n of 38 lines selected" is it at work.
    await until(() => document.querySelectorAll('[data-audit="gitDiffHunkBox"]').length > 2)
    document.querySelectorAll<HTMLElement>('[data-audit="gitDiffHunkBox"]')[1]?.click()
    await sleep(100)
    for (const row of document.querySelectorAll<HTMLElement>('[data-audit="gitDiffRow"]')) {
      const text = row.textContent ?? ''
      if (/coalesce::\{Coalescer|coalescer: Mutex|The frame being built|use std::time/.test(text)) {
        row.querySelector<HTMLElement>('[data-audit="gitDiffLineBox"]')?.click()
        await sleep(50)
      }
    }
    await sleep(300)
  },
}
