/**
 * What a freshly split pane should spawn, held between the split and the mount.
 *
 * # Why this exists at all
 *
 * `pane_split` creates a pane and deliberately leaves it session-less: only the frontend
 * knows how large the pane is, so spawning is the frontend's job and happens after layout.
 * That gap is fine for an ordinary new session, where the pane's `kind` says everything.
 * It is not fine for the two intents that carry an argument:
 *
 * * `forkPrimary` needs the parent session to branch from;
 * * `mirror` needs the session to attach to, and must not spawn at all.
 *
 * Neither survives in the pane itself, and neither should: `Pane` is persisted, and "this
 * pane was once created by forking" is a fact about one gesture, not durable state. So the
 * intent is parked here for the moment between the split committing and `TerminalPane`
 * mounting, and consumed exactly once.
 *
 * # Consumed once, on purpose
 *
 * React 19's StrictMode mounts effects twice in development. A plan that survived being read
 * would fork twice — two `claude` children sharing a parent, one of them orphaned. Taking
 * the plan out of the map on read makes the second mount fall through to the ordinary
 * spawn-or-adopt path, which finds the session the first mount already bound.
 */
import type { SplitIntent } from '@/ipc/client'

const plans = new Map<string, SplitIntent>()

/** Record what a split asked for, before the pane is rendered. */
export function rememberSpawnPlan(pane: string, intent: SplitIntent): void {
  plans.set(pane, intent)
}

/** Take a pane's plan, if it has one. Removes it: a plan is good for one spawn. */
export function takeSpawnPlan(pane: string): SplitIntent | undefined {
  const plan = plans.get(pane)
  plans.delete(pane)
  return plan
}

/** Drop a plan for a pane that went away before it ever spawned. */
export function forgetSpawnPlan(pane: string): void {
  plans.delete(pane)
}
