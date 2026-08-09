/**
 * The Settings screen's form vocabulary.
 *
 * Presentational only: nothing here reads the store or the IPC surface, so the whole screen
 * can be rendered from a fixture. The mock draws exactly three kinds of control — a toggle
 * row, a segmented choice and a small numeric field — and this file is all of them.
 */
import type { ReactNode } from 'react'
import styles from './controls.module.css'

export interface ToggleProps {
  checked: boolean
  onChange: (next: boolean) => void
  /** The accessible name. Visible text lives in the row, so the control needs its own. */
  label: string
  disabled?: boolean | undefined
}

/**
 * The 34x19 pill with a 15px knob, `--accent` when on.
 *
 * A `button` with `role="switch"` rather than a styled checkbox: a checkbox brings its own
 * indeterminate state and a label association this layout does not use, and hiding one behind
 * a pill is how a control ends up unreachable from the keyboard.
 */
export function Toggle({ checked, onChange, label, disabled }: ToggleProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled === true}
      className={checked ? `${styles.pill} ${styles.pillOn}` : styles.pill}
      onClick={() => onChange(!checked)}
    >
      <span className={styles.knob} aria-hidden="true" />
    </button>
  )
}

export interface RowProps {
  label: string
  /** The italic second line under the label in the mock. */
  hint?: string | undefined
  control: ReactNode
}

/** One line of the settings form: text on the left, one control hard right. */
export function Row({ label, hint, control }: RowProps) {
  return (
    <div className={styles.row}>
      <div className={styles.rowText}>
        <div className={styles.label}>{label}</div>
        {hint !== undefined && <div className={styles.hint}>{hint}</div>}
      </div>
      <div className={styles.control}>{control}</div>
    </div>
  )
}

/** A toggle row: the pair the mock uses for every boolean. */
export function ToggleRow({
  label,
  hint,
  checked,
  onChange,
  disabled,
}: Omit<RowProps, 'control'> & Omit<ToggleProps, 'label'>) {
  return (
    <Row
      label={label}
      hint={hint}
      control={
        <Toggle label={label} checked={checked} onChange={onChange} disabled={disabled} />
      }
    />
  )
}

export interface SegmentedProps<T extends string> {
  value: T
  options: readonly { value: T; label: string }[]
  onChange: (next: T) => void
  label: string
}

/** A small exclusive choice — theme, terminal renderer. */
export function Segmented<T extends string>({
  value,
  options,
  onChange,
  label,
}: SegmentedProps<T>) {
  return (
    <div className={styles.segmented} role="radiogroup" aria-label={label}>
      {options.map((option) => {
        const active = option.value === value
        return (
          <button
            key={option.value}
            type="button"
            role="radio"
            aria-checked={active}
            className={active ? `${styles.segment} ${styles.segmentActive}` : styles.segment}
            onClick={() => onChange(option.value)}
          >
            {option.label}
          </button>
        )
      })}
    </div>
  )
}

export interface NumberFieldProps {
  value: number
  min: number
  max: number
  onChange: (next: number) => void
  label: string
}

/**
 * A bounded integer.
 *
 * Clamped here rather than trusted: these values reach a terminal's font metrics and a
 * scrollback allocation, and a pasted `999999` scrollback is a live session's memory. An
 * unparseable value is ignored rather than treated as zero, so deleting the contents to type
 * a new number does not momentarily set the setting to 0.
 */
export function NumberField({ value, min, max, onChange, label }: NumberFieldProps) {
  return (
    <input
      className={styles.number}
      type="number"
      inputMode="numeric"
      aria-label={label}
      min={min}
      max={max}
      value={value}
      onChange={(e) => {
        const next = Number.parseInt(e.target.value, 10)
        if (Number.isNaN(next)) return
        onChange(Math.min(max, Math.max(min, next)))
      }}
    />
  )
}

/** A titled block of rows. */
export function Group({ title, children }: { title?: string | undefined; children: ReactNode }) {
  return (
    <section className={styles.group}>
      {title !== undefined && <h3 className={styles.groupTitle}>{title}</h3>}
      {children}
    </section>
  )
}

/** A block of prose that explains something the controls cannot. */
export function Note({ title, children }: { title?: string | undefined; children: ReactNode }) {
  return (
    <div className={styles.note}>
      {title !== undefined && <div className={styles.noteTitle}>{title}</div>}
      {children}
    </div>
  )
}
