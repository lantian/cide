/**
 * "You are about to do this to these files. Are you sure?"
 *
 * The house dialog for a gesture that removes things, wherever it is made. It lived in
 * `sidebar/GitPanel/` and moved here when the file tree needed the same dialog: the reasons
 * the two panels want it are different, the dialog is identical, and two copies of a
 * safeguard is how one of them quietly stops matching the other.
 *
 * `CloseConfirm` beside it is the original precedent and all three of them follow its rules:
 *
 * 1. **It names what would be lost**, every path, not a count. The user is about to act on
 *    *specific* files, and "12 files" is precisely the thing they cannot check before
 *    clicking. This is why [`ConfirmState.files`] is a list and not a number.
 * 2. **It only appears when something is at risk.** `useGitPanel` builds no state for an empty
 *    group, so the menu item is disabled rather than opening a dialog about nothing.
 * 3. **Cancel is the default.** Escape cancels, initial focus is on Cancel, and the
 *    destructive button is the plain one while the accent-filled button is the safe one — so
 *    a reflexive Enter, the keystroke already in flight when the dialog appeared, backs out.
 *
 * It takes a `run` rather than a specific verb because there are six of them now, and they
 * differ only in their wording and their command.
 *
 * # The two callers, and why they are not the same argument
 *
 * * **The Git panel**, for reverting a changelist or a file, deleting unversioned files, and
 *   dropping a shelf entry. `git_rollback` checks out HEAD over the tracked files and
 *   *deletes* the untracked ones, `cmd/git.rs` will not ask first — *"a confirmation the
 *   backend cannot show is not a safeguard"* — and git has no undo for any of it. The dialog
 *   is there because the act is **irreversible**.
 * * **The file tree**, for *Move to Trash*. That act is reversible: `fs_delete` moves to the
 *   freedesktop trash and never unlinks, which is exactly why the context-menu item shipped
 *   with no confirmation at all and was right to. The dialog appeared when the gesture got a
 *   **bare Delete key**, one finger from the arrow keys that same handler uses to move the
 *   selection — the cost of a misfire went from "impossible by accident" to "trivially
 *   likely", and the recovery went from nothing to a desktop trash the user has to go and
 *   find. The dialog is there because the act became **easy to trigger by accident**.
 *
 * * **A terminal pane**, for opening a file that is not in the project. Nothing is destroyed
 *   and nothing is hard to trigger; the dialog is there because the path was **chosen by
 *   attacker-influenced bytes** and the whole safeguard *is* the user reading it. That makes
 *   rule 1 — name every path, in full, never a basename — load-bearing rather than considerate:
 *   `credentials.json` looks like a project file and `/home/you/.claude/.credentials.json` does
 *   not. The rules live in `terminal/outsideOpen.ts` so a check script can run them; this
 *   component only draws what that module decided.
 *
 * * **The commit log**, for `git reset`, for checking a commit out (which detaches `HEAD`) and
 *   for moving a tag that already exists. `chrome/logActions.ts` writes those sentences, the
 *   same way `terminal/outsideOpen.ts` writes the third caller's. Reset is the one that made
 *   this component grow a radio group — see below.
 *
 * All four reasons produce the same dialog; the body text differs, which is what the body text
 * is for, and the third overrides the list glyph because it is not a removal. What they must not
 * do is diverge in their *shape* — one that named the paths and one that counted them would
 * teach the user to read one and skim the other.
 *
 * # The mode picker, and why it is here rather than in a second dialog
 *
 * `git reset` is not one act. `--soft`, `--mixed` and `--hard` move the same branch by the same
 * number of commits and cost the user wildly different things: nothing, the staging selection,
 * and the entire working tree. So the log's reset needs a dialog that asks *which*.
 *
 * The tempting shape was a `ResetDialog` of its own — three radios and a Reset button, next to
 * this file. It was rejected for the reason at the top of this header, stated the other way
 * round: a second dialog is a second place where Cancel has to be the focused, accented,
 * Escape-bound default, and the moment there are two, one of them stops being it. cide has
 * already paid for that once, which is why this component was moved out of `sidebar/GitPanel/`
 * at all.
 *
 * So [`ConfirmState.choices`] extends *this* dialog instead, and the extension is shaped so the
 * three rules keep holding **per choice**:
 *
 * * rule 1 — every choice carries its own [`ConfirmChoice.files`], and the list redraws when
 *   the radio moves. A single `files` at the top would have named the `--hard` casualties while
 *   `--soft` was selected, which is the dialog lying in the most dangerous possible direction;
 * * rule 2 — unchanged. The caller does not open this at all when nothing is at stake;
 * * rule 3 — Cancel keeps the focus and the accent whatever is selected, and the red on the
 *   destructive button is now driven by [`ConfirmChoice.danger`], so a `--soft` reset — which
 *   removes nothing — does not borrow the colour that means "this deletes work". `--red` still
 *   appears exactly once and still means exactly one thing.
 *
 * [`ConfirmState.option`] is the other half of the same gesture: a single checkbox, for the
 * *recoverable* version of the act. Reset's is "shelve my changes first", and it is **ticked by
 * default** — the same instinct that puts focus on Cancel, one step further in. A reflexive
 * Enter on this dialog cancels; a deliberate click on the destructive button, with nothing else
 * touched, still leaves the work retrievable from the Shelf.
 */
