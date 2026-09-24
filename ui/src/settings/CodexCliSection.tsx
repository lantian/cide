/**
 * *How codex is launched* — Settings → Harness → Codex. (M93)
 *
 * `ClaudeCliSection`'s twin: the binary, extra arguments, extra environment, which of cide's own
 * additions it still makes, and the command line a console actually gets. Presentational like
 * it — the stored value, an `onChange`, and Rust's readout — so the screen renders from a
 * fixture.
 *
 * # One difference, deliberate
 *
 * The claude screen strikes a token out *as it is typed*, from a TypeScript port of
 * `cide_core::claude_cli` that `check-claude-cli.mjs` holds to the Rust. This one has no port:
 * every verdict — each row's, the binary's, and the resolved argv — arrives from
 * `codex_cli_support`, computed by the same `cide_core::codex_cli::plan` a spawn runs. The rows
 * commit on blur, so the readout is one round trip behind a keystroke and never behind a commit,
 * and there is no second copy of the rules to drift.
 *
 * Everything is stored verbatim and nothing is rejected on save, for `ClaudeCliSection`'s
 * reason: a value you cannot see is a value you cannot correct.
 */
import type { CodexCli, CodexCliSupport, CodexInjections, ClaudeEnvVar } from '@/ipc/client'
import { Group, Note, PathReadout, Row, Toggle } from './controls'
import { Add, Argv, Remove, TextField, Why } from './ClaudeCliSection'

import styles from './ClaudeCliSection.module.css'

export interface CodexCliSectionProps {
  cli: CodexCli
  onChange: (next: CodexCli) => void
  /** Rust's readout. `null` before the first answer. */
  support: CodexCliSupport | null
}

/**
 * What switching each addition off costs, on its row — `ClaudeCliSection`'s `INJECTION_COPY`
 * argument: every one of these silently disables something the user will report as broken
 * without connecting it to this screen.
 */
const INJECTIONS: readonly { key: keyof CodexInjections; label: string; cost: string }[] = [
  {
    key: 'hooks',
    label: 'Hooks (-c hooks.* and --dangerously-bypass-hook-trust)',
    cost: 'Off: no state at all. The pane never shows busy or awaiting, its thread is never learned — so Resume and a restore after restart cannot find the conversation — and a line cide types waits for a readiness it never hears.',
  },
  {
    key: 'mcpConfig',
    label: 'cide’s MCP server (-c mcp_servers.cide.*)',
    cost: 'Off: no mcp__cide__* tools. No task tracker, and on a console no subagents.',
  },
  {
    key: 'developerInstructions',
    label: 'The roster paragraph (-c developer_instructions)',
    cost: 'Off: the console is not told what cide’s tools are for, and a run is not given its brief.',
  },
  {
    key: 'resume',
    label: 'Resume (codex resume <thread>)',
    cost: 'Off: a restored or resumed pane starts a new conversation.',
  },
  {
    key: 'fork',
    label: 'Fork (codex fork <thread>)',
    cost: 'Off: Split → fork starts a fresh conversation instead of branching this one.',
  },
]

