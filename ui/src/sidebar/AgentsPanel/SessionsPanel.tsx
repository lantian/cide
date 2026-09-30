/**
 * The Agents panel's **Sessions** tab: every agent conversation cide has hosted for the project,
 * newest first, with a kind filter, a search, and Open. (M134)
 *
 * `cide_ipc::sessions` says what a session is here and why the journal behind this tab exists at
 * all; `sessionsModel.ts` holds the filter and the labels.
 *
 * # A view with no store and no IPC
 *
 * `WaitingPanel`'s rule, for `check-agents-render.mjs`: every fact arrives as a prop and every
 * gesture leaves as one, so a server render can draw it from a fixture. `AgentsPanelHost` reads
 * `sessionsStore` and owns Open.
 *
 * # Built from what the sidebar already draws
 *
 * The frame and header are `TasksPanel.module.css`'s, as `WaitingPanel`'s are, so the header does
 * not jump when the tab changes. The kind filter is the Tasks panel's status filter row — the same
 * classes, so two filter rows one panel apart read as one control — multi-select here because
 * "consoles and tabs" is a real question and "one kind at a time" would make it two searches. The
 * search box, the role select and the transcripts checkbox are the kit's; the rows are kit
 * `ListItem`s.
 *
 * # Open is a button, not the row
 *
 * The row carries a second gesture — its task is a link that opens the task's card, as on the
 * Subagents tab — and a row that opened its conversation on any click turned a missed task link
 * into a resumed `claude`, which spawns a process and can spend a turn. So the row itself does
 * nothing, and the two gestures are two controls that say what they do.
 */
import type { ReactNode } from 'react'

import { Button } from '@/kit/components/Button'
import { Checkbox } from '@/kit/components/Choice'
import { SearchField } from '@/kit/components/Field'
import { Select } from '@/kit/components/Select'
import { List, ListItem } from '@/kit/components/Surface'
import { Badge, Code, Dot } from '@/kit/components/Status'

import panelStyles from '../TasksPanel/TasksPanel.module.css'
import styles from './SessionsPanel.module.css'
import {
  agentCounts,
  ago,
  displayTitle,
  harnessLabel,
  kindCounts,
  kindLabel,
  type SessionKindName,
  type SessionView,
} from './sessionsModel'

export interface SessionsPanelProps {
  project: string | null
  /** The tab strip (`AgentsPanelTabs`), drawn where the header's title goes. */
  title: ReactNode
  /** Every row, unfiltered — the chips count these — or `null` before the first answer. */
  rows: readonly SessionView[] | null
  /** `filterSessions(rows, …)`: what the list shows. */
  shown: readonly SessionView[]
  error: string | null
  query: string
  kinds: ReadonlySet<SessionKindName>
  /** The subagent role picked, or `null` for all. */
  agent: string | null
  transcripts: boolean
  /** Conversation id → snippet from the transcript search, or `null`. */
  hits: Readonly<Record<string, string>> | null
  searching: boolean
  capped: boolean
  nowMs: number
  onQuery: (query: string) => void
  onToggleKind: (kind: SessionKindName) => void
  onAllKinds: () => void
  onAgent: (agent: string | null) => void
  onTranscripts: (on: boolean) => void
  onOpen: (id: string) => void
  /** Open a task's card — the run row's own task link. Absent, the task is plain text. */
  onRevealTask?: ((task: string) => void) | undefined
}

/** The role select's value for "every role" — not a string a role id can be (ids are `[a-z0-9-]`). */
const ALL_ROLES = ' all'


/** A run's kinds, tinted apart from the panes the user opened themselves. */
const RUN_KINDS: ReadonlySet<SessionKindName> = new Set(['subagent', 'mrReview', 'openSpec'])

function cx(...names: (string | false | undefined)[]): string {
  return names.filter(Boolean).join(' ')
}

