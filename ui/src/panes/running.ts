/**
 * The live "what is working in each project" table, and the hook the header's project tabs
 * read. (M94)
 *
 * `runningRule.ts` owns the decisions and is compiled on its own by a check script; this file
 * owns the part that needs a webview: one listener on `cide://project-running`, one catch-up
 * because that event only reports *changes*, and a `useSyncExternalStore` subscription per tab.
 *
 * # Why this is so much smaller than `awaiting.ts`, which it otherwise mirrors
 *
 * That module has a fold, an acknowledgement path and a report direction, because the *decision*
 * it makes genuinely lives here: a click or a keystroke into a pane is a webview event Rust
 * never sees, so the frontend has something Rust does not and has to send it up.
 *
 * Nothing here is acknowledged. What a run's state is, whether a pane's child is alive, whether
 * a session belongs to a run rather than to a person, and what the hook server last heard — all
 * four live in Rust registries, and are needed **for projects this window is not drawing**,
 * which is the entire point of putting the number on a background project's tab. The frontend
 * has no input to contribute, so it contributes none: one listener, one catch-up, no report
 * direction at all. `cide_app::running`'s header carries the argument from the other side.
 *
 * # Installation
 *
 * Lazily, from the first `useRunningInProject` — that is, from the first project tab to render
 * in this window. `awaiting.ts`'s rule, and for its reason: *a feature that only works when
 * someone remembers to install it is a feature that is off until it is filed as a bug, and this
 * app has shipped three of those.* Any window with a header has project tabs; a window with none
 * has nothing to count.
 */
import { useSyncExternalStore } from 'react'

import { onProjectRunning, running as runningApi } from '@/ipc/client'
import type { ProjectRunning } from '@/ipc/generated'

import {
  foldRunning,
  isNewer,
  runningIn,
  runsIn,
  type Counts,
  type RunningEntry,
} from './runningRule'

/**
 * The equivalence check between the generated wire type and the rule module's own shape.
 *
 * `runningRule.ts` cannot import the bindings — it is compiled standalone — so this function is
 * what fails to typecheck if `ProjectRunning` gains, renames or retypes a field, instead of the
 * new spelling reading `undefined` into a badge that silently says nothing.
 * `awaiting.ts`'s `phaseOf` is the same device for the same reason.
 */
const entryOf = (entry: ProjectRunning): RunningEntry => ({
  project: entry.project,
  runs: entry.runs,
  panes: entry.panes,
})

let counts: Counts = new Map()

/**
 * The generation of the set currently in `counts`.
 *
 * `-1` rather than `0`, because Rust's counter starts at `0` and `isNewer` is a strict
 * comparison — a window that started at `0` would discard the very first set it was told about
 * and then look correct for ever after, which is the shape of bug that is found by a user and
 * not by a test.
 */
let seen = -1

const subscribers = new Set<() => void>()

/**
 * Take a set, if it is news.
 *
 * Every subscriber is notified on every accepted set, and each one's `getSnapshot` returns a
 * plain number for its own project. `useSyncExternalStore` compares with `Object.is`, so a
 * per-project scalar means a tab re-renders only when *its* count moves, however many other
 * projects are working. Handing out the `Map` instead would re-render every tab in the header on
 * every tool call of every agent in the app.
 */
function accept(projects: readonly ProjectRunning[], at: number): void {
  if (!isNewer(seen, at)) return
  seen = at
  counts = foldRunning(projects.map(entryOf))
  for (const notify of subscribers) notify()
}

let installed = false

function install(): void {
  if (installed) return
  installed = true

  // Nothing else can supply the counts, so a failure here leaves the feature off in this window
  // rather than wrong: no chip, and nothing that could contradict another window's.
  void onProjectRunning(accept).catch(() => {})

  // The other half of a change notification, and not a nicety: the event fires when the answer
  // *moves*, so a window that opens between two moves has heard nothing — and a window built by
  // a detach is precisely that. Unlike the awaiting set there is nothing such a window could
  // work out for itself, because none of the facts are in its tree.
  void runningApi
    .current()
    .then((set) => accept(set.projects, set.at))
    .catch(() => {
      /* A window that cannot ask still follows every broadcast from here on. */
    })
}

function subscribe(notify: () => void): () => void {
  install()
  subscribers.add(notify)
  return () => {
    subscribers.delete(notify)
  }
}

/**
 * How many things are working in one project — agent runs plus console panes.
 *
 * Takes the **project id**, not the project object, and that is the whole difference from
 * `useAwaitingInProject`: that count is a fold over sessions held by panes in *this window's*
 * tree, while this one is Rust's answer over registries and over panes that may not be in this
 * window at all. There is nothing on a `Project` this could read, which is also what makes it
 * correct for a project whose panes are mounted nowhere.
 */
export function useRunningInProject(project: string): number {
  return useSyncExternalStore(
    subscribe,
    () => runningIn(counts, project),
    () => 0,
  )
}

/**
 * The agent-run half alone, which is what the tooltip needs to word itself.
 *
 * A second scalar subscription rather than one hook returning both numbers: an object snapshot
 * is a fresh reference on every notification, so `useSyncExternalStore`'s `Object.is` comparison
 * would never hold and every project tab would re-render whenever *any* project's count moved —
 * which is the exact cost the scalar shape exists to avoid.
 */
export function useRunsInProject(project: string): number {
  return useSyncExternalStore(
    subscribe,
    () => runsIn(counts, project),
    () => 0,
  )
}