export function CodexCliSection({ cli, onChange, support }: CodexCliSectionProps) {
  // Only when the readout is about the value in the field — `ClaudeCliSection`'s rule.
  const current = support !== null && support.binary === cli.binary.trim() ? support : null
  const argNote = (index: number) => current?.argNotes.find((n) => n.index === index) ?? null
  const envNote = (index: number) => current?.envNotes.find((n) => n.index === index) ?? null
  const inject = cli.inject

  return (
    <>
      <Group title="Binary">
        <TextField
          label="Program"
          hint="What every codex console, every run of a role whose harness is codex, and the one-shots behind Generate commit message run when Harness is Codex. “codex” — the default — is resolved by your PATH at each spawn: the standalone installer updates codex in place, and resolving it once would pin a release it may since have removed. An absolute path or a wrapper script works too."
          value={cli.binary}
          placeholder="codex"
          onCommit={(binary) => onChange({ ...cli, binary })}
        />
        {current?.problem != null && (
          <Note title="That binary cannot be run" tone="warn">
            {current.problem} Until it is corrected, every codex console and codex run fails to
            start.
          </Note>
        )}
        {current?.problem == null && current?.resolved != null && (
          <Row
            label="Resolves to"
            hint={
              current.version ??
              'Nothing came back from --version. That is ordinary for a wrapper script and is not an error.'
            }
            control={<span />}
          />
        )}
        {current?.problem == null && current?.resolved != null && (
          <PathReadout path={current.resolved} />
        )}
      </Group>

      <Group title="Arguments">
        <div className={styles.list}>
          {cli.args.map((value, index) => {
            const note = argNote(index)
            return (
              <div key={`${index}:${value}`} className={styles.listRow}>
                <input
                  className={`${styles.input} ${note?.refused === true ? styles.refused : note?.reason != null ? styles.warned : ''}`}
                  type="text"
                  spellCheck={false}
                  autoCapitalize="off"
                  autoCorrect="off"
                  aria-label={`Codex argument ${index + 1}`}
                  placeholder="--search"
                  defaultValue={value}
                  onBlur={(e) => {
                    const next = [...cli.args]
                    next[index] = e.target.value
                    if (next[index] !== value) onChange({ ...cli, args: next })
                  }}
                />
                <Remove
                  label={`Remove codex argument ${index + 1}`}
                  onClick={() => onChange({ ...cli, args: cli.args.filter((_, i) => i !== index) })}
                />
                <Why text={note?.reason ?? null} />
              </div>
            )
          })}
          <Add label="Add argument" onClick={() => onChange({ ...cli, args: [...cli.args, ''] })} />
        </div>
        <Note title="One token per row">
          <code>-c</code> and <code>model="o3"</code> are two rows. They go right after the{' '}
          <code>resume</code>/<code>fork</code> subcommand and before everything cide adds.
          An override of a key cide writes itself — <code>hooks.*</code>,{' '}
          <code>mcp_servers.cide.*</code>, <code>developer_instructions</code> — is struck out
          while cide is still writing it.
        </Note>
      </Group>

      <Group title="Environment">
        <div className={styles.list}>
          {cli.env.map((pair, index) => {
            const note = envNote(index)
            const set = (next: ClaudeEnvVar) => {
              const env = [...cli.env]
              env[index] = next
              onChange({ ...cli, env })
            }
            return (
              <div key={`${index}:${pair.name}:${pair.value}`} className={styles.listRow}>
                <input
                  className={`${styles.input} ${styles.name} ${note?.refused === true ? styles.refused : note?.reason != null ? styles.warned : ''}`}
                  type="text"
                  spellCheck={false}
                  aria-label={`Codex variable ${index + 1} name`}
                  placeholder="NAME"
                  defaultValue={pair.name}
                  onBlur={(e) => {
                    if (e.target.value !== pair.name) set({ ...pair, name: e.target.value })
                  }}
                />
                <input
                  className={styles.input}
                  type="text"
                  spellCheck={false}
                  aria-label={`Codex variable ${index + 1} value`}
                  placeholder="value"
                  defaultValue={pair.value}
                  onBlur={(e) => {
                    if (e.target.value !== pair.value) set({ ...pair, value: e.target.value })
                  }}
                />
                <Remove
                  label={`Remove codex variable ${index + 1}`}
                  onClick={() => onChange({ ...cli, env: cli.env.filter((_, i) => i !== index) })}
                />
                <Why text={note?.reason ?? null} />
              </div>
            )
          })}
          <Add
            label="Add variable"
            onClick={() => onChange({ ...cli, env: [...cli.env, { name: '', value: '' }] })}
          />
        </div>
        <Note title="codex children only">
          Consoles, codex runs and the codex one-shot lane — never shell panes. Values are stored
          in plain text in <code>workspace.json</code> (written <code>0600</code>) and redacted
          from every log line. <code>CODEX_HOME</code> is refused: cide reads it from its own
          environment to find the conversation behind a pane, so export it before launching cide
          instead.
        </Note>
      </Group>

      <Group title="What cide adds to the command line">
        <div className={styles.injections}>
          {INJECTIONS.map((row) => (
            <div key={row.key} className={styles.injection}>
              <div className={styles.injectionHead}>
                <Toggle
                  label={row.label}
                  checked={inject?.[row.key] ?? true}
                  onChange={(on) => onChange({ ...cli, inject: { ...inject, [row.key]: on } })}
                />
                <span className={styles.injectionFlag}>{row.label}</span>
              </div>
              <p className={styles.injectionCost}>{row.cost}</p>
            </div>
          ))}
        </div>
        <Note title="Always added">
          <code>-C &lt;directory&gt;</code>, and two overrides that keep a startup modal from
          eating the first key cide types: <code>check_for_update_on_startup=false</code> (the
          update chooser’s first row is “Update now”) and{' '}
          <code>notice.hide_rate_limit_model_nudge=true</code>.
        </Note>
      </Group>

      <Group title="What a codex console is actually spawned with">
        {current !== null && current.argv.length > 0 ? (
          <Argv parts={current.argv.map((p, i) => ({ text: i === 0 ? p.text : ` ${p.text}`, ours: p.ours }))} />
        ) : (
          <p className={styles.foot}>Waiting for cide to compute it.</p>
        )}
        <p className={styles.foot}>
          A resume puts <code>resume</code> first and the thread last; a run adds its sandbox,
          model and prompt. Refusals are applied again at every spawn, so editing{' '}
          <code>workspace.json</code> by hand does not get past them.
        </p>
      </Group>
    </>
  )
}
