/**
 * Switch tabs and, when the tab's focused pane is a console, arrive in it.
 *
 * > *"When selecting tab with harness console - need to focus console, not tab, when tab
 * > opens"*
 *
 * `ws.activateTab` alone makes the tab visible and leaves the keyboard where the gesture put it —
 * on the strip's button after a click, or in an element the previous tab just hid (`TabContent`
 * hides with `visibility: hidden`, which blurs). So the first keystroke after picking a claude,
 * codex or opencode tab went nowhere, and the user had to click into the terminal as well.
 *
 * Why not `revealPane`, which the Ctrl+Tab switcher uses: it also scrolls the terminal to the
 * bottom, which is right for a mention that just landed and wrong for a plain tab switch — a
 * user who had scrolled up through a transcript would lose their place every time they came
 * back to the tab. And the domain's focus needs no move here: the tab's `tree.focused` is the
 * pane being focused, so the DOM is being brought into line with the domain, not the reverse.
 *
 * Only a **terminal** takes the caret. An editor tab keeps today's behaviour; the request was
 * about consoles, and a document pane has its own opinions about where a click should leave the
 * selection.
 */
import { peekHost } from '@/layout/paneHosts'
import type { ProjectId, TabId } from '@/ipc/client'
import { useWorkspace } from '@/store/workspace'

/**
 * Resolve once React has committed the tab switch and the browser has laid out against it —
 * the browser refuses focus to an element still under `visibility: hidden`, so focusing before
 * this is a call that silently does nothing. Two frames for the reason `editor/revealPane.ts`
 * gives for its copy.
 */
function committed(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
  })
}

export async function activateTabFocused(project: ProjectId, tab: TabId): Promise<void> {
  await useWorkspace.getState().activateTab(project, tab)
  await committed()
  // Read after the switch, not before: the mirror the click saw may predate a focus move the
  // broadcast just carried in.
  const found = useWorkspace
    .getState()
    .boot?.workspace.projects[project]?.tabs.find((t) => t.id === tab)
  if (found === undefined) return
  // Absent for a document pane, a pane in another window, or a host not yet rebuilt — in each
  // case the tab still switched, which is what was asked for.
  peekHost(found.tree.focused)?.terminal?.term.focus()
}
