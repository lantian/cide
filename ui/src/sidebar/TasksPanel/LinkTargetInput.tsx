/**
 * An `<input>` that searches the board and offers matching tasks — the link picker's
 * autocomplete. (M30)
 *
 * # Why an input, when the first version was a `<select>`
 *
 * A `<select>` over a board is a list nobody can *search*: the reported gesture was "find the
 * task by its key or its summary", and a native dropdown answers that with scroll. This is
 * `MentionTextarea`'s arrangement on a single-line field: the pure matching lives in
 * `model.ts::linkTargetOptions` (tiers — id prefix first, so `t-1` reaches for the key, then
 * title word-prefix, then substrings), the popup is the exported [`LinkTargetList`] so
 * `check:agents-render` can draw it open, and the keys go through `overlays/listKeys.ts` so
 * arrows/Enter/Escape mean here what they mean in every list in the app.
 *
 * # Choosing IS the commit
 *
 * Picking a row calls `onPick` and clears the input — the assignee `<select>`'s argument: the
 * choice carries the whole value, and a separate Add button would have nothing left to do.
 * The caller decides what a pick does (the card writes the edge; the compose dialog adds a
 * draft chip) and whether the picker then closes.
 *
 * # The popup is in normal flow, under the input
 *
 * `MentionTextarea`'s decision, for its stated reasons: absolute positioning is clipped by
 * `cardBody`'s scroll container, needs a z-index argument, and — decisive here — is markup the
 * render check cannot see open. A list that pushes the section taller while open is one flex
 * child and no stacking question.
 *
 * # Escape is scoped to the innermost open thing
 *
 * With the popup open it closes the popup and nothing else; with it shut, `onDismiss` (the
 * card wires it to closing the picker) — and only when a caller passed one does the key stop
 * propagating, so in the compose dialog an Escape on the empty input still discards the
 * dialog, exactly as it does from every other field. `closeCard`'s innermost rule, one level
 * further in.
 */
import { useState, type KeyboardEvent } from 'react'
import { isListKey, listAction } from '@/overlays/listKeys'
import { linkTargetOptions, statusLabel, type LinkTargetOption } from './model'

import styles from './TasksPanel.module.css'

export interface LinkTargetInputProps {
  /** What may be offered — already self-excluded and in panel order (`linkableTargets`). */
  targets: readonly LinkTargetOption[]
  /** A stable id stem for the listbox and its options, so `aria-controls` has a target. */
  listboxId: string
  /** The choice, made. The input clears itself; what the pick *does* is the caller's. */
  onPick: (id: string) => void
  /** Escape with the popup already shut. Absent, the key bubbles (the compose dialog's case). */
  onDismiss?: (() => void) | undefined
  'data-audit'?: string | undefined
  'data-write'?: string | undefined
  id?: string | undefined
  autoFocus?: boolean | undefined
  'aria-label'?: string | undefined
}

/** What is open: which rows, and which of them is highlighted. */
interface Popup {
  options: readonly LinkTargetOption[]
  selected: number
}

export function LinkTargetInput({
  targets,
  listboxId,
  onPick,
  onDismiss,
  ...rest
}: LinkTargetInputProps) {
  const [query, setQuery] = useState('')
  const [popup, setPopup] = useState<Popup | null>(null)

  /** Recompute the popup for a query — the one door both the keystroke and the focus use. */
  const sync = (text: string) => {
    const options = linkTargetOptions(text, targets)
    if (options.length === 0) {
      setPopup(null)
      return
    }
    setPopup((current) => ({
      options,
      // Keep the highlight while the user keeps typing, never past the shorter list.
      selected: Math.min(current?.selected ?? 0, options.length - 1),
    }))
  }

  const accept = (option: LinkTargetOption) => {
    onPick(option.id)
    // Cleared and closed: the pick is consumed (a chip, an edge), and the input is ready for
    // the next search rather than holding a query whose answer has already been taken.
    setQuery('')
    setPopup(null)
  }

  const keyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (popup !== null) {
      const action = listAction(event, popup.options.length, popup.selected)
      if (isListKey(action)) {
        event.preventDefault()
        event.stopPropagation()
        if (action.kind === 'select') {
          setPopup({ ...popup, selected: action.index })
        } else if (action.kind === 'dismiss') {
          setPopup(null)
        } else if (action.kind === 'accept') {
          const option = popup.options[popup.selected]
          if (option !== undefined) accept(option)
        }
        return
      }
      return
    }
    if (event.key === 'Escape' && onDismiss !== undefined) {
      event.stopPropagation()
      onDismiss()
      return
    }
    // Enter with the popup shut must not submit an enclosing form (the compose dialog is one):
    // the input's whole vocabulary is pick-from-the-list, and a form submit from inside it
    // would create the task mid-search.
    if (event.key === 'Enter') event.preventDefault()
  }

  const open = popup !== null
  return (
    <div className={styles.linkTargetWrap}>
      <input
        {...rest}
        className={styles.input}
        type="text"
        placeholder="Search tasks by key or title…"
        value={query}
        onChange={(event) => {
          setQuery(event.target.value)
          sync(event.target.value)
        }}
        /* Open on focus with the empty query's capped panel-order rows, so the input is
           discoverable as a picker and not only as a search — a `<select>` opened on click,
           and this must not offer less for being better. */
        onFocus={() => sync(query)}
        onBlur={() => setPopup(null)}
        onKeyDown={keyDown}
        aria-autocomplete="list"
        aria-expanded={open}
        {...(open
          ? {
              'aria-controls': listboxId,
              'aria-activedescendant': optionId(listboxId, popup.selected),
            }
          : {})}
      />
      {open && (
        <LinkTargetList
          options={popup.options}
          selected={popup.selected}
          listboxId={listboxId}
          onPick={accept}
        />
      )}
    </div>
  )
}

/** A row's DOM id — stable, so `aria-activedescendant` can point at it. */
function optionId(listboxId: string, index: number): string {
  return `${listboxId}-opt-${index}`
}

/**
 * The open popup, as plain markup — [`MentionList`]'s arrangement, exported for its reason:
 * the component above only ever opens it from DOM events a server render cannot fire, and this
 * is the half `check:agents-render`'s stories draw.
 */
export function LinkTargetList({
  options,
  selected,
  listboxId,
  onPick,
}: {
  options: readonly LinkTargetOption[]
  selected: number
  listboxId: string
  onPick?: ((option: LinkTargetOption) => void) | undefined
}) {
  return (
    <div
      className={styles.mentionPopup}
      data-audit="taskLinkTargetPopup"
      role="listbox"
      id={listboxId}
      aria-label="Link a task"
    >
      {options.map((option, index) => (
        <div
          key={option.id}
          id={optionId(listboxId, index)}
          className={styles.mentionOption}
          role="option"
          aria-selected={index === selected}
          /* `onMouseDown`, and prevented: a click must pick without stealing focus from the
             input — `MentionList`'s rule, which is BranchSelector's. */
          onMouseDown={(event) => {
            event.preventDefault()
            onPick?.(option)
          }}
        >
          <span className={styles.linkTargetId}>{option.id}</span>
          <span className={styles.mentionLabel}>
            {option.title.trim() !== '' ? option.title : 'Untitled'}
          </span>
          {/* The status, dim at the row's end: which tasks are already done is half of
              choosing a blocker. Through `statusLabel`, so a rogue status reads `Unknown`. */}
          <span className={styles.linkTargetStatus}>{statusLabel(option.status)}</span>
        </div>
      ))}
    </div>
  )
}
