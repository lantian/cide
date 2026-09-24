/**
 * The kit's dropdown. See `Select.module.css` for why it is not a native `<select>`.
 *
 * Built on the WAI-ARIA "select-only combobox" pattern, because that is what the native control
 * gave a screen reader and a keyboard user for free, and dropping it would be the regression
 * that nobody sighted notices:
 *
 *   - focus never leaves the trigger; the active option is announced through
 *     `aria-activedescendant`, so Tab order and the focus ring stay where the field is;
 *   - closed: ArrowDown / ArrowUp / Enter / Space open it on the current value;
 *   - open: arrows move (skipping disabled options), Home / End jump, Enter / Space choose,
 *     Escape closes without choosing, Tab closes and moves on;
 *   - a printable key jumps to the next option starting with it, as the native one does;
 *   - a mouse-down outside closes it.
 *
 * The popup is portalled to `<body>` with `position: fixed`, measured from the trigger and
 * re-measured on scroll and resize, so no ancestor's `overflow` can clip it.
 */
import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type ReactElement,
} from 'react'
import { createPortal } from 'react-dom'

import { Icon, type IconName } from '@/icons/Icon'
import type { ControlSize } from './Button'
import { cx } from './cx'
import field from './Field.module.css'
import styles from './Select.module.css'

export type SelectOption = {
  value: string
  label: string
  icon?: IconName | undefined
  /** Secondary text at the right of the row: a count, a remote, a shortcut. */
  detail?: string | undefined
  disabled?: boolean | undefined
}

export type SelectProps = {
  value: string | null
  onChange: (value: string) => void
  options: readonly SelectOption[]
  size?: ControlSize | undefined
  placeholder?: string | undefined
  disabled?: boolean | undefined
  invalid?: boolean | undefined
  id?: string | undefined
  'aria-label'?: string | undefined
  'aria-describedby'?: string | undefined
}

/** Room the popup wants below the trigger before it prefers opening upward. */
const PREFERRED_ROOM = 220
const GAP = 4

