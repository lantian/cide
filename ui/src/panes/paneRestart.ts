/**
 * How a command reaches a pane's restart.
 *
 * `keys/dispatch.ts` runs outside React — the key gate is a window listener and the palette is
 * a list of strings — so it cannot call into a component. Restarting, on the other hand, is
 * something only `TerminalPane` can do: it is the one place that knows the pane's geometry, its
 * spec, and the terminal the new child has to attach to. Doing it anywhere else would mean
 * unmounting the pane to respawn it, which destroys the terminal DOM and is the one thing
 * `layout/paneHosts.ts` exists to prevent.
 *
 * So the pane publishes a handle here for as long as it is mounted, exactly the way
 * `terminal/pathLinks.ts` publishes its environment and `editor/openBuffers.ts` publishes its
 * savers. A pane with no entry is a pane nothing can restart, and the dispatcher says so rather
 * than pretending.
 */
import type { RestartMode } from './restartRule'

export interface PaneRestarter {
  /**
   * Kill whatever this pane is running and start again — a new conversation, or the same one.
   *
   * Rejects for the same reasons a spawn does; the caller reports. It never unmounts the pane.
   */
  restart(mode: RestartMode): Promise<void>
  /**
   * Whether Claude Code still holds a transcript for the session this pane holds *now*.
   *
   * Asked over IPC at the moment the question matters rather than cached, because a command can
   * be run against a pane whose child is still alive and there is no event that would refresh a
   * cached answer. A `false` here is what stops `claude.resume` spawning a `--resume` for a
   * transcript that is gone.
   */
  canResume(): Promise<boolean>
}

const restarters = new Map<string, PaneRestarter>()

/**
 * Publish a pane's restart handle. Returns the teardown.
 *
 * The teardown checks ownership before it deletes, for the reason `installKeyGate` documents
 * about the same shape: React can mount the replacement before it runs the previous mount's
 * cleanup, and a stale teardown that deleted unconditionally would leave the live pane with no
 * entry and every restart command reporting that the pane cannot be restarted.
 */
export function registerRestarter(paneId: string, restarter: PaneRestarter): () => void {
  restarters.set(paneId, restarter)
  return () => {
    if (restarters.get(paneId) === restarter) restarters.delete(paneId)
  }
}

/** The pane's restart handle, or `null` when nothing in this window is showing that pane. */
export function paneRestarter(paneId: string): PaneRestarter | null {
  return restarters.get(paneId) ?? null
}
