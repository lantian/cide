/**
 * The pure core of the OpenSpec configuration form — `openspec/config.yaml`, as a draft. (M28)
 *
 * `model.ts` next door is the panel's core; this is the *config* surface's, and it is a separate
 * module for the reason that one is import-free: so `check-openspec-config.mjs` can compile it
 * with the TypeScript in `node_modules` and drive it under node with no bundler, no React and no
 * Tauri. **Keep it import-free.** The wire shapes below are declared structurally rather than
 * imported from `../../ipc/generated`, which is the same trade `model.ts` makes and costs the
 * same thing: a field rename in Rust is caught by `tsc` at the *call sites* (which do import the
 * generated types) rather than here.
 *
 * # The three failures this module exists to make unrepresentable
 *
 * **A Save that rewrites a committed file for nothing.** `cide_spec::config` promises that
 * writing a value back unchanged changes no byte and does not move the mtime, and it keeps that
 * promise by comparing each value against what the file states. A form that sent "everything, on
 * every Save" would rely entirely on that comparison — and the comparison is against the value
 * the *reader* produces, which is not the value a textarea produces. A textarea ends in a
 * newline; the reader's block scalar does not. So [`normaliseBlock`] and [`normaliseLine`] mirror
 * what Rust's reader and writer do, and [`editsFor`] emits an edit only where the draft really
 * differs. That is also what makes the Save button honest: enabled means something will change.
 *
 * **A schema line added to a file that never asked for one.** `schema:` unstated *means*
 * `spec-driven`, and the wire deliberately keeps `null` rather than filling the default in — see
 * `SpecConfig::schema`. If the form treated "unstated" as "stated `spec-driven`", the first Save
 * on any project would add a line to a committed file that nobody typed. [`editsFor`] has the
 * one rule that prevents it, and the check states it.
 *
 * **A select whose value is not one of its options.** `spec_schemas` can only offer what the CLI
 * lists, and it degrades to a single row when the CLI cannot be asked at all — which is a
 * supported state, because the file is read and written by cide's own scanner and the form works
 * with no `openspec` on PATH. A `<select>` whose `value` matches no `<option>` renders as *some
 * other option*, silently, and the next Save writes that other schema over the user's. So
 * [`schemaOptions`] always contains the schema the file states, marked when it is one the CLI did
 * not list.
 *
 * # Why rules are a list of pairs and not a `Record`
 *
 * `check-problems.mjs`'s `ROGUE` lesson, and `check-openspec.mjs` restates it: a `Record` lookup
 * misses into `undefined`, and a key like `constructor` returns `Object.prototype.constructor` —
 * a function, which React refuses as a child. An artifact id here is a **free string** read out
 * of somebody's YAML (a schema declares its own artifacts, so cide may not assume the four it
 * knows), which means a prototype key is a value the file can genuinely contain. Ordered pairs
 * also keep document order, which is what the file says and what the writer splices back.
 */

/* ------------------------------------------------------------------ the shapes, structurally */

/** One artifact's rules, as `SpecArtifactRules`. */
export interface ArtifactRulesLike {
  artifact: string
  rules: string[]
}

/** One operation's guidance, as `SpecOperationGuidance`. */
export interface OperationGuidanceLike {
  operation: string
  guidance: string[]
}

/** `SpecConfig`, structurally. */
export interface ConfigLike {
  schema: string | null
  defaultSchema: string
  context: string | null
  rules: ArtifactRulesLike[]
  operations: OperationGuidanceLike[]
  path: string
  exists: boolean
}

/** `SpecSchema`, structurally. */
export interface SchemaLike {
  name: string
  description: string | null
  artifacts: string[]
  source: string | null
}

/** `SpecConfigEdit`, structurally. */
export type ConfigEditLike =
  | { kind: 'schema'; schema: string }
  | { kind: 'context'; context: string | null }
  | { kind: 'rules'; artifact: string; rules: string[] }
  | { kind: 'operationGuidance'; operation: string; guidance: string[] }

