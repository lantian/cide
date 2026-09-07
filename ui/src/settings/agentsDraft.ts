/**
 * The rules behind Settings → Agents: what a role definition may say, what a form may refuse
 * before it asks Rust, and what makes an edit worth protecting.
 *
 * # Import-free, deliberately — and this is the *only* place this screen's rules live
 *
 * `ui/scripts/check-settings-agents.mjs` compiles this module standalone with the TypeScript in
 * `node_modules` and drives every function below. An import — even a type-only one of
 * `@/ipc/client` — makes that impossible, so the wire shapes it needs are restated structurally
 * here. They are `cide_ipc::AgentDraft`, `AgentScope`, `Harness` and `AgentField` and nothing
 * else; `tsc --noEmit` over `AgentsSection.tsx`, which holds both this module's types and the
 * generated ones, is what pins the restatement to the real thing.
 *
 * `theme.ts` and `claudeCli.ts` next door are the same arrangement for the same reason: a rule
 * inside a React component is a rule no check script can reach, and this project has shipped
 * six defects that lived in exactly that place.
 *
 * # What this file does NOT do, and must not grow into
 *
 * **It is not a second validator.** `cide_agents::defs::validate` is the authority and says in
 * its own doc why: `valid_name` is a path-safety rule about `Path::join`, `PERMISSION_MODES` is
 * the Claude CLI's vocabulary that module is the keeper of, and `.cide/agents/` has two other
 * writers — a hand editor and an MCP writer — that never touch a form. A copy here would drift
 * towards being the only one anybody consults, because it is the one the user sees.
 *
 * So [`localProblems`] refuses exactly two things, and they are the two that are *structurally*
 * hopeless rather than merely wrong: an empty name and an empty system prompt. Neither can ever
 * be written, both are visible from the box, and refusing them locally turns "press Save, wait
 * for a file write that was never going to happen, read the refusal" into a disabled button with
 * the sentence already under the field. Everything else — a name with a `/` in it, a permission
 * mode from a CLI release this build has not heard of, a tool name with a comma — is Rust's
 * answer and arrives as `AgentDraftProblem`s keyed by [`AgentFieldKey`].
 *
 * # The vocabularies
 *
 * [`SCOPES`], [`HARNESSES`] and [`PERMISSION_MODES`] are **closed** and the check script pins
 * each one to the Rust that defines it, as a set. [`EFFORT_SUGGESTIONS`] is deliberately *not*
 * closed — see its own doc. That asymmetry is the single most likely thing to be "tidied" here,
 * so [`isClosedVocabulary`] states it as a function and the check asserts both halves.
 */

/* -------------------------------------------------------------------- the wire, restated */

/** `cide_ipc::AgentScope`. Which directory the definition file lives in. */
export type Scope = 'project' | 'global' | 'claudeProject' | 'claudeGlobal'

/**
 * Is this scope a directory **Claude Code** owns rather than one cide does?
 *
 * `cide_ipc::AgentScope::is_claude_code`, restated. Four things on this screen turn on it — the
 * name rule, the harness control, the scope control, and whether Save may create — so it is a
 * function rather than four comparisons that can drift apart.
 */
export function isClaudeScope(scope: Scope): boolean {
  return scope === 'claudeProject' || scope === 'claudeGlobal'
}

/** `cide_ipc::Harness`. Which CLI actually runs a role. */
export type HarnessName = 'claude' | 'opencode' | 'qwen' | 'codex'

/**
 * `cide_ipc::AgentField` — which box on the form a refusal belongs to.
 *
 * Every variant Rust can produce must be a field this form actually draws, or the sentence
 * attached to it is a sentence the user never sees. That is what the check script asserts,
 * against the enum in `crates/cide-ipc/src/agents.rs` and against the `errorsFor('…')` calls in
 * `AgentsSection.tsx`.
 */
export type AgentFieldKey =
  | 'scope'
  | 'name'
  | 'label'
  | 'harness'
  | 'description'
  | 'model'
  | 'effort'
  | 'tools'
  | 'permissionMode'
  | 'maxConcurrent'
  | 'systemPrompt'
  | 'extras'

/** One refusal, against the field that carries it. Mirrors `cide_ipc::AgentDraftProblem`. */
export interface Problem {
  field: AgentFieldKey
  message: string
}

/** Where a form was populated from. Mirrors `cide_ipc::AgentLocation`. */
export interface Location {
  scope: Scope
  name: string
}

/**
 * One role as this form holds it.
 *
 * **`null` where the wire has an absent key**, throughout, and that is not cosmetic. The wire
 * type is `#[ts(optional)]`, so its unset fields are missing properties; `exactOptionalPropertyTypes`
 * then makes "set it to undefined" a type error and "delete the key" the only spelling. A form
 * holds every field at all times — a `<select>` has a value even when that value means *unset* —
 * so form state uses `null` and [`toWire`] is the one place the two representations meet.
 */
