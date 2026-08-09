/**
 * The 30px workspace tab strip, below the header and to the right of the rail.
 *
 * The first tab is the pinned Claude console and never draws a close button. That is read
 * off `kind === 'claudeHome'`, never off the index: array order is a rendering detail,
 * whereas the pin is a property of the tab. Withholding the button is a courtesy to the
 * user, not the enforcement — the Rust side stays the authority that refuses the close.
 *
 * The component is deliberately store-free so a detached-tab window, which mirrors a
 * different workspace slice, can render the same strip from props.
 */
import type { ReactNode } from 'react'
import type { Tab, TabId, TabKind } from '@/ipc/client'
import styles from './TabStrip.module.css'

export interface TabStripProps {
  tabs: Tab[]
  activeTab: TabId
  onActivate?: ((id: TabId) => void) | undefined
  onClose?: ((id: TabId) => void) | undefined
  onSplit?: (() => void) | undefined
  onDetach?: (() => void) | undefined
}

export function TabStrip({ tabs, activeTab, onActivate, onClose, onSplit, onDetach }: TabStripProps) {
  // The `data-audit` attributes below are how chrome/layoutAudit.ts locates this surface:
  // CSS Module class names are hashed at build time, so nothing outside can select on them.
  return (
    <div className={styles.strip} data-audit="tabStrip">
      {/* The tablist holds tabs and nothing else — the split and detach buttons are not
          tabs, and a tablist that owns them mis-announces the count to a screen reader. */}
      <div className={styles.tabs} role="tablist" aria-label="Open tabs">
        {tabs.map((tab) => (
          <TabItem
            key={tab.id}
            tab={tab}
            active={tab.id === activeTab}
            onActivate={onActivate}
            onClose={onClose}
          />
        ))}
      </div>
      <div className={styles.spacer} />
      <div className={styles.cluster}>
        <button type="button" className={styles.clusterButton} title="Split right" onClick={onSplit}>
          ⊞
        </button>
        <button type="button" className={styles.clusterButton} title="Detach tab" onClick={onDetach}>
          ⧉
        </button>
      </div>
    </div>
  )
}

interface TabItemProps {
  tab: Tab
  active: boolean
  onActivate?: ((id: TabId) => void) | undefined
  onClose?: ((id: TabId) => void) | undefined
}

function TabItem({ tab, active, onActivate, onClose }: TabItemProps) {
  const view = viewFor(tab.kind)
  const shell = active ? `${styles.tab} ${styles.tabActive}` : styles.tab
  const label = view.closable ? styles.label : `${styles.label} ${styles.labelPinned}`

  return (
    // `presentation` so the row does not sit between the tablist and its tabs: an
    // intervening generic element breaks the ownership the tabs pattern requires.
    <div
      className={`${shell} ${view.shape}`}
      role="presentation"
      data-audit={view.audit}
      data-active={active ? 'true' : 'false'}
    >
      <button
        type="button"
        className={label}
        role="tab"
        aria-selected={active}
        title={view.hint}
        onClick={() => onActivate?.(tab.id)}
      >
        {view.body}
      </button>
      {view.closable && (
        <button
          type="button"
          className={view.dirty ? `${styles.close} ${styles.closeDirty}` : styles.close}
          title={view.dirty ? 'Close (unsaved changes)' : 'Close tab'}
          onClick={() => onClose?.(tab.id)}
        >
          {view.dirty ? '•' : '×'}
        </button>
      )}
    </div>
  )
}

interface TabView {
  /** Metrics class: font, padding and the gap shared with the label button. */
  shape: string | undefined
  body: ReactNode
  closable: boolean
  dirty: boolean
  /** Tooltip text, and the only place the full path is legible once the name is clipped. */
  hint: string
  /** Layout-audit hook, on the kinds the mock states dimensions for. */
  audit: string | undefined
}

function viewFor(kind: TabKind): TabView {
  switch (kind.kind) {
    case 'claudeHome':
      return {
        shape: styles.shapeClaude,
        body: (
          <>
            <span className={styles.swatch} data-audit="consoleSwatch" />
            <span>Claude</span>
            <span className={styles.pinned} data-audit="pinnedChip">
              PINNED
            </span>
          </>
        ),
        closable: false,
        dirty: false,
        hint: 'Claude console (pinned)',
        audit: 'consoleTab',
      }

    case 'claudeFull':
      return {
        shape: styles.shapeClaude,
        body: (
          <>
            <span className={styles.swatch} />
            <span>{kind.title}</span>
          </>
        ),
        closable: true,
        dirty: false,
        hint: kind.title,
        audit: undefined,
      }

    case 'file': {
      const badge = badgeFor(kind.path)
      return {
        shape: styles.shapeFile,
        body: (
          <>
            <span className={`${styles.badge} ${badge.tone}`} data-audit="fileTabBadge">
              {badge.label}
            </span>
            <span>{basename(kind.path)}</span>
          </>
        ),
        closable: true,
        dirty: kind.dirty,
        hint: kind.path,
        audit: 'fileTab',
      }
    }

    case 'diff': {
      // The mock has no diff tab, so it borrows the file shape and takes its badge from the
      // new side — a diff of a .rs file should sit next to that file under the same RS mark.
      const badge = badgeFor(kind.spec.newPath)
      return {
        shape: styles.shapeFile,
        body: (
          <>
            <span className={`${styles.badge} ${badge.tone}`} data-audit="fileTabBadge">
              {badge.label}
            </span>
            <span>{kind.spec.title}</span>
          </>
        ),
        closable: true,
        dirty: false,
        hint: `${kind.spec.oldPath} → ${kind.spec.newPath}`,
        audit: 'fileTab',
      }
    }

    case 'settings':
      return {
        shape: styles.shapeSettings,
        body: <span>Settings</span>,
        closable: true,
        dirty: false,
        hint: 'Settings',
        audit: undefined,
      }

    default:
      return unhandled(kind)
  }
}

/** Turns a future `TabKind` variant into a compile error here rather than a blank tab. */
function unhandled(kind: never): never {
  throw new Error(`unhandled tab kind: ${JSON.stringify(kind)}`)
}

function basename(path: string): string {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  return path.slice(cut + 1)
}

/** Label and colour for a path's language, from the mock's own badge table. */
function badgeFor(path: string) {
  const name = basename(path)
  const dot = name.lastIndexOf('.')
  // `> 0` rather than `>= 0`: a leading dot makes a hidden file, not an extension, so
  // `.gitignore` falls through to the neutral marker instead of claiming a GITIGNORE badge.
  const ext = dot > 0 ? name.slice(dot + 1).toLowerCase() : ''

  switch (ext) {
    case 'rs':
      return { label: 'RS', tone: styles.toneAccent }
    case 'ts':
      return { label: 'TS', tone: styles.toneBlue }
    case 'tsx':
      return { label: 'TSX', tone: styles.toneCyan }
    case 'js':
      return { label: 'JS', tone: styles.toneYellow }
    case 'toml':
      return { label: 'TOML', tone: styles.toneGreen }
    case 'lock':
      return { label: 'LOCK', tone: styles.toneFaint }
    case 'md':
      return { label: 'MD', tone: styles.toneDim }
    case 'yml':
    case 'yaml':
      return { label: 'YML', tone: styles.tonePurple }
    case 'json':
      return { label: 'JSON', tone: styles.toneYellow }
    case 'sh':
      return { label: 'SH', tone: styles.toneGreen }
    default:
      return { label: '·', tone: styles.toneFaint }
  }
}
