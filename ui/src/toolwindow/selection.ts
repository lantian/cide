import { create } from 'zustand'
import { toolWindow, type ProjectId } from '@/ipc/client'

/** Only transient tabs live here. Rust still owns the selected Log/History tab. All
 * entrances share this override, so a command can reveal Log while a search is selected. */
export const useToolSelection = create<{ byProject: Record<string, string | null>; logs: Record<string, { source: string; label: string }[]> }>(() => ({ byProject: {}, logs: {} }))
export function selectTransientToolTab(project: string | null, id: string | null): void {
  useToolSelection.setState(s => ({ byProject: { ...s.byProject, [project ?? '']: id } }))
}
export function clearToolSelection(project: string | null): void {
  selectTransientToolTab(project, null)
}
export const searchTabId = (id: string): string => `search:${id}`
export const isSearchTab = (id: string | null): boolean => id?.startsWith('search:') === true

export const PROBLEMS_TAB = 'problems'
export const serverLogTabId = (source: string): string => `lsp-log:${source}`
async function revealTransientTab(project: ProjectId, id: string): Promise<void> {
  const previous = useToolSelection.getState().byProject[project] ?? null
  // The workspace event can open the panel before this command resolves. Select first so
  // its first mount is the requested tab, without starting a Git history walk on the way.
  selectTransientToolTab(project, id)
  try {
    await toolWindow.setLayout(project, { open: true })
  } catch (error) {
    if (useToolSelection.getState().byProject[project] === id) selectTransientToolTab(project, previous)
    throw error
  }
}
export function showProblems(project: ProjectId): Promise<void> {
  return revealTransientTab(project, PROBLEMS_TAB)
}
export async function openServerLogs(project: ProjectId, source: string, label: string): Promise<void> {
  useToolSelection.setState(s => {
    const rows = s.logs[project] ?? []
    return { logs: { ...s.logs, [project]: rows.some(row => row.source === source) ? rows : [...rows, { source, label }] } }
  })
  await revealTransientTab(project, serverLogTabId(source))
}
export function closeServerLogs(project: string, source: string): void {
  useToolSelection.setState(s => ({
    logs: { ...s.logs, [project]: (s.logs[project] ?? []).filter(row => row.source !== source) },
    byProject: s.byProject[project] === serverLogTabId(source) ? { ...s.byProject, [project]: PROBLEMS_TAB } : s.byProject,
  }))
}
