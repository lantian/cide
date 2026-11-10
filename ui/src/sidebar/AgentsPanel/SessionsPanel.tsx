/**
 * The Agents panel's **Sessions** tab: the project's agent conversations as a tree the user
 * arranges. (M134, reworked M199)
 *
 * `cide_ipc::sessions` says what a session is here and why the journal behind this tab exists;
 * `sessionsModel.ts` names and searches rows, `sessionsTree.ts` decides where each one is drawn
 * and what a drop does, and `useSessionsDrag.ts` is the pointer mechanism.
 *
 * # Two lists
 *
 * - **Opened** is a tree: the user's groups (folders, nestable, ordered by hand), a rule, then
 *   every live session in no group — the ones a pane shows and the ones nothing shows (a run, a
 *   tab closed with its child kept), the second kind marked *Background*. A grouped session stays
 *   in its group after it ends, which is the point of a group.
 * - **Closed** is what has ended and is in no group, under day captions.
 *
 * The kind chips and the role select of M134 are gone; their words are in the search's haystack.
 *
 * # A view with no store and no IPC
 *
 * `WaitingPanel`'s rule, for `check-agents-render.mjs`: every fact arrives as a prop and every
 * gesture leaves as one, so a server render can draw it from a fixture. `AgentsPanelHost` reads
 * the stores, owns the context menu (`@/menus` reaches the DOM at import) and sends the edits.
 * The drag hook lives here because it is DOM-only: with no `onDrop` it never starts a drag.
 *
 * # Opening a row, and why selecting one moves nothing
 *
 * A click selects; a **double-click** or Enter opens — Show (a pane has it), Watch (live, no pane)
 * or Open (resume an ended one) — and so does the row's own button. M134's reason a click alone
 * must not open still holds: a stray click on a closed row would resume a `claude`.
 *
 * Selecting a row **changes no row's height**. The first build gave the selected row a second
 * line (status in words, task link, id), and that broke the double-click it relied on: selecting
 * a row below the selected one collapsed the old second line, the clicked row jumped up under the
 * pointer, and the second click landed on another row. The facts are in `SessionDetail`, a strip
 * under the tree, now; the only second line left is a transcript hit's snippet, which a search
 * draws and a click does not move. The host also drops an open asked for a conversation whose last
 * open has not answered, so a quick third click cannot resume it into a second pane.
 */
import { useMemo, useRef, type MouseEvent, type ReactElement, type ReactNode } from 'react'

import { Button, IconButton } from '@/kit/components/Button'
import { SearchField } from '@/kit/components/Field'
import { DragGhost } from '@/kit/components/Overlay'
import { Badge, Code, Dot } from '@/kit/components/Status'
import { InlineRename, TreeRow } from '@/kit/components/Surface'
import { Icon, asIcon } from '@/icons/Icon'

import panelStyles from '../TasksPanel/TasksPanel.module.css'
import runStyles from './AgentsPanel.module.css'
import styles from './SessionsPanel.module.css'
import {
  DAY_LABEL,
  ago,
  dayOf,
  displayName,
  hasOnlyGenericName,
  harnessLabel,
  isBackground,
  keeps,
  kindLabel,
  matchesText,
  rowStatus,
  searchWords,
  statusLook,
  type GroupView,
  type SessionView,
  type StatusTone,
} from './sessionsModel'
import { buildClosed, buildOpened, placeOf, stepSelection, tabCounts, type DropEdit, type TreeItem } from './sessionsTree'
import { useSessionsDrag } from './useSessionsDrag'

