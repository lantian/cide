/**
 * Draws *merge or rebase?*, wherever Ctrl+T was pressed.
 *
 * `OutsideOpenGate.tsx`'s shape, and rendered in **both** branches of `App.tsx` for the same
 * reason: the key gate is installed before the `pane:<uuid>` early return, so a detached pane
 * window can raise this and a gate wired only into the shell would leave it asking nobody.
 *
 * Unlike that one, this dialog is *controlled* — `ConfirmDestructive` owns neither the selected
 * radio nor the checkbox — so the two `useState`s live here. They are reset on every new
 * question: carrying `remember` across a dismissal would mean a user who ticked it, cancelled,
 * and pulled again silently writing a config for repositories they never saw agree to it.
 * `PasteConfirm` does the same, keyed on its question.
 *
 * It owns no rules. `chrome/pullStrategyModel.ts` decides what the dialog says,
 * `chrome/pullStrategyStore.ts` holds it, and `ConfirmDestructive` draws it — Cancel focused
 * and accented, Escape cancelling.
 */
import { useEffect, useState } from 'react'
import { ConfirmDestructive } from './ConfirmDestructive'
import { rememberLabel, strategyOf } from './pullStrategyModel'
import { usePullStrategy } from './pullStrategyStore'

export function PullStrategyGate() {
  const pending = usePullStrategy((s) => s.pending)
  const [chosen, setChosen] = useState<string | null>(null)
  const [remember, setRemember] = useState(false)

  useEffect(() => {
    setChosen(null)
    setRemember(false)
  }, [pending])

  if (pending === null) return null
  const { ask, repos } = pending
  return (
    <ConfirmDestructive
      state={{
        title: ask.title,
        body: ask.body,
        // Ignored while `choices` is set — the selected choice carries both — but `ConfirmState`
        // requires them, and inventing values a reader might believe would be worse than these.
        files: [],
        confirmLabel: '',
        mark: ask.mark,
        split: ask.split,
        choices: ask.choices,
        chosen,
        onChoose: setChosen,
        option: {
          label: rememberLabel(repos, chosen),
          checked: remember,
          onToggle: setRemember,
        },
        // `ConfirmDestructive` calls neither `run` nor `onCancel` itself — the caller runs and
        // closes — so the store's `answer` is the whole of it and this is never invoked.
        run: () => {},
      }}
      onCancel={() => usePullStrategy.getState().dismiss()}
      onConfirm={() => usePullStrategy.getState().answer(strategyOf(chosen), remember)}
    />
  )
}
