/**
 * The kit's feedback: notes, banners, toasts, the empty state and progress. See
 * `Feedback.module.css`.
 *
 * Tones here are outcomes (`info`, `ok`, `warn`, `bad`), not hues: a note says how the thing
 * went, and the hue follows. `Status.tsx`'s tones are hues because a badge labels a state that
 * some other system named.
 */
import type { ReactElement, ReactNode } from 'react'

import { Icon, type IconName } from '@/icons/Icon'
import styles from './Feedback.module.css'

export type Outcome = 'info' | 'ok' | 'warn' | 'bad'

const OUTCOME_ICON: Record<Outcome, IconName> = {
  info: 'info',
  ok: 'circle-check',
  warn: 'triangle-alert',
  bad: 'circle-alert',
}

/* Inside the note's round mark: glyphs with no circle of their own, since a `circle-check` in a
   filled disc draws a ring inside a ring. `info` has no uncircled form in the set. */
const NOTE_MARK: Record<Outcome, IconName> = {
  info: 'info',
  ok: 'check',
  warn: 'triangle-alert',
  bad: 'x',
}

export type NoteProps = {
  tone?: Outcome | undefined
  title?: string | undefined
  children: ReactNode
  /** A single button at the right edge, for the one thing the note lets you do about it. */
  action?: ReactNode
}

export function Note({ tone = 'info', title, children, action }: NoteProps): ReactElement {
  return (
    <div className={styles.note} data-tone={tone} role={tone === 'bad' ? 'alert' : 'status'}>
      <span className={styles.noteMark}>
        <Icon name={NOTE_MARK[tone]} size={0} />
      </span>
      <div className={styles.noteBody}>
        {title !== undefined && <strong className={styles.noteTitle}>{title}</strong>}
        {children}
      </div>
      {action !== undefined && <div className={styles.noteAction}>{action}</div>}
    </div>
  )
}

export function Banner({
  tone = 'info',
  children,
  action,
}: {
  tone?: Outcome | undefined
  children: ReactNode
  action?: ReactNode
}): ReactElement {
  return (
    <div className={styles.banner} data-tone={tone} role="status">
      <Icon name={OUTCOME_ICON[tone]} size={1} />
      <span className={styles.bannerText}>{children}</span>
      {action}
    </div>
  )
}

export function Toast({
  tone = 'info',
  title,
  children,
  action,
}: {
  tone?: Outcome | undefined
  title: string
  children?: ReactNode
  action?: ReactNode
}): ReactElement {
  return (
    <div className={styles.toast} data-tone={tone} role="status">
      <Icon name={OUTCOME_ICON[tone]} size={2} />
      <div className={styles.toastBody}>
        <strong className={styles.toastTitle}>{title}</strong>
        {children}
      </div>
      {action}
    </div>
  )
}

export function EmptyState({
  icon,
  title,
  children,
  action,
}: {
  icon: IconName
  title: string
  children?: ReactNode
  action?: ReactNode
}): ReactElement {
  return (
    <div className={styles.empty}>
      <span className={styles.emptyMark}>
        <Icon name={icon} size={3} />
      </span>
      <span className={styles.emptyTitle}>{title}</span>
      {children !== undefined && <p className={styles.emptyText}>{children}</p>}
      {action}
    </div>
  )
}

export function Spinner({ label }: { label?: string | undefined }): ReactElement {
  return (
    <span className={styles.loading} role="status">
      <Icon name="loader-circle" size={2} className={styles.spin} />
      {label}
    </span>
  )
}

export function Progress({
  value,
  label,
  tone,
}: {
  /** 0–1. */
  value: number
  label?: string | undefined
  tone?: 'ok' | 'bad' | undefined
}): ReactElement {
  const pct = Math.round(Math.min(1, Math.max(0, value)) * 100)
  return (
    <div className={styles.progress}>
      {label !== undefined && (
        <div className={styles.progressHead}>
          <span>{label}</span>
          <span>{pct}%</span>
        </div>
      )}
      <div
        className={styles.track}
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct}
        aria-label={label}
      >
        <div className={styles.fill} data-tone={tone} style={{ width: `${pct}%` }} />
      </div>
    </div>
  )
}

export function Skeleton({ width }: { width: string }): ReactElement {
  return <span className={styles.skeleton} style={{ width }} aria-hidden />
}

export type CheckState = 'waiting' | 'running' | 'done' | 'failed'

const CHECK_ICON: Record<CheckState, IconName> = {
  waiting: 'circle-dashed',
  running: 'loader-circle',
  done: 'circle-check',
  failed: 'circle-x',
}

export function Checklist({
  items,
}: {
  items: ReadonlyArray<{ label: string; state: CheckState; detail?: string }>
}): ReactElement {
  return (
    <ol className={styles.checklist}>
      {items.map((it) => (
        <li key={it.label} className={styles.check} data-state={it.state}>
          <span className={styles.checkMark}>
            <Icon
              name={CHECK_ICON[it.state]}
              size={2}
              className={it.state === 'running' ? styles.spin : undefined}
            />
          </span>
          <span className={styles.checkBody}>
            {it.label}
            {it.detail !== undefined && (
              <span className={styles.checkDetail}>{it.detail}</span>
            )}
          </span>
        </li>
      ))}
    </ol>
  )
}
