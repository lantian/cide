/**
 * Holds every tab of the active project at once and shows exactly one.
 *
 * The obvious implementation — render the active tab, drop the rest — is the one thing this
 * component exists to avoid. Three separate failures follow from it:
 *
 * 1. Unmounting an inactive tab unmounts its `PaneSlot`s. Slots park their hosts rather than
 *    destroy them, so the terminals survive, but the panes' React state, their measured
 *    geometry and their resize observers all go, and rebuilding that on every tab switch is
 *    work the user pays for while a Claude turn is streaming.
 * 2. Hiding with `display: none` is worse than unmounting. The subtree still exists, so
 *    nothing warns you, but xterm's IntersectionObserver reports non-intersecting and every
 *    measurement reads zero; `fitAddon.fit()` then computes garbage and the next resize
 *    sends a corrupt size to the child process, which reflows its output around a terminal
 *    that is 0 columns wide.
 * 3. A tab that mounts lazily on first activation pays a PTY spawn — and a fresh Claude
 *    session — on every switch to a tab that was closed and reopened in the same window.
 *
 * So: one positioned container, every tab rendered into it, exactly one visible. The hidden
 * panels are laid out at full size against the container, which is what keeps their
 * terminals' measurements honest while they are out of sight.
 *
 * Nothing here reads the IPC surface. The tabs, which one is active, and how to draw a
 * tree all arrive as props, so the component can be driven from a fixture in the layout
 * audit with no Rust behind it.
 */
import type { ReactNode } from 'react'
import styles from './TabContent.module.css'

/**
 * The shape this component needs from a tab. Deliberately narrower than the IPC `Tab`:
 * `renderTree` is generic over the caller's own type, so the caller still hands its full
 * tab — tree and all — to its own render function.
 */
export interface TabLike {
  id: string
}

export interface TabContentProps<T extends TabLike = TabLike> {
  tabs: readonly T[]
  activeTab: string
  /**
   * May return nothing: a settings or diff tab has no pane tree to draw.
   *
   * `active` is passed because the hiding here is pure CSS and therefore invisible to
   * everything below: a hidden tab's panes are mounted, laid out at full size and still
   * painting. Anything that has to distinguish the tab in front from the ones behind it —
   * the WebGL pool, which grants contexts to a fixed number of terminals and has no other
   * way to learn that four of them are off screen; restoring keyboard focus, which the
   * browser drops when a focused element goes `visibility: hidden` — needs this flag,
   * because the DOM alone will not tell it. Ignoring the argument is fine.
   */
  renderTree: (tab: T, active: boolean) => ReactNode
}

export function TabContent<T extends TabLike>({
  tabs,
  activeTab,
  renderTree,
}: TabContentProps<T>): ReactNode {
  return (
    <div className={styles.stack}>
      {tabs.map((tab) => {
        const active = tab.id === activeTab
        return (
          // Keyed by tab id, never by index: reordering or closing a tab with index keys
          // would make React reuse one tab's panel for another's contents, unmounting the
          // wrong slots and moving hosts into the wrong tree.
          <div
            key={tab.id}
            data-tab-id={tab.id}
            role="tabpanel"
            aria-hidden={!active}
            className={active ? `${styles.panel} ${styles.panelActive}` : styles.panel}
          >
            {renderTree(tab, active)}
          </div>
        )
      })}
    </div>
  )
}
