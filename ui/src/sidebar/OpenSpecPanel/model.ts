/**
 * The OpenSpec panel's pure core: the board's states, the tree it draws, and the one gate that
 * decides whether a change can be accepted. (M28)
 *
 * # Why this file imports nothing
 *
 * `ui/scripts/check-openspec.mjs` compiles this module **alone** with the TypeScript already in
 * `node_modules` and imports the emitted JS under node. That works only while the module has no
 * imports at all — not even a type-only one through the `@/*` alias, which a bare `tsc` cannot
 * resolve. `TasksPanel/model.ts`, `AgentsPanel/model.ts`, `chrome/sidebarView.ts` and
 * `ProblemsPanel/model.ts` are all import-free for exactly this reason: this project has no JS
 * test runner, and a panel must never have to be launched to find out whether a button is offered.
 *
 * So the shapes below are **structural restatements** of `SpecBoard`, `SpecChange`, `SpecDelta`
 * and the rest from `ui/src/ipc/generated.ts` rather than those types. `adapt.ts` is the seam
 * allowed to import the generated DTOs, it calls these functions with values built from the real
 * ones, and a field renamed on the Rust side therefore still fails the build — one module further
 * out. What a restatement cannot catch on its own is a renamed enum **variant**, so
 * `check-openspec.mjs` reads `pub enum DeltaOperation` out of `crates/cide-ipc/src/spec.rs` and
 * pins [`DELTA_OPS`] against it as a set.
 */

/* ----------------------------------------------------------------- what a delta does */

/**
 * The four things a change can do to a requirement. Closed upstream, and closed here.
 *
 * OpenSpec's own set: a delta file's sections are `## ADDED | MODIFIED | REMOVED | RENAMED
 * Requirements`. Pinned against the Rust enum by the check, so a fifth arriving upstream fails
 * the build here rather than rendering an empty chip nobody notices.
 */
export type DeltaOp = 'added' | 'modified' | 'removed' | 'renamed'

export const DELTA_OPS: readonly DeltaOp[] = ['added', 'modified', 'removed', 'renamed']

export function isDeltaOp(value: string): value is DeltaOp {
  return DELTA_OPS.includes(value as DeltaOp)
}

/**
 * Colour roles a delta can take. Each maps to one token in the panel's stylesheet.
 *
 * Four names for four operations, and `renamed` gets its own rather than sharing `modified`'s: a
 * rename changes no behaviour at all, and a reviewer scanning a list needs to be able to skip it.
 */
export type OpTone = 'add' | 'change' | 'remove' | 'move'

export const OP_TONES: readonly OpTone[] = ['add', 'change', 'remove', 'move']

/**
 * What the chip prints.
 *
 * Falls back to the raw string rather than to a placeholder, `harnessLabel`'s rule one panel
 * over: an operation cide has not heard of is still a fact about the delta, and printing what the
 * backend sent is strictly more useful than printing `unknown`.
 */
export function opLabel(op: string): string {
  if (op === 'added') return 'Added'
  if (op === 'modified') return 'Changed'
  if (op === 'removed') return 'Removed'
  if (op === 'renamed') return 'Renamed'
  return op.trim() === '' ? 'unknown change' : op
}

export function opTone(op: string): OpTone {
  if (op === 'added') return 'add'
  if (op === 'removed') return 'remove'
  if (op === 'renamed') return 'move'
  // Everything unknown reads as a change, which is the honest default: something about this
  // requirement is different and cide cannot say what kind of different.
  return 'change'
}

/** The icon name a delta row draws. Icon *names*, never characters — `check:ui-icons`' rule. */
export function opIcon(op: string): string {
  if (op === 'added') return 'plus'
  if (op === 'removed') return 'minus'
  if (op === 'renamed') return 'arrow-right'
  return 'pencil'
}

/* ------------------------------------------------------------------- where a change is */

/**
 * A change's stage, derived rather than stored.
 *
 * Deliberately **not** the CLI's `status` string, which cide carries but does not switch on: that
 * is upstream's vocabulary and it can grow in a point release. This is cide's own reading of the
 * two numbers and the verdict, and it is what the panel groups and colours by.
 */
export type ChangeStage = 'proposed' | 'inProgress' | 'ready' | 'invalid' | 'archived'

export const CHANGE_STAGES: readonly ChangeStage[] = [
  'proposed',
  'inProgress',
  'ready',
  'invalid',
  // Terminal, and reachable only through `stageOf` — a summary row cannot be archived, because
  // `openspec list` does not list an archived change at all.
  'archived',
]

export function isChangeStage(value: string): value is ChangeStage {
  return CHANGE_STAGES.includes(value as ChangeStage)
}

export function stageLabel(stage: string): string {
  if (stage === 'proposed') return 'Proposed'
  if (stage === 'inProgress') return 'In progress'
  if (stage === 'ready') return 'Ready to accept'
  if (stage === 'invalid') return 'Needs fixing'
  if (stage === 'archived') return 'Archived'
  return stage.trim() === '' ? 'unknown' : stage
}

/* ----------------------------------------------------------------------- the wire shapes */

export interface ScenarioView {
  /** Off the `#### Scenario: <title>` header. Recovered from the file — see `block::parse`. */
  title: string
  /** Everything under the header, as written. */
  body: string
}

export interface RequirementView {
  name: string
  text: string
  scenarios: readonly ScenarioView[]
  /**
   * The whole block, header included, exactly as it is in the file.
   *
   * What the editor edits and what `spec_requirement_set` replaces. Carried verbatim rather than
   * recomposed from the fields above, because recomposing is lossy in a way nobody notices until
   * a diff: prose the user never touched comes back reflowed, and blank lines, indentation and
   * any markup the fields do not model are silently normalised away.
   */
  block: string
}

export interface DeltaView {
  spec: string
  op: DeltaOp
  description: string
  requirements: readonly RequirementView[]
  renamedFrom: string | null
  renamedTo: string | null
}

export interface TaskItem {
  done: boolean
  description: string
}

export interface IssueView {
  level: string
  path: string
  message: string
  line: number | null
}

export interface ValidationView {
  valid: boolean
  issues: readonly IssueView[]
}

export interface ArtifactView {
  id: string
  generates: string
  state: string
  existing: readonly string[]
}

export interface ChangeSummaryView {
  name: string
  completed: number
  total: number
  status: string
}

export interface CapabilityView {
  id: string
  requirements: number
}

