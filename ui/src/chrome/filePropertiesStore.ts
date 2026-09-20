/**
 * The path whose properties card is up, in whichever window asked. (M70)
 *
 * A store rather than local state in `App.tsx` for `pushStore.ts`'s two reasons, both of which
 * apply unchanged: the gesture is made from `keys/dispatch.ts`, which is outside React's tree
 * entirely, and **both window kinds have to draw it** — `file.properties` carries no
 * `shellWindow` clause (an overlay is raisable from any window, which is `git.blame`'s stated
 * precedent), so the command is live in a detached pane window. A card wired only into the shell
 * tree would leave that keystroke asking nobody, silently.
 *
 * Deliberately **not** part of `store/workspace.ts`: that store mirrors Rust-owned durable state
 * and holds nothing local. Which card is open is transient by definition, and a properties card
 * restored across a restart would be a window reopening onto a file the user has forgotten
 * asking about.
 *
 * # Why this one *does* replace rather than drop
 *
 * `pushStore` drops a second request, because replacing would swap a commit list under somebody
 * who is reading it before pressing a button. Nothing here is a question and nothing is pending
 * a decision: the card is a **reading**, every button on it is a separate deliberate gesture,
 * and the natural way to reach a second one is Escape-then-right-click. Replacing is what a
 * reader who somehow got there would mean, so this takes the newer path.
 */
import { create } from 'zustand'

export interface PendingProperties {
  /** The project the path belongs to — the git half is asked per project. */
  project: string
  /** Absolute. The card resolves its own display form; see `displayPath`. */
  path: string
}

interface PropertiesStore {
  /** Null whenever no card is up, which is almost always. */
  pending: PendingProperties | null
  open: (pending: PendingProperties) => void
  close: () => void
}

export const useFileProperties = create<PropertiesStore>((set) => ({
  pending: null,
  open: (pending) => set({ pending }),
  close: () => set({ pending: null }),
}))

/**
 * Raise the card, from outside React.
 *
 * Callers go through `filePropertiesOpen.ts` rather than calling this directly — that module
 * exists for `pushRun.ts`'s reason, and its header says which cycle it breaks.
 */
export function requestProperties(pending: PendingProperties): void {
  useFileProperties.getState().open(pending)
}
