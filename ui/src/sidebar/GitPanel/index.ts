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
 * The model and the fixtures are exported too, for the check script under `ui/scripts` and
 * for anyone wiring the stories into a layout audit later.
 */
export { GitPanel, type GitPanelProps } from './GitPanel'
export { useGitPanel, type GitPanelActions, type GitPanelModel } from './useGitPanel'
export { storyFromQuery, type GitStory, type GitStoryName } from './fixture'
export type { ChangeFile, ChangeGroup, ChangesTree, FileStatus, RepoChanges } from './types'