/**
 * The form's working copy.
 *
 * Plain data with no `null` in it: a form field is a string, and the tri-state the wire carries
 * ("no key" vs "an empty value") is reconstructed by [`editsFor`] from the comparison rather than
 * carried through every keystroke. A `context: string | null` on the draft would mean every
 * `onChange` had to decide whether an empty box is a removal, and it would decide on every
 * keystroke — including the one in the middle of clearing a field before retyping it.
 */
export interface ConfigDraft {
  schema: string
  context: string
  rules: ArtifactRulesLike[]
  operations: OperationGuidanceLike[]
}

/* ------------------------------------------------------------------------- the vocabularies */

/**
 * The operations `operations:` gives guidance for.
 *
 * Frozen here and pinned against `cide_spec::config::GUIDED_OPERATIONS` by
 * `check-openspec-config.mjs`. It has to exist in TypeScript because the form draws a section for
 * each **whether or not the file states one** — that is the whole point of a form over a YAML
 * file whose keys are commented out — so it cannot be derived from what was read.
 */
export const GUIDED_OPERATIONS: readonly string[] = ['apply', 'archive']

/** What to call one, with a fallback for an operation this build has never heard of. */
export function operationLabel(operation: string): string {
  switch (operation) {
    case 'apply':
      return 'Apply'
    case 'archive':
      return 'Archive'
    default:
      // Never `undefined`: a lookup that misses must still yield a heading, or the section
      // renders with no title and the guidance under it belongs to nothing.
      return operation === '' ? 'Operation' : operation
  }
}

/** One line about when that guidance is read. */
export function operationHint(operation: string): string {
  switch (operation) {
    case 'apply':
      return 'Read when an agent implements a change — while it is working through tasks.md.'
    case 'archive':
      return 'Read when a finished change is folded back into openspec/specs/.'
    default:
      return 'Read when this operation runs.'
  }
}

/* ---------------------------------------------------------------------------- normalisation */

/**
 * One line, with its whitespace collapsed — what `cmd::spec`'s `tidy_list` does in Rust.
 *
 * A rule and a piece of guidance become `- ` items in a YAML sequence, which is a **line**. A
 * value with a newline in it would be written as several items, one of which is the CLI's next
 * parse error. Mirrored here rather than left to Rust because the dirty check has to agree with
 * the writer: if the two normalise differently, a field the user has not touched compares unequal
 * for ever and the Save button never goes dark.
 */
export function normaliseLine(text: string): string {
  return text.split(/\s+/).filter(Boolean).join(' ')
}

/**
 * A block of prose, normalised the way the reader's block scalar reads it.
 *
 * `cide_spec::config`'s `read_block` maps a whitespace-only line to an empty one and drops
 * trailing empties — clip chomping, which is what `|` means and what `openspec init`'s own
 * example writes. A textarea produces neither, so without this every Save would compare unequal,
 * rewrite the file, and turn up in `git status` having changed nothing anybody typed.
 *
 * CRs are dropped: a textarea in a webview can produce `\r\n`, and a `\r` spliced into a YAML
 * block scalar is a byte in the value rather than a line ending.
 */
export function normaliseBlock(text: string): string {
  const lines = text
    .replace(/\r\n?/g, '\n')
    .split('\n')
    .map((line) => (line.trim() === '' ? '' : line))
  while (lines.length > 0 && lines[lines.length - 1] === '') lines.pop()
  return lines.join('\n')
}

/** A list of one-line strings with the blanks gone — `tidy_list`, exactly. */
export function normaliseList(items: readonly string[]): string[] {
  return items.map(normaliseLine).filter((item) => item !== '')
}

/* -------------------------------------------------------------------------------- lookups */

/** One artifact's rules, or an empty list. Never `undefined`, never a prototype member. */
export function rulesIn(entries: readonly ArtifactRulesLike[], artifact: string): string[] {
  for (const entry of entries) if (entry.artifact === artifact) return entry.rules
  return []
}

/** One operation's guidance, or an empty list. */
export function guidanceIn(
  entries: readonly OperationGuidanceLike[],
  operation: string,
): string[] {
  for (const entry of entries) if (entry.operation === operation) return entry.guidance
  return []
}

