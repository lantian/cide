/**
 * A `<textarea>` that offers the project's roles when the user types `@`.
 *
 * # One component, two ownership modes, and why both exist
 *
 * The card's body editor and the compose dialog's body are **controlled** (the draft lives in
 * the host or the store, for the dirty-tracking their file headers argue), so this takes
 * `value`/`onValueChange` and routes an accepted mention through them. The comment composer is
 * **uncontrolled** — deliberately, the last such control on the card, because `FormData` reads
 * the DOM and `form.reset()` is the whole of its state — so this also works with
 * `defaultValue`/`name`, applying an accepted mention by writing `el.value` directly, which is
 * exactly what FormData will read. Forcing the composer controlled to fit this component would
 * have been the more invasive change, and its "only uncontrolled control left" comment says why
 * it is shaped the way it is.
 *
 * # The popup is in normal flow, under the textarea — not anchored to the caret
 *
 * The caret-anchored version needs a mirror-`div` that copies the textarea's computed styles to
 * measure the caret's pixel position — a pile of style-copying no check can see, plus absolute
 * positioning that `cardBody`'s scroll container would clip and a z-index to argue about. A
 * list that opens directly below the field is one flex child, cannot be clipped, and — the
 * deciding point in this codebase — is plain markup `check:agents-render` renders open through
 * the exported [`MentionList`]. In a 620px card the list is never far from the caret anyway.
 *
 * # Keys, and the two guards that matter
 *
 * While the popup is open, `overlays/listKeys.ts::listAction` decides: arrows/Home/End move,
 * plain Enter accepts, Escape dismisses. Claimed keys get `preventDefault()` **and**
 * `stopPropagation()` — Escape must close the popup and not the card (the innermost-open-thing
 * rule `closeCard` relies on), and Enter must not submit the composer's form. Everything
 * unclaimed — including *modified* Enter, so Ctrl+Enter still saves a body mid-popup — falls
 * through to the caller's own `onKeyDown`.
 *
 * # ARIA mirrors `chrome/BranchSelector.tsx`, minus one role, on purpose
 *
 * Focus never leaves the textarea; the open popup is `role="listbox"` with `aria-selected`
 * option rows carrying stable ids, and the textarea points at it with `aria-controls` and
 * `aria-activedescendant` while `aria-expanded` says it is open. What is *not* copied is
 * `role="combobox"`: that would replace the multiline textbox role on a real prose field, and a
 * screen reader told "combobox" expects a single-line value. Option rows handle `onMouseDown`
 * with `preventDefault` so a click never steals focus — BranchSelector's reason, verbatim.
 */
import {
  useEffect,
  useRef,
  useState,
  type JSX,
  type KeyboardEvent,
  type TextareaHTMLAttributes,
} from 'react'
import { isListKey, listAction } from '@/overlays/listKeys'
import { applyMention, mentionOptions, mentionQuery } from './mentionModel'
import type { MentionOption, MentionQuery } from './mentionModel'
import { MARKDOWN_TOOLS, applyTool } from './markdownTools'
import { Icon, asIcon } from '@/icons/Icon'

import styles from './TasksPanel.module.css'

export interface MentionTextareaProps
  extends Omit<
    TextareaHTMLAttributes<HTMLTextAreaElement>,
    'value' | 'defaultValue' | 'onChange' | 'onKeyDown'
  > {
  /** Agent id → label — the same map the assignee `<select>` reads. `{}` offers nothing. */
  roles: Readonly<Record<string, string>>
  /** A stable id stem for the listbox and its options, so `aria-controls` has a target. */
  listboxId: string
  /*
   * The audit vocabulary, spelled out because hyphenated attributes pass unchecked only on
   * intrinsic elements — on a component they must be real props. All three ride the `...rest`
   * spread onto the textarea, so the rendered markup is byte-identical to the plain one it
   * replaced and every `data-audit` grep and digest keeps matching.
   */
  'data-audit'?: string | undefined
  'data-field'?: string | undefined
  'data-write'?: string | undefined
  /** Controlled mode: the text, and where a keystroke or an accepted mention goes. */
  value?: string | undefined
  onValueChange?: ((text: string) => void) | undefined
  /** Uncontrolled mode: the initial text; pair with `name` for the enclosing form. */
  defaultValue?: string | undefined
  /** Runs only for keys the popup declined — the caller's shortcuts keep working. */
  onKeyDown?: ((event: KeyboardEvent<HTMLTextAreaElement>) => void) | undefined
  /**
   * Draw the markdown formatting toolbar above the field. (M27)
   *
   * Opt-in per call site rather than always-on, because this component is about the `@` popup
   * and a future caller may want that without offering syntax buttons — but every *current*
   * caller writes markdown-rendered text (a body, a comment), so all of them pass it.
   */
  tools?: boolean | undefined
}