export interface SessionsPanelProps {
  project: string | null
  /** The tab strip (`AgentsPanelTabs`), drawn where the header's title goes. */
  title: ReactNode
  /** Every row, or `null` before the first answer. */
  rows: readonly SessionView[] | null
  groups: readonly GroupView[]
  error: string | null
  query: string
  transcripts: boolean
  /** Conversation id → snippet from the transcript search, or `null`. */
  hits: Readonly<Record<string, string>> | null
  searching: boolean
  capped: boolean
  /** Conversations with an open question — `questionedIds`. */
  questioned: ReadonlySet<string>
  sub: 'opened' | 'closed'
  selected: string | null
  renaming: string | null
  /** The pane that has focus, so the row it shows gets the accent bar. */
  currentPane?: string | null | undefined
  nowMs: number
  onQuery: (query: string) => void
  onTranscripts: (on: boolean) => void
  onSub: (sub: 'opened' | 'closed') => void
  onSelect: (id: string | null) => void
  onOpen: (id: string) => void
  /** Fold or unfold a group. Absent, folders draw but do not fold. */
  onToggleGroup?: ((id: string, collapsed: boolean) => void) | undefined
  onExpandAll?: (() => void) | undefined
  onCollapseAll?: (() => void) | undefined
  onNewGroup?: (() => void) | undefined
  /** Begin renaming a row in place (F2). */
  onStartRename?: ((id: string) => void) | undefined
  /** A rename's answer: the text, or `null` for cancelled. */
  onRename?: ((id: string, text: string | null) => void) | undefined
  onDeleteGroup?: ((id: string) => void) | undefined
  /** Send a drop's edit. Absent, nothing drags. */
  onDrop?: ((edit: DropEdit) => void) | undefined
  /** Open what a drag rested on: `'tab'` or `'group:<id>'`. */
  onSpring?: ((key: string) => void) | undefined
  /** The right-click menu, over the tree. */
  onContextMenu?: ((e: MouseEvent) => void) | undefined
  /**
   * A row's `⋯` button: open the same menu as a right-click on that row, **under the button**.
   * Both elements, because the menu is built from the row and placed at the button — placed at
   * the row, it opened at the row's left edge, a panel's width from the button that asked.
   */
  onMenuButton?: ((button: HTMLElement, row: HTMLElement) => void) | undefined
  /** Open a task's card — the run row's own task link. Absent, the task is plain text. */
  onRevealTask?: ((task: string) => void) | undefined
}

const TONE_CLASS: Readonly<Record<StatusTone, string | undefined>> = {
  idle: runStyles.toneIdle,
  busy: runStyles.toneBusy,
  attention: runStyles.toneAttention,
  paused: runStyles.tonePaused,
  done: runStyles.toneDone,
  error: runStyles.toneError,
}

/** Join class names, dropping a rule a stylesheet did not define (`RunRow`'s `cx`, and why). */
function cx(...names: (string | false | null | undefined)[]): string {
  return names.filter(Boolean).join(' ')
}

/** The status mark: RunRow's glyph, with its tone and spin. */
function StatusGlyph({ status }: { status: string }): ReactElement {
  const look = statusLook(status)
  return (
    <span
      className={cx(runStyles.glyph, TONE_CLASS[look.tone], look.glyph === 'loader-circle' && runStyles.glyphSpin, styles.glyph)}
      data-audit="sessionGlyph"
      data-status={status}
      title={look.label}
      aria-hidden="true"
    >
      <Icon name={asIcon(look.glyph)} size={1} />
    </span>
  )
}

/** What a row's open button says, and does. */
function openVerb(v: SessionView): { icon: 'arrow-up-right' | 'eye' | 'play'; label: string; title: string } {
  if (v.pane !== null) return { icon: 'arrow-up-right', label: 'Show', title: 'Show — go to the pane showing this conversation' }
  if (v.live) return { icon: 'eye', label: 'Watch', title: 'Watch — open a pane on this running session' }
  return { icon: 'play', label: 'Open', title: 'Open — resume this conversation in its harness' }
}

