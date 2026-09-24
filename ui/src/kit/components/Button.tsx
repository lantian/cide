/**
 * The kit's buttons. See `Button.module.css` for which variant is for what.
 *
 * `busy` swaps the leading icon for a turning `loader-circle` and disables the button, because a
 * button that can be pressed twice while its first press is in flight is the double-submit every
 * dialog in this app has had to guard by hand.
 */
import type { ButtonHTMLAttributes, ReactElement, ReactNode, Ref } from 'react'

import { Icon, type IconName } from '@/icons/Icon'
import { cx } from './cx'
import styles from './Button.module.css'

export type ButtonVariant = 'primary' | 'secondary' | 'quiet' | 'danger' | 'link'
export type ControlSize = 'sm' | 'md'

/*
 * `ref` is a plain prop in React 19, so spreading `rest` forwards it — the typing just has to
 * admit it. Dialogs need it: a destructive confirm focuses its safe button on mount.
 */
type Native = Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'className' | 'children'> & {
  ref?: Ref<HTMLButtonElement> | undefined
}

export type ButtonProps = Native & {
  variant?: ButtonVariant | undefined
  size?: ControlSize | undefined
  icon?: IconName | undefined
  /** An icon after the label — a chevron on a button that opens a menu. */
  trailingIcon?: IconName | undefined
  busy?: boolean | undefined
  block?: boolean | undefined
  children?: ReactNode
}

export function Button({
  variant = 'secondary',
  size = 'md',
  icon,
  trailingIcon,
  busy = false,
  block = false,
  disabled,
  type = 'button',
  children,
  ...rest
}: ButtonProps): ReactElement {
  const iconSize = size === 'sm' ? 1 : 2
  return (
    <button
      {...rest}
      type={type}
      disabled={disabled === true || busy}
      aria-busy={busy || undefined}
      className={cx(
        styles.button,
        styles[size],
        styles[variant],
        block && styles.block,
      )}
    >
      {busy ? (
        <Icon name="loader-circle" size={iconSize} className={styles.busy} />
      ) : (
        icon !== undefined && <Icon name={icon} size={iconSize} />
      )}
      {children}
      {trailingIcon !== undefined && <Icon name={trailingIcon} size={iconSize} />}
    </button>
  )
}

export type IconButtonProps = Omit<Native, 'aria-label' | 'title'> & {
  icon: IconName
  /** Required: the icon is the only content, so this is the control's whole name. */
  label: string
  size?: ControlSize | undefined
  /** A toggle's state. Leave undefined for a plain action. */
  pressed?: boolean | undefined
}

export function IconButton({
  icon,
  label,
  size = 'sm',
  pressed,
  type = 'button',
  ...rest
}: IconButtonProps): ReactElement {
  return (
    <button
      {...rest}
      type={type}
      aria-label={label}
      title={label}
      aria-pressed={pressed}
      className={cx(styles.icon, styles[size])}
    >
      <Icon name={icon} size={size === 'sm' ? 1 : 2} />
    </button>
  )
}