/** What is open: the token it is open *for*, and which row is highlighted. */
interface Popup {
  at: MentionQuery
  options: readonly MentionOption[]
  selected: number
}

export function MentionTextarea({
  roles,
  listboxId,
  value,
  onValueChange,
  defaultValue,
  onKeyDown,
  tools = false,
  ...rest
}: MentionTextareaProps) {
  const el = useRef<HTMLTextAreaElement>(null)
  const [popup, setPopup] = useState<Popup | null>(null)
  /*
   * The selection an accepted mention or a toolbar press wants, applied after React round-trips
   * the controlled value. A ref rather than state: it is not renderable, and a re-render for it
   * would be a render about nothing. A range, not a caret, because a formatting toggle must
   * leave the selection over what it transformed — that is what makes the second press undo.
   */
  const pendingCaret = useRef<{ start: number; end: number } | null>(null)
  // After *every* commit, not on a dependency: the caret can only be applied once React has
  // written the new value into the DOM, and during render the textarea still holds the old
  // text, which would clamp a caret past its end. In the uncontrolled mode the caret was set
  // synchronously in `accept` and this never arms. Effects do not run under SSR, so the render
  // check is untouched.
  useEffect(() => {
    if (el.current !== null && pendingCaret.current !== null) {
      el.current.setSelectionRange(pendingCaret.current.start, pendingCaret.current.end)
      pendingCaret.current = null
    }
  })

  /** Recompute the popup from what the DOM says right now — the one source both modes share. */
  const sync = () => {
    const area = el.current
    if (area === null) return
    const at = mentionQuery(area.value, area.selectionStart)
    if (at === null) {
      setPopup(null)
      return
    }
    const options = mentionOptions(roles, at.query)
    if (options.length === 0) {
      setPopup(null)
      return
    }
    setPopup((current) => ({
      at,
      options,
      // Keep the highlight while the user keeps typing, but never past the shorter list.
      selected: Math.min(current?.selected ?? 0, options.length - 1),
    }))
  }

  const accept = (option: MentionOption) => {
    const area = el.current
    if (area === null || popup === null) return
    const next = applyMention(area.value, popup.at, option.id)
    if (onValueChange !== undefined) {
      onValueChange(next.text)
      pendingCaret.current = { start: next.caret, end: next.caret }
    } else {
      // Uncontrolled: the DOM is the state, exactly as FormData will read it.
      area.value = next.text
      area.setSelectionRange(next.caret, next.caret)
    }
    setPopup(null)
  }

  /**
   * One toolbar press. The selection is read off the DOM — it survives the button taking a
   * click because `onMouseDown` is prevented below — and the result goes back through whichever
   * mode owns the text, exactly as `accept` routes a mention.
   */
  const format = (tool: string) => {
    const area = el.current
    if (area === null) return
    const edit = applyTool(area.value, area.selectionStart, area.selectionEnd, tool)
    if (onValueChange !== undefined) {
      onValueChange(edit.text)
      pendingCaret.current = { start: edit.selStart, end: edit.selEnd }
    } else {
      area.value = edit.text
      area.setSelectionRange(edit.selStart, edit.selEnd)
    }
    // Focus back for the keyboard path — a pointer press never let it leave. The popup is
    // closed rather than re-synced: the splice may have moved the caret out of its @token, and
    // in the controlled mode the DOM still holds the old text until React commits.
    area.focus()
    setPopup(null)
  }

  const keyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (popup !== null) {
      // Modified Enter is the caller's (Ctrl+Enter saves a body); everything else the list
      // claims is claimed whole — see the header for the two stopPropagation arguments.
      const modified =
        event.key === 'Enter' && (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey)
      const action = modified
        ? ({ kind: 'none' } as const)
        : listAction(event, popup.options.length, popup.selected)
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
    }
    onKeyDown?.(event)
  }

  const open = popup !== null
  return (
    <>
      {tools && (
        /*
         * The styling tools, above the field — GitHub's composer row, at this scale. Each
         * button spells syntax into the draft (`markdownTools.ts` decides what); none of them
         * writes to disk, so none carries `data-write` — the same argument the field pencil
         * makes. `onMouseDown` is prevented so a pointer press cannot blur the textarea and
         * collapse the very selection the tool is aimed at; the action rides `onClick`, which
         * a keyboard Enter on the focused button also fires.
         */
        <div className={styles.mdToolbar} role="toolbar" aria-label="Formatting">
          {MARKDOWN_TOOLS.map((spec) => (
            <button
              key={spec.tool}
              type="button"
              className={styles.mdTool}
              data-audit="tasksMdTool"
              data-tool={spec.tool}
              title={spec.label}
              aria-label={spec.label}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => format(spec.tool)}
            >
              <Icon name={asIcon(spec.icon)} size={1} />
            </button>
          ))}
        </div>
      )}
      <textarea
        {...rest}
        ref={el}
        {...(value !== undefined
          ? { value, onChange: (event) => onValueChange?.(event.target.value) }
          : { defaultValue: defaultValue ?? '' })}
        onKeyDown={keyDown}
        /*
         * `onInput` fires for every text change in both modes; `onSelect` fires when the caret
         * moves without one (arrows, clicks), which must close a popup the caret walked out of
         * or reopen one it walked into. Both funnel through `sync`, which reads the DOM.
         */
        onInput={sync}
        onSelect={sync}
        onBlur={() => setPopup(null)}
        aria-autocomplete="list"
        aria-expanded={open}
        {...(open ? { 'aria-controls': listboxId, 'aria-activedescendant': optionId(listboxId, popup.selected) } : {})}
      />
      {open && (
        <MentionList
          options={popup.options}
          selected={popup.selected}
          listboxId={listboxId}
          onPick={accept}
        />
      )}
    </>
  )
}

