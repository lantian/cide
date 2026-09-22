import { Icon } from '@/icons/Icon'
import type { Approval } from './types'
import { approvalStatus } from './model'
import styles from './ReviewChrome.module.css'

export function MRState({
  state,
  draft = false,
}: {
  state: string
  draft?: boolean
}) {
  const label =
    state === 'opened'
      ? draft
        ? 'Draft'
        : 'Open'
      : state === 'merged'
        ? 'Merged'
        : 'Closed'
  return (
    <span
      className={styles.state}
      data-state={draft && state === 'opened' ? 'draft' : state}
    >
      <Icon
        name={
          state === 'merged'
            ? 'git-branch'
            : state === 'closed'
              ? 'circle-x'
              : draft
                ? 'circle-dashed'
                : 'circle-dot'
        }
        size={1}
      />
      {label}
    </span>
  )
}

export function ApprovalStatus({
  approval,
  stale,
  error,
}: {
  approval: Approval | null
  stale?: boolean
  error?: string | null
}) {
  const status = approvalStatus(approval, stale)
  return (
    <div
      className={styles.approval}
      data-status={status.kind}
      role="status"
      aria-label="Overall MR approval status"
      title={error ?? undefined}
    >
      <Icon
        name={
          status.kind === 'approved'
            ? 'circle-check'
            : status.kind === 'pending'
              ? 'circle-dashed'
              : status.kind === 'optional'
                ? 'circle-minus'
                : 'circle-alert'
        }
      />
      <div>
        <strong>{status.title}</strong>
        <span>{status.detail}</span>
      </div>
    </div>
  )
}
