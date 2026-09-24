/**
 * The kit's containers and rows. See `Surface.module.css` for the surface ladder and the radii.
 */
import type { HTMLAttributes, KeyboardEvent, ReactElement, ReactNode } from 'react'

import { Icon, type IconName } from '@/icons/Icon'
import styles from './Surface.module.css'

export function PanelHeader({
  title,
  aside,
  tools,
}: {
  title: string
  /** Next to the title: a `Counter`, a `Badge`. */
  aside?: ReactNode
  /** Right edge: `IconButton`s, at most three before they go into a `…` menu. */
  tools?: ReactNode
}): ReactElement {
  return (
    <header className={styles.panelHeader}>
      <h2 className={styles.panelTitle}>
        {title}
        {aside}
      </h2>
      {tools !== undefined && <div className={styles.tools}>{tools}</div>}
    </header>
  )
}

export function Heading({ title, lead }: { title: string; lead?: ReactNode }): ReactElement {
  return (
    <div className={styles.heading}>
      <h2 className={styles.title}>{title}</h2>
      {lead !== undefined && <p className={styles.lead}>{lead}</p>}
    </div>
  )
}

export function Section({
  caption,
  aside,
  children,
}: {
  caption: string
  aside?: ReactNode
  children: ReactNode
}): ReactElement {
  return (
    <section className={styles.section}>
      <h3 className={styles.caption}>
        {caption}
        {aside !== undefined && <span className={styles.captionAside}>{aside}</span>}
      </h3>
      {children}
    </section>
  )
}

export function Card({
  title,
  tone,
  draft = false,
  children,
}: {
  title?: ReactNode
  tone?: 'ok' | 'warn' | 'bad' | undefined
  draft?: boolean | undefined
  children: ReactNode
}): ReactElement {
  return (
    <div className={styles.card} data-tone={tone} data-draft={draft || undefined}>
      {title !== undefined && <div className={styles.cardHead}>{title}</div>}
      <div className={styles.cardBody}>{children}</div>
    </div>
  )
}

export function Summary({
  rows,
}: {
  rows: ReadonlyArray<{ label: string; value: ReactNode }>
}): ReactElement {
  return (
    <dl className={styles.summary}>
      {rows.map((r) => (
        <div key={r.label} className={styles.summaryRow}>
          <dt>{r.label}</dt>
          <dd>{r.value}</dd>
        </div>
      ))}
    </dl>
  )
}

export function Toolbar({
  label,
  children,
}: {
  /** What the tools act on, for a screen reader: "Changes", "Branches". */
  label?: string | undefined
  children: ReactNode
}): ReactElement {
  return (
    <div className={styles.toolbar} role="toolbar" aria-label={label}>
      {children}
    </div>
  )
}

export function ToolbarSeparator(): ReactElement {
  return <span className={styles.toolbarSep} aria-hidden />
}

export function ToolbarSpacer(): ReactElement {
  return <span className={styles.toolbarSpacer} aria-hidden />
}

export type TabsProps<T extends string> = {
  label: string
  value: T
  onChange: (value: T) => void
  tabs: ReadonlyArray<{ value: T; label: string; aside?: ReactNode }>
}

export function Tabs<T extends string>({
  label,
  value,
  onChange,
  tabs,
}: TabsProps<T>): ReactElement {
  const move = (e: KeyboardEvent<HTMLDivElement>): void => {
    const step = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0
    if (step === 0) return
    e.preventDefault()
    const at = tabs.findIndex((t) => t.value === value)
    const next = tabs[(at + step + tabs.length) % tabs.length]
    if (next === undefined) return
    onChange(next.value)
    e.currentTarget.querySelectorAll<HTMLButtonElement>('[role="tab"]')[tabs.indexOf(next)]?.focus()
  }
  return (
    <div role="tablist" aria-label={label} className={styles.tabs} onKeyDown={move}>
      {tabs.map((t) => (
        <button
          key={t.value}
          type="button"
          role="tab"
          aria-selected={t.value === value}
          tabIndex={t.value === value ? 0 : -1}
          className={styles.tab}
          onClick={() => onChange(t.value)}
        >
          <span className={styles.steadyLabel} data-label={t.label}>
            {t.label}
          </span>
          {t.aside}
        </button>
      ))}
    </div>
  )
}

