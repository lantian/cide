/**
 * The app's chrome, as kit parts: the activity rail's button, a strip of editor-style tabs, the
 * status bar and its items, a pane's title bar, and the window's own controls.
 *
 * Until the redesign (2026-09-24) the kit left these out (its rule 7, "app chrome is out of
 * scope"); the user asked for the chrome on the kit too. These components are the *drawing*:
 * the app's `chrome/TabStrip.tsx`, `chrome/ActivityRail.tsx`, `chrome/StatusBar.tsx` and
 * `layout/PaneTitleBar.tsx` keep their own markup — drag, overflow, detach and the layout audit
 * all read it — and compose these classes from `Chrome.module.css`, so a tab, a rail button or
 * a pane bar looks the same wherever it is drawn, and a change here reaches the app.
 *
 * Same numbers as the app: `--h-tabstrip`, `--h-rail`, `--h-status`, `--h-panelheader`.
 */
import type { ReactElement, ReactNode } from 'react'

import { Icon, type IconName } from '@/icons/Icon'
import { Counter, type Tone } from './Status'
import styles from './Chrome.module.css'

/** One destination on the activity rail: an icon, the current bar, an optional count. */
export function RailButton({
  icon,
  label,
  current = false,
  count,
  countTone = 'neutral',
  onClick,
}: {
  icon: IconName
  /** The panel's name; the rail has no text, so this is its name and its tooltip. */
  label: string
  current?: boolean | undefined
  /** A number worth a glance (changed files, tasks to review). Zero draws nothing. */
  count?: number | undefined
  /** `accent` only when the count asks for the user; `red` when it is failures. */
  countTone?: Tone | undefined
  onClick?: (() => void) | undefined
}): ReactElement {
  return (
    <button
      type="button"
      className={styles.railButton}
      aria-label={label}
      title={label}
      aria-pressed={current}
      onClick={onClick}
    >
      <Icon name={icon} size={3} />
      {count !== undefined && count > 0 && (
        <span className={styles.railCount}>
          <Counter value={count} tone={countTone} />
        </span>
      )}
    </button>
  )
}

/** The activity rail itself: a column of `RailButton`s on `--chrome`. */
export function Rail({ label, children }: { label: string; children: ReactNode }): ReactElement {
  return (
    <nav className={styles.rail} aria-label={label}>
      {children}
    </nav>
  )
}

/** A strip of editor-style tabs, `--h-tabstrip` tall on `--panel-2`. */
export function ChromeTabs({ label, children }: { label: string; children: ReactNode }): ReactElement {
  return (
    <div className={styles.strip} role="tablist" aria-label={label}>
      {children}
    </div>
  )
}

export function ChromeTab({
  title,
  icon,
  current = false,
  dirty = false,
  onSelect,
  onClose,
}: {
  title: string
  icon?: IconName | undefined
  current?: boolean | undefined
  /** Unsaved: the close button wears the accent dot until it is pointed at. */
  dirty?: boolean | undefined
  onSelect?: (() => void) | undefined
  onClose?: (() => void) | undefined
}): ReactElement {
  return (
    <div className={styles.tab} role="tab" aria-selected={current} tabIndex={current ? 0 : -1} onClick={onSelect}>
      {icon !== undefined && <Icon name={icon} size={1} />}
      <span className={styles.tabTitle}>{title}</span>
      {onClose !== undefined && (
        <button
          type="button"
          className={styles.tabClose}
          data-dirty={dirty || undefined}
          aria-label={`Close ${title}`}
          title={dirty ? `${title} has unsaved changes` : `Close ${title}`}
          onClick={(e) => {
            e.stopPropagation()
            onClose()
          }}
        >
          {dirty && <span className={styles.dirtyDot} aria-hidden="true" />}
          <Icon name="x" size={1} />
        </button>
      )}
    </div>
  )
}

/** The bottom bar: `--h-status` tall, `--chrome`, 11px. Items on the left, a spacer, items right. */
export function StatusBar({ children }: { children: ReactNode }): ReactElement {
  return (
    <footer className={styles.status} role="status">
      {children}
    </footer>
  )
}

export function StatusItem({
  icon,
  tone,
  children,
  onClick,
}: {
  icon?: IconName | undefined
  /** A status hue for the icon only (`green`, `yellow`, `red`); the words stay `--dim`. */
  tone?: 'green' | 'yellow' | 'red' | 'blue' | 'purple' | undefined
  children: ReactNode
  onClick?: (() => void) | undefined
}): ReactElement {
  const body = (
    <>
      {icon !== undefined && (
        <span className={styles.statusIcon} data-tone={tone}>
          <Icon name={icon} size={0} />
        </span>
      )}
      {children}
    </>
  )
  return onClick === undefined ? (
    <span className={styles.statusItem}>{body}</span>
  ) : (
    <button type="button" className={styles.statusItem} onClick={onClick}>
      {body}
    </button>
  )
}

/** A pane's title bar: the title (lit when the pane has focus) and its tools at the right. */
export function PaneBar({
  title,
  icon,
  focused = false,
  tools,
}: {
  title: string
  icon?: IconName | undefined
  focused?: boolean | undefined
  tools?: ReactNode
}): ReactElement {
  return (
    <div className={styles.paneBar} data-focused={focused || undefined}>
      {icon !== undefined && <Icon name={icon} size={1} />}
      <span className={styles.paneTitle}>{title}</span>
      {tools !== undefined && <span className={styles.paneTools}>{tools}</span>}
    </div>
  )
}

/** A pane bar's or a window's own button: 22px, no border, `--panel-2` under the pointer. */
export function ChromeButton({
  icon,
  label,
  on = false,
  close = false,
  onClick,
}: {
  icon: IconName
  label: string
  /** A toggle that is on: the accent on a 12% accent wash, like every icon toggle. */
  on?: boolean | undefined
  /** The window's close: red under the pointer only, the way a traffic light behaves. */
  close?: boolean | undefined
  onClick?: (() => void) | undefined
}): ReactElement {
  return (
    <button
      type="button"
      className={close ? `${styles.chromeButton} ${styles.chromeClose}` : styles.chromeButton}
      aria-label={label}
      title={label}
      aria-pressed={close ? undefined : on}
      onClick={onClick}
    >
      <Icon name={icon} size={1} />
    </button>
  )
}

/** Minimise, maximise, close — for a window that draws its own frame. */
export function WindowControls({
  onMinimize,
  onMaximize,
  onClose,
}: {
  onMinimize?: (() => void) | undefined
  onMaximize?: (() => void) | undefined
  onClose?: (() => void) | undefined
}): ReactElement {
  return (
    <span className={styles.windowControls}>
      <ChromeButton icon="minus" label="Minimize" onClick={onMinimize} />
      <ChromeButton icon="square" label="Maximize" onClick={onMaximize} />
      <ChromeButton icon="x" label="Close" close onClick={onClose} />
    </span>
  )
}
