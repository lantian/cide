/**
 * The header strip of a `tab:<uuid>` window: what the tab is, a way to drag the window, a
 * way to put the tab back, and the window's own lights.
 *
 * Everything else about a torn-out tab renders through the shell's own path — `App.tsx`
 * feeds `TabContent` the one tab and the panes are the same `PaneFrame`/`PaneBody` the
 * shell mounts — so this component is deliberately only the chrome the shell would
 * otherwise supply: `AppHeader` carries the drag region and the traffic lights there, and a
 * window without either cannot be moved or closed.
 *
 * The stylesheet is `DetachedPaneWindow.module.css`, shared rather than copied: the two
 * torn-out window kinds sit side by side on the desktop, and the pane window's header
 * comment already states the rule — a bar a few pixels off reads as a different
 * application. The lights are `AppHeader`'s own `WindowLights`, for the same reason one
 * layer up; unlike the pane window, whose single pane draws window controls in its corner
 * cluster, a tab window can hold several panes and each cluster keeps its *pane* actions —
 * so the window controls live here.
 *
 * Closing this window (the light, Alt+F4, `window_close`) re-docks the tab; the Redock
 * button is the same outcome asked for by name, exactly as the pane window offers both.
 */
import type { ReactNode } from 'react'
import type { Tab, TabKind } from '@/ipc/client'
import { WindowLights } from '@/chrome/AppHeader'
import { currentUserAgent, windowControlLayout } from '@/chrome/windowControls'
import styles from './DetachedPaneWindow.module.css'

const CONTROLS = windowControlLayout(currentUserAgent())

/**
 * What the strip would call this tab, as plain text.
 *
 * The same names `TabStrip::viewFor` renders — `Claude` for the console, the basename for a
 * file, the stored titles a diff and a revision carry — minus the badges and swatches,
 * which need the strip's stylesheet. Exhaustive on purpose: a `TabKind` variant added later
 * fails to compile here rather than falling back to a blank header.
 */
function tabTitle(kind: TabKind): string {
  switch (kind.kind) {
    case 'claudeHome':
      return 'Claude'
    case 'claudeFull':
      return kind.title
    case 'file':
      return kind.path.split('/').at(-1) ?? kind.path
    case 'diff':
      return kind.spec.title
    case 'revision':
      return kind.title
    case 'merge':
      return kind.path.split('/').at(-1) ?? kind.path
    case 'settings':
      return 'Settings'
    case 'extension':
      return kind.name
    case 'openSpec':
      return kind.subject.kind === 'change' ? kind.subject.change : kind.subject.spec
  }
}

export interface DetachedTabHeaderProps {
  /** `null` while the mirror has not caught up, or after the project closed under the window. */
  tab: Tab | null
  /** Put the tab back in its shell's strip and close this window. */
  onRedock: () => void
}

export function DetachedTabHeader({ tab, onRedock }: DetachedTabHeaderProps): ReactNode {
  const title = tab === null ? 'cide' : tabTitle(tab.kind)
  return (
    <div className={styles.header} data-audit="detachedHeader">
      {CONTROLS.side === 'left' ? <WindowLights /> : null}

      {/* `title` as well as the text, for the reason the pane window gives: the strip is
          narrow and this is the only thing naming what the window shows. */}
      <span className={styles.title} title={title}>
        {title}
      </span>

      {/* The only drag surface — `data-tauri-drag-region` does not inherit, so it sits on
          an empty box rather than on the header, where it would turn the buttons into drag
          handles. `data-window-drag` is `WindowFrame`'s double-click-to-maximize hook. */}
      <div className={styles.filler} data-tauri-drag-region data-window-drag="true" />

      <button
        type="button"
        className={styles.redock}
        title="Put this tab back in its window's strip"
        onClick={onRedock}
      >
        Redock
      </button>

      {CONTROLS.side === 'right' ? <WindowLights /> : null}
    </div>
  )
}
