/**
 * The whole output of a check — a milestone's gate or verify on a task's branch. (M83)
 *
 * The panel and the card keep sixty lines of tail, which is where a verdict is; this is for
 * drilling in. Read from the log cide writes as the check runs (`cide_core::check::run_logged`,
 * colour and all), decorated into the GitLab job-log dialect (`checkLogModel.ts`: each step a
 * section, verdict lines coloured) and drawn by that viewer — one renderer for every log in cide.
 *
 * While the check runs the view re-reads the file every second and follows the end — unless the
 * reader has scrolled up, which is them reading, and must not be yanked back down.
 */
import { useEffect, useMemo, useRef, useState } from 'react'

import { JobLog } from '@/gitlab/JobLog'
import gitlabStyles from '@/gitlab/GitLab.module.css'
import { milestones as milestonesApi, type ProjectId } from '@/ipc/client'
import { errorText } from '@/ipc/errorText'
import { OverlayCard } from '@/overlays/ModalShell'

import { decorateCheckLog } from './checkLogModel'
import styles from './MilestonesPanel.module.css'
import panelStyles from './TasksPanel.module.css'

export interface CheckLogTarget {
  kind: 'gate' | 'verify'
  key: string
  title: string
}

/** A check's log, read (and re-read while `running`) and drawn. The body of the modal and tab. */
export function CheckLogView({
  project,
  target,
  running,
}: {
  project: ProjectId
  target: CheckLogTarget
  running: boolean
}) {
  const [text, setText] = useState<string | null | undefined>(undefined)
  const [error, setError] = useState<string | null>(null)
  const [search, setSearch] = useState('')
  const box = useRef<HTMLDivElement | null>(null)
  const follow = useRef(true)

  useEffect(() => {
    let live = true
    const read = () =>
      void milestonesApi
        .checkLog(project, target.kind, target.key)
        .then((next) => {
          if (live) setText(next)
        })
        .catch((e: unknown) => {
          if (live) setError(errorText(e))
        })
    read()
    // Re-read while it runs; the effect re-runs once `running` flips, which is the final read
    // that catches the last lines and the exit status.
    const timer = running ? setInterval(read, 1000) : undefined
    return () => {
      live = false
      if (timer !== undefined) clearInterval(timer)
    }
  }, [project, target.kind, target.key, running])

  const decorated = useMemo(() => (text == null ? null : decorateCheckLog(text)), [text])

  useEffect(() => {
    const el = box.current
    if (el !== null && follow.current) el.scrollTop = el.scrollHeight
  }, [decorated])

  if (error !== null) return <p className={styles.error}>{error}</p>
  if (text === undefined) return <p className={styles.quiet}>Reading…</p>
  if (decorated === null) {
    return (
      <p className={styles.quiet}>
        There is no log for this check on this machine yet — it is written the next time the check
        runs.
      </p>
    )
  }
  return (
    <div className={styles.logWrap}>
      <input
        className={styles.logSearch}
        type="text"
        placeholder="Filter lines"
        aria-label="Filter log lines"
        value={search}
        onChange={(e) => setSearch(e.target.value)}
      />
      <div
        ref={box}
        className={`${styles.log} ${gitlabStyles.review ?? ''}`}
        data-audit="checkLog"
        onScroll={(e) => {
          const el = e.currentTarget
          follow.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24
        }}
      >
        <JobLog text={decorated} search={search} />
      </div>
    </div>
  )
}

export function CheckLogModal({
  project,
  target,
  running,
  onClose,
}: {
  project: ProjectId
  target: CheckLogTarget
  running: boolean
  onClose: () => void
}) {
  return (
    <OverlayCard label={target.title} onDismiss={onClose}>
      <div className={`${styles.modal} ${styles.wide}`} data-audit="checkLogModal">
        <h2 className={styles.modalTitle}>
          {target.title}
          {running ? ' · running' : ''}
        </h2>
        <CheckLogView project={project} target={target} running={running} />
        <div className={styles.modalActions}>
          <span className={styles.spacer} />
          <button type="button" className={panelStyles.action} onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </OverlayCard>
  )
}
