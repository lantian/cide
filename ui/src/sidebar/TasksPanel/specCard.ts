/**
 * What the task card knows about its OpenSpec change. (M28)
 *
 * # Why this is its own module and not part of `TasksPanel/model.ts`
 *
 * Because that module is compiled standalone by `check:agents` and pinned, field by field,
 * against `crates/cide-ipc/src/tasks.rs`. Putting an OpenSpec vocabulary into it would put a
 * second wire contract inside a module whose whole job is to restate one — and the check that
 * reads `pub struct Task` would start seeing shapes that no Rust struct defines.
 *
 * This is import-free for the same reason `model.ts` is: `check:openspec` compiles it with a bare
 * `tsc` alongside the panel's own model.
 */

/** One `- [ ]` line, as the card draws it. */
export interface SpecTaskItem {
  done: boolean
  description: string
}

/** One artifact the change has on disk, and where it is. */
export interface SpecArtifactRef {
  id: string
  /** Absolute; the CLI resolved it, and cide never joins a filename of its own. */
  path: string
}

/**
 * A requirement draft, restated rather than imported.
 *
 * This module is compiled standalone alongside the panel's models, so it may not import
 * `../OpenSpecPanel/editModel` — not even for a type. `adapt`-style seams are where the two are
 * pinned together; here the shape is written out, and `tsc` over the components that use both is
 * what makes a divergence a build failure.
 */
export interface RequirementDraft {
  name: string
  text: string
  scenarios: { title: string; body: string }[]
}

/** One `#### Scenario:` section, with the title its header gives it. */
export interface SpecScenarioRef {
  title: string
  body: string
}

/** One requirement a delta carries. */
export interface SpecRequirementRef {
  /** `d0.r2` — the address a pencil sends back, and the one `editModel::parseTarget` reads. */
  target: string
  /** Validator complaints whose path names this requirement, drawn beside it. */
  issues: readonly string[]
  name: string
  text: string
  scenarios: readonly SpecScenarioRef[]
  /**
   * The whole block as it is in the file — what the editor opens on and what a save replaces.
   *
   * Never recomposed from the fields above. `openspec show --json` drops a requirement's header
   * and every scenario's title, so a block rebuilt from the wire would rename the requirement to
   * nothing and delete every title; and even with those recovered, recomposing reflows prose the
   * user never touched. `cide_spec::block::parse` reads all of it from the file instead.
   */
  block: string
}

/** One capability a change edits. */
export interface SpecDeltaRef {
  spec: string
  op: string
  requirements: readonly SpecRequirementRef[]
}

/**
 * Everything the card's spec block draws.
 *
 * A flat restatement rather than the panel's `ChangeView`, because the two are read by different
 * surfaces and a shared type would drag the panel's tree vocabulary into the card.
 */
export interface SpecCardView {
  change: string
  /** `null` while the change is being read — the chip draws, the numbers do not. */
  done: number | null
  total: number | null
  /** `null` means unchecked, which must never render as valid. */
  valid: boolean | null
  issues: number
  tasks: readonly SpecTaskItem[]
  deltas: readonly SpecDeltaRef[]
  artifacts: readonly SpecArtifactRef[]
  /** The one prominent action, already gated by `OpenSpecPanel/model.ts::primaryAction`. */
  action: SpecCardAction
  /**
   * The conversation this work was handed to, or `null` for a role, or for nothing yet. (M28)
   *
   * When this is set the primary action is `none` — `primaryAction` refuses the approve road on
   * a task that already has a session — so this row is what stands in its place: it says whether
   * that conversation is working or waiting, opens it, and offers the way to hand the task
   * somewhere else. Removing the button without it would have been a card with no way forward.
   */
  session: SpecSessionRef | null
  /**
   * The archive directory this was read out of, or `null` for a change still in flight. (M28)
   *
   * `openspec archive` moves the change and no CLI command reads the result, so cide reads the
   * directory — `cide_spec::archived_change`. **`done`, `total`, `valid` and `issues` mean
   * nothing on this arm**: they come back `0`, `0`, `true` and `0` because a directory listing
   * cannot answer them, which renders as a fresh unplanned proposal with a green tick — the
   * opposite of the truth in every part. `progressLabel` and `validityLabel` check this first,
   * and the card draws the archive line in place of the bar.
   *
   * The card said *"add-dark-mode could not be read; it may have been archived"* before this
   * existed, from the moment the work was accepted onwards.
   */
  archived: string | null
}