export interface ChangeView {
  name: string
  title: string
  deltas: readonly DeltaView[]
  artifacts: readonly ArtifactView[]
  tasks: readonly TaskItem[]
  completed: number
  total: number
  validation: ValidationView
  /**
   * The archive directory this was read out of, or `null` for a change still in flight. (M28)
   *
   * `openspec archive` moves a change to `openspec/changes/archive/<stamp>-<name>/` and no CLI
   * command reads it, so cide reads the directory itself — see `cide_spec::archived_change`. What
   * comes back is deliberately smaller than a live read, and **`completed`, `total` and
   * `validation` are meaningless here**: they are `0`, `0` and vacuously clean, because a
   * directory listing cannot answer them and a progress bar over finished work or a green verdict
   * nobody ran would each be a worse claim than the *"could not be read"* this replaced.
   *
   * Every function below that touches those three checks this first. The string is the folder's
   * own name, stamp included: the only place the archive date is recoverable, and what a user
   * needs to find the files.
   */
  archivedAs: string | null
}

/* ------------------------------------------------------------------------- the board */

/**
 * What the panel is looking at.
 *
 * **Five arms, and the extra two are the point.** Rust answers with three (`absent`, `unusable`,
 * `ready`); the frontend adds `unknown` and splits nothing else.
 *
 * `unknown` is *nobody has asked yet*, and it must never render as `absent`. "We have not looked"
 * and "there is nothing here" are different screens: the first draws nothing, the second draws a
 * pitch card with a button that writes a tracked directory into somebody's repository. Showing
 * the second while the first is true is offering to change a repo on the strength of not knowing.
 * `TasksPanel`'s `BOARD_UNKNOWN` exists for the same reason and its render check pins the pair.
 */
export type Board =
  | { kind: 'unknown' }
  | { kind: 'absent'; hint: string; path: string }
  | { kind: 'unusable'; reason: string }
  | {
      kind: 'ready'
      changes: readonly ChangeSummaryView[]
      specs: readonly CapabilityView[]
      root: string
      /** What this project types to run each of OpenSpec's commands — see `invocation`. */
      commands: readonly SpecCommandView[]
    }

/**
 * One of OpenSpec's workflow commands, as this project can actually invoke it.
 *
 * `name` is cide's handle (`propose`) and `line` is the whole thing to type
 * (`/openspec-propose`). Read from the project's own directory by `cide_spec::claude`, because
 * the invocation has changed under cide once already — see that module's header, and
 * `invocation` below for what the panel does with it.
 */
export type SpecCommandView = { name: string; line: string }

export const BOARD_UNKNOWN: Board = { kind: 'unknown' }

/** Can this board be written to — set up, or proposed into? */
export function canWrite(board: Board): boolean {
  return board.kind === 'absent' || board.kind === 'ready'
}

/**
 * The figure in the panel header, or `null` when there is nothing to count.
 *
 * `null` and never `'0'`: a header that says zero is a header claiming to have looked, and on an
 * `unknown` board nothing has. `metaFigure`'s rule one panel over.
 */
export function metaFigure(board: Board): string | null {
  if (board.kind !== 'ready') return null
  if (board.changes.length === 0 && board.specs.length === 0) return null
  return `${board.specs.length} spec${board.specs.length === 1 ? '' : 's'} · ${board.changes.length} change${board.changes.length === 1 ? '' : 's'}`
}

/**
 * The rail badge: how many changes are in flight, or `null` when nobody has looked.
 *
 * `null` and `0` are different answers and the rail draws neither — but they reach it as
 * different values, which is what lets a badge appear the moment the first board arrives rather
 * than one refresh later.
 */
export function openChangeCount(board: Board): number | null {
  if (board.kind !== 'ready') return null
  return board.changes.length
}

/**
 * Drop a board that is not newer than what is on screen.
 *
 * Returns `current` when the update should be ignored, so a caller can compare by identity. The
 * rule is narrower than `TasksPanel`'s `newerBoard` because `cide://spec-changed` carries no
 * revision — it does not need one, since `spec_state`'s coalescer runs one read at a time per
 * project — so the only drop here is the one that matters: a `ready` board must not be replaced
 * by an `unknown` one, which is what an unrelated project's event or a torn-down store would
 * otherwise do.
 */
export function newerBoard(current: Board, next: Board): Board {
  if (next.kind === 'unknown' && current.kind !== 'unknown') return current
  return next
}

/* -------------------------------------------------------------------------- the tree */

export type SpecRow =
  | { kind: 'section'; id: 'specs' | 'changes'; label: string; count: number; hint: string }
  | { kind: 'spec'; id: string; label: string; requirements: number }
  | {
      kind: 'change'
      id: string
      label: string
      stage: ChangeStage
      done: number
      total: number
      /**
       * The task that already tracks this change, or `null`. (M28)
       *
       * The row's action turns on it, and that is the whole reason the panel knows about tasks
       * at all: a change with nobody working it is a proposal, and the *only* way to start work
       * on one is to link it to a task and assign that task — assignment is what dispatches.
       * Without this, the panel listed changes and offered no way to act on any of them.
       */
      task: string | null
    }

/** The one-line explanation under each section heading. */
/**
 * What a section says when it has nothing in it.
 *
 * **Drawn only when the section is empty**, which is the whole of the sizing decision. These
 * shipped under every heading, always, and on a 320px panel that meant four wrapped lines of
 * explanation above two rows of content — a caption louder than the list it captions. A section
 * with rows in it is explained by its rows; a section with none has nothing else to say, so it
 * gets the long form and can afford to wrap.
 */
export function sectionHint(id: 'specs' | 'changes'): string {
  return id === 'specs'
    ? 'openspec/specs/ — the requirements as they stand. A change moves its edits here when it is archived.'
    : 'openspec/changes/ — a proposal saying why, a task list saying how, and the requirement edits it makes. Nothing here is merged into the specs yet.'
}

/**
 * The mark on a change row.
 *
 * **Not a pencil.** The row draws a change, and a pencil beside something that is not editable
 * reads as an affordance that is missing rather than as a category — the first version used
 * `opIcon('modified')` and every change in the tree looked like a broken Edit button. A change is
 * a proposed difference to the specs, which is what `file-diff` says.
 */
export const CHANGE_ICON = 'file-diff'

