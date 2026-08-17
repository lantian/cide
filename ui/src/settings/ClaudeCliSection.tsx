/**
 * *How claude is launched*: the binary, extra arguments, extra environment.
 *
 * Presentational like `ProxySection` next door — it takes the stored value, a `patch` callback
 * and the Rust verdict, and reads nothing else — so the whole screen still renders from a
 * fixture.
 *
 * # Nothing is rejected on save, and that is the design
 *
 * `useSettings.ts` sends its patch fire-and-forget with `.catch(() => {})`. A `settings_set`
 * that returned `Err` today is **invisible**: the field snaps back to the stored value and
 * nothing anywhere says why. So a "reject the write" design is not deliverable without also
 * rewriting that callback, and even then it would be the wrong shape — a value you cannot see
 * is a value you cannot correct, which is the argument the proxy screen already makes about a
 * password.
 *
 * What happens instead: **everything is stored verbatim, and the readout says what the child
 * actually gets.** A refused token stays in its row, struck through, with the reason beside it,
 * and it is absent from the resolved argv below. The user can read their own mistake and edit
 * it. Rust enforces the same list at the spawn — see `cide_core::claude_cli` — so a hand-edited
 * `workspace.json` cannot get past it either.
 *
 * The one exception is the **binary**, which is a hard verdict and is drawn as one: a path that
 * cannot be executed gets a warning panel naming the value and where to fix it, because unlike
 * a stray argument it kills every pane at once. The verdict comes from Rust (`ClaudeCliSupport.
 * problem`), which is the only side that can `stat` a path.
 *
 * # Where the rules live
 *
 * `./claudeCli` — import-free, compiled and driven standalone by
 * `ui/scripts/check-claude-cli.mjs`, which also reads the tables out of
 * `crates/cide-core/src/claude_cli.rs` to prove the two agree. A rule inside a component is a
 * rule no check script can run, and this project has six shipped bugs that lived in exactly
 * that place.
 */
import { useState, type ReactNode } from 'react'
import type { ClaudeCli, ClaudeCliSupport, ClaudeEnvVar } from '@/ipc/client'
import { Group, Note, PathReadout, Row } from './controls'
import {
  judgeArgs,
  judgeEnv,
  reasonFor,
  refusalSummary,
  resolvedArgvParts,
  type Fate,
} from './claudeCli'
import styles from './ClaudeCliSection.module.css'

export interface ClaudeCliSectionProps {
  cli: ClaudeCli
  onChange: (next: ClaudeCli) => void
  /** Rust's verdict on the binary. `null` before the first probe answers. */
  support: ClaudeCliSupport | null
}

export function ClaudeCliSection({ cli, onChange, support }: ClaudeCliSectionProps) {
  const args = judgeArgs(cli)
  const env = judgeEnv(cli)
  const summary = refusalSummary(cli)

  // Only when the probe is about the value in the field. A verdict in flight is about the
  // *previous* binary, and drawing it beside the new one is how a screen tells a user their
  // corrected path is still broken.
  const current = support !== null && support.binary === cli.binary.trim() ? support : null

  return (
    <>
      <Group title="Binary">
        <TextField
          label="Program"
          hint="What every Claude pane and the one-shots behind Generate commit message run. “claude” — the default — is passed through as a bare name and resolved by your PATH at each spawn, which matters: the CLI updates itself, and resolving it once at launch would pin panes to a version that no longer exists. An absolute path pins a version deliberately, and a wrapper (mise, asdf, a shim) works too."
          value={cli.binary}
          placeholder="claude"
          onCommit={(binary) => onChange({ ...cli, binary })}
        />
        {current?.problem != null && (
          <Note title="That binary cannot be run" tone="warn">
            {current.problem} Until it is corrected, every Claude pane fails to start — the
            message above is what each one prints into its own transcript.
          </Note>
        )}
        {current?.problem == null && current?.resolved != null && (
          <Row
            label="Resolves to"
            hint={
              current.version != null
                ? `${current.version}. cide spawns the name above, not this path — the resolution happens afresh every time, so a self-updating CLI is picked up without relaunching.`
                : 'Nothing parseable came back from --version. That is ordinary for a wrapper script and is not an error; cide only warns when it can read a version and that version is outside the range it was checked against.'
            }
            control={<span />}
          />
        )}
        {current?.problem == null && current?.resolved != null && (
          <PathReadout path={current.resolved} />
        )}
      </Group>

      <Group title="Arguments">
        <TokenList
          rows={args}
          reasons={support?.argReasons ?? []}
          addLabel="Add argument"
          placeholder="--model"
          onChange={(next) => onChange({ ...cli, args: next })}
          values={cli.args}
        />
        <Note title="One token per row">
          <code>--append-system-prompt</code> and <code>be terse</code> are two rows, not one:
          cide passes these to <code>execvp</code> as they stand and never through a shell, so
          there is no quoting to get wrong and no quoting parser to be surprised by. They go{' '}
          <em>before</em> everything cide adds, which is what stops a variadic flag like{' '}
          <code>--add-dir</code> from swallowing cide’s own session id.
          <br />
          They reach <strong>panes only</strong>. The one-shots behind Generate commit message
          and Explain selection keep cide’s own argv — <code>-p --output-format json</code> and
          the rest — because a <code>--model</code> or <code>--tools</code> folded into that
          vector does not customise anything, it breaks the parse of a reply that never arrives.
        </Note>
      </Group>

      <Group title="Environment">
        <PairList
          rows={env}
          reasons={support?.envReasons ?? []}
          values={cli.env}
          onChange={(next) => onChange({ ...cli, env: next })}
        />
        <Note title="claude panes and the one-shot lane, and nothing else">
          Not shell panes — deliberately, and it is worth stating because the switches above{' '}
          <em>do</em> reach them. <code>CLAUDE_CODE_*</code> means nothing to <code>bash</code>,
          so a shell that inherits those simply ignores them; a <code>NODE_OPTIONS</code> or a{' '}
          <code>PATH</code> from this list is not inert to anything, and a field labelled “the
          environment claude is spawned with” must not quietly become your shell’s.
          <br />
          Values are stored in plain text in <code>workspace.json</code>, which is written{' '}
          <code>0600</code>. They are redacted from every log line and from any debug print of
          the settings, and not from the field above, because you have to be able to correct
          them. cide has no keyring yet; a proxy password is stored the same way and for the
          same reason.
        </Note>
      </Group>

      <Group title="What a pane is actually spawned with">
        <Argv parts={resolvedArgvParts(cli, 'fresh')} />
        {summary !== null && (
          <p className={styles.foot}>
            {summary} and struck out above. Refusals are applied again when a pane starts, so
            editing <code>workspace.json</code> by hand does not get past them.
          </p>
        )}
      </Group>

      <Note title="Applied at spawn">
        A pane already running keeps the binary, arguments and environment it was spawned with.
        Close and reopen it, or relaunch cide, for a change here to reach it.
      </Note>
    </>
  )
}

