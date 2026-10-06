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
import type { CodexCli, CodexCliSupport, CodexInjections, CodexPermissionMode, ClaudeEnvVar } from '@/ipc/client'
import { IconButton } from '@/kit/components/Button'
import { InfoPara, InfoTip } from '@/kit/components/InfoTip'
import { Group, PathReadout, Row, Select, Toggle, resetTo, useSettingsDefaults } from './controls'
import { Add, Argv, ProgramInput, Remove, Why } from './ClaudeCliSection'

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
 * without connecting it to this screen. The headline is on the row (`short`), the whole of it
 * behind the name's (i) (M133). The flag used to be the label, in parentheses after the name,
 * which made seven two-line labels; it is its own mono cell now.
 */
const INJECTIONS: readonly {
  key: keyof CodexInjections
  /** The row's name. */
  title: string
  /** What it writes on the command line, drawn in mono beside the name. */
  flag: string
  /** The headline of the cost, one line on the row (M133); `cost` is behind the (i). */
  short: string
  cost: string
}[] = [
  {
    key: 'hooks',
    title: 'Hooks',
    flag: '-c hooks.* --dangerously-bypass-hook-trust',
    short: 'Off: the pane never shows busy or awaiting, and Resume cannot find it.',
    cost: 'Off: no state at all. The pane never shows busy or awaiting, its thread is never learned — so Resume and a restore after restart cannot find the conversation — and a line cide types waits for a readiness it never hears.',
  },
  {
    key: 'mcpConfig',
    title: 'cide’s MCP server',
    flag: '-c mcp_servers.cide.*',
    short: 'Off: no task tracker, and on a console no subagents.',
    cost: 'Off: no mcp__cide__* tools. No task tracker, and on a console no subagents.',
  },
  {
    key: 'developerInstructions',
    title: 'Roster paragraph',
    flag: '-c developer_instructions',
    short: 'Off: the console is not told what cide’s tools are for.',
    cost: 'Off: the console is not told what cide’s tools are for, and a run is not given its brief.',
  },
  {
    key: 'resume',
    title: 'Resume',
    flag: 'codex resume <thread>',
    short: 'Off: a restored or resumed pane starts a new conversation.',
    cost: 'Off: a restored or resumed pane starts a new conversation.',
  },
  {
    key: 'fork',
    title: 'Fork',
    flag: 'codex fork <thread>',
    short: 'Off: Split → fork starts fresh instead of branching.',
    cost: 'Off: Split → fork starts a fresh conversation instead of branching this one.',
  },
  {
    // M108: for a wrapper that pins the sandbox itself and refuses cide's, which is what stopped
    // every MR review while the console the wrapper opens worked — a console under *ask* carries
    // no policy flag, a run always did.
    key: 'permissions',
    title: 'Run sandbox and approvals',
    flag: '-s -a --dangerously-bypass-approvals-and-sandbox',
    short: 'Off: your codex config or wrapper decides; a run may wait on an approval.',
    cost: 'Off: runs, reviews and tabs cide opens pass no sandbox or approval flag — your codex config, or the wrapper named above, decides. A role’s permission-mode and the project’s unattended default are no longer applied, so a run may wait on an approval nobody is watching.',
  },
  {
    key: 'gitPermissions',
    title: 'Git permissions',
    flag: '-c permissions.cide-git -c default_permissions',
    short: 'On: eligible consoles and worker worktrees can commit inside the sandbox.',
    cost: 'Applied on the next launch. Grants writes to this checkout’s Git metadata while keeping other protected paths and approval settings. Custom profiles, legacy sandbox settings, wrappers and read-only sessions keep their existing permissions. Off: Git writes may require the supported approval route.',
  },
  {
    key: 'reviewPermissions',
    title: 'MR-review permissions',
    flag: 'without prompts',
    short: 'Off: Codex uses the wrapper’s own approval policy.',
    cost: 'Off: Codex uses the wrapper’s own approval policy. On: approval requests go to Codex’s automatic reviewer, which may approve or deny them. The wrapper or Codex config controls the sandbox.',
  },
]

