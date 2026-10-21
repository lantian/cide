/** Runtime mirrors and view state. Rust owns sessions and pins; this store never persists them. */
import { create } from 'zustand'
import { searchSessions as api, type ProjectId, type SearchDescriptor, type SearchSession, type SearchSessionFrame } from '@/ipc/client'
import { EMPTY_QUERY, spliceHits } from '@/sidebar/SearchModel'
import { useOverlays } from '@/overlays/store'
import { activeProjectIdOf } from '@/keys/target'
import { useWorkspace } from '@/store/workspace'
import { notifyFailure } from '@/chrome/notices'
import { needsAnotherPage, type SearchMode } from './model'

export interface Draft { mode: SearchMode; descriptor: SearchDescriptor }
export interface ResultView { selected: string | null; collapsed: Set<string>; scroll: number; ratio: number; stackedRatio: number }
interface SearchState {
  drafts: Record<string, Draft>
  focus: number
  tabs: Record<string, SearchSession[]>
  frames: Record<string, SearchSessionFrame>
  errors: Record<string, string>
  views: Record<string, ResultView>
  patchDraft: (project: string | null, patch: Partial<Draft>) => void
  patchView: (id: string, patch: Partial<ResultView>) => void
}
export const EMPTY_DRAFT: Draft = { mode: 'all', descriptor: { source: 'all', query: { ...EMPTY_QUERY }, libraries: false } }
export const EMPTY_VIEW: ResultView = { selected: null, collapsed: new Set(), scroll: 0, ratio: 40, stackedRatio: 50 }
export const useSearchSessions = create<SearchState>((set) => ({
  drafts: {}, focus: 0, tabs: {}, frames: {}, errors: {}, views: {},
  patchDraft: (project, patch) => set(s => ({ drafts: { ...s.drafts, [project ?? '']: { ...(s.drafts[project ?? ''] ?? EMPTY_DRAFT), ...patch } } })),
  patchView: (id, patch) => set(s => ({ views: { ...s.views, [id]: { ...(s.views[id] ?? EMPTY_VIEW), ...patch } } })),
}))

export function openSearch(mode: SearchMode, descriptor?: SearchDescriptor): void {
  const project = activeProjectIdOf(useWorkspace.getState().boot)
  const state = useSearchSessions.getState()
  const draft = state.drafts[project ?? ''] ?? EMPTY_DRAFT
  state.patchDraft(project, { mode, descriptor: descriptor ?? { ...draft.descriptor, source: mode === 'actions' ? 'all' : mode } })
  useSearchSessions.setState(s => ({ focus: s.focus + 1 }))
  useOverlays.getState().show('search')
}

const polling = new Map<string, { cancelled: boolean; timer?: ReturnType<typeof setTimeout> }>()
const pinning = new Map<string, Promise<SearchSession>>()
const revisions = new Map<string, number>()
const lists = new Map<string, number>()
const changed = (project: string) => revisions.set(project, (revisions.get(project) ?? 0) + 1)
export function stopPolling(id: string): void {
  const poll = polling.get(id)
  if (poll) { poll.cancelled = true; clearTimeout(poll.timer); polling.delete(id) }
}
export function pollSession(session: SearchSession): void {
  stopPolling(session.id)
  const poll: { cancelled: boolean; timer?: ReturnType<typeof setTimeout> } = { cancelled: false }
  polling.set(session.id, poll)
  const pump = async () => {
    const held = useSearchSessions.getState().frames[session.id]
    const offset = held?.session.generation === session.generation ? held.text?.hits.length ?? 0 : 0
    try {
      const incoming = await api.page(session.project, session.id, offset)
      if (poll.cancelled) return
      const state = useSearchSessions.getState()
      const before = state.frames[session.id]
      if (before && before.session.generation > incoming.session.generation) return
      const same = before?.session.generation === incoming.session.generation
      // Refresh in another window may reset the generation under this poll. Its first page
      // must start at zero; an old offset is never a valid prefix of the replacement run.
      if (incoming.session.generation !== session.generation) {
        pollSession(incoming.session)
        return
      }
      const hits = incoming.text === null ? [] : spliceHits(same ? before?.text?.hits ?? [] : [], incoming.text.offset, incoming.text.hits)
      const frame = { ...incoming, running: needsAnotherPage(incoming, hits.length), text: incoming.text === null ? null : { ...incoming.text, hits, offset: 0 } }
      useSearchSessions.setState({ frames: { ...state.frames, [session.id]: frame }, errors: { ...state.errors, [session.id]: '' } })
      if (needsAnotherPage(incoming, hits.length)) poll.timer = setTimeout(() => void pump(), 80)
      else polling.delete(session.id)
    } catch (error) {
      if (poll.cancelled) return
      polling.delete(session.id)
      useSearchSessions.setState(s => ({ errors: { ...s.errors, [session.id]: String(error) } }))
    }
  }
  void pump()
}

