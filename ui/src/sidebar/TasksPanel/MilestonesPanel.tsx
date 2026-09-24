/**
 * The Tasks panel's Milestones tab. (M83)
 *
 * A project's ordered goals — each with a **gate**, a command cide runs whose exit 0 means *met* —
 * and the checks around them: `verify` (run on an agent's branch before it may be merged), the
 * guarded paths (files a gate reads, which no agent branch may change) and the open-task limit.
 * All of it is `.cide/config.json`'s `milestones` key.
 *
 * # Why here and not in Settings
 *
 * It was a block in Settings → Agents first, and that was the wrong home: Settings is global and
 * this belongs to one repository, beside the board whose work it points. The OpenSpec config
 * modal off its panel header made the same move for the same reason.
 *
 * # A list, and the contents in modals
 *
 * The sidebar is narrow and a gate command is long; a form drawn inline would be a column of
 * truncated inputs. The list says what matters at a glance — which milestone is active, whether
 * its gate passes — and a click opens the whole of one thing to edit.
 *
 * # Whole-value writes, and the draft stays in the modal
 *
 * `milestones.set` replaces the plan, and a modal edits a **draft** that is sent only on Save —
 * so a refused write (two milestones with one id) leaves the modal open with the sentence in it,
 * and the list keeps drawing what Rust stored (ADR 0002). The inputs are controlled rather than
 * Settings' blur-commit fields: a blur that commits races the Save click that caused it, and the
 * click would send the draft from before the last keystroke.
 */
import { useCallback, useEffect, useState, type ReactNode } from 'react'

import { followMilestones, useMilestones, type TasksTab } from '@/sidebar/milestonesStore'
import { useTasks } from '@/sidebar/tasksStore'

import {
  milestones as milestonesApi,
  type CheckResult,
  type Milestone,
  type MilestonePlan,
  type MilestoneTask,
  type ProjectId,
} from '@/ipc/client'
import { errorText } from '@/ipc/errorText'
import { OverlayCard } from '@/overlays/ModalShell'
import { Icon, asIcon } from '@/icons/Icon'
import fieldStyles from '@/settings/controls.module.css'

import { CheckLogModal, CheckLogView, type CheckLogTarget } from './CheckLogModal'
import { useEscapeClose } from './escapeClose'
import { TONE_CLASS, cx } from './TaskDetail'
import { statusGlyph, statusTone } from './model'
import { ProposalModal } from './ProposalModal'
import panelStyles from './TasksPanel.module.css'
import styles from './MilestonesPanel.module.css'


/**
 * The two tabs, drawn where the header says *Tasks*, and — beside them — what cide is checking
 * right now (M83): a gate after a merge, verify on a run's branch. Both run for minutes with
 * nothing else on screen saying so, and a project that looks idle while its gate runs invites
 * somebody to start work the gate is about to judge.
 */
export function TasksTabs({
  tab,
  onTab,
  gatesRunning = [],
  verifying = 0,
  verifyingTasks = [],
  project = null,
  proposals = 0,
}: {
  tab: TasksTab
  onTab: (tab: TasksTab) => void
  gatesRunning?: readonly string[]
  verifying?: number
  /** Which tasks verify is running on, so a click can open one's log. */
  verifyingTasks?: readonly string[]
  project?: ProjectId | null
  /** Proposals waiting for the user, counted on the Milestones tab's label. */
  proposals?: number
}) {
  const [log, setLog] = useState<CheckLogTarget | null>(null)
  const busy: string[] = []
  if (gatesRunning.length > 0) busy.push(`gate ${gatesRunning.join(', ')}`)
  if (verifying > 0) busy.push(verifying === 1 ? 'verify' : `verify ×${verifying}`)
  return (
    <span className={styles.tabs} role="tablist" aria-label="Tasks panel">
      {(['tasks', 'milestones'] as const).map((t) => (
        <button
          key={t}
          type="button"
          role="tab"
          aria-selected={tab === t}
          className={tab === t ? `${styles.tab} ${styles.tabOn}` : styles.tab}
          data-audit="tasksTab"
          data-tab={t}
          onClick={() => onTab(t)}
        >
          {t === 'tasks' ? 'Tasks' : 'Milestones'}
          {t === 'milestones' && proposals > 0 && (
            <span
              className={styles.tabCount}
              data-audit="proposalsCount"
              title={`${proposals} proposal${proposals === 1 ? '' : 's'} waiting for you`}
            >
              {proposals}
            </span>
          )}
        </button>
      ))}
      {busy.length > 0 && (
        <button
          type="button"
          className={styles.busy}
          data-audit="tasksChecking"
          title={
            gatesRunning.length > 0
              ? `The gate of ${gatesRunning[0]} is running — open the milestone on its log`
              : `verify is running on ${verifyingTasks[0] ?? 'a branch'} — watch its log`
          }
          onClick={() => {
            // The log of what is running: the gate first (it judges the milestone), then the
            // first task being verified. (M83)
            // A gate opens its milestone's own modal on the Log tab — one window per milestone,
            // not a second one showing part of it. Verify belongs to a task, so it keeps the log
            // window.
            const gate = gatesRunning[0]
            const task = verifyingTasks[0]
            if (gate !== undefined) useMilestones.getState().reveal(gate, 'log')
            else if (task !== undefined) setLog({ kind: 'verify', key: task, title: `Verify of ${task}` })
          }}
        >
          <span className={styles.spin} aria-hidden="true">
            <Icon name={asIcon('loader-circle')} size={0} />
          </span>
          {busy.join(' · ')}
        </button>
      )}
      {log !== null && project !== null && (
        <CheckLogModal
          project={project}
          target={log}
          running={
            log.kind === 'gate' ? gatesRunning.includes(log.key) : verifyingTasks.includes(log.key)
          }
          onClose={() => setLog(null)}
        />
      )}
    </span>
  )
}

