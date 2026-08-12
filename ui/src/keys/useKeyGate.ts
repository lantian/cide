/**
 * React wiring for entry point 2.
 *
 * The gate is installed once per window and never reinstalled, because reinstalling swaps
 * the singleton and a terminal created before the swap would be resolving against the old
 * one's prefix machine. Everything that changes between renders — the keymap, the context
 * flags, the dispatcher — is read through a ref at keystroke time instead.
 *
 * Entry point 1 is not installed here. It belongs at terminal construction, one line in
 * `terminal/xterm.ts`:
 *
 * ```ts
 * import { attachKeyGate } from '@/keys/gate'
 * attachKeyGate(term)   // right after `new Terminal({...})`
 * ```
 *
 * `terminal/xterm.ts` is another milestone's file, so that line is not applied here; see the
 * M8 frontend report.
 */
import { useEffect, useRef } from 'react'
import { installKeyGate } from './gate'
import { mergeContext } from './context'
import { switcherCapture } from './switcherStore'
import type { KeyBinding, KeyContext } from './keymap'

export interface KeyGateWiring {
  /** `Bootstrap.keymap`. `ResolvedBinding` satisfies `KeyBinding`. */
  bindings: readonly KeyBinding[]
  /**
   * The flags only this component knows — an overlay being up, a menu being open.
   *
   * Everything derivable from the workspace mirror is merged over it by [`mergeContext`], so
   * the host does not have to remember the whole vocabulary. It could not, in fact: the
   * registry gated every git command on `repoOpen` and no host ever set it, so all four were
   * unreachable by key *and* filtered out of the palette, with nothing reporting either. See
   * `keys/context.ts` for which side owns which flag and why the mirror wins the overlap.
   */
  context: KeyContext
  run: (command: string, args: unknown) => void
  /** For a status-bar readout while a chord prefix is armed. */
  onPending?: ((sequence: string | null) => void) | undefined
}

export function useKeyGate(wiring: KeyGateWiring): void {
  const live = useRef(wiring)
  live.current = wiring

  useEffect(
    () =>
      installKeyGate({
        bindings: () => live.current.bindings,
        // Merged per keystroke, not per render: the gate asks at the moment the chord
        // resolves, which is the only moment the answer has to be true.
        context: () => mergeContext(live.current.context),
        run: (command, args) => live.current.run(command, args),
        onPending: (sequence) => live.current.onPending?.(sequence),
        /*
         * The project switcher's claim on Tab and Escape while its popup is up.
         *
         * Wired here rather than through `KeyGateWiring` on purpose: the switcher is global
         * to the window and owns its own listeners (`keys/switcherStore.ts`), so a host that
         * passes nothing gets a working gesture. Making it a prop would mean every window
         * that installs a gate has to remember to pass it, and the one that forgot would
         * leave a popup swallowing nothing while Tab reached the shell underneath it.
         */
        capture: switcherCapture,
      }),
    [],
  )
}
