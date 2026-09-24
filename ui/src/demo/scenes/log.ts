import type { CommitDetail, CommitLineCounts, CommitPage, RepoInfo } from '../../ipc/generated'
import type { Args } from '../fakeTauri'
import type { Scene } from '../scenes'
import { click, until } from '../drive'
import { uid } from '../world'
import { REPO } from '../data/git'
import { detailFor, LOG_PAGE, SELECTED } from '../data/log'

/**
 * The Git tool window docked under the Claude grid, on its Log tab: the branch graph with its
 * merge lanes and ref chips, and the commit that split the coalescer out selected, its message
 * and changed files on the right. A History tab for `coalesce.rs` sits beside it in the strip.
 */
export const log: Scene = {
  setup: (world, handlers) => {
    // Opened through the workspace, as `tool_window_open_history` would leave it: `active: null`
    // *is* the Log tab, and the History tab is a second chip in the strip.
    world.project.toolWindow = {
      ...world.project.toolWindow,
      open: true,
      height: 500,
      // A flat file list and a wider commit list: both keep the commit's message on screen
      // below its files instead of scrolled out of a short tool window.
      filesAsTree: false,
      logSplit: 580,
      history: [{ id: uid('history'), repo: REPO.id, path: 'crates/cide-pty/src/coalesce.rs', title: 'coalesce.rs', split: null }],
      active: null,
    }
    handlers.set('git_repos', (): RepoInfo[] => [REPO])
    handlers.set('git_log', (): CommitPage => LOG_PAGE)
    handlers.set('git_log_cancel', () => false)
    handlers.set('git_commit_detail', (a: Args): CommitDetail => detailFor(String(a['rev'])))
    handlers.set('git_commit_line_counts', (a: Args): CommitLineCounts => {
      const d = detailFor(String(a['rev']))
      return { files: d.files, total: d.total, deadlineHit: false }
    })
  },
  drive: async () => {
    await until(() => document.querySelector('[data-audit="logRow"]') !== null)
    await click(`[data-audit="logRow"][data-row-id="${REPO.id}:${SELECTED}"]`)
  },
}