import { useEffect, useId, useRef } from 'react'
import { OverlayCard } from '@/overlays/ModalShell'
import { basename, dirname } from '@/overlays/format'
import styles from './ConfirmDestructive.module.css'

/**
 * One mode of an act that has several, e.g. a `--soft` / `--mixed` / `--hard` reset.
 *
 * A choice owns the parts of the dialog that *change* with it, and it owns all of them rather
 * than only the body. That is deliberate and it is rule 1 restated: `--soft` risks nothing,
 * `--mixed` risks the staging selection and `--hard` risks the working tree, so a dialog whose
 * file list did not follow the radio would be naming the wrong casualties — and it would be
 * naming them at exactly the moment the user is deciding whether the act is safe.
 *
 * `id` is not decoration. For reset it is literally the `ResetKind` that goes on the wire, so
 * the value the radio holds is the value the command is called with and there is no second
 * table mapping one to the other.
 */
export interface ConfirmChoice {
  id: string
  /** The word on the radio, e.g. `Hard`. Short — the sentence is the body's job. */
  label: string
  /** Replaces [`ConfirmState.body`] while this choice is the one selected. */
  body: string
  /** What **this** choice puts at risk. Rule 1 holds per choice, not per dialog. */
  files: readonly string[]
  /** The word on the destructive button while this choice is selected. */
  confirmLabel: string
  /**
   * Paint the destructive button red.
   *
   * Optional and defaulting to *not* red, because a choice that risks nothing must not be
   * dressed as one that does. `--red` means "this destroys work" in exactly one place in this
   * app (see `ConfirmDestructive.module.css`), and spending it on a `--soft` reset is how it
   * stops meaning anything on a `--hard` one.
   */
  danger?: boolean
}

/**
 * A single checkbox, for the recoverable version of the act.
 *
 * One, not a list: this is a confirmation, not a settings panel, and a dialog that grows a
 * second option is a dialog that has started asking the user to design the operation. Reset's
 * is *shelve my changes first*; nothing else has needed one yet.
 *
 * Controlled from outside, like [`ConfirmState.chosen`], because the answer has to survive the
 * dialog: the caller is the thing that passes it to the command, and a value living in this
 * component's own state would have to be lifted back out through `onConfirm` anyway.
 */
export interface ConfirmOption {
  label: string
  checked: boolean
  onToggle: (next: boolean) => void
}

/**
 * A confirmation for something a user cannot take back easily.
 *
 * `files` is every path at stake and the dialog names all of them. A count is not enough: the
 * user is about to act on *specific* files, and "12 files" is not something anyone can check
 * before clicking. Same rule `CloseConfirm` follows, for the same reason.
 */
