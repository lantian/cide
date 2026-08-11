/**
 * The theme, for the panel that draws icons.
 *
 * A one-line re-export with a reason: `FileIcon` needs `'dark' | 'light'`, and the honest
 * source of that is the workspace mirror — `useSettings.applyTheme` sets it locally *before*
 * the IPC round trip, so it is already correct on the frame the user clicks. Reading
 * `document.documentElement.dataset.theme` instead would be a second source that lags a
 * render behind, and reading it inside a row would be a DOM read on every scroll tick.
 *
 * Subscribe once, at the panel, and pass the value to each `FileIcon`. See `FileIconProps`.
 */
import { useWorkspace } from '@/store/workspace'
import type { IconTheme } from './iconFor'

export function useIconTheme(): IconTheme {
  return useWorkspace((s) => s.theme)
}
