import { create } from 'zustand'
import { DEFAULT_PROBLEMS_VIEW, type ProblemsView } from './problemsModel'

export const useProblemsState = create<{
  views: Record<string, ProblemsView>
  focused: boolean
}>(() => ({ views: {}, focused: false }))
export function setProblemsView(project: string | null, patch: Partial<ProblemsView>): void {
  const key = project ?? ''
  useProblemsState.setState(s => ({ views: { ...s.views, [key]: { ...(s.views[key] ?? DEFAULT_PROBLEMS_VIEW), ...patch } } }))
}