export function CodexCliSection({ cli, onChange, support }: CodexCliSectionProps) {
  // Only when the readout is about the value in the field — `ClaudeCliSection`'s rule.
  const current = support !== null && support.binary === cli.binary.trim() ? support : null
  const argNote = (index: number) => current?.argNotes.find((n) => n.index === index) ?? null
  const envNote = (index: number) => current?.envNotes.find((n) => n.index === index) ?? null
  const inject = cli.inject
  const def = useSettingsDefaults()?.codex.cli

  return (
    <>
      <Group title="Permissions">
        <Row
          label="Default permissions"
          hint="The permission mode Codex uses on its next launch."
          status={current?.permissionsNote ? { tone: 'info', text: current.permissionsNote } : undefined}
          info={<>
            <InfoPara>Applies to new, reopened, resumed and forked consoles, agent runs and automatic tabs. Running sessions keep their current mode; use /permissions to change it.</InfoPara>
            <InfoPara>Use Codex config preserves existing launch defaults. Explicit role and review policies take precedence, followed by permission options in Extra arguments. Agent run injection switches still apply.</InfoPara>
            <InfoPara>Approve for me keeps the workspace sandbox and sends eligible approvals to Codex’s automatic reviewer. Full access removes sandbox restrictions and approval prompts. Read-only allows inspection without workspace edits.</InfoPara>
            <InfoPara>With scoped Git writes enabled, /permissions shows cide-git as current for Ask for approval and Approve for me. The profile description identifies the selected approval mode.</InfoPara>
          </>}
          {...resetTo(cli.permissionMode, def?.permissionMode, (permissionMode) => onChange({ ...cli, permissionMode }))}
          control={<Select<CodexPermissionMode>
            label="Codex default permissions"
            value={cli.permissionMode}
            options={[
              { value: 'useConfig', label: 'Use Codex config' },
              { value: 'askForApproval', label: 'Ask for approval' },
              { value: 'approveForMe', label: 'Approve for me' },
              { value: 'fullAccess', label: 'Full access' },
              { value: 'readOnly', label: 'Read-only' },
              { value: 'customProfile', label: 'Custom profile' },
            ]}
            onChange={(permissionMode) => onChange({ ...cli, permissionMode })}
          />}
        />
        {cli.permissionMode === 'customProfile' && <Row
          label="Permission profile"
          hint="The name of an existing permissions profile in your Codex config."
          status={!cli.permissionProfile.trim() ? { tone: 'bad', text: 'Enter a profile name to use Custom profile.' } : undefined}
          {...resetTo(cli.permissionProfile, def?.permissionProfile, (permissionProfile) => onChange({ ...cli, permissionProfile }))}
          control={<ProgramInput
            label="Codex permission profile"
            value={cli.permissionProfile}
            placeholder="my-profile"
            onCommit={(permissionProfile) => onChange({ ...cli, permissionProfile: permissionProfile.trim() })}
          />}
        />}
      </Group>
      <Group title="Binary">
        {/* The hard verdict is the row's status, `ClaudeCliSection`'s shape (M133). */}
        <Row
          label="Program"
          hint="What codex consoles, codex runs and its one-shots run; found on your PATH."
          status={
            current?.problem != null
              ? { tone: 'bad', text: `${current.problem} Every codex console and run fails to start until it is corrected.` }
              : undefined
          }
          info={
            <>
              <InfoPara>
                What every codex console, every run of a role whose harness is codex, and the
                one-shots behind Generate commit message run when Harness is Codex.
              </InfoPara>
              <InfoPara>
                “codex” — the default — is resolved by your PATH at each spawn: the standalone
                installer updates codex in place, and resolving it once would pin a release it
                may since have removed. An absolute path or a wrapper script works too.
              </InfoPara>
            </>
          }
          {...resetTo(cli.binary, def?.binary, (binary) => onChange({ ...cli, binary }))}
          control={
            <ProgramInput
              label="Codex program"
              value={cli.binary}
              placeholder="codex"
              onCommit={(binary) => onChange({ ...cli, binary })}
            />
          }
        />
        {current?.problem == null && current?.resolved != null && (
          <Row
            label="Resolves to"
            hint={current.version ?? 'No version from --version — ordinary for a wrapper script.'}
            control={<span />}
          />
        )}
        {current?.problem == null && current?.resolved != null && (
          <PathReadout path={current.resolved} />
        )}
      </Group>

      <Group
        title="Arguments"
        info={
          <>
            One token per row: <code>-c</code> and <code>model="o3"</code> are two rows. They go
            right after the <code>resume</code>/<code>fork</code> subcommand and before everything
            cide adds. An override of a key cide writes itself — <code>hooks.*</code>,{' '}
            <code>mcp_servers.cide.*</code>, <code>developer_instructions</code> — is struck out
            while cide is still writing it.
          </>
        }
      >
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
      </Group>

      <Group
        title="Environment"
        info={
          <>
            codex children only: consoles, codex runs and the codex one-shot lane — never shell
            panes. Values are stored in plain text in <code>workspace.json</code> (written{' '}
            <code>0600</code>) and redacted from every log line. <code>CODEX_HOME</code> is
            refused: cide reads it from its own environment to find the conversation behind a
            pane, so export it before launching cide instead.
          </>
        }
      >
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
      </Group>

      <Group
        title="What cide adds to the command line"
        info={
          <>
            Always added, whatever is switched here: <code>-C &lt;directory&gt;</code>, and two
            overrides that keep a startup modal from eating the first key cide types:{' '}
            <code>check_for_update_on_startup=false</code> (the update chooser’s first row is
            “Update now”) and <code>notice.hide_rate_limit_model_nudge=true</code>.
          </>
        }
      >
        <div className={styles.injections}>
          {INJECTIONS.map((row) => {
            const on = inject?.[row.key] ?? true
            const original = def?.inject?.[row.key]
            return (
              <div key={row.key} className={styles.injection}>
                <div className={styles.injectionHead}>
                  <Toggle
                    label={row.title}
                    checked={on}
                    onChange={(next) => onChange({ ...cli, inject: { ...inject, [row.key]: next } })}
                  />
                  <span className={styles.injectionTitle}>
                    {row.title}
                    <InfoTip label={`About ${row.title}`}>{row.cost}</InfoTip>
                  </span>
                  <code className={styles.injectionCode}>{row.flag}</code>
                  {original !== undefined && original !== on && (
                    <IconButton
                      icon="undo-2"
                      label={`Reset ${row.title} to default`}
                      onClick={() => onChange({ ...cli, inject: { ...inject, [row.key]: original } })}
                    />
                  )}
                </div>
                <p className={styles.injectionShort}>{row.short}</p>
              </div>
            )
          })}
        </div>
      </Group>

      <Group
        title="What a codex console is actually spawned with"
        info={
          <>
            A resume puts <code>resume</code> first and the thread last; a run adds its sandbox,
            model and prompt. Refusals are applied again at every spawn, so editing{' '}
            <code>workspace.json</code> by hand does not get past them.
          </>
        }
      >
        {current?.gitPermissionsNote != null && <InfoPara>{current.gitPermissionsNote}</InfoPara>}
        {current !== null && current.argv.length > 0 ? (
          <Argv parts={current.argv.map((p, i) => ({ text: i === 0 ? p.text : ` ${p.text}`, ours: p.ours }))} />
        ) : (
          <p className={styles.foot}>Waiting for cide to compute it.</p>
        )}
      </Group>
    </>
  )
}
