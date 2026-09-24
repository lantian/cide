/**
 * The kit's overlays. See `Overlay.module.css`.
 *
 * These render the *surface* only — where it sits on screen (the scrim, anchoring a menu to its
 * button, dismissing on Escape) is the caller's, as it is in the app today with
 * `overlays/OverlayCard.tsx`. That is also what lets the kit page show them inline.
 */
import type { HTMLAttributes, ReactElement, ReactNode, Ref } from 'react'

import { Icon, type IconName } from '@/icons/Icon'
import { IconButton } from './Button'
import { cx } from './cx'
import styles from './Overlay.module.css'

/** Everything a `<div>` takes except what the component owns — `data-audit`, `onKeyDown`, … */
type DivAttrs = Omit<HTMLAttributes<HTMLDivElement>, 'className' | 'title' | 'children' | 'role'>

/**
 * The dimmed layer under a dialog or picker. Clicking it is the caller's to decide.
 *
 * Only a press on the scrim itself dismisses, never one that bubbled up from the dialog: a drag
 * that starts in a text field and ends outside it would otherwise close the dialog it was typed
 * into. Extra attributes (the app's `data-audit` hooks) land on the layer.
 */
export function Scrim({
  children,
  onDismiss,
  ...rest
}: DivAttrs & {
  children: ReactNode
  onDismiss?: (() => void) | undefined
}): ReactElement {
  return (
    <div
      {...rest}
      className={styles.scrim}
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onDismiss?.()
      }}
    >
      {children}
    </div>
  )
}

/** `narrow` 420 for a one-question confirm, `default` 520, `picker` 620, `wide` 880. */
export type DialogWidth = 'narrow' | 'default' | 'picker' | 'wide'

export type DialogProps = DivAttrs & {
  title: string
  /** Beside the title, right-aligned: which question this is ("2 of 7"), a `Badge`. */
  titleAside?: ReactNode
  lead?: ReactNode
  /**
   * More of the head, under the lead: a mode picker the body is *about* (a reset's soft/mixed/
   * hard), which has to be read before the body and so cannot live in it.
   */
  head?: ReactNode
  children?: ReactNode
  /**
   * The body without its padding, for a list that runs edge to edge (a file list, a picker's
   * rows). The body scrolls either way once the dialog reaches the window's height.
   */
  flush?: boolean | undefined
  /**
   * A strip between the body and the footer that does not scroll with the body: provenance
   * that is the same for every page of a long output (who ran a log line, on what model).
   */
  below?: ReactNode
  /** Left of the footer: what pressing the primary will do, or why it is disabled. */
  footNote?: ReactNode
  /** Right of the footer, primary last. Absent means no footer at all (a viewer, a log). */
  actions?: ReactNode
  onClose?: (() => void) | undefined
  width?: DialogWidth | undefined
  /** @deprecated spelling of `width="wide"`, kept for the specimens. */
  wide?: boolean | undefined
}

export function Dialog({
  title,
  titleAside,
  lead,
  head,
  children,
  flush = false,
  below,
  footNote,
  actions,
  onClose,
  width,
  wide = false,
  ...rest
}: DialogProps): ReactElement {
  const w = width ?? (wide ? 'wide' : 'default')
  return (
    <div
      {...rest}
      className={styles.dialog}
      role="dialog"
      aria-modal="true"
      aria-label={title}
      data-width={w === 'default' ? undefined : w}
    >
      <div className={styles.dialogHead}>
        <div className={styles.dialogHeadText}>
          <div className={styles.dialogTitleRow}>
            <h2 className={styles.dialogTitle}>{title}</h2>
            {titleAside !== undefined && <span className={styles.titleAside}>{titleAside}</span>}
          </div>
          {lead !== undefined && <p className={styles.dialogLead}>{lead}</p>}
          {head}
        </div>
        {onClose !== undefined && <IconButton icon="x" label="Close" onClick={onClose} />}
      </div>
      {children !== undefined && (
        <div className={cx(styles.dialogBody, flush && styles.dialogBodyFlush)}>{children}</div>
      )}
      {below !== undefined && <div className={styles.dialogBelow}>{below}</div>}
      {actions !== undefined && (
        <div className={styles.dialogFoot}>
          <span className={styles.footNote}>{footNote}</span>
          {actions}
        </div>
      )}
    </div>
  )
}

