/**
 * Synthetic chrome state for the layout audit.
 *
 * A freshly opened project has one console tab, a clean working tree and no editor, so a
 * live app never renders a file tab, a language badge or a git badge — and the audit
 * reports those dimensions as absent, which it counts as a failure. That is the right
 * default (a check that silently skips is indistinguishable from one that passes), but it
 * means the audit measures whatever the app happens to be showing rather than every state
 * the chrome has to get right.
 *
 * This is the fixture that closes the gap. It exists because every chrome component takes
 * its state as props and reads nothing from the store: they can be driven from here with
 * no workspace, no processes and no IO.
 */
import type { Tab } from '@/ipc/client'

/**
 * A tab list covering every shape the strip renders: the pinned console, a dirty file tab,
 * a clean file tab whose extension has its own badge colour, and the settings tab.
 */
export const AUDIT_TABS: Tab[] = [
  {
    id: 'audit-console',
    kind: { kind: 'claudeHome' },
    tree: auditTree('audit-pane-console'),
  },
  {
    id: 'audit-file-dirty',
    kind: { kind: 'file', path: '/home/dev/work/cide/crates/cide-core/src/lib.rs', dirty: true },
    tree: auditTree('audit-pane-rs'),
  },
  {
    id: 'audit-file-clean',
    kind: { kind: 'file', path: '/home/dev/work/cide/ui/src/App.tsx', dirty: false },
    tree: auditTree('audit-pane-tsx'),
  },
  {
    id: 'audit-settings',
    kind: { kind: 'settings', section: 'appearance' },
    tree: auditTree('audit-pane-settings'),
  },
]

/** The console tab is the active one, matching what a real project opens on. */
export const AUDIT_ACTIVE_TAB = 'audit-console'

/**
 * A single-leaf tree. The audit never looks inside a pane — the pane grid is M4's surface —
 * so this only has to satisfy the type.
 */
function auditTree(pane: string): Tab['tree'] {
  return {
    root: { kind: 'leaf', pane },
    focused: pane,
    maximized: null,
    panes: {
      [pane]: {
        id: pane,
        kind: 'claude',
        role: 'primary',
        session: null,
        title: 'audit',
      },
    },
  }
}
