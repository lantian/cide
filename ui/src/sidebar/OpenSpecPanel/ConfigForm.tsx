/**
 * `openspec/config.yaml` as a form, and the wizard that writes the first one. (M28)
 *
 * Pure views: no store, no IPC, no `useEffect` that fetches. Everything arrives as props and
 * every gesture leaves as a callback, which is what lets `ConfigSmokeEntry.tsx` render both of
 * them from a fixture and `check-openspec-config.mjs` assert on the markup. `ConfigDialog.tsx`
 * beside them is the host that owns the state and the calls — it was `settings/OpenSpecSection.tsx`
 * and moved, because cide's Settings is *global* and this file belongs to one repository; see
 * that file's header for the argument.
 *
 * # Why the form shows less than the file, and links to the file
 *
 * `openspec init` writes 922 bytes of which roughly eight hundred are **comments** — three
 * commented example blocks that are the only documentation `context`, `rules` and `operations`
 * have anywhere. A form is a better way to *set* those keys and a worse way to *learn* what they
 * are for, so the path is printed and openable at the top rather than tucked at the bottom: the
 * one thing this screen must not do is become the reason somebody never reads the file.
 *
 * # Why `context` gets a whole group and a paragraph, and the others get a line
 *
 * Because it is the field that pays. It is injected into every artifact-generation prompt, so it
 * is what an agent knows about this project's stack and conventions before it writes a proposal,
 * a design or a spec. `rules` and `operations` tune the output; `context` decides whether the
 * output is about this codebase at all. A screen that gave all three the same weight would be
 * describing the file's structure rather than what any of it is worth doing.
 *
 * # Why every list row is an `<input>` and not one textarea of lines
 *
 * A rule becomes a `- ` item in a YAML sequence, which is a *line*. One textarea split on
 * newlines is fewer elements and is the shape that lets somebody paste a paragraph in and get a
 * file the CLI cannot parse — the same trap `cmd::agents::one_line` guards at the PTY. Separate
 * inputs make the one-line-per-rule rule visible instead of enforced silently afterwards.
 */
import type { ReactNode } from 'react'
import styles from './ConfigForm.module.css'
import {
  artifactRows,
  guidanceIn,
  operationHint,
  operationLabel,
  operationRows,
  rulesIn,
  SCHEMA_FORK_HINT,
  onlyDefaultSchema,
  schemaOptions,
  withGuidance,
  withRules,
  wizardAction,
  wizardLabel,
  type ConfigDraft,
  type ConfigLike,
  type SchemaLike,
} from './configModel'

/* ------------------------------------------------------------------------------- the form */

export type SaveStatus = 'idle' | 'saving' | 'saved' | 'unchanged'

export interface ConfigFormProps {
  /** What the file states, as it was last read. The baseline every comparison is against. */
  config: ConfigLike
  /** What the CLI listed, or the single fallback row. Never empty. */
  schemas: readonly SchemaLike[]
  draft: ConfigDraft
  onDraft: (draft: ConfigDraft) => void
  /** Whether Save would write anything — `isDirty`, computed by the host from the same module. */
  dirty: boolean
  status: SaveStatus
  /** A rejection, already turned into a sentence by `errorText`. Never `String(e)`. */
  error: string | null
  /**
   * Why the `openspec` CLI could not be asked, or `null`.
   *
   * Drawn as a note and **not** as a reason to disable anything: the file is read and written by
   * cide's own scanner, so a project is perfectly configurable with no `openspec` on PATH. What
   * is degraded is the schema list, and that is what the note says.
   */
  cliNote: string | null
  onSave: () => void
  onRevert: () => void
  onOpenFile: () => void
}

