/**
 * A git diff, with the per-hunk and per-line gestures M10's staging engine was built for.
 *
 * `cide-git` has supported `Selection::Hunks` and `Selection::Lines` since M10 and nothing
 * could produce either: `wholeFiles()` in `useGitPanel` was the only selection the UI could
 * build, so the headline feature was reachable by no gesture at all. This pane is the
 * gesture.
 *
 * # Two components, for the reason `DiffPane` and `ClaudeDiffPane` are two
 *
 * {@link GitDiffView} is pure: a diff, a selection, and callbacks. It talks to nothing, so a
 * fixture can render it under node — which is what `ui/scripts/check-diff-render.mjs` does,
 * and how the claim below is checked against real markup rather than against intent.
 * {@link GitDiffPane} is the wiring: it fetches, holds the selection, and turns a button into
 * a command.
 *
 * # What it is not
 *
 * Not `DiffPane`. That one is a `@codemirror/merge` view over two whole documents, built to
 * *answer* a Claude proposal (accept / accept as proposed / reject), and its unit of
 * interaction is the document. This one is a unified diff whose unit of interaction is the
 * line, because that is what a `Selection` names. Sharing a component between them would mean
 * a merge view that also has line checkboxes, and a `MergeView` chunk is not a
 * `DiffHunkView` — mapping between the two is exactly the kind of index arithmetic that
 * stages the wrong line.
 *
 * # The rule this pane exists to honour
 *
 * **What is highlighted is what is sent.** The painted set comes from
 * `diffSelection.highlightedKeys` and the wire selection from `diffSelection.toSelection`;
 * `check-diff-selection.mjs` proves those two denote the same positions over every subset of
 * a sample diff, and `check-diff-render.mjs` proves the rows this component actually marks
 * are that same set.
 *
 * # Sides are not interchangeable
 *
 * `cide_git` re-derives the diff before applying a selection, and *which* diff depends on the
 * operation: `stage` uses index→worktree, `unstage` uses HEAD→index, `commit` and `shelve`
 * use HEAD→worktree. A set of positions in one of those is a different set in another, so
 * each side offers only the operation it is valid for, and switching sides clears the
 * selection rather than carrying it across.
 */
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from 'react'
import {
  countLines,
  hunkMarks,
  hunkState,
  highlightedKeys,
  mark,
  pathSelection,
  selectEverything,
  toSelection,
  toggleHunk,
  toggleLine,
  type Marks,
} from '@/sidebar/GitPanel/diffSelection'
import {
  clearPartial,
  getServerSnapshot,
  getSnapshot,
  setPartial,
  subscribe,
} from '@/sidebar/GitPanel/partialStore'
import {
  diag,
  events,
  git as gitApi,
  type DiffSide,
  type DiffSpec,
  type FileDiff,
  type LineOrigin,
  type ProjectId,
  type RepoId,
} from '@/ipc/client'
import { markDiffPaneAvailable } from '@/sidebar/GitPanel/diffHost'
import { noteRepoRoots, repoRoot, touchesFile } from '@/sidebar/GitPanel/repoRoots'
import styles from './GitDiffPane.module.css'

/*
 * Announced at import time: the git panel refuses to open a diff tab in a build whose shell
 * never linked this module, because the tab would draw as an empty pane with nothing to say
 * why. See `sidebar/GitPanel/diffHost.ts`.
 */
markDiffPaneAvailable()

/**
 * Above this many rows the hunks open collapsed.
 *
 * There is no virtualisation here on purpose — a unified diff is a list of rows on a shared
 * column grid, and windowing it would break both find-in-page and a text selection dragged
 * across hunks. The cap is what keeps a 40,000-line refactor from laying out 40,000 nodes
 * before the first frame; every hunk still has its header, so nothing is hidden, only folded.
 */
const COLLAPSE_ABOVE = 2_000

/** Which operation a side can perform. The other two would be refused, so they are absent. */
export type DiffOp = 'stage' | 'unstage' | 'commit'

export function opFor(side: DiffSide): DiffOp {
  switch (side) {
    case 'unstaged':
      return 'stage'
    case 'staged':
      return 'unstage'
    case 'combined':
      return 'commit'
  }
}

