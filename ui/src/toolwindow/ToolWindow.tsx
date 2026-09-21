/**
 * The git tool window's frame: the tab row, and a slot for whatever tab is in front.
 *
 * Pure, in the sense `sidebar/GitPanel/GitPanel.tsx` is: every value is a prop, it reads no store
 * and calls no IPC, and nothing in its import graph touches `document` at module scope. That is
 * what lets it be server-rendered by a `ui/scripts/check-*.mjs` smoke entry, and it is why the
 * window concerns — the workspace mirror, the commands, the splitter — live in
 * `ToolWindowHost.tsx` instead. The `GitPanel`/`GitPanelHost` split is the precedent and the
 * reason is the same: a check that has to boot a webview is a check nobody runs.
 */
import type { ReactNode } from 'react'
import type { TabRow } from './toolWindowModel'
import { Icon } from '@/icons/Icon'

import styles from './ToolWindow.module.css'

export interface ToolWindowViewProps {
  /** Log first, then one per open history tab. From `toolWindowModel::tabRow`. */
  rows: readonly TabRow[]
  /** `null` is the Log tab. */
  onActivate: (id: string | null) => void
  onClose: (id: string) => void
  /** Hide the panel. The `×` at the right of the tab row, and `view.toolWindow.toggle`. */
  onHide: () => void
  /** The active tab's body. */
  children: ReactNode
}

export function ToolWindowView({
  rows,
  onActivate,
  onClose,
  onHide,
  children,
}: ToolWindowViewProps) {
  return (
    <section className={styles.toolWindow} data-audit="toolWindow" aria-label="Git">
      {/*
       * A real tablist, unlike the activity rail's new button: these *are* tabs — one selection,
       * each swapping the body below. The rail's tool-window button is a toggle and carries
       * `aria-pressed` instead, because it opens a panel rather than selecting within one.
       */}
      <div className={styles.tabs} role="tablist" aria-label="Git tool window tabs">
        {rows.map((row) => (
          <div
            key={row.id ?? 'log'}
            className={row.active ? `${styles.tab} ${styles.tabActive}` : styles.tab}
            data-audit="toolWindowTab"
          >
            <button
              type="button"
              role="tab"
              aria-selected={row.active}
              title={row.title}
              className={styles.tabButton}
              onClick={() => onActivate(row.id)}
            >
              {row.label}
            </button>
            {row.closable && row.id !== null && (
              /*
               * A sibling of the tab, not a child of it: a `<button>` inside a `role="tab"` is
               * not focusable in the tab's own reading order, and a close control that a keyboard
               * cannot reach is one only a mouse can use.
               */
              <button
                type="button"
                className={styles.close}
                aria-label={`Close ${row.label}`}
                title={`Close ${row.label}`}
                data-audit="toolWindowTabClose"
                onClick={() => onClose(row.id as string)}
              >
                <Icon name="x" size={1} />
              </button>
            )}
          </div>
        ))}
        <div className={styles.spacer} aria-hidden="true" />
        <button
          type="button"
          className={styles.hide}
          aria-label="Hide the git tool window"
          title="Hide the git tool window"
          data-audit="toolWindowHide"
          onClick={onHide}
        >
          <Icon name="panel-bottom-close" size={1} />
        </button>
      </div>
      <div className={styles.body} data-audit="toolWindowBody">
        {children}
      </div>
    </section>
  )
}

/**
 * The Log tab's body in the shell that has no project open. (M74)
 *
 * The panel is reachable from the empty frame now, and the Docker tab is genuinely live there —
 * a daemon is a property of the machine, not of a checkout. A commit log is not: it walks *this
 * project's* repositories, and there are none.
 *
 * So the tab says which of the two it is rather than drawing an empty list, on
 * `ProblemsPanel`'s recorded rule: an empty list is a claim about a repository that was read,
 * and nothing here has read one. It is pure and lives beside the frame so the smoke entry can
 * render it — the body is the half `check:toolwindow` cannot see.
 */
export function ToolWindowNoProject() {
  return (
    <div className={styles.noProject} data-audit="toolWindowNoProject">
      <p className={styles.claim}>No project open</p>
      <p className={styles.detail}>
        The commit log walks the open project’s repositories. Open one from the header’s{' '}
        <b>+</b> button — or stay here: the <b>Docker</b> tab is about this machine and works
        either way.
      </p>
    </div>
  )
}
