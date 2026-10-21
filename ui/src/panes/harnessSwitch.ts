/** The console menu's one-pane harness choice, independent of terminal and React state. */
import type { ConsoleHarness, Pane } from '@/ipc/client'
import type { MenuEntry } from '@/menus'

const LABELS: Record<ConsoleHarness, string> = {
  claude: 'Claude Code',
  codex: 'Codex',
  opencode: 'OpenCode',
}

export function harnessSwitchEntry(
  pane: Pick<Pane, 'kind' | 'continues' | 'origin'>,
  mirrored: boolean,
  current: ConsoleHarness,
  ready: boolean,
  busy: boolean,
  choose: (harness: ConsoleHarness) => void,
): MenuEntry | null {
  if (pane.kind !== 'claude' || pane.continues != null || pane.origin != null || mirrored) {
    return null
  }
  return {
    id: 'switch-harness',
    label: 'Switch to another harness',
    disabledReason: ready ? undefined : 'This pane cannot restart yet',
    submenu: () => (Object.keys(LABELS) as ConsoleHarness[]).map((value) => ({
      id: `switch-harness-${value}`,
      label: LABELS[value],
      checked: value === current,
      run: value === current || busy ? undefined : () => choose(value),
      disabledReason: value === current
        ? 'Already running in this pane'
        : busy ? 'This pane is restarting' : undefined,
    })),
  }
}
