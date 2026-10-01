/**
 * The kit's choice controls. `Choice.module.css` says which one a choice calls for.
 *
 * `Switch` and `Segmented` are ARIA widgets (`role="switch"`, `role="radiogroup"`) over plain
 * buttons, because nothing native draws either. `Segmented` moves with the arrow keys, as a
 * radio group must — a segmented control that only answers Tab is a row of buttons pretending.
 */
import { useEffect, useRef, useState, type KeyboardEvent, type ReactElement, type ReactNode } from 'react'

import { Icon, type IconName } from '@/icons/Icon'
import type { ControlSize } from './Button'
import { cx } from './cx'
import { TextInput } from './Field'
import styles from './Choice.module.css'

export type CheckboxProps = {
  label: ReactNode
  hint?: ReactNode
  checked: boolean
  onChange: (checked: boolean) => void
  disabled?: boolean | undefined
  indeterminate?: boolean | undefined
}

/** Image/paragraph choices with native radios or checkboxes and a separate preview target. */
export function VisualChoices({ label, name, multiple, values, options, disabled, onChange }: {
  label: string
  name: string
  multiple: boolean
  values: readonly string[]
  options: readonly { id: string; title: string; description?: string; art?: ReactNode }[]
  disabled?: boolean | undefined
  onChange: (values: string[]) => void
}): ReactElement {
  return (
    <fieldset className={styles.visualChoices} aria-label={label}>
      <legend className={styles.legend}>{label}</legend>
      <div className={styles.visualGrid}>
        {options.map((option) => (
          <div key={option.id} className={styles.visualCard} data-selected={values.includes(option.id)}>
            {option.art !== undefined && <div className={styles.visualArt}>{option.art}</div>}
            <label className={styles.check}>
              <input type={multiple ? 'checkbox' : 'radio'} name={name} value={option.id}
                checked={values.includes(option.id)} disabled={disabled}
                onChange={(e) => onChange(multiple
                  ? e.target.checked ? [...values, option.id] : values.filter((id) => id !== option.id)
                  : [option.id])} />
              <span className={styles.checkText}>
                <strong>{option.title}</strong>
                {option.description !== undefined && <span className={styles.checkHint}>{option.description}</span>}
              </span>
            </label>
          </div>
        ))}
      </div>
    </fieldset>
  )
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

export type ColorSwatchesProps = {
  label: string
  /** The chosen colour as `#rrggbb`, or `null` for the preset whose `value` is `null` (a default). */
  value: string | null
  onChange: (value: string | null) => void
  /** 4–10 hand-picked colours. At most one may be `null`: the default, drawn in `defaultColor`. */
  presets: ReadonlyArray<{ value: string | null; label: string }>
  /** What the `null` preset looks like. */
  defaultColor?: string | undefined
  /** Why the last pick was refused, under the row. */
  error?: string | null | undefined
}

const HEX = /^#[0-9a-f]{6}$/

/**
 * One colour: a row of preset swatches, a Custom swatch that opens the system colour dialog, and
 * a hex field. The accent picker in Settings → Appearance is the first user.
 *
 * A radio group over buttons, like `Segmented` (the arrow keys move through the presets). The
 * Custom swatch is a `<label>` over a visually hidden `<input type="color">`: the native dialog is
 * the only full picker WebKitGTK offers, and drawing a second one would be a colour wheel to
 * maintain for one setting.
 *
 * **It commits on the input's `change`, not React's `onChange`.** React maps `onChange` to the
 * DOM's `input` event, which a colour dialog fires on every pointer move while its wheel is
 * dragged; the caller's `onChange` is a settings write, and hundreds of them per drag is a
 * workspace snapshot per frame in every window. The native `change` fires once, when the dialog
 * closes. The hex field commits on Enter or blur, for the same reason.
 */
export function ColorSwatches({
  label,
  value,
  onChange,
  presets,
  defaultColor,
  error,
}: ColorSwatchesProps): ReactElement {
  const normal = value?.toLowerCase() ?? null
  const isPreset = presets.some((p) => p.value?.toLowerCase() === normal)
  const [draft, setDraft] = useState(normal ?? '')
  useEffect(() => setDraft(normal ?? ''), [normal])
  const native = useRef<HTMLInputElement>(null)
  // Held in a ref so the `change` listener below is attached once and still calls the newest
  // `onChange`.
  const commit = useRef(onChange)
  commit.current = onChange
  useEffect(() => {
    const input = native.current
    if (input === null) return
    const done = (): void => commit.current(input.value.toLowerCase())
    input.addEventListener('change', done)
    return () => input.removeEventListener('change', done)
    // On `normal` because the input is re-keyed by it: a new element needs the listener again.
  }, [normal])

  const commitDraft = (): void => {
    const hex = (draft.startsWith('#') ? draft : `#${draft}`).trim().toLowerCase()
    if (hex === (normal ?? '')) return
    if (HEX.test(hex)) onChange(hex)
    else setDraft(normal ?? '')
  }

  const move = (e: KeyboardEvent<HTMLDivElement>): void => {
    const step = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0
    if (step === 0) return
    e.preventDefault()
    const at = presets.findIndex((p) => p.value?.toLowerCase() === normal)
    const next = presets[(at + step + presets.length) % presets.length]
    if (next === undefined) return
    onChange(next.value)
    const buttons = e.currentTarget.querySelectorAll<HTMLButtonElement>('[role="radio"]')
    buttons[presets.indexOf(next)]?.focus()
  }

  const custom = !isPreset && normal !== null
  return (
    <div className={styles.swatchField}>
      <div className={styles.swatchRow}>
        <div role="radiogroup" aria-label={label} className={styles.swatches} onKeyDown={move}>
          {presets.map((p) => {
            const on = p.value?.toLowerCase() === normal
            const paint = p.value ?? defaultColor
            return (
              <button
                key={p.value ?? 'default'}
                type="button"
                role="radio"
                aria-checked={on}
                aria-label={p.label}
                title={p.label}
                tabIndex={on || (!isPreset && p === presets[0]) ? 0 : -1}
                className={styles.swatch}
                style={paint === undefined ? undefined : { background: paint }}
                onClick={() => onChange(p.value)}
              >
                {on && <Icon name="check" size={0} />}
              </button>
            )
          })}
        </div>
        <label
          className={cx(styles.swatch, styles.swatchCustom)}
          data-on={custom || undefined}
          title="Custom colour…"
          style={custom ? { background: normal } : undefined}
        >
          <input
            ref={native}
            type="color"
            className={styles.swatchNative}
            aria-label="Custom colour…"
            // Uncontrolled on purpose: a controlled value would be reset by React on every
            // render while the dialog is open. `defaultValue` plus the `key` below re-seeds it
            // when the stored colour moves.
            key={normal ?? 'none'}
            defaultValue={normal ?? defaultColor ?? '#000000'}
          />
          <Icon name={custom ? 'check' : 'plus'} size={0} />
        </label>
        <span className={styles.swatchHex}>
          <TextInput
            size="sm"
            mono
            aria-label={`${label}, hex`}
            placeholder="#rrggbb"
            value={draft}
            invalid={error !== undefined && error !== null && error !== ''}
            onChange={(e) => setDraft(e.target.value)}
            onBlur={commitDraft}
            onKeyDown={(e) => {
              if (e.key === 'Enter') commitDraft()
              if (e.key === 'Escape') setDraft(normal ?? '')
            }}
          />
        </span>
      </div>
      {error !== undefined && error !== null && error !== '' && (
        <p className={styles.swatchError} role="alert">
          {error}
        </p>
      )}
    </div>
  )
}
