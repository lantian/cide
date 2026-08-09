/**
 * The pane frame and the 26px bar at the top of it: index, centred title, pane actions.
 *
 * The two live in one module because they share a single rule — the focus ring on the
 * frame's border and the lift of the title's colour are the same piece of state, and
 * splitting them across files invites one being updated without the other.
 *
 * The index is computed depth-first by the caller at render time rather than stored.
 * Storing it would mean renumbering every sibling on each split and close, and the number
 * is presentation — it exists so "focus pane 3" has something to refer to.
 *
 * Neither component talks to Rust. Both take callbacks, so a detached-pane window that
 * mirrors a different workspace slice can render the same frame from props.
 */
import type { ReactNode } from 'react'
import type { Pane } from '@/ipc/client'
import styles from './PaneTitleBar.module.css'

export interface PaneFrameProps {
  pane: Pane
  /** 1-based depth-first position of this pane in its tab's tree. */
  index: number
  focused: boolean
  maximized: boolean
  children: ReactNode
  onFocus?: (() => void) | undefined
  onMaximize?: (() => void) | undefined
  onDetach?: (() => void) | undefined
  onClose?: (() => void) | undefined
}

export function PaneFrame({
  pane,
  index,
  focused,
  maximized,
  children,
  onFocus,
  onMaximize,
  onDetach,
  onClose,
}: PaneFrameProps): ReactNode {
  // Withheld once this pane is already the focused one. `onFocus` is an IPC round trip
  // that re-reads the tree and re-renders every pane in the tab, and a click inside a
  // terminal the user is already typing in must not cost that — nor must it re-run the
  // effects of a `TerminalPane` whose props are rebuilt by that render.
  const raise = focused ? undefined : onFocus

  return (
    // Pointer-down *capture*, so a click that lands inside a terminal focuses the pane
    // before xterm consumes the event. Focus-in capture for the same reason one step out:
    // a pane reached by Tab, or by anything that moves DOM focus without going through the
    // domain, would otherwise take the keystrokes while the ring stayed on another pane.
    // This is not `:focus-within` — the handler asks Rust to move focus and the ring is
    // still drawn only from `tree.focused`, so an overlay taking focus cannot steal it.
    <div
      className={focused ? `${styles.frame} ${styles.frameFocused}` : styles.frame}
      data-audit="pane"
      data-pane-id={pane.id}
      data-focused={focused ? 'true' : 'false'}
      onPointerDownCapture={raise}
      onFocusCapture={raise}
    >
      <PaneTitleBar
        index={index}
        title={pane.title}
        focused={focused}
        maximized={maximized}
        closable={pane.role !== 'primary'}
        onMaximize={onMaximize}
        onDetach={onDetach}
        onClose={onClose}
      />
      <div className={styles.body}>{children}</div>
    </div>
  )
}

export interface PaneTitleBarProps {
  index: number
  title: string
  focused: boolean
  maximized?: boolean | undefined
  /**
   * False for the project console's pane. Withholding the button is a courtesy to the
   * user, not the enforcement — Rust answers `PanePrimary` whether or not we offer it.
   */
  closable?: boolean | undefined
  onMaximize?: (() => void) | undefined
  onDetach?: (() => void) | undefined
  onClose?: (() => void) | undefined
}

export function PaneTitleBar({
  index,
  title,
  focused,
  maximized = false,
  closable = true,
  onMaximize,
  onDetach,
  onClose,
}: PaneTitleBarProps): ReactNode {
  return (
    <div className={styles.bar} data-audit="paneTitle">
      <span className={styles.index}>{index}</span>
      <span className={focused ? `${styles.title} ${styles.titleFocused}` : styles.title}>
        {title}
      </span>
      <span className={styles.actions}>
        <button
          type="button"
          className={maximized ? `${styles.action} ${styles.actionOn}` : styles.action}
          title={maximized ? 'Restore pane' : 'Maximize pane'}
          aria-label={maximized ? 'Restore pane' : 'Maximize pane'}
          aria-pressed={maximized}
          onClick={onMaximize}
        >
          ⛶
        </button>
        <button
          type="button"
          className={styles.action}
          title="Detach into a window"
          aria-label="Detach pane into a window"
          onClick={onDetach}
        >
          ⧉
        </button>
        {/*
         * `aria-disabled` rather than `disabled`: a disabled button swallows pointerdown
         * without bubbling, so clicking the close button of the primary pane would be the
         * one click in the frame that fails to focus it.
         */}
        <button
          type="button"
          className={closable ? styles.action : `${styles.action} ${styles.actionDisabled}`}
          title={closable ? 'Close pane' : 'The project console pane cannot be closed'}
          // The reason travels in the name, not only in the tooltip: `title` reaches a
          // mouse pointer, and the one user who cannot see the dimming is the one reading
          // this button through a screen reader.
          aria-label={closable ? 'Close pane' : 'The project console pane cannot be closed'}
          aria-disabled={!closable}
          onClick={closable ? onClose : undefined}
        >
          ×
        </button>
      </span>
    </div>
  )
}
