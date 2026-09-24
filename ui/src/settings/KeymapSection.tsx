/**
 * Settings → Keymap: every command, what it answers to, and the two gestures that change it.
 *
 * # What this file is and is not allowed to decide
 *
 * Three rules live elsewhere on purpose, and each of them was somewhere else first:
 *
 * * **Which entries a change writes** is `cide_core::keymap::apply_edit`. Moving a command off
 *   a default chord is a removal *and* an add, the removal has to carry the binding's own
 *   `when` or it matches nothing, and the file must stay a diff against compiled-in defaults
 *   or every future default change is frozen out. That is layering arithmetic, it needs the
 *   defaults to do it, and a second implementation of it here would be a second set of the
 *   bugs `keymap.rs` documents at length. This component names a command, a context and a key.
 * * **What the table shows and what a chord would collide with** is `./keymapModel.ts`, which
 *   is import-free so `ui/scripts/check-keymap.mjs` can compile it and drive every case. A rule
 *   inside a React component is a rule no check script can reach.
 * * **What a keystroke means while recording** is `keys/recorder.ts` + `keys/recorderStore.ts`,
 *   for the same reason and one more: the key gate is a window **capture** listener, so a
 *   dialog that listened for its own `keydown` would never see Ctrl+P — the gate would have
 *   opened the file picker on top of it. Recording works by being the gate's `capture`, which
 *   is global state and must not depend on this component having rendered.
 *
 * # Conflicts warn, they never block
 *
 * `cide_core::keymap` states the policy — "which of them should win is a judgement only the
 * user can make, and picking one silently would hide the mistake" — and blocking is the same
 * mistake wearing a different coat: a user rebinding two commands in sequence would be stopped
 * halfway through a legitimate edit. So a chord that is already taken raises a box that says
 * what holds it, whether the collision would actually be *reported* afterwards, and offers to
 * unbind the other command as one atomic edit. Both buttons write; neither is disabled.
 *
 * The post-hoc conflict banner stays too, because it is what catches a hand-edited file.
 */
import { Button } from '@/kit/components/Button'
import { SearchField } from '@/kit/components/Field'
import { Note as KitNote } from '@/kit/components/Feedback'
import { Dialog } from '@/kit/components/Overlay'
import { Kbd } from '@/kit/components/Status'
import { Modal } from '@/overlays/ModalShell'
import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  settings as settingsApi,
  type KeymapEdit,
  type KeymapReport,
} from '@/ipc/client'
import { chipLabel } from '@/keys/chords'
import { canExtend, canSave, chipFor } from '@/keys/recorder'
import {
  commitRecording,
  extendRecording,
  startRecording,
  stopRecording,
  useRecorder,
} from '@/keys/recorderStore'
import { errorText } from '@/ipc/errorText'
import { useWorkspace } from '@/store/workspace'
import {
  bareKeyWarning,
  bindable,
  buildRows,
  clashesFor,
  customised,
  editSummary,
  editTarget,
  entriesOf,
  matchesFilter,
  overridesSummary,
  type KeyClash,
  type KeymapEntry,
} from './keymapModel'
import { Note } from './controls'
import styles from './panels.module.css'


/** A change the user has asked for that needs answering before it is written. */
type Pending =
  | { kind: 'clash'; title: string; command: string; when: string | null; key: string; clashes: KeyClash[] }
  | { kind: 'resetAll'; count: number }

