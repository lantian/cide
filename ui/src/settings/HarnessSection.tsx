/** Console selection and launch overrides. The owner supplies the effective settings and writes. */
import type { ConsoleHarness } from '@/ipc/client'
import type { SectionProps } from './sections'
import { Group, Row, Readout, Segmented, Toggle, resetTo, useSettingsDefaults } from './controls'
import { InfoPara } from '@/kit/components/InfoTip'
import { ClaudeCliSection } from './ClaudeCliSection'
import { CodexCliSection } from './CodexCliSection'
import { OpencodeCliSection } from './OpencodeCliSection'
import styles from './HarnessSection.module.css'

export const HARNESSES: readonly { value: ConsoleHarness; label: string }[] = [
  { value: 'claude', label: 'Claude Code' },
  { value: 'codex', label: 'Codex' },
  { value: 'opencode', label: 'OpenCode' },
]

/**
 * Which CLI a console runs. (M93)
 *
 * Read at one moment only — when a console spawns a *fresh* conversation — so the hint says
 * exactly that: an open console keeps the CLI it was started with, and so does its Resume, and
 * Restart session is how a user moves one over. Agent runs are not governed by it: a role runs
 * the harness its definition names.
 */
export function HarnessChoice({ settings, patch, projectHarness, editProjectHarness }: SectionProps) {
  const def = useSettingsDefaults()
  const projectScope = projectHarness !== undefined
  const reset = projectScope
    ? { modified: projectHarness.consoleHarness != null, onReset: () => editProjectHarness?.({ field: 'consoleHarness', value: null }) }
    : resetTo(settings.consoleHarness, def?.consoleHarness, (consoleHarness) => patch({ consoleHarness }))
  return (
    <Group title="Console">
      <div className={styles.choiceRow}>
        <Row
          label="Harness"
          hint="The CLI a new console runs."
          info={
            <>
              <InfoPara>
                Which CLI a new console runs — the project’s pinned tab, a new pane, a tab cide
                opens by itself, and the one-shots behind Generate commit message.
              </InfoPara>
              <InfoPara>
                A console that is already running keeps its CLI and its Resume; Restart session
                starts the one chosen here. Roles keep the harness their definition names; matching roles
                use this project’s launch configuration.
              </InfoPara>
            </>
          }
          {...reset}
          control={
            <Segmented
              label="Console harness"
              value={projectScope ? projectHarness.consoleHarness ?? 'global' : settings.consoleHarness}
              options={projectScope ? [{ value: 'global' as const, label: 'Use global' }, ...HARNESSES] : HARNESSES}
              onChange={(value) => {
                if (projectScope) editProjectHarness?.({ field: 'consoleHarness', value: value === 'global' ? null : value as ConsoleHarness })
                else patch({ consoleHarness: value as ConsoleHarness })
              }}
            />
          }
        />
      </div>
    </Group>
  )
}

/** Harness → Launch: the chosen CLI's binary, arguments and environment. (M133) */
export function HarnessLaunch(props: SectionProps) {
  const { settings, patch, cliSupport, codexSupport, opencodeSupport, projectHarness, editProjectHarness } = props
  const harness = settings.consoleHarness
  const overridden = projectHarness?.[harness] !== undefined
  const inheritance = projectHarness === undefined ? null : (
    <Group title="Project launch">
      <Row
        label="Override global launch"
        hint="Use a local launch configuration for this project."
        modified={overridden}
        onReset={() => editProjectHarness?.({ field: harness, value: null })}
        control={
          <Toggle
            label="Override global launch"
            checked={overridden}
            onChange={(value) => {
              if (!value) editProjectHarness?.({ field: harness, value: null })
              else if (harness === 'claude') editProjectHarness?.({ field: 'claude', value: settings.claude.cli })
              else if (harness === 'codex') editProjectHarness?.({ field: 'codex', value: settings.codex.cli })
              else editProjectHarness?.({ field: 'opencode', value: settings.opencode.cli })
            }}
          />
        }
      />
    </Group>
  )
  if (projectHarness !== undefined && !overridden) {
    return <>{inheritance}<Group>
      <Row
        label="Inherited launch"
        hint="Enable the override to edit this project’s launch settings."
        control={<Readout text={settings[harness].cli.binary} />}
      />
    </Group></>
  }
  let editor
  if (harness === 'opencode') editor = <OpencodeCliSection cli={settings.opencode.cli} onChange={(cli) => patch({ opencode: { cli } })} support={opencodeSupport ?? null} />
  else if (harness === 'codex') editor = <CodexCliSection cli={settings.codex.cli} onChange={(cli) => patch({ codex: { ...settings.codex, cli } })} support={codexSupport} />
  else editor = <ClaudeCliSection cli={settings.claude.cli} onChange={(cli) => patch({ claude: { ...settings.claude, cli } })} support={cliSupport} />
  return <>{inheritance}{editor}</>
}