type Editing =
  | { kind: 'milestone'; index: number | null; tab?: 'overview' | 'log' }
  | { kind: 'checks' }
  | null

export function MilestonesPanel({
  project,
  title,
}: {
  project: ProjectId | null
  title: ReactNode
}) {
  // The store the task card reads too (`milestonesStore`), so the two cannot disagree.
  useEffect(() => followMilestones(), [])
  const view = useMilestones((state) => (state.project === project ? state.view : null))
  const storeError = useMilestones((state) => state.error)
  const [actionError, setError] = useState<string | null>(null)
  const error = actionError ?? storeError
  const [editing, setEditing] = useState<Editing>(null)
  const [openProposal, setOpenProposal] = useState<string | null>(null)
  const setView = useMilestones.getState().adopt

  /** Send a whole plan; resolves with the error sentence, or `null` when it was stored. */
  const write = useCallback(
    async (plan: MilestonePlan): Promise<string | null> => {
      if (project === null) return 'no project is open'
      try {
        setView(await milestonesApi.set(project, plan))
        return null
      } catch (e: unknown) {
        return errorText(e)
      }
    },
    [project],
  )

  const plan = view?.plan ?? null
  const current = plan === null ? null : currentOf(plan)

  /*
   * A task card asked to see a milestone (its chip). Opened once the plan is in, then the request
   * is cleared — an unknown id (a milestone removed meanwhile) is cleared too, leaving the list.
   */
  const focus = useMilestones((state) => state.focus)
  useEffect(() => {
    if (focus === null || plan === null) return
    const index = plan.items.findIndex((m) => m.id === focus.milestone)
    if (index >= 0) setEditing({ kind: 'milestone', index, tab: focus.tab })
    useMilestones.getState().clearFocus()
  }, [focus, plan])
  const gateOf = (id: string) => view?.gates.find((g) => g.milestone === id)
  const tasksOf = (id: string): readonly MilestoneTask[] =>
    view?.tasks.find((t) => t.milestone === id)?.tasks ?? NO_TASKS
  const currentGate = current === null ? undefined : gateOf(current.id)

  const runGate = () => {
    if (project === null) return
    void milestonesApi.runGate(project).catch((e: unknown) => setError(errorText(e)))
  }
  /** Accept the current milestone; resolves with the error sentence, or `null` when accepted. */
  const acceptCurrent = useCallback(async (): Promise<string | null> => {
    if (project === null) return 'no project is open'
    try {
      setView(await milestonesApi.accept(project))
      return null
    } catch (e: unknown) {
      return errorText(e)
    }
  }, [project])
  const accept = () => {
    void acceptCurrent().then((refused) => {
      if (refused !== null) setError(refused)
    })
  }

  return (
    <aside className={panelStyles.panel} data-audit="sidebarMilestones" aria-label="Milestones">
      <div className={panelStyles.header} data-audit="tasksHeader">
        <span className={panelStyles.headerTitle}>{title}</span>
        <span className={panelStyles.headerMeta}>
          {plan === null || plan.items.length === 0
            ? ''
            : `${view?.accepted.length ?? 0}/${plan.items.length}`}
        </span>
      </div>
      <div className={panelStyles.body} data-audit="milestonesBody">
        {project === null ? (
          <p className={styles.quiet}>Open a project to see its milestones.</p>
        ) : plan === null ? (
          error === null ? null : <p className={styles.error}>{error}</p>
        ) : (
          <>
            {/*
              * The tab's own actions lead it, as the Tasks tab's do: they act on the whole plan,
              * and under a long list they were a scroll away. Accept is not among them — it is
              * about one milestone, so it sits on that milestone's card, and only once there is
              * something to accept (the gate passes). Offered before that, it read as the next
              * step and let a click skip the gate it exists to respect.
              */}
            <div className={`${panelStyles.actions} ${styles.topActions}`}>
              <button
                type="button"
                className={`${panelStyles.action} ${panelStyles.actionPrimary}`}
                onClick={() => setEditing({ kind: 'milestone', index: null })}
              >
                Add milestone
              </button>
              {current !== null && (
                <button
                  type="button"
                  className={panelStyles.action}
                  disabled={currentGate?.running === true}
                  onClick={runGate}
                >
                  Run gate
                </button>
              )}
            </div>
            {/*
              * Proposals before the milestones (M83): they are the one thing on this tab waiting on the user, and
              * an agent that proposed a gate change is working around it until they answer.
              */}
            {(view?.proposals.length ?? 0) > 0 && (
              <>
                <h3 className={styles.section}>Proposals · {view?.proposals.length}</h3>
                <ol className={styles.list} data-audit="proposals">
                  {view?.proposals.map((p) => (
                    <li key={p.id}>
                      <button
                        type="button"
                        className={`${styles.row} ${styles.proposalRow}`}
                        data-audit="proposalRow"
                        data-kind={p.change.kind}
                        onClick={() => setOpenProposal(p.id)}
                      >
                        <span className={styles.rowHead}>
                          <span className={styles.badge}>
                            {p.change.kind === 'plan'
                              ? 'Milestones'
                              : p.change.kind === 'files'
                                ? 'Files'
                                : 'Note'}
                          </span>
                          <span className={styles.rowId}>{p.id}</span>
                          {p.task !== undefined && <span className={styles.rowTask}>{p.task}</span>}
                        </span>
                        <span className={styles.rowTitle}>{p.title}</span>
                        {p.change.kind === 'files' && (
                          <span className={styles.rowCount}>
                            {p.change.files.map((f) => f.path).join(', ')}
                          </span>
                        )}
                      </button>
                    </li>
                  ))}
                </ol>
                <h3 className={styles.section}>Milestones</h3>
              </>
            )}
            {plan.items.length === 0 ? (
              <p className={styles.quiet}>
                No milestones. A milestone is a goal with a gate — a command cide runs in the
                project root, where exit 0 means it is met. Agents then plan only towards the
                active one.
              </p>
            ) : (
              <ol className={styles.list}>
                {plan.items.map((m, index) => {
                  const accepted = view?.accepted.includes(m.id) === true
                  const active = current?.id === m.id && !accepted
                  const gate = gateOf(m.id)
                  // `accept` takes no id: Rust accepts the current milestone, so only the
                  // active card can offer it, and only once its gate has passed.
                  const ready = readyToAccept(active, gate?.running === true, gate?.last, tasksOf(m.id))
                  return (
                    <li
                      key={`${index}:${m.id}`}
                      className={styles.card}
                      data-state={accepted ? 'accepted' : active ? 'active' : 'later'}
                    >
                      <button
                        type="button"
                        className={styles.row}
                        data-audit="milestoneRow"
                        data-state={accepted ? 'accepted' : active ? 'active' : 'later'}
                        onClick={() => setEditing({ kind: 'milestone', index })}
                      >
                        <span className={styles.rowHead}>
                          <span className={styles.badge}>
                            {accepted ? 'Accepted' : active ? 'Active' : 'Later'}
                          </span>
                          <span className={styles.rowId}>{m.id}</span>
                          {m.task !== undefined && <span className={styles.rowTask}>{m.task}</span>}
                        </span>
                        <span className={styles.rowTitle}>{m.title || m.id}</span>
                        <span className={styles.rowCount}>{countLine(tasksOf(m.id))}</span>
                        <span
                          className={styles.rowGate}
                          data-verdict={verdictKind(gate?.running === true, gate?.last)}
                        >
                          {gate?.running === true && (
                            <span className={styles.spin} aria-hidden="true">
                              <Icon name={asIcon('loader-circle')} size={0} />
                            </span>
                          )}
                          {gateLine(gate?.running === true, gate?.last)}
                        </span>
                      </button>
                      {ready && (
                        <div className={styles.cardFoot}>
                          <button
                            type="button"
                            className={`${panelStyles.action} ${panelStyles.actionPrimary}`}
                            data-audit="milestoneAccept"
                            onClick={accept}
                          >
                            Accept
                          </button>
                        </div>
                      )}
                    </li>
                  )
                })}
              </ol>
            )}

            {current !== null &&
              currentGate?.last?.passed === true &&
              view?.accepted.includes(current.id) !== true && (
                <p className={styles.note}>
                  {gateNote(current, currentGate?.running === true, currentGate?.last, tasksOf(current.id))}
                </p>
              )}

            <h3 className={styles.section}>Checks</h3>
            <button
              type="button"
              className={styles.row}
              data-audit="milestoneChecks"
              onClick={() => setEditing({ kind: 'checks' })}
            >
              <span className={styles.kv}>
                <span className={styles.k}>Verify</span>
                <span className={styles.v}>{plan.verify || 'none'}</span>
              </span>
              <span className={styles.kv}>
                <span className={styles.k}>Guarded</span>
                <span className={styles.v}>
                  {plan.guardPaths.length === 0 ? 'nothing' : plan.guardPaths.join(', ')}
                </span>
              </span>
              <span className={styles.kv}>
                <span className={styles.k}>Open tasks</span>
                <span className={styles.v}>{plan.maxOpen ?? 12} per milestone</span>
              </span>
            </button>
            {error !== null && <p className={styles.error}>{error}</p>}
          </>
        )}
      </div>

      {plan !== null && editing?.kind === 'milestone' && (
        <MilestoneModal
          /* Keyed on the tab asked for, so a request for the log reaches a modal already open. */
          key={`${editing.index ?? 'new'}:${editing.tab ?? 'overview'}`}
          initialTab={editing.tab}
          plan={plan}
          index={editing.index}
          gate={
            editing.index === null ? undefined : gateOf(plan.items[editing.index]?.id ?? '')?.last
          }
          gateRunning={
            editing.index !== null && gateOf(plan.items[editing.index]?.id ?? '')?.running === true
          }
          project={project}
          accepted={
            editing.index !== null &&
            view?.accepted.includes(plan.items[editing.index]?.id ?? '') === true
          }
          active={editing.index !== null && current?.id === plan.items[editing.index]?.id}
          tasks={editing.index === null ? NO_TASKS : tasksOf(plan.items[editing.index]?.id ?? '')}
          onWrite={write}
          onAccept={acceptCurrent}
          onClose={() => setEditing(null)}
        />
      )}
      {project !== null &&
        openProposal !== null &&
        (() => {
          const proposal = view?.proposals.find((p) => p.id === openProposal)
          return proposal === undefined ? null : (
            <ProposalModal
              project={project}
              proposal={proposal}
              onClose={() => setOpenProposal(null)}
            />
          )
        })()}
      {plan !== null && editing?.kind === 'checks' && (
        <ChecksModal plan={plan} onWrite={write} onClose={() => setEditing(null)} />
      )}
    </aside>
  )
}

