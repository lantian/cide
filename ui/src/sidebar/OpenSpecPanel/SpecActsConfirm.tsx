/**
 * Archive's confirm, for whichever surface asked — the panel row, its menu, or the change page.
 * Rendered once by `App.tsx`, like `ApplyDialog`. See `specActs.ts`.
 */
import { ConfirmDestructive } from '@/chrome/ConfirmDestructive'
import { useSpecConfirm } from './specActs'

export function SpecActsConfirm() {
  const confirming = useSpecConfirm((state) => state.confirming)
  const set = useSpecConfirm((state) => state.set)
  if (confirming === null) return null
  return (
    <ConfirmDestructive
      state={confirming}
      onCancel={() => set(null)}
      onConfirm={() => {
        const run = confirming.run
        set(null)
        run?.()
      }}
    />
  )
}