/**
 * The conversation a task's work was handed to, as the card draws it. (M28)
 *
 * `id` and `label` come off the task and the workspace tree; `open` and `awaiting` are looked up
 * at render and stored nowhere. That split is the point. `Task::session` is a **record of where
 * the work went** and deliberately survives the pane being closed — so whether a pane still holds
 * it is a fact about the workspace tree, and whether it is waiting is a fact about the live
 * session, and neither is something the tracker may cache. Two writers for one fact is how a card
 * comes to claim a conversation that closed an hour ago.
 */
export interface SpecSessionRef {
  /** The session id — the value cide passed to `claude --session-id`. */
  id: string
  /** What to call it: the conversation's own name, or the shortened id. */
  label: string
  /** Does a pane in this project still hold it? */
  open: boolean
  /** Is it waiting for the user? Only meaningful while `open`. */
  awaiting: boolean
}

/**
 * What the session row says, in four states.
 *
 * Four and not three, on `validityLabel`'s rule one function up, extended once more: *archived*
 * is not a quieter shade of any of the other three. A conversation the user closed is work that
 * has nowhere to continue, and drawing it as "working" would be the card asserting activity it
 * has no evidence for — while drawing it as "waiting for you" would send somebody looking for a
 * pane that is not there.
 *
 * # Why `archived` is asked **first**, before the pane is even looked at (M31)
 *
 * Because the other three are all claims about *this task's work*, and once the change is
 * archived there is no work: it is merged into `openspec/specs/` and the change directory has
 * moved. The card shipped drawing **Working** directly beneath its own *Archived* line, over a
 * conversation that had finished an hour before — and under it, *"It is working. The checklist
 * above ticks as it goes."* beside a card that draws no checklist at all on this arm, because
 * `archived` is exactly the state in which `done`/`total` mean nothing.
 *
 * The pane's own state is genuinely irrelevant here rather than merely less interesting, which
 * is why this is an override and not a fourth branch at the bottom. Whether a pane still holds
 * the conversation decides whether work can *continue* in it, and on archived work the only
 * question left is which conversation did it.
 */
export function sessionState(
  session: SpecSessionRef,
  archived: boolean,
): { state: string; label: string } {
  if (archived) return { state: 'archived', label: 'Did this work' }
  if (!session.open) return { state: 'closed', label: 'Conversation closed' }
  if (session.awaiting) return { state: 'awaiting', label: 'Waiting for you' }
  return { state: 'working', label: 'Working' }
}

/**
 * The sentence under the session row.
 *
 * A closed conversation gets the only one that names a next step, because it is the only one of
 * the three where the reader has a decision to make — and the sentence exists to say that the
 * decision is **not** "start again somewhere else". Its transcript is on disk under the id cide
 * passed to `--session-id`, so `claude --resume` brings all of it back; the row's two controls are
 * that spawn with and without a nudge, and this line is what tells them apart.
 */
export function sessionHint(session: SpecSessionRef, archived: boolean): string {
  /*
   * The archived arm names no next step, and that absence is the sentence's whole content.
   *
   * Every other hint here offers one — Resume, Reopen, hand it elsewhere — because every other
   * state is work that could continue. This one is the card saying there is nothing to carry
   * forward, which is what the row's two write controls are withdrawn for; a line that went on
   * describing how to hand the task back would be describing buttons that are no longer drawn.
   */
  if (archived) {
    return 'The work is finished and archived. Open reads what this conversation did.'
  }
  if (!session.open) {
    return 'No pane is showing it. Resume brings the whole transcript back with `claude --resume` and hands it this task again; Reopen brings it back without asking it for anything.'
  }
  return session.awaiting
    ? 'It has finished a turn and is waiting for you.'
    : 'It is working. The checklist above ticks as it goes.'
}

export interface SpecCardAction {
  id: 'approve' | 'accept' | 'none'
  label: string
  hint: string
  enabled: boolean
  /** Empty exactly when `enabled` — a greyed control with no sentence is a dead control. */
  reason: string
}

