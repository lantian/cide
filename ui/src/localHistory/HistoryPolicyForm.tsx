import { useEffect, useState } from 'react'
import type { HistoryPolicy, HistoryStatus } from '@/ipc/generated'
import { Button } from '@/kit/components/Button'
import { Checkbox } from '@/kit/components/Choice'
import { Field, TextInput, Textarea } from '@/kit/components/Field'
import { Banner, Note } from '@/kit/components/Feedback'
import { Dialog } from '@/kit/components/Overlay'
import { Disclosure, Summary } from '@/kit/components/Surface'
import { policyFromForm } from './model'

function draft(status: HistoryStatus) { return { generation: status.generation, enabled: status.policy.enabled, days: String(status.policy.retentionDays), budget: String(status.policy.budgetBytes / (1024 * 1024)), exclusions: status.policy.exclusions.join('\n') } }
export function HistoryPolicyForm({ status, onSave, onClear, onCancel }: { status: HistoryStatus; onSave: (policy: HistoryPolicy, generation: number) => Promise<void>; onClear: () => void; onCancel: () => void }) {
  const [value, setValue] = useState(() => draft(status))
  const [error, setError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  useEffect(() => { setValue(draft(status)); setError(null) }, [status.root, status.generation])
  const save = async (): Promise<void> => {
    setError(null)
    try {
      const policy = policyFromForm(value.enabled, value.days, value.budget, value.exclusions)
      setSaving(true); await onSave(policy, value.generation)
    } catch (why) { setError(String(why)) }
    finally { setSaving(false) }
  }
  return <Dialog title="Local History settings" lead={status.canonicalRoot} onClose={saving ? undefined : onCancel} onKeyDown={(e) => { if (e.key === 'Escape' && !saving) { e.stopPropagation(); onCancel() } }} actions={<><Button variant="danger" disabled={saving} onClick={onClear}>Clear history…</Button><Button disabled={saving} onClick={onCancel}>Cancel</Button><Button variant="primary" busy={saving} onClick={() => { void save() }}>Save settings</Button></>}>
    {error && <Banner tone="bad">{error}</Banner>}
    <Checkbox label="Capture future changes in this root" checked={value.enabled} disabled={saving} onChange={(enabled) => setValue((v) => ({ ...v, enabled }))} hint="Disabling retains existing revisions. Enabling starts a new baseline." />
    <Summary rows={[{ label: 'Used space', value: `${(status.usedBytes / (1024 * 1024)).toFixed(2)} MiB` }, { label: 'Defaults', value: '7 days · 256 MiB per root · UTF-8 text up to 2 MiB' }]} />
    <Field label="Retention (days)" hint="Oldest unpinned revisions expire by elapsed age and budget, including unchanged baselines.">{({ id, describedBy }) => <TextInput id={id} aria-describedby={describedBy} type="number" min="1" step="1" value={value.days} disabled={saving} onChange={(e) => { const next = e.currentTarget.value; setValue((v) => ({ ...v, days: next })) }} />}</Field>
    <Field label="Root budget (MiB)" hint="The budget includes committed contents and metadata.">{({ id, describedBy }) => <TextInput id={id} aria-describedby={describedBy} type="number" min="0.000001" step="any" value={value.budget} disabled={saving} onChange={(e) => { const next = e.currentTarget.value; setValue((v) => ({ ...v, budget: next })) }} />}</Field>
    <Field label="Exclusions" hint="One root-relative glob per line, such as private/** or **/*.secret.">{({ id, describedBy }) => <Textarea id={id} aria-describedby={describedBy} value={value.exclusions} rows={4} disabled={saving} onChange={(e) => { const next = e.currentTarget.value; setValue((v) => ({ ...v, exclusions: next })) }} />}</Field>
    <Note>Git-ignored paths, dependency folders, Git metadata, .env and .env.* are excluded by default. New exclusions stop capture; already retained content remains until expiry or explicit clear.</Note>
    <Disclosure title="Coverage and recovery limits">
      <p>History is stored in this computer’s application profile, outside project files. It is local recovery storage, not a backup.</p>
      <p>The baseline starts with each file’s earliest read. External writes may coalesce. Unsaved keystrokes and intermediate edits while a root is closed are unavailable. Restores require clean, acknowledging editors, unchanged disk contents, an existing parent, and a durable safety revision. Enable capture before restoring.</p>
      {status.coverage.baselineRunning && <p>Baseline / reconciliation is running: {status.coverage.scannedFiles} files observed.</p>}
      {status.coverage.notices.map((notice) => <p key={notice}>{notice}</p>)}
    </Disclosure>
  </Dialog>
}
