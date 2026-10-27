import { create } from 'zustand'
import { events, userRequests, type UserRequestResponse, type UserRequestsView } from '@/ipc/client'

interface State {
  project: string | null
  view: UserRequestsView | null
  error: string | null
  attach: (project: string | null) => Promise<void>
  adopt: (project: string, view: UserRequestsView) => void
  respond: (id: string, response: UserRequestResponse) => Promise<void>
  dismiss: (id: string) => Promise<void>
}
let generation = 0
export const useUserRequests = create<State>((set, get) => ({
  project: null, view: null, error: null,
  attach: async (project) => {
    const mine = ++generation
    set({ project, view: null, error: null })
    if (project === null) return
    try {
      const view = await userRequests.list(project)
      if (generation === mine) get().adopt(project, view)
    } catch (error) {
      if (generation === mine) set({ error: String(error) })
    }
  },
  adopt: (project, view) => {
    const held = get().view
    if (get().project === project && (held === null || view.rev > held.rev)) set({ view, error: null })
  },
  respond: async (id, response) => {
    const project = get().project
    if (project === null) throw new Error('The project is no longer open.')
    get().adopt(project, await userRequests.respond(project, id, response))
  },
  dismiss: async (id) => {
    const project = get().project
    if (project === null) throw new Error('The project is no longer open.')
    get().adopt(project, await userRequests.dismiss(project, id))
  },
}))
void events.onUserRequestsChanged(({ project, view }) => useUserRequests.getState().adopt(project, view))