/** A row's DOM id — stable, so `aria-activedescendant` can point at it. */
function optionId(listboxId: string, index: number): string {
  return `${listboxId}-opt-${index}`
}

/**
 * The open popup, as plain markup. Exported so `check:agents-render`'s stories can render the
 * open state under `react-dom/server` — the component above only ever opens it from DOM events
 * a server render cannot fire.
 */
export function MentionList({
  options,
  selected,
  listboxId,
  onPick,
}: {
  options: readonly MentionOption[]
  selected: number
  listboxId: string
  onPick?: ((option: MentionOption) => void) | undefined
}): JSX.Element {
  return (
    <div
      className={styles.mentionPopup}
      data-audit="tasksMentionPopup"
      role="listbox"
      id={listboxId}
      aria-label="Mention a role"
    >
      {options.map((option, index) => (
        <div
          key={option.id}
          id={optionId(listboxId, index)}
          className={styles.mentionOption}
          role="option"
          aria-selected={index === selected}
          /* `onMouseDown`, and prevented: a click must pick without stealing focus from the
             textarea — BranchSelector's rule for its rows. */
          onMouseDown={(event) => {
            event.preventDefault()
            onPick?.(option)
          }}
        >
          <span className={styles.mentionId}>@{option.id}</span>
          {option.label !== option.id && (
            <span className={styles.mentionLabel}>{option.label}</span>
          )}
        </div>
      ))}
    </div>
  )
}
