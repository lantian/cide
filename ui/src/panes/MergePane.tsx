/**
 * IDEA's three-pane merge window, as a tab. (M20)
 *
 * ```
 * ┌─ HEAD (main) ──┐┌─ Result ───────┐┌─ origin/main ──┐
 * │ read-only      ││ editable       ││ read-only      │
 * └────────────────┘└────────────────┘└────────────────┘
 * ```
 *
 * # Split in two, for the reason every pane in this directory is
 *
 * [`MergePaneView`] is pure — props in, no IPC, no store — so `ui/scripts/check-merge-render.mjs`
 * can render it under node through `react-dom/server` and assert that it *paints*. `MergePane`
 * is the wiring. `pnpm build` proves this file compiles; only the SSR check proves the resolver
 * draws three panes rather than one empty box, which is the failure mode this project keeps
 * producing.
 *
 * # Why the panes are three plain editors and not a `MergeView`
 *
 * `@codemirror/merge` ships a two-pane `MergeView` and a `unifiedMergeView`, and no three-way
 * primitive at all — `MergeView` is strictly `{a, b}`. `panes/DiffPane.tsx` already uses it for
 * the Claude diff, and its shape does not generalise: a third editor is not something that
 * component can be given.
 *
 * What is reused instead is the thing that matters — **git's own merge result**. See
 * `panes/mergeModel.ts`, whose header carries the argument in full: the working-tree file *is*
 * the merge, every one-sided hunk already applied, so the resolver parses what git wrote rather
 * than re-deriving a merge that could disagree with the index cide is about to commit.
 *
 * # Size guards, borrowed wholesale from `DiffPane`
 *
 * Three CodeMirror documents laid side by side force `height: auto` on their content and defeat
 * the viewport windowing that makes a large file survivable. `cide_git::conflict::read` refuses
 * over `MAX_SIDE_BYTES` and reports `tooLarge`; this pane draws the one-click answers and no
 * editors at all for that case, and for a binary conflict.
 */
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { Compartment, EditorState } from '@codemirror/state'
import { EditorView, keymap, lineNumbers } from '@codemirror/view'
// `history` is aliased: this component already has a `history` ref of its own, holding the
// block-decision undo stack. The two are genuinely different undos and both are wanted — see
// the `onKeyDown` handler near the bottom of the file, which routes between them.
import { history as textHistory, historyKeymap } from '@codemirror/commands'
import { syntaxHighlighting } from '@codemirror/language'
import { lineEditKeymap } from '@/editor/editorKeys'
import { cideHighlightStyle } from '@/editor/highlight'
import { loadLanguage } from '@/editor/languages'
import { notify } from '@/chrome/notices'
import { explain } from '@/chrome/branchModel'
import {
  branch as branchApi,
  file as fileApi,
  tab as tabApi,
  type ConflictFile,
  type ProjectId,
} from '@/ipc/client'
import {
  accept,
  applyNonConflicting,
  build,
  canApply,
  decisionOf,
  ignore,
  inlineDiff,
  answered,
  unset,
  pending,
  progressLabel,
  regionText,
  regionTone,
  renderResult,
  reset,
  resolveSimple,
  resultSpans,
  settled,
  sideSpans,
  type Decisions,
  type MergeDoc,
  type Span,
  type Side,
} from './mergeModel'
import { afterResolve } from '@/chrome/conflictsStore'
import { useSettings } from '@/settings/useSettings'
import { paint, paintedField } from './mergeDecorations'
import { conflictGutter, setGutter } from './mergeGutter'
import { Icon, type IconName } from '@/icons/Icon'

import styles from './MergePane.module.css'

export interface MergePaneViewProps {
  /** The conflicted path, repo-relative. */
  path: string
  /** `null` while the first read is in flight. */
  file: ConflictFile | null
  /** Set when the read failed, or when the path is no longer conflicted. */
  unavailable: string | null
  /** The centre pane's current text. */
  result: string
  doc: MergeDoc
  decisions: Decisions
  /** Which region the toolbar's Reset acts on. */
  currentId: string | null
  busy: boolean
  /** Accept one side of the current region — the fallback for a side with no line to click. */
  onTake: (side: Side) => void
  onReset: () => void
  onApplyNonConflicting: () => void
  onResolveSimple: () => void
  onStep: (delta: 1 | -1) => void
  onApply: () => void
  onAbandon: () => void
  /** Ctrl+Z over the block decisions. See the handler for why it is on the wrapper. */
  onKeyDown?: ((event: React.KeyboardEvent<HTMLDivElement>) => void) | undefined
  /** Mounts the three editors. Absent in an SSR render, which has no DOM to mount into. */
  mountEditors?:
    | ((host: HTMLDivElement | null, side: 'ours' | 'result' | 'theirs') => void)
    | undefined
}