/**
 * What the primary button says while its call is in flight. (M28)
 *
 * A present participle, not the resting label with a spinner bolted on: *Integrate & Archive*
 * still reading *Integrate & Archive* is a button that looks pressable and is not, and the whole
 * reason this exists is that the card drew **nothing** while a branch merge, an
 * `openspec archive`, a re-validate and a status write ran — seconds in which the only available
 * reading was that the click had missed.
 *
 * Total over the three ids so a caller cannot reach a state with no label. `approve` opens a
 * picker and never waits, but a function that answers for two of three arms is one somebody
 * later calls with the third.
 */
export function busyLabel(action: 'approve' | 'accept' | 'none'): string {
  if (action === 'accept') return 'Integrating…'
  if (action === 'approve') return 'Dispatching…'
  return 'Working…'
}

/**
 * What the accept gesture actually did, as a notice. (M31)
 *
 * # Why this exists
 *
 * Because the press used to say **nothing at all**. `TaskDetailHost` did
 * `.then(() => setSpecBusy(false))` and threw the answer away — and `SpecAccepted` carries two
 * failures as ordinary `Ok` arms rather than rejections, so neither reached `notifyFailure`:
 *
 * * `refused` — the plan re-run at press time found a reason not to go ahead;
 * * `conflicts` — the merge refused, and **nothing was archived**, which is the ordering
 *   guarantee `spec_accept` exists to keep.
 *
 * A conflicted merge and a clean accept were therefore the same thing on screen: the spinner
 * stopped. That is the silent-failure class this codebase refuses everywhere else, and the
 * conflict is the one the user most needs told, because it leaves work to do.
 *
 * # Why it is here and not in the host
 *
 * So it can be tested. `check:openspec` compiles this module standalone, and the sentences a
 * failure path prints are only ever read by the person it is failing — the same argument
 * `ui/src/ipc/errorText.ts` is pinned under.
 *
 * The parameter is the shape of `cide_ipc::SpecAccepted` restated, on `RequirementDraft`'s rule
 * one type up: this module may import nothing, and `tsc` over `TaskDetailHost` — which passes
 * the generated type straight in — is what makes a divergence a build failure.
 */
export function acceptNotice(
  outcome:
    | { kind: 'accepted'; commit?: string | null; files: number; change: string }
    | { kind: 'refused'; plan: { refusals: readonly string[] } }
    | { kind: 'conflicts'; paths: readonly string[] },
): { kind: 'ok' | 'error'; text: string; detail?: string } {
  if (outcome.kind === 'conflicts') {
    return {
      kind: 'error',
      // Named, not counted, and the second sentence is the load-bearing one: the merge is the
      // *first* step, so a refusal there means `openspec/specs/` was never touched and the task
      // is still open. A message that only said "conflict" would leave a reader guessing which
      // half of the gesture had landed.
      text:
        outcome.paths.length === 1
          ? 'The merge conflicts in 1 file, so nothing was archived.'
          : `The merge conflicts in ${outcome.paths.length} files, so nothing was archived.`,
      detail: `${outcome.paths.join('\n')}\n\nResolve these on the branch, then try again. The change is untouched and the task is still open.`,
    }
  }
  if (outcome.kind === 'refused') {
    // Rust's own sentences, verbatim. Each one names its next action — `plan_change`'s rule —
    // and paraphrasing them here would be a second refusal table to keep in step by hand.
    return {
      kind: 'error',
      text: 'This change was not archived.',
      detail: outcome.plan.refusals.join('\n\n'),
    }
  }
  /*
   * Accepted. Two sentences, because `commit` absent is not a quieter shade of present.
   *
   * It is absent for a task whose work was done in the user's own checkout — a conversation, or
   * a `worktree: false` role — and for a branch that was already merged. Saying so is the point:
   * a message that named a merge that never happened is the same lie the button itself told
   * before `primaryAction` learnt to ask.
   */
  /*
   * `== null`, which catches **both** `null` and `undefined`, and the distinction crashed the
   * card. `SpecAccepted::Accepted::commit` is `Option<String>` with `#[ts(optional)]` and **no
   * `skip_serializing_if`** — so a merge that had nothing to merge does not omit the key, it
   * sends `"commit": null`. A check for `undefined` alone therefore fell through to
   * `outcome.commit.slice(0, 8)`, and pressing *Integrate & Archive* on a task whose work was
   * done in the user's own checkout threw `null is not an object` — after the archive had
   * already happened, so the gesture succeeded and reported a crash.
   *
   * `TasksPanel/adapt.ts` carries the same rule for `Task::session`, where it was the same bug.
   * `undefined` stays possible and still has to be caught: ts-rs renders the field optional, and
   * this module is compiled under `exactOptionalPropertyTypes` where the two are not one value.
   */
  if (outcome.commit == null || outcome.commit === '') {
    return {
      kind: 'ok',
      text: `${outcome.change} archived and the task closed. There was nothing to merge.`,
    }
  }
  const files = outcome.files === 1 ? '1 file' : `${outcome.files} files`
  return {
    kind: 'ok',
    text: `${outcome.change} archived. Merged ${files} at ${outcome.commit.slice(0, 8)}.`,
  }
}

