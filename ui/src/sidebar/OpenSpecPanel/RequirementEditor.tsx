/**
 * Editing one requirement, in place. (M28)
 *
 * Pure: everything it needs arrives as a prop, so `check:openspec-render` can SSR it with fixed
 * fixtures. `editModel.ts` holds every decision it makes.
 *
 * # Why the scenario body is a textarea and not a pair of WHEN/THEN fields
 *
 * `editModel`'s header has the argument in full. In short: a scenario is free markdown, nothing
 * enforces two clauses, and structured fields would need a serialiser that round-trips every
 * scenario anybody ever hand-wrote — the first one that did not would silently rewrite a
 * committed file a reviewer had already approved. The template and the clause chips teach the
 * shape without owning it.
 */
import { useRef } from 'react'
import { Icon, asIcon } from '@/icons/Icon'
import styles from './OpenSpecPanel.module.css'
import {
  CLAUSES,
  addScenario,
  insertClause,
  removeScenario,
  saveRefusal,
  setScenario,
  type RequirementDraft,
} from './editModel'

export interface RequirementEditorProps {
  draft: RequirementDraft
  /** Is a save in flight? The form stays up and every control goes inert. */
  busy: boolean
  /**
   * What the last save answered, when it was not a plain success.
   *
   * `null` at rest. A regression carries the validator's own sentences and a conflict says the
   * file moved — both are *states this form draws*, not errors thrown past it, because a failed
   * save must leave the user's typing on screen to fix.
   */
  problem?: { kind: 'regressed' | 'conflicted'; messages: readonly string[] } | null | undefined
  onDraft: (draft: RequirementDraft) => void
  onSave: () => void
  onCancel: () => void
}

function cx(...parts: readonly (string | false | null | undefined)[]): string {
  return parts.filter((part): part is string => typeof part === 'string' && part !== '').join(' ')
}

export function RequirementEditor(props: RequirementEditorProps) {
  const { draft, busy, problem, onDraft, onSave, onCancel } = props
  const refusal = saveRefusal(draft)
  // One ref per scenario body, so a clause chip can put the caret back where it belongs. A
  // controlled textarea loses the selection on every re-render otherwise, and the second chip
  // press would insert at position zero.
  const bodies = useRef<Record<number, HTMLTextAreaElement | null>>({})

  return (
    <div className={styles.editor} data-audit="specEditor">
      <label className={styles.editorLabel} htmlFor="spec-req-name">
        Requirement
      </label>
      <input
        id="spec-req-name"
        className={styles.editorInput}
        data-audit="specEditName"
        data-write="true"
        value={draft.name}
        disabled={busy}
        onChange={(event) => onDraft({ ...draft, name: event.target.value })}
      />

      <label className={styles.editorLabel} htmlFor="spec-req-text">
        Behaviour
      </label>
      <textarea
        id="spec-req-text"
        className={styles.editorArea}
        data-audit="specEditText"
        data-write="true"
        rows={3}
        value={draft.text}
        disabled={busy}
        onChange={(event) => onDraft({ ...draft, text: event.target.value })}
      />

      {draft.scenarios.map((scenario, index) => (
        <div key={index} className={styles.editorScenario} data-audit="specEditScenario">
          <div className={styles.editorScenarioHead}>
            <input
              className={styles.editorInput}
              data-audit="specEditScenarioTitle"
              data-write="true"
              placeholder="What case is this?"
              value={scenario.title}
              disabled={busy}
              onChange={(event) =>
                onDraft(setScenario(draft, index, { title: event.target.value }))
              }
            />
            <button
              type="button"
              className={styles.editorDrop}
              data-audit="specEditDropScenario"
              data-write="true"
              title="Remove this scenario"
              disabled={busy}
              onClick={() => onDraft(removeScenario(draft, index))}
            >
              <Icon name={asIcon('trash-2')} size={1} />
            </button>
          </div>
          {/*
            * The clause chips. They insert a line at the caret rather than replacing the body,
            * which is what keeps this a *helper* — a user who types their own `- **WHEN**` gets
            * exactly the same file, and one who writes three clauses or a table is not fought.
            */}
          <div className={styles.editorClauses}>
            {CLAUSES.map((clause) => (
              <button
                key={clause}
                type="button"
                className={styles.editorClause}
                data-audit="specEditClause"
                data-clause={clause}
                data-write="true"
                disabled={busy}
                onClick={() => {
                  const field = bodies.current[index]
                  const at = field?.selectionStart ?? scenario.body.length
                  const next = insertClause(scenario.body, at, clause)
                  onDraft(setScenario(draft, index, { body: next.text }))
                  // After the controlled re-render, or the caret snaps to the end.
                  requestAnimationFrame(() => {
                    const again = bodies.current[index]
                    again?.focus()
                    again?.setSelectionRange(next.selStart, next.selEnd)
                  })
                }}
              >
                {clause}
              </button>
            ))}
          </div>
          <textarea
            ref={(node) => {
              bodies.current[index] = node
            }}
            className={styles.editorArea}
            data-audit="specEditScenarioBody"
            data-write="true"
            rows={3}
            value={scenario.body}
            disabled={busy}
            onChange={(event) =>
              onDraft(setScenario(draft, index, { body: event.target.value }))
            }
          />
        </div>
      ))}

      <button
        type="button"
        className={styles.editorAdd}
        data-audit="specEditAddScenario"
        data-write="true"
        disabled={busy}
        onClick={() => onDraft(addScenario(draft))}
      >
        + Scenario
      </button>

      {/*
        * A failed save draws here and the form stays up with the typing in it. A save that
        * closed the editor and reported elsewhere would throw away the paragraph it failed to
        * write, which is `TaskCompose`'s rule about the create dialog, one surface along.
        */}
      {problem != null && (
        <div className={styles.editorProblem} data-audit="specEditProblem" data-kind={problem.kind}>
          <p className={styles.editorProblemHead}>
            {problem.kind === 'conflicted'
              ? 'This file changed while you were editing — most likely an agent working in its worktree. Nothing was written.'
              : 'openspec validate refused this, so it has been put back:'}
          </p>
          {problem.messages.map((message, index) => (
            <p key={index} className={styles.editorProblemLine}>
              {message}
            </p>
          ))}
        </div>
      )}

      <div className={styles.editorActions}>
        <button
          type="button"
          className={cx(styles.button, refusal === null && styles.primary)}
          data-audit="specEditSave"
          data-write="true"
          disabled={busy || refusal !== null}
          title={refusal ?? 'Write this requirement back to its delta file'}
          onClick={onSave}
        >
          Save
        </button>
        <button
          type="button"
          className={styles.button}
          data-audit="specEditCancel"
          disabled={busy}
          onClick={onCancel}
        >
          Cancel
        </button>
      </div>
      {/*
        * The refusal, drawn as text and not only as a tooltip: `disabled` tells a screen reader
        * the control is off and never why, and a title attribute is invisible to one.
        */}
      {refusal !== null && (
        <p className={styles.editorRefusal} data-audit="specEditRefusal">
          {refusal}
        </p>
      )}
    </div>
  )
}