export function SessionsPanel(props: SessionsPanelProps): ReactElement {
  const { project, title, rows, groups, error, query, transcripts, searching, capped, sub, onQuery, onTranscripts } = props
  return (
    <aside className={panelStyles.panel} data-audit="sidebarSessions" aria-label="Sessions">
      <div className={panelStyles.header} data-audit="agentsHeader">
        <span className={panelStyles.headerTitle}>{title}</span>
      </div>
      <div className={styles.body} data-audit="sessionsBody">
        {project === null ? (
          <p className={cx(panelStyles.waitQuiet, styles.quiet)}>Open a project to see its sessions.</p>
        ) : (
          <>
            <div className={styles.controls}>
              <SearchField
                size="sm"
                aria-label="Search sessions"
                placeholder={transcripts ? 'Search sessions and transcripts' : 'Search sessions'}
                value={query}
                data-audit="sessionsSearch"
                onChange={(e) => onQuery(e.target.value)}
                onKeyDown={(e) => {
                  // Escape empties the box first and only then lets the panel have the key —
                  // the Tasks panel's search, one panel over, behaves the same.
                  if (e.key === 'Escape' && query !== '') {
                    e.stopPropagation()
                    onQuery('')
                  }
                }}
                trailing={
                  <IconButton
                    icon="scroll-text"
                    size="sm"
                    pressed={transcripts}
                    data-audit="sessionsTranscripts"
                    label={
                      transcripts
                        ? 'Searching transcripts too — press to search names only'
                        : 'Also search transcripts: what was said in them, not only their titles. Slower; opencode keeps none.'
                    }
                    onClick={() => onTranscripts(!transcripts)}
                  />
                }
              />
              {transcripts && query.trim() !== '' && (searching || capped) && (
                <p className={styles.searchNote} data-audit="sessionsSearchNote">
                  {searching ? 'Reading transcripts…' : 'Only the newest 200 transcripts were read.'}
                </p>
              )}
              {rows !== null && rows.length > 0 && <SubBar {...props} rows={rows} />}
            </div>
            {error !== null && (
              <p className={cx(panelStyles.waitQuiet, styles.quiet)} role="alert" data-audit="sessionsError">
                {error}
              </p>
            )}
            {rows === null ? (
              error === null && <p className={cx(panelStyles.waitQuiet, styles.quiet)}>Reading sessions…</p>
            ) : rows.length === 0 ? (
              <p className={cx(panelStyles.waitQuiet, styles.quiet)} data-audit="sessionsEmpty">
                No agent session in this project yet. Every console, tab, subagent, review and
                OpenSpec session that does some work is listed here.
              </p>
            ) : (
              <>
                <SessionsTree {...props} rows={rows} groups={groups} sub={sub} />
                <SelectedDetail {...props} rows={rows} />
              </>
            )}
          </>
        )}
      </div>
    </aside>
  )
}

/** `SessionDetail` for the selected session, when it is one the showing list draws. */
function SelectedDetail(props: SessionsPanelProps & { rows: readonly SessionView[] }): ReactElement | null {
  const { rows, groups, selected, sub, questioned, onRevealTask } = props
  const v = selected === null ? undefined : rows.find((r) => r.id === selected)
  if (v === undefined) return null
  const closed = placeOf(v, groups) === 'closed'
  if (closed !== (sub === 'closed')) return null
  return <SessionDetail v={v} status={rowStatus(v, questioned)} onRevealTask={onRevealTask} />
}

