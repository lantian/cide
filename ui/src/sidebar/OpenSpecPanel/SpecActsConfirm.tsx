/** Application-owned archive dialog; operation state survives dismissal. */
import { ConfirmDestructive } from '@/chrome/ConfirmDestructive'
import { confirmArchive, needsRetireConsent, useSpecConfirm } from './specActs'

export function SpecActsConfirm() {
  const confirming = useSpecConfirm((state) => state.confirming)
  const set = useSpecConfirm((state) => state.set)
  if (confirming === null) return null
  const { change, phase, plan, error } = confirming
  const retiring = plan?.specsTouched.filter((touch) => touch.retires).length ?? 0
  const consent = needsRetireConsent(confirming)
  const busy = phase === 'preparing' || phase === 'archiving'
  return (
    <ConfirmDestructive
      state={{
        title: `Archive ${change}?`,
        body: phase === 'preparing' ? 'Preparing the archive preview…' :
          phase === 'archiving' ? 'Archiving… You can close this dialog; the result will appear in a notification.' :
          `Merges requirement edits into openspec/specs/ and moves the change to openspec/changes/archive/. Checkout: ${plan?.archiveRoot ?? 'unavailable'}.`,
        files: plan?.specsTouched.map((touch) => `openspec/specs/${touch.spec}/spec.md — ${touch.requirements} ${touch.requirements === 1 ? 'requirement' : 'requirements'} ${touch.operation}${touch.retires ? ' — retires the capability (spec.md is deleted)' : ''}`) ?? [],
        confirmLabel: phase === 'preparing' ? 'Preparing…' : phase === 'archiving' ? 'Archiving…' : phase === 'failed' ? 'Review again' : 'Archive',
        defaultButton: 'confirm', danger: retiring > 0, mark: 'file-diff',
        run: () => { void confirmArchive() },
        ...(consent && phase !== 'failed' ? { option: {
          label: `Retire ${retiring} ${retiring === 1 ? 'capability' : 'capabilities'}: every requirement is removed, so its spec.md is deleted (restore from git)`,
          checked: confirming.retire,
          onToggle: (next: boolean) => set({ ...confirming, retire: next }),
        } } : {}),
      }}
      busy={busy}
      confirmDisabled={phase === 'ready' && consent && !confirming.retire}
      error={error}
      virtualizeFiles
      onCancel={() => set(null)}
      onConfirm={() => { void confirmArchive() }}
    />
  )
}