export interface Draft {
  scope: Scope
  name: string
  original: Location | null
  label: string | null
  harness: HarnessName | null
  description: string
  model: string | null
  effort: string | null
  tools: string[]
  permissionMode: string | null
  maxConcurrent: number | null
  systemPrompt: string
  /**
   * Every front-matter key cide does not model, in the order the file had them.
   *
   * Mirrors `cide_ipc::AgentExtra`. Ordinarily empty for a cide role and ordinarily not for a
   * Claude Code subagent, where `hooks`, `skills`, `mcpServers` and `maxTurns` all land here.
   *
   * **Edited as rows, not carried invisibly.** A draft that preserved text the form did not show
   * would make the form lie about what the file contains — which is the objection `AgentDraft`'s
   * own header used to raise against preserving anything at all, and the reason the answer is a
   * visible list rather than a hidden one.
   */
  extras: Extra[]
}

/** One unmodelled front-matter key. Mirrors `cide_ipc::AgentExtra`. */
export interface Extra {
  key: string
  /**
   * Everything after the colon. **May contain newlines**: a key that opened a nested block or a
   * block sequence carries the block's own lines, indentation intact, because the one honest
   * thing to do with YAML cide does not parse is not to touch it.
   */
  value: string
}

/* --------------------------------------------------------------------- closed vocabularies */

/**
 * Both scopes, project first.
 *
 * Project first because it is the answer for most roles and because it is the one that is
 * *reviewable*: a definition under `<root>/.cide/agents/` is committed, arrives in a pull
 * request, and is the same for everyone who checks the project out.
 */
export const SCOPES: readonly Scope[] = [
  'project',
  'global',
  'claudeProject',
  'claudeGlobal',
]

/** `cide_ipc::Harness`, as a set. Pinned to the Rust enum by the check script. */
export const HARNESSES: readonly HarnessName[] = ['claude', 'opencode', 'qwen', 'codex']

/**
 * `cide_agents::defs::PERMISSION_MODES`, in that module's order.
 *
 * Closed here because it is closed *there*: Rust refuses a value outside this list against the
 * `permissionMode` field, with the list in the sentence. A free-text box would therefore be a
 * box whose only interesting inputs are refusals — which is the state this whole screen exists
 * to end.
 *
 * It is somebody else's vocabulary and Rust's own doc says it is allowed to go stale. The
 * consequence for this form is stated on the control rather than hidden: a release that adds a
 * mode is a mode neither side offers, and the fix is to leave it unset until cide catches up.
 */
export const PERMISSION_MODES: readonly string[] = [
  'acceptEdits',
  'auto',
  'bypassPermissions',
  'manual',
  'dontAsk',
  'plan',
]

/**
 * Reasoning-effort values worth offering — and **not** a vocabulary anything is checked against.
 *
 * `AgentDraft::effort` is an `Option<String>` "carried verbatim and validated against nothing",
 * because the set differs per harness and per release. So this is a menu, not a rule: the form
 * draws these as choices *plus* a Custom escape, and a value outside the list is stored, sent
 * and written exactly as typed.
 *
 * The distinction matters enough to be a function ([`isClosedVocabulary`]) rather than a
 * convention, because the obvious tidy — "make effort a union like the other three" — would
 * silently start refusing a knob a `claude` release added last week, which is precisely the
 * failure `KNOWN_TOOLS` and `PERMISSION_MODES` document at length in Rust.
 */
export const EFFORT_SUGGESTIONS: readonly string[] = ['low', 'medium', 'high']

/** The fields whose values come from a closed set, and the set. */
export const CLOSED_VOCABULARIES: Readonly<Record<string, readonly string[]>> = {
  scope: SCOPES,
  harness: HARNESSES,
  permissionMode: PERMISSION_MODES,
}

/**
 * Whether a field's value must come from a list this build knows.
 *
 * `effort` answers `false` on purpose — see [`EFFORT_SUGGESTIONS`] — and so does `tools`, whose
 * `KNOWN_TOOLS` list in Rust is advisory for the same reason and additionally cannot see MCP
 * tools at all.
 */
export function isClosedVocabulary(field: AgentFieldKey): boolean {
  return Object.prototype.hasOwnProperty.call(CLOSED_VOCABULARIES, field)
}

/** Every field the form draws, in the order it draws them. Pinned to `AgentField` in Rust. */
export const AGENT_FIELDS: readonly AgentFieldKey[] = [
  'scope',
  'name',
  'label',
  'harness',
  'description',
  'model',
  'effort',
  'tools',
  'permissionMode',
  'maxConcurrent',
  'systemPrompt',
  'extras',
]

/**
 * The two fields a draft cannot be written without.
 *
 * Not "the required fields of the format" — `description` is empty in plenty of legal
 * definitions and Rust warns rather than refuses. These two are the ones whose absence makes the
 * file meaningless: the name *is* the file, and the prompt is, in `cide_agents::defs`' words,
 * "the whole of what makes this a role rather than a name".
 */
export const REQUIRED_FIELDS: readonly AgentFieldKey[] = ['name', 'systemPrompt']

/* ------------------------------------------------------------------------------ the draft */

/** A role that does not exist yet. `original: null` is what tells Rust this is a create. */
export function blankDraft(scope: Scope): Draft {
  return {
    scope,
    name: '',
    original: null,
    label: null,
    harness: null,
    description: '',
    model: null,
    effort: null,
    tools: [],
    permissionMode: null,
    maxConcurrent: null,
    systemPrompt: '',
    extras: [],
  }
}

