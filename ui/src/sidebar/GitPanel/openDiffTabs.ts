/**
 * Whether a git diff tab is open in a project **right now**.
 *
 * # Why this is not the panel's own memory
 *
 * > *"Only when diff is already opened one click should change current diff to selected file.
 * > Otherwise diff should be opened only via double click."*
 *
 * That rule is a state dependency, and the state is not the sidebar's. A diff tab is a tab in
 * the Rust-owned workspace: it is written to `workspace.json` and survives a restart, a second
 * window can open one, and the user can close it with the tab's `×` while the sidebar is
 * looking the other way. A flag this panel set when it last opened a diff would be right the
 * first time and wrong every time afterwards — and wrong in the direction that makes a single
 * click throw a diff in the user's face, which is precisely the interruption being fixed.
 *
 * So the answer comes from the workspace snapshot, which every window already receives:
 * `app_get_bootstrap` once, then `cide://workspace-changed` for ever after. Opening,
 * activating and closing a tab are all workspace mutations, so the answer arrives on the same
 * event that changes it and there is no polling anywhere.
 *
 * # Why not `store/workspace`
 *
 * That store holds exactly this snapshot and following it would be one selector. It is also
 * unreachable from here: `store/workspace` imports `layout/paneHosts`, which calls
 * `document.createElement` at module scope and pulls the xterm addons in behind it, and
 * `ui/scripts/check-git-render.mjs` renders this panel under node with `react-dom/server` and
 * no DOM at all. `useGitPanel` already refuses the same import for the same reason — see the
 * note on `showDiff`. This module imports `@/ipc/client` and nothing else, so the panel's
 * render check keeps working.
 *
 * # "A diff tab", not "the diff tab for this file"
 *
 * The question the click rule asks is whether the user is *reading diffs*. Any git diff tab in
 * the project answers that; whether it happens to be showing the file under the pointer is a
 * different question and the wrong one — under the narrower rule the first click on every new
 * file would only select, which is exactly the behaviour the user asked to be rid of once a
 * diff is up.
 *
 * `panes/diffTabs.ts` answers the narrow question, for the pane that has to know whether *it*
 * is on screen. The two are deliberately separate.
 */
import { useCallback, useSyncExternalStore } from 'react'
import { app as appApi, events, type ProjectId, type Workspace } from '@/ipc/client'

/** Projects that currently hold at least one git diff tab. */
let holders: ReadonlySet<string> = new Set()
const listeners = new Set<() => void>()
let started = false

function scan(workspace: Workspace): ReadonlySet<string> {
  const found = new Set<string>()
  for (const [id, project] of Object.entries(workspace.projects)) {
    if (project === undefined) continue
    for (const tab of project.tabs) {
      // `origin.kind === 'git'` and not merely `kind === 'diff'`: a Claude diff is also a diff
      // tab, and a review of Claude's edits is not the user reading git diffs — clicking a
      // changelist row would start switching a tab that has nothing to do with the sidebar.
      if (tab.kind.kind === 'diff' && tab.kind.spec.origin.kind === 'git') {
        found.add(id)
        break
      }
    }
  }
  return found
}

function apply(workspace: Workspace): void {
  const next = scan(workspace)
  // Compared rather than written through: `cide://workspace-changed` fires on every accepted
  // mutation in every window — every keystroke that marks a tab dirty, every pane focus — and
  // notifying on all of them would re-render the whole changes tree several times a second
  // during a turn.
  if (next.size === holders.size && [...next].every((id) => holders.has(id))) return
  holders = next
  for (const fn of listeners) fn()
}

/**
 * Start following the workspace, once per window.
 *
 * Never torn down. The subscription is one listener for the lifetime of the window and the
 * panel it serves is mounted and unmounted every time the user touches the activity rail;
 * refcounting it would mean re-reading the bootstrap on every visit to the Git tab to answer a
 * question whose answer had not changed.
 *
 * Only ever called from `subscribe`, which React calls in an effect — so it does not run
 * during a server render, which is what keeps `check-git-render.mjs` free of IPC.
 */
function start(): void {
  if (started) return
  started = true
  // A failure here is not worth a notice: the honest fallback is "no diff is open", which
  // makes single-click select-only — the conservative half of the rule, and the one that
  // never surprises anyone.
  void appApi
    .getBootstrap()
    .then((boot) => apply(boot.workspace))
    .catch(() => {})
  void events.onWorkspaceChanged(apply).catch(() => {})
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn)
  start()
  return () => {
    listeners.delete(fn)
  }
}

/** The unsubscribe handed back when there was nothing to subscribe to. */
const NOOP = (): void => {}

/**
 * Whether this project has a git diff tab open.
 *
 * `false` for `null` — a panel with no project has nothing open — and the subscription is not
 * started at all in that case, which is what keeps the fixture stories (`?git-story=…`, which
 * render with `project={null}`) from reaching Rust.
 */
export function useGitDiffTabOpen(project: ProjectId | null): boolean {
  const listen = useCallback(
    (fn: () => void) => (project === null ? NOOP : subscribe(fn)),
    [project],
  )
  return useSyncExternalStore(
    listen,
    () => project !== null && holders.has(project),
    // Server render: no workspace, no tabs, nothing open.
    () => false,
  )
}