/* ------------------------------------------------------------------------------ the modals */

function MilestoneModal({
  plan,
  index,
  gate,
  accepted,
  active,
  tasks,
  gateRunning,
  project,
  initialTab,
  onWrite,
  onAccept,
  onClose,
}: {
  plan: MilestonePlan
  index: number | null
  gate: CheckResult | undefined
  gateRunning: boolean
  /** Open on the log — the header's "gate running" asks for this. */
  initialTab?: 'overview' | 'log' | undefined
  project: ProjectId | null
  accepted: boolean
  active: boolean
  tasks: readonly MilestoneTask[]
  onWrite: (plan: MilestonePlan) => Promise<string | null>
  /** Accept the current milestone — offered only when this one is it and its gate passed. */
  onAccept: () => Promise<string | null>
  onClose: () => void
}) {
  const original: Milestone =
    index === null ? { id: freshId(plan.items), title: '', gate: '' } : (plan.items[index] as Milestone)
  const [id, setId] = useState(original.id)
  const [title, setTitle] = useState(original.title)
  const [gateCmd, setGateCmd] = useState(original.gate)
  const [minutes, setMinutes] = useState(String(Math.round((original.timeoutSecs ?? 1800) / 60)))
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [armedRemove, setArmedRemove] = useState(false)

  const send = async (next: MilestonePlan) => {
    setBusy(true)
    const refused = await onWrite(next)
    setBusy(false)
    if (refused === null) onClose()
    else setError(refused)
  }

  // The card's rule (see the list): only the active milestone, only once its gate has passed.
  // The modal stays open on accept — the title turns to "accepted" and the button goes, which
  // says it happened; closing would leave the user to find that out from the list.
  const ready = index !== null && !accepted && readyToAccept(active, gateRunning, gate, tasks)
  const accept = async () => {
    setBusy(true)
    const refused = await onAccept()
    setBusy(false)
    setError(refused)
  }

  const edited = (): Milestone => {
    const mins = Number.parseInt(minutes, 10)
    const base: Milestone = { ...original, id: id.trim(), title: title.trim(), gate: gateCmd.trim() }
    if (Number.isFinite(mins) && mins > 0) base.timeoutSecs = mins * 60
    return base
  }

  const save = () => {
    const m = edited()
    if (m.gate === '') {
      setError('A milestone needs a gate: a milestone nothing can check is a wish.')
      return
    }
    const items =
      index === null ? [...plan.items, m] : plan.items.map((x, i) => (i === index ? m : x))
    // A renamed active milestone keeps being the active one.
    const nextActive = plan.active !== undefined && plan.active === original.id ? m.id : plan.active
    void send(withActive({ ...plan, items }, nextActive))
  }

  const move = (by: -1 | 1) => {
    if (index === null) return
    const items = [...plan.items]
    const [taken] = items.splice(index, 1)
    if (taken === undefined) return
    items.splice(index + by, 0, taken)
    void send({ ...plan, items })
  }

  const dismiss = busy ? () => {} : onClose
  const escape = useEscapeClose(dismiss)

  // Three tabs (M83): the log drawn under the form ran into it, and a long task list pushed the
  // actions off the card. A new milestone has nothing to list and nothing logged — one tab.
  const [tab, setTab] = useState<'overview' | 'tasks' | 'log'>(
    initialTab === 'log' && index !== null ? 'log' : 'overview',
  )

  return (
    <OverlayCard
      label={index === null ? 'New milestone' : `Milestone ${original.id}`}
      onDismiss={dismiss}
    >
      <div className={`${styles.modal} ${styles.wide}`} data-audit="milestoneModal" {...escape}>
        <h2 className={styles.modalTitle}>
          {index === null ? 'New milestone' : `Milestone ${original.id}`}
          {accepted ? ' · accepted' : active ? ' · active' : ''}
        </h2>
        {index !== null && (
          <nav className={styles.modalTabs} aria-label="Milestone sections">
            {(
              [
                ['overview', 'Overview'],
                ['tasks', `Tasks · ${countLine(tasks)}`],
                ['log', gateRunning ? 'Log · running' : 'Log'],
              ] as const
            ).map(([key, label]) => (
              <button
                key={key}
                type="button"
                className={styles.modalTab}
                aria-pressed={tab === key}
                data-audit="milestoneTab"
                data-tab={key}
                onClick={() => setTab(key)}
              >
                {key === 'log' && gateRunning && (
                  <span className={styles.spin} aria-hidden="true">
                    <Icon name={asIcon('loader-circle')} size={0} />
                  </span>
                )}
                {label}
              </button>
            ))}
          </nav>
        )}

        {tab === 'overview' && (
          <>
            <Field label="Id" value={id} onChange={setId} hint="A short handle, e.g. slice or p2." />
            <Field label="Title" value={title} onChange={setTitle} />
            <Field
              label="Gate"
              value={gateCmd}
              onChange={setGateCmd}
              mono
              placeholder="tools/ci/milestone.sh slice"
              hint="Run with sh -c in the project root. Exit 0 means the milestone is met."
            />
            <Field label="Time limit, minutes" value={minutes} onChange={setMinutes} mono />
            {index === null && (
              <p className={styles.quiet}>Saving creates the milestone's task on the board.</p>
            )}
            {index !== null && (gate !== undefined || gateRunning) && (
              <button
                type="button"
                className={styles.result}
                data-verdict={verdictKind(gateRunning, gate)}
                onClick={() => setTab('log')}
              >
                <span className={styles.resultHead}>
                  {gateRunning ? 'Gate is running now' : `Last gate: ${gateLine(false, gate)}`}
                </span>
                <span className={styles.taskLink}>
                  {gateRunning ? 'watch the log' : 'open the log'}
                </span>
              </button>
            )}
          </>
        )}

        {tab === 'tasks' && index !== null && (
          <div className={styles.taskTab} data-audit="milestoneTasks">
            {original.task !== undefined && (
              <p className={styles.quiet}>
                Work joins this milestone by being a subtask of{' '}
                <button
                  type="button"
                  className={styles.taskLink}
                  onClick={() => openTask(original.task as string, onClose)}
                >
                  {original.task}
                </button>
                , at any depth.
              </p>
            )}
            {tasks.length === 0 ? (
              <p className={styles.quiet}>Nothing is linked to this milestone yet.</p>
            ) : (
              <MilestoneTaskList tasks={tasks} onOpen={(t) => openTask(t, onClose)} />
            )}
          </div>
        )}

        {tab === 'log' && index !== null && project !== null && (
          <CheckLogView
            project={project}
            target={{ kind: 'gate', key: original.id, title: `Gate of ${original.id}` }}
            running={gateRunning}
          />
        )}

        {error !== null && <p className={styles.error}>{error}</p>}
        <div className={styles.modalActions}>
          {index !== null && tab === 'overview' && (
            <>
              <button type="button" className={panelStyles.action} disabled={busy || index === 0} onClick={() => move(-1)}>
                Earlier
              </button>
              <button
                type="button"
                className={panelStyles.action}
                disabled={busy || index === plan.items.length - 1}
                onClick={() => move(1)}
              >
                Later
              </button>
              {!active && !accepted && (
                <button
                  type="button"
                  className={panelStyles.action}
                  disabled={busy}
                  onClick={() => void send(withActive(plan, original.id))}
                >
                  Make active
                </button>
              )}
              <button
                type="button"
                className={armedRemove ? `${panelStyles.action} ${styles.danger}` : panelStyles.action}
                disabled={busy}
                onClick={() => {
                  if (!armedRemove) {
                    setArmedRemove(true)
                    return
                  }
                  void send({ ...plan, items: plan.items.filter((_, i) => i !== index) })
                }}
              >
                {armedRemove ? 'Remove — sure?' : 'Remove'}
              </button>
            </>
          )}
          <span className={styles.spacer} />
          {ready && (
            <button
              type="button"
              className={`${panelStyles.action} ${panelStyles.actionPrimary}`}
              data-audit="milestoneModalAccept"
              disabled={busy}
              onClick={() => void accept()}
            >
              Accept
            </button>
          )}
          <button type="button" className={panelStyles.action} disabled={busy} onClick={onClose}>
            {tab === 'overview' ? 'Cancel' : 'Close'}
          </button>
          {tab === 'overview' && (
            <button
              type="button"
              className={`${panelStyles.action} ${panelStyles.actionPrimary}`}
              disabled={busy}
              onClick={save}
            >
              Save
            </button>
          )}
        </div>
      </div>
    </OverlayCard>
  )
}