/**
 * The two refusals this side is allowed to make, each as the sentence that belongs under its box.
 *
 * Deliberately shorter than Rust's, and deliberately not a paraphrase of it: these fire while the
 * user is still typing, so they say what is missing rather than explaining the rule. Rust's
 * longer sentence — the one about `Path::join` and `.cide/worktrees/` — arrives on the same field
 * the moment a *non-empty* name is refused, which is the case this cannot see.
 */
export function localProblems(draft: Draft): Problem[] {
  const problems: Problem[] = []
  if (draft.name.trim() === '') {
    problems.push({
      field: 'name',
      message: 'A role needs a name. It becomes the file name — <name>.md — and the word the orchestrator asks for this role by.',
    })
  }
  if (draft.systemPrompt.trim() === '') {
    problems.push({
      field: 'systemPrompt',
      message:
        'A role needs a system prompt. Without one the run is the harness’s default agent wearing a label, and cide greys the role for exactly that reason.',
    })
  }
  return problems
}

/** Whether Save may be pressed at all. Never a substitute for Rust's answer, only a shortcut. */
export function canSave(draft: Draft): boolean {
  return localProblems(draft).length === 0
}

/**
 * Problems bucketed by field, so each box can draw its own.
 *
 * A `Map` and not an object literal, on `check-problems.mjs`' `ROGUE` argument: an object keyed
 * by a string from the wire answers `constructor` with a function, React refuses a function as a
 * child, and `className` stringifies it into the entire source text of `Object`. `Map.get` on a
 * key nobody set answers `undefined` and nothing else.
 */
export function problemsByField(problems: readonly Problem[]): Map<AgentFieldKey, string[]> {
  const byField = new Map<AgentFieldKey, string[]>()
  for (const problem of problems) {
    const list = byField.get(problem.field)
    if (list === undefined) byField.set(problem.field, [problem.message])
    else list.push(problem.message)
  }
  return byField
}

/**
 * Has this draft moved since it was loaded?
 *
 * The whole of the unsaved-edit policy rests on this one answer, so it compares *values* rather
 * than identity: the form rebuilds the draft object on every keystroke, so an identity check
 * would report every untouched role as dirty and every dirty one as dirty, which is the same
 * thing as not asking.
 *
 * `null` for `saved` means a role being created, which is dirty as soon as anything is typed —
 * and clean while it is still blank, so opening New role and clicking away costs no confirm.
 */
export function isDirty(saved: Draft | null, draft: Draft): boolean {
  const baseline = saved ?? blankDraft(draft.scope)
  return signature(baseline) !== signature(draft)
}

/** Every field that decides equality, in one string. `original` is excluded: it is not edited. */
function signature(draft: Draft): string {
  return JSON.stringify([
    draft.scope,
    draft.name,
    draft.label,
    draft.harness,
    draft.description,
    draft.model,
    draft.effort,
    draft.tools,
    draft.permissionMode,
    draft.maxConcurrent,
    draft.systemPrompt,
    // Included, or editing a `hooks:` block and clicking away would cost no confirm and the edit
    // would be gone. `JSON.stringify` over an array of objects compares order as well as
    // content, which is right: reordering the keys of a file *is* a change to that file.
    draft.extras,
  ])
}

/**
 * What a cached draft is worth on the way back into the screen.
 *
 * The form survives a section switch and a tab switch in a module-level cache, because a system
 * prompt is long and losing one to a mis-click on the nav is the exact complaint that produced
 * this screen. But a *clean* cached draft is worth nothing and costs correctness: the file has
 * three other writers — the user's editor, a `git checkout`, an agent working in this repository
 * — and `cide_app::cmd::agents::agents_draft`'s doc is explicit that a form populated from an old
 * read and then saved reverts whatever landed in between.
 *
 * So: restore only what would otherwise be *lost*. Anything clean is dropped and read again.
 */
export function shouldRestore(cached: { saved: Draft | null; draft: Draft } | null): boolean {
  if (cached === null) return false
  return isDirty(cached.saved, cached.draft)
}

/* ---------------------------------------------------------------------------- scope moves */

/**
 * What pressing Save will do to the *file*, when that is more than writing it — or `null`.
 *
 * # Why the scope control lives on the form and not on the list
 *
 * `scope` is a field of `AgentDraft` because changing it is a **move between two directories**,
 * and `AgentDraft.original` exists so the backend can perform one. A control in the list — two
 * tabs, or a drag between them — would make that move happen on a *click*, with no moment in
 * between at which anything could say what was about to happen and nothing to undo it with. On
 * the form it is a value like any other: the user changes it, reads this sentence, and the move
 * happens when they press Save, together with everything else they changed.
 *
 * The sentence is composed here rather than in the component so the check script can drive every
 * branch, including the one that matters most: moving a role onto a name the other scope already
 * uses is refused by `cide_agents::defs::save`, and a user who finds that out by pressing Save
 * has been told about a collision they could have been shown.
 *
 * `taken` is the set of definition files that already exist, as `scope:name` — the listing the
 * screen has already read, rather than a fresh probe, because this runs on every keystroke.
 */