/**
 * `3 / 9 steps`, or the sentence for a change whose checklist has not been read or planned.
 *
 * Three answers and not two. `null` is *not read yet*; `0/0` is *read, and nobody has written the
 * task list* — which is emphatically not "finished", and printing it as `0 / 0 steps` beside a
 * full progress bar is exactly the lie the Review hop's `total > 0` guard exists to prevent.
 */
export function progressLabel(spec: SpecCardView): string {
  // First: an archived change carries `0/0`, which reads as *no steps planned yet* — a sentence
  // about work that is finished and merged. See `SpecCardView.archived`.
  // `!= null`, `acceptNotice`'s rule one screen up and `adapt.ts`'s: a wire value read
  // strictly turns "the binary is older than this webview" into a confident wrong answer.
  if (spec.archived != null) return `archived as ${spec.archived}`
  if (spec.done === null || spec.total === null) return 'reading the checklist…'
  if (spec.total === 0) return 'no steps planned yet'
  return `${spec.done} / ${spec.total} steps`
}

/** The bar's fill, 0–100, and 0 whenever there is nothing to divide by. */
export function progressPercent(spec: SpecCardView): number {
  if (spec.archived != null) return 0
  if (spec.done === null || spec.total === null || spec.total === 0) return 0
  return Math.max(0, Math.min(100, Math.round((spec.done / spec.total) * 100)))
}

/** The validity badge's three states, in the card's own words. */
export function validityLabel(spec: SpecCardView): { state: string; label: string } {
  // An archived change is *vacuously* valid — nothing validated it and nothing can, because
  // `openspec validate` resolves `openspec/changes/<name>/` and it is not there. `unchecked` is
  // the honest state, and the pair of it with `ok` is what these three states exist for.
  if (spec.archived != null) return { state: 'unchecked', label: 'Archived' }
  if (spec.valid === null) return { state: 'unchecked', label: 'Not checked' }
  if (spec.valid && spec.issues === 0) return { state: 'ok', label: 'Valid' }
  return { state: 'failed', label: spec.issues === 1 ? '1 issue' : `${spec.issues} issues` }
}

/**
 * The artifact a section header opens, by the schema's id for it.
 *
 * `null` when the change has no such artifact, which is an ordinary state and not a failure: the
 * artifact set is decided by the workflow schema in `openspec/config.yaml`, so a project can
 * legitimately have no `design.md` — and a section that opened onto nothing would be a control
 * that does nothing, which is the state this codebase refuses everywhere else.
 */
export function artifactPath(spec: SpecCardView, id: string): string | null {
  const found = spec.artifacts.find((artifact) => artifact.id === id)
  return found === undefined ? null : found.path
}

/**
 * The requirement editor, as the card is handed it. (M28)
 *
 * A view of somebody else's state: `TaskDetailHost` owns the draft, the busy flag and the last
 * failure, and the card only draws what it is given. `target` says which requirement is open,
 * and a `null` draft is the same statement as the whole prop being absent — nothing is being
 * edited.
 */
export interface SpecEditView {
  target: string | null
  draft: RequirementDraft | null
  busy: boolean
  problem: { kind: 'regressed' | 'conflicted'; messages: readonly string[] } | null
  onDraft: (draft: RequirementDraft) => void
  onSave: () => void
  onCancel: () => void
}