/**
 * A control's label, its tooltip, and — when it must be drawn inert — the sentence saying why.
 *
 * Extracted from `RowAction` when the change page grew a second button (`splitAction`), because
 * the two share every field except the one `RowAction` exists for. Not folded into one function
 * returning both: the sidebar's change row draws exactly *one* control — a 320px row must not
 * grow a second — so a two-action return would be a value one of the two callers structurally
 * throws away.
 */
export interface ActionOffer {
  label: string
  title: string
  /**
   * Set when the control must be drawn inert, and it is the sentence explaining why.
   *
   * A reason rather than a boolean, for `MenuItem`'s rule: a greyed control that does not say
   * what would un-grey it is indistinguishable from a broken one, which is the exact complaint
   * this field exists because of.
   */
  disabledReason?: string
}

export interface RowAction extends ActionOffer {
  id: 'start' | 'open'
}

/**
 * What the task tracker has said, restated so this module can reason about it.
 *
 * The four arms of `TasksPanel/model.ts`'s `Board`, by name and no more — that module is
 * import-free and so is this one, so the vocabulary is copied rather than shared. Copied and not
 * collapsed: the whole bug below is what collapsing it costs.
 */
export type TrackerKind = 'unknown' | 'absent' | 'unreadable' | 'ready'

/**
 * The action a change row offers.
 *
 * Two, and they are the same gesture at two moments: a change nobody is working needs a task
 * before anything can happen to it, and a change that has one needs a way back to it. Naming
 * them differently matters — *Start work* says what pressing it does, where an *Open task* on a
 * change with no task would be a button that creates something while claiming to open it.
 *
 * # Why the tracker's state is an argument
 *
 * Because `task === null` has two meanings and this used to print the wrong one. It means *this
 * change has no task* only when the board is `ready`; on `unknown` and `unreadable` it means
 * **nobody knows**, and the row was rendering that as a confident *Start work* — on a change
 * that already had a finished task. Pressing it then did **nothing at all**: `tasksStore.create`
 * gates on `canWrite`, which is `false` for both of those arms for reasons its own doc argues at
 * length, and returns silently. A label that lies and a control that is inert without saying so
 * are one bug, and this is where it is fixed, because `TasksPanel.tsx` already asks the same
 * question before drawing *New task* — one rule, now two callers, rather than one caller and one
 * surface that forgot.
 *
 * `absent` is *not* one of the refusing arms, exactly as `canWrite` has it: there is no
 * `.cide/tasks.json` yet and creating the first task is what writes one.
 */
export function rowAction(task: string | null, tracker: TrackerKind = 'ready'): RowAction {
  // The tracker's arm is asked **first**, before the id — a task id read off a board nobody has
  // read is not evidence that the task is there. In practice the host cannot produce one (the
  // map is built from the same board), and answering it here rather than relying on that is what
  // keeps the rule in the module the check can drive.
  if (task !== null && tracker === 'ready') {
    return {
      id: 'open',
      label: 'Open task',
      title: `Opens ${task}, which tracks this change.`,
    }
  }
  const start: RowAction = {
    id: 'start',
    label: 'Start work',
    title:
      'Creates a task for this change and opens it. Assigning that task is what starts an agent on the checklist — approving a change is assigning it.',
  }
  if (tracker === 'unknown') {
    return {
      ...start,
      disabledReason:
        'Still reading this project’s task board. Until it answers, cide cannot tell whether this change already has a task.',
    }
  }
  if (tracker === 'unreadable') {
    return {
      ...start,
      disabledReason:
        '.cide/tasks.json could not be read, so cide cannot tell whether this change already has a task — and will not write over a tracker it could not parse. Open the Tasks panel to see what is wrong with it.',
    }
  }
  return start
}

/**
 * What the change page's *Split work* button offers.
 *
 * The other half of [`rowAction`]'s gesture, and deliberately a different question. *Start work*
 * makes **one** task for a change, which is what a single agent needs; this asks the project's
 * Claude conversation to make a main task carrying the change link and one `subtaskOf` child per
 * unit of the checklist, so an orchestrator has pieces it can hand to different roles and so the
 * decomposition outlives the session that made it.
 *
 * # It takes no task id, because the page only draws it when there is no task
 *
 * A change gets exactly one task carrying `change` — `spec_triggers::consider_one` finds a
 * change's task by that field and, on more than one match, moves *neither* — so splitting is a
 * thing you do to a change nobody has started. Once a task exists the page shows `Open t-NN` and
 * this button is gone. Rust refuses the same case by name anyway, because a board the page read a
 * moment ago is not evidence.
 *
 * The tracker's arm is the whole argument: `rowAction`'s doc carries it, and it is the same one
 * here — a control that writes must not be live while nobody knows what is already on the board.
 * `absent` stays live, exactly as `canWrite` has it: there is no `.cide/tasks.json` yet and
 * writing the first task is what makes one.
 */
export function splitAction(tracker: TrackerKind = 'ready'): ActionOffer {
  const split: ActionOffer = {
    label: 'Split work',
    title:
      'Asks this project’s Claude conversation to turn the change’s checklist into a main task with one subtask per unit of work, say which role should take each, and ask you before anything starts.',
  }
  if (tracker === 'unknown') {
    return {
      ...split,
      disabledReason:
        'Still reading this project’s task board. Until it answers, cide cannot tell whether this change already has a task.',
    }
  }
  if (tracker === 'unreadable') {
    return {
      ...split,
      disabledReason:
        '.cide/tasks.json could not be read, so cide will not write tasks into a tracker it could not parse. Open the Tasks panel to see what is wrong with it.',
    }
  }
  return split
}

/**
 * The tree the panel draws.
 *
 * `tasks` maps a change name to the task tracking it, so a row can offer the right action. It is
 * a plain lookup rather than a search per row: the board can hold as many changes as the project
 * has, and a scan per row is the shape that stops being free without anybody noticing.
 */