export function scopeChangeWarning(draft: Draft, taken: ReadonlySet<string>): string | null {
  const original = draft.original
  const name = draft.name.trim()
  const target = `${draft.scope}:${name}`

  if (original === null) {
    // A create. There is no move, but there is still a file that may already be there, and
    // "create over an existing role" is refused rather than merged — the file it would replace
    // is somebody's system prompt.
    if (name !== '' && taken.has(target)) {
      return `A ${scopeLabel(draft.scope).toLowerCase()} role called “${name}” already exists. Saving will be refused rather than overwrite it — open that role to edit it, or choose another name.`
    }
    return null
  }

  const renamed = original.name !== name && name !== ''
  const moved = original.scope !== draft.scope

  // A subagent is rewritten where it lies: Claude Code keys by the `name:` value and its filename
  // need not agree, so renaming one is an edit *inside* a file rather than a move between two.
  // The sentences below all describe a file appearing and another going away, and none of that
  // happens here — saying it would be a warning about a consequence that has none.
  if (isClaudeScope(draft.scope) && !moved) return null

  if (!renamed && !moved) return null

  if (name !== '' && taken.has(target)) {
    return `${describeMove(original, draft, moved)} A role already occupies that file, so the save will be refused rather than overwrite it.`
  }
  return describeMove(original, draft, moved)
}

/** The move itself, in the user's terms: which file goes away and which one appears. */
function describeMove(original: Location, draft: Draft, moved: boolean): string {
  const from = `${scopeLabel(original.scope).toLowerCase()} role “${original.name}”`
  const to = `${scopeLabel(draft.scope).toLowerCase()} role “${draft.name.trim()}”`
  if (moved && draft.scope === 'global') {
    return `Saving moves ${from} out of this project and into your ${to}: the file leaves the repository — and its history — and applies to every project you open instead.`
  }
  if (moved) {
    return `Saving moves ${from} into this project as ${to}: the file lands in the repository, so it will show up in your next commit and your teammates get it too.`
  }
  return `Saving renames ${from} to “${draft.name.trim()}”. The old file is removed; anything that dispatches this role by name has to be updated.`
}

/* ------------------------------------------------------------------------------- the list */

/** One definition file the screen found, before it is arranged into rows. */
export interface Entry {
  scope: Scope
  name: string
  label: string
  /**
   * The harness *this file* names, or `null` for "whatever the project defaults to".
   *
   * Read from the file's own draft rather than from the merged roster row, and the two really do
   * differ: `AgentDef.harness` is a `Harness` with no unset arm, so the roster has already
   * resolved the default and cannot say whether the file named one. That distinction is the
   * whole of [`harnessLabel`]'s `null` case, and it is a fact about the file the user is about
   * to edit.
   */
  harness: HarnessName | null
  /**
   * `AgentDef.unavailable` — why a dispatch of this name cannot run, as Rust's own sentence — or
   * `null` when it can.
   *
   * It comes off the **merged** roster, so it is a fact about whichever file wins, not about
   * this one. [`rowsFor`] is where that is reconciled: a shadowed row is not allowed to carry
   * it, because the sentence would be describing the project file while sitting under the global
   * one. See the note there.
   */
  unavailable: string | null
}

/**
 * Which scope wins when two files declare one name. Lower is stronger.
 *
 * `cide_agents::defs::scopes()` merges lowest-precedence first, so this is that list reversed and
 * it must stay that list reversed: a screen that ranked them differently from the loader would
 * draw the *inert* file as the one that runs, which is the failure `shadowed` exists to prevent
 * pointing the wrong way round.
 *
 * The rule is two rules composed. *Project beats user* is what both formats already say about
 * themselves. *cide's own directory beats Claude Code's* is the one this had to invent, and it
 * goes this way because `.cide/agents/` is the only one of the four that can state how cide
 * should run a role — `max-concurrent`, `worktree`, a harness that is not Claude.
 */
const SCOPE_RANK: Readonly<Record<Scope, number>> = {
  project: 0,
  global: 1,
  claudeProject: 2,
  claudeGlobal: 3,
}

/**
 * One row of the role list.
 *
 * `shadowed` is the reason this function exists. Two files may share a name across any of the
 * four scopes, and the stronger one wins **whole-file** — not key by key — so the rows are not
 * duplicates and are not alternatives either: one of them is what runs and the other is inert
 * until it is deleted. Two identical-looking rows would be the worst possible drawing of that,
 * and dropping the shadowed one would hide a file the user still owns and can still edit.
 */
export interface Row extends Entry {
  /** This file is what a dispatch of this name actually uses. */
  effective: boolean
  /** A stronger scope declares this name too, so nothing will ever run this file. */
  shadowed: boolean
  /** This row wins over a weaker scope's file of the same name. */
  shadows: boolean
}

/**
 * The list, sorted by name and with the shadowing worked out.
 *
 * Stronger scope first within a name, so a shadowed row is drawn directly under the row that
 * shadows it and the relationship needs no line to connect them.
 */