export function MergePaneView(props: MergePaneViewProps) {
  const { file, unavailable, doc, decisions, result, busy, currentId } = props

  if (unavailable !== null) {
    return (
      <div className={styles.host} data-audit="mergePane">
        <div className={styles.empty} data-audit="mergePaneUnavailable">
          <p className={styles.note}>{unavailable}</p>
          <p className={styles.note}>
            Conflicts are listed in the Git panel while a merge or rebase is in progress.
          </p>
        </div>
      </div>
    )
  }

  if (file === null) {
    return (
      <div className={styles.host} data-audit="mergePane">
        <div className={styles.empty}>Reading {props.path}…</div>
      </div>
    )
  }

  // Binary, or too large for three editors. The two answers that need no editor are still
  // offered, because they are the two that resolve this file without one.
  const textless = file.binary || file.tooLarge !== null
  const current = doc.regions.find((r) => r.id === currentId) ?? null
  const decision = current === null ? null : decisionOf(decisions, current.id)
  const position = current === null ? 0 : doc.regions.findIndex((r) => r.id === current.id) + 1

  /**
   * *Accept this side of the current region*, in the pane heading.
   *
   * The per-block chevrons in the gutter are the gesture; this covers the one case they cannot.
   * A side that **deleted** the region has no lines of its own, so there is no line to hang a
   * chevron on — and "take the side that deleted it" is a real answer that would otherwise be
   * unreachable.
   */
  const accepted = (side: Side) => decision !== null && decision.taken.includes(side)
  const acceptButton = (side: Side, mark: IconName) => {
    // Only for a side that deleted the current block, and only while it is still asking. Every
    // other block has its own chevron in the gutter, which is where the gesture belongs.
    const empty =
      current !== null && (side === 'ours' ? current.ours.length : current.theirs.length) === 0
    if (!empty) return null
    return (
      <button
        type="button"
        className={`${styles.accept} ${accepted(side) ? styles.acceptOn : ''}`}
        disabled={busy || current === null}
        onClick={() => props.onTake(side)}
        data-audit={`mergeAccept-${side}`}
        title={`This side deletes block ${position}. Accept it to remove those lines.`}
      >
        <Icon name={mark} size={1} /> {accepted(side) ? 'Deleted' : 'Delete'}
      </button>
    )
  }

  return (
    <div className={styles.host} data-audit="mergePane" onKeyDown={props.onKeyDown}>
      <div className={styles.bar} data-audit="mergePaneBar">
        <span
          className={`${styles.counter} ${
            canApply(result, doc, decisions) ? styles.resolved : styles.pending
          }`}
          data-audit="mergePaneCounter"
        >
          {textless ? 'Binary or very large' : progressLabel(doc, decisions)}
        </span>
        {!textless && current !== null && (
          <span className={styles.counter} data-audit="mergePaneAt">
            {`· block ${position} of ${doc.regions.length}`}
            {decision !== null && decision.taken.length > 0 && ` · ${decision.taken.join(', then ')}`}
            {decision !== null &&
              decision.taken.length === 0 &&
              settled(current, decision) &&
              ' · base kept'}
          </span>
        )}
        <span className={styles.spacer} />
        {!textless && (
          <>
            <button
              type="button"
              className={styles.action}
              onClick={() => props.onStep(-1)}
              disabled={busy || doc.regions.length < 2}
              title="Previous block"
            >
              <Icon name="chevron-left" size={1} />
            </button>
            <button
              type="button"
              className={styles.action}
              onClick={() => props.onStep(1)}
              disabled={busy || doc.regions.length < 2}
              title="Next block"
            >
              <Icon name="chevron-right" size={1} />
            </button>
            <button
              type="button"
              className={styles.action}
              onClick={props.onReset}
              disabled={busy || current === null}
              title="Put this block back to asking"
            >
              Reset
            </button>
            <button
              type="button"
              className={styles.action}
              onClick={props.onApplyNonConflicting}
              disabled={busy}
              title="Accept every block only one side changed"
            >
              Apply non-conflicting
            </button>
            <button
              type="button"
              className={styles.action}
              onClick={props.onResolveSimple}
              disabled={busy || doc.conflicts.length === 0}
              title="Answer every conflict whose two sides say the same thing"
            >
              Resolve simple
            </button>
          </>
        )}
        {textless && (
          <>
            <button
              type="button"
              className={styles.action}
              onClick={() => props.onTake('ours')}
              disabled={busy}
            >
              Accept {file.ourLabel}
            </button>
            <button
              type="button"
              className={styles.action}
              onClick={() => props.onTake('theirs')}
              disabled={busy}
            >
              Accept {file.theirLabel}
            </button>
          </>
        )}
        <button
          type="button"
          className={styles.action}
          onClick={props.onAbandon}
          disabled={busy}
          title="Close without writing anything. The conflict stays as it is."
        >
          Close
        </button>
        <button
          type="button"
          className={`${styles.action} ${styles.primary}`}
          onClick={props.onApply}
          disabled={busy || textless || !canApply(result, doc, decisions)}
          data-audit="mergePaneApply"
        >
          Apply
        </button>
      </div>

      {textless ? (
        <div className={styles.empty} data-audit="mergePaneTextless">
          <p className={styles.note}>
            {file.binary
              ? 'At least one side of this file is binary, so there is nothing to merge line by line.'
              : `This file is ${kb(file.tooLarge)} KB, too large to open three editors over.`}
          </p>
          <p className={styles.note}>
            Take one side whole with the buttons above, or resolve it in a terminal.
          </p>
        </div>
      ) : (
        <div className={styles.panes} data-audit="mergePanePanes">
          <Pane
            label={file.ourLabel}
            audit="mergePaneOurs"
            control={acceptButton('ours', 'chevrons-right')}
            mount={(el) => props.mountEditors?.(el, 'ours')}
            fallback={file.ours ?? null}
          />
          <Pane
            label="Result"
            active
            audit="mergePaneResult"
            mount={(el) => props.mountEditors?.(el, 'result')}
            fallback={result}
          />
          <Pane
            label={file.theirLabel}
            audit="mergePaneTheirs"
            control={acceptButton('theirs', 'chevrons-left')}
            mount={(el) => props.mountEditors?.(el, 'theirs')}
            fallback={file.theirs ?? null}
          />
        </div>
      )}
    </div>
  )
}

