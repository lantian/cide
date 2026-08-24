/**
 * The Settings screen's form vocabulary.
 *
 * Presentational only: nothing here reads the store or the IPC surface, so the whole screen
 * can be rendered from a fixture. The mock draws exactly three kinds of control — a toggle
 * row, a segmented choice and a small numeric field — and this file is all of them.
 */
import { useState, type ReactNode } from 'react'
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

export interface SelectProps<T extends string> {
  value: T
  options: readonly { value: T; label: string }[]
  onChange: (next: T) => void
  label: string
}

/**
 * An exclusive choice with too many options to be a `Segmented`. (M24)
 *
 * The line between the two is how many rows there are and whether the set is closed. Theme is
 * two fixed values and stays segmented; a colour scheme is `cide` plus however many the user has
 * imported, which is unbounded and could be one or twenty.
 *
 * A native `<select>` rather than a listbox built out of divs. It gets keyboard behaviour,
 * typeahead, screen-reader semantics and the platform's own popup for free — and the popup is
 * the part worth having: a custom one inside a webview cannot escape the window, so a long list
 * near the bottom of the Settings tab would scroll inside a panel instead of opening over it.
 * The arrow is drawn with `appearance: none` and a background image so the control matches the
 * rest of the form; everything else is the browser's.
 */
export function Select<T extends string>({ value, options, onChange, label }: SelectProps<T>) {
  return (
    <select
      className={styles.select}
      aria-label={label}
      value={value}
      onChange={(e) => onChange(e.target.value as T)}
    >
      {options.map((option) => (
        <option key={option.value} value={option.value}>
          {option.label}
        </option>
      ))}
    </select>
  )
}

export interface NumberFieldProps {
  value: number
  min: number
  max: number
  /** Spinner increment. Omitted, the browser uses 1, which cannot express a half-pixel size. */
  step?: number | undefined
  onChange: (next: number) => void
  label: string
}

/**
 * A bounded number.
 *
 * Clamped, because these values reach a terminal's font metrics and a scrollback allocation
 * and a pasted `999999` scrollback is a live session's memory — but clamped **on commit, not
 * on every keystroke**. Clamping per keystroke reads as correct and makes the field
 * unenterable: with `min=8`, typing `16` clamps the intermediate `1` to `8`, the controlled
 * input snaps to "8", and the next keystroke produces "86". Every numeric setting on this
 * screen behaves that way, so the half-typed value is held locally as text and only parsed
 * when the user is finished with it.
 *
 * An unparseable commit is ignored rather than treated as zero: clearing the field to type a
 * new number must not set a scrollback of 0 on the way past.
 */
export function NumberField({ value, min, max, step, onChange, label }: NumberFieldProps) {
  /** The text being typed, or `null` when the field is showing the stored value. */
  const [draft, setDraft] = useState<string | null>(null)

  const commit = (text: string) => {
    setDraft(null)
    // `parseFloat`, not `parseInt`. The font sizes default to the mock's 12.5px, and
    // `parseInt` truncated every commit to an integer — so the field showed 12.5, and the
    // moment it was touched it stored 12 and could never get back.
    const next = Number.parseFloat(text)
    if (Number.isNaN(next)) return
    const clamped = Math.min(max, Math.max(min, next))
    // Guarded so that tabbing through an untouched field is not a workspace write and a
    // broadcast to every window.
    if (clamped !== value) onChange(clamped)
  }

  return (
    <input
      className={styles.number}
      type="number"
      // `decimal`, not `numeric`: a phone keypad without a decimal point cannot type 12.5.
      inputMode="decimal"
      aria-label={label}
      min={min}
      max={max}
      // Absent, the browser's default step is 1 and the spinner arrows snap a 12.5 to 13.
      {...(step === undefined ? {} : { step })}
      value={draft ?? String(value)}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={(e) => commit(e.target.value)}
      onKeyDown={(e) => {
        // Enter commits without waiting for focus to leave; Escape abandons the draft, which
        // re-renders the stored value.
        if (e.key === 'Enter') commit(e.currentTarget.value)
        else if (e.key === 'Escape') setDraft(null)
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

/**
 * A block of prose that explains something the controls cannot.
 *
 * `tone="warn"` recolours the left rule only. A whole yellow panel would out-shout the
 * section it sits in, and the one warning this screen carries — an unverified `claude` — is a
 * thing to notice on the way past rather than an error to stop at.
 */
export function Note({
  title,
  tone = 'info',
  children,
}: {
  title?: string | undefined
  tone?: 'info' | 'warn' | undefined
  children: ReactNode
}) {
  return (
    <div className={tone === 'warn' ? `${styles.note} ${styles.noteWarn}` : styles.note}>
      {title !== undefined && <div className={styles.noteTitle}>{title}</div>}
      {children}
    </div>
  )
}

/**
 * A filesystem path shown under an action row.
 *
 * Selectable and monospaced on purpose. The whole reason to print the path next to a button
 * that opens it is the machine where the button does nothing — a desktop with no file manager
 * registered for `inode/directory` — and on that machine the only useful thing is a path the
 * user can copy into a terminal.
 */
export function PathReadout({ path }: { path: string }) {
  return <div className={styles.pathReadout}>{path}</div>
}

export interface ActionButtonProps {
  label: string
  onClick: () => void
  disabled?: boolean | undefined
}

/**
 * A small secondary button, for a row whose control is an action rather than a value.
 *
 * The mock has no button in Settings because the mock has no action rows; "open the log
 * directory" is the first. Drawn as a quieter sibling of `Segmented` — same height, same
 * border, same radius — so it reads as part of the same form rather than as something
 * borrowed from elsewhere in the app.
 */
export function ActionButton({ label, onClick, disabled }: ActionButtonProps) {
  return (
    <button
      type="button"
      className={styles.action}
      disabled={disabled === true}
      onClick={onClick}
    >
      {label}
    </button>
  )
}
