/**
 * The Extensions panel's view. (M22)
 *
 * Pure: reads no store, calls no IPC, never reads the clock. `ui/scripts/check-ext-render.mjs`
 * renders it under node through `react-dom/server`, which is the only gate in this project that
 * can see a panel that compiles, mounts and draws nothing.
 *
 * Every handler is optional, and **an absent one means the control is not drawn** rather than
 * drawn dead — `GitPanelProps`' contract, in its own words: the panel cannot act by itself and
 * must not pretend to.
 *
 * `ExtensionsPanel.module.css`'s header describes the layout and argues it. The one decision worth
 * repeating here, because it is about content rather than pixels: a row shows its permissions as
 * **short tags with the full sentence as a tooltip**, and the sentence itself is on the
 * extension's page beside the Install button there. The first version printed the whole sentence
 * on every row, which in a 252px column is four wrapped lines per extension.
 */
import { useState } from 'react'

import { Icon } from '@/icons/Icon'

import styles from './ExtensionsPanel.module.css'
import {
  CAPABILITY_PROSE,
  CAPABILITY_SHORT,
  EXT_FILTERS,
  FILTER_LABEL,
  isStrongCapability,
  type CatalogRow,
  type ExtFilter,
  type ExtensionsModel,
  type MarketGroup,
} from './model'

export interface ExtensionsPanelViewProps {
  readonly model: ExtensionsModel
  /** Whether a call is in flight. Disables the buttons that would race it. */
  readonly busy?: boolean
  readonly onConnect?: ((source: string) => void) | undefined
  readonly onRefresh?: ((marketplace: string) => void) | undefined
  readonly onDisconnect?: ((marketplace: string) => void) | undefined
  /** Install or update. The capabilities the row was drawn with are echoed back — see `model.ts`. */
  readonly onInstall?:
    | ((marketplace: string, extension: string, capabilities: readonly string[]) => void)
    | undefined
  readonly onUninstall?: ((marketplace: string, extension: string) => void) | undefined
  readonly onSetEnabled?:
    | ((marketplace: string, extension: string, enabled: boolean) => void)
    | undefined
  /**
   * The search text changed, or a filter chip was pressed.
   *
   * One handler for both, because they are one query — see `ExtQuery`. Absent means the panel is
   * not searchable, which is what the render check drives to prove the controls are *not drawn*
   * rather than drawn dead.
   */
  readonly onQuery?: ((text: string, filter: ExtFilter) => void) | undefined
  /** Open an extension's page as a workspace tab. Absent means the row's name is not a button. */
  readonly onOpen?: ((marketplace: string, extension: string, name: string) => void) | undefined
}