export function rowsFor(entries: readonly Entry[]): Row[] {
  const rank = (scope: Scope) => SCOPE_RANK[scope]
  // The strongest scope holding each name. Computed once rather than per row, and as a *rank*
  // rather than as a set of names per scope — with four scopes the set-per-scope shape would be
  // four sets and six comparisons, and the one that got forgotten would be a silent wrong answer.
  const strongest = new Map<string, number>()
  for (const entry of entries) {
    const best = strongest.get(entry.name)
    if (best === undefined || rank(entry.scope) < best) strongest.set(entry.name, rank(entry.scope))
  }
  return [...entries]
    .sort((a, b) => a.name.localeCompare(b.name) || rank(a.scope) - rank(b.scope))
    .map((entry) => {
      const shadowed = rank(entry.scope) > (strongest.get(entry.name) ?? rank(entry.scope))
      return {
        ...entry,
        harness: entry.harness ?? null,
        /*
         * **A shadowed row never carries the unavailable sentence.**
         *
         * `AgentDef.unavailable` arrives on a *merged* roster row: one entry per name, with the
         * project file's answer where both scopes hold the name. Spreading it onto both rows
         * would put "its system prompt is empty" under the global file — which may have a
         * perfectly good prompt — because the project file that shadows it does not. That is the
         * dialog lying in the most expensive direction: it would send the user to edit the file
         * that is not the problem.
         *
         * The shadowed row's real state is *shadowed*, which [`usability`] says instead, and
         * which is a stronger claim anyway: nothing will run this file at all, whatever it says.
         */
        unavailable: shadowed ? null : (entry.unavailable ?? null),
        effective: !shadowed,
        shadowed,
        shadows: entries.some(
          (other) => other.name === entry.name && rank(other.scope) > rank(entry.scope),
        ),
      }
    })
}

/**
 * Whether a row's role can actually run, as one of three answers and never as two.
 *
 * `cide_ipc::AgentDef::unavailable`'s doc is the rule this follows, one layer out: *"a role whose
 * harness binary is not on `PATH`, whose system prompt is empty, or which two config files
 * declared under one name, is drawn greyed **with this sentence**"* — never greyed with nothing
 * saying why, and never offered and then refused. `AgentsPanel/model.ts`'s `canDispatch` returns
 * exactly one of a green light or a sentence for the same reason; this is the settings screen's
 * version of that answer, for a list whose rows are files rather than dispatch targets.
 *
 * `shadowed` is checked **first**, and that order is the whole reason this is a function. A
 * shadowed file's own state is beside the point — a flawless global `qa` is just as inert as a
 * broken one while the project defines `qa` — and the two words say different things to a user
 * deciding what to fix: *blocked* means edit this file, *shadowed* means delete the other one.
 */
export type Usability =
  | { kind: 'ready' }
  | { kind: 'shadowed' }
  | { kind: 'blocked'; why: string }

export function usability(row: Row): Usability {
  if (row.shadowed) return { kind: 'shadowed' }
  if (row.unavailable !== null) return { kind: 'blocked', why: row.unavailable }
  return { kind: 'ready' }
}

/** The word on the row's status chip. The sentence, when there is one, is drawn under it. */
export function usabilityLabel(state: Usability): string {
  if (state.kind === 'ready') return 'Ready'
  if (state.kind === 'shadowed') return 'Shadowed'
  return 'Cannot run'
}

/** `scope:name`, the key both the list and [`scopeChangeWarning`] identify a *file* by. */
export function fileKey(scope: Scope, name: string): string {
  return `${scope}:${name}`
}

/* ------------------------------------------------------------------- the modal, and focus */

/**
 * What the role dialog is open for, or `null` for closed.
 *
 * # Why the form is a dialog at all
 *
 * It was an inline panel under the list, and the user asked for a modal in those words. The
 * request is also the right shape for what this form *is*: eleven fields and a 320px system
 * prompt, below a list the user has to scroll past to reach it, with a Save that has to be found
 * again afterwards. A dialog puts the whole definition in one place, and — the part that matters
 * for correctness — it makes "which role am I editing" impossible to get wrong, because there is
 * exactly one and nothing else on screen is clickable while it is up.
 *
 * # Why the state is a discriminated union and not a boolean
 *
 * `{ open: boolean }` beside the existing `selection` would be two variables that can disagree:
 * open-with-nothing-selected and closed-with-a-dirty-draft are both representable and both
 * wrong. This says what the dialog is *for* in one value, and [`modalFor`] derives it from the
 * draft rather than letting a caller assert it — a create is `original === null` and nothing
 * else, which is the same fact `agents_save` reads to decide between writing and moving.
 */
export type Modal = { kind: 'new' } | { kind: 'edit'; file: Location } | null

/** The dialog a draft opens: a create, or an edit of the file it was read from. */
export function modalFor(draft: Draft): Modal {
  const original = draft.original
  return original === null ? { kind: 'new' } : { kind: 'edit', file: original }
}

/** The dialog's own title. `New role` names the act; an edit names the *file*, not the label. */
export function modalTitle(modal: Modal): string {
  if (modal === null) return ''
  return modal.kind === 'new' ? 'New role' : `${modal.file.name}.md`
}

/**
 * The dialog the screen comes back to when a draft survived the unmount.
 *
 * **A restored draft reopens the dialog, always.** [`shouldRestore`] only restores a *dirty*
 * one — a clean cached draft is dropped and read again, for the staleness reason its own doc
 * gives — so anything that comes back is unsaved writing. Coming back to a closed dialog would
 * leave that writing alive in a module-level map with nothing on screen pointing at it: the list
 * would look untouched, the user would open the role again, and the form would be populated from
 * their own edits with no sign of where they came from. Reopening is what makes the promise on
 * the screen — *edits survive switching sections and switching tabs* — a thing the user can see.
 */
