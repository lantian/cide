/**
 * One proposal, for the user to accept or reject. (M83)
 *
 * An agent proposed a change to what only the user may change — the milestones, or a guarded file
 * such as a gate script. This draws the whole of it: why (the agent's rationale, as markdown) and
 * what (the plan before and after, or each file's diff), and the two answers. Accept applies it
 * exactly as drawn — Rust refuses when a file has moved since, so the diff read is the change
 * made — and Reject drops it. Either is noted on the task it came from.
 */
import { useState } from 'react'

import {
  milestones as milestonesApi,
  type Milestone,
  type MilestonePlan,
  type ProjectId,
  type Proposal,
} from '@/ipc/client'
import { errorText } from '@/ipc/errorText'
import { OverlayCard } from '@/overlays/ModalShell'
import { useMilestones } from '@/sidebar/milestonesStore'

import styles from './MilestonesPanel.module.css'
import panelStyles from './TasksPanel.module.css'
import { TaskMarkdown } from './TaskMarkdown'

export function ProposalModal({
  project,
  proposal,
  onClose,
}: {
  project: ProjectId
  proposal: Proposal
  onClose: () => void
}) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [armedReject, setArmedReject] = useState(false)

  const answer = (accept: boolean) => {
    setBusy(true)
    setError(null)
    const call = accept
      ? milestonesApi.acceptProposal(project, proposal.id)
      : milestonesApi.rejectProposal(project, proposal.id)
    void call
      .then((view) => {
        useMilestones.getState().adopt(view)
        onClose()
      })
      .catch((e: unknown) => {
        setBusy(false)
        setError(errorText(e))
      })
  }

  const change = proposal.change
  return (
    <OverlayCard label={`Proposal ${proposal.id}`} onDismiss={busy ? () => {} : onClose}>
      <div className={styles.modal} data-audit="proposalModal">
        <h2 className={styles.modalTitle}>
          {proposal.id} · {proposal.title}
        </h2>
        <p className={styles.quiet}>
          Proposed by {byLabel(proposal.by)} · {new Date(proposal.createdUnixMs).toLocaleString()}
          {proposal.task !== undefined ? ` · from ${proposal.task}` : ''} ·{' '}
          {change.kind === 'plan'
            ? 'changes the milestones'
            : change.kind === 'files'
              ? `changes ${change.files.map((f) => f.path).join(', ')}`
              : 'a note — accepting it only acknowledges it'}
        </p>
        <div className={styles.rationale}>
          <TaskMarkdown text={proposal.rationale} />
        </div>

        {change.kind === 'files' &&
          change.files.map((f) => (
            <div key={f.path} className={styles.diffBlock}>
              <div className={styles.resultHead}>
                {f.path}
                {f.content === undefined ? ' — deleted' : f.before === undefined ? ' — new file' : ''}
              </div>
              <pre className={styles.diff}>
                {f.diff.split('\n').map((line, i) => (
                  <span
                    key={i}
                    className={
                      line.startsWith('+') && !line.startsWith('+++')
                        ? styles.diffAdd
                        : line.startsWith('-') && !line.startsWith('---')
                          ? styles.diffDel
                          : line.startsWith('@@')
                            ? styles.diffHunk
                            : undefined
                    }
                  >
                    {line}
                    {'\n'}
                  </span>
                ))}
              </pre>
            </div>
          ))}

        {change.kind === 'plan' && <PlanDiff before={change.before} after={change.plan} />}

        {error !== null && <p className={styles.error}>{error}</p>}
        <div className={styles.modalActions}>
          <button
            type="button"
            className={armedReject ? `${panelStyles.action} ${styles.danger}` : panelStyles.action}
            disabled={busy}
            onClick={() => {
              if (!armedReject) {
                setArmedReject(true)
                return
              }
              answer(false)
            }}
          >
            {armedReject ? 'Reject — sure?' : 'Reject'}
          </button>
          <span className={styles.spacer} />
          <button type="button" className={panelStyles.action} disabled={busy} onClick={onClose}>
            Later
          </button>
          <button
            type="button"
            className={`${panelStyles.action} ${panelStyles.actionPrimary}`}
            disabled={busy}
            onClick={() => answer(true)}
          >
            {change.kind === 'note' ? 'Acknowledge' : 'Accept and apply'}
          </button>
        </div>
      </div>
    </OverlayCard>
  )
}

/** The plan, line by line: each milestone and setting that differs, before → after. */
function PlanDiff({ before, after }: { before: MilestonePlan; after: MilestonePlan }) {
  const rows: Array<{ key: string; kind: 'add' | 'del' | 'same' | 'change'; text: string }> = []
  const line = (m: Milestone) => `${m.id} — ${m.title || m.id} — gate: ${m.gate}`
  for (const m of after.items) {
    const old = before.items.find((b) => b.id === m.id)
    if (old === undefined) rows.push({ key: `+${m.id}`, kind: 'add', text: line(m) })
    else if (old.title !== m.title || old.gate !== m.gate || old.timeoutSecs !== m.timeoutSecs) {
      rows.push({ key: `-${m.id}`, kind: 'del', text: line(old) })
      rows.push({ key: `+${m.id}`, kind: 'add', text: line(m) })
    } else rows.push({ key: m.id, kind: 'same', text: line(m) })
  }
  for (const m of before.items) {
    if (!after.items.some((a) => a.id === m.id)) {
      rows.push({ key: `-${m.id}`, kind: 'del', text: line(m) })
    }
  }
  const setting = (name: string, a: string, b: string) => {
    if (a === b) return
    rows.push({ key: `-${name}`, kind: 'del', text: `${name}: ${a || '—'}` })
    rows.push({ key: `+${name}`, kind: 'add', text: `${name}: ${b || '—'}` })
  }
  setting('verify', before.verify, after.verify)
  setting('guarded', before.guardPaths.join(', '), after.guardPaths.join(', '))
  setting('open-task limit', String(before.maxOpen ?? 12), String(after.maxOpen ?? 12))
  return (
    <pre className={styles.diff}>
      {rows.map((r) => (
        <span
          key={r.key}
          className={r.kind === 'add' ? styles.diffAdd : r.kind === 'del' ? styles.diffDel : undefined}
        >
          {r.kind === 'add' ? '+ ' : r.kind === 'del' ? '- ' : '  '}
          {r.text}
          {'\n'}
        </span>
      ))}
    </pre>
  )
}

function byLabel(by: Proposal['by']): string {
  if (by.kind === 'user') return 'you'
  if (by.kind === 'orchestrator') return 'the orchestrator'
  return by.label || String(by.agent)
}
