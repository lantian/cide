/**
 * The kit's text entry. See `Field.module.css`.
 *
 * `Field` owns the label, the hint and the error and wires them to the control by id, so a field
 * built from the kit is labelled for a screen reader without the caller remembering `htmlFor`
 * and `aria-describedby` — the two attributes every hand-rolled form in the app forgot at least
 * once.
 */
import {
  useId,
  type InputHTMLAttributes,
  type ReactElement,
  type ReactNode,
  type Ref,
  type TextareaHTMLAttributes,
} from 'react'

import { Icon, type IconName } from '@/icons/Icon'
import type { ControlSize } from './Button'
import { cx } from './cx'
import styles from './Field.module.css'

export type FieldProps = {
  label: string
  optional?: boolean | undefined
  hint?: ReactNode
  error?: string | null | undefined
  /** Renders the control with the ids it must carry. */
  children: (ids: { id: string; describedBy: string | undefined; invalid: boolean }) => ReactNode
}

export function Field({ label, optional, hint, error, children }: FieldProps): ReactElement {
  const id = useId()
  const hintId = `${id}-hint`
  const invalid = error !== undefined && error !== null && error !== ''
  const describedBy = invalid ? hintId : hint !== undefined ? hintId : undefined
  return (
    <div className={styles.field}>
      <label htmlFor={id} className={styles.label}>
        {label}
        {optional === true && <span className={styles.optional}>optional</span>}
      </label>
      {children({ id, describedBy, invalid })}
      {invalid ? (
        <p id={hintId} className={styles.error} role="alert">
          {error}
        </p>
      ) : (
        hint !== undefined && (
          <p id={hintId} className={styles.hint}>
            {hint}
          </p>
        )
      )}
    </div>
  )
}

type NativeInput = Omit<InputHTMLAttributes<HTMLInputElement>, 'className' | 'size'> & {
  /** A plain prop in React 19; `rest` forwards it to the `<input>`. */
  ref?: Ref<HTMLInputElement> | undefined
}

export type TextInputProps = NativeInput & {
  size?: ControlSize | undefined
  icon?: IconName | undefined
  mono?: boolean | undefined
  invalid?: boolean | undefined
  /** Something at the right edge inside the box: a clear button, a `Kbd`. */
  trailing?: ReactNode
}

export function TextInput({
  size = 'md',
  icon,
  mono = false,
  invalid = false,
  trailing,
  disabled,
  ...rest
}: TextInputProps): ReactElement {
  return (
    <div
      className={cx(styles.box, styles[size])}
      data-invalid={invalid || undefined}
      data-disabled={disabled === true || undefined}
    >
      {icon !== undefined && (
        <span className={styles.lead}>
          <Icon name={icon} size={size === 'sm' ? 1 : 2} />
        </span>
      )}
      <input
        {...rest}
        disabled={disabled}
        aria-invalid={invalid || undefined}
        className={cx(styles.input, mono && styles.mono)}
      />
      {trailing !== undefined && <span className={styles.trail}>{trailing}</span>}
    </div>
  )
}

/** A `TextInput` with the search mark, `type="search"` and nothing else to decide. */
export function SearchField(props: Omit<TextInputProps, 'icon' | 'type'>): ReactElement {
  return <TextInput {...props} type="search" icon="search" />
}

type NativeTextarea = Omit<TextareaHTMLAttributes<HTMLTextAreaElement>, 'className'> & {
  ref?: Ref<HTMLTextAreaElement> | undefined
}

export function Textarea(props: NativeTextarea): ReactElement {
  return <textarea {...props} className={styles.textarea} />
}

export type FormRowProps = {
  label: string
  hint?: ReactNode
  children: ReactNode
}

/** Label and hint on the left, the control on the right — a settings row. */
export function FormRow({ label, hint, children }: FormRowProps): ReactElement {
  return (
    <div className={styles.row}>
      <div className={styles.rowText}>
        <div className={styles.rowLabel}>{label}</div>
        {hint !== undefined && <div className={styles.rowHint}>{hint}</div>}
      </div>
      <div className={styles.rowControl}>{children}</div>
    </div>
  )
}