/**
 * A milestone's tasks, drawn exactly as the card's Links list draws a task — the same row, the
 * same status mark in the same tone — so a task looks like one thing wherever it is listed, and
 * todo and done are told apart at a glance. Nesting is indentation; the assignee sits at the end.
 */
function MilestoneTaskList({
  tasks,
  onOpen,
}: {
  tasks: readonly MilestoneTask[]
  onOpen: (task: string) => void
}) {
  return (
    <div className={panelStyles.linkList}>
      {tasks.map((t) => (
        <div
          key={t.id}
          className={panelStyles.linkRow}
          style={{ paddingLeft: `calc(${t.depth} * var(--sp-5))` }}
        >
          <button
            type="button"
            className={panelStyles.linkRowMain}
            data-audit="milestoneTask"
            data-status={t.status}
            title={`${t.id}: ${t.title}`}
            onClick={() => onOpen(t.id)}
          >
            <span
              className={cx(panelStyles.glyph, TONE_CLASS[statusTone(t.status)])}
              data-status={t.status}
              aria-hidden="true"
            >
              <Icon name={asIcon(statusGlyph(t.status))} size={1} />
            </span>
            <span className={panelStyles.taskId}>{t.id}</span>
            <span className={panelStyles.taskTitle}>{t.title}</span>
            {t.agent !== undefined && <span className={styles.taskAgent}>{t.agent}</span>}
          </button>
        </div>
      ))}
    </div>
  )
}