export interface ConfirmState {
  title: string
  /** The sentence under the title. Ignored while `choices` is set — the choice carries it. */
  body: string
  /** Ignored while `choices` is set, for the reason [`ConfirmChoice.files`] gives. */
  files: readonly string[]
  /** The word on the destructive button, e.g. `Revert 4 files`. Ignored while `choices` is set. */
  confirmLabel: string
  /**
   * The glyph before each path. `−` (removal) unless the caller says otherwise.
   *
   * Optional because the third caller is not a removal — see the header — and the mark is the
   * one part of this dialog that states *what kind of thing* is about to happen. Reusing `−`
   * for an open would say "these files are about to go", which is a sentence the dialog would
   * then be contradicting.
   */
  mark?: string
  /**
   * The modes this act has, drawn as a radio group. Absent for an act that has only one.
   *
   * Absent rather than a one-element array for the single-mode case: a radio group with one
   * radio in it is a control that cannot be operated, and every caller that predates the log's
   * reset would have had to grow one to keep drawing the same dialog it already drew.
   */
  choices?: readonly ConfirmChoice[]
  /**
   * Which choice is selected, by `id`. An id no choice carries — including the `null` the
   * caller starts with — falls back to the **first** choice, which is why `choices` is ordered
   * least-destructive-first.
   */
  chosen?: string | null
  onChoose?: (id: string) => void
  option?: ConfirmOption
  /**
   * Go ahead.
   *
   * Both parameters are optional, and that is load-bearing rather than lazy. This signature has
   * to be assignable **in both directions** against the `() => void` it grew out of: every
   * existing producer writes `run: () => {}`, and two existing consumers — `useGitPanel.ts`'s
   * `pending?.run()` and `FileTree.tsx`'s `closeDelete(pendingDelete.run)`, which wants a bare
   * `() => void` — would both stop compiling against required parameters. Optional parameters
   * satisfy all four sites at once; required ones satisfy the producers only.
   *
   * `chosen` is the selected [`ConfirmChoice.id`] and `option` the checkbox's state. The caller
   * already holds both — it owns them — so passing them here is a convenience, not the channel:
   * this component never calls `run` itself. See [`ConfirmDestructiveProps.onConfirm`].
   */
  run: (chosen?: string | null, option?: boolean) => void
}

export interface ConfirmDestructiveProps {
  state: ConfirmState
  onCancel: () => void
  /** Go ahead. The caller runs `state.run` and closes; this component does neither. */
  onConfirm: () => void
}