/* --------------------------------------------------------- where a run will happen (M28) */

/**
 * One place the work can be sent.
 *
 * Three kinds, and the difference between the first and the other two is the whole reason this
 * type exists rather than a list of strings:
 *
 * * **`role`** — a definition in `.cide/agents/`. Choosing it *assigns the task*, and assigning
 *   is what `cide_agents::autodispatch` watches: the run starts because the assignee changed,
 *   through the same queue the panel's own Dispatch button uses. The card must therefore make
 *   **one** write and start nothing itself, or the task gets two runs.
 * * **`session`** — a Claude conversation already open in this project's pinned tab. Nothing
 *   about it is an assignment edge: `Task::session` is a different field and no trigger reads
 *   it, so this cannot dispatch a subagent by accident.
 * * **`fresh`** — the same, after making a new pane to hold it.
 *
 * That split is why "do not start it twice" is a property of the shape rather than a check.
 */
export type DispatchTargetKind = 'role' | 'session' | 'fresh'

export interface DispatchTarget {
  kind: DispatchTargetKind
  /** A role id, a session id, or `''` for the fresh one, which names nothing yet. */
  id: string
  label: string
  /** The line under it — a role's description, a conversation's own name. */
  detail: string
}

/** The label for the option that makes a new conversation. */
export const FRESH_TARGET_LABEL = 'New Claude session'
export const FRESH_TARGET_DETAIL = 'Adds a pane to this project’s Claude tab and sends the task to it.'

/**
 * Everything the picker offers, in the order it offers them.
 *
 * Roles first: a task with a checklist is work for something that will run unattended, and the
 * conversations below are for the case where the user wants to watch. `fresh` is last because it
 * costs a pane, and an option that changes the layout should not be the one the eye lands on.
 *
 * A session with no name of its own gets a positional one — *Conversation 2* — rather than its
 * uuid: the id is meaningless to a reader and identical-looking to every other id in the list.
 */
export function dispatchTargets(
  roles: Readonly<Record<string, string>>,
  descriptions: Readonly<Record<string, string>>,
  sessions: readonly { id: string; name: string | null }[],
): DispatchTarget[] {
  const targets: DispatchTarget[] = []
  for (const id of Object.keys(roles).sort()) {
    targets.push({
      kind: 'role',
      id,
      label: roles[id] ?? id,
      detail: descriptions[id] ?? 'Runs unattended in its own worktree.',
    })
  }
  sessions.forEach((session, index) => {
    targets.push({
      kind: 'session',
      id: session.id,
      label: session.name ?? `Conversation ${index + 1}`,
      detail: 'Already open — the task is typed into it.',
    })
  })
  targets.push({
    kind: 'fresh',
    id: '',
    label: FRESH_TARGET_LABEL,
    detail: FRESH_TARGET_DETAIL,
  })
  return targets
}

/**
 * A target's stable key, for a React list and for a `data-target` attribute.
 *
 * Kind-prefixed because a role and a session can never collide but a *reader* of the attribute
 * has to know which of the two roads to take — and that decision is the one that must not be got
 * wrong, since one of them dispatches through the queue and the other does not.
 */
export function targetKey(target: DispatchTarget): string {
  return `${target.kind}:${target.id}`
}

/** A `data-target` back into its parts, or `null` for anything that is not one. */
export function parseTargetKey(key: string): { kind: DispatchTargetKind; id: string } | null {
  const at = key.indexOf(':')
  if (at < 0) return null
  const kind = key.slice(0, at)
  if (kind !== 'role' && kind !== 'session' && kind !== 'fresh') return null
  return { kind, id: key.slice(at + 1) }
}

/**
 * Should the assignee row be drawn?
 *
 * Hidden on an OpenSpec task that has not been approved yet, because on those the assignee is
 * **not a field the user fills in** — it is what Approve & dispatch sets, and offering a second
 * road to the same fact would let somebody assign a role without ever seeing the choice between
 * a role and a conversation. Once something is assigned the row comes back, so it can be changed
 * the ordinary way.
 *
 * An ordinary task is unaffected: it has no Approve button, so the row is the only road there is.
 */
export function showsAssignee(spec: SpecCardView | null | undefined, assigned: boolean): boolean {
  if (spec === null || spec === undefined) return true
  return assigned
}