/**
 * The row's tint class.
 *
 * A switch rather than `styles[line.origin]`: CSS Modules types its default export as an
 * index signature, so a typo would compile and paint `undefined` into the class list.
 */
function rowClass(origin: LineOrigin): string {
  switch (origin) {
    case 'addition':
      return styles.addition ?? ''
    case 'deletion':
      return styles.deletion ?? ''
    case 'context':
      return styles.context ?? ''
  }
}

const SIDES: ReadonlyArray<{ side: DiffSide; label: string; title: string }> = [
  { side: 'unstaged', label: 'Unstaged', title: 'index → working tree. Staging acts on this.' },
  { side: 'staged', label: 'Staged', title: 'HEAD → index. Unstaging acts on this.' },
  { side: 'combined', label: 'All', title: 'HEAD → working tree. What a commit selects from.' },
]

// --- the view ---------------------------------------------------------------------------

export interface GitDiffViewProps {
  /** `null` while the fetch is in flight or after it failed; the chrome still renders. */
  diff: FileDiff | null
  /** Shown in the header before the first diff arrives. */
  path: string
  side: DiffSide
  marks: Marks
  collapsed: ReadonlySet<number>
  busy: boolean
  /** One line under the header: a refusal, or why the selection was dropped. */
  note: string | null
  /** Why there is no diff, when there is none. */
  reason: string | null
  /** Lines of this file already held for the next commit, if any. */
  held: number | null
  onSide: (side: DiffSide) => void
  onMarks: (marks: Marks) => void
  onCollapse: (hunk: number) => void
  /** Apply these marks: stage, unstage, or hold for the commit — whatever the side means. */
  onApply: (marks: Marks) => void
  onDropHeld: () => void
}

/**
 * The diff, its gutters, and the three buttons. Pure.
 *
 * Every "is this row selected" answer comes from `highlightedKeys`, computed once for the
 * whole render — never from `marks.has(...)` inline. That is the difference between a
 * component that agrees with the wire selection and one that merely usually does: the wire
 * selection is derived from the same function, so a row painted here and a position sent to
 * Rust cannot come apart without one of them throwing.
 */