export type StepState = 'done' | 'current' | 'ahead'

export function Stepper({
  steps,
  current,
}: {
  steps: readonly string[]
  /** Index of the current step. */
  current: number
}): ReactElement {
  return (
    <ol className={styles.stepper}>
      {steps.map((name, i) => {
        const state: StepState = i < current ? 'done' : i === current ? 'current' : 'ahead'
        return (
          <li
            key={name}
            className={styles.step}
            data-state={state}
            aria-current={state === 'current' ? 'step' : undefined}
          >
            <span className={styles.stepDot}>
              {state === 'done' ? <Icon name="check" size={0} /> : i + 1}
            </span>
            <span className={styles.stepName}>{name}</span>
          </li>
        )
      })}
    </ol>
  )
}

/** The New Project wizard's frame: a rail with brand and steps, the step, a footer. */
export function Wizard({
  brand,
  brandIcon,
  steps,
  current,
  railNote,
  children,
  footNote,
  actions,
}: {
  brand: string
  brandIcon: IconName
  steps: readonly string[]
  current: number
  railNote?: ReactNode
  children: ReactNode
  footNote?: ReactNode
  actions: ReactNode
}): ReactElement {
  return (
    <div className={styles.dialog} data-width="wide" role="dialog" aria-label={brand}>
      <div className={styles.wizard}>
        <div className={styles.rail}>
          <div className={styles.brand}>
            <span className={styles.brandMark}>
              <Icon name={brandIcon} size={2} />
            </span>
            <span className={styles.brandText}>{brand}</span>
          </div>
          <Stepper steps={steps} current={current} />
          {railNote !== undefined && <p className={styles.railNote}>{railNote}</p>}
        </div>
        <div className={styles.wizardMain}>
          <div className={styles.wizardBody}>{children}</div>
          <div className={styles.dialogFoot}>
            <span className={styles.footNote}>{footNote}</span>
            {actions}
          </div>
        </div>
      </div>
    </div>
  )
}

/**
 * The picker, in parts, so a virtualised list can use it. `PickerFrame` is the box;
 * `PickerInput` is the 44px row the query is typed into (it holds the caller's own `<input>`,
 * which may draw its own caret); `PickerList` is the scroll container a virtualiser measures;
 * `PickerRow` one choice; `PickerFoot` the key hints, each a `PickerHint`. `Picker` below is
 * the whole thing assembled, for a short static list.
 */
export function PickerFrame({
  label,
  narrow = false,
  children,
  ...rest
}: DivAttrs & {
  label: string
  /** 340 wide, for a one-field popup (go to line, a scratch type) instead of 620. */
  narrow?: boolean | undefined
  children: ReactNode
}): ReactElement {
  return (
    <div
      {...rest}
      className={styles.picker}
      data-width={narrow ? 'narrow' : undefined}
      role="dialog"
      aria-modal="true"
      aria-label={label}
    >
      {children}
    </div>
  )
}

export function PickerInput({
  lead,
  trailing,
  children,
}: {
  /** The mark before the field: the search glass, a mode's sigil (`>`, `#`). */
  lead?: ReactNode
  /** Right of the field: a match counter. Mono, `--faint`. */
  trailing?: ReactNode
  /** The field itself. A bare `<input>` child is styled; anything else brings its own. */
  children: ReactNode
}): ReactElement {
  return (
    <div className={styles.pickerInput}>
      {lead !== undefined && <span className={styles.pickerLead}>{lead}</span>}
      {children}
      {trailing !== undefined && <span className={styles.pickerCount}>{trailing}</span>}
    </div>
  )
}

export function PickerList({
  children,
  ...rest
}: DivAttrs & { children: ReactNode; ref?: Ref<HTMLDivElement> | undefined }): ReactElement {
  return (
    <div {...rest} className={styles.pickerList} role="listbox">
      {children}
    </div>
  )
}