export function KeymapSection() {
  const [report, setReport] = useState<KeymapReport | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [filter, setFilter] = useState('')
  const [pending, setPending] = useState<Pending | null>(null)
  const [note, setNote] = useState<string | null>(null)

  /*
   * The registry, read from the mirror rather than fetched.
   *
   * Every other section here is a pure function of its props, and this one is not — it already
   * makes its own IPC call for the report. The commands come from `app_get_bootstrap`, which
   * this window has had since it opened, and fetching them again would be a round trip for an
   * answer already in memory.
   */
  const commands = useWorkspace((s) => s.boot?.commands)

  useEffect(() => {
    let cancelled = false
    void settingsApi
      .keymap()
      .then((next) => {
        if (!cancelled) setReport(next)
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(errorText(e))
      })
    return () => {
      cancelled = true
    }
  }, [])

  /*
   * The recorder is global state, so leaving this screen has to close it.
   *
   * It is the gate's `capture` while it is armed and consumes **every** keystroke; a Settings
   * tab closed or switched away from with the popup up would leave the window unable to type
   * anything at all, with nothing on screen to say why. The store is what owns the claim
   * precisely so it survives a render — which is the same reason it cannot be left to one.
   */
  useEffect(() => stopRecording, [])

  const apply = useCallback(async (edits: readonly KeymapEdit[]) => {
    setPending(null)
    try {
      const result = await settingsApi.keymapEdit(edits)
      setReport(result.report)
      setNote(editSummary(result.removed, result.added))
      setError(null)
    } catch (e: unknown) {
      // Shown rather than logged: the one refusal this command makes is "your keymap.json does
      // not parse, so it will not be rewritten", which is a sentence the user has to read.
      setError(errorText(e))
    }
  }, [])

  const rows = useMemo(
    () => buildRows(commands ?? [], report?.bindings ?? [], report?.overrides ?? []),
    [commands, report],
  )
  const entries = useMemo(
    () => entriesOf(rows).filter((entry) => matchesFilter(entry.row, filter)),
    [rows, filter],
  )

  const beginEdit = useCallback(
    (entry: KeymapEntry) => {
      const target = editTarget(entry.row, entry.binding)
      startRecording({
        command: target.command,
        title: entry.row.title,
        when: target.when,
        current: entry.binding?.key ?? null,
        save: (key) => {
          // The preview runs against the resolved table, normalised on both sides, which is
          // why it can see that `ctrl+backquote` and ``ctrl+` `` are one keystroke where
          // `cide_core::keymap` cannot. Nothing in the way means nothing to ask about.
          const clashes = clashesFor(report?.bindings ?? [], key, target.when, target.command)
          if (clashes.length === 0) {
            void apply([{ kind: 'rebind', command: target.command, when: target.when, key }])
            return
          }
          setPending({
            kind: 'clash',
            title: entry.row.title,
            command: target.command,
            when: target.when,
            key,
            clashes,
          })
        },
      })
    },
    [apply, report],
  )

  if (error !== null && report === null) {
    return <Note title="The keymap could not be read">{error}</Note>
  }
  if (report === null) {
    return <div className={styles.clean}>Reading the keymap…</div>
  }

  const overrides = report.overrides.length
  /*
   * *Reset all* is offered when there is something to reset **or when the file cannot be read**.
   *
   * The second half is the whole point. A parse failure resolves to an empty override list — the
   * screen still shows every default, which is right — so `overrides === 0` says the same thing
   * for a fresh install and for a file with one stray comment in it. Disabling on that count
   * greyed out the one control that can repair an unreadable file, on the screen that exists to
   * repair it, immediately after `keymap_edit` had refused an edit with the words "use Reset all
   * to replace it".
   */
  const canResetAll = overrides > 0 || !report.readable

  return (
    <>
      {report.conflicts.length === 0 && report.problems.length === 0 && (
        <div className={styles.clean}>No conflicts. Every keystroke reaches one command.</div>
      )}

      {report.conflicts.map((conflict) => (
        <KitNote key={`${conflict.key}${conflict.when ?? ''}`} tone="warn">
          <Kbd keys={[conflict.key]} /> is bound to{' '}
          {conflict.commands.length} commands
          {conflict.when !== null && <> when <code>{conflict.when}</code></>}:{' '}
          {conflict.commands.join(', ')}.{' '}
          {/* Resolution order is the contract: later layers win, and within a layer the last
              entry does. Naming the winner is what makes this actionable rather than alarming. */}
          <strong>{conflict.commands[conflict.commands.length - 1]}</strong> wins.
        </KitNote>
      ))}

      {report.problems.map((problem, i) => (
        <KitNote key={`${problem.key}-${problem.command}-${i}`} tone="bad">
          {problem.key !== '' && <Kbd keys={[problem.key]} />}{' '}
          {problem.command !== '' && <code className={styles.command}>{problem.command}</code>}{' '}
          {problem.message}
        </KitNote>
      ))}

      {/* The failure `KeymapEditResult` carries counts for: an edit that matched nothing looks
          exactly like one that worked, so the screen says which it was. */}
      {note !== null && <div className={styles.clean}>{note}</div>}
      {error !== null && <KitNote tone="bad">{error}</KitNote>}

      <div className={styles.toolbar}>
        <div className={styles.filter}>
          <SearchField
            size="sm"
            placeholder="Filter by command, group or key"
            aria-label="Filter keybindings"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
        </div>
        <Button
          size="sm"
          data-audit="keymapResetAll"
          disabled={!canResetAll}
          onClick={() => setPending({ kind: 'resetAll', count: overrides })}
        >
          Reset all
        </Button>
      </div>

      <table className={styles.table}>
        <thead>
          <tr>
            <th>Command</th>
            <th>Key</th>
            <th>When</th>
            <th>Source</th>
            <th aria-label="Actions" />
          </tr>
        </thead>
        <tbody>
          {entries.map((entry, i) => (
            <Line
              key={`${entry.row.id}-${entry.binding?.key ?? ''}-${i}`}
              entry={entry}
              onEdit={() => beginEdit(entry)}
              onUnbind={() => {
                const target = editTarget(entry.row, entry.binding)
                void apply([{ kind: 'unbind', command: target.command, when: target.when }])
              }}
              onReset={() => {
                const target = editTarget(entry.row, entry.binding)
                void apply([{ kind: 'reset', command: target.command, when: target.when }])
              }}
            />
          ))}
        </tbody>
      </table>

      <div className={styles.path}>
        {report.path}
        {' — '}
        {overridesSummary(overrides)}
      </div>
      {/*
        * Said before the first write rather than discovered after it. `keymap.json` is strict
        * JSON and always was — a `//` comment has never parsed — but the formatting and the
        * order of the fields inside an entry are the author's, and saving replaces them with
        * serde's. Entry *order* is kept, because it is semantic.
        */}
      <div className={styles.footnote}>
        Saving rewrites this file: your indentation and any field cide does not know about are
        replaced, though the order of your entries is kept. The previous contents are saved
        beside it as <code>keymap.json.bak</code> on every write.
      </div>

      <RecorderPopup
        onCancel={stopRecording}
        onSave={commitRecording}
        onExtend={extendRecording}
      />
      {pending !== null && (
        <ConfirmPopup
          pending={pending}
          onCancel={() => setPending(null)}
          onApply={(edits) => void apply(edits)}
        />
      )}
    </>
  )
}

