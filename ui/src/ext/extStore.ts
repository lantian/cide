/**
 * The window's mirror of the extension registry. (M22)
 *
 * `sidebar/agentsStore.ts`'s shape, and every one of its rules applies here for the same reasons:
 *
 * * **`adopt` takes the event payload whole.** Answering an event that just told you the answer
 *   with a round trip asking for it is a question with a known answer.
 * * **`refresh` is generation-guarded, and the generation is claimed *before* the call.** Without
 *   that, a slow answer lands after a newer broadcast and the panel shows the older of the two.
 * * **Nothing optimistic touches `snapshot`.** A store that guessed would draw an extension as
 *   installed for a frame, which is not a cosmetic flicker but a claim that code was copied onto
 *   the user's disk when it was not.
 *
 * One thing is different and it is the reason this file is not just a copy: adopting a snapshot
 * also **reconciles the workers**. That is a side effect in a store, which is normally the wrong
 * shape — and it is right here because the alternative is worse. The workers are the registry's
 * only visible consequence, and a component that reconciled them in an effect would start and stop
 * an extension depending on whether a panel happened to be mounted.
 */
import { create } from 'zustand'

import { ext as extApi, extEvents } from '@/ipc/client'
import type { ExtensionSnapshot } from '@/ipc/client'
import { reconcile, stopAll } from './host'

/** The empty snapshot, shared. */
const NONE: ExtensionSnapshot = {
  // `bigint`, because `rev` is a `u64` on the wire and ts-rs maps it that way. Comparing two of
  // them is the whole of what the field is for, and `bigint` compares with `<` exactly as a
  // number does.
  rev: 0n,
  marketplaces: [],
  extensions: [],
  resolved: { languages: [], servers: [], panels: [], commands: [], conflicts: [] },
  problems: [],
}

interface ExtStoreState {
  readonly snapshot: ExtensionSnapshot
  /** Whether a call is in flight, for the panel's own spinner. */
  readonly busy: boolean
  adopt: (snapshot: ExtensionSnapshot) => void
  refresh: () => Promise<void>
  /** Run a mutation, adopt its answer, and clear `busy` whatever happens. */
  run: (work: () => Promise<ExtensionSnapshot>) => Promise<void>
}

/**
 * The high-water mark.
 *
 * `ExtensionSnapshot.rev` exists because this state has several writers — two windows, a
 * background refresh, and the user running `git pull` in a clone by hand — so snapshots can arrive
 * out of order. `emit.rs` argues the choice to carry one at length. Dropping a stale snapshot is
 * the whole of what the number is for.
 */
let seen = 0n
let generation = 0

export const useExtStore = create<ExtStoreState>((set, get) => ({
  snapshot: NONE,
  busy: false,

  adopt: (snapshot) => {
    if (snapshot.rev < seen) return
    seen = snapshot.rev
    set({ snapshot })
    reconcile(snapshot.extensions)
  },

  refresh: async () => {
    const mine = ++generation
    try {
      const snapshot = await extApi.snapshot()
      // Claimed before the call and checked after: a snapshot that arrived while a newer request
      // was already in flight is one the store must not apply, however recent its `rev` looks.
      if (mine !== generation) return
      get().adopt(snapshot)
    } catch (error) {
      console.error('[cide] could not read the extension registry', error)
    }
  },

  run: async (work) => {
    set({ busy: true })
    try {
      get().adopt(await work())
    } finally {
      set({ busy: false })
    }
  },
}))

/**
 * Start listening, and take the first snapshot.
 *
 * Called from `App.tsx` and not from the panel, on the rule the agents and tasks stores follow: a
 * rail badge — and, more importantly here, the *workers* — must stay live while the sidebar is
 * shut. An extension that only ran while somebody was looking at its panel would be an extension
 * that could never contribute a diagnostic.
 */
export function attachExtensions(): () => void {
  void useExtStore.getState().refresh()
  const pending = extEvents.onChanged((snapshot) => {
    useExtStore.getState().adopt(snapshot)
  })
  return () => {
    void pending.then((unlisten) => {
      unlisten()
    })
    // Every worker in this window, stopped. A detached pane is a separate JavaScript realm with
    // its own copy of this module, so this is per window and not per process — which is correct:
    // the workers are per window too.
    stopAll()
  }
}

/** Every panel a contributed extension is showing, in registry order. */
export function useExtPanels(): ExtensionSnapshot['resolved']['panels'] {
  // The mirror's own array, never a fresh one — `check:selectors`' rule, and the failure it
  // guards is a render loop that unmounts the root rather than anything cosmetic.
  return useExtStore((state) => state.snapshot.resolved.panels)
}

/** The installed rows, for the Extensions panel. */
export function useInstalledExtensions(): ExtensionSnapshot['extensions'] {
  return useExtStore((state) => state.snapshot.extensions)
}

/** The connected marketplaces. */
export function useMarketplaces(): ExtensionSnapshot['marketplaces'] {
  return useExtStore((state) => state.snapshot.marketplaces)
}