export function restoredModal(cached: { saved: Draft | null; draft: Draft } | null): Modal {
  if (!shouldRestore(cached) || cached === null) return null
  return modalFor(cached.draft)
}

/**
 * What a request to close the dialog does — Escape, the scrim, or the Cancel button.
 *
 * # Escape does not discard writing, and that is a deliberate divergence
 *
 * `chrome/ConfirmDestructive.tsx` is this codebase's dialog precedent and its rule 3 is that
 * **Cancel is the default**: Escape cancels, initial focus is on Cancel, and the reflexive Enter
 * already in flight backs out. That rule holds there because backing out of a confirmation costs
 * nothing — the act simply does not happen.
 *
 * Here it costs a system prompt. This screen's whole unsaved-edit policy exists because that
 * value is long and expensive to retype: nothing is sent until Save, opening another role while
 * dirty asks first, and the draft survives an unmount in `DRAFTS`. An Escape that closed the
 * dialog would be the one gesture on the screen that throws that away silently, and it would be
 * the gesture most likely to be made by accident — Escape is the key a user presses at whatever
 * is in front of them.
 *
 * So the answer splits on whether anything is at stake, which is rule 2 of that same header
 * — *it only appears when something is at risk* — applied to the dismissal instead of the
 * dialog:
 *
 * * **clean** → `'close'`. Nothing is lost; Escape behaves exactly as it does everywhere else.
 * * **dirty** → `'ask'`. The dialog **stays open** and arms its discard confirm. Escape never
 *   closes it and never discards; discarding takes a deliberate click on a named button.
 *
 * The rejected options, both of which were on the table:
 *
 * * *refuse Escape while dirty* — a key that does nothing is a key the user presses harder, and
 *   it leaves them with no way out of the dialog that does not involve reading the footer
 *   anyway. `'ask'` is that option plus an answer.
 * * *close and keep the draft* — cheap, and `DRAFTS` would even make it work. It was rejected
 *   because the draft would then be invisible: a closed dialog over an unchanged-looking list,
 *   with the user's writing recoverable only by guessing that reopening the role would produce
 *   it. Unsaved work must be either on screen or explicitly discarded, never merely parked.
 *
 * The one Escape-bound default that header insists on therefore still exists and is still
 * exactly one: *the dialog's safe answer*. It is "close" when closing is safe and "ask" when it
 * is not, and it is never "discard".
 */
export function closeRequest(dirty: boolean): 'close' | 'ask' {
  return dirty ? 'ask' : 'close'
}

/**
 * Which file a focus request names, once the listing has arrived.
 *
 * `sidebar/agentsStore.ts`'s `configure` records an `AgentId` and opens the Settings tab; the id
 * is all it can record, because `AgentDef` on the wire does not say which scope a definition
 * lives in — the roster is merged. Resolving it is this side's job, and it needs the 2N probe
 * the list already performs.
 *
 * Three answers, and the first one is the reason this is not a `find`:
 *
 * * **`wait`** — the listing is still in flight (`rows === null`). The request is *held*, not
 *   dropped: `configure` opens the tab and the screen mounts in the same frame, so the request
 *   almost always arrives before the file probes come back. Answering "no such role" during that
 *   window would make the feature fail exactly when it is used normally.
 * * **`file`** — one row, or two. Two happens when a project role and a global role share a
 *   name, and the **project** one is the answer: it is the row whose `effective` is true, which
 *   is to say it is the file a dispatch of that name actually runs. The user pressed Configure
 *   on a *roster* row, and the roster merged that pair into the project file's answer — so
 *   opening the global one would open a file that has nothing to do with what they were reading.
 * * **`unknown`** — the listing is in and nothing matches. Real, not defensive: the roster can
 *   name a role whose definition file neither `agents_draft` probe could read, and a role can be
 *   deleted between the Configure press and the tab opening.
 */
export type FocusTarget = { kind: 'wait' } | { kind: 'file'; file: Location } | { kind: 'unknown' }

export function focusTarget(name: string, rows: readonly Row[] | null): FocusTarget {
  if (rows === null) return { kind: 'wait' }
  const matches = rows.filter((row) => row.name === name)
  // `effective` and not `scope === 'project'`: the two agree today, and the one that states the
  // *reason* is the one that keeps agreeing if shadowing ever grows a third case.
  const winner = matches.find((row) => row.effective) ?? matches[0]
  if (winner === undefined) return { kind: 'unknown' }
  return { kind: 'file', file: { scope: winner.scope, name: winner.name } }
}

