/**
 * The git commit tool window.
 *
 * One import for the sidebar that hosts it:
 *
 * ```tsx
 * import { GitPanel } from '@/sidebar/GitPanel'
 * ...
 * {view === 'git' && <GitPanel project={activeProjectId} />}
 * ```
 *
 * ## Wiring the diff
 *
 * Double-clicking a file fetches its `FileDiff` and hands it to `onOpenDiff`. The panel does
 * not own a pane and cannot open one, so the prop is optional and the un-wired case is
 * *reported*, not swallowed. A host that has somewhere to put a diff passes:
 *
 * ```tsx
 * <GitPanel project={activeProjectId} onOpenDiff={(diff, repo) => …} />
 * ```
 *
 * The hook, the story selector and the wire types come with it, for anyone wiring the
 * stories into a layout audit later. `model.ts` deliberately does *not*: it is imported by
 * file path from `ui/scripts/check-git-tree.mjs`, which compiles that one module with tsc
 * and imports the output, so routing it through a barrel would only drag React in.
 */
/*
 * `GitPanel` comes from `GitPanelHost`, not from `GitPanel.tsx`.
 *
 * That file holds `GitPanelView`, which is everything the panel draws and nothing that needs a
 * window — no context menu, no theme lookup — so that `check-git-render.mjs` can render it
 * under node with `react-dom/server` and no DOM. The host adds the two window-shaped pieces.
 * Callers import `GitPanel` and are unaffected; see the header of either file for why the
 * split exists.
 */
export { GitPanel, type GitPanelProps } from './GitPanelHost'
export { GitPanelView, type GitPanelViewProps, type TreeMenu } from './GitPanel'
export {
  useGitPanel,
  type GitPanelActions,
  type GitPanelModel,
  type GitPanelOptions,
} from './useGitPanel'
export { storyFromQuery, type GitStory, type GitStoryName } from './fixture'
export type {
  ChangeEntry,
  ChangesTree,
  GroupKind,
  GroupView,
  RepoChanges,
  RepoView,
  ShelfRow,
  StatusView,
} from './types'
