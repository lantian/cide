/** Status-bar labels, independent of pane focus and the DOM. */
import type { ConsoleHarness, HarnessCliVersion, ProjectId, Settings } from '@/ipc/client'
import type { ProjectHarnessSettings } from '@/ipc/generated'

export interface HarnessReadout {
  text: string
  title: string
}

const NAMES: Record<ConsoleHarness, string> = {
  claude: 'Claude Code',
  codex: 'Codex',
  opencode: 'OpenCode',
}

export const HARNESS_LOADING: HarnessReadout = {
  text: 'Harness · …',
  title: 'Reading the configured default console harness version.',
}

/** Only launch identity enters the key; editor changes and session ticks never re-probe. */
export function harnessVersionKey(
  project: ProjectId | null,
  settings: Settings | null,
  overrides: Record<string, ProjectHarnessSettings> | undefined,
): string | null {
  if (settings === null) return null
  return JSON.stringify([
    project,
    settings.consoleHarness,
    settings.claude.cli.binary,
    settings.codex.cli.binary,
    settings.opencode.cli.binary,
    Object.entries(overrides ?? {}).map(([root, config]) => [
      root, config.consoleHarness, config.claude?.binary, config.codex?.binary, config.opencode?.binary,
    ]),
  ])
}

export function defaultHarnessReadout(info: HarnessCliVersion | null): HarnessReadout {
  if (info === null) return {
    text: 'Harness · version unavailable',
    title: 'Could not read the configured default console harness version.',
  }
  const name = NAMES[info.harness]
  let version = info.version?.trim() ?? ''
  if (info.harness === 'codex') version = version.replace(/^codex-cli\s+/, '')
  if (info.harness === 'claude') version = version.replace(/\s*\(Claude Code\)$/, '')
  return {
    text: `${name} · ${version || 'version unavailable'}`,
    title: `Default console harness: ${name}\nBinary: ${info.binary}`,
  }
}

/** A focused session's figures keep priority, even if its CLI differs from the default. */
export function statusHarnessReadout(live: string | undefined, fallback: HarnessReadout): HarnessReadout {
  if (live === undefined) return fallback
  const harness = live.split(' · ')[0] ?? ''
  const name = harness === 'claude' ? 'Claude' : NAMES[harness as ConsoleHarness] ?? harness
  return { text: live, title: `${name} session` }
}
