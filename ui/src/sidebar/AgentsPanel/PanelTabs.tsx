/**
 * The Agents panel's top-level tabs, drawn where its header says *Agents*: **Subagents** and
 * **Waiting**. (M132)
 *
 * The Tasks panel's `TasksTabs` markup and classes, on purpose and not a lookalike: two sidebar
 * panels whose headers both carry tabs must read as one application, and a third tab look in the
 * sidebar (there are already the header strip and the Agents/History segmented pair under it) is
 * what the kit's rule 1 exists to prevent. So this imports `MilestonesPanel.module.css` rather
 * than copying its four rules — a copy is the version that drifts first.
 *
 * *Waiting* on the strip and *Waiting for you* in its tooltip and on the panel, `TasksTabs`'
 * reason when it carried the tab: the header also holds the run figure and the project-scope
 * Pause, and the long label pushed them off a 280px header.
 *
 * Pure, like everything the render check draws: the tab and the count arrive as props, the click
 * leaves as one. The host (`AgentsPanelHost`) reads `agentsTabStore`.
 */
import type { AgentsPanelTab } from '@/sidebar/agentsTabStore'

import styles from '../TasksPanel/MilestonesPanel.module.css'

export function AgentsPanelTabs({
  tab,
  onTab,
  waiting = 0,
}: {
  tab: AgentsPanelTab
  onTab: (tab: AgentsPanelTab) => void
  /** Tasks waiting for the user — `waitingCount` over the board — for the Waiting tab's count. */
  waiting?: number
}) {
  return (
    <span className={styles.tabs} role="tablist" aria-label="Agents panel" data-audit="agentsPanelTabs">
      {(['subagents', 'waiting'] as const).map((t) => (
        <button
          key={t}
          type="button"
          role="tab"
          aria-selected={tab === t}
          className={tab === t ? `${styles.tab} ${styles.tabOn}` : styles.tab}
          data-audit="agentsPanelTab"
          data-tab={t}
          title={t === 'waiting' ? 'Waiting for you' : 'Subagents, and what each is doing now'}
          onClick={() => onTab(t)}
        >
          {t === 'subagents' ? 'Subagents' : 'Waiting'}
          {/* Only a positive count: an empty Waiting tab is the ordinary state, and a `0` pill
              beside it would be a call to action with nothing to act on. */}
          {t === 'waiting' && waiting > 0 && (
            <span
              className={styles.tabCount}
              data-audit="waitingCount"
              title={`${waiting} task${waiting === 1 ? '' : 's'} waiting for you`}
            >
              {waiting}
            </span>
          )}
        </button>
      ))}
    </span>
  )
}