/**
 * The same list with one artifact's rules replaced, keeping document order.
 *
 * An artifact the list does not hold is **appended**, not inserted at the front: the writer
 * splices a new key in at the end of the mapping, so the form and the file agree about where a
 * newly-configured artifact goes.
 */
export function withRules(
  entries: readonly ArtifactRulesLike[],
  artifact: string,
  rules: string[],
): ArtifactRulesLike[] {
  const next = entries.map((entry) => (entry.artifact === artifact ? { artifact, rules } : entry))
  if (!entries.some((entry) => entry.artifact === artifact)) next.push({ artifact, rules })
  return next
}

/** The same, for one operation's guidance. */
export function withGuidance(
  entries: readonly OperationGuidanceLike[],
  operation: string,
  guidance: string[],
): OperationGuidanceLike[] {
  const next = entries.map((entry) =>
    entry.operation === operation ? { operation, guidance } : entry,
  )
  if (!entries.some((entry) => entry.operation === operation)) next.push({ operation, guidance })
  return next
}

/* ---------------------------------------------------------------------------- the schema */

/** The schema this project is on, with an unstated key resolved to the CLI's default. */
export function resolvedSchema(config: ConfigLike): string {
  const stated = config.schema === null ? '' : config.schema.trim()
  return stated === '' ? config.defaultSchema : stated
}

/** One row of the schema select. */
export interface SchemaOption {
  value: string
  label: string
  /** The CLI's own description, or the sentence explaining why this row is here at all. */
  hint: string
  /** The artifacts this schema declares, in workflow order. */
  artifacts: string[]
  /** True when the CLI did not list this schema and it is only here because the file states it. */
  unlisted: boolean
}

/**
 * The rows the schema select offers, guaranteed to contain the value it is showing.
 *
 * The guarantee is the point. `spec_schemas` degrades to a single row whenever the CLI cannot be
 * asked — no binary, a version with no `schemas --json`, a document this build cannot read — and
 * the form is still usable then, because the file is read and written by cide's own scanner. A
 * `<select>` whose `value` is not among its options does not fail: it displays the first option,
 * so the screen would quietly claim the project is on `spec-driven` and the next Save would make
 * that true. The row is appended rather than prepended so the installed schemas read in the CLI's
 * own order, and it is marked so the form can say *why* it is unlike the others.
 */
export function schemaOptions(config: ConfigLike, schemas: readonly SchemaLike[]): SchemaOption[] {
  const options: SchemaOption[] = schemas
    .filter((schema) => schema.name.trim() !== '')
    .map((schema) => ({
      value: schema.name,
      label: schema.name,
      hint: schema.description ?? '',
      artifacts: schema.artifacts,
      unlisted: false,
    }))
  const current = resolvedSchema(config)
  if (!options.some((option) => option.value === current)) {
    options.push({
      value: current,
      label: current,
      hint: 'This project states this schema and the openspec CLI did not list it — either it is installed somewhere cide could not ask, or the name is a typo. Leaving it alone is safe; changing it rewrites the file.',
      artifacts: [],
      unlisted: true,
    })
  }
  return options
}

/**
 * Is this list *only* the one schema every installation has? (M28)
 *
 * Upstream ships exactly one — `spec-driven`, `source: "package"` — so a picker with a single
 * entry is the honest answer for almost every project, and it reads as a broken control or as a
 * hard-coded value. It is neither: cide asks `openspec schemas --json` and renders what comes
 * back, and the moment a project has a second one the list has two.
 *
 * The way to get a second is `openspec schema fork <source> [name]`, which copies a schema into
 * `openspec/schemas/<name>/` for editing (`source: "project"`), or `openspec schema init <name>`
 * for a fresh one. Both are marked *experimental* by the CLI's own help, which is why this is a
 * sentence under the control rather than a button beside it — cide does not put a gesture on an
 * upstream command that says it may change.
 *
 * The `unlisted` row does not count: it is there because the *file* names a schema the CLI did
 * not list, which is a different situation with its own sentence already.
 */
export function onlyDefaultSchema(options: readonly SchemaOption[]): boolean {
  const listed = options.filter((option) => !option.unlisted)
  return listed.length === 1 && listed[0]?.value === DEFAULT_SCHEMA_NAME
}

