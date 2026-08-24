/**
 * Drawing a contributed panel. (M22)
 *
 * Pure: reads no store, calls no IPC, never reads the clock. Every fact and every gesture arrives
 * as a prop, which is what lets `ui/scripts/check-ext-render.mjs` render it under node through
 * `react-dom/server` — the only gate in this project that can see a panel which compiles, mounts
 * and draws nothing.
 *
 * That split is `AgentsPanel`'s and is worth restating for this file specifically: the *content*
 * of a contributed panel comes from a worker, so there is no fixture cide controls and no other
 * way to be sure the renderer handles the union it claims to. The check feeds it one of each.
 *
 * Every optional handler follows the panel convention: **an absent handler means the control is
 * not drawn**, never drawn dead. A toolbar button that did nothing would be a button whose
 * extension looks broken.
 */
import { useMemo, useState } from 'react'

import { Icon, type IconName } from '@/icons/Icon'

import styles from './ExtPanel.module.css'
import {
  type NodeIcon,
  type NodeTone,
  type PanelView,
  type ViewRow,
  initialExpansion,
  visibleRows,
} from './viewModel'

export interface ExtPanelViewProps {
  /** What the rail or the tab calls this panel, when the view does not name itself. */
  readonly label: string
  readonly view: PanelView
  /** Where this is drawn. The bottom tool window fills its tab instead of owning a width. */
  readonly placement?: 'sidebar' | 'bottom'
  /** A toolbar button was pressed. Absent means the toolbar is not drawn. */
  readonly onAction?: ((id: string) => void) | undefined
  /** A row was activated. Absent means rows are not clickable, which is a real state. */
  readonly onActivate?: ((id: string) => void) | undefined
}

/**
 * The glyph for each icon name.
 *
 * Text and not SVG, and that is a decision rather than a shortcut. The rail's icons are 24x24
 * paths because they are chrome specified to the pixel; a row icon is a 14px column beside a
 * label, themeable by `color`, and — the point — **cannot be supplied by an extension**, because
 * this table is the only way to get one. That last clause is why the table survives the move to
 * drawn marks unchanged in shape: an extension still names a member of a closed set, and the set
 * is what decides what the name draws. What it no longer does is depend on the host's fonts —
 * `warning` and `info` were the ASCII stand-ins `!` and `i` precisely because no glyph could be
 * relied on, and they are real marks now.
 */
const MARK: Readonly<Record<NodeIcon, IconName | null>> = {
  none: null,
  file: 'file',
  folder: 'folder',
  symbol: 'diamond',
  error: 'circle-x',
  warning: 'triangle-alert',
  info: 'info',
  run: 'play',
  check: 'check',
}

const TONE: Readonly<Record<NodeTone, string | undefined>> = {
  normal: undefined,
  dim: styles.toneDim,
  accent: styles.toneAccent,
  error: styles.toneError,
  warning: styles.toneWarning,
}

