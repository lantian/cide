/**
 * Which top-level tab the Agents panel shows: *Subagents*, or *Waiting* for you. (M132)
 *
 * Here rather than in the panel, for `milestonesStore`'s `TasksTab` reason: something outside the
 * panel has to be able to ask for a tab. The run row's "Waiting for your answer: …" line is the
 * first — it is drawn on the Subagents tab and is a link to the Waiting tab, where the answer box
 * is. The rail's waiting mark on the Agents button counts what that tab holds, but a click on the
 * button still only opens the panel on whichever tab it was left on: the button is the panel's,
 * and a rail button that sometimes also switched tabs would be two gestures in one.
 *
 * *Waiting* sat on the Tasks panel first, as a third tab beside Tasks and Milestones. It moved
 * here because what it lists is the other end of the runs this panel draws: a question a run
 * asked, a user-accepted task a run finished. Read from the Tasks panel it was one more tab on a
 * board the user was not looking at while a run stood idle behind it.
 *
 * For the life of the window and not per project, the way the Tasks panel's tab is: the choice is
 * "what I look at in this panel", and a switch of project that silently changed it would be the
 * panel moving under the user. (The inner Agents/History pair stays per project in
 * `AgentsPanelHost` — a History left open is a statement about that project's runs.)
 *
 * A mirror of nothing, and no IPC: gesture state, which ADR 0002 leaves to the webview.
 */
import { create } from 'zustand'

import { requestPanel } from '@/chrome/panelRequests'

/**
 * The Agents panel's top-level tabs. Not `AgentsTab`, which is the inner Agents/History pair.
 * *Sessions* (M134) sits between the two: it is about the runs and consoles the first tab shows,
 * and *Waiting* stays last, where the eye finds its count.
 */
export type AgentsPanelTab = 'subagents' | 'sessions' | 'waiting'

interface AgentsTabStore {
  tab: AgentsPanelTab
  setTab: (tab: AgentsPanelTab) => void
  /**
   * Switch to *Waiting* and make sure the panel is on screen. The rail may be showing Files, or
   * the sidebar may be shut: a tab switched in a panel nobody can see is a click that did nothing.
   */
  showWaiting: () => void
}

export const useAgentsTab = create<AgentsTabStore>((set) => ({
  tab: 'subagents',
  setTab: (tab) => set({ tab }),
  showWaiting: () => {
    set({ tab: 'waiting' })
    requestPanel('agents')
  },
}))