export function Select({
  value,
  onChange,
  options,
  size = 'md',
  placeholder = 'Choose…',
  disabled = false,
  invalid = false,
  id,
  'aria-label': ariaLabel,
  'aria-describedby': describedBy,
}: SelectProps): ReactElement {
  const listId = useId()
  const trigger = useRef<HTMLButtonElement>(null)
  const popup = useRef<HTMLUListElement>(null)
  const [open, setOpen] = useState(false)
  const [active, setActive] = useState(-1)
  const [place, setPlace] = useState<CSSProperties>({})

  const selectedIndex = options.findIndex((o) => o.value === value)
  const selected = options[selectedIndex]

  const step = useCallback(
    (from: number, delta: number): number => {
      if (options.length === 0) return -1
      let i = from
      for (let n = 0; n < options.length; n++) {
        i = (i + delta + options.length) % options.length
        if (options[i]?.disabled !== true) return i
      }
      return from
    },
    [options],
  )

  const openAt = (index: number): void => {
    if (disabled) return
    setActive(index >= 0 && options[index]?.disabled !== true ? index : step(-1, 1))
    setOpen(true)
  }

  const close = (): void => {
    setOpen(false)
    setActive(-1)
  }

  const choose = (index: number): void => {
    const o = options[index]
    if (o === undefined || o.disabled === true) return
    onChange(o.value)
    close()
    trigger.current?.focus()
  }

  // Place the popup from the trigger's rectangle; again on any scroll or resize while open.
  useLayoutEffect(() => {
    if (!open) return
    const measure = (): void => {
      const r = trigger.current?.getBoundingClientRect()
      if (r === undefined) return
      const below = window.innerHeight - r.bottom
      const up = below < PREFERRED_ROOM && r.top > below
      setPlace({
        left: r.left,
        minWidth: r.width,
        ...(up ? { bottom: window.innerHeight - r.top + GAP } : { top: r.bottom + GAP }),
      })
    }
    measure()
    window.addEventListener('resize', measure)
    window.addEventListener('scroll', measure, true)
    return () => {
      window.removeEventListener('resize', measure)
      window.removeEventListener('scroll', measure, true)
    }
  }, [open])

  // A mouse-down anywhere but the trigger or the popup closes it.
  useEffect(() => {
    if (!open) return
    const away = (e: MouseEvent): void => {
      const t = e.target as Node
      if (trigger.current?.contains(t) === true || popup.current?.contains(t) === true) return
      close()
    }
    document.addEventListener('mousedown', away)
    return () => document.removeEventListener('mousedown', away)
  }, [open])

  // Keep the keyboard-active option in view in a long list.
  useEffect(() => {
    if (!open || active < 0) return
    popup.current
      ?.querySelector<HTMLElement>(`[data-index="${active}"]`)
      ?.scrollIntoView({ block: 'nearest' })
  }, [open, active])

  const onKey = (e: KeyboardEvent<HTMLButtonElement>): void => {
    if (!open) {
      if (['ArrowDown', 'ArrowUp', 'Enter', ' '].includes(e.key)) {
        e.preventDefault()
        openAt(selectedIndex)
      }
      return
    }
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault()
        setActive((a) => step(a, 1))
        return
      case 'ArrowUp':
        e.preventDefault()
        setActive((a) => step(a < 0 ? 0 : a, -1))
        return
      case 'Home':
        e.preventDefault()
        setActive(step(-1, 1))
        return
      case 'End':
        e.preventDefault()
        setActive(step(0, -1))
        return
      case 'Enter':
      case ' ':
        e.preventDefault()
        choose(active)
        return
      case 'Escape':
        e.preventDefault()
        close()
        return
      case 'Tab':
        close()
        return
      default:
        if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
          const ch = e.key.toLowerCase()
          for (let n = 1; n <= options.length; n++) {
            const i = (active + n + options.length) % options.length
            const o = options[i]
            if (o !== undefined && o.disabled !== true && o.label.toLowerCase().startsWith(ch)) {
              setActive(i)
              break
            }
          }
        }
    }
  }

  const iconSize = size === 'sm' ? 1 : 2
  return (
    <>
      <button
        ref={trigger}
        id={id}
        type="button"
        role="combobox"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        aria-activedescendant={open && active >= 0 ? `${listId}-${active}` : undefined}
        aria-label={ariaLabel}
        aria-describedby={describedBy}
        aria-invalid={invalid || undefined}
        disabled={disabled}
        data-invalid={invalid || undefined}
        data-disabled={disabled || undefined}
        className={cx(field.box, field[size], styles[size], styles.trigger)}
        onClick={() => (open ? close() : openAt(selectedIndex))}
        onKeyDown={onKey}
      >
        {selected !== undefined ? (
          <span className={styles.value}>
            {selected.icon !== undefined && <Icon name={selected.icon} size={iconSize} />}
            {selected.label}
          </span>
        ) : (
          <span className={styles.placeholder}>{placeholder}</span>
        )}
        <span className={styles.chevron}>
          <Icon name="chevron-down" size={1} />
        </span>
      </button>
      {open &&
        createPortal(
          <ul
            ref={popup}
            id={listId}
            role="listbox"
            aria-label={ariaLabel}
            className={styles.popup}
            style={place}
          >
            {options.map((o, i) => (
              <li
                key={o.value}
                id={`${listId}-${i}`}
                data-index={i}
                role="option"
                aria-selected={o.value === value}
                aria-disabled={o.disabled === true || undefined}
                data-active={i === active || undefined}
                className={styles.option}
                // Keep the focus on the trigger: a mouse-down on an option would otherwise
                // blur it, and the activedescendant pattern depends on the trigger holding it.
                onMouseDown={(e) => e.preventDefault()}
                onMouseEnter={() => {
                  if (o.disabled !== true) setActive(i)
                }}
                onClick={() => choose(i)}
              >
                <span className={styles.check}>
                  {o.value === value && <Icon name="check" size={1} />}
                </span>
                {o.icon !== undefined && <Icon name={o.icon} size={1} />}
                <span className={styles.optionLabel}>{o.label}</span>
                {o.detail !== undefined && <span className={styles.optionDetail}>{o.detail}</span>}
              </li>
            ))}
          </ul>,
          document.body,
        )}
    </>
  )
}
