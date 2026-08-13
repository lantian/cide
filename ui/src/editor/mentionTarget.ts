/**
 * Which Claude the editor's *@-mention this selection* item should type into.
 *
 * `claudeSend.lines` is addressed — it needs a `PaneId` — and only the window knows which
 * conversation the user was last looking at. `App.tsx` already works that out for Ctrl+P's
 * ⌥⏎ and calls it `mentionTarget`; this is the same rule, derived from the same mirror, for a
 * caller that is not in `App.tsx` and cannot be handed a prop by it.
 *
 * What this answers is where the mention is *aimed*, not where it lands. Nothing the webview
 * can see says whether a Claude pane has a `claude` on cide's IDE server — a pane at a resume
 * splash looks identical here to one mid-conversation — so `claude_send_lines` treats this as
 * a preference and answers with the pane it actually used. Callers reveal *that* one.
 *
 * # Why it is derived here rather than threaded down as a prop
 *
 * `EditorSurface` is three components below `App.tsx` (`TabContent` → `PaneBody` →
 * `EditorPane` → here), and `EditorPane` is not this change's file. Threading a `mentionPane`
 * prop through all of them is four edits in files owned elsewhere, and — the part that
 * matters — a feature that is switched off until every one of them lands. `useContextMenu`
 * makes the same call for the same reason and says so: "threading a `keymap` prop down to each
 * of them would be five call sites in `App.tsx` that all have to be remembered — the shape that
 * has already left three controls in this app wired to nothing."
 *
 * The cost is that this duplicates a rule `App.tsx` also states. It is stated once here, in a
 * function whose name is greppable from there, rather than inline at a call site.
 */
import { useMemo } from 'react'
import { useWorkspace } from '@/store/workspace'
import type { PaneId, ProjectId } from '@/ipc/client'

export interface MentionTarget {
  project: ProjectId
  pane: PaneId
}

/**
 * The separator inside the packed selector result below.
 *
 * `'\u0000'` written as an escape, never as a literal NUL byte in the source. A raw NUL makes
 * `git` classify this file as binary: no diff, no merge, no `git grep`, and a review of it
 * shows `Bin 0 -> 3990 bytes`. The escape compiles to exactly the same character and costs
 * nothing — a NUL is still the right separator, because it cannot occur in a `ProjectId` or a
 * `PaneId` and so no id can forge a split point.
 */
const PACK = '\u0000'

/**
 * The project this window is showing and the Claude pane a mention should land in, or `null`
 * when there is no such pair.
 *
 * Both roles are covered, and that is the whole reason this is not `useActiveProject()`:
 * a `shell:` window names its active project in `role.active`, while a detached pane or tab
 * window has no active project at all and names its own in `role.project`. An editor detached
 * into its own window is a normal thing to do and the mention has to keep working there — it
 * mentions into the project's console session, which is still running in the shell.
 *
 * The pane is the focused one when it is a Claude, otherwise the project's console — its first
 * tab's Claude pane, which is the conversation the project is *about*. Falling back rather than
 * returning `null` matters because the gesture is made *from an editor*, where by definition no
 * Claude pane is focused: without the fallback this would be disabled every single time.
 */
export function useMentionTarget(): MentionTarget | null {
  /*
   * The selector returns a *string*, not the record.
   *
   * zustand compares a selector's result with `Object.is` on every store notification, so a
   * selector that builds an object returns a new identity every time and re-renders for ever.
   * Two ids and a separator is the cheapest stable value that carries both; the record is
   * rebuilt only when the string actually moves.
   */
  const packed = useWorkspace((s) => {
    const boot = s.boot
    if (boot === null) return null

    const project = boot.role.kind === 'shell' ? boot.role.active : boot.role.project
    if (project === null || project === undefined) return null
    const open = boot.workspace.projects[project]
    if (open === undefined) return null

    const active = open.tabs.find((t) => t.id === open.activeTab)
    const focused = active === undefined ? undefined : active.tree.panes[active.tree.focused]
    if (focused?.kind === 'claude') return `${project}${PACK}${focused.id}`

    const consoleTab = open.tabs[0]
    if (consoleTab === undefined) return null
    const pane = Object.values(consoleTab.tree.panes).find((p) => p.kind === 'claude')
    return pane === undefined ? null : `${project}${PACK}${pane.id}`
  })

  return useMemo(() => {
    if (packed === null) return null
    const [project, pane] = packed.split(PACK)
    return project === undefined || pane === undefined ? null : { project, pane }
  }, [packed])
}