export function GitDiffView({
  diff,
  path,
  side,
  marks,
  collapsed,
  busy,
  note,
  reason,
  held,
  onSide,
  onMarks,
  onCollapse,
  onApply,
  onDropHeld,
}: GitDiffViewProps): ReactNode {
  const op = opFor(side)
  const painted = diff === null ? new Set<string>() : highlightedKeys(marks, diff)

  /*
   * The header is drawn whether or not there is a diff, and the side switcher with it.
   *
   * `git_diff_file` answers `NoSuchChange` for a file with nothing on the side being asked
   * about — a file with no staged changes, viewed on `Staged`, is the ordinary case — and a
   * failure view without the switcher is a pane the user cannot get out of except by closing
   * the tab and opening it again from the panel.
   */
  const header = (
    <header className={styles.header}>
      <span className={styles.path}>{diff?.path ?? path}</span>
      {diff?.oldPath != null && (
        <>
          <span className={styles.arrow}>←</span>
          <span className={styles.path}>{diff.oldPath}</span>
        </>
      )}
      {diff !== null && <span className={styles.status}>{diff.status}</span>}
      <div className={styles.sides} role="group" aria-label="Diff side">
        {SIDES.map((entry) => (
          <button
            key={entry.side}
            type="button"
            title={entry.title}
            aria-pressed={side === entry.side}
            className={side === entry.side ? `${styles.side} ${styles.sideOn}` : styles.side}
            onClick={() => onSide(entry.side)}
          >
            {entry.label}
          </button>
        ))}
      </div>
    </header>
  )

  if (diff === null) {
    return (
      <div className={styles.pane} data-audit="gitDiffPane">
        {header}
        <div className={styles.notice}>
          {reason === null ? (
            'Reading the diff…'
          ) : (
            <>
              <div>Nothing to show on this side.</div>
              <div className={styles.noticeWhy}>{reason}</div>
            </>
          )}
        </div>
      </div>
    )
  }

  const selectable = diff.partialOk && diff.hunks.length > 0
  const selection = toSelection(marks, diff)
  const totalChanges = diff.hunks.reduce(
    (n, hunk) => n + hunk.lines.filter((l) => l.origin !== 'context').length,
    0,
  )
  const actionLabel =
    op === 'stage' ? 'Stage selection' : op === 'unstage' ? 'Unstage selection' : 'Use for commit'

  return (
    <div className={styles.pane} data-audit="gitDiffPane">
      {header}

      {note !== null && (
        <p className={styles.note} data-audit="gitDiffNote">
          {note}
        </p>
      )}
      {!diff.partialOk && (
        <p className={styles.note} data-audit="gitDiffWhole">
          This file can only be staged whole — it is binary, a submodule, a symlink, a deletion
          or a rename. Tick it in the panel instead.
        </p>
      )}
      {held !== null && (
        <p className={styles.note} data-audit="gitDiffHeld">
          {held} line{held === 1 ? '' : 's'} of this file are held for the next commit.{' '}
          <button type="button" className={styles.link} onClick={onDropHeld}>
            Commit the whole file instead
          </button>
        </p>
      )}

      <div className={styles.body}>
        {diff.hunks.length === 0 && <p className={styles.notice}>No text changes on this side.</p>}
        {diff.hunks.map((hunk) => {
          const state = hunkState(marks, diff, hunk.index)
          const shut = collapsed.has(hunk.index)
          return (
            <section key={hunk.index} className={styles.hunk}>
              <div className={styles.hunkHeader}>
                {selectable && (
                  <button
                    type="button"
                    role="checkbox"
                    aria-checked={state === 'all' ? true : state === 'some' ? 'mixed' : false}
                    aria-label={`Select hunk ${hunk.index + 1}`}
                    className={styles.box}
                    data-audit="gitDiffHunkBox"
                    data-state={state}
                    onClick={() => onMarks(toggleHunk(marks, diff, hunk.index))}
                  >
                    {state === 'all' ? '✓' : state === 'some' ? '–' : ''}
                  </button>
                )}
                <button
                  type="button"
                  className={styles.hunkTitle}
                  aria-expanded={!shut}
                  onClick={() => onCollapse(hunk.index)}
                >
                  <span className={styles.caret}>{shut ? '▸' : '▾'}</span>
                  {hunk.header}
                </button>
                {selectable && op !== 'commit' && (
                  <button
                    type="button"
                    className={styles.hunkAction}
                    disabled={busy}
                    // The whole hunk, whatever is ticked — the gesture people expect from a
                    // hunk header, and the shortest path to `Selection::Hunks`. The same
                    // `hunkMarks` the checkbox uses, so the two cannot disagree about what
                    // "this hunk" means.
                    onClick={() => onApply(new Set(hunkMarks(diff, hunk.index)))}
                  >
                    {op === 'stage' ? 'Stage hunk' : 'Unstage hunk'}
                  </button>
                )}
              </div>

              {!shut && (
                <div className={styles.lines}>
                  {hunk.lines.map((line, index) => {
                    const change = line.origin !== 'context'
                    const on = painted.has(mark(hunk.index, index))
                    return (
                      <div
                        key={index}
                        className={`${styles.row} ${rowClass(line.origin)}`}
                        data-audit="gitDiffRow"
                        data-at={`${hunk.index}:${index}`}
                        data-selected={on ? 'true' : 'false'}
                      >
                        {selectable && (
                          <button
                            type="button"
                            role="checkbox"
                            aria-checked={on}
                            aria-label={`Select line ${line.newLineno ?? line.oldLineno ?? ''}`}
                            className={styles.lineBox}
                            disabled={!change}
                            onClick={() => onMarks(toggleLine(marks, diff, hunk.index, index))}
                          >
                            {on ? '✓' : ''}
                          </button>
                        )}
                        <span className={styles.lineno}>{line.oldLineno ?? ''}</span>
                        <span className={styles.lineno}>{line.newLineno ?? ''}</span>
                        <span className={styles.sign}>
                          {line.origin === 'addition' ? '+' : line.origin === 'deletion' ? '-' : ' '}
                        </span>
                        <span className={styles.text}>
                          {line.content}
                          {line.noNewline && (
                            <span className={styles.noNewline}> ⏎ no newline at end of file</span>
                          )}
                        </span>
                      </div>
                    )
                  })}
                </div>
              )}
            </section>
          )
        })}
      </div>

      <footer className={styles.actions}>
        <span className={styles.count} data-audit="gitDiffCount">
          {painted.size} of {totalChanges} line{totalChanges === 1 ? '' : 's'} selected
          {selection !== null && <span className={styles.kind}> · {selection.kind}</span>}
        </span>
        <button
          type="button"
          className={styles.button}
          disabled={!selectable || busy}
          onClick={() => onMarks(selectEverything(diff))}
        >
          Select all
        </button>
        <button
          type="button"
          className={styles.button}
          disabled={painted.size === 0 || busy}
          onClick={() => onMarks(new Set<string>())}
        >
          Clear
        </button>
        <button
          type="button"
          className={`${styles.button} ${styles.primary}`}
          data-audit="gitDiffApply"
          disabled={selection === null || busy}
          onClick={() => onApply(marks)}
        >
          {actionLabel}
        </button>
      </footer>
    </div>
  )
}