export function specRows(
  board: Board,
  expanded: Readonly<Record<string, boolean>>,
  tasks: Readonly<Record<string, string>> = {},
): SpecRow[] {
  // Specs alphabetically — a stable order somebody can learn. Changes by *remaining work*
  // ascending, so the one closest to being acceptable is at the top, then by name; a
  // recently-modified order was tried and rejected, because it reshuffles under an agent that is
  // working and the row you were reading moves.
  if (board.kind !== 'ready') return []
  const rows: SpecRow[] = []

  rows.push({
    kind: 'section',
    id: 'changes',
    label: 'Changes',
    count: board.changes.length,
    hint: sectionHint('changes'),
  })
  if (expanded['changes'] !== false) {
    const changes = board.changes.slice().sort((a, b) => {
      const left = a.total - a.completed
      const right = b.total - b.completed
      if (left !== right) return left - right
      return a.name.localeCompare(b.name)
    })
    for (const change of changes) {
      rows.push({
        kind: 'change',
        id: change.name,
        label: change.name,
        stage: summaryStage(change),
        done: change.completed,
        total: change.total,
        // `?? null` and not `Object.hasOwn`: a prototype key would otherwise hand back a
        // function, which React refuses as a child and `className` stringifies into the source
        // of `Object`. `check-problems.mjs` exists partly because that happened.
        task: Object.hasOwn(tasks, change.name) ? (tasks[change.name] ?? null) : null,
      })
    }
  }

  rows.push({
    kind: 'section',
    id: 'specs',
    label: 'Specs',
    count: board.specs.length,
    hint: sectionHint('specs'),
  })
  if (expanded['specs'] !== false) {
    const specs = board.specs.slice().sort((a, b) => a.id.localeCompare(b.id))
    for (const spec of specs) {
      rows.push({
        kind: 'spec',
        id: spec.id,
        label: spec.id,
        requirements: spec.requirements,
      })
    }
  }
  return rows
}

/**
 * A summary row's stage, from the two numbers alone.
 *
 * A summary carries no verdict — validating every change to draw a list would be a subprocess per
 * row — so `invalid` is not reachable here. It appears on the *card*, which has one.
 */
export function summaryStage(change: ChangeSummaryView): ChangeStage {
  if (change.total > 0 && change.completed >= change.total) return 'ready'
  if (change.completed > 0) return 'inProgress'
  return 'proposed'
}

/**
 * A whole change's stage.
 *
 * **`total === 0` is `proposed`, never `ready`.** A change whose task list exists and holds no
 * checkboxes has not been planned yet, and reading `0/0` as finished is what would offer to
 * archive it. The same rule guards the Review hop in Rust; both sides state it because both sides
 * would be wrong in the same way without it.
 */
export function stageOf(change: ChangeView): ChangeStage {
  // First, and before anything reads the three fields an archived change does not carry: it is
  // `0/0` and vacuously valid, which every branch below would misread — as `proposed`, which is
  // the *opposite* of what happened to it. See `ChangeView.archivedAs`.
  //
  // `!= null` and never `!== null`: this is a wire value, and a webview newer than its binary
  // sees `undefined` here. Read strictly, that made every live change archived. See `adapt.ts`.
  if (change.archivedAs != null) return 'archived'
  if (!change.validation.valid) return 'invalid'
  if (change.total > 0 && change.completed >= change.total) return 'ready'
  if (change.completed > 0) return 'inProgress'
  return 'proposed'
}

/** `{done, total, pct}`, with `pct` clamped and safe at `total === 0`. */
export function taskProgress(change: ChangeView): { done: number; total: number; pct: number } {
  const total = Math.max(0, change.total)
  const done = Math.min(Math.max(0, change.completed), total)
  return { done, total, pct: total === 0 ? 0 : Math.round((done / total) * 100) }
}

/* ----------------------------------------------------------------- artifacts, sorted */

/**
 * The schema ids of the two artifacts the page treats specially.
 *
 * **Ids, not filenames**, and the distinction is the whole reason these are safe to name. The
 * artifact set is schema-driven — `openspec/config.yaml` picks a workflow schema whose artifacts
 * declare `generates` globs — so `proposal.md` is a default and not a guarantee, which is why
 * nothing anywhere in this feature writes that string. The *id* is the schema's own vocabulary:
 * it is what `applyRequires` and `nextSteps` name, and what `status --json` keys its
 * `artifactPaths` by.
 *
 * A schema that declares neither costs nothing: [`proposalArtifact`] answers `null` and the page
 * draws what it drew before, with every artifact in Documents and no prose block.
 */
export const PROPOSAL_ARTIFACT = 'proposal'
export const DESIGN_ARTIFACT = 'design'

/**
 * The proposal, when there is one to read.
 *
 * `null` when the schema declares no `proposal` **or** when it declares one that has not been
 * written — the second is the case that matters, because that artifact still belongs in Documents
 * saying "not written yet". Only a proposal with a file is lifted out of the list and drawn as
 * prose.
 */
export function proposalArtifact(change: ChangeView): ArtifactView | null {
  const found = change.artifacts.find(
    (artifact) => artifact.id === PROPOSAL_ARTIFACT && artifact.existing.length > 0,
  )
  return found ?? null
}

/**
 * What the Documents list draws: everything except a proposal that is drawn in full below it.
 *
 * The proposal is the one document a reader opens the page *for* — it is the change, in prose —
 * and a link to it under a heading called Documents made the page an index of a directory rather
 * than an answer. Every other artifact stays a link, deliberately: `tasks.md` is drawn as the
 * checklist, the delta specs are drawn as requirement cards, and `design.md` is the one thing
 * here that is genuinely a separate document.
 *
 * `proseShown` is what keeps the removal honest. Its text is a **second read**, so between the
 * change arriving and the file arriving there is a render where the prose is not on screen yet —
 * and dropping the row in that window would take the proposal off the page entirely for a beat.
 * The row leaves the list only once the thing that replaces it is there.
 */
export function documentArtifacts(
  change: ChangeView,
  proseShown: boolean,
): readonly ArtifactView[] {
  const proposal = proposalArtifact(change)
  if (proposal === null || !proseShown) return change.artifacts
  return change.artifacts.filter((artifact) => artifact !== proposal)
}

/**
 * Does this change have a design document?
 *
 * Drawn as a marker on the page, and it is not decoration. `design.md` is optional in the
 * `spec-driven` schema and `propose` does not write one: a change that has one is a change
 * somebody decided needed an argument written down before it could be implemented, and that is
 * the single cheapest signal on the page that this is **not a trivial change**. Without a marker
 * the only way to learn it was to notice one extra row in a list of file links.
 */
export function hasDesign(change: ChangeView): boolean {
  return change.artifacts.some(
    (artifact) => artifact.id === DESIGN_ARTIFACT && artifact.existing.length > 0,
  )
}

/** The marker's words. One place, so the page and its check cannot disagree. */
export const DESIGN_LABEL = 'Has design'
export const DESIGN_TITLE =
  'This change carries a design document, which is written only when a change needs an argument ' +
  'settled before it is implemented. Read it before starting: it is not a trivial change.'