export function ExtensionsPanelView(props: ExtensionsPanelViewProps): React.JSX.Element {
  const { model, busy = false, onConnect, onRefresh, onQuery } = props
  const [source, setSource] = useState('')
  /*
   * Whether the connect field is showing.
   *
   * Behind the header's `+` rather than always present, because connecting a marketplace is done
   * once and then never again — and a permanent field above the search box was most of what made
   * this panel look cluttered. Open by default when nothing is connected, so the one screen where
   * it is the *only* useful control does not need a click to reveal it.
   */
  const [connecting, setConnecting] = useState(model.kind === 'none')

  const total = model.kind === 'ready' ? model.counts.all : 0

  return (
    <div className={styles.panel}>
      <div className={styles.header}>
        <span className={styles.headerTitle}>Extensions</span>
        {onRefresh !== undefined && model.kind === 'ready' && model.groups.length > 0 && (
          <button
            type="button"
            className={styles.headerAction}
            disabled={busy}
            aria-label="Refresh every marketplace"
            title="Refresh every marketplace"
            onClick={() => {
              for (const group of model.groups) onRefresh(group.id)
            }}
          >
            <Icon name="refresh-cw" size={1} />
          </button>
        )}
        {onConnect !== undefined && (
          <button
            type="button"
            className={connecting ? `${styles.headerAction} ${styles.headerActionOn}` : styles.headerAction}
            aria-expanded={connecting}
            aria-label="Connect a marketplace"
            title="Connect a marketplace"
            onClick={() => setConnecting((open) => !open)}
          >
            +
          </button>
        )}
      </div>

      {onConnect !== undefined && connecting && (
        <form
          className={styles.connect}
          onSubmit={(event) => {
            event.preventDefault()
            const trimmed = source.trim()
            if (trimmed === '') return
            onConnect(trimmed)
            setSource('')
          }}
        >
          <input
            className={styles.input}
            value={source}
            spellCheck={false}
            placeholder="git URL, or a path on this machine"
            aria-label="Marketplace URL or path"
            onChange={(event) => setSource(event.target.value)}
          />
          <button type="submit" className={styles.primary} disabled={busy || source.trim() === ''}>
            Connect
          </button>
        </form>
      )}

      {model.kind === 'ready' && onQuery !== undefined && total > 0 && (
        <>
          <div className={styles.search}>
            <span className={styles.searchIcon} aria-hidden="true">
              <Icon name="search" size={1} />
            </span>
            <input
              className={`${styles.input} ${styles.searchInput}`}
              value={model.query.text}
              spellCheck={false}
              type="search"
              placeholder="Search extensions"
              aria-label="Search extensions"
              onChange={(event) => onQuery(event.target.value, model.query.filter)}
            />
          </div>
          {/*
           * Seven chips, each a question somebody actually asks — "what have I got", "what did I
           * turn off", "what needs updating". Each carries its count over the *unfiltered* set, so
           * a chip reading `0` is telling the truth about what pressing it would show rather than
           * about what the current selection happens to leave.
           *
           * A `tablist`/`tab` pair and not a `<select>`: one press rather than two, and the counts
           * are visible without opening anything. One scrolling row and never a wrapping block —
           * `ExtensionsPanel.module.css` says why.
           */}
          <div className={styles.filters} role="tablist" aria-label="Filter extensions">
            {EXT_FILTERS.map((filter) => {
              const active = model.query.filter === filter
              return (
                <button
                  key={filter}
                  type="button"
                  role="tab"
                  aria-selected={active}
                  className={active ? `${styles.chip} ${styles.chipOn}` : styles.chip}
                  // Never disabled, even at zero. A chip the user cannot press is a chip whose
                  // count they cannot confirm, and pressing an empty one shows the sentence that
                  // says so — which is more useful than a control that does not respond.
                  onClick={() => onQuery(model.query.text, filter)}
                >
                  {FILTER_LABEL[filter]}
                  <span className={styles.chipCount}>{model.counts[filter]}</span>
                </button>
              )
            })}
          </div>
        </>
      )}

      <div className={styles.body}>
        {model.kind === 'none' ? (
          <p className={`${styles.notice} ${styles.empty}`}>
            <span className={styles.emptyTitle}>No marketplace is connected</span>
            {model.hint.replace(/^No marketplace is connected\. /, '')}
          </p>
        ) : (
          <>
            {model.empty !== undefined && <p className={styles.notice}>{model.empty}</p>}
            {model.groups.map((group) => (
              <Group key={group.id} group={group} {...props} />
            ))}
            {model.orphans.length > 0 && (
              <section className={styles.group}>
                <h3 className={styles.groupName}>
                  <span className={styles.groupTitle}>Installed elsewhere</span>
                </h3>
                {model.orphans.map((row) => (
                  <Row key={`${row.marketplace}.${row.extension}`} row={row} {...props} />
                ))}
              </section>
            )}
            {/*
             * Languages and Problems are *not* filtered. They are readouts about the whole
             * registry rather than rows in the catalog — "why is my .sql coloured like that" has
             * the same answer whatever is typed in the search box — and hiding them when a search
             * is active would make the answer disappear exactly when somebody is looking for it.
             */}
            {model.languages.length > 0 && (
              <section className={`${styles.group} ${styles.report}`}>
                <h3 className={styles.groupName}>
                  <span className={styles.groupTitle}>Languages</span>
                </h3>
                {model.languages.map((lang) => (
                  <div key={lang.id} className={styles.langRow}>
                    <span className={styles.langName}>{lang.label}</span>
                    <span className={styles.by}>{lang.by}</span>
                    <span className={styles.langExt}>{lang.extensions}</span>
                    {lang.instead !== undefined && (
                      <span className={styles.instead}>{lang.instead}</span>
                    )}
                  </div>
                ))}
              </section>
            )}
            {model.problems.length > 0 && (
              <section className={`${styles.group} ${styles.report}`}>
                <h3 className={styles.groupName}>
                  <span className={styles.groupTitle}>Problems</span>
                </h3>
                {model.problems.map((problem, at) => (
                  <p
                    key={`${problem.path}-${at}`}
                    className={
                      problem.severity === 'error'
                        ? `${styles.problem} ${styles.problemError}`
                        : styles.problem
                    }
                    // The path and, where there is one, the line — in the form an editor's
                    // Go to line understands. `cide-agents`' rule for a malformed definition, and
                    // the reason a refusal is worth printing rather than counting.
                    title={
                      problem.line === undefined ? problem.path : `${problem.path}:${problem.line}`
                    }
                  >
                    {problem.message}
                  </p>
                ))}
              </section>
            )}
          </>
        )}
      </div>
    </div>
  )
}