// --- the wiring -------------------------------------------------------------------------

export interface GitDiffPaneProps {
  project: ProjectId
  /** The tab's spec. Only a `git` origin renders here; a Claude one belongs to `DiffPane`. */
  spec: DiffSpec
  /**
   * Whether this tab is the one on screen.
   *
   * `TabContent` keeps every tab of the project mounted and hides all but one with
   * `visibility: hidden`, so the DOM alone cannot answer this — it is the same flag its
   * `renderTree` already hands the WebGL pool and the focus restorer. Omitting it is safe and
   * means "assume visible": the pane then refetches the moment its file changes, which is
   * what it did before this argument existed.
   */
  visible?: boolean
}

export function GitDiffPane({ project, spec, visible = true }: GitDiffPaneProps): ReactNode {
  if (spec.origin.kind !== 'git') {
    // Not reachable through `tab_open_diff`, which only ever writes a git origin. Said out
    // loud rather than rendered blank so a mis-wired host sees why nothing is here.
    return <div className={styles.notice}>This tab is not a git diff.</div>
  }
  const { repo, path, side } = spec.origin
  // Keyed on the file, so a tab that comes to show a different diff re-derives everything
  // rather than painting the previous file's selection against the new file's line numbers.
  return (
    <GitDiff
      key={`${repo} ${path}`}
      project={project}
      repo={repo}
      path={path}
      from={side}
      visible={visible}
    />
  )
}

interface GitDiffProps {
  project: ProjectId
  repo: RepoId
  path: string
  /** The side the tab was opened on. The pane owns it from here. */
  from: DiffSide
  visible: boolean
}