export function ConfirmDestructive({ state, onCancel, onConfirm }: ConfirmDestructiveProps) {
  const cancel = useRef<HTMLButtonElement>(null)
  // Native `<input type="radio">`s group by `name`, and a fixed string would make two dialogs
  // mounted at once — which is not supposed to happen, but is one `&&` away from happening —
  // share one group and steal each other's selection. `useId` costs nothing and removes the
  // class of bug entirely.
  const group = useId()

  useEffect(() => {
    cancel.current?.focus()
  }, [])

  const onKeyDown = (ev: React.KeyboardEvent) => {
    if (ev.key === 'Escape') {
      ev.stopPropagation()
      onCancel()
    }
  }

  /*
   * Hoisted out of `state` before the JSX, and not inlined as `state.choices?.…`.
   *
   * Two reasons, one of them a compile error waiting to happen: TypeScript's narrowing of
   * `state.option` does not survive into the `onChange` closure below — a property of a prop
   * object can be reassigned as far as the checker knows — so the inline form needs an
   * `option?.` inside the handler and would silently do nothing if it were ever undefined
   * there. A local `const` narrows once and stays narrowed.
   */
  const choices = state.choices
  const onChoose = state.onChoose
  const option = state.option

  /*
   * The selected choice, or `undefined` for a dialog that has no modes.
   *
   * Falling back to `choices[0]` rather than rendering nothing selected: an unselected radio
   * group means the destructive button has no body, no file list and no label behind it, and
   * the alternative — disabling the button until something is picked — makes the *safest*
   * default (do nothing) require a click. `logActions.ts` orders the choices
   * least-destructive-first precisely so this fallback is the harmless one.
   */
  const active = choices === undefined ? undefined : (choices.find((c) => c.id === state.chosen) ?? choices[0])

  // Rule 1, per choice. The list, the sentence and the button all follow the radio together, or
  // the dialog describes one mode while it is about to perform another.
  const body = active?.body ?? state.body
  const files = active?.files ?? state.files
  const confirmLabel = active?.confirmLabel ?? state.confirmLabel
  // No choices means the older callers, every one of which *is* destroying something — so the
  // red stays theirs by default and only a mode that explicitly risks nothing gives it up.
  const danger = active === undefined || active.danger === true

  return (
    <OverlayCard label={state.title} onDismiss={onCancel}>
      <div className={styles.dialog} onKeyDown={onKeyDown} data-audit="confirmDestructive">
        <div className={styles.head}>
          <h2 className={styles.title} data-audit="confirmDestructiveTitle">
            {state.title}
          </h2>

          {/*
            * Above the body, not below it, because the body is *about* the selection. Reading
            * order has to be "reset what, to where / which kind / and here is what that kind
            * costs"; putting the radios under the sentence makes the first read of the dialog
            * describe a mode the user has not chosen yet.
            *
            * Real `<input type="radio">`s rather than buttons with `role="radio"`: the native
            * control brings arrow-key navigation, roving tab order and the right screen-reader
            * semantics for free, and a hand-rolled group that got any of the three wrong would
            * be wrong in a dialog whose entire job is being read carefully.
            */}
          {choices !== undefined && (
            <div
              className={styles.choices}
              role="radiogroup"
              aria-label={state.title}
              data-audit="confirmDestructiveChoices"
            >
              {choices.map((choice) => (
                <label
                  key={choice.id}
                  className={styles.choice}
                  data-audit="confirmDestructiveChoice"
                  data-choice={choice.id}
                >
                  <input
                    type="radio"
                    name={group}
                    value={choice.id}
                    checked={choice.id === active?.id}
                    onChange={() => onChoose?.(choice.id)}
                  />
                  <span className={styles.choiceLabel}>{choice.label}</span>
                </label>
              ))}
            </div>
          )}

          <p className={styles.body}>{body}</p>
        </div>

        <ul className={styles.list} data-audit="confirmDestructiveList">
          {files.map((path) => {
            // `basename`/`dirname` rather than the Git panel's `splitPath`, which does the
            // same arithmetic: this file no longer lives in that panel, and a chrome component
            // reaching back into a feature folder for a two-line helper is the import that
            // makes a shared component un-shareable again.
            const name = basename(path)
            const dir = dirname(path)
            return (
              <li key={path} className={styles.row} title={path}>
                {/* `−` rather than the tab strip's `●`: what is about to happen to these rows
                    is removal, and reusing the dirty dot would say "unsaved" instead. The
                    out-of-project open overrides it with `↗`, because nothing is being
                    removed there — see `ConfirmState.mark`. */}
                <span className={styles.mark} aria-hidden="true">
                  {state.mark ?? '−'}
                </span>
                <span className={styles.name}>{name}</span>
                <span className={styles.where}>{dir}</span>
              </li>
            )
          })}
        </ul>

        <div className={styles.footer}>
          {/*
            * Pushed hard left, away from the two answers — `PasteConfirm`'s rule, for the same
            * reason: a checkbox next to the destructive button is one that gets toggled by a
            * click that was 4px off, and this one decides whether the act is recoverable.
            *
            * It is *ticked* by default where it appears at all, which is the opposite of the
            * usual "opt in to the extra step". The safe answer should be the one a user reaches
            * by doing nothing; that is why Cancel has the focus, and this is the same rule one
            * step further in — the person who deliberately clicks *Discard 3 files* still gets
            * their work back out of the Shelf.
            */}
          {option !== undefined && (
            <label className={styles.option} data-audit="confirmDestructiveOption">
              <input
                type="checkbox"
                checked={option.checked}
                onChange={(ev) => option.onToggle(ev.target.checked)}
              />
              {option.label}
            </label>
          )}
          {/*
            * Never `buttonPrimary`. Rule 3 lives in this one className: the accent-filled
            * button is Cancel, so the Enter already in flight when the dialog appeared backs
            * out. `buttonDanger` is only the red *text*, and it is conditional now because a
            * `--soft` reset destroys nothing and must not wear the colour that says it does.
            */}
          <button
            type="button"
            className={`${styles.button} ${danger ? styles.buttonDanger : ''}`}
            data-audit="confirmDestructiveRun"
            onClick={onConfirm}
          >
            {confirmLabel}
          </button>
          <button
            ref={cancel}
            type="button"
            className={`${styles.button} ${styles.buttonPrimary}`}
            data-audit="confirmDestructiveCancel"
            onClick={onCancel}
          >
            Cancel
          </button>
        </div>
      </div>
    </OverlayCard>
  )
}