/** The heading over the proposal's prose. */
export const PROPOSAL_TITLE = 'Proposal'

/* ------------------------------------------------------------------ validation, shown */

/**
 * The badge beside a change: three states, never two.
 *
 * `unchecked` must never render as `ok`. "We have not validated this" and "this is valid" are
 * different claims, and collapsing them is how a panel comes to assert something it never
 * checked — the failure this codebase writes guards against everywhere else.
 */
export type ValidationBadge = { state: 'unchecked' | 'ok' | 'failed'; label: string }

export function validateBadge(
  validation: ValidationView | null,
  /**
   * `true` when the change was read out of the archive. (M28)
   *
   * Its `validation` is vacuously clean — nothing validated it, and nothing can: `openspec
   * validate` resolves `openspec/changes/<name>/` and an archived change is not there. Printing
   * *Valid* off that would be the exact claim this badge's three states exist to prevent, so the
   * archived answer is `unchecked` with a label saying why.
   */
  archived = false,
): ValidationBadge {
  if (archived) return { state: 'unchecked', label: 'Archived' }
  if (validation === null) return { state: 'unchecked', label: 'Not checked' }
  const blocking = validation.issues.filter(isBlocking)
  if (blocking.length === 0 && validation.valid) return { state: 'ok', label: 'Valid' }
  const n = blocking.length
  return { state: 'failed', label: n === 1 ? '1 issue' : `${n} issues` }
}

/**
 * Does this issue make a change invalid?
 *
 * Case-insensitive, because the level is upstream's own string: a comparison that assumed its
 * case would silently classify every issue as non-blocking the day it changed, and the panel
 * would go green over a broken file.
 */
export function isBlocking(issue: IssueView): boolean {
  return issue.level.toLowerCase() === 'error'
}

/**
 * The issues that belong beside one requirement card.
 *
 * Matched on the requirement's name appearing in the issue's `path`, which is how the validator
 * addresses them. Anything that matches nothing is not dropped — see [`unattributedIssues`].
 */
export function issuesFor(validation: ValidationView | null, requirement: string): IssueView[] {
  if (validation === null || requirement.trim() === '') return []
  return validation.issues.filter((issue) => issue.path.includes(requirement))
}

/**
 * The issues no card claimed, for the banner.
 *
 * The pair with [`issuesFor`] is the point: **every issue is drawn exactly once**. An issue whose
 * path names no requirement — a missing `## Why`, a delta with no capability — would otherwise be
 * reported by the validator, carried across the wire, and rendered nowhere at all.
 */
export function unattributedIssues(
  validation: ValidationView | null,
  requirements: readonly string[],
): IssueView[] {
  if (validation === null) return []
  return validation.issues.filter(
    (issue) => !requirements.some((name) => name.trim() !== '' && issue.path.includes(name)),
  )
}

/* ------------------------------------------------------------------ the primary action */

/**
 * Whether a control may be pressed, and if not, why in words.
 *
 * A greyed control with no sentence is a dead control in grey. `cide_core::commands`' rule about
 * a listed-and-silently-inert command, applied to a button: every refusal names the next action.
 */
export type Gate = { ok: true } | { ok: false; reason: string }

export interface PrimaryAction {
  id: 'approve' | 'accept' | 'none'
  label: string
  hint: string
  gate: Gate
}

/**
 * The one prominent thing to do about this change, and the line under it.
 *
 * One action per state, with its hint, because the hints *are* the onboarding: a user who has
 * never read OpenSpec's docs learns what a change, a delta and an archive are from these lines at
 * the moment each one matters.
 */