function GitDiff({ project, repo, path, from, visible }: GitDiffProps): ReactNode {
  const [side, setSide] = useState<DiffSide>(from)
  const [diff, setDiff] = useState<FileDiff | null>(null)
  const [reason, setReason] = useState<string | null>(null)
  const [marks, setMarks] = useState<Marks>(() => new Set<string>())
  const [collapsed, setCollapsed] = useState<ReadonlySet<number>>(() => new Set<number>())
  const [note, setNote] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  /** Bumped to re-run the fetch; a git mutation anywhere invalidates this view. */
  const [nonce, setNonce] = useState(0)
  /** Something invalidated this view while the tab was behind another one. See below. */
  const [stale, setStale] = useState(false)

  // Read through refs inside the fetch so a re-render caused by a tick cannot re-run it, and
  // so the staleness check can see the marks without depending on them.
  const marksRef = useRef<Marks>(marks)
  marksRef.current = marks
  const revRef = useRef<string | null>(null)
  // The invalidation listeners below are subscribed once per file and must not be torn down
  // and rebuilt every time the user switches tabs, so visibility reaches them through a ref.
  const visibleRef = useRef(visible)
  visibleRef.current = visible

  const partials = useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot)
  const held = useMemo(
    () => partials.find((p) => p.repo === repo && p.path === path)?.lines ?? null,
    [partials, repo, path],
  )

  useEffect(() => {
    let disposed = false
    gitApi
      .diffFile(project, repo, path, side)
      .then((fresh) => {
        if (disposed) return
        setDiff(fresh)
        setReason(null)
        /*
         * A moved diff drops the selection.
         *
         * The positions in `marks` index *this* diff. When the file changes underneath —
         * Claude edits it, a build regenerates it, the user saves — the new diff has
         * different hunks and the same indices name different lines. Carrying them over is
         * precisely the silent wrong answer `FileDiff.rev` exists to catch, so they go, and
         * the pane says so. Re-deriving them by content matching would be a guess on the one
         * code path where a guess destroys work.
         */
        if (revRef.current !== null && revRef.current !== fresh.rev) {
          if (marksRef.current.size > 0) {
            setNote('The file changed while it was open, so the selection was cleared.')
          }
          setMarks(new Set<string>())
        }
        revRef.current = fresh.rev
        const rows = fresh.hunks.reduce((n, hunk) => n + hunk.lines.length, 0)
        if (rows > COLLAPSE_ABOVE) setCollapsed(new Set(fresh.hunks.map((h) => h.index)))
      })
      .catch((e: unknown) => {
        if (disposed) return
        const detail = e instanceof Error ? e.message : String(e)
        // `NoSuchChange` is the ordinary end of a diff's life: the change was committed,
        // reverted or staged away. Not a dialog — the side simply has nothing to show, and
        // the switcher above is still there to look at another one.
        setDiff(null)
        setReason(detail)
        void diag.log(`git diff pane: ${path} on ${side} failed: ${detail}`)
      })
    return () => {
      disposed = true
    }
  }, [project, repo, path, side, nonce])

  /*
   * What invalidates this view, and what does not.
   *
   * A mutation cide made anywhere invalidates it — including one made in the panel beside it,
   * or in another window. Tool events cover the case with no cide mutation at all: an agent
   * writing the file this tab is showing.
   *
   * # Both filters are load-bearing
   *
   * `cide://session-tool` is emitted with `AppHandle::emit`, which reaches **every window**,
   * and it names a session rather than a project. So this handler hears every tool call every
   * Claude in the app makes, in any project. `TabContent` keeps every tab of the project
   * mounted at once — that is what makes switching tabs free — so an unfiltered bump meant N
   * open diff tabs each issuing a `git_diff_file` on every tool call anywhere, and all but at
   * most one of those tabs was behind another one.
   *
   * The event names the paths it touched, which is the filter: a tool call is only this tab's
   * business if it wrote *this tab's file*. Project scoping falls out of the same comparison
   * rather than needing a `project` field on the event, because an absolute path under another
   * project's root is not this file — see `repoRoots.touchesFile`.
   *
   * `cide://git-status` already carries a project and stays filtered on it alone. It is not
   * narrowed to the path as well: it fires once per cide mutation rather than once per tool
   * call, so it is not the storm, and staging a *neighbouring* file does move this view — the
   * `held` line and the side switcher both describe an index that just changed.
   *
   * # Hidden tabs defer rather than skip
   *
   * A hidden tab records that it is stale and fetches when it is next revealed, once, however
   * many events went by. It deliberately does not refetch on *every* reveal: the whole reason
   * `TabContent` keeps tabs mounted is that coming back to one is instant, and a tab that
   * re-reads a 4,000-line diff every time it is looked at would trade the storm for a stutter
   * on a gesture people make constantly. So the cost is paid exactly when the file it is
   * showing actually moved, and a tab nobody touched draws from the state it already has.
   *
   * The one thing this gives up is a hidden tab that is *watched* rather than looked at —
   * split off into its own window, say. That case is not reachable today: a detached window
   * hosts a pane, not a tab, and its tab is by definition the visible one in its own shell.
   */
  useEffect(() => {
    let gone = false
    let timer: number | null = null
    const unlisten: Array<() => void> = []
    const bump = () => {
      if (timer !== null) window.clearTimeout(timer)
      // One edit reports several paths and a `git_status` broadcast follows every mutation, so
      // the triggers arrive together; coalescing them is one fetch instead of three. Which of
      // the two this becomes is decided when the timer fires, not when it is set: a tab
      // revealed inside the window fetches straight away rather than waiting to be revealed
      // again.
      timer = window.setTimeout(() => {
        if (visibleRef.current) setNonce((n) => n + 1)
        else setStale(true)
      }, 120)
    }
    const track = (p: Promise<() => void>) => {
      void p
        .then((fn) => {
          if (gone) fn()
          else unlisten.push(fn)
        })
        .catch((e: unknown) => diag.log(`git diff pane: events unavailable: ${String(e)}`))
    }
    track(
      events.onGitStatus((forProject, tree) => {
        // Noted whoever it is for: the roots in another project's tree are still true, and a
        // tab in this window may be showing one of those repos after a project switch.
        noteRepoRoots(tree)
        if (forProject === project) bump()
      }),
    )
    track(
      events.onSessionTool((_session, paths) => {
        if (touchesFile(paths, repoRoot(repo), path)) bump()
      }),
    )
    return () => {
      gone = true
      if (timer !== null) window.clearTimeout(timer)
      for (const fn of unlisten) fn()
    }
  }, [project, repo, path])

  // Reveal is where a deferred fetch is spent. The flag is cleared by the reveal and not by
  // the fetch's outcome: a file staged away answers `NoSuchChange` for as long as it stays
  // that way, and a flag that only cleared on success would re-arm every render into a loop.
  useEffect(() => {
    if (!visible || !stale) return
    setStale(false)
    setNonce((n) => n + 1)
  }, [visible, stale])

  const switchSide = useCallback((next: DiffSide) => {
    // Positions do not transfer between sides; see the module comment.
    setMarks(new Set<string>())
    setNote(null)
    revRef.current = null
    setSide(next)
  }, [])

  const run = useCallback(async (what: string, call: () => Promise<unknown>) => {
    setBusy(true)
    try {
      await call()
      setNote(null)
      setMarks(new Set<string>())
      setNonce((n) => n + 1)
    } catch (e: unknown) {
      const detail = e instanceof Error ? e.message : String(e)
      // Every refusal lands here, and the two that matter read plainly: `staleSelection`
      // means the file moved and *nothing was applied*; `partialRefused` means this file can
      // only go through the index whole.
      setNote(`${what} refused — ${detail}`)
      void diag.log(`git diff pane: ${what} failed: ${detail}`)
    } finally {
      setBusy(false)
    }
  }, [])

  const apply = useCallback(
    (chosen: Marks) => {
      if (diff === null) return
      const selection = pathSelection(chosen, diff)
      if (selection === null) return
      if (opFor(side) === 'stage') {
        void run('stage', () => gitApi.stage(project, repo, [selection]))
        return
      }
      if (opFor(side) === 'unstage') {
        void run('unstage', () => gitApi.unstage(project, repo, [selection]))
        return
      }
      /*
       * Commit does not happen here. The panel owns the message box and the changelist, so
       * this hands the selection over and the next Commit uses it — see `partialStore`.
       *
       * "Everything" is stored as *nothing*: a `whole` selection is what a ticked row already
       * means, and an entry for it would put the file in the panel's "will be committed in
       * part" warning while committing all of it — a warning that is false is worse than no
       * warning. It would also attach a rev to a selection that names no positions, so a
       * later unrelated edit would have the commit refused for no reason at all.
       */
      if (selection.selection.kind === 'whole') {
        clearPartial(repo, diff.path)
        setNote('The whole file will be committed; nothing is held back.')
        return
      }
      setPartial({
        repo,
        path: diff.path,
        side: diff.side,
        rev: diff.rev,
        selection: selection.selection,
        lines: countLines(diff, selection.selection),
      })
      setNote(null)
    },
    [diff, side, project, repo, run],
  )

  return (
    <GitDiffView
      diff={diff}
      path={path}
      side={side}
      marks={marks}
      collapsed={collapsed}
      busy={busy}
      note={note}
      reason={reason}
      held={held}
      onSide={switchSide}
      onMarks={setMarks}
      onCollapse={(hunk) =>
        setCollapsed((prev) => {
          const next = new Set(prev)
          if (!next.delete(hunk)) next.add(hunk)
          return next
        })
      }
      onApply={apply}
      onDropHeld={() => clearPartial(repo, path)}
    />
  )
}
