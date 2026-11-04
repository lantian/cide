import { useEffect, useRef, useState } from 'react'
import { describe } from '@/chrome/notices'
import { FormRow } from '@/kit/components/Field'
import { Switch } from '@/kit/components/Choice'
import { specSessions, type ProjectId } from '@/ipc/client'

const fields = {
  propose: 'proposePlanMode',
  explore: 'explorePlanMode',
  apply: 'applyPlanMode',
} as const

/** A preference belongs to a project and workflow, never to the mounted dialog. */
export function usePlanMode(project: ProjectId | null, op: keyof typeof fields, open: boolean) {
  const [value, setValue] = useState(false)
  const [ready, setReady] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const generation = useRef(0)
  const [loaded, setLoaded] = useState('')
  const key = project === null || !open ? '' : `${project}:${op}`
  useEffect(() => {
    const mine = ++generation.current
    setReady(false)
    setError(null)
    setLoaded('')
    if (project === null || !open) return
    void specSessions.settings(project).then((settings) => {
      if (mine !== generation.current) return
      setValue(settings[fields[op]])
      setLoaded(key)
      setReady(true)
    }, (e: unknown) => {
      if (mine === generation.current) setError(describe(e))
    })
    return () => { generation.current++ }
  }, [project, op, open, key])
  return {
    value, ready: ready && loaded === key && key !== '', error,
    change(next: boolean) {
      if (project === null || !ready || loaded !== key) return
      const mine = generation.current
      setReady(false)
      setError(null)
      void specSessions.setSettings(project, { [fields[op]]: next }).then((settings) => {
        if (mine !== generation.current) return
        setValue(settings[fields[op]])
        setReady(true)
      }, (e: unknown) => {
        if (mine !== generation.current) return
        setError(describe(e))
        setReady(true)
      })
    },
  }
}

export function PlanMode({ preference, busy }: {
  preference: ReturnType<typeof usePlanMode>
  busy: boolean
}) {
  return (
    <FormRow label="Start in plan mode"
      hint={preference.error ?? 'Review the plan before execution. Remembered for this workflow in this project.'}>
      <Switch label="Start in plan mode" checked={preference.value}
        disabled={busy || !preference.ready} onChange={preference.change} />
    </FormRow>
  )
}
