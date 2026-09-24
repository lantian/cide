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
/** A heading over one part of a section that covers several subjects. (M93) */
export function Part({ title }: { title: string }) {
  return <h2 className={styles.part}>{title}</h2>
}

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

/**
 * A value that is read, not set, in a row's control slot — the app's own version is the first.
 *
 * `PathReadout`'s sibling: the same mono, the same dim, and selectable for the same reason —
 * the point of printing a version next to the log directory is pasting it into a report — but
 * inline, so it sits where a row's control goes rather than on a line of its own underneath.
 */
export function Readout({ text }: { text: string }) {
  return <span className={styles.readout}>{text}</span>
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

/**
 * A [`TextField`] for text that is a paragraph rather than a token.
 *
 * # Why this exists beside `TextField` rather than as a prop on it
 *
 * The two differ in one thing that is not styling: **what Enter means.** `TextField` commits on
 * Enter, which is right for an id, a URL or a key — there is one line and pressing Enter means
 * "that is the value". Here Enter is a newline, because the thing being edited is prose, and a
 * control that committed on it could not hold a second sentence. Making that a `multiline` flag
 * would put a branch on the one keystroke the two disagree about, in the one place a reader
 * would not think to look for it.
 *
 * So: **commits on blur and on Escape only**, and Escape reverts the draft exactly as
 * `TextField`'s does. Blur-commit is not a style choice — `TextField`'s header argues it — and
 * it matters more here, because this control's one caller writes a **file in the user's
 * repository** on every commit.
 *
 * # Newlines
 *
 * It does not strip them, deliberately. What a newline *means* is the caller's question, and for
 * the spinner's prompt Rust already answers it: `AgentsConfig::apply` flattens the string before
 * it reaches `.cide/config.json`, because the prompt is typed at a terminal where a newline is a
 * second Enter. A control that flattened on its own would be a second implementation of that
 * rule, in the realm that cannot see why it is needed — and the box would fight the person as
 * they typed. Here they type freely, the file holds one line, and the box shows what the file
 * holds after the round trip.
 */
export function TextArea({
  label,
  hint,
  value,
  placeholder,
  rows,
  onCommit,
}: {
  label: string
  hint?: ReactNode
  value: string
  placeholder?: string
  rows?: number
  onCommit: (next: string) => void
}) {
  const [draft, setDraft] = useState<string | null>(null)
  const commit = (text: string) => {
    setDraft(null)
    const next = text.trim()
    // Guarded, so tabbing through an untouched field is not a write and a broadcast —
    // `TextField`'s rule, and here the write is a file in the user's repository.
    if (next !== value) onCommit(next)
  }
  return (
    <label className={styles.field}>
      <span className={styles.fieldLabel}>{label}</span>
      <textarea
        className={styles.fieldArea}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        rows={rows ?? 5}
        placeholder={placeholder ?? ''}
        value={draft ?? value}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={(e) => commit(e.target.value)}
        onKeyDown={(e) => {
          // No Enter arm: see the header. Escape throws the draft away, which is the only way
          // back out of a half-typed paragraph that does not involve retyping the old one.
          if (e.key === 'Escape') setDraft(null)
        }}
      />
      {hint !== undefined && <span className={styles.fieldHint}>{hint}</span>}
    </label>
  )
}

/**
 * A stacked label + input + hint that commits on blur, Enter or Escape.
 *
 * Promoted out of `ClaudeCliSection` in M45 — this was its third caller, and this file's header
 * says it *is* the screen's form vocabulary. Blur-commit is not a style choice: every keystroke
 * would otherwise be an IPC round trip, a `workspace.json` rewrite and a broadcast to every
 * window, and for a credential field a per-keystroke write of a half-typed secret.
 *
 * `trim` exists for exactly one caller and is the reason this is a prop rather than a constant.
 * Trimming is right for an id, a URL and a package name, which is why it is the default. It is
 * **wrong for a credential**: whitespace in a key is the user's to see, and silently editing one
 * is how a key that works in a terminal stops working in cide — the rule
 * `cide_ipc::LlmSettings::cleaned` states on the other side of the wire, where `api_key` is the
 * one field it will not touch.
 */
export function TextField({
  label,
  hint,
  value,
  placeholder,
  trim = true,
  onCommit,
}: {
  label: string
  hint?: ReactNode
  value: string
  placeholder?: string
  trim?: boolean
  onCommit: (next: string) => void
}) {
  const [draft, setDraft] = useState<string | null>(null)
  const commit = (text: string) => {
    setDraft(null)
    const next = trim ? text.trim() : text
    // Guarded, so tabbing through an untouched field is not a write and a broadcast.
    if (next !== value) onCommit(next)
  }
  return (
    <label className={styles.field}>
      <span className={styles.fieldLabel}>{label}</span>
      <input
        className={styles.fieldInput}
        type="text"
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        placeholder={placeholder ?? ''}
        value={draft ?? value}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={(e) => commit(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') commit(e.currentTarget.value)
          else if (e.key === 'Escape') setDraft(null)
        }}
      />
      {hint !== undefined && <span className={styles.fieldHint}>{hint}</span>}
    </label>
  )
}
