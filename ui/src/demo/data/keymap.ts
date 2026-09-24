/**
 * The demo's `keymap_report`: the real compiled-in keymap from the bootstrap, with the handful of
 * personal edits somebody who lives in panes makes — so the Source column shows both layers and
 * the path line counts real overrides rather than saying the file is empty.
 */
import type { Binding, KeymapReport, ResolvedBinding } from '../../ipc/generated'

/** What `~/.config/cide/keymap.json` holds, in file order. */
const OVERRIDES: Binding[] = [
  // Alt+←/→ belong to the shell's word motion in a terminal; the vim-shaped chords stay.
  { key: 'alt+left', command: '-pane.navigate.left', when: null, args: null },
  { key: 'alt+right', command: '-pane.navigate.right', when: null, args: null },
  { key: 'ctrl+shift+m', command: 'pane.maximize', when: null, args: null },
  { key: 'ctrl+alt+e', command: 'pane.evenRow', when: null, args: null },
  { key: 'ctrl+k ctrl+g', command: 'git.pull', when: null, args: null },
]

export function keymapReport(defaults: readonly ResolvedBinding[]): KeymapReport {
  const removed = new Set(OVERRIDES.filter((o) => o.command.startsWith('-')).map((o) => `${o.key} ${o.command.slice(1)}`))
  const bindings: ResolvedBinding[] = [
    ...defaults.filter((b) => !removed.has(`${b.key} ${b.command}`)),
    ...OVERRIDES.filter((o) => !o.command.startsWith('-')).map((o): ResolvedBinding => ({ ...o, layer: 'user' })),
  ]
  return {
    bindings,
    conflicts: [],
    problems: [],
    overrides: OVERRIDES,
    readable: true,
    path: '/home/dev/.config/cide/keymap.json',
  }
}