/** The Opened / Closed segment, with Expand all, Collapse all and New group on Opened. */
function SubBar(props: SessionsPanelProps & { rows: readonly SessionView[] }): ReactElement {
  const { rows, groups, query, hits, sub, onSub, onExpandAll, onCollapseAll, onNewGroup } = props
  const counts = tabCounts(rows, groups, (v) => keeps(v, query, hits))
  const tab = (which: 'opened' | 'closed', label: string, n: number) => (
    <button
      type="button"
      className={cx(panelStyles.filter, sub === which && panelStyles.filterOn)}
      aria-pressed={sub === which}
      data-audit="sessionsSub"
      data-sessions-tab={which}
      onClick={() => onSub(which)}
    >
      {label} <span className={styles.count}>{n}</span>
    </button>
  )
  return (
    <div className={styles.subbar} role="group" aria-label="Which sessions" data-audit="sessionsSubs">
      {tab('opened', 'Opened', counts.opened)}
      {tab('closed', 'Closed', counts.closed)}
      <span className={styles.spacer} />
      {sub === 'opened' && (
        <>
          {onExpandAll !== undefined && (
            <IconButton icon="chevrons-up-down" size="sm" label="Expand all groups" disabled={groups.length === 0} data-audit="sessionsExpandAll" onClick={onExpandAll} />
          )}
          {onCollapseAll !== undefined && (
            <IconButton icon="chevrons-down-up" size="sm" label="Collapse all groups" disabled={groups.length === 0} data-audit="sessionsCollapseAll" onClick={onCollapseAll} />
          )}
          {onNewGroup !== undefined && (
            <IconButton icon="square-plus" size="sm" label="New group" data-audit="sessionsNewGroup" onClick={onNewGroup} />
          )}
        </>
      )}
    </div>
  )
}