export function ConfigForm({
  config,
  schemas,
  draft,
  onDraft,
  dirty,
  status,
  error,
  cliNote,
  onSave,
  onRevert,
  onOpenFile,
}: ConfigFormProps) {
  const options = schemaOptions(config, schemas)
  const chosen = options.find((option) => option.value === draft.schema) ?? null
  const busy = status === 'saving'

  return (
    <div className={styles.form} data-audit="specConfig">
      <div className={styles.fileRow} data-audit="specConfigFile">
        <div className={styles.fileText}>
          <div className={styles.fileLabel}>
            {config.exists ? 'This project’s OpenSpec configuration' : 'Not written yet'}
          </div>
          <div className={styles.path} data-audit="specConfigPath">
            {config.path}
          </div>
          <div className={styles.fileHint}>
            {config.exists
              ? 'The file carries commented examples this form does not show — they are the only documentation these keys have. Saving here keeps every comment, every blank line and the key order exactly as they are.'
              : 'There is no config.yaml yet. Saving here writes one; everything below reads as unset until then.'}
          </div>
        </div>
        <button
          type="button"
          className={styles.secondary}
          data-audit="specConfigOpenFile"
          onClick={onOpenFile}
        >
          Open the file
        </button>
      </div>

      {cliNote !== null && (
        <Note tone="warn" hook="specConfigCliNote" title="The openspec CLI could not be asked">
          {cliNote} The form still works — cide reads and writes this file itself — but the schema
          list below is a fallback rather than what is installed.
        </Note>
      )}

      <Group title="Workflow schema" hook="specConfigSchemaGroup">
        <p className={styles.groupHint}>
          Which workflow this project follows, and therefore which artifacts a change is made of.
          The names come from <code>openspec schemas</code>.
        </p>
        <select
          className={styles.select}
          data-audit="specConfigSchema"
          data-write="true"
          aria-label="Workflow schema"
          value={draft.schema}
          disabled={busy}
          onChange={(event) => onDraft({ ...draft, schema: event.target.value })}
        >
          {options.map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </select>
        {chosen !== null && chosen.hint !== '' && (
          <p
            className={chosen.unlisted ? `${styles.groupHint} ${styles.warnHint}` : styles.groupHint}
            data-audit="specConfigSchemaHint"
            data-unlisted={String(chosen.unlisted)}
          >
            {chosen.hint}
          </p>
        )}
        {/*
          * Drawn only when the list holds the one schema upstream ships.
          *
          * A picker with a single option reads as a broken control or as a hard-coded value, and
          * it is neither — but nothing on screen said so, and the first question it got was
          * "only one type of schema?". The sentence names the command that adds another rather
          * than offering a button, because the CLI's own help marks schema commands
          * experimental.
          *
          * Not when `cliNote` is set: there the single row is *cide's fallback* rather than the
          * CLI's answer, and the note above already says the list is degraded. Claiming this list
          * is what `openspec schemas` reports would be false in exactly the case where the CLI
          * could not be asked at all.
          */}
        {cliNote === null && onlyDefaultSchema(options) && (
          <p className={styles.groupHint} data-audit="specConfigSchemaOnlyOne">
            {SCHEMA_FORK_HINT}
          </p>
        )}
        {config.schema === null && (
          <p className={styles.groupHint} data-audit="specConfigSchemaUnstated">
            The file states no <code>schema:</code>, which means <code>{config.defaultSchema}</code>
            . Leaving this alone writes nothing; choosing another schema adds the line.
          </p>
        )}
      </Group>

      <Group title="Project context" hook="specConfigContextGroup">
        <p className={styles.groupHint}>
          <strong>This is the field that pays.</strong> Everything here is injected into every
          artifact-generation prompt — it is what an agent knows about this codebase before it
          writes a proposal, a design or a spec. Name the stack, the conventions somebody would
          otherwise have to guess, and the things this project deliberately does not do.
        </p>
        <textarea
          className={styles.textarea}
          data-audit="specConfigContext"
          data-write="true"
          aria-label="Project context"
          rows={10}
          spellCheck={false}
          value={draft.context}
          placeholder={CONTEXT_PLACEHOLDER}
          disabled={busy}
          onChange={(event) => onDraft({ ...draft, context: event.target.value })}
        />
        <p className={styles.groupHint}>
          Written as a YAML block scalar, so line breaks and indented lists survive. Clearing the
          box removes the key rather than writing an empty one.
        </p>
      </Group>

      <Group title="Artifact rules" hook="specConfigRulesGroup">
        <p className={styles.groupHint}>
          Extra instructions for one artifact, read whenever that artifact is generated. One line
          each. The artifacts are the ones the chosen schema declares — a schema can declare its
          own, so this list is not fixed.
        </p>
        {artifactRows(draft, schemas).map((row) => (
          <StringList
            key={row.artifact}
            hook="specConfigRule"
            group={row.artifact}
            title={row.artifact}
            hint={
              row.declared
                ? undefined
                : 'The chosen schema does not declare this artifact — the file states rules for it anyway. Clearing them removes the entry.'
            }
            addLabel="Add a rule"
            placeholder="e.g. Keep the proposal under one page."
            items={rulesIn(draft.rules, row.artifact)}
            disabled={busy}
            onChange={(rules) => onDraft({ ...draft, rules: withRules(draft.rules, row.artifact, rules) })}
          />
        ))}
      </Group>

      <Group title="Operation guidance" hook="specConfigOperationsGroup">
        <p className={styles.groupHint}>
          Instructions read while an operation runs, rather than while an artifact is written.
        </p>
        {operationRows(draft).map((operation) => (
          <StringList
            key={operation}
            hook="specConfigGuidance"
            group={operation}
            title={operationLabel(operation)}
            hint={operationHint(operation)}
            addLabel="Add guidance"
            placeholder="e.g. Run the full test suite before ticking the last task."
            items={guidanceIn(draft.operations, operation)}
            disabled={busy}
            onChange={(guidance) =>
              onDraft({ ...draft, operations: withGuidance(draft.operations, operation, guidance) })
            }
          />
        ))}
      </Group>

      {error !== null && (
        <Note tone="warn" hook="specConfigError" title="That save did not happen">
          {error}
        </Note>
      )}

      <div className={styles.footer} data-audit="specConfigFooter">
        <button
          type="button"
          className={styles.primary}
          data-audit="specConfigSave"
          data-write="true"
          disabled={busy || !dirty}
          onClick={onSave}
        >
          {busy ? 'Saving…' : 'Save'}
        </button>
        <button
          type="button"
          className={styles.secondary}
          data-audit="specConfigRevert"
          disabled={busy || !dirty}
          onClick={onRevert}
        >
          Discard edits
        </button>
        <span className={styles.status} data-audit="specConfigStatus" data-status={status}>
          {statusLine(status, dirty)}
        </span>
      </div>
    </div>
  )
}

/**
 * The one line under the buttons.
 *
 * `unchanged` exists because it is the outcome nothing else on the screen can express. A Save
 * that found every value already spelled that way writes no file and does not move the mtime —
 * which is a *success*, and a screen that said "Saved" for it would be claiming a commit that a
 * `git status` will not show. Saying so out loud is also the only feedback the round-trip
 * guarantee ever gets in front of a user.
 */
function statusLine(status: SaveStatus, dirty: boolean): string {
  switch (status) {
    case 'saving':
      return 'Writing openspec/config.yaml…'
    case 'saved':
      return 'Saved. Comments, blank lines and key order are untouched.'
    case 'unchanged':
      return 'Nothing to write — the file already says exactly this, so it was not touched at all.'
    case 'idle':
      return dirty ? 'Unsaved edits.' : 'Up to date with the file.'
    default:
      // A `SaveStatus` this build does not know. A status line that came back `undefined` would
      // render as an empty span, which reads as "no message" rather than as a bug.
      return ''
  }
}

const CONTEXT_PLACEHOLDER = `Tech stack: Rust + Tauri 2 backend, React 19 + Vite frontend.
Linux-first; the only platform this has ever run on.
Every wire type is generated from Rust — never hand-edit ui/src/ipc/generated.ts.`

/* ----------------------------------------------------------------------------- the wizard */

export interface ConfigWizardProps {
  /** The directory `openspec init` would create, from the board's `absent` arm. */
  path: string
  /** Rust's own sentence about what setting up does. Drawn, never summarised. */
  hint: string
  context: string
  onContext: (text: string) => void
  busy: boolean
  error: string | null
  onSetUp: () => void
}

/**
 * Set OpenSpec up, having asked for the project context **first**.
 *
 * # Why the box comes before the button, in the markup and not only visually
 *
 * `context:` is what every later agent prompt reads, and it is prose somebody has to sit down and
 * write. The file `openspec init` leaves behind mentions it only as a commented-out example, so
 * the realistic outcome of "set it up now, write the context later" is a project whose every
 * agent prompt is context-free for ever. The one moment a person is already thinking about what
 * this project *is* is the moment they are setting it up — so the question is asked at the only
 * time it will be answered, and it is asked *above* the button rather than beside it, because a
 * field read after the click is a field that was skipped.
 *
 * `check-openspec-config.mjs` asserts the document order, which is the half a screenshot cannot
 * defend against the next tidy-up.
 *
 * # Why it is skippable, and why the button says so
 *
 * A required field here would be a wizard that refuses to set a project up until somebody writes
 * an essay, and the essay would be written badly to get past it. So the box is optional and the
 * *button changes its words* — `wizardLabel` — so skipping is a thing the user chose and can see
 * they chose, rather than a silent default. A second "Skip" button would put the two outcomes at
 * war over which is the primary action.
 */
export function ConfigWizard({
  path,
  hint,
  context,
  onContext,
  busy,
  error,
  onSetUp,
}: ConfigWizardProps) {
  const action = wizardAction(context, busy)

  return (
    <div className={styles.form} data-audit="specConfigWizard">
      <Group title="Set up OpenSpec in this project" hook="specConfigWizardGroup">
        <p className={styles.groupHint} data-audit="specConfigWizardHint">
          {hint}
        </p>
        <div className={styles.path} data-audit="specConfigWizardPath">
          {path}
        </div>
      </Group>

      <Group title="First: what should every agent know about this project?" hook="specConfigWizardContextGroup">
        <p className={styles.groupHint}>
          This becomes <code>context:</code> in <code>openspec/config.yaml</code>, and it is
          injected into <strong>every</strong> artifact-generation prompt from here on — the stack,
          the conventions, the things this project deliberately does not do. It is optional and you
          can edit it here afterwards, but this is the moment you are already thinking about it.
        </p>
        <textarea
          className={styles.textarea}
          data-audit="specConfigWizardContext"
          data-write="true"
          aria-label="Project context"
          rows={10}
          spellCheck={false}
          value={context}
          placeholder={CONTEXT_PLACEHOLDER}
          disabled={busy}
          onChange={(event) => onContext(event.target.value)}
        />
      </Group>

      {error !== null && (
        <Note tone="warn" hook="specConfigWizardError" title="Setting up did not happen">
          {error}
        </Note>
      )}

      <div className={styles.footer} data-audit="specConfigWizardFooter">
        <button
          type="button"
          className={styles.primary}
          data-audit="specConfigSetUp"
          data-write="true"
          data-action={action}
          disabled={busy}
          onClick={onSetUp}
        >
          {wizardLabel(action)}
        </button>
        <span className={styles.status} data-audit="specConfigWizardStatus">
          {busy
            ? 'Running openspec init…'
            : 'Creates openspec/, installs OpenSpec’s workflow commands into this project’s Claude Code, and writes the context above.'}
        </span>
      </div>
    </div>
  )
}

/* --------------------------------------------------------------------------- the furniture */

/**
 * A list of one-line strings with add and remove on each row.
 *
 * # Why the row key is the index
 *
 * The items are strings and two rows can legitimately hold the same text, so there is no stable
 * identity to key on. An index key costs a re-render of the rows below a removal, which is
 * exactly what is wanted here — the row that took position *n* really is a different value, and
 * a keyed-by-content list would carry the removed row's focus onto the wrong input.
 */
function StringList({
  hook,
  group,
  title,
  hint,
  addLabel,
  placeholder,
  items,
  disabled,
  onChange,
}: {
  hook: string
  group: string
  title: string
  hint?: string | undefined
  addLabel: string
  placeholder: string
  items: readonly string[]
  disabled: boolean
  onChange: (items: string[]) => void
}) {
  return (
    <div className={styles.list} data-audit={`${hook}List`} data-group={group}>
      <div className={styles.listHead}>
        <span className={styles.listTitle}>{title}</span>
        <span className={styles.listCount}>{items.length === 0 ? 'none' : `${items.length}`}</span>
      </div>
      {hint !== undefined && <p className={styles.listHint}>{hint}</p>}
      {items.map((item, index) => (
        <div className={styles.listRow} key={index}>
          <input
            type="text"
            className={styles.text}
            data-audit={`${hook}Item`}
            data-group={group}
            data-index={String(index)}
            data-write="true"
            aria-label={`${title} ${index + 1}`}
            value={item}
            placeholder={placeholder}
            disabled={disabled}
            onChange={(event) =>
              onChange(items.map((old, at) => (at === index ? event.target.value : old)))
            }
          />
          <button
            type="button"
            className={styles.remove}
            data-audit={`${hook}Remove`}
            data-group={group}
            data-index={String(index)}
            data-write="true"
            aria-label={`Remove ${title} ${index + 1}`}
            disabled={disabled}
            onClick={() => onChange(items.filter((_, at) => at !== index))}
          >
            Remove
          </button>
        </div>
      ))}
      <button
        type="button"
        className={styles.add}
        data-audit={`${hook}Add`}
        data-group={group}
        data-write="true"
        disabled={disabled}
        onClick={() => onChange([...items, ''])}
      >
        {addLabel}
      </button>
    </div>
  )
}

/** A titled block. Local rather than `settings/controls`'s, because these carry audit hooks. */
function Group({ title, hook, children }: { title: string; hook: string; children: ReactNode }) {
  return (
    <section className={styles.group} data-audit={hook}>
      <h3 className={styles.groupTitle}>{title}</h3>
      {children}
    </section>
  )
}

/** A block of prose that explains something the controls cannot. */
function Note({
  title,
  tone,
  hook,
  children,
}: {
  title: string
  tone: 'info' | 'warn'
  hook: string
  children: ReactNode
}) {
  return (
    <div
      className={tone === 'warn' ? `${styles.note} ${styles.noteWarn}` : styles.note}
      data-audit={hook}
      data-tone={tone}
    >
      <div className={styles.noteTitle}>{title}</div>
      <div className={styles.noteBody}>{children}</div>
    </div>
  )
}