function ChecksModal({
  plan,
  onWrite,
  onClose,
}: {
  plan: MilestonePlan
  onWrite: (plan: MilestonePlan) => Promise<string | null>
  onClose: () => void
}) {
  const [verify, setVerify] = useState(plan.verify)
  const [guards, setGuards] = useState(plan.guardPaths.join('\n'))
  const [limit, setLimit] = useState(String(plan.maxOpen ?? 12))
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const save = async () => {
    const n = Number.parseInt(limit, 10)
    const next: MilestonePlan = {
      ...plan,
      verify: verify.trim(),
      guardPaths: guards
        .split('\n')
        .map((l) => l.trim())
        .filter((l) => l !== ''),
    }
    if (Number.isFinite(n) && n > 0) next.maxOpen = n
    setBusy(true)
    const refused = await onWrite(next)
    setBusy(false)
    if (refused === null) onClose()
    else setError(refused)
  }

  const dismiss = busy ? () => {} : onClose
  const escape = useEscapeClose(dismiss)

  return (
    <OverlayCard label="Project checks" onDismiss={dismiss}>
      <div className={styles.modal} data-audit="milestoneChecksModal" {...escape}>
        <h2 className={styles.modalTitle}>Project checks</h2>
        <Field
          label="Verify"
          value={verify}
          onChange={setVerify}
          mono
          placeholder="tools/dev/check.sh"
          hint="Run in an agent's checkout before its branch may be merged; exit 0 means the work is done. Empty means none. Your own Integrate is not gated."
        />
        <Field
          label="Guarded paths"
          value={guards}
          onChange={setGuards}
          mono
          multiline
          hint="One per line, relative to the project root; a directory ends in /. An agent branch that changes one is not merged — put the files your gates read here."
        />
        <Field
          label="Open tasks per milestone"
          value={limit}
          onChange={setLimit}
          mono
          hint="Past this many open tasks under the active milestone, new ones the orchestrator files go to the inbox."
        />
        {error !== null && <p className={styles.error}>{error}</p>}
        <div className={styles.modalActions}>
          <span className={styles.spacer} />
          <button type="button" className={panelStyles.action} disabled={busy} onClick={onClose}>
            Cancel
          </button>
          <button
            type="button"
            className={`${panelStyles.action} ${panelStyles.actionPrimary}`}
            disabled={busy}
            onClick={() => void save()}
          >
            Save
          </button>
        </div>
      </div>
    </OverlayCard>
  )
}