/* -------------------------------------------------------------------------- the pieces */

/**
 * The resolved command line.
 *
 * cide's own tokens are drawn dim so the two halves are distinguishable at a glance — the
 * question this readout answers is "which of these did I write", and a uniform monospace line
 * answers it badly. The split is positional rather than semantic: everything up to the first
 * token cide adds is the user's, which is exactly the ordering guarantee the argv is built on.
 */
function Argv({ parts }: { parts: { text: string; ours: boolean }[] }) {
  return (
    <pre className={styles.argv}>
      {parts.map((part, i) => (
        <span key={`${i}-${part.text}`} className={part.ours ? styles.argvOurs : undefined}>
          {part.text}
        </span>
      ))}
    </pre>
  )
}

/** The class for a row's fate. `accepted` gets none, which is the common case. */
function fateClass(fate: Fate): string | undefined {
  if (fate === 'refused') return styles.refused
  if (fate === 'warned') return styles.warned
  return undefined
}

/**
 * A list of single-token rows with add and remove.
 *
 * `onCommit`-on-blur rather than on every keystroke, like `ProxySection`'s field: each write
 * is an IPC round trip, a `workspace.json` rewrite and a broadcast to every open window.
 */
function TokenList({
  rows,
  reasons,
  values,
  addLabel,
  placeholder,
  onChange,
}: {
  rows: { index: number; text: string; fate: Fate }[]
  reasons: readonly { name: string; reason: string }[]
  values: string[]
  addLabel: string
  placeholder: string
  onChange: (next: string[]) => void
}) {
  return (
    <div className={styles.list}>
      {values.map((value, index) => (
        /*
         * Keyed by CONTENT as well as position, because these inputs are uncontrolled.
         *
         * `defaultValue` is read once, at mount. Keyed by index alone, removing a row does not
         * remount anything — React reuses the same DOM node for the shifted-up value and the
         * input goes on displaying the deleted text. The next blur on that field then compares
         * the stale DOM value against the new prop, finds them different, and commits the
         * deletion back: the removed entry returns and the one that was there is gone.
         *
         * For the environment editor that is worse than surprising. Removing an
         * `ANTHROPIC_API_KEY=sk-…` row leaves its VALUE in the reused value input, so a later
         * blur grafts a deleted secret onto whichever variable moved up into that slot — a
         * credential attached to a name the user never paired it with.
         *
         * Content in the key means the node remounts exactly when the underlying row changes and
         * never while the user is typing (the prop does not move for an uncontrolled input), so
         * this costs nothing per keystroke.
         */
        <div key={`${index}:${value}`} className={styles.listRow}>
          <input
            className={`${styles.input} ${fateClass(rows[index]?.fate ?? 'accepted') ?? ''}`}
            type="text"
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            aria-label={`${addLabel} ${index + 1}`}
            placeholder={placeholder}
            defaultValue={value}
            onBlur={(e) => {
              const next = [...values]
              next[index] = e.target.value
              if (next[index] !== value) onChange(next)
            }}
          />
          <Remove
            label={`Remove ${addLabel.toLowerCase()} ${index + 1}`}
            onClick={() => onChange(values.filter((_, i) => i !== index))}
          />
          {/* The sentence Rust wrote for this rule. Without it a struck-out token is a refusal
              with no reason, which is the state a settings screen exists to avoid. */}
          <Why text={reasonFor(value, reasons)} />
        </div>
      ))}
      <Add label={addLabel} onClick={() => onChange([...values, ''])} />
    </div>
  )
}

