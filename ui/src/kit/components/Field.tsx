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
import { IconButton, type ControlSize } from './Button'
import { OUTCOME_ICON, type Outcome } from './Feedback'
import { InfoTip } from './InfoTip'
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
  /** One line under the label. More than that goes in `info`. */
  hint?: ReactNode
  /** The rest of the explanation, behind an (i) after the label. (M133) */
  info?: ReactNode
  /**
   * Something wrong (or worth knowing) about **this** setting's current state: the row takes the
   * tone as a bar down its left edge and says why in one line under the label. The replacement
   * for a `Note` placed between two rows, which the user could not tie to either of them.
   */
  status?: { tone: Outcome; text: string } | undefined
  /** The value differs from its default: an accent dot before the label. */
  modified?: boolean | undefined
  /** Drawn while `modified`: a quiet Reset to default beside the control. */
  onReset?: (() => void) | undefined
  /** `data-setting`, what Settings search scrolls to and flashes. */
  anchor?: string | undefined
  children: ReactNode
}

/** Label and hint on the left, the control on the right — a settings row. */
export function FormRow({
  label,
  hint,
  info,
  status,
  modified,
  onReset,
  anchor,
  children,
}: FormRowProps): ReactElement {
  return (
    <div className={styles.row} data-tone={status?.tone} data-setting={anchor}>
      <div className={styles.rowText}>
        <div className={styles.rowLabel}>
          {modified === true && (
            <span className={styles.rowModified} role="img" aria-label="Changed from the default" />
          )}
          {label}
          {info !== undefined && <InfoTip label={`About ${label}`}>{info}</InfoTip>}
        </div>
        {status !== undefined && (
          <div className={styles.rowStatus}>
            <Icon name={OUTCOME_ICON[status.tone]} size={1} />
            <span>{status.text}</span>
          </div>
        )}
        {hint !== undefined && <div className={styles.rowHint}>{hint}</div>}
      </div>
      <div className={styles.rowControl}>
        {modified === true && onReset !== undefined && (
          <IconButton icon="undo-2" label={`Reset ${label} to default`} onClick={onReset} />
        )}
        {children}
      </div>
    </div>
  )
}
