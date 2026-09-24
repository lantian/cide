/**
 * What the pinned console tab is called: the CLI its console pane runs. (M93)
 *
 * "Claude" until M93, a literal in four places (`TabStrip::viewFor`, `menuModel::overflowLabel`,
 * `Switcher::tabRow`, `DetachedTabHeader::tabTitle`). The console can be a codex TUI now, and a
 * tab reading "Claude" over codex is the kind of small lie the user has to reconcile. The answer
 * is the **pane's** recorded harness (`Pane::harness`, stamped when a spawn binds), not Settings →
 * Harness: the setting names the *next* fresh console, and the tab is about the one that exists.
 *
 * Import-free and structurally typed, so the modules that call it stay compilable on their own
 * — `menuModel.ts` is compiled standalone by a check script.
 */
export interface ConsoleTabLike {
  tree: {
    panes: Readonly<
      Record<string, { role: string; kind: string; harness?: string | null } | undefined>
    >
  }
}

export type ConsoleName = 'Claude' | 'Codex'

export function consoleName(tab: ConsoleTabLike | null | undefined): ConsoleName {
  if (tab == null) return 'Claude'
  // Through `?.`, for a fixture or an older window's snapshot that carries no tree.
  for (const pane of Object.values(tab.tree?.panes ?? {})) {
    if (pane !== undefined && pane.role === 'primary' && pane.kind === 'claude') {
      return pane.harness === 'codex' ? 'Codex' : 'Claude'
    }
  }
  return 'Claude'
}