/**
 * The schema every installation has, and the commands that add another.
 *
 * The name is spelled here **and** in `cide_spec::config::DEFAULT_SCHEMA`, and the two cannot
 * import one another. That is the same standing hazard as every other pair in this codebase; what
 * makes it safe is that this constant is only ever compared against what the *CLI* listed, so a
 * rename upstream makes the hint stop appearing rather than making it lie.
 */
export const DEFAULT_SCHEMA_NAME = 'spec-driven'
export const SCHEMA_FORK_HINT =
  'OpenSpec ships one workflow, so one entry here is the usual case rather than a fault — this ' +
  'list is whatever `openspec schemas` reports. To add your own, `openspec schema fork ' +
  'spec-driven <name>` copies it into openspec/schemas/ for editing. Schema commands are marked ' +
  'experimental by the CLI.'

/**
 * The artifacts the rules editor draws a section for, in order.
 *
 * The chosen schema's artifacts first, in the workflow order the CLI reports, then any artifact
 * the draft *already states rules for* that the schema does not declare. That second half is not
 * tidiness: `cide_spec::config::DEFAULT_ARTIFACTS` is documented as a default and **not** a
 * closed set, so `rules:` may legally name an artifact from a schema the project has since
 * changed away from. Drawing only the schema's would hide those rules while leaving them in the
 * file — a form that shows something different from what the agent reads.
 */
export function artifactRows(
  draft: ConfigDraft,
  schemas: readonly SchemaLike[],
): { artifact: string; declared: boolean }[] {
  const declared = schemas.find((schema) => schema.name === draft.schema)?.artifacts ?? []
  const rows = declared
    .filter((artifact) => artifact.trim() !== '')
    .map((artifact) => ({ artifact, declared: true }))
  for (const entry of draft.rules) {
    if (rows.some((row) => row.artifact === entry.artifact)) continue
    if (normaliseList(entry.rules).length === 0) continue
    rows.push({ artifact: entry.artifact, declared: false })
  }
  return rows
}

/** The operations the guidance editor draws, in order: the two known ones, then any others. */
export function operationRows(draft: ConfigDraft): string[] {
  const rows = [...GUIDED_OPERATIONS]
  for (const entry of draft.operations) {
    if (rows.includes(entry.operation)) continue
    if (normaliseList(entry.guidance).length === 0) continue
    rows.push(entry.operation)
  }
  return rows
}

/* ------------------------------------------------------------------------- draft and edits */

/**
 * A working copy of what the file states.
 *
 * Built from the wire and from nothing else — in particular the schema is the **resolved** one,
 * so the select shows what the project is really on, while [`editsFor`] still knows the file
 * stated nothing and declines to write a line for it.
 */
export function draftFrom(config: ConfigLike): ConfigDraft {
  return {
    schema: resolvedSchema(config),
    context: config.context ?? '',
    rules: config.rules.map((entry) => ({ artifact: entry.artifact, rules: [...entry.rules] })),
    operations: config.operations.map((entry) => ({
      operation: entry.operation,
      guidance: [...entry.guidance],
    })),
  }
}

/**
 * The edits one Save should send: every value that really differs, and nothing else.
 *
 * # Why an edit per changed value and not "everything, always"
 *
 * `config::apply` would cope: it compares each value against what the file states, returns the
 * lines untouched when they are equal, and does not write the file at all when the whole rendered
 * text matches. Sending everything would therefore be *correct*. It would also make the Save
 * button a liar — enabled whenever the form is mounted, because nothing here can tell whether a
 * write would happen — and it would put the entire round-trip guarantee on one comparison in
 * Rust with nothing on this side able to state what it expects. [`isDirty`] is this function's
 * length, so the button and the write agree by construction.
 *
 * # The schema rule, spelled out
 *
 * A file that states no `schema:` *means* `spec-driven`. Emitting `Schema("spec-driven")` for it
 * would add a line to a committed file that nobody asked for — and `SpecConfig::schema` keeps the
 * `null` precisely so this function can tell the two apart. So: no edit when the file states
 * nothing and the draft is the default; an edit as soon as the draft is anything else.
 *
 * # The union, and why deleting is not what it looks like
 *
 * Rules are compared over the artifacts the form draws **and** every artifact the file already
 * states, so switching schema cannot silently delete the rules of the schema being left behind:
 * the draft still carries them, they compare equal, and no edit is emitted. Clearing a section's
 * last row *does* emit an edit with an empty list, which is how `config::Edit::Rules` spells
 * removal — the artifact's entry goes, and with the last one, `rules:` goes too.
 */
