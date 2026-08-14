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
import { installKeyGate, mouseNavGate } from './gate'
import { mergeContext } from './context'
import { switcherCapture } from './switcherStore'
import { recorderCapture } from './recorderStore'
import { events } from '@/ipc/client'
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
         * The two stateful claims on a stroke, in the one hook the gate offers.
         *
         * Wired here rather than through `KeyGateWiring` on purpose: both are global to the
         * window and own their own state (`keys/switcherStore.ts`, `keys/recorderStore.ts`),
         * so a host that passes nothing gets working gestures. Making it a prop would mean
         * every window that installs a gate has to remember to pass it, and the one that
         * forgot would leave a popup swallowing nothing while Tab reached the shell
         * underneath it.
         *
         * **The recorder first, and the order is not arbitrary.** It is modal: while Settings
         * → Keymap is waiting for a chord it consumes every stroke, because a chord being
         * recorded must not also run. A switcher walk cannot legitimately be open behind it —
         * `startRecording` cancels one — so the second claim is only ever consulted when the
         * first is closed, and `||` short-circuits to exactly that.
         *
         * `check-key-gate.mjs` sweeps the whole modifier × key space three times, once with
         * nothing armed and once with each of these, and holds both of the gate's keyboard
         * entry points to the same verdict every time.
         */
        capture: (stroke) => recorderCapture(stroke) || switcherCapture(stroke),
      }),
    [],
  )

  /*
   * Entry point 3: the mouse's thumb buttons, which arrive as an event from Rust because the
   * DOM cannot tell them apart. See `gate.ts`'s module note.
   *
   * A separate effect from the install above rather than a line inside it, because the two have
   * different shapes: `installKeyGate` is synchronous and returns its own teardown, while
   * `listen` is a promise that resolves a tick or more later. `dropped` and not merely a null
   * check on the teardown — a window closed inside that tick would otherwise leave a
   * subscription nobody can cancel, holding this closure for the life of the window. The same
   * guard, for the same reason, as `panes/EditorPane.tsx`'s `onSessionTool` wiring.
   *
   * The decision is thrown away deliberately: there is nothing to `preventDefault`. The GTK
   * handler already returned `Propagation::Stop`, so the web process never saw the press.
   */
  useEffect(() => {
    let unlisten: (() => void) | null = null
    let dropped = false
    void events
      .onMouseNav((button, modifiers) => {
        // Guarded rather than cast: the payload crosses a process boundary, and a Rust change
        // that started sending a third button must not resolve as a stroke nobody bound —
        // which would swallow it and leave no trace of why.
        if (button !== 'mouseback' && button !== 'mouseforward') return
        mouseNavGate(button, modifiers)
      })
      .then((fn) => {
        if (dropped) fn()
        else unlisten = fn
      })
      .catch(() => {})
    return () => {
      dropped = true
      unlisten?.()
    }
  }, [])
}
