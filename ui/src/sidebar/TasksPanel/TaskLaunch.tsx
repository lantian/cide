import { useState } from 'react'
import type { PlanningPolicy } from '@/ipc/generated'
import { errorText } from '@/ipc/errorText'
import { Button } from '@/kit/components/Button'
import { Checkbox } from '@/kit/components/Choice'
import { Select } from '@/kit/components/Select'
import styles from './TasksPanel.module.css'

/** One launch's transient options; successful and failed requests both reset the choice. */
export function TaskLaunch({ roles, assigned, onLaunch }: {
  roles: Readonly<Record<string, string>>
  assigned: string | null
  onLaunch: (agent: string, planning?: PlanningPolicy) => Promise<void>
}) {
  const [options, setOptions] = useState(false)
  const [agent, setAgent] = useState(assigned ?? Object.keys(roles)[0] ?? '')
  const [plan, setPlan] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  if (Object.keys(roles).length === 0) return null
  const launch = async () => {
    setBusy(true)
    setError(null)
    const planning = options && plan ? 'required' : undefined
    setPlan(false)
    setOptions(false)
    try { await onLaunch(agent, planning) }
    catch (cause) { setError(errorText(cause)) }
    finally { setBusy(false) }
  }
  return <section className={styles.dispatchInfo} aria-label="Launch agent" data-audit="taskLaunch">
    {options && <>
      <Select aria-label="Agent" value={agent} disabled={busy} options={Object.entries(roles).map(([value, label]) => ({ value, label }))} onChange={setAgent} />
      <Checkbox label="Plan before execution" hint="Cide accepts the native plan automatically, then starts execution." checked={plan} disabled={busy} onChange={setPlan} />
    </>}
    <Button size="sm" disabled={busy || agent === ''} onClick={() => { void launch() }}>Start agent</Button>
    <Button size="sm" variant="link" disabled={busy} aria-expanded={options} onClick={() => { setOptions(!options); setPlan(false) }}>Launch options</Button>
    {error !== null && <p className={styles.assigneeHint} role="alert">{error}</p>}
  </section>
}