/**
 * What the screen does with a resolved focus request, given the state of the form.
 *
 * The dirty case is the whole of this function. Two answers were obviously wrong:
 *
 * * **switch anyway** — a Configure press in the sidebar would silently replace a half-written
 *   system prompt with another role's. That is the exact loss this screen was built to prevent,
 *   arriving through the one door that does not go past the existing guard, because the guard is
 *   on the list rows and this request comes from another panel entirely.
 * * **ignore it** — the user clicked a control that promises to open a role and nothing happens.
 *   This project has a name for that state and a check script whose job is to make it
 *   unrepresentable; a Configure that is drawn on every roster row unconditionally is the last
 *   control that may quietly do nothing.
 *
 * So it becomes `confirm`: the request is honoured as far as it safely can be, which is that the
 * open dialog arms its discard confirm and *names the role that was asked for*. One click
 * completes the gesture, one click keeps the writing, and neither of them is the default. It is
 * the same shape the list rows already use for the same collision, moved to where the collision
 * is now visible — with the dialog up, the rows are behind a scrim and cannot draw anything.
 *
 * A declined confirm **drops** the request rather than parking it. A held request would fire at
 * the next close of the dialog, which may be minutes and three roles later, and a jump that
 * arrives long after the click that caused it is indistinguishable from a bug. `takeSpawnPlan`'s
 * rule, one layer out: a request is good for one gesture.
 */
export type FocusAction =
  | { kind: 'wait' }
  | { kind: 'open'; file: Location }
  | { kind: 'confirm'; file: Location }
  | { kind: 'unknown' }

export function focusAction(target: FocusTarget, dirty: boolean): FocusAction {
  if (target.kind !== 'file') return target
  return dirty ? { kind: 'confirm', file: target.file } : { kind: 'open', file: target.file }
}

/**
 * Which role the screen opens on: the focus request, the dialog already up, or nothing.
 *
 * The single entry point, so that the ordering is one claim in one place: **a focus request
 * outranks whatever the screen was showing, and neither outranks unsaved writing.**
 *
 * The first half is what makes the sidebar's Configure feel like a command rather than a
 * suggestion — a screen that kept last session's restored draft over the role the user just
 * clicked is a screen that ignored them. The second half is [`focusAction`]'s `confirm` arm
 * doing its work before the branch order is ever reached.
 *
 * `standing` is the dialog that is open right now, which on the frame the screen mounts is
 * exactly [`restoredModal`]'s answer — a draft that survived an unmount, and therefore, by
 * [`shouldRestore`], a dirty one. So the collision between "a role was requested" and "there is
 * unsaved writing on screen" cannot be silent: it arrives as `confirm`, naming both sides.
 *
 * `wait` and `unknown` deliberately leave `standing` alone rather than clearing it. A request
 * that cannot be answered yet, or at all, is no reason to take away what the user was looking
 * at.
 */
export type Opening =
  | { kind: 'focus'; file: Location }
  | { kind: 'confirm'; file: Location }
  | { kind: 'wait' }
  /** Carries the name so the screen can say *which* role it could not find. */
  | { kind: 'unknown'; name: string }
  | { kind: 'keep'; modal: Modal }
  | { kind: 'none' }

export function screenOpening(
  request: string | null,
  rows: readonly Row[] | null,
  dirty: boolean,
  standing: Modal,
): Opening {
  if (request === null) return standing === null ? { kind: 'none' } : { kind: 'keep', modal: standing }
  const action = focusAction(focusTarget(request, rows), dirty)
  if (action.kind === 'open') return { kind: 'focus', file: action.file }
  if (action.kind === 'confirm') return { kind: 'confirm', file: action.file }
  return action.kind === 'wait' ? { kind: 'wait' } : { kind: 'unknown', name: request }
}

/* ------------------------------------------------------------------------------- wording */

/** What the scope segment and the badges say. */
export function scopeLabel(scope: Scope): string {
  if (scope === 'claudeProject') return 'Claude Code (project)'
  if (scope === 'claudeGlobal') return 'Claude Code (user)'
  return scope === 'project' ? 'Project' : 'Global'
}

/** The sentence under the scope control, per scope. Both name the directory, because that is
 *  the whole of what scope means — there is no key in the file for it. */
export function scopeHint(scope: Scope): string {
  return scope === 'project'
    ? '<root>/.cide/agents/<name>.md — committed with the project, reviewed in its pull requests, the same for everyone who checks it out.'
    : '$XDG_CONFIG_HOME/cide/agents/<name>.md — beside keymap.json, yours, and applied to every project you open unless that project defines the same name.'
}

/** What a harness segment says. Unset is a real and common state, not a missing value. */
export function harnessLabel(harness: HarnessName | null): string {
  if (harness === null) return 'Project default'
  switch (harness) {
    case 'claude':
      return 'Claude'
    case 'opencode':
      return 'opencode'
    case 'qwen':
      return 'qwen'
    case 'codex':
      return 'Codex'
    // A trailing `: 'opencode'` is what this used to be, and it was a mislabel waiting for the
    // third harness: every CLI cide had not heard of would have been drawn as opencode, in a
    // dropdown, with nothing anywhere saying so. A `never` makes the next one a compile error.
    default: {
      const unreachable: never = harness
      return unreachable
    }
  }
}

/**
 * Which harness's models this draft should be offered.
 *
 * `AgentDraft.harness` is nullable — "project default" is a real state and not a missing value,
 * which is the whole of [`harnessLabel`]'s `null` case — so the form cannot ask for a model list
 * with it directly. This is the resolution, and it mirrors `cide_agents::defs::load_from`'s:
 * a Claude Code scope is Claude whatever the file says, otherwise the file, otherwise the
 * project's `.cide/config.json` default, otherwise Claude.
 *
 * The claude-scope override is not a nicety. `.claude/agents/` has no `harness:` key at all, and
 * `harness/claude.rs` suppresses `--model` entirely for a subagent because the CLI reads it out
 * of the definition file itself — so offering that role opencode's `provider/model` ids would be
 * offering values cide has already decided never to pass.
 *
 * Here rather than in the component so a check script can drive it: a rule inside a React
 * component is a rule nothing in this project can test.
 */