function SessionsTree(
  props: SessionsPanelProps & { rows: readonly SessionView[] },
): ReactElement {
  const { rows, groups, query, hits, questioned, sub, selected, renaming, nowMs } = props
  const { onSelect, onOpen, onToggleGroup, onStartRename, onRename, onDeleteGroup, onDrop, onSpring } = props
  const box = useRef<HTMLDivElement | null>(null)
  const words = searchWords(query)
  const status = (v: SessionView) => rowStatus(v, questioned)

  const opened = useMemo(() => {
    if (sub !== 'opened') return []
    const words = searchWords(query)
    return buildOpened(rows, groups, {
      keep: (v) => keeps(v, query, hits),
      status: (v) => rowStatus(v, questioned),
      searching: words.length > 0,
      words,
    })
  }, [sub, rows, groups, query, hits, questioned])
  // The day captions move at midnight, so the minute is enough of the clock to depend on.
  const minute = Math.floor(nowMs / 60_000)
  const closed = useMemo(
    () =>
      sub === 'closed'
        ? buildClosed(rows, groups, (v) => keeps(v, query, hits), (v) => dayOf(minute * 60_000, v.lastSeenMs))
        : [],
    [sub, rows, groups, query, hits, minute],
  )
  const order: string[] = sub === 'opened' ? opened.filter((i) => i.type !== 'rule').map((i) => i.id) : closed.flatMap((b) => b.views.map((v) => v.id))

  const drag = useSessionsDrag({
    container: box,
    views: rows,
    groups,
    sub,
    labelOf: (load) =>
      load.type === 'group'
        ? (groups.find((g) => g.id === load.id)?.name ?? '')
        : displayName(rows.find((v) => v.id === load.id) ?? rows[0]!),
    onDrop,
    onSpring,
  })
  const target = drag.state?.target ?? null
  const verdict = drag.state?.outcome ?? null
  const dropKind = verdict === null || verdict.kind === 'spring' ? undefined : verdict.kind
  // The rows a band washes: everything drawn inside the destination group.
  const bandOf = (item: TreeItem): boolean => {
    if (target?.kind !== 'group' || dropKind === undefined || dropKind === 'refuse') return false
    let at: string | null = item.type === 'group' ? item.group.parent : item.type === 'session' ? item.view.group : null
    for (let i = 0; i <= groups.length && at !== null; i += 1) {
      if (at === target.id) return true
      at = groups.find((g) => g.id === at)?.parent ?? null
    }
    return false
  }

  const groupItem = (item: Extract<TreeItem, { type: 'group' }>): ReactElement => {
    const g = item.group
    const isTarget = target?.kind === 'group' && target.id === g.id
    const line = target?.kind === 'line' && target.id === g.id && dropKind !== 'refuse' ? target.where : undefined
    return (
      <TreeRow
        key={g.id}
        depth={item.depth}
        expanded={item.open}
        onToggle={onToggleGroup === undefined ? undefined : () => onToggleGroup(g.id, !g.collapsed)}
        icon={<Icon name="folder" size={1} className={styles.folder} />}
        label={
          renaming === g.id && onRename !== undefined ? (
            <InlineRename value={g.name} label="Rename group" onDone={(text) => onRename(g.id, text)} />
          ) : (
            <span className={styles.groupName} data-audit="sessionGroupName" title={g.name}>
              <Mark text={g.name} words={words} />
            </span>
          )
        }
        aside={
          !item.open && (item.working || item.waiting > 0) ? (
            <span className={styles.rollup} data-audit="sessionGroupRollup">
              {item.working && <StatusGlyph status="working" />}
              {item.waiting > 0 && <Dot tone="yellow" label={`${item.waiting} waiting for you`} />}
            </span>
          ) : undefined
        }
        detail={item.count}
        selected={selected === g.id}
        dragging={drag.state?.load.type === 'group' && drag.state.load.id === g.id}
        drop={isTarget ? dropKind : undefined}
        band={bandOf(item)}
        line={line}
        data-session-row="group"
        data-id={g.id}
        data-audit="sessionGroup"
        onPointerDown={(e) => drag.onPointerDown(e, { type: 'group', id: g.id })}
        onClick={() => {
          if (drag.dragged()) return
          onSelect(g.id)
          onToggleGroup?.(g.id, !g.collapsed)
        }}
      />
    )
  }

  const sessionItem = (v: SessionView, depth: number, inTree: boolean): ReactElement => {
    const s = status(v)
    const look = statusLook(s)
    const isSelected = selected === v.id
    const hit = hits !== null && Object.hasOwn(hits, v.id) && !matchesText(v, query) ? hits[v.id] : undefined
    const verb = openVerb(v)
    const name = displayName(v)
    const ended = !v.live
    return (
      <TreeRow
        key={v.id}
        depth={inTree ? depth : 0}
        icon={<StatusGlyph status={s} />}
        label={
          renaming === v.id && onRename !== undefined ? (
            <InlineRename value={name} label="Rename session" onDone={(text) => onRename(v.id, text)} />
          ) : (
            <span className={cx(styles.name, hasOnlyGenericName(v) && styles.generic)} data-audit="sessionTitle" title={name}>
              <Mark text={name} words={words} />
            </span>
          )
        }
        aside={
          isBackground(v) ? (
            <span className={styles.badge} data-audit="sessionBackground">
              <Badge tone="purple" soft>
                Background
              </Badge>
            </span>
          ) : undefined
        }
        detail={v.harness}
        meta={<span title={new Date(v.lastSeenMs).toLocaleString()}>{ago(nowMs, v.lastSeenMs)}</span>}
        actions={
          <>
            <IconButton icon={verb.icon} size="sm" label={verb.title} data-audit="sessionsOpen" data-verb={verb.label} onClick={(e) => { e.stopPropagation(); onOpen(v.id) }} />
            {props.onMenuButton !== undefined && (
              <IconButton
                icon="ellipsis"
                size="sm"
                label="More"
                data-audit="sessionsMenu"
                onClick={(e) => {
                  e.stopPropagation()
                  const button = e.currentTarget as HTMLElement
                  const row = button.closest<HTMLElement>('[data-session-row]')
                  if (row !== null) props.onMenuButton?.(button, row)
                }}
              />
            )}
          </>
        }
        sub={
          hit === undefined ? undefined : (
            <span className={styles.snippet} data-audit="sessionsHit">
              “<Mark text={hit} words={words} />”
            </span>
          )
        }
        selected={isSelected}
        current={v.pane !== null && v.pane === props.currentPane}
        dim={ended}
        dragging={drag.state?.load.type === 'session' && drag.state.load.id === v.id}
        band={bandOf({ type: 'session', id: v.id, depth, view: v })}
        title={`${look.label}${isBackground(v) ? ' · in the background, no pane shows it' : ''}`}
        data-session-row="session"
        data-id={v.id}
        data-group={inTree ? (v.group !== null && groups.some((g) => g.id === v.group) ? v.group : '') : ''}
        data-audit="sessionRow"
        data-kind={v.kind}
        data-live={v.live}
        data-status={s}
        onPointerDown={(e) => drag.onPointerDown(e, { type: 'session', id: v.id })}
        onClick={() => {
          if (drag.dragged()) return
          onSelect(v.id)
        }}
        onDoubleClick={() => onOpen(v.id)}
      />
    )
  }

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.target !== e.currentTarget) return
    const group = groups.find((g) => g.id === selected)
    const view = rows.find((v) => v.id === selected)
    const next = stepSelection(order, selected, e.key)
    if (next !== null) {
      onSelect(next)
      e.preventDefault()
      return
    }
    if (e.key === 'ArrowRight' && group !== undefined && group.collapsed) onToggleGroup?.(group.id, false)
    else if (e.key === 'ArrowLeft') {
      if (group !== undefined && !group.collapsed) onToggleGroup?.(group.id, true)
      else if (view?.group != null && groups.some((g) => g.id === view.group)) onSelect(view.group)
      else if (group?.parent != null) onSelect(group.parent)
      else return
    } else if (e.key === 'Enter') {
      if (group !== undefined) onToggleGroup?.(group.id, !group.collapsed)
      else if (view !== undefined) onOpen(view.id)
      else return
    } else if (e.key === 'F2' && selected !== null && (group !== undefined || view !== undefined)) onStartRename?.(selected)
    else if (e.key === 'Delete' && group !== undefined) onDeleteGroup?.(group.id)
    else return
    e.preventDefault()
  }

  const ghost = drag.state === null ? null : (
    <DragGhost
      x={drag.state.x}
      y={drag.state.y}
      load={drag.state.label}
      hint={verdict?.hint ?? (sub === 'closed' ? 'Drop it on the Opened tab, or use Move to group' : 'Drop it on a group')}
      outcome={verdict === null ? 'none' : verdict.kind === 'spring' ? 'move' : verdict.kind}
    />
  )

  return (
    <div
      ref={box}
      className={styles.tree}
      role="tree"
      tabIndex={0}
      aria-label={sub === 'opened' ? 'Opened sessions' : 'Closed sessions'}
      data-audit="sessionsTree"
      data-sub={sub}
      data-dragging={drag.state !== null || undefined}
      data-drop-root={target?.kind === 'root' && dropKind !== undefined ? dropKind : undefined}
      onKeyDown={onKeyDown}
      onContextMenu={props.onContextMenu}
    >
      {sub === 'opened' ? (
        <>
          {opened.map((item) =>
            item.type === 'group' ? (
              groupItem(item)
            ) : item.type === 'rule' ? (
              <div key={item.id} className={styles.rule} role="separator" data-audit="sessionsRule" />
            ) : (
              sessionItem(item.view, item.depth, true)
            ),
          )}
          {opened.length === 0 && (
            <p className={cx(panelStyles.waitQuiet, styles.quiet)} data-audit="sessionsNoMatch">
              {words.length > 0 ? (props.searching ? 'Searching…' : 'Nothing open matches.') : 'Nothing is open. Closed sessions are one tab over.'}
            </p>
          )}
          {groups.length === 0 && words.length === 0 && props.onNewGroup !== undefined && (
            <p className={styles.hint} data-audit="sessionsGroupHint">
              Make a group with <Icon name="square-plus" size={0} className={styles.inlineIcon} /> and drag sessions into it. A grouped
              session stays in its group after it ends — it never moves to Closed.
            </p>
          )}
          <div className={styles.rootZone} data-audit="sessionsRoot" />
        </>
      ) : (
        <>
          {closed.map((bucket) => (
            <div key={bucket.day} role="group" aria-label={DAY_LABEL[bucket.day]}>
              <p className={styles.caption} data-audit="sessionsDay">
                {DAY_LABEL[bucket.day]}
              </p>
              {bucket.views.map((v) => sessionItem(v, 0, false))}
            </div>
          ))}
          {closed.length === 0 && (
            <p className={cx(panelStyles.waitQuiet, styles.quiet)} data-audit="sessionsNoMatch">
              {words.length > 0 ? (props.searching ? 'Searching…' : 'Nothing closed matches.') : 'Nothing closed yet.'}
            </p>
          )}
        </>
      )}
      {ghost}
    </div>
  )
}

