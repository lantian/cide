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
 * The hook, the story selector and the wire types come with it, for anyone wiring the
 * stories into a layout audit later. `model.ts` deliberately does *not*: it is imported by
 * file path from `ui/scripts/check-git-tree.mjs`, which compiles that one module with tsc
 * and imports the output, so routing it through a barrel would only drag React in.
 */
export { GitPanel, type GitPanelProps } from './GitPanel'
export { useGitPanel, type GitPanelActions, type GitPanelModel } from './useGitPanel'
export { storyFromQuery, type GitStory, type GitStoryName } from './fixture'
export type { ChangeFile, ChangeGroup, ChangesTree, FileStatus, RepoChanges } from './types'