function Field({
  label,
  value,
  onChange,
  hint,
  placeholder,
  mono = false,
  multiline = false,
}: {
  label: string
  value: string
  onChange: (next: string) => void
  hint?: string
  placeholder?: string
  mono?: boolean
  multiline?: boolean
}) {
  const cls = mono ? styles.mono : undefined
  return (
    <label className={fieldStyles.field}>
      <span className={fieldStyles.fieldLabel}>{label}</span>
      {multiline ? (
        <textarea
          className={cls === undefined ? fieldStyles.fieldArea : `${fieldStyles.fieldArea} ${cls}`}
          rows={4}
          spellCheck={false}
          value={value}
          placeholder={placeholder ?? ''}
          onChange={(e) => onChange(e.target.value)}
        />
      ) : (
        <input
          type="text"
          className={cls === undefined ? fieldStyles.fieldInput : `${fieldStyles.fieldInput} ${cls}`}
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          value={value}
          placeholder={placeholder ?? ''}
          onChange={(e) => onChange(e.target.value)}
        />
      )}
      {hint !== undefined && <span className={fieldStyles.fieldHint}>{hint}</span>}
    </label>
  )
}

/* ------------------------------------------------------------------------------ the rules */

const NO_TASKS: readonly MilestoneTask[] = []