function Pane(props: {
  label: string
  active?: boolean
  audit: string
  control?: ReactNode
  mount: (el: HTMLDivElement | null) => void
  fallback: string | null
}) {
  return (
    <div className={styles.pane} data-audit={props.audit}>
      <div className={`${styles.paneHead} ${props.active === true ? styles.paneHeadActive : ''}`}>
        <span className={styles.paneLabel}>
          {props.label}
          {props.fallback === null && ' — deleted on this side'}
        </span>
        {props.control}
      </div>
      <div className={styles.paneBody} ref={props.mount} />
    </div>
  )
}

export interface MergePaneProps {
  project: ProjectId
  repo: string
  path: string
  tab: string
}

export function MergePane({ project, repo, path, tab }: MergePaneProps) {
  const [file, setFile] = useState<ConflictFile | null>(null)
  const [unavailable, setUnavailable] = useState<string | null>(null)
  const [decisions, setDecisions] = useState<Decisions>({})
  /**
   * Every state the decisions have been in, and where in it we are.
   *
   * The centre pane's own CodeMirror history covers *typing* and nothing else — a block accepted
   * by a chevron is not a document edit anybody made, it is a re-render — so Ctrl+Z over the
   * buttons needs its own stack. Kept as whole snapshots rather than as inverse operations
   * because a `Decisions` is a handful of small arrays: the entire history of a forty-block
   * merge is smaller than one line of the file.
   */
  const history = useRef<{ past: Decisions[]; future: Decisions[] }>({ past: [], future: [] })

  /** Record a decision change so Ctrl+Z can walk back through it. */
  const remember = useCallback((next: (held: Decisions) => Decisions) => {
    setDecisions((held) => {
      const after = next(held)
      if (after === held) return held
      history.current.past.push(held)
      // A new decision after an undo abandons the redo branch, which is what every editor does
      // and what stops Ctrl+Y replaying a future the user has walked away from.
      history.current.future = []
      return after
    })
  }, [])
  const [busy, setBusy] = useState(false)
  /** The centre pane's text once the user has typed into it. `null` until then. */
  const [edited, setEdited] = useState<string | null>(null)
  /** Which region the toolbar's Reset acts on. */
  const [at, setAt] = useState<string | null>(null)
  const [autoApplied, setAutoApplied] = useState(false)
  const hosts = useRef<Record<string, HTMLDivElement | null>>({})
  const views = useRef<Record<string, EditorView | null>>({})
  /**
   * One language slot per pane, reconfigured when the grammar chunk lands.
   *
   * A compartment rather than an extension baked into the initial state, for the reason
   * `EditorSurface` uses one: the grammar is a dynamic `import()` — Vite emits a chunk per
   * language and opening a `.toml` must not download the Rust keyword table — so it is not
   * available when the editor is built, and rebuilding the editor to add it would take the
   * scroll position and selection with it.
   */
  const languageSlots = useRef<Record<string, Compartment>>({
    ours: new Compartment(),
    result: new Compartment(),
    theirs: new Compartment(),
  })
  const act = useRef<(id: string, what: 'accept' | 'ignore' | 'revert', side: Side) => void>(
    () => {},
  )
  /**
   * One stable accessor per side, so a marker's identity never changes.
   *
   * `ChevronMarker.eq` deliberately ignores the handler — two markers differing only by closure
   * identity are the same buttons, and re-creating them on every render would drop the pointer
   * mid-click. These are built once and read `act.current` when pressed.
   */
  const accessors = useRef({
    ours: () => (id: string, what: 'accept' | 'ignore' | 'revert') =>
      act.current(id, what, 'ours'),
    theirs: () => (id: string, what: 'accept' | 'ignore' | 'revert') =>
      act.current(id, what, 'theirs'),
  })

  /**
   * The merge, computed from the three index stages.
   *
   * **Not from the working tree.** Git's markers show only the hunks it could not decide; the
   * ones it merged for you are already applied and invisible, and a merge tool that hides
   * two-thirds of what happened is not one. See `mergeModel`'s header.
   */
  const doc = useMemo(
    () => build(file?.base ?? null, file?.ours ?? null, file?.theirs ?? null),
    [file],
  )

  useEffect(() => {
    let live = true
    void branchApi
      .conflictRead(project, repo, path)
      .then((read) => {
        if (live) setFile(read)
      })
      .catch((error: unknown) => {
        if (live) setUnavailable(explain(error))
      })
    return () => {
      live = false
    }
  }, [project, repo, path])

  /*
   * `Settings › Git › Apply non-conflicting changes automatically`, applied once per file.
   *
   * Off by default, which is IDEA's default too and is the whole point of starting from the
   * base: every change is a decision you can see and take back. On, this is the same button the
   * toolbar carries, pressed for you.
   */
  const settings = useSettings()
  useEffect(() => {
    if (autoApplied || doc.regions.length === 0) return
    setAutoApplied(true)
    if (settings?.git.autoApplyNonConflicting !== true) return
    // Not through `remember`: this is the state the pane *opened* in, and putting it on the undo
    // stack would let Ctrl+Z walk back to a document the user never saw.
    setDecisions((d) => applyNonConflicting(doc, d))
  }, [doc, settings, autoApplied])

  // Start on the first block, so Reset and the counter mean something the moment it opens.
  useEffect(() => {
    setAt((held) => held ?? doc.regions[0]?.id ?? null)
  }, [doc])

  /**
   * What the centre pane holds.
   *
   * A hand edit wins while it stands, because the pane is a real editor and a user who has typed
   * a resolution must not have it overwritten by a re-render. Any button clears it, so the two
   * never fight over the document.
   */
  const result = useMemo(
    () => edited ?? renderResult(doc, decisions),
    [doc, decisions, edited],
  )

  useEffect(() => {
    act.current = (id, what, side) => {
      setAt(id)
      setEdited(null)
      remember((d) => {
        const held = decisionOf(d, id)
        const next =
          what === 'accept'
            ? accept(held, side)
            : what === 'ignore'
              ? ignore(held, side)
              : unset(held, side)
        return { ...d, [id]: next }
      })
    }
  })

  const step = (delta: 1 | -1) => {
    const ids = doc.regions.map((r) => r.id)
    if (ids.length === 0) return
    const here = at === null ? -1 : ids.indexOf(at)
    const next = ids[(here + delta + ids.length) % ids.length] ?? null
    setAt(next)
    if (next === null) return
    for (const side of ['ours', 'result', 'theirs'] as const) {
      const view = views.current[side]
      const span =
        side === 'result'
          ? resultSpans(doc, decisions).find((sp) => sp.id === next)
          : sideSpans(doc, side).find((sp) => sp.id === next)
      if (!view || span === undefined) continue
      const line = Math.min(Math.max(span.from + 1, 1), view.state.doc.lines)
      view.dispatch({
        effects: EditorView.scrollIntoView(view.state.doc.line(line).from, { y: 'center' }),
      })
    }
  }

  // The centre pane is a real editor and the tab carries a real dirty flag, so a stray Ctrl+W
  // asks before discarding the work. Cleared by Apply, which is the only thing that writes.
  useEffect(() => {
    const dirty = Object.keys(decisions).length > 0 || edited !== null
    void fileApi.setDirty(project, tab, dirty).catch(() => {})
  }, [project, tab, decisions, edited])

  const mountEditors = useCallback(
    (el: HTMLDivElement | null, side: 'ours' | 'result' | 'theirs') => {
      hosts.current[side] = el
    },
    [],
  )

  // Built once per document and reconfigured after, which is `EditorSurface`'s rule and the one
  // this project has paid for more than any other: a rebuilt `EditorView` loses the selection,
  // the undo history and the scroll position, and here it would lose an in-progress resolution.
  useEffect(() => {
    if (file === null || file.binary || file.tooLarge !== null) return
    const sides: ['ours' | 'result' | 'theirs', string | null, boolean][] = [
      ['ours', file.ours ?? null, false],
      ['result', result, true],
      ['theirs', file.theirs ?? null, false],
    ]
    for (const [side, value, editable] of sides) {
      const host = hosts.current[side]
      if (host === undefined || host === null || views.current[side]) continue
      const view = new EditorView({
        parent: host,
        state: EditorState.create({
          doc: value ?? '',
          extensions: [
            EditorView.lineWrapping,
            // Line numbers in all three. They do not line up across the panes — the three
            // documents are different lengths — and that mismatch is itself information.
            lineNumbers(),
            // The same style the editor paints with, so a merge does not show the file in
            // different colours from the tab beside it. `TOKEN_ROLES` is the one table both read.
            syntaxHighlighting(cideHighlightStyle),
            languageSlots.current[side]?.of([]) ?? [],
            paintedField,
            // The chevrons, on the side panes only. The result pane is where they point.
            ...(side === 'result' ? [] : conflictGutter(side)),
            /*
             * Ctrl+D in all three, and it needs no `editable` branch of its own: `copyLine`
             * guards on `state.readOnly` and returns `false`, so the two side panes refuse it
             * on their own. The binding and the chord trade are in `editor/editorKeys.ts`.
             */
            keymap.of(lineEditKeymap),
            ...(editable
              ? [
                  /*
                   * **A text undo history, which this pane claimed to have and did not.**
                   *
                   * The `onKeyDown` handler below says "the centre pane's CodeMirror history
                   * owns typing, and it must go on owning it", and steps out of the way for any
                   * Ctrl+Z that came from inside an editable surface. There was no CodeMirror
                   * history: this file imported nothing from `@codemirror/commands` and
                   * installed no `history()`. `undo` is a `StateCommand` over a `StateField`, so
                   * with no field there are no transactions to walk — the chord was handed to a
                   * history that did not exist and died there. Ctrl+Z in the result pane did
                   * nothing at all, and the comment saying otherwise had been read past.
                   *
                   * It matters more now than it did: `lineEditKeymap` above puts a
                   * document-editing command on Ctrl+D, and shipping an edit into a pane whose
                   * undo is inert is how a hand-merged block gets lost with nothing to reach for.
                   *
                   * Editable side only. `readOnly` refuses the transactions anyway, so a history
                   * on the side panes would be a field with nothing in it.
                   */
                  textHistory(),
                  keymap.of(historyKeymap),
                  EditorView.updateListener.of((update) => {
                    if (update.docChanged) setEdited(update.state.doc.toString())
                  }),
                ]
              : /*
                 * `readOnly` alone — **not** `editable.of(false)` beside it, which is what
                 * `DiffPane` uses and what this pane used to.
                 *
                 * `editable.of(false)` takes the caret away, and with it the keyboard selection,
                 * Ctrl+A and Ctrl+C. These panes are where the text a user wants to *copy into*
                 * the result lives — a name, a signature, a line they mean to merge by hand —
                 * and a pane you cannot copy out of is a pane you have to retype from.
                 *
                 * `readOnly` on its own is exactly the right half: transactions that change the
                 * document are refused, so nothing here can be edited, while the selection and
                 * the clipboard work as they do in any editor.
                 */
                [EditorState.readOnly.of(true)]),
          ],
        }),
      })
      views.current[side] = view

      /*
       * Fire-and-forget, and guarded on the view still being the live one: a tab closed while
       * its grammar chunk is in flight would otherwise dispatch into a destroyed editor.
       *
       * All three panes get the *same* language, from the one path this tab is about — the two
       * sides are versions of that file, whatever either of them did to it.
       */
      void loadLanguage(path).then((extension) => {
        const slot = languageSlots.current[side]
        if (extension === null || slot === undefined || views.current[side] !== view) return
        view.dispatch({ effects: slot.reconfigure(extension) })
      })
    }
    return () => {
      for (const key of Object.keys(views.current)) {
        views.current[key]?.destroy()
        views.current[key] = null
      }
    }
    // `result` is deliberately absent: the centre editor owns its own text after it is built,
    // and re-running this on every keystroke would destroy and rebuild it under the caret.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [file, path])

  // Decisions made by button have to reach the centre editor, which owns its document.
  useEffect(() => {
    const view = views.current['result']
    if (!view) return
    const held = view.state.doc.toString()
    if (held === result) return
    view.dispatch({ changes: { from: 0, to: held.length, insert: result } })
  }, [result])

  /*
   * Repaint all three panes, and rebuild the gutters, whenever anything moves.
   *
   * After the effect above, so the centre pane's spans are measured against the text it now
   * holds rather than the one it held a tick ago — a repaint against a stale document is the
   * range error `paintedField`'s header is about.
   */
  useEffect(() => {
    if (file === null) return
    const centre = views.current['result']
    if (centre) {
      paint(
        centre,
        resultSpans(doc, decisions).flatMap((span) => {
          const region = doc.regions.find((r) => r.id === span.id)
          if (region === undefined) return []
          // `null` is *settled*. A highlight is work you have not done, so an answered block goes
          // dark here as it does in the side panes — the text it left behind is the record.
          const tone = regionTone(region, decisionOf(decisions, span.id))
          if (tone === null) return []
          return [{ from: span.from, to: span.to, tone, current: span.id === at }]
        }),
        // The word-level detail, on the blocks that are still lit, against the base each replaced.
        resultSpans(doc, decisions).flatMap((span) => {
          const region = doc.regions.find((r) => r.id === span.id)
          if (region === undefined) return []
          if (regionTone(region, decisionOf(decisions, span.id)) === null) return []
          const text = regionText(doc, region, decisionOf(decisions, span.id))
          return inlineDiff(doc.base.slice(region.baseFrom, region.baseTo), text, span.from).b
        }),
      )
    }
    for (const side of ['ours', 'theirs'] as const) {
      const view = views.current[side]
      if (!view) continue
      const spans = sideSpans(doc, side)
      /*
       * A band per block this side has **not answered yet**, and the word-level detail inside it.
       *
       * Per *side*, not per region, and that is the point of asking `pending` here rather than
       * `settled`: answering the left half of a conflict quietens the left pane while the right
       * stays lit, which is what tells you the half that is left.
       *
       * A block this side did not change is not painted either — it has no say there, and a band
       * would claim it had done something.
       */
      const lit = spans.filter((span) => {
        const region = doc.regions.find((r) => r.id === span.id)
        return region !== undefined && pending(region, decisionOf(decisions, span.id), side)
      })
      const marks = lit.flatMap((span) => {
        const region = doc.regions.find((r) => r.id === span.id)
        if (region === undefined) return []
        const mine = side === 'ours' ? region.ours : region.theirs
        return inlineDiff(doc.base.slice(region.baseFrom, region.baseTo), mine, span.from).b
      })
      paint(
        view,
        lit.flatMap((span) => {
          const region = doc.regions.find((r) => r.id === span.id)
          if (region === undefined) return []
          const tone = regionTone(region, decisionOf(decisions, span.id))
          if (tone === null) return []
          return [{ from: span.from, to: span.to, tone, current: span.id === at }]
        }),
        marks,
      )

      /*
       * A chevron only where this side still has something to say. `pending` is false once the
       * side has been accepted **or** ignored, so the buttons go away when they are used — which
       * is IDEA's behaviour and the fix for the complaint that a used chevron looked like it had
       * not worked.
       */
      setGutter(
        view,
        side,
        spans.flatMap((span) => {
          const region = doc.regions.find((r) => r.id === span.id)
          if (region === undefined) return []
          const held = decisionOf(decisions, span.id)
          const open = pending(region, held, side)
          const done = answered(region, held, side)
          // Nothing at all for a side that did not change this block — it has no say here, and a
          // button would invite a decision about somebody else's edit.
          if (!open && !done) return []
          /*
           * A side that **deleted** the block still gets a chevron.
           *
           * Its span is zero-width — there are no lines of its own to point at — and the first
           * version skipped it, which left "take the side that deleted this" reachable only
           * through a button in the pane heading. The chevron goes on the line the deletion
           * happened *at*, which is the line that closed over the gap; `setGutter` clamps it
           * into the document. A button on the line where something is missing is a great deal
           * better than no button at all.
           */
          return [{ id: span.id, line: span.from, answered: done }]
        }),
        accessors.current[side],
      )
    }
  }, [doc, decisions, at, file, result])

  /*
   * Keep the three panes on the same part of the file.
   *
   * # Anchored on blocks, not on pixels
   *
   * Scrolling the others to the same fraction is the obvious version and it drifts: the three
   * documents are different lengths, so 40% down `ours` is not 40% down the result, and the
   * mismatch grows with every accepted block. This maps the **line at the top of the pane that
   * moved** onto the corresponding line in each other pane, interpolating between the two
   * nearest block boundaries — which are the only positions the three documents agree about.
   *
   * `syncing` is what stops the answer bouncing back: setting another pane's scroll fires its own
   * handler, which would set this one's, which would set the others'.
   */
  /**
   * Which panes are about to receive a scroll we caused.
   *
   * A time-based guard was the first version and it could not work. Assigning `scrollTop` fires
   * that pane's own `scroll` event **asynchronously** — a frame later, or several under load —
   * so a flag released on the next `requestAnimationFrame` was routinely already down when the
   * echo arrived. Each echo then moved the other two, and the whole thing converged on line one:
   * exactly the *"scrolling takes me back to the top"* that was reported.
   *
   * A set of marks cannot drift like that. A pane whose mark is present consumes it and does
   * nothing; the mark is only laid when the value genuinely changes, so it cannot be left behind
   * to swallow a real scroll later.
   */
  const echoes = useRef<Set<string>>(new Set())
  useEffect(() => {
    if (file === null || file.binary || file.tooLarge !== null) return
    const spans: Record<string, Span[]> = {
      ours: sideSpans(doc, 'ours'),
      theirs: sideSpans(doc, 'theirs'),
      result: resultSpans(doc, decisions),
    }

    /** The line in `to` that corresponds to `line` in `from`. */
    const map = (from: string, to: string, line: number): number => {
      const a = spans[from] ?? []
      const b = spans[to] ?? []
      let lastA = 0
      let lastB = 0
      for (let i = 0; i < a.length && i < b.length; i += 1) {
        const sa = a[i]
        const sb = b[i]
        if (sa === undefined || sb === undefined) break
        // Above this block: the run in between is identical in both, so the offset carries.
        if (sa.from > line) return lastB + (line - lastA)
        lastA = sa.to
        lastB = sb.to
      }
      return lastB + (line - lastA)
    }

    /**
     * The document-relative height currently at the top of a pane's viewport.
     *
     * `scrollTop` is **not** that number, which is the other half of the bug. CodeMirror measures
     * blocks from the top of the *document*, and the document sits at `view.documentTop` in
     * screen coordinates — a value that already accounts for the scroll, the editor's padding and
     * anything the pane put above it. The difference between the scroller's own top edge and that
     * is the only correct conversion, and reading `scrollTop` instead was off by the padding on
     * every pane and by a whole heading on some.
     */
    const topHeight = (view: EditorView) =>
      view.scrollDOM.getBoundingClientRect().top - view.documentTop

    const listeners: (() => void)[] = []
    for (const from of ['ours', 'result', 'theirs'] as const) {
      const view = views.current[from]
      if (!view) continue
      const onScroll = () => {
        // Our own doing. Consume the mark and stop, or the echo becomes a loop.
        if (echoes.current.delete(from)) return
        const pos = view.lineBlockAtHeight(topHeight(view)).from
        const line = view.state.doc.lineAt(pos).number - 1
        for (const to of ['ours', 'result', 'theirs'] as const) {
          if (to === from) continue
          const other = views.current[to]
          if (!other) continue
          const target = Math.min(Math.max(map(from, to, line) + 1, 1), other.state.doc.lines)
          const want = other.lineBlockAt(other.state.doc.line(target).from).top
          // A delta, not an absolute: it needs no assumption about where the document begins
          // inside the scroller, which is the assumption that was wrong.
          const delta = want - topHeight(other)
          if (Math.abs(delta) < 1) continue
          echoes.current.add(to)
          other.scrollDOM.scrollTop += delta
        }
      }
      view.scrollDOM.addEventListener('scroll', onScroll, { passive: true })
      listeners.push(() => view.scrollDOM.removeEventListener('scroll', onScroll))
    }
    return () => {
      for (const off of listeners) off()
      echoes.current.clear()
    }
  }, [doc, decisions, file])

  /*
   * Ctrl+Z / Ctrl+Shift+Z over the block decisions.
   *
   * Bound on the pane's own wrapper rather than inside any editor: the centre pane's CodeMirror
   * history owns typing, and it must go on owning it — a Ctrl+Z with the caret in the result
   * should undo the character you just typed, not the block you accepted five minutes ago. So
   * this listens on the pane and steps out of the way whenever the event came from inside an
   * editable surface.
   *
   * That handoff was to nobody until M17. This file installed no `history()`, so stepping aside
   * handed the chord to a `StateField` that did not exist and Ctrl+Z in the result pane did
   * nothing whatsoever — the sentence above was true about the intent and false about the code
   * for as long as the pane has shipped. The extension is installed on the editable side now
   * (see `mountEditors`' `extensions`), which is what makes this paragraph describe the
   * behaviour rather than the plan.
   */
  const onKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const key = event.key.toLowerCase()
    if (!(event.ctrlKey || event.metaKey) || key !== 'z') return
    const target = event.target as HTMLElement | null
    if (target?.closest('[contenteditable="true"]') !== null && target !== null) return
    const redo = event.shiftKey
    const from = redo ? history.current.future : history.current.past
    if (from.length === 0) return
    event.preventDefault()
    setDecisions((held) => {
      const next = from.pop()
      if (next === undefined) return held
      ;(redo ? history.current.past : history.current.future).push(held)
      return next
    })
    setEdited(null)
  }

  const apply = () => {
    if (!canApply(result, doc, decisions)) return
    setBusy(true)
    void branchApi
      .conflictResolve(project, repo, path, result)
      .then(async () => {
        notify(`Resolved ${path}`, { kind: 'ok' })
        void fileApi.setDirty(project, tab, false).catch(() => {})
        await tabApi.close(project, tab, true).catch(() => {})
        // Back to the list, with this row ticked — or the merge commits itself if that was the
        // last one. See `afterResolve`.
        await afterResolve(project, repo, '')
      })
      .catch((error: unknown) => notify(explain(error), { kind: 'error' }))
      .finally(() => setBusy(false))
  }

  return (
    <MergePaneView
      onKeyDown={onKeyDown}
      path={path}
      file={file}
      unavailable={unavailable}
      result={result}
      doc={doc}
      decisions={decisions}
      currentId={at}
      busy={busy}
      onTake={(side) => {
        if (at === null) return
        setEdited(null)
        remember((d) => ({ ...d, [at]: accept(decisionOf(d, at), side) }))
      }}
      onReset={() => {
        if (at === null) return
        setEdited(null)
        remember((d) => ({ ...d, [at]: reset() }))
      }}
      onApplyNonConflicting={() => {
        setEdited(null)
        remember((d) => applyNonConflicting(doc, d))
      }}
      onResolveSimple={() => {
        setEdited(null)
        remember((d) => resolveSimple(doc, d))
      }}
      onStep={step}
      onApply={apply}
      onAbandon={() => {
        void (async () => {
          await tabApi.close(project, tab, true).catch(() => {})
          // Closing without answering is not abandoning the merge — the repository is still
          // mid-operation, and leaving nothing on screen saying so is how somebody ends up
          // wondering why every other git action refuses.
          await afterResolve(project, repo, '')
        })()
      }}
      mountEditors={mountEditors}
    />
  )
}

/**
 * A byte count as whole kilobytes.
 *
 * `number | bigint`, because ts-rs maps Rust's `u64` to `bigint` while `serde_json` puts a plain
 * JSON number on the wire — so which of the two arrives depends on whether anything in the
 * transport ever grows a reviver. `branchModel`'s own `bytes()` helper makes the same allowance
 * for the same field type, and accepting both costs one line.
 */
function kb(value: number | bigint | null | undefined): number {
  const n = typeof value === 'bigint' ? Number(value) : (value ?? 0)
  return Math.round(n / 1024)
}