export function primaryAction(
  change: ChangeView,
  assignee: string | null,
  taskStatus: string | null,
  /**
   * The conversation this task's work was handed to, or `null`. (M28)
   *
   * A **fourth** parameter and not folded into `assignee`, because they are different facts with
   * different id spaces — a role name against the uuid cide passed to `claude --session-id` — and
   * Rust keeps them in different fields for the reason that makes the whole picker safe: a task
   * cannot claim both, so choosing a conversation is structurally incapable of also spawning a
   * subagent.
   */
  session: string | null,
  /**
   * Does the assigned role run in its own checkout? `null` for no role, or no roster yet.
   *
   * A **fifth** parameter, and the one that decides whether the accept gesture merges anything
   * at all. `spec_accept` merges only when the task names a role *and* `plan_accept` found a
   * worktree registration for it; everything else archives and nothing more. Two shapes reach
   * that second state, and both are ordinary:
   *
   * * the work went to a Claude **conversation** — `TaskEdit::SetSession` clears `Task::agent`,
   *   so there is no `cide/<role>-<task>` by construction;
   * * the role declares `worktree: false` and commits straight into the checked-out tree.
   *
   * In both, the button used to read *Integrate & Archive* over a conversation that had already
   * committed everything onto the user's own branch — a step named and never taken, which is
   * `Command::unavailable`'s failure wearing a green button. It is a *fact* here rather than a
   * caller-computed "will it merge" for the reason `session` is carried beside `assignee`: two
   * callers deriving one answer independently is two chances to derive it differently, and the
   * card and the change page must not disagree about what the next step is.
   */
  worktree: boolean | null,
): PrimaryAction {
  // Read from the checklist and not from `stageOf`, because a change can be both finished and
  // invalid — and that combination is the one that matters most. Deriving the action from the
  // stage would send it down the `approve` road, offering to *start* work that is already done
  // and hiding the validation problem that is the only thing standing in the way.
  /*
   * An archived change first, before any of the three fields it does not carry is read.
   *
   * `0/0` and vacuously valid would take this down the `approve` road — offering to dispatch a
   * role at work that is finished and merged — which is the worst of the four answers and the one
   * a naive fallthrough gives. The task's own status cannot be relied on to catch it either: a
   * change archived by the agent, or by hand in a terminal, leaves the task wherever it was.
   */
  // `!= null`, catching `undefined` as well — and that is not defensive padding. Read as
  // `!== null` this branch swallowed **every** change the moment a webview outran its binary and
  // `origin` stopped arriving: no Approve & dispatch anywhere, and a line reading *this change
  // has been archived* under a change nobody had archived. `adapt.ts` carries the full account.
  if (change.archivedAs != null) {
    return {
      id: 'none',
      label: '',
      hint: `Archived as openspec/changes/archive/${change.archivedAs}/. Its requirements are in openspec/specs/ now.`,
      gate: { ok: false, reason: 'this change has been archived' },
    }
  }

  const { done, total } = taskProgress(change)
  const finished = total > 0 && done >= total

  if (taskStatus === 'done') {
    return {
      id: 'none',
      label: '',
      hint: 'Archived. Its requirements are in openspec/specs/ now.',
      gate: { ok: false, reason: 'this change has been accepted' },
    }
  }

  if (finished) {
    const blocking = change.validation.issues.filter(isBlocking).length
    const gate: Gate =
      change.validation.valid && blocking === 0
        ? { ok: true }
        : {
            ok: false,
            reason:
              blocking === 1
                ? 'openspec validate found 1 problem in this change. Fix it above first.'
                : `openspec validate found ${blocking} problems in this change. Fix them above first.`,
          }
    /*
     * Whether the press will merge anything, which is what the button is allowed to say.
     *
     * `worktree !== false` and not `=== true`: `null` is *no roster read yet*, and a role that
     * exists almost always has a checkout, so the unknown case keeps the name of the step it is
     * about to take rather than promising less than it does. The reverse default would print
     * *Archive* over a run whose branch is about to be merged, which is the more damaging lie —
     * the user would have no reason to look for the merge commit.
     */
    const integrates = assignee !== null && assignee.trim() !== '' && worktree !== false
    return {
      id: 'accept',
      label: integrates ? 'Integrate & Archive' : 'Archive',
      hint: integrates
        ? "Merges the agent's branch, then merges these requirement edits into openspec/specs/ and closes the task."
        : 'Merges these requirement edits into openspec/specs/ and closes the task. This work was done in your own checkout, so there is no branch to merge.',
      gate,
    }
  }

  if (done > 0) {
    return {
      id: 'none',
      label: '',
      hint: 'The agent ticks these off in the task list as it works.',
      gate: { ok: false, reason: `${total - done} of ${total} steps are still open.` },
    }
  }

  /*
   * Nothing ticked yet: the gesture is to approve it, and approving *is* assigning.
   *
   * **Which is why there is no precondition here.** This gate used to refuse until an assignee
   * had been picked — correct while the assignee row was the only way to start work, and exactly
   * backwards once the button became the thing that picks one. Together with the row being
   * hidden on an unapproved change it made a deadlock: a greyed button reading "pick an assignee
   * first", above no way to pick one. The button opens the picker; the picker is where the
   * choice between a role, an open conversation and a new one is made.
   *
   * `assignee` stays a parameter because the *hint* still reads differently once something is
   * assigned — a change that already has a role can be re-approved onto another target, and the
   * line should not go on saying nothing has been chosen.
   */
  /*
   * Already handed to a conversation: there is nothing prominent left to press.
   *
   * This branch is the one the shipped build was missing, and the symptom was exact: choosing
   * *New Claude session* started the session, typed the task in, and left the card still offering
   * **Approve & dispatch** — a button whose whole job had just been done, sitting over a
   * conversation that was already working. Pressing it again would have opened the picker and
   * invited a second dispatch of the same task.
   *
   * The reason it was missed is that the card only ever saw `assignee`, and `TaskStore::edit`
   * *clears* `Task::agent` when a session is set. So the one gesture that hands work to a
   * conversation left every field this function reads exactly as it found them.
   *
   * What replaces the button is the session row, which says whether that conversation is working
   * or waiting and offers the way in — and the way to hand it somewhere else, which is why
   * removing the button does not trap anybody.
   */
  if (session !== null && session.trim() !== '') {
    return {
      id: 'none',
      label: '',
      hint: 'This change’s work is with a conversation. Open it to see where it has got to.',
      // A whole sentence, because the *change page* prints this reason where the card prints the
      // session row: `SpecTabView` draws `gate.reason` under its actions and has no row of its
      // own, so a fragment there would be the page's only word on the subject.
      gate: {
        ok: false,
        reason:
          'Its work is with a Claude conversation — open this change’s task to see where it has got to.',
      },
    }
  }

  const assigned = assignee !== null && assignee.trim() !== ''
  return {
    id: 'approve',
    label: assigned ? 'Dispatch again' : 'Approve & dispatch',
    hint: assigned
      ? 'Sends this change’s work somewhere — a role, a conversation already open, or a new one.'
      : 'Choose where the work happens. Approving a change is assigning it — the specs stay untouched until it is archived.',
    gate: { ok: true },
  }
}

/**
 * The sentence the accept confirmation prints.
 *
 * Names the capabilities and not just a count, `ConfirmDestructive`'s stated rule: the user is
 * about to act on specific files and "3 requirements" is not something anyone can check before
 * clicking.
 */
export function archivePreview(deltas: readonly DeltaView[]): string {
  if (deltas.length === 0) return 'This change edits no requirements.'
  const parts = deltas.map((delta) => {
    const n = delta.requirements.length
    const noun = n === 1 ? 'requirement' : 'requirements'
    return `${n} ${noun} ${opLabel(delta.op).toLowerCase()} in ${delta.spec}`
  })
  return `${parts.join(', ')}.`
}

/* ------------------------------------------------------------------------ finding text */

/** One run of a string, and whether it is part of a match. */
export interface Piece {
  text: string
  hit: boolean
}

/**
 * Split a string into matched and unmatched runs.
 *
 * # Why the page marks its own text instead of using the browser's find
 *
 * `ctrl+f` is deliberately **not** a global binding — `cide_core::keymap` has a test forbidding
 * it, because CodeMirror means one thing by it in an editor and a terminal means another, and a
 * window-capture gate would take it from both. Every surface that wants it handles it locally,
 * so this page does too.
 *
 * `window.find()` was the cheap alternative and loses: it searches the whole document, so a hit
 * in the sidebar or the tab strip counts, and there is no way to scope it to one pane. Marking
 * the text this page renders is the only way the count on screen can be a count *of this page*.
 *
 * Case-insensitive, because a reader looking for `acl` means `ACL` too, and a page whose search
 * missed the capitalised half of a document would be worse than no search.
 *
 * An empty or whitespace query matches nothing — deliberately, so opening the find bar does not
 * light up every character on the page before anything has been typed.
 */
export function splitMatches(text: string, query: string): Piece[] {
  const needle = query.trim().toLowerCase()
  if (needle === '' || text === '') return [{ text, hit: false }]

  const pieces: Piece[] = []
  const hay = text.toLowerCase()
  let at = 0
  for (;;) {
    const found = hay.indexOf(needle, at)
    if (found < 0) break
    if (found > at) pieces.push({ text: text.slice(at, found), hit: false })
    pieces.push({ text: text.slice(found, found + needle.length), hit: true })
    at = found + needle.length
  }
  if (at < text.length) pieces.push({ text: text.slice(at), hit: false })
  return pieces.length === 0 ? [{ text, hit: false }] : pieces
}

