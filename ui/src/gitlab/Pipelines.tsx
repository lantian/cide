import { JobLog } from './JobLog'
import { useEffect, useRef, useState } from 'react'
import { gitlab } from '@/ipc/client'
import { api, pages } from './store'
import { message } from './model'
import type { Pipeline, Job } from './types'
import styles from './GitLab.module.css'
export const active = (status: string) =>
  [
    'created',
    'waiting_for_resource',
    'preparing',
    'pending',
    'running',
    'scheduled',
  ].includes(status)
export interface Trace {
  state: 'available' | 'archived' | 'erased' | 'unavailable'
  text: string
  message: string | null
}
export function Status({ value }: { value: string }) {
  return (
    <span className={styles.badge} data-status={value}>
      {value.replaceAll('_', ' ')}
    </span>
  )
}
export function JobRow({
  job,
  selected,
  onSelect,
}: {
  job: Job
  selected: boolean
  onSelect: () => void
}) {
  return (
    <button className={styles.job} aria-pressed={selected} onClick={onSelect}>
      <span className={styles.jobName}>{job.name}</span>
      <Status value={job.status} />
      <span className={styles.muted}>
        {job.duration != null ? `${Math.round(job.duration)}s` : ''}
        {job.archived ? ' · archived' : ''}
      </span>
    </button>
  )
}
export function Pipelines({ review }: { review: string }) {
  const [pipelines, setPipelines] = useState<Pipeline[]>([])
  const [selected, setSelected] = useState<Pipeline | null>(null)
  const [jobs, setJobs] = useState<Job[]>([])
  const [job, setJob] = useState<Job | null>(null)
  const [trace, setTrace] = useState<Trace | null>(null)
  const log = trace?.text ?? ''
  const [logError, setLogError] = useState('')
  const [logLoading, setLogLoading] = useState(false)
  const [loading, setLoading] = useState(true)
  const [jobsLoading, setJobsLoading] = useState(false)
  const [error, setError] = useState('')
  const [refresh, setRefresh] = useState(0)
  const [search, setSearch] = useState('')
  const [follow, setFollow] = useState(true)
  const logRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    let current = true
    let timer: ReturnType<typeof setTimeout> | undefined
    async function update() {
      try {
        const p = await pages<Pipeline>((page) => ({
          kind: 'pipelines',
          review,
          page,
        }))
        if (!current) return
        setLoading(false)
        setPipelines(p)
        setSelected((old) => p.find((v) => v.id === old?.id) ?? p[0] ?? null)
        setError('')
        if (p.some((p) => active(p.status)))
          timer = setTimeout(() => void update(), 10000)
      } catch (e) {
        if (current) {
          setLoading(false)
          setError(message(e))
        }
      }
    }
    void update()
    return () => {
      current = false
      clearTimeout(timer)
    }
  }, [review, refresh])
  useEffect(() => {
    setJob(null)
    setJobs([])
    setTrace(null)
  }, [selected?.id])
  useEffect(() => {
    let current = true
    if (!selected) return
    let timer: ReturnType<typeof setTimeout> | undefined
    const p = selected
    setJobsLoading(true)
    async function update() {
      try {
        const [normal, bridges] = await Promise.all([
          pages<Job>((page) => ({
            kind: 'jobs',
            review,
            project: p.project_id,
            pipeline: p.id,
            page,
            bridges: false,
          })),
          pages<Job>((page) => ({
            kind: 'jobs',
            review,
            project: p.project_id,
            pipeline: p.id,
            page,
            bridges: true,
          })),
        ])
        if (!current) return
        setJobsLoading(false)
        setJobs([...normal, ...bridges])
        setJob((old) =>
          old
            ? ([...normal, ...bridges].find((j) => j.id === old.id) ?? old)
            : null,
        )
        if (active(p.status)) timer = setTimeout(() => void update(), 5000)
      } catch (e) {
        if (current) {
          setJobsLoading(false)
          setError(message(e))
        }
      }
    }
    void update()
    return () => {
      current = false
      clearTimeout(timer)
    }
  }, [review, selected?.id, selected?.status, refresh])
  useEffect(() => {
    let current = true
    let timer: ReturnType<typeof setTimeout> | undefined
    setTrace(null)
    setLogError('')
    setLogLoading(false)
    if (!job || !selected || 'downstream_pipeline' in job) return
    setLogLoading(true)
    const j = job,
      p = selected
    async function update() {
      try {
        const result = await api<Trace>({
          kind: 'trace',
          review,
          project: p.project_id,
          job: j.id,
        })
        if (!current) return
        setTrace(result)
        setLogLoading(false)
        setLogError('')
        if (active(j.status)) timer = setTimeout(() => void update(), 3000)
      } catch (e) {
        if (current) {
          setLogLoading(false)
          setLogError(message(e))
        }
      }
    }
    void update()
    return () => {
      current = false
      clearTimeout(timer)
    }
  }, [review, job?.id, job?.status, selected?.id, refresh])
  useEffect(() => {
    if (follow && logRef.current)
      logRef.current.scrollTop = logRef.current.scrollHeight
  }, [log, follow])
  const shown = search
    ? log
        .split('\n')
        .filter((line) => line.toLowerCase().includes(search.toLowerCase()))
        .join('\n')
    : log
  return (
    <div className={styles.review}>
      <div className={styles.bar}>
        <select
          aria-label="Pipeline"
          value={selected?.id ?? ''}
          onChange={(e) =>
            setSelected(
              pipelines.find((p) => p.id === Number(e.target.value)) ?? null,
            )
          }
        >
          {pipelines.map((p) => (
            <option key={p.id} value={p.id}>
              #{p.id} · {p.status} · {p.ref}
            </option>
          ))}
        </select>
        {selected && (
          <>
            <Status value={selected.status} />
            <code className={styles.muted}>{selected.sha.slice(0, 8)}</code>
          </>
        )}
        <button
          onClick={() => {
            setError('')
            setRefresh((n) => n + 1)
          }}
        >
          Refresh
        </button>
        {selected && (
          <button onClick={() => void gitlab.openUrl(selected.web_url)}>
            Open pipeline in GitLab
          </button>
        )}
      </div>
      {error && (
        <div role="alert" className={styles.error}>
          {error}
        </div>
      )}
      <div className={styles.body}>
        <div className={styles.tree}>
          {loading ? (
            <p role="status">Loading pipelines…</p>
          ) : (
            !pipelines.length && <p>No pipelines.</p>
          )}
          {jobsLoading && <p role="status">Loading jobs…</p>}
          {Array.from(new Set(jobs.map((j) => j.stage))).map((stage) => (
            <section key={stage} className={styles.stage}>
              <h3>{stage}</h3>
              {jobs
                .filter((j) => j.stage === stage)
                .map((j) => (
                  <JobRow
                    key={j.id}
                    job={j}
                    selected={job?.id === j.id}
                    onSelect={() => setJob(j)}
                  />
                ))}
            </section>
          ))}
          {selected && !jobsLoading && !jobs.length && (
            <p className={styles.muted}>No jobs in this pipeline.</p>
          )}
        </div>
        <div className={styles.surface}>
          {job && (
            <div className={styles.bar}>
              <strong>{job.name}</strong>
              <Status value={job.status} />
              <span className={styles.muted}>
                {job.stage} · #{job.id}
              </span>
            </div>
          )}
          <div className={styles.bar}>
            <input
              aria-label="Filter log lines"
              placeholder="Filter log lines"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
            <label>
              <input
                type="checkbox"
                checked={follow}
                onChange={(e) => setFollow(e.target.checked)}
              />
              Follow output
            </label>
            {job && (
              <button onClick={() => void gitlab.openUrl(job.web_url)}>
                Full log in GitLab
              </button>
            )}
          </div>
          {logError && (
            <div role="alert" className={styles.error}>
              {logError}
            </div>
          )}
          {job?.downstream_pipeline && (
            <div className={styles.bar}>
              <button
                onClick={() =>
                  void gitlab.openUrl(job.downstream_pipeline!.web_url)
                }
              >
                Open downstream pipeline #{job.downstream_pipeline.id}
              </button>
              <Status value={job.downstream_pipeline.status} />
            </div>
          )}
          <div ref={logRef} className={styles.log} aria-label="Job output">
            {!job
              ? 'Select a job to view its log.'
              : 'downstream_pipeline' in job
                ? 'This trigger job has no trace. Open its downstream pipeline to view jobs.'
                : logLoading
                  ? 'Loading job log…'
                  : logError
                    ? 'The log could not be loaded. Retry with Refresh or open it in GitLab.'
                    : trace?.state !== 'available'
                      ? trace?.message
                      : (shown ? (
                          <JobLog key={job.id} text={log} search={search} />
                        ) : null) ||
                        (search && log
                          ? 'No log lines match the filter.'
                          : active(job.status)
                            ? 'Waiting for job output…'
                            : 'This job has no log output.')}
          </div>
        </div>
      </div>
    </div>
  )
}