/**
 * `listbox` for compact rows you select, `tree` for tree rows; leave it unset for `ListItem`s,
 * which are opened rather than selected.
 */
export function List({
  label,
  role,
  children,
}: {
  label: string
  role?: 'listbox' | 'tree' | undefined
  children: ReactNode
}): ReactElement {
  return (
    <ul className={styles.list} aria-label={label} role={role}>
      {children}
    </ul>
  )
}

export type ListItemProps = {
  title: ReactNode
  /** Small facts above the title: an id, a state badge. */
  top?: ReactNode
  /** Right end of the top line: a time, "Opened". */
  topAside?: ReactNode
  meta?: ReactNode
  /** Signals along the bottom: comment count, pipeline, diff stat. */
  foot?: ReactNode
  current?: boolean | undefined
  onOpen?: (() => void) | undefined
}

export function ListItem({
  title,
  top,
  topAside,
  meta,
  foot,
  current = false,
  onOpen,
}: ListItemProps): ReactElement {
  return (
    <li
      className={styles.item}
      aria-current={current || undefined}
      tabIndex={0}
      onClick={onOpen}
      onKeyDown={(e) => {
        if (e.key === 'Enter') onOpen?.()
      }}
    >
      {(top !== undefined || topAside !== undefined) && (
        <div className={styles.itemTop}>
          {top}
          {topAside !== undefined && <span className={styles.itemTopAside}>{topAside}</span>}
        </div>
      )}
      <div className={styles.itemTitle}>{title}</div>
      {meta !== undefined && <div className={styles.itemMeta}>{meta}</div>}
      {foot !== undefined && <div className={styles.itemFoot}>{foot}</div>}
    </li>
  )
}

export type RowProps = {
  icon?: IconName | undefined
  label: ReactNode
  detail?: ReactNode
  trailing?: ReactNode
  selected?: boolean | undefined
  onSelect?: (() => void) | undefined
  /** Tree rows only: nesting depth and whether the node is open. `undefined` → a leaf. */
  depth?: number | undefined
  expanded?: boolean | undefined
}

export function Row({
  icon,
  label,
  detail,
  trailing,
  selected = false,
  onSelect,
  depth,
  expanded,
}: RowProps): ReactElement {
  const tree = depth !== undefined
  return (
    <li
      role={tree ? 'treeitem' : 'option'}
      className={styles.row}
      style={tree ? { paddingLeft: `calc(${6 + depth * 12}px * var(--ui-scale))` } : undefined}
      aria-selected={selected}
      aria-expanded={tree ? expanded : undefined}
      tabIndex={selected ? 0 : -1}
      onClick={onSelect}
    >
      {tree && (
        <span className={styles.twisty}>
          {expanded !== undefined && (
            <Icon name={expanded ? 'chevron-down' : 'chevron-right'} size={0} />
          )}
        </span>
      )}
      {icon !== undefined && <Icon name={icon} size={1} />}
      <span className={styles.rowLabel}>{label}</span>
      {detail !== undefined && <span className={styles.rowDetail}>{detail}</span>}
      {trailing !== undefined && <span className={styles.rowTrail}>{trailing}</span>}
    </li>
  )
}

export function Breadcrumbs({
  trail,
  onPick,
}: {
  /** Root first; the last is the current place. */
  trail: readonly string[]
  onPick?: ((index: number) => void) | undefined
}): ReactElement {
  return (
    <nav aria-label="Breadcrumb">
      <ol className={styles.crumbs}>
        {trail.map((seg, i) => (
          <li key={`${i}-${seg}`}>
            {i > 0 && <Icon name="chevron-right" size={0} />}
            {i === trail.length - 1 ? (
              <span className={styles.crumbCurrent} aria-current="page">
                {seg}
              </span>
            ) : (
              <button type="button" className={styles.crumb} onClick={() => onPick?.(i)}>
                {seg}
              </button>
            )}
          </li>
        ))}
      </ol>
    </nav>
  )
}

