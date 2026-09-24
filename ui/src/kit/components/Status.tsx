/**
 * The kit's status marks. `Status.module.css` says what each tone means — a tone is a meaning,
 * not a colour to pick for looks.
 */
import type { ReactElement, ReactNode } from 'react'

import { Icon, type IconName } from '@/icons/Icon'
import { cx } from './cx'
import styles from './Status.module.css'

export type Tone = 'neutral' | 'accent' | 'green' | 'yellow' | 'red' | 'purple' | 'blue' | 'cyan'

export type BadgeProps = {
  tone?: Tone | undefined
  /** A state (running, passed) gets the dot; a kind (MR state, severity) is `squared`. */
  dot?: boolean | undefined
  squared?: boolean | undefined
  /** The quiet form — hue on a wash, no fill — for a badge repeated down a long list. */
  soft?: boolean | undefined
  icon?: IconName | undefined
  children: ReactNode
}

export function Badge({
  tone = 'neutral',
  dot = false,
  squared = false,
  soft = false,
  icon,
  children,
}: BadgeProps): ReactElement {
  return (
    <span
      data-tone={tone}
      className={cx(
        styles.badge,
        dot && styles.dotted,
        squared && styles.squared,
        soft && styles.soft,
      )}
    >
      {icon !== undefined && <Icon name={icon} size={0} />}
      {children}
    </span>
  )
}

export type TagProps = {
  tone?: Tone | undefined
  children: ReactNode
  /** Present → the tag gets its ✕. The label names what is removed. */
  onRemove?: (() => void) | undefined
}

export function Tag({ tone = 'neutral', children, onRemove }: TagProps): ReactElement {
  return (
    <span data-tone={tone} className={styles.tag}>
      {children}
      {onRemove !== undefined && (
        <button
          type="button"
          className={styles.tagRemove}
          aria-label={`Remove ${typeof children === 'string' ? children : 'tag'}`}
          onClick={onRemove}
        >
          <Icon name="x" size={0} />
        </button>
      )}
    </span>
  )
}

export function Counter({
  value,
  tone = 'neutral',
}: {
  value: number
  tone?: Tone | undefined
}): ReactElement {
  return (
    <span data-tone={tone} className={styles.counter}>
      {value > 99 ? '99+' : value}
    </span>
  )
}

export function Dot({
  tone = 'neutral',
  pulse = false,
  label,
}: {
  tone?: Tone | undefined
  pulse?: boolean | undefined
  label: string
}): ReactElement {
  return (
    <span
      data-tone={tone}
      role="img"
      aria-label={label}
      title={label}
      className={cx(styles.dot, pulse && styles.pulse)}
    />
  )
}

/** `keys` is one chord: `['Ctrl', 'Shift', 'P']`. */
export function Kbd({ keys }: { keys: readonly string[] }): ReactElement {
  return (
    <kbd className={styles.kbd}>
      {keys.map((k) => (
        <span key={k} className={styles.key}>
          {k}
        </span>
      ))}
    </kbd>
  )
}

export function Avatar({
  name,
  src,
  tone = 'neutral',
}: {
  name: string
  src?: string | undefined
  tone?: Tone | undefined
}): ReactElement {
  const initials = name
    .split(/\s+/)
    .map((w) => w[0] ?? '')
    .join('')
    .slice(0, 2)
    .toUpperCase()
  return (
    <span data-tone={tone} className={styles.avatar} title={name}>
      {src !== undefined ? <img src={src} alt="" /> : initials}
    </span>
  )
}

export function Person({
  name,
  tone,
}: {
  name: string
  tone?: Tone | undefined
}): ReactElement {
  return (
    <span className={styles.person}>
      <Avatar name={name} tone={tone} />
      {name}
    </span>
  )
}

export function Code({ children }: { children: ReactNode }): ReactElement {
  return <code className={styles.code}>{children}</code>
}