/** The same, for `NAME` / `value` pairs. */
function PairList({
  rows,
  reasons,
  values,
  onChange,
}: {
  rows: { index: number; text: string; fate: Fate }[]
  reasons: readonly { name: string; reason: string }[]
  values: ClaudeEnvVar[]
  onChange: (next: ClaudeEnvVar[]) => void
}) {
  // Judged rows skip blanks, so a row's fate is found by index rather than by position.
  const fateOf = (index: number): Fate => rows.find((row) => row.index === index)?.fate ?? 'accepted'

  return (
    <div className={styles.list}>
      {values.map((entry, index) => (
        /*
         * Keyed by CONTENT as well as position, because these inputs are uncontrolled.
         *
         * `defaultValue` is read once, at mount. Keyed by index alone, removing a row does not
         * remount anything — React reuses the same DOM node for the shifted-up value and the
         * input goes on displaying the deleted text. The next blur on that field then compares
         * the stale DOM value against the new prop, finds them different, and commits the
         * deletion back: the removed entry returns and the one that was there is gone.
         *
         * For the environment editor that is worse than surprising. Removing an
         * `ANTHROPIC_API_KEY=sk-…` row leaves its VALUE in the reused value input, so a later
         * blur grafts a deleted secret onto whichever variable moved up into that slot — a
         * credential attached to a name the user never paired it with.
         *
         * Content in the key means the node remounts exactly when the underlying row changes and
         * never while the user is typing (the prop does not move for an uncontrolled input), so
         * this costs nothing per keystroke.
         */
        <div key={`${index}:${entry.name}=${entry.value}`} className={styles.listRow}>
          <input
            className={`${styles.input} ${styles.name} ${fateClass(fateOf(index)) ?? ''}`}
            type="text"
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            aria-label={`Variable ${index + 1} name`}
            placeholder="NAME"
            defaultValue={entry.name}
            onBlur={(e) => {
              if (e.target.value === entry.name) return
              const next = [...values]
              next[index] = { ...entry, name: e.target.value }
              onChange(next)
            }}
          />
          <input
            className={styles.input}
            type="text"
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            aria-label={`Variable ${index + 1} value`}
            placeholder="value"
            defaultValue={entry.value}
            onBlur={(e) => {
              if (e.target.value === entry.value) return
              const next = [...values]
              next[index] = { ...entry, value: e.target.value }
              onChange(next)
            }}
          />
          <Remove
            label={`Remove variable ${index + 1}`}
            onClick={() => onChange(values.filter((_, i) => i !== index))}
          />
          {/* Matched on the NAME, not the value — a secret must never be the lookup key, and the
              rules are about which variable this is. */}
          <Why text={reasonFor(entry.name, reasons)} />
        </div>
      ))}
      <Add label="Add variable" onClick={() => onChange([...values, { name: '', value: '' }])} />
    </div>
  )
}

/**
 * The sentence beside a struck-out or flagged row.
 *
 * Renders nothing at all when there is none — an accepted row, or a build whose
 * `claude_cli_support` did not answer. An empty element would leave a gap under every ordinary
 * row and make the section look broken in its normal state.
 *
 * `role="note"` rather than `alert`: the row is already marked visually and by its input's
 * class, and the user is reading a settings screen rather than being interrupted by it.
 */
function Why({ text }: { text: string | null }) {
  if (text === null) return null
  return (
    <p className={styles.why} role="note">
      {text}
    </p>
  )
}

function Add({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button type="button" className={styles.add} onClick={onClick}>
      + {label}
    </button>
  )
}

function Remove({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button type="button" className={styles.remove} aria-label={label} onClick={onClick}>
      ×
    </button>
  )
}

/**
 * A stacked label + input + hint, the shape `ProxySection` uses for a URL.
 *
 * A local copy rather than a shared export, matching what is already there: the two differ in
 * their commit rules — this one has no draft state because a program name is short enough that
 * Escape-to-abandon buys nothing, where a half-typed proxy password is worth protecting.
 */
function TextField({
  label,
  hint,
  value,
  placeholder,
  onCommit,
}: {
  label: string
  hint: ReactNode
  value: string
  placeholder: string
  onCommit: (next: string) => void
}) {
  const [draft, setDraft] = useState<string | null>(null)
  const commit = (text: string) => {
    setDraft(null)
    // Guarded, so tabbing through an untouched field is not a write and a broadcast.
    if (text.trim() !== value) onCommit(text.trim())
  }
  return (
    <label className={styles.field}>
      <span className={styles.fieldLabel}>{label}</span>
      <input
        className={styles.input}
        type="text"
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        placeholder={placeholder}
        value={draft ?? value}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={(e) => commit(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') commit(e.currentTarget.value)
          else if (e.key === 'Escape') setDraft(null)
        }}
      />
      <span className={styles.fieldHint}>{hint}</span>
    </label>
  )
}
