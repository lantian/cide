import type { OpencodeCli, OpencodeInjections } from '@/ipc/generated'
import type { CodexCliSupport } from '@/ipc/client'
import { Group, Row, Toggle, resetTo, useSettingsDefaults } from './controls'
import { Add, Argv, ProgramInput, Remove, Why } from './ClaudeCliSection'
import styles from './ClaudeCliSection.module.css'

const INJECTIONS: { key: keyof OpencodeInjections; label: string; hint: string }[] = [
  { key: 'events', label: 'Session events', hint: 'Off: the console cannot report busy or awaiting.' },
  { key: 'mcpConfig', label: 'cide’s MCP server', hint: 'Off: the console has no cide tools.' },
  { key: 'instructions', label: 'Roster instructions', hint: 'Off: cide’s role and tool instructions are omitted.' },
  { key: 'resume', label: 'Resume', hint: 'Off: restored panes start a new conversation.' },
  { key: 'fork', label: 'Fork', hint: 'Off: a fork starts a new conversation.' },
]

export function OpencodeCliSection({ cli, onChange, support }: {
  cli: OpencodeCli; onChange: (cli: OpencodeCli) => void; support: CodexCliSupport | null
}) {
  const current = support?.binary === cli.binary.trim() ? support : null
  const def = useSettingsDefaults()?.opencode.cli
  return <>
    <Group title="Binary">
      <Row label="Program" hint="What OpenCode consoles, agent runs, and one-shots run."
        status={current?.problem ? { tone: 'bad', text: current.problem } : undefined}
        {...resetTo(cli.binary, def?.binary, (binary) => onChange({ ...cli, binary }))}
        control={<ProgramInput label="OpenCode program" value={cli.binary} placeholder="opencode" onCommit={(binary) => onChange({ ...cli, binary })} />} />
      <Row label="Version" control={<span>{current?.version ?? '…'}</span>} />
    </Group>
    <Group title="Arguments">
      <div className={styles.list}>
        {cli.args.map((arg, index) => <div className={styles.listRow} key={`${index}:${arg}`}>
          <input className={styles.input} aria-label={`OpenCode argument ${index + 1}`} defaultValue={arg} spellCheck={false}
            onBlur={(e) => { if (e.target.value !== arg) onChange({ ...cli, args: cli.args.map((v, i) => i === index ? e.target.value : v) }) }} />
          <Remove label={`Remove OpenCode argument ${index + 1}`} onClick={() => onChange({ ...cli, args: cli.args.filter((_, i) => i !== index) })} />
          <Why text={current?.argNotes.find((note) => note.index === index)?.reason ?? null} />
        </div>)}
        <Add label="Add argument" onClick={() => onChange({ ...cli, args: [...cli.args, ''] })} />
      </div>
    </Group>
    <Group title="Environment" info="Launch environments are stored locally with file permissions 0600. cide’s connection and configuration variables are reserved.">
      <div className={styles.list}>
        {cli.env.map((pair, index) => <div className={styles.listRow} key={`${index}:${pair.name}:${pair.value}`}>
          <input className={`${styles.input} ${styles.name}`} aria-label={`OpenCode variable ${index + 1} name`} defaultValue={pair.name} spellCheck={false}
            onBlur={(e) => { if (e.target.value !== pair.name) onChange({ ...cli, env: cli.env.map((v, i) => i === index ? { ...v, name: e.target.value } : v) }) }} />
          <input className={styles.input} aria-label={`OpenCode variable ${index + 1} value`} defaultValue={pair.value} spellCheck={false}
            onBlur={(e) => { if (e.target.value !== pair.value) onChange({ ...cli, env: cli.env.map((v, i) => i === index ? { ...v, value: e.target.value } : v) }) }} />
          <Remove label={`Remove OpenCode variable ${index + 1}`} onClick={() => onChange({ ...cli, env: cli.env.filter((_, i) => i !== index) })} />
          <Why text={current?.envNotes.find((note) => note.index === index)?.reason ?? null} />
        </div>)}
        <Add label="Add variable" onClick={() => onChange({ ...cli, env: [...cli.env, { name: '', value: '' }] })} />
      </div>
    </Group>
    <Group title="Console integration">
      {INJECTIONS.map(({ key, label, hint }) => <Row key={key} label={label} hint={hint}
        {...resetTo(cli.inject[key], def?.inject[key], (value) => onChange({ ...cli, inject: { ...cli.inject, [key]: value } }))}
        control={<Toggle label={label} checked={cli.inject[key]} onChange={(value) => onChange({ ...cli, inject: { ...cli.inject, [key]: value } })} />} />)}
    </Group>
    {current && <Group title="Console command"><Argv parts={current.argv} /></Group>}
  </>
}
