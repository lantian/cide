/**
 * The kit's choice controls. `Choice.module.css` says which one a choice calls for.
 *
 * `Switch` and `Segmented` are ARIA widgets (`role="switch"`, `role="radiogroup"`) over plain
 * buttons, because nothing native draws either. `Segmented` moves with the arrow keys, as a
 * radio group must — a segmented control that only answers Tab is a row of buttons pretending.
 */
import type { KeyboardEvent, ReactElement, ReactNode } from 'react'

import { Icon, type IconName } from '@/icons/Icon'
import type { ControlSize } from './Button'
import { cx } from './cx'
import styles from './Choice.module.css'

export type CheckboxProps = {
  label: ReactNode
  hint?: ReactNode
  checked: boolean
  onChange: (checked: boolean) => void
  disabled?: boolean | undefined
  indeterminate?: boolean | undefined
}

export function Checkbox({
  label,
  hint,
  checked,
  onChange,
  disabled,
  indeterminate,
}: CheckboxProps): ReactElement {
  return (
    <label className={styles.check} data-disabled={disabled === true || undefined}>
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        ref={(el) => {
          if (el !== null) el.indeterminate = indeterminate === true
        }}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span className={styles.checkText}>
        {label}
        {hint !== undefined && <span className={styles.checkHint}>{hint}</span>}
      </span>
    </label>
  )
}

export type RadioGroupProps<T extends string> = {
  legend: string
  name: string
  value: T
  onChange: (value: T) => void
  options: ReadonlyArray<{ value: T; label: ReactNode; hint?: ReactNode; disabled?: boolean }>
  /**
   * The radios in a row, for short labels whose consequences the text *around* the group
   * explains (a reset's Soft / Mixed / Hard). The legend is then read by a screen reader only.
   */
  inline?: boolean | undefined
}

export function RadioGroup<T extends string>({
  legend,
  name,
  value,
  onChange,
  options,
  inline = false,
}: RadioGroupProps<T>): ReactElement {
  return (
    <fieldset
      className={cx(styles.group, inline && styles.groupInline)}
      role="radiogroup"
      aria-label={legend}
    >
      <legend className={cx(styles.legend, inline && styles.legendHidden)}>{legend}</legend>
      {options.map((o) => (
        <label
          key={o.value}
          className={styles.check}
          data-disabled={o.disabled === true || undefined}
          data-choice={o.value}
        >
          <input
            type="radio"
            name={name}
            value={o.value}
            checked={value === o.value}
            disabled={o.disabled}
            onChange={() => onChange(o.value)}
          />
          <span className={styles.checkText}>
            {o.label}
            {o.hint !== undefined && <span className={styles.checkHint}>{o.hint}</span>}
          </span>
        </label>
      ))}
    </fieldset>
  )
}

export type SwitchProps = {
  checked: boolean
  onChange: (checked: boolean) => void
  /** Required: a switch has no visible text of its own. */
  label: string
  disabled?: boolean | undefined
}

export function Switch({ checked, onChange, label, disabled }: SwitchProps): ReactElement {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      className={styles.switch}
      onClick={() => onChange(!checked)}
    >
      <span className={styles.knob} />
    </button>
  )
}

export type SegmentedProps<T extends string> = {
  label: string
  value: T
  onChange: (value: T) => void
  options: ReadonlyArray<{ value: T; label: string; icon?: IconName }>
  size?: ControlSize | undefined
  /** Stretch across the container, as the inbox's scope tabs do. */
  block?: boolean | undefined
}

export function Segmented<T extends string>({
  label,
  value,
  onChange,
  options,
  size = 'md',
  block = false,
}: SegmentedProps<T>): ReactElement {
  const move = (e: KeyboardEvent<HTMLDivElement>): void => {
    const step = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0
    if (step === 0) return
    e.preventDefault()
    const at = options.findIndex((o) => o.value === value)
    const next = options[(at + step + options.length) % options.length]
    if (next === undefined) return
    onChange(next.value)
    const buttons = e.currentTarget.querySelectorAll<HTMLButtonElement>('[role="radio"]')
    buttons[options.indexOf(next)]?.focus()
  }
  return (
    <div
      role="radiogroup"
      aria-label={label}
      data-size={size}
      className={cx(styles.segmented, block && styles.segmentedBlock)}
      onKeyDown={move}
    >
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={o.value === value}
          tabIndex={o.value === value ? 0 : -1}
          className={styles.segment}
          onClick={() => onChange(o.value)}
        >
          {o.icon !== undefined && <Icon name={o.icon} size={1} />}
          <span className={styles.steadyLabel} data-label={o.label}>
            {o.label}
          </span>
        </button>
      ))}
    </div>
  )
}

export type ChoiceCardsProps<T extends string> = {
  label: string
  value: T
  onChange: (value: T) => void
  options: ReadonlyArray<{ value: T; title: string; text: ReactNode; art: ReactNode }>
}

export function ChoiceCards<T extends string>({
  label,
  value,
  onChange,
  options,
}: ChoiceCardsProps<T>): ReactElement {
  return (
    <div role="radiogroup" aria-label={label} className={styles.cards}>
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={o.value === value}
          className={styles.card}
          onClick={() => onChange(o.value)}
        >
          <span className={styles.cardArt}>{o.art}</span>
          <span className={styles.cardTitle}>
            {o.title}
            {o.value === value && (
              <span className={styles.cardCheck}>
                <Icon name="check" size={0} />
              </span>
            )}
          </span>
          <span className={styles.cardText}>{o.text}</span>
        </button>
      ))}
    </div>
  )
}

export type ToggleCardProps = {
  title: string
  hint: ReactNode
  checked: boolean
  onChange: (checked: boolean) => void
  disabled?: boolean | undefined
}

export function ToggleCard({
  title,
  hint,
  checked,
  onChange,
  disabled,
}: ToggleCardProps): ReactElement {
  return (
    <label className={styles.toggleCard} data-disabled={disabled === true || undefined}>
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span>
        <span className={styles.toggleTitle}>{title}</span>
        <span className={styles.toggleHint}>{hint}</span>
      </span>
    </label>
  )
}