export function SessionsPanel({
  project,
  title,
  rows,
  shown,
  error,
  query,
  kinds,
  agent,
  transcripts,
  hits,
  searching,
  capped,
  nowMs,
  onQuery,
  onToggleKind,
  onAllKinds,
  onAgent,
  onTranscripts,
  onOpen,
  onRevealTask,
}: SessionsPanelProps) {
  const counts = rows === null ? [] : kindCounts(rows)
  const roles = rows === null ? [] : agentCounts(rows)

  return (
    <aside className={panelStyles.panel} data-audit="sidebarSessions" aria-label="Sessions">
      <div className={panelStyles.header} data-audit="agentsHeader">
        <span className={panelStyles.headerTitle}>{title}</span>
        {/* No figure here: the three-tab strip fills the header at the default sidebar width,
            and the kind chips below already count every kind. */}
      </div>
      <div className={panelStyles.body} data-audit="sessionsBody">
        {project === null ? (
          <p className={panelStyles.waitQuiet}>Open a project to see its sessions.</p>
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
              />
              <Checkbox
                label="Search transcripts"
                hint={
                  searching
                    ? 'Reading transcripts…'
                    : capped
                      ? 'Only the newest 200 were read.'
                      : 'What was said in them, not only their titles. Slower; opencode keeps none.'
                }
                checked={transcripts}
                onChange={onTranscripts}
              />
              {counts.length > 1 && (
                <div
                  className={cx(panelStyles.filters, styles.kinds)}
                  role="group"
                  aria-label="Filter sessions by kind"
                  data-audit="sessionsKinds"
                >
                  <button
                    type="button"
                    className={cx(panelStyles.filter, kinds.size === 0 && panelStyles.filterOn)}
                    aria-pressed={kinds.size === 0}
                    data-audit="sessionsKind"
                    data-kind=""
                    onClick={onAllKinds}
                  >
                    All
                  </button>
                  {counts.map(({ kind, count }) => {
                    const on = kinds.has(kind)
                    return (
                      <button
                        key={kind}
                        type="button"
                        className={cx(panelStyles.filter, on && panelStyles.filterOn)}
                        aria-pressed={on}
                        data-audit="sessionsKind"
                        data-kind={kind}
                        onClick={() => onToggleKind(kind)}
                      >
                        {kindLabel(kind)} <span className={styles.count}>{count}</span>
                      </button>
                    )
                  })}
                </div>
              )}
              {/* Drawn with one role too: it is then the one way to say "only runs", and a
                  control that appeared at the second role would move everything under it. Kept
                  while a picked role has gone from the rows, so the filter can be undone. */}
              {(roles.length > 0 || agent !== null) && (
                <div className={styles.role} data-audit="sessionsRole">
                  <Select
                    size="sm"
                    aria-label="Filter sessions by subagent"
                    value={agent ?? ALL_ROLES}
                    onChange={(value) => onAgent(value === ALL_ROLES ? null : value)}
                    options={[
                      { value: ALL_ROLES, label: 'Every subagent' },
                      ...roles.map((r) => ({ value: r.agent, label: r.agent, detail: String(r.count) })),
                    ]}
                  />
                </div>
              )}
            </div>
            {error !== null && (
              <p className={panelStyles.waitQuiet} role="alert" data-audit="sessionsError">
                {error}
              </p>
            )}
            {rows === null ? (
              error === null && <p className={panelStyles.waitQuiet}>Reading sessions…</p>
            ) : rows.length === 0 ? (
              <p className={panelStyles.waitQuiet} data-audit="sessionsEmpty">
                No agent session in this project yet. Every console, tab, subagent, review and
                OpenSpec session that does some work is listed here.
              </p>
            ) : shown.length === 0 ? (
              <p className={panelStyles.waitQuiet} data-audit="sessionsNoMatch">
                {searching ? 'Searching…' : 'Nothing matches.'}
              </p>
            ) : (
              <List label="Sessions">
                {shown.map((v) => (
                  <SessionItem
                    key={v.id}
                    v={v}
                    hit={hits?.[v.id]}
                    nowMs={nowMs}
                    onOpen={onOpen}
                    onRevealTask={onRevealTask}
                  />
                ))}
              </List>
            )}
          </>
        )}
      </div>
    </aside>
  )
}

function SessionItem({
  v,
  hit,
  nowMs,
  onOpen,
  onRevealTask,
}: {
  v: SessionView
  hit: string | undefined
  nowMs: number
  onOpen: (id: string) => void
  onRevealTask: ((task: string) => void) | undefined
}) {
  // The second line says what the conversation is about: the task a run was on — a link to its
  // card — else what it was first asked. A transcript snippet goes under it, as a quotation.
  const task = v.task
  const taskText = task === null ? null : `${task}${v.taskTitle !== null ? ` · ${v.taskTitle}` : ''}`
  const about =
    task === null || taskText === null ? (
      v.prompt
    ) : onRevealTask === undefined ? (
      taskText
    ) : (
      <Button
        variant="link"
        size="sm"
        data-audit="sessionsTask"
        title={`Open ${task}`}
        onClick={() => onRevealTask(task)}
      >
        {taskText}
      </Button>
    )
  // The role is already the title of a run's row, so the facts are where and on what.
  const facts = [harnessLabel(v.harness), v.branch]
    .filter((s): s is string => s !== null && s !== '')
    .join(' · ')
  return (
    <ListItem
      top={
        <span className={styles.top} data-audit="sessionRow" data-kind={v.kind} data-live={v.live}>
          <Badge tone={RUN_KINDS.has(v.kind) ? 'purple' : 'neutral'} soft>
            {kindLabel(v.kind)}
          </Badge>
          {v.live && <Dot tone="green" label="Running now" />}
          {!v.live && v.pane !== null && <span className={styles.onScreen}>open</span>}
        </span>
      }
      topAside={<span title={new Date(v.lastSeenMs).toLocaleString()}>{ago(nowMs, v.lastSeenMs)}</span>}
      title={
        <span className={styles.title} data-audit="sessionTitle">
          {displayTitle(v)}
        </span>
      }
      meta={
        about === null && hit === undefined ? undefined : (
          <>
            {about !== null && <span className={styles.about}>{about}</span>}
            {hit !== undefined && (
              <span className={styles.snippet} data-audit="sessionsHit">
                “{hit}”
              </span>
            )}
          </>
        )
      }
      foot={
        <span className={styles.facts}>
          {facts}
          <Code>{v.id.length > 12 ? `${v.id.slice(0, 8)}…` : v.id}</Code>
          <span className={styles.open}>
            <Button
              size="sm"
              variant="secondary"
              data-audit="sessionsOpen"
              title={
                v.pane !== null
                  ? 'Go to the pane showing this conversation'
                  : v.live
                    ? 'Watch this running session'
                    : 'Resume this conversation in its harness'
              }
              onClick={() => onOpen(v.id)}
            >
              {v.pane !== null ? 'Show' : 'Open'}
            </Button>
          </span>
        </span>
      }
    />
  )
}