export function ExtPanelView(props: ExtPanelViewProps): React.JSX.Element {
  const { label, view, placement = 'sidebar', onAction, onActivate } = props
  const body = view.body

  // Expansion is the view's, not the worker's: `ViewRow.expanded` only *seeds* it, so a worker
  // re-posting its view — which a good one does on every editor change — does not stamp on what
  // the user has opened and closed since. Keyed on the row ids rather than on the view object, so
  // a re-post with the same shape keeps the state and a genuinely different tree starts fresh.
  const identity = body.kind === 'tree' ? body.rows.map((row) => row.id).join(' ') : ''
  const seed = useMemo(
    () => (body.kind === 'tree' ? initialExpansion(body.rows) : []),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- the ids are the identity, see above
    [identity],
  )
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set(seed))
  const [seeded, setSeeded] = useState(seed)
  if (seeded !== seed) {
    setSeeded(seed)
    setExpanded(new Set(seed))
  }

  const className = placement === 'bottom' ? `${styles.panel} ${styles.docked}` : styles.panel

  return (
    <div className={className}>
      <div className={styles.header}>
        <span className={styles.title}>{view.title ?? label}</span>
        {onAction !== undefined && view.actions !== undefined && view.actions.length > 0 && (
          <span className={styles.actions}>
            {view.actions.map((action) => (
              <button
                key={action.id}
                type="button"
                className={styles.action}
                disabled={action.disabled !== undefined}
                title={action.disabled ?? action.label}
                onClick={() => onAction(action.id)}
              >
                {action.label}
              </button>
            ))}
          </span>
        )}
      </div>
      <div className={styles.body}>{renderBody()}</div>
    </div>
  )

  function renderBody(): React.JSX.Element {
    switch (body.kind) {
      case 'empty':
        return <p className={styles.notice}>{body.message}</p>
      case 'loading':
        return <p className={styles.notice}>{body.what}…</p>
      case 'failed':
        // An error, drawn as an error. Never an empty list: "this extension broke" and "there is
        // nothing here" are the two answers a user must not have to tell apart by guessing.
        return <p className={`${styles.notice} ${styles.failed}`}>{body.message}</p>
      case 'list':
        return rows(
          body.rows.map((row) => ({ row, depth: 0, hasChildren: false })),
          0,
        )
      case 'tree': {
        const { rows: flat, truncated } = visibleRows(body.rows, expanded)
        return rows(flat, truncated)
      }
      case 'table':
        return (
          <table className={styles.table}>
            <thead>
              <tr>
                {body.columns.map((column, at) => (
                  <th key={`${column}-${at}`}>{column}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {body.rows.map((row) => (
                <tr key={row.id}>
                  {body.columns.map((_, at) => (
                    <td key={at} title={row.cells[at] ?? ''}>
                      {row.cells[at] ?? ''}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        )
      case 'markdown':
        // `white-space: pre-wrap` and the text verbatim. Not the markdown pipeline, deliberately:
        // that renders links and HTML, and a panel whose content comes from a third party is the
        // one place in cide where rendering somebody else's markup would be a real hole. A
        // formatted variant is worth adding the day something needs it, through a renderer that
        // strips rather than escapes.
        return <div className={styles.markdown}>{body.text}</div>
    }
  }

  function rows(
    flat: readonly { row: ViewRow; depth: number; hasChildren: boolean }[],
    truncated: number,
  ): React.JSX.Element {
    if (flat.length === 0) {
      // A list that is genuinely empty. The worker should have posted `empty` with a sentence;
      // this is what it gets for not doing so, and it says which of the two happened.
      return <p className={styles.notice}>This extension returned no rows.</p>
    }
    return (
      <div className={styles.rows}>
        {flat.map(({ row, depth, hasChildren }) => {
          const open = expanded.has(row.id)
          const clickable = onActivate !== undefined || hasChildren
          return (
            <button
              key={row.id}
              type="button"
              className={`${styles.row} ${clickable ? styles.clickable : ''} ${
                TONE[row.tone ?? 'normal'] ?? ''
              }`}
              style={{ paddingLeft: `${10 + depth * 12}px` }}
              title={row.detail === undefined ? row.label : `${row.label} — ${row.detail}`}
              onClick={() => {
                // A parent toggles; a leaf activates. A parent that also activated would make
                // every expand a navigation, which is the file tree's rule and the one users
                // already have in their hands.
                if (hasChildren) {
                  setExpanded((prior) => {
                    const next = new Set(prior)
                    if (open) next.delete(row.id)
                    else next.add(row.id)
                    return next
                  })
                  return
                }
                onActivate?.(row.id)
              }}
            >
              <span className={styles.twisty}>
                {hasChildren ? (
                  <Icon name={open ? 'chevron-down' : 'chevron-right'} size={1} />
                ) : null}
              </span>
              <span className={styles.icon}>
                {(() => {
                  const mark = MARK[row.icon ?? 'none']
                  return mark === null ? null : <Icon name={mark} size={1} />
                })()}
              </span>
              <span className={styles.label}>{row.label}</span>
              {row.detail !== undefined && <span className={styles.detail}>{row.detail}</span>}
            </button>
          )
        })}
        {truncated > 0 && (
          // Said out loud. A silently truncated tree is an extension that looks like it lost data.
          <p className={styles.truncated}>
            {truncated} more row{truncated === 1 ? '' : 's'} not shown.
          </p>
        )}
      </div>
    )
  }
}
