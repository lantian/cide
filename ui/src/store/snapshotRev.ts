/**
 * "A workspace snapshot landed", as a number any component can subscribe to without importing
 * the store.
 *
 * # Why it is its own module
 *
 * `layout/SplitTree.tsx` needs to re-render on every snapshot (see its `ChainNode`) and is
 * rendered under node by `check:rows` from a fixture. `store/workspace` imports the pane hosts,
 * which import xterm, which touches `self` at module scope — so importing the store there made the
 * check unrunnable. This module imports nothing; the store writes it (`noteSnapshot`, from
 * `applySnapshot` and `hydrate`) and a component reads it through `useSnapshotRev`.
 *
 * # Why anything needs it
 *
 * Before the mirror was structurally shared (`store/shareEqual`), every snapshot handed every
 * project, tab and pane a new identity, so every `SplitTree` re-rendered on every revision
 * whether it wanted to or not. `ChainNode` leaned on that to re-assert its grid tracks after a
 * clamped commit or a cancelled drag. With sharing, a snapshot that did not touch a tab renders
 * nothing in it — correct everywhere except where "a snapshot landed" was itself the signal.
 */
import { useSyncExternalStore } from 'react'

let rev = 0
const listeners = new Set<() => void>()

/** Called by the store each time it adopts a workspace. */
export function noteSnapshot(): void {
  rev += 1
  for (const listener of listeners) listener()
}

/** A number that changes on every adopted snapshot, and only then. `0` under SSR. */
export function useSnapshotRev(): number {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    () => rev,
    () => 0,
  )
}