export function Disclosure({
  title,
  aside,
  defaultOpen = false,
  children,
}: {
  title: string
  aside?: ReactNode
  defaultOpen?: boolean | undefined
  children: ReactNode
}): ReactElement {
  return (
    <details className={styles.disclosure} open={defaultOpen}>
      <summary>
        <Icon name="chevron-right" size={0} />
        {title}
        {aside !== undefined && <span className={styles.disclosureAside}>{aside}</span>}
      </summary>
      <div className={styles.disclosureBody}>{children}</div>
    </details>
  )
}

export function CodeBlock({ children }: { children: string }): ReactElement {
  return <pre className={styles.codeBlock}>{children}</pre>
}

export type Column<R> = {
  key: string
  label: string
  numeric?: boolean
  render: (row: R) => ReactNode
}

export function Table<R>({
  columns,
  rows,
  rowKey,
}: {
  columns: ReadonlyArray<Column<R>>
  rows: readonly R[]
  rowKey: (row: R) => string
}): ReactElement {
  return (
    <table className={styles.table}>
      <thead>
        <tr>
          {columns.map((c) => (
            <th key={c.key} data-numeric={c.numeric === true || undefined}>
              {c.label}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.map((r) => (
          <tr key={rowKey(r)}>
            {columns.map((c) => (
              <td key={c.key} data-numeric={c.numeric === true || undefined}>
                {c.render(r)}
              </td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  )
}

/**
 * What an act is about to touch, named one row each: the files a revert discards, the tabs a
 * close loses, the commits a pull brings in. Sits flush in a `Dialog` body.
 *
 * Every row is named, never counted — "12 files" is exactly what a user cannot check before
 * clicking — and the name is kept whole while the place beside it ellipsises first: a
 * truncated name is a row nobody can identify, a truncated directory is still a row.
 */
export function PathList({
  label,
  max = true,
  children,
  ...rest
}: Omit<HTMLAttributes<HTMLUListElement>, 'className' | 'children'> & {
  label?: string | undefined
  /** Cap the height (260 × scale) and scroll inside; `false` lets the dialog body scroll. */
  max?: boolean | undefined
  children: ReactNode
}): ReactElement {
  return (
    <ul {...rest} className={styles.pathList} aria-label={label} data-max={max || undefined}>
      {children}
    </ul>
  )
}

export type PathRowProps = Omit<HTMLAttributes<HTMLLIElement>, 'className' | 'children' | 'title'> & {
  /** The name: a file's basename, a tab's title, a commit's summary. Never truncated first. */
  name: ReactNode
  /** Where it is: a directory, a session state. Mono, `--faint`, ellipsises first. */
  where?: ReactNode
  /** A mark before the name — an `Icon`, a `Dot`. It carries the tone; the text stays neutral. */
  mark?: ReactNode
  /** The whole value, for the tooltip, when `where` may be cut. */
  full?: string | undefined
  /** Right end: the row's own answers (`Button size="sm"`s) or its state in words. */
  trailing?: ReactNode
}

export function PathRow({ name, where, mark, full, trailing, ...rest }: PathRowProps): ReactElement {
  return (
    <li {...rest} className={styles.pathRow} title={full}>
      {mark !== undefined && (
        <span className={styles.pathMark} aria-hidden="true">
          {mark}
        </span>
      )}
      <span className={styles.pathName}>{name}</span>
      {where !== undefined && where !== '' && <span className={styles.pathWhere}>{where}</span>}
      {trailing !== undefined && <span className={styles.pathTrail}>{trailing}</span>}
    </li>
  )
}

/** A caption between two groups of a `PathList` — "Unsaved files", "Sessions mid-turn". */
export function PathGroup({ children }: { children: ReactNode }): ReactElement {
  return <li className={styles.pathGroup}>{children}</li>
}