export function editsFor(
  draft: ConfigDraft,
  config: ConfigLike,
  schemas: readonly SchemaLike[],
): ConfigEditLike[] {
  const edits: ConfigEditLike[] = []

  const schema = draft.schema.trim()
  const statedNothing = config.schema === null || config.schema.trim() === ''
  if (schema !== '' && !(statedNothing && schema === config.defaultSchema)) {
    if (schema !== (config.schema ?? '')) edits.push({ kind: 'schema', schema })
  }

  const context = normaliseBlock(draft.context)
  if (context !== (config.context ?? '')) {
    edits.push({ kind: 'context', context: context === '' ? null : context })
  }

  const artifacts = [
    ...artifactRows(draft, schemas).map((row) => row.artifact),
    ...config.rules.map((entry) => entry.artifact),
    ...draft.rules.map((entry) => entry.artifact),
  ].filter((artifact, index, all) => all.indexOf(artifact) === index)
  for (const artifact of artifacts) {
    const want = normaliseList(rulesIn(draft.rules, artifact))
    const have = rulesIn(config.rules, artifact)
    if (!sameList(want, have)) edits.push({ kind: 'rules', artifact, rules: want })
  }

  const operations = [
    ...operationRows(draft),
    ...config.operations.map((entry) => entry.operation),
    ...draft.operations.map((entry) => entry.operation),
  ].filter((operation, index, all) => all.indexOf(operation) === index)
  for (const operation of operations) {
    const want = normaliseList(guidanceIn(draft.operations, operation))
    const have = guidanceIn(config.operations, operation)
    if (!sameList(want, have)) {
      edits.push({ kind: 'operationGuidance', operation, guidance: want })
    }
  }

  return edits
}

/** Would Save write anything? Exactly [`editsFor`]'s answer, so the button cannot disagree. */
export function isDirty(
  draft: ConfigDraft,
  config: ConfigLike,
  schemas: readonly SchemaLike[],
): boolean {
  return editsFor(draft, config, schemas).length > 0
}

function sameList(a: readonly string[], b: readonly string[]): boolean {
  if (a.length !== b.length) return false
  for (let i = 0; i < a.length; i += 1) if (a[i] !== b[i]) return false
  return true
}

/* ---------------------------------------------------------------------------- the wizard */

/**
 * What the set-up wizard will do with the context box as it stands.
 *
 * Three states rather than a boolean, because "Skip" has to be a *visible* outcome. A wizard
 * whose only button said `Set up OpenSpec` would leave somebody who typed nothing wondering
 * whether they had failed a required field, and one that hid the button until the box had text
 * would make the optional field feel mandatory. The button says which of the two it is about to
 * do, and the third state is the press that is already in flight.
 */
export type WizardAction = 'setUpWithContext' | 'setUpWithoutContext' | 'working'

export function wizardAction(context: string, busy: boolean): WizardAction {
  if (busy) return 'working'
  return normaliseBlock(context) === '' ? 'setUpWithoutContext' : 'setUpWithContext'
}

/** The button's words, for each of those. */
export function wizardLabel(action: WizardAction): string {
  switch (action) {
    case 'setUpWithContext':
      return 'Set up OpenSpec with this context'
    case 'setUpWithoutContext':
      return 'Set up OpenSpec without context'
    case 'working':
      return 'Setting up…'
    default:
      // Unreachable while `WizardAction` has three members, and still a string: a button whose
      // label came back `undefined` renders as an empty box that is nonetheless clickable.
      return 'Set up OpenSpec'
  }
}