/** `3 open · 5 done`, or a sentence for none. */
function countLine(tasks: readonly MilestoneTask[]): string {
  if (tasks.length === 0) return 'no tasks'
  const done = tasks.filter((t) => t.status === 'done').length
  return `${tasks.length - done} open · ${done} done`
}

/**
 * Work still in flight under a milestone: todo, doing, review. The inbox is counted separately,
 * by `undecided` — it is by definition not work yet (`TaskStatus`'s account of it).
 */
function openWork(tasks: readonly MilestoneTask[]): number {
  return tasks.filter((t) => t.status !== 'done' && t.status !== 'inbox').length
}

/**
 * Decisions owed under a milestone: rows sitting in the inbox *under its goal*. (M99)
 *
 * These were not counted at all, on the reasoning that the inbox is not work and any passing
 * observation could otherwise block a milestone. Half right: the inbox at large is somebody
 * else's problem, and nothing here looks at it — `tasks` is only what is linked under this
 * goal. But a row a run put in the inbox *under this goal* is something noticed while working on
 * this milestone that nobody has ruled on, and Accept marks that goal done, burying it. So it
 * holds Accept, exactly as an open task does, and the way to clear it is one decision: move it
 * to todo, or unlink it from the goal. In selfcraft's `slice` this was the whole bug — gate
 * green, eighty subtasks done, one inbox row under the goal, and cide offering Accept while the
 * spinner stood down (`spinner::gate_blocks_planning` is the same rule on the Rust side).
 */
