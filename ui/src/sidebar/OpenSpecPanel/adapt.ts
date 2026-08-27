/**
 * The wire → view seam for the OpenSpec panel. (M28)
 *
 * The **only** file in this directory allowed to import both `@/ipc/client`'s generated DTOs and
 * `./model`'s restatements of them. `model.ts` is import-free so `check:openspec` can compile it
 * standalone, which means nothing in it can reference a generated type; this module is where the
 * two are pinned together, and `tsc --noEmit` over it is what makes a field renamed in Rust a
 * build failure rather than a panel that silently draws nothing.
 *
 * Built **field by field, never with `as`**. A cast here would be the seam agreeing to whatever
 * the other side became, which is the one thing a seam must not do.
 */
import type {
  ChangeSummary as WireChangeSummary,
  SpecBoard as WireBoard,
  SpecChange as WireChange,
  SpecDelta as WireDelta,
  SpecIssue as WireIssue,
  SpecRequirement as WireRequirement,
  SpecSummary as WireSummary,
  SpecArtifact as WireArtifact,
} from '@/ipc/client'
import {
  BOARD_UNKNOWN,
  isDeltaOp,
  type ArtifactView,
  type Board,
  type CapabilityView,
  type ChangeSummaryView,
  type ChangeView,
  type DeltaOp,
  type DeltaView,
  type IssueView,
  type RequirementView,
  type ValidationView,
} from './model'

/**
 * A board off the wire, or [`BOARD_UNKNOWN`] when there is none.
 *
 * `null` — which is what `pendingCommand`'s fallback answers with while a command is not yet
 * registered — becomes `unknown` and **never `absent`**. The two are different screens, and the
 * absent one offers to write a tracked directory into somebody's repository: showing it because
 * a call has not answered yet would be proposing a commit on the strength of not knowing.
 */
export function adaptBoard(from: WireBoard | null): Board {
  if (from === null) return BOARD_UNKNOWN
  if (from.kind === 'absent') return { kind: 'absent', hint: from.hint, path: from.path }
  if (from.kind === 'unusable') return { kind: 'unusable', reason: from.reason }
  return {
    kind: 'ready',
    root: from.root,
    changes: from.changes.map(summary),
    specs: from.specs.map(capability),
    commands: from.commands.map((entry) => ({ name: entry.name, line: entry.line })),
  }
}

function summary(from: WireChangeSummary): ChangeSummaryView {
  return {
    name: from.name,
    completed: Number(from.completedTasks),
    total: Number(from.totalTasks),
    status: from.status,
  }
}

function capability(from: WireSummary): CapabilityView {
  return { id: from.id, requirements: Number(from.requirementCount) }
}

/** One change in full, or `null` when the call has not answered. */
export function adaptChange(from: WireChange | null): ChangeView | null {
  if (from === null) return null
  return {
    name: from.name,
    title: from.title,
    deltas: from.deltas.map(delta),
    artifacts: from.artifacts.map(artifact),
    tasks: from.progress.tasks.map((task) => ({
      done: task.done,
      description: task.description,
    })),
    completed: Number(from.progress.completed),
    total: Number(from.progress.total),
    validation: validation(from.validation.valid, from.validation.issues),
    /*
     * The only field that says the other three are not to be believed. See `ChangeView`.
     *
     * **`?.` and `?? null`, because `origin` is a promise from another process rather than a
     * fact.** A webview newer than the binary it is talking to gets a `SpecChange` with no
     * `origin` at all — which is not hypothetical here: Vite hot-reloads this file the moment it
     * is saved and the Rust side only changes on a relaunch, so every dev loop passes through
     * that state. Read as `from.origin.kind` it threw, the card's `.catch` turned the throw into
     * `specProblem`, and the whole spec block collapsed to one sentence; read as `!== null`
     * downstream, `undefined` would have made **every live change claim to be archived** —
     * removing Approve & dispatch from all of them. Both were reported as "we broke dispatch".
     *
     * An absent origin is `Active`, which is the honest reading: a change the CLI answered for is
     * one that is still in `openspec/changes/`.
     */
    archivedAs: from.origin?.kind === 'archived' ? (from.origin.folder ?? null) : null,
  }
}

function delta(from: WireDelta): DeltaView {
  return {
    spec: from.spec,
    // Guarded rather than cast: the wire type says this is one of four, and that is a promise
    // from another process rather than a fact. An operation this build has not heard of reads as
    // a change — see `opTone`'s note — instead of putting a value the tables cannot look up into
    // a `className`.
    op: op(from.operation),
    description: from.description,
    requirements: from.requirements.map(requirement),
    renamedFrom: from.rename?.from ?? null,
    renamedTo: from.rename?.to ?? null,
  }
}

function op(raw: string): DeltaOp {
  return isDeltaOp(raw) ? raw : 'modified'
}

function requirement(from: WireRequirement): RequirementView {
  return {
    name: from.name,
    text: from.text,
    scenarios: from.scenarios.map((scenario) => ({
      title: scenario.title,
      body: scenario.body,
    })),
    block: from.block,
  }
}

function artifact(from: WireArtifact): ArtifactView {
  return {
    id: from.id,
    generates: from.generates,
    state: from.state,
    existing: from.existing,
  }
}

function validation(valid: boolean, issues: readonly WireIssue[]): ValidationView {
  return { valid, issues: issues.map(issue) }
}

function issue(from: WireIssue): IssueView {
  return {
    level: from.level,
    path: from.path,
    message: from.message,
    // `u32` on the wire is a `number` here already, but the field is optional and an absent one
    // must be `null` rather than `undefined`: the model is compiled under
    // `exactOptionalPropertyTypes`, where those are not the same value.
    line: from.line ?? null,
  }
}
