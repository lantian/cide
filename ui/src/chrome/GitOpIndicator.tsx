/**
 * `◜ Pushing…` on the status bar, beside the branch, while a git operation is in flight. (M65)
 *
 * > *"Need a push animation progress in bottom bar near branch to be able to see that push
 * > currently in progress."*
 *
 * # Why this is its own component rather than a prop on the bar
 *
 * `chrome/StatusBar.tsx` says it "reads nothing from the store, so it stays a pure render target
 * that a screenshot test can drive directly", and `App.tsx` subscribes `revealRoots` on its
 * behalf for exactly that reason. This is the shape the bar already has an answer for:
 * `<BranchSelector />` is one line in the bar and owns its own store, because the state it draws
 * is ambient rather than something `App.tsx` holds. So is this — a git operation is started from
 * `keys/dispatch.ts`, from the branch popup and from the Git panel, none of which `App.tsx` is
 * on the path of — and threading it through as a prop would put a fourth producer's plumbing in
 * a component that has no other use for it.
 *
 * The rules are still in a module a check script can compile and drive (`chrome/gitOpModel.ts`),
 * which is the half of "pure render target" that actually catches bugs.
 *
 * # Nothing in `chrome/` is under a `PanelBoundary`
 *
 * `check:boundary` wraps the sidebar's panels and the tool window; the bar is outside it, so a
 * throw here unmounts the whole window with nothing on screen to say why. That is why the label
 * is read through `gitOpLabel` — which is total for every input, including an empty list — and
 * why nothing below indexes a list or asserts non-null.
 */
import { useWorkspace } from '@/store/workspace'
import { activeProjectIdOf } from '@/keys/target'
import { Icon } from '@/icons/Icon'
import { gitOpLabel, gitOpTitle } from './gitOpModel'
import { useGitOps } from './gitOpStore'

import styles from './GitOpIndicator.module.css'

export function GitOpIndicator() {
  /*
   * The window's project, the way `BranchSelector`'s `useBranchData` reads it — one subscription
   * to `boot` and a pure derivation. `chrome/Failures.tsx` selects it with the identical
   * expression, and that is not a coincidence worth removing: both surfaces are answering "does
   * this belong to what the user is looking at", and two derivations of that are two chances to
   * disagree about it.
   */
  const boot = useWorkspace((s) => s.boot)
  const project = activeProjectIdOf(boot)

  /*
   * A **string** out of the store, never the array.
   *
   * `check:selectors` refuses a selector that builds a fresh value, because `useSyncExternalStore`
   * compares snapshots with `Object.is`: returning `s.running` filtered here would be a new array
   * every read, which re-renders for ever and ends at *Maximum update depth exceeded* — and that
   * unmounts the entire root, not this component. A primitive is `Object.is`-equal to itself, so
   * the filtering lives behind `gitOpLabel`'s door and only its result crosses.
   *
   * That also means the array work must *stay* behind that door: the check tests the return
   * expression as text, so inlining a `.filter(` into this arrow would fail it even though the
   * result is still a string.
   */
  const label = useGitOps((s) => (project === null ? '' : gitOpLabel(s.running, project)))
  const title = useGitOps((s) => (project === null ? '' : gitOpTitle(s.running, project)))

  /*
   * Nothing running, nothing rendered — not an empty box.
   *
   * `.left` is a flex row with a 12px gap, so an element that merely had no text would still
   * take a gap's worth of the row and shift the file trail beside it. Returning `null` removes
   * the flex item and the gap together, which is the same outcome `.path:empty { display: none }`
   * buys for the trail by a different route.
   */
  if (label === '') return null

  return (
    <span className={styles.indicator} data-audit="gitOpIndicator" title={title}>
      {/*
       * No `aria-live`. The *outcome* is already announced — `chrome/Failures.tsx` gives a notice
       * `role="status"` when it succeeded and `role="alert"` when it did not — and a live region
       * here would additionally read `0/3`, `1/3`, `2/3` aloud on the way. The fact is in the
       * visible text and in the tooltip, which is what a sighted user gets too.
       */}
      {/*
       * The class goes on a wrapping span and not on the `<svg>`, which is
       * `sidebar/AgentsPanel/RunRow.tsx`'s shape for the same mark. `Icon`'s `className` is a
       * required `string` under `exactOptionalPropertyTypes`, and a CSS module's key is
       * `string | undefined`, so passing it straight through does not type — and a wrapper is
       * what the other three spinners in this app use anyway.
       *
       * `aria-hidden`, because the word beside it says the same thing in text.
       */}
      <span className={styles.spin} aria-hidden="true">
        <Icon name="loader-circle" size={1} />
      </span>
      {label}
    </span>
  )
}
