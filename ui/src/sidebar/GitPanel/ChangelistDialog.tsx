/**
 * The changelist chooser: pick one, or make one right here.
 *
 * > *"in idea also there is a right click 'Move to another changelist' that leads to popup
 * > with list/group selection and it allows to create new one"*
 *
 * One component for all three of create, rename and move, because the move dialog has to be
 * able to create a list inline and once it can, a separate create dialog is the same field and
 * the same duplicate-name rule written a second time. `state.mode` is the only difference:
 * what the title says, whether the existing lists are clickable, and what the primary button
 * does.
 *
 * # Why the inline create is a field and not a "New…" item that opens a second dialog
 *
 * A second modal on top of a modal has to hand focus back and restore the first one's state,
 * and the state it would restore is the set of paths being moved — the one thing that must not
 * be lost. The field is always there instead: typing a name and pressing Enter creates *and*
 * moves in one gesture, which is what the operation actually is.
 *
 * # Names, not counts
 *
 * The moved paths are listed, not counted. This dialog is reachable from a right-click on a
 * row that may or may not have been ticked, so "4 files" is exactly the thing the user cannot
 * check. Same rule as `CloseConfirm`.
 *
 * The card and the scrim come from `OverlayCard`, so this dialog cannot drift away from the
 * file picker and the close confirmation.
 */
import { useEffect, useMemo, useRef, useState } from 'react'
import { OverlayCard } from '@/overlays/ModalShell'
import { DEFAULT_CHANGELIST } from './model'
import type { ChangelistDialogState } from './types'
import styles from './ChangelistDialog.module.css'

export interface ChangelistDialogProps {
  state: ChangelistDialogState
  onCancel: () => void
  /** Move the paths into an existing list. `move` only. */
  onPick: (id: string) => void
  /** Create a list with this name — and, in `move`, put the paths in it. Also does `rename`. */
  onSubmitName: (name: string) => void
}

const TITLE: Record<ChangelistDialogState['mode'], string> = {
  create: 'New changelist',
  rename: 'Rename changelist',
  move: 'Move to changelist',
}

export function ChangelistDialog({
  state,
  onCancel,
  onPick,
  onSubmitName,
}: ChangelistDialogProps) {
  const [name, setName] = useState(state.name)
  const field = useRef<HTMLInputElement>(null)

  // The field, not a button: every mode opens ready for a name, and in `move` the name field
  // is also the fastest route to the list the user wanted and has not made yet.
  useEffect(() => {
    field.current?.focus()
    field.current?.select()
  }, [])

  const trimmed = name.trim()
  /**
   * `Sidecar::create` and `Sidecar::rename` both refuse a duplicate name with
   * `GitError::DuplicateChangelist`. Saying so here rather than letting the command fail is
   * not belt and braces: the failure would land in the panel's one-line note *after* the
   * dialog had closed and taken the list of paths with it.
   */
  const clash = useMemo(
    () => state.lists.some((l) => l.name === trimmed && l.id !== state.id),
    [state.lists, state.id, trimmed],
  )
  const nameOk = trimmed !== '' && !clash
  const submitLabel = state.mode === 'rename' ? 'Rename' : state.mode === 'create' ? 'Create' : 'Create and move'

  const submit = () => {
    if (nameOk) onSubmitName(trimmed)
  }

  // Escape on the card rather than on `document`: a window listener would also answer for the
  // terminal underneath, and for any other overlay that happens to be open.
  const onKeyDown = (ev: React.KeyboardEvent) => {
    if (ev.key === 'Escape') {
      ev.stopPropagation()
      onCancel()
    }
  }

  return (
    <OverlayCard label={TITLE[state.mode]} onDismiss={onCancel}>
      <div className={styles.dialog} onKeyDown={onKeyDown} data-audit="changelistDialog">
        <div className={styles.head}>
          <h2 className={styles.title} data-audit="changelistDialogTitle">
            {TITLE[state.mode]}
            {state.repoName !== '' && <span className={styles.repo}> · {state.repoName}</span>}
          </h2>
          {state.mode === 'move' && (
            <p className={styles.body}>
              {state.paths.length === 1
                ? 'This file moves to the changelist you pick.'
                : `These ${state.paths.length} files move to the changelist you pick.`}
            </p>
          )}
        </div>

        {state.mode === 'move' && (
          <ul className={styles.paths} data-audit="changelistDialogPaths">
            {state.paths.map((path) => (
              <li key={path} className={styles.path} title={path}>
                {path}
              </li>
            ))}
          </ul>
        )}

        <div className={styles.nameRow}>
          <input
            ref={field}
            className={styles.input}
            data-audit="changelistDialogName"
            value={name}
            placeholder={state.mode === 'move' ? 'New changelist…' : 'Changelist name'}
            aria-label="Changelist name"
            spellCheck={false}
            autoComplete="off"
            onChange={(ev) => setName(ev.target.value)}
            onKeyDown={(ev) => {
              if (ev.key !== 'Enter') return
              ev.preventDefault()
              submit()
            }}
          />
          <button
            type="button"
            className={`${styles.button} ${styles.buttonPrimary}`}
            data-audit="changelistDialogSubmit"
            disabled={!nameOk}
            onClick={submit}
          >
            {submitLabel}
          </button>
        </div>
        {clash && (
          <p className={styles.warn} data-audit="changelistDialogClash">
            A changelist called “{trimmed}” already exists here.
          </p>
        )}

        {state.mode === 'move' && (
          <ul className={styles.list} data-audit="changelistDialogList">
            {state.lists.map((list) => {
              // The list the paths are already in is not a move. Disabled rather than hidden:
              // a chooser that silently omits the current list makes the user wonder whether
              // it was deleted.
              const here = list.id === state.id
              return (
                <li key={list.id}>
                  <button
                    type="button"
                    className={styles.entry}
                    disabled={here}
                    onClick={() => onPick(list.id)}
                  >
                    <span className={styles.entryName}>{list.name}</span>
                    <span className={styles.entryMeta}>
                      {here && 'already here · '}
                      {list.active && 'active · '}
                      {list.id === DEFAULT_CHANGELIST && 'default · '}
                      {list.count} {list.count === 1 ? 'file' : 'files'}
                    </span>
                  </button>
                </li>
              )
            })}
          </ul>
        )}

        {state.mode !== 'move' && state.lists.length > 0 && (
          <ul className={styles.list} data-audit="changelistDialogExisting">
            {state.lists.map((list) => (
              <li key={list.id} className={styles.existing}>
                {list.name}
              </li>
            ))}
          </ul>
        )}

        <div className={styles.footer}>
          <button
            type="button"
            className={styles.button}
            data-audit="changelistDialogCancel"
            onClick={onCancel}
          >
            Cancel
          </button>
        </div>
      </div>
    </OverlayCard>
  )
}