/** One line: a command with one of its bindings, or a command with none. */
function Line({
  entry,
  onEdit,
  onUnbind,
  onReset,
}: {
  entry: KeymapEntry
  onEdit: () => void
  onUnbind: () => void
  onReset: () => void
}) {
  const { row, binding } = entry
  return (
    <tr>
      <td>
        <div className={styles.rowTitle}>{row.title}</div>
        <div className={styles.rowId}>{row.id}</div>
        {row.unavailable !== null && (
          <div className={styles.rowNote}>Not in this build: {row.unavailable}</div>
        )}
      </td>
      <td>
        {binding !== null ? (
          <Kbd keys={[chipLabel(binding.key)]} />
        ) : (
          /* The row a resolved-bindings list cannot draw. "Unbound" is what the user did;
             "no shortcut" is what shipped, and telling them apart is the whole point of
             `KeymapReport.overrides`. */
          <span className={styles.rowNote}>{row.unbound ? 'Unbound by you' : 'No shortcut'}</span>
        )}
      </td>
      <td className={styles.layer}>{binding?.when ?? ''}</td>
      <td className={customised(row) ? `${styles.layer} ${styles.layerUser}` : styles.layer}>
        {customised(row) ? 'user' : (binding?.layer ?? '')}
      </td>
      <td className={styles.actions}>
        {bindable(row) && (
          <Button size="sm" variant="quiet" data-audit="keymapEdit" onClick={onEdit}>
            {binding === null ? 'Add' : 'Edit'}
          </Button>
        )}
        {bindable(row) && binding !== null && (
          <Button size="sm" variant="quiet" onClick={onUnbind}>
            Unbind
          </Button>
        )}
        {/*
          * Restore default is a **deletion** — every user entry aimed at this (command,
          * `when`), the `-old` removal and the new add together — and never a write of the
          * default value. Writing it in would freeze today's default into the user's file,
          * which is the same mistake as saving the whole resolved table.
          */}
        {customised(row) && (
          <Button size="sm" variant="quiet" onClick={onReset}>
            Restore default
          </Button>
        )}
      </td>
    </tr>
  )
}

/**
 * The capture box.
 *
 * Rendered whenever the store has a target, which is what keeps "the recorder is armed" and
 * "the popup is on screen" the same fact. It draws no scrim-click-to-dismiss of its own beyond
 * the usual one, because the keyboard route out — Escape — is the one that has to work: the
 * gate hands every stroke to the recorder while it is up, so nothing else on the page can be
 * reached with the keyboard anyway.
 */