/**
 * The selected session's facts, in a strip under the tree: its name whole, what it is doing in
 * words, what it is and where, its task (a link to the card) and its id. Under the tree rather
 * than under the row — see the header for the double-click that a growing row broke.
 */
function SessionDetail({
  v,
  status,
  onRevealTask,
}: {
  v: SessionView
  status: string
  onRevealTask: ((task: string) => void) | undefined
}): ReactElement {
  const look = statusLook(status)
  const tone = look.tone
  const statusLabel = look.label
  const task = v.task
  const taskText = task === null ? null : `${task}${v.taskTitle !== null ? ` · ${v.taskTitle}` : ''}`
  const planning = [
    v.planningPolicy == null ? null : v.planningPolicy === 'required' ? 'Plan before execution' : 'Direct execution',
    v.planningPhase == null ? null : { planning: 'Planning', approving: 'Accepting plan', implementing: 'Implementing' }[v.planningPhase],
  ].filter((s): s is string => s !== null)
  return (
    <div className={styles.detail} data-audit="sessionsDetail" aria-label="Selected session">
      <div className={styles.detailName} title={displayName(v)}>
        {displayName(v)}
      </div>
      <div className={styles.detailLine}>
        <span className={cx(styles.status, tone === 'attention' && styles.attention, tone === 'error' && styles.error)} data-audit="sessionStatusWords">
          {statusLabel}
        </span>
        <span>· {kindLabel(v.kind)}</span>
        {v.harness !== '' && <span>· {harnessLabel(v.harness)}</span>}
        {v.branch !== null && <span>· {v.branch}</span>}
        {planning.map((p) => (
          <span key={p}>· {p}</span>
        ))}
      </div>
      <div className={styles.detailLine}>
        {task !== null && taskText !== null && (
          onRevealTask === undefined ? (
            <span>{taskText}</span>
          ) : (
            <Button variant="link" size="sm" data-audit="sessionsTask" title={`Open ${task}`} onClick={() => onRevealTask(task)}>
              {taskText}
            </Button>
          )
        )}
        <Code>{v.id.length > 12 ? `${v.id.slice(0, 8)}…` : v.id}</Code>
      </div>
      {isBackground(v) && v.run === null && <div className={styles.note}>Its tab is closed; it keeps running until it is shown or ends.</div>}
    </div>
  )
}

/** `text` with the search's words marked. */
function Mark({ text, words }: { text: string; words: readonly string[] }): ReactElement {
  if (words.length === 0) return <>{text}</>
  const lower = text.toLowerCase()
  const marks: [number, number][] = []
  for (const w of words) {
    let at = lower.indexOf(w)
    while (at >= 0) {
      marks.push([at, at + w.length])
      at = lower.indexOf(w, at + w.length)
    }
  }
  if (marks.length === 0) return <>{text}</>
  marks.sort((a, b) => a[0] - b[0])
  const parts: ReactNode[] = []
  let end = 0
  for (const [from, to] of marks) {
    if (to <= end) continue
    const start = Math.max(from, end)
    if (start > end) parts.push(text.slice(end, start))
    parts.push(
      <mark key={start} className={styles.mark}>
        {text.slice(start, to)}
      </mark>,
    )
    end = to
  }
  if (end < text.length) parts.push(text.slice(end))
  return <>{parts}</>
}