/** How many times `query` occurs in `text`. Counted the same way [`splitMatches`] splits. */
export function countMatches(text: string, query: string): number {
  return splitMatches(text, query).filter((piece) => piece.hit).length
}

/**
 * Step `index` by `delta` within `total`, wrapping at both ends.
 *
 * Wrapping rather than clamping: a find bar that stops at the last hit makes the user work out
 * how many there were, and every find bar anybody has used wraps. `total === 0` answers 0 rather
 * than a negative index — pressing next with nothing found must be a no-op, not a crash.
 */
export function stepMatch(index: number, delta: number, total: number): number {
  if (total <= 0) return 0
  return (((index + delta) % total) + total) % total
}

/* ---------------------------------------------------------------- the row context menu */

/** What a right-click on a row can do. Ids, so the check can assert on them by name. */
export type RowMenuActionId = 'open' | 'validate' | 'archive'

/**
 * One entry in a row's right-click menu.
 *
 * Named `RowMenuAction` and not `RowAction`: that name is already the row's own inline button
 * (*Start work* / *Open task*), and the two are different things — one is a control the row
 * draws, the other is a list it offers on demand.
 */
export interface RowMenuAction {
  id: RowMenuActionId
  label: string
  /** Present ⇒ the item is disabled, and this is the sentence shown in its place. */
  disabledReason?: string
  danger?: boolean
}

/**
 * The menu for one change row.
 *
 * # Why Archive is offered from here at all
 *
 * The accept gesture was built on the task card, because that is where the review conversation
 * is. That left a change proposed straight from the pinned session — the ordinary road, not the
 * exceptional one — with **no route to archive anywhere in the UI**. A panel that shows a change
 * through its whole life and then cannot finish it is not a frontend to the format.
 *
 * # Why the checklist gate is computed here and validation is not
 *
 * A menu is built synchronously, at the moment of the right-click, and the board already knows
 * the two numbers. Validation does not live on a summary row — it costs a subprocess per change —
 * so it is checked **server-side** when the item is pressed, and a refusal comes back as a
 * sentence to show. The split is deliberate: the cheap gate greys the item with a reason before
 * anything runs, and the expensive one cannot be skipped just because the menu did not know.
 */
export function changeRowActions(row: { done: number; total: number }): RowMenuAction[] {
  const remaining = row.total - row.done
  // `total === 0` is a change whose task list has not been written yet — not a finished one. The
  // same rule guards the Review hop in Rust; both sides state it because both would otherwise be
  // wrong in the same direction.
  const unfinished =
    row.total === 0
      ? 'This change has no task list yet, so nothing says the work is done.'
      : remaining > 0
        ? `${remaining} of ${row.total} steps are still unticked. Archiving now would put behaviour into openspec/specs/ that nobody has implemented.`
        : null

  return [
    { id: 'open', label: 'Open change' },
    { id: 'validate', label: 'Validate' },
    {
      id: 'archive',
      label: 'Archive change…',
      ...(unfinished === null ? {} : { disabledReason: unfinished }),
    },
  ]
}

/** The menu for one capability row. */
export function specRowActions(): RowMenuAction[] {
  return [{ id: 'open', label: 'Open spec' }]
}

/* --------------------------------------------------------------------- what to print */

export const ABSENT_CLAIM = 'No OpenSpec in this project.'
export const SETUP_LABEL = 'Set up OpenSpec'
/**
 * The button's words while `openspec init` runs — a present participle, `busyLabel`'s rule and
 * the config wizard's own wording.
 *
 * It exists because the button shipped with only `disabled`: init is two node subprocesses and
 * takes seconds, and for all of them a slightly dimmer *Set up OpenSpec* was the whole account
 * of the click. The only available reading was that the press had missed, and pressing again is
 * exactly what `busy` exists to prevent.
 */
export const SETUP_BUSY_LABEL = 'Setting up…'
export const SETUP_TITLE =
  'Runs `openspec init` here. Adds an openspec/ folder to this repository, and OpenSpec’s own workflow commands to this project’s Claude Code. The project’s Claude conversation then restarts — keeping its transcript — so those commands are known to it.'

/**
 * What to say once Set up has landed, per how the console reload went.
 *
 * # Why the reload happens at all
 *
 * `openspec init --tools claude` writes `.claude/skills/openspec-<name>/SKILL.md`, and Claude
 * Code reads a project's skills **once, at startup** — so the conversation already open in this
 * project cannot know them, and the very next press of *Propose* is refused by
 * `spec_run_command` with a sentence about restarting the pane. This shipped as exactly that
 * sentence shown here as a notice, and the report on it was the obvious question: cide is the
 * thing that knows which pane and owns its respawn, so why is the user the one doing it? The
 * console pane is restarted by `reloadConsole` now, and these are the words for each way that
 * can go.
 *
 * A function over a closed union rather than three constants, so a caller cannot invent a
 * fourth outcome with no sentence — `busyLabel`'s shape, for its reason.
 */
export function setUpNotice(reload: 'resumed' | 'restarted' | 'unreachable'): string {
  switch (reload) {
    case 'resumed':
      // The conversation is kept — `claude --resume` re-reads the skills and the transcript
      // both — and the sentence says so, because "restarted" alone reads as "your conversation
      // is gone" to the person who just watched their pane clear and redraw.
      return 'OpenSpec is set up. The project’s Claude conversation was restarted — resuming where it was — so its commands are available.'
    case 'restarted':
      return 'OpenSpec is set up. The project’s Claude conversation was restarted, so its commands are available.'
    default:
      // No pane this window can respawn — a detached console, whose restarter lives in another
      // window's realm. The manual sentence is the fallback, not the feature.
      return 'OpenSpec is set up. Restart this project’s Claude conversation before using its commands — Claude Code reads a project’s skills when it launches, so the one that is open does not have them yet.'
  }
}

/**
 * When the init landed and only the respawn failed. Said rather than swallowed, because "set
 * up" alone would leave the next *Propose* refusing for a reason the user was never told.
 */