function RecorderPopup({
  onCancel,
  onSave,
  onExtend,
}: {
  onCancel: () => void
  onSave: () => void
  onExtend: () => void
}) {
  const target = useRecorder((s) => s.target)
  const recording = useRecorder((s) => s.recording)
  if (target === null) return null

  const chip = chipFor(recording)
  const key = recording.strokes.join(' ')
  const bare = key === '' ? null : bareKeyWarning(key)

  return (
    <Modal onDismiss={onCancel}>
      <Dialog
        title={`Shortcut for ${target.title}`}
        width="narrow"
        data-audit="keymapRecorder"
        footNote={
          <Button
            size="sm"
            variant="quiet"
            icon="plus"
            disabled={!canExtend(recording)}
            onClick={onExtend}
          >
            second stroke
          </Button>
        }
        actions={
          <>
            <Button onClick={onCancel}>Cancel</Button>
            <Button
              variant="primary"
              data-audit="keymapRecorderSave"
              disabled={!canSave(recording)}
              onClick={onSave}
            >
              Save
            </Button>
          </>
        }
      >
        <div className={styles.capture} aria-live="polite">
          {chip === '' ? <span className={styles.rowNote}>Press a shortcut…</span> : <Kbd keys={[chip]} />}
        </div>
        <div className={styles.hint}>
          {target.current !== null && <>Currently {chipLabel(target.current)}. </>}
          {bare ?? 'Enter saves, Escape cancels — so those two, unmodified, cannot be recorded here.'}
        </div>
      </Dialog>
    </Modal>
  )
}

/**
 * "That chord is taken" and "throw away every override" — the two questions worth asking.
 *
 * The clash box carries the fix, because the fix is the part a user cannot work out by hand:
 * unbinding the other command needs a `-command` entry with *its* `when`, and getting that
 * wrong removes nothing. Both edits go in one call, so the file never passes through a state
 * where the chord runs two commands.
 */
function ConfirmPopup({
  pending,
  onCancel,
  onApply,
}: {
  pending: Pending
  onCancel: () => void
  onApply: (edits: KeymapEdit[]) => void
}) {
  const bind: KeymapEdit[] =
    pending.kind === 'clash'
      ? [{ kind: 'rebind', command: pending.command, when: pending.when, key: pending.key }]
      : [{ kind: 'resetAll' }]

  const title = pending.kind === 'clash' ? 'That shortcut is taken' : 'Reset all shortcuts'
  return (
    <Modal onDismiss={onCancel}>
      <Dialog
        title={title}
        width="narrow"
        data-audit="keymapConfirm"
        onKeyDown={(ev) => {
          if (ev.key !== 'Escape') return
          ev.stopPropagation()
          onCancel()
        }}
        actions={
          <>
            <Button onClick={onCancel}>Cancel</Button>
            {pending.kind === 'clash' && (
              <Button
                data-audit="keymapDisplace"
                onClick={() =>
                  onApply([
                    // The removals first, so the file reads as "take it away, then put this
                    // here" — and each carries the losing binding's own `when`, copied from the
                    // resolved row rather than reconstructed.
                    ...pending.clashes.map(
                      (clash): KeymapEdit => ({
                        kind: 'unbind',
                        command: clash.command,
                        when: clash.when,
                      }),
                    ),
                    ...bind,
                  ])
                }
              >
                Bind and unbind the other
              </Button>
            )}
            <Button
              variant={pending.kind === 'resetAll' ? 'danger' : 'primary'}
              data-audit="keymapConfirmApply"
              onClick={() => onApply(bind)}
            >
              {pending.kind === 'clash' ? 'Bind anyway' : 'Reset all'}
            </Button>
          </>
        }
      >
      <div className={styles.hint}>
          {pending.kind === 'resetAll' ? (
            <>
              Deletes all {pending.count} {pending.count === 1 ? 'entry' : 'entries'} from your
              keymap.json. Every binding goes back to the compiled-in default.
            </>
          ) : (
            <>
              <Kbd keys={[chipLabel(pending.key)]} />{' '}
              {pending.clashes.map((clash, i) => (
                <span key={`${clash.command}-${i}`}>
                  {i > 0 && ' and '}
                  {describe(clash)}
                </span>
              ))}
              .{' '}
              {pending.clashes.some((c) => c.contested)
                ? 'Binding it anyway leaves both alive and the conflict will be reported above.'
                : 'Binding it anyway silently takes the shortcut away from that command.'}
            </>
          )}
        </div>
      </Dialog>
    </Modal>
  )
}

/**
 * One clash, as the clause that follows the chip.
 *
 * The two sequence cases read as prose rather than as jargon because they are the ones a user
 * will never guess: `keys/keymap.ts` prefers an applicable continuation over an exact match, so
 * a chord and a sequence beginning with it cannot both fire, and which of them loses depends
 * only on which is longer.
 */
function describe(clash: KeyClash): string {
  const layer = clash.layer === 'user' ? 'your override' : `the ${clash.layer} keymap`
  if (clash.kind === 'prefix') {
    return `is the first stroke of ${chipLabel(clash.key)}, which runs ${clash.command} (${layer}) — a binding on it alone could never fire`
  }
  if (clash.kind === 'extends') {
    return `starts with ${chipLabel(clash.key)}, which runs ${clash.command} (${layer}) — that binding would stop firing`
  }
  return `already runs ${clash.command} (${layer})`
}