export function effectiveHarness(draft: Draft, projectDefault: HarnessName | null): HarnessName {
  if (isClaudeScope(draft.scope)) return 'claude'
  return draft.harness ?? projectDefault ?? 'claude'
}

/**
 * What the empty Model box suggests, per harness.
 *
 * The field was placeholdered `sonnet` for every harness, which is Claude's alias vocabulary and
 * is not a thing opencode accepts: it wants `provider/model`. A placeholder that names a value
 * the harness would reject is worse than an empty one, because it reads as an example.
 */
export function modelPlaceholder(harness: HarnessName): string {
  switch (harness) {
    case 'claude':
      return 'sonnet'
    case 'opencode':
      return 'anthropic/claude-sonnet-4-5'
    case 'qwen':
      return 'qwen3-coder-plus'
    // A slug from `codex debug models` — the catalog's own spelling, which the menu beside the
    // box is read from on this machine. (M44)
    case 'codex':
      return 'gpt-5.5'
    default: {
      const unreachable: never = harness
      return unreachable
    }
  }
}

/** `code-reviewer` → `Code Reviewer`, the same rule `cide_agents::defs::label_from_id` uses. */
export function titleCase(name: string): string {
  return name
    .split('-')
    .filter((word) => word !== '')
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(' ')
}

/* ---------------------------------------------------------------------------------- wire */

/** `cide_ipc::AgentDraft`, restated. Optional keys are *absent*, never `undefined`. */
export interface WireDraft {
  scope: Scope
  name: string
  original?: Location
  label?: string
  harness?: HarnessName
  description: string
  model?: string
  effort?: string
  tools: string[]
  permissionMode?: string
  maxConcurrent?: number
  systemPrompt: string
  extras: Extra[]
}

/**
 * Form state as the command's argument.
 *
 * Every `null` becomes an *absent key* rather than a `null` value, because `AgentDraft` is
 * `deny_unknown_fields` and its optional fields are `Option<T>` behind `#[ts(optional)]`: serde
 * reads a missing key as `None` and would read an explicit `null`… also as `None`, but only for
 * the ones that are `Option`, and a key the form invented is a hard deserialisation error. The
 * safe spelling is the one the generated type describes, and `exactOptionalPropertyTypes` makes
 * the compiler enforce it here.
 *
 * Blank strings are dropped too, on the same grounds `cide_agents::defs::normalize` drops them:
 * `label:` with nothing after it and no `label:` line mean the same thing, and the shorter file
 * is the one a reviewer can take in. `name` and `systemPrompt` are trimmed but never dropped —
 * they are required, and [`localProblems`] has already refused them when they are empty.
 */
export function toWire(draft: Draft): WireDraft {
  const wire: WireDraft = {
    scope: draft.scope,
    name: draft.name.trim(),
    description: draft.description.trim(),
    tools: draft.tools.map((tool) => tool.trim()).filter((tool) => tool !== ''),
    systemPrompt: draft.systemPrompt,
    // Sent whole and always, empty list included: `extras` is `Vec` and not `Option`, and a save
    // that omitted it would deserialise as "this file has no unmodelled keys" — which is how a
    // `hooks:` block gets deleted by a form that thought it was preserving one. The empty rows a
    // half-typed key leaves behind are dropped, so an abandoned "Add" is not a refusal.
    extras: draft.extras
      .map((extra) => ({ key: extra.key.trim(), value: extra.value }))
      .filter((extra) => extra.key !== ''),
  }
  if (draft.original !== null) wire.original = draft.original
  const text = (value: string | null): string | null => {
    const trimmed = value === null ? '' : value.trim()
    return trimmed === '' ? null : trimmed
  }
  const label = text(draft.label)
  if (label !== null) wire.label = label
  if (draft.harness !== null) wire.harness = draft.harness
  const model = text(draft.model)
  if (model !== null) wire.model = model
  const effort = text(draft.effort)
  if (effort !== null) wire.effort = effort
  const mode = text(draft.permissionMode)
  if (mode !== null) wire.permissionMode = mode
  if (draft.maxConcurrent !== null) wire.maxConcurrent = draft.maxConcurrent
  return wire
}

/** A `agents_draft` answer as form state: the same conversion the other way. */
export function fromWire(wire: WireDraft): Draft {
  return {
    scope: wire.scope,
    name: wire.name,
    original: wire.original ?? null,
    label: wire.label ?? null,
    harness: wire.harness ?? null,
    description: wire.description,
    model: wire.model ?? null,
    effort: wire.effort ?? null,
    tools: [...wire.tools],
    permissionMode: wire.permissionMode ?? null,
    maxConcurrent: wire.maxConcurrent ?? null,
    systemPrompt: wire.systemPrompt,
    extras: wire.extras.map((extra) => ({ ...extra })),
  }
}