export const SETUP_RELOAD_FAILED =
  'OpenSpec is set up, but restarting the project’s Claude conversation failed — restart it yourself (the pane’s Restart, or *Resume Claude session* in the palette) before using its commands.'
/**
 * The two entry points an empty board offers, and the commands behind them.
 *
 * **`propose` and `explore`, because those are what OpenSpec's default profile installs.** The
 * first version of this offered *Walk me through it* and typed `onboard`, which exists in the
 * CLI's templates and is not installed by the profile `openspec init` uses — so Claude answered
 * `Unknown command` and the button looked broken with nothing in cide able to say why. Rust
 * checks the project's own directory before typing anything now, so a profile that lacks one of
 * these refuses with a sentence naming what it does have.
 *
 * These are cide's *handles*, never the invocation: which prefix a project types is a fact about
 * its directory and is carried on the board. See `invocation`.
 *
 * Two rather than one because they answer different questions. *Propose* is the first action on
 * a board with nothing on it; *Explore* is for somebody who does not yet know what they want,
 * which is the more common state and the one OpenSpec exists to serve.
 */
/**
 * What the composer asks for, per command.
 *
 * Both commands take free text and say so in their own files: *"the argument after propose is
 * the change name (kebab-case), OR a description of what the user wants to build"*, and
 * explore's is *"whatever the user wants to think about"*. Sending either bare is legal and
 * useless — Claude simply asks what to propose, which is a round trip the panel already had the
 * user's attention for.
 */
/**
 * The dialog's heading, and the accessible name of the modal that holds it.
 *
 * cide's own words for what the command is *for*, not the invocation — that is drawn separately,
 * verbatim, off the project's board. A dialog whose title bar was `/openspec-propose` would name
 * the mechanism where the reader needs the intent.
 */
export function askTitle(command: string): string {
  return command === EXPLORE_COMMAND
    ? 'Explore before proposing'
    : 'Propose an OpenSpec change'
}

export function askPlaceholder(command: string): string {
  if (command === EXPLORE_COMMAND) return 'What do you want to think through?'
  return 'What should this change do?'
}

/**
 * What this project types to run `command` — the whole line, prefix and all.
 *
 * # Why this is a lookup and not a template
 *
 * Because the template was wrong for a year. OpenSpec installed its workflow as slash commands
 * (`/opsx:propose`) and then moved it to Claude Code skills (`/openspec-propose`); cide spelled
 * the old prefix out in nine places, so every button in this panel refused on every correctly
 * set up project. The invocation is now read from the project's directory by
 * `cide_spec::claude` and carried on the board, and this is the only place the panel spells one.
 *
 * # The fallback, and why it is safe to guess here
 *
 * A board that lists no such command still has to render *something* above the composer, and the
 * current spelling is the honest guess. It is a guess that cannot be typed: `spec_run_command`
 * resolves the name against the directory itself and refuses with a sentence naming what the
 * project does have, so a preview drawn from this fallback is never a preview of a line that
 * gets sent.
 */
export function invocation(board: Board, command: string): string {
  if (board.kind === 'ready') {
    const found = board.commands.find((entry) => entry.name === command)
    if (found !== undefined) return found.line
  }
  return `${CURRENT_PREFIX}${command}`
}

/**
 * The spelling a current `openspec init --tools claude` installs.
 *
 * Here for the fallback above and for nothing else — a project's real invocation always comes
 * off its board. Pinned by `check:openspec` against `cide_spec::claude`'s newest surface.
 */
export const CURRENT_PREFIX = '/openspec-'

/**
 * The line that will be typed, exactly.
 *
 * **One line, always.** It is typed into a PTY and terminated with Enter, so a newline in the
 * middle submits the first half as a turn and feeds the rest in as further turns — the failure
 * that looks like a model answering nonsense. Rust flattens it again on the way through, which is
 * the guard that counts; this one exists so the panel can *show* what it is about to send, and
 * showing something different from what is sent would be its own small lie.
 */
export function commandLine(invocation: string, text: string): string {
  const flat = text.split(/\s+/).filter((word) => word !== '').join(' ')
  return flat === '' ? invocation : `${invocation} ${flat}`
}

/** May this be sent? Empty text is allowed for `explore`, which is a mode, not a request. */
export function canAsk(command: string, text: string): boolean {
  if (command === EXPLORE_COMMAND) return true
  return text.trim() !== ''
}

export const ASK_SEND = 'Send'
export const ASK_CANCEL = 'Cancel'

export const PROPOSE_LABEL = 'Propose a change'
export const PROPOSE_COMMAND = 'propose'
export const EXPLORE_LABEL = 'Explore first'
export const EXPLORE_COMMAND = 'explore'

/**
 * The tooltip, naming the line *this* project would type.
 *
 * A function rather than two constants because the invocation is a fact about the directory —
 * the tooltip said `/opsx:propose` on projects where that command does not exist, which is the
 * bug this panel shipped with. See `invocation`.
 */
export function commandTitle(board: Board, command: string): string {
  const line = invocation(board, command)
  if (command === EXPLORE_COMMAND) {
    return `Types ${line} into this project’s Claude tab — think a problem through before anything is written down. Nothing is created until you propose.`
  }
  return `Types ${line} into this project’s Claude tab, which drafts a change’s proposal, its task list and its requirement edits in one step.`
}
export const CLI_MISSING_CLAIM = 'The openspec command line tool is not installed.'
export const CLI_INSTALL = 'npm install -g @fission-ai/openspec'
/*
 * `EMPTY_CHANGES_CLAIM` and `EMPTY_CHANGES_DETAIL` used to be here, for a "no changes yet" screen
 * that also held the Propose and Explore buttons — which is why those buttons disappeared the
 * moment the first change arrived. The screen is gone: the actions live in the toolbar, always,
 * and what a change *is* is now `sectionHint`'s job, drawn under the Changes heading exactly when
 * there is nothing else in it.
 */
/**
 * The header gear's words.
 *
 * "Configuration" and not "Settings", deliberately: cide's Settings is a global surface and this
 * is one committed file in one project. A control here labelled *Settings* would re-assert
 * exactly the thing moving it off that surface was meant to stop saying.
 */
export const CONFIGURE_LABEL = 'OpenSpec configuration'
export const CONFIGURE_TITLE =
  'Edit this project’s openspec/config.yaml — the workflow schema, the project context injected ' +
  'into every artifact-generation prompt, and the per-artifact rules. It is a file in this ' +
  'repository, not a cide preference.'

export const RETRY_LABEL = 'Check again'