// Start requests are serialized, including disposal of obsolete responses. The server replaces
// the window's unpinned job; overlapping Starts could otherwise complete in the wrong order.
let starts: Promise<unknown> = Promise.resolve()
export function startSession(project: ProjectId, descriptor: SearchDescriptor, current: () => boolean): Promise<SearchSession | null> {
  const next = starts.catch(() => {}).then(async () => {
    await Promise.allSettled(pinning.values())
    if (!current()) return null
    const session = await api.start(project, descriptor)
    if (!current()) { await api.cancel(project, session.id); return null }
    pollSession(session)
    return session
  })
  starts = next
  return next
}
export function cancelSession(session: SearchSession): void {
  const transfer = pinning.get(session.id)
  if (transfer) { void transfer.then(() => cancelSession(session), () => cancelSession(session)); return }
  const pinned = useSearchSessions.getState().tabs[session.project]?.some(t => t.id === session.id)
  if (pinned) return
  stopPolling(session.id)
  void api.cancel(session.project, session.id).catch(notifyFailure)
  forgetSession(session.id)
}
function forgetSession(id: string): void {
  useSearchSessions.setState(s => {
    const frames = { ...s.frames }; delete frames[id]
    const views = { ...s.views }; delete views[id]
    const errors = { ...s.errors }; delete errors[id]
    return { frames, views, errors }
  })
}
export function pinSession(session: SearchSession): Promise<SearchSession> {
  const pending = pinning.get(session.id)
  if (pending) return pending
  changed(session.project)
  const transfer = api.pin(session.project, session.id).then(pinned => {
    const boot = useWorkspace.getState().boot
    if (boot && !boot.workspace.projects[session.project]) throw new Error('The project was closed.')
    useSearchSessions.setState(s => ({ tabs: { ...s.tabs, [session.project]: [...(s.tabs[session.project] ?? []).filter(t => t.id !== pinned.id), pinned] } }))
    return pinned
  }).finally(() => { pinning.delete(session.id); changed(session.project) })
  pinning.set(session.id, transfer)
  return transfer
}
export async function refreshSession(session: SearchSession): Promise<void> {
  changed(session.project)
  const next = await api.rerun(session.project, session.id)
  changed(session.project)
  if (!useSearchSessions.getState().tabs[session.project]?.some(t => t.id === next.id && t.generation <= next.generation)) return
  stopPolling(session.id)
  forgetSession(session.id)
  useSearchSessions.setState(s => ({ tabs: { ...s.tabs, [session.project]: (s.tabs[session.project] ?? []).map(t => t.id === next.id ? next : t) } }))
  pollSession(next)
}
export async function closeSession(session: SearchSession): Promise<void> {
  changed(session.project)
  await api.close(session.project, session.id)
  changed(session.project)
  stopPolling(session.id)
  forgetSession(session.id)
  useSearchSessions.setState(s => ({ tabs: { ...s.tabs, [session.project]: (s.tabs[session.project] ?? []).filter(t => t.id !== session.id) } }))
}
export async function syncSessions(project: ProjectId): Promise<void> {
  const revision = revisions.get(project)
  const request = (lists.get(project) ?? 0) + 1
  lists.set(project, request)
  const tabs = await api.list(project)
  if (revision !== revisions.get(project) || request !== lists.get(project)) return
  const state = useSearchSessions.getState()
  for (const old of state.tabs[project] ?? []) {
    if (!tabs.some(t => t.id === old.id)) { stopPolling(old.id); forgetSession(old.id) }
  }
  useSearchSessions.setState(s => ({ tabs: { ...s.tabs, [project]: tabs } }))
  for (const tab of tabs) {
    const frame = useSearchSessions.getState().frames[tab.id]
    if (!polling.has(tab.id) && (!frame || frame.session.generation !== tab.generation || frame.running)) pollSession(tab)
  }
}

// A closed project releases all of its local mirrors too. Switching the active project does
// not: its pinned jobs still belong to the process and complete while their panel is hidden.
useWorkspace.subscribe((state, previous) => {
  const projects = state.boot?.workspace.projects
  if (!projects || projects === previous.boot?.workspace.projects) return
  const search = useSearchSessions.getState()
  for (const project of new Set([...Object.keys(search.tabs), ...Object.keys(search.drafts), ...Object.keys(previous.boot?.workspace.projects ?? {})])) {
    if (projects[project]) continue
    changed(project)
    for (const tab of search.tabs[project] ?? []) { stopPolling(tab.id); forgetSession(tab.id) }
    useSearchSessions.setState(s => {
      const tabs = { ...s.tabs }; delete tabs[project]
      const drafts = { ...s.drafts }; delete drafts[project]
      return { tabs, drafts }
    })
  }
})