function Group({
  group,
  busy = false,
  onRefresh,
  onDisconnect,
  ...rest
}: { group: MarketGroup } & ExtensionsPanelViewProps): React.JSX.Element {
  return (
    <section className={styles.group}>
      <h3 className={styles.groupName}>
        <span className={styles.groupTitle} title={group.source}>
          {group.name}
        </span>
        {/*
         * Whether connecting to this reaches the network and may use the user's credential
         * helper. On screen rather than left to be inferred from the URL, because it is the
         * difference between "this will use your SSH key" and "this is a directory on your disk".
         */}
        <span className={styles.route}>{group.authenticated ? 'remote' : 'local'}</span>
        <span className={styles.groupActions}>
          {onRefresh !== undefined && (
            <button
              type="button"
              className={styles.action}
              disabled={busy}
              onClick={() => onRefresh(group.id)}
            >
              Refresh
            </button>
          )}
          {onDisconnect !== undefined && (
            <button
              type="button"
              className={styles.action}
              disabled={busy}
              title="Forget this marketplace and uninstall everything from it"
              onClick={() => onDisconnect(group.id)}
            >
              Disconnect
            </button>
          )}
        </span>
      </h3>
      {group.detail !== undefined && (
        <p
          className={
            group.state === 'failed'
              ? `${styles.problem} ${styles.problemError}`
              : styles.problem
          }
        >
          {group.detail}
        </p>
      )}
      {group.rows.length === 0 && group.state === 'ready' && (
        <p className={styles.notice}>This marketplace lists no extensions.</p>
      )}
      {group.rows.map((row) => (
        <Row key={`${row.marketplace}.${row.extension}`} row={row} busy={busy} {...rest} />
      ))}
    </section>
  )
}

function Row({
  row,
  busy = false,
  onInstall,
  onUninstall,
  onSetEnabled,
  onOpen,
}: { row: CatalogRow } & ExtensionsPanelViewProps): React.JSX.Element {
  const installed = row.action === 'installed' || row.action === 'update'
  // The row's one-glance answer, and a *shape* difference as well as a colour one so it survives a
  // monochrome display — see `.dot` in the stylesheet.
  const dot =
    row.note !== undefined
      ? `${styles.dot} ${styles.dotBad}`
      : !installed
        ? styles.dot
        : row.enabled
          ? `${styles.dot} ${styles.dotOn}`
          : `${styles.dot} ${styles.dotOff}`
  const state =
    row.note !== undefined
      ? 'has a problem'
      : !installed
        ? 'not installed'
        : row.enabled
          ? 'enabled'
          : 'disabled'

  return (
    <div className={styles.row}>
      <span className={dot} role="img" aria-label={state} title={state} />
      <div className={styles.rowHead}>
        {/*
         * The name opens the extension's page — its README, what it contributes, what it asks
         * for. A button and not the whole row: the row already holds Install, Enable and Remove,
         * and a row-wide click target that sometimes navigated and sometimes hit a button is the
         * gesture the file tree's own rules exist to avoid.
         *
         * Absent when there is nowhere to open it — a detached window has no tab strip — and then
         * it is plain text rather than a dead button.
         */}
        {onOpen === undefined ? (
          <span className={styles.name}>{row.name}</span>
        ) : (
          <button
            type="button"
            className={`${styles.name} ${styles.nameLink}`}
            title={`Open ${row.name}'s page`}
            onClick={() => onOpen(row.marketplace, row.extension, row.name)}
          >
            {row.name}
          </button>
        )}
        {row.version !== '' && <span className={styles.version}>{row.version}</span>}
      </div>

      {row.description !== '' && <p className={styles.description}>{row.description}</p>}

      {row.capabilities.length > 0 && (
        <div className={styles.perms}>
          {row.capabilities.map((cap) => (
            <span
              key={cap}
              className={
                isStrongCapability(cap) ? `${styles.perm} ${styles.permStrong}` : styles.perm
              }
              // The short tag is never the only form: the sentence a person decides by is the
              // tooltip here and the body of the extension's page there.
              title={CAPABILITY_PROSE[cap] ?? cap}
            >
              {CAPABILITY_SHORT[cap] ?? cap}
            </span>
          ))}
        </div>
      )}

      {row.note !== undefined && (
        <p className={`${styles.problem} ${styles.problemError}`}>{row.note}</p>
      )}

      <div className={styles.rowActions}>
        {onInstall !== undefined && (row.action === 'install' || row.action === 'update') && (
          <button
            type="button"
            className={styles.primary}
            disabled={busy}
            onClick={() => onInstall(row.marketplace, row.extension, row.capabilities)}
          >
            {row.action === 'update' ? 'Update' : 'Install'}
          </button>
        )}
        {onSetEnabled !== undefined && installed && (
          <button
            type="button"
            className={styles.action}
            disabled={busy}
            onClick={() => onSetEnabled(row.marketplace, row.extension, !row.enabled)}
          >
            {row.enabled ? 'Disable' : 'Enable'}
          </button>
        )}
        {onUninstall !== undefined && (installed || row.action === 'unavailable') && (
          <button
            type="button"
            className={styles.action}
            disabled={busy}
            onClick={() => onUninstall(row.marketplace, row.extension)}
          >
            Remove
          </button>
        )}
        {/* A resting state, and never a disabled button: a control that cannot be pressed invites
            a press, and there is nothing here to do. */}
        {row.action === 'installed' && onSetEnabled === undefined && (
          <span className={styles.installed}>Installed</span>
        )}
      </div>
    </div>
  )
}