function undecided(tasks: readonly MilestoneTask[]): number {
  return tasks.filter((t) => t.status === 'inbox').length
}

/**
 * Whether Accept is offered — the card's and the modal's one rule. The active milestone only
 * (`accept` takes no id; Rust accepts the current one), its gate's last run passed and none is
 * running, and nothing under it is still open or still undecided. The gate alone was the first
 * version, and it offered Accept on a milestone with tasks in doing: a gate that passed an hour
 * ago says nothing about work started since, and accepting marks the milestone's task done over
 * its open subtasks — or, with `undecided`, over an inbox row nobody ruled on.
 */
function readyToAccept(
  active: boolean,
  gateRunning: boolean,
  gate: CheckResult | undefined,
  tasks: readonly MilestoneTask[],
): boolean {
  return (
    active &&
    !gateRunning &&
    gate?.passed === true &&
    openWork(tasks) === 0 &&
    undecided(tasks) === 0
  )
}

/**
 * The line drawn under a green gate. (M99)
 *
 * It used to say one thing — *cide will not wake this project to plan until you accept it* — and
 * that is only true of a milestone that is actually finished. A green gate over work still in
 * flight, or over a row a run left in this milestone's inbox, is a milestone cide goes on
 * planning for (`spinner::gate_blocks_planning`) and does not offer Accept on, so the old
 * sentence told the reader to look for a button that was not drawn and to expect a silence that
 * was not coming. Now it says which of the two it is, and when it is the second it names what is
 * in the way and the one move that clears it.
 */
function gateNote(
  current: Milestone,
  gateRunning: boolean,
  gate: CheckResult | undefined,
  tasks: readonly MilestoneTask[],
): string {
  if (readyToAccept(true, gateRunning, gate, tasks)) {
    return `The gate passes. cide will not wake this project to plan until you accept ${current.id} — or tighten its gate if this is not what you meant.`
  }
  const open = openWork(tasks)
  const owed = undecided(tasks)
  const waits: string[] = []
  if (open > 0) waits.push(`its ${open} open task${open === 1 ? '' : 's'} to be done`)
  if (owed > 0) {
    waits.push(
      `${owed} task${owed === 1 ? '' : 's'} in its inbox to be decided — move ${
        owed === 1 ? 'it' : 'each'
      } to todo if this milestone needs it, or unlink it from ${current.task ?? 'the goal'} if it does not`,
    )
  }
  const head = `The gate passes, but ${current.id} is not finished, so cide keeps planning rather than waiting on you.`
  return waits.length === 0 ? `${head} Accept is offered once the gate has finished running.` : `${head} Accept waits for ${waits.join(', and for ')}.`
}

/** Open a task's card — the same selection the board's rows make — and close this modal. */
function openTask(task: string, close: () => void): void {
  close()
  useTasks.getState().select(task)
}

/** The active milestone: the named one, or the first. `MilestonePlan::current`'s rule. */
function currentOf(plan: MilestonePlan): Milestone | null {
  const named = plan.active === undefined ? undefined : plan.items.find((m) => m.id === plan.active)
  return named ?? plan.items[0] ?? null
}

/** `active` set or removed, never written as `undefined` (the wire omits an absent key). */
function withActive(plan: MilestonePlan, active: string | undefined): MilestonePlan {
  const { active: _drop, ...rest } = plan
  return active === undefined ? rest : { ...rest, active }
}

function freshId(items: readonly Milestone[]): string {
  for (let n = items.length + 1; ; n += 1) {
    const id = `m${n}`
    if (!items.some((m) => m.id === id)) return id
  }
}

function verdictKind(running: boolean, last: CheckResult | undefined): string {
  if (running) return 'running'
  if (last === undefined) return 'none'
  return last.passed ? 'passed' : 'failed'
}

function gateLine(running: boolean, last: CheckResult | undefined): string {
  if (running) return 'gate running…'
  if (last === undefined) return 'gate not run yet'
  const when = new Date(last.startedUnixMs).toLocaleString()
  const took = `${Math.round(last.durationMs / 1000)}s`
  if (last.passed) return `gate passed · ${when} · ${took}`
  if (last.timedOut) return `gate timed out · ${when}`
  return `gate failed, exit ${last.exitCode ?? '?'} · ${when} · ${took}`
}