export function PickerRow({
  selected = false,
  disabled = false,
  virtual = false,
  children,
  ...rest
}: DivAttrs & {
  selected?: boolean | undefined
  /** Shown, not chosen: `--faint`, and the reason is the caller's to put in the row. */
  disabled?: boolean | undefined
  /** Positioned by a virtualiser: absolute at the top of the list, moved by `style.transform`. */
  virtual?: boolean | undefined
  children: ReactNode
  /** A disabled row's reason, as its tooltip. */
  title?: string | undefined
  ref?: Ref<HTMLDivElement> | undefined
}): ReactElement {
  return (
    <div
      {...rest}
      role="option"
      aria-selected={selected}
      aria-disabled={disabled || undefined}
      className={cx(styles.pickerRow, virtual && styles.pickerRowVirtual)}
    >
      {children}
    </div>
  )
}

/** The characters of a label the query matched. */
export function PickerMatch({ children }: { children: ReactNode }): ReactElement {
  return <span className={styles.match}>{children}</span>
}

/** A plain line in the list's place: "No matching commands", "Indexing symbols…". */
export function PickerStatus({ children, ...rest }: DivAttrs & { children: ReactNode }): ReactElement {
  return (
    <div {...rest} className={styles.pickerStatus} role="status">
      {children}
    </div>
  )
}

export function PickerFoot({ children }: { children: ReactNode }): ReactElement {
  return <div className={styles.pickerFoot}>{children}</div>
}

/** One `⏎ open` hint in a `PickerFoot`. */
export function PickerHint({ keys, children }: { keys: string; children: ReactNode }): ReactElement {
  return (
    <span className={styles.pickerHint}>
      <span className={styles.pickerHintKey}>{keys}</span>
      {children}
    </span>
  )
}

export type PickerItem = {
  id: string
  icon: IconName
  /** The label split around the matched run: `[before, match, after]`. */
  label: readonly [string, string, string]
  detail?: string
  trailing?: ReactNode
}

/** The whole picker for a short list that needs no virtualiser. */
export function Picker({
  placeholder,
  query,
  rows,
  selected,
  foot,
}: {
  placeholder: string
  query: string
  rows: readonly PickerItem[]
  selected: string
  foot?: ReactNode
}): ReactElement {
  return (
    <PickerFrame label={placeholder}>
      <PickerInput lead={<Icon name="search" size={2} />}>
        <input placeholder={placeholder} defaultValue={query} aria-label={placeholder} />
      </PickerInput>
      <PickerList>
        {rows.map((r) => (
          <PickerRow key={r.id} selected={r.id === selected}>
            <Icon name={r.icon} size={1} />
            <span>
              {r.label[0]}
              <PickerMatch>{r.label[1]}</PickerMatch>
              {r.label[2]}
            </span>
            <span className={styles.pickerDetail}>{r.detail}</span>
            {r.trailing}
          </PickerRow>
        ))}
      </PickerList>
      {foot !== undefined && <PickerFoot>{foot}</PickerFoot>}
    </PickerFrame>
  )
}

export type MenuEntry =
  | {
      kind: 'item'
      label: string
      icon?: IconName
      shortcut?: string
      danger?: boolean
      disabled?: boolean
    }
  | { kind: 'separator' }
  | { kind: 'group'; label: string }

export function Menu({ label, entries }: { label: string; entries: readonly MenuEntry[] }): ReactElement {
  return (
    <ul className={styles.menu} role="menu" aria-label={label}>
      {entries.map((e, i) => {
        if (e.kind === 'separator') {
          return <li key={`sep-${i}`} role="separator" className={styles.menuSep} />
        }
        if (e.kind === 'group') {
          return (
            <li key={`group-${e.label}`} role="presentation" className={styles.menuGroup}>
              {e.label}
            </li>
          )
        }
        return (
          <li key={e.label} role="none">
            <button
              type="button"
              role="menuitem"
              className={styles.menuItem}
              disabled={e.disabled}
              data-danger={e.danger === true || undefined}
            >
              {e.icon !== undefined ? (
                <Icon name={e.icon} size={1} />
              ) : (
                <span className={styles.menuIconSlot} aria-hidden />
              )}
              <span className={styles.menuLabel}>{e.label}</span>
              {e.shortcut !== undefined && <span className={styles.menuShortcut}>{e.shortcut}</span>}
            </button>
          </li>
        )
      })}
    </ul>
  )
}

export function Tooltip({ text, shortcut }: { text: string; shortcut?: string | undefined }): ReactElement {
  return (
    <span className={styles.tooltip} role="tooltip">
      {text}
      {shortcut !== undefined && <span className={styles.tooltipKey}>{shortcut}</span>}
    </span>
  )
}
