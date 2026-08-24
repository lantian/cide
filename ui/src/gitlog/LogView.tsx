/**
 * The commit list and its details pane — the Log tab's body, and a History tab's. (M19)
 *
 * Pure, in the sense `sidebar/GitPanel/GitPanel.tsx` is: every value is a prop, it reads no store
 * and calls no IPC, and nothing in its import graph touches `document` at module scope, so a
 * check script can server-render it. `LogTab.tsx` is the wiring.
 *
 * One component for both tabs, because the two differ by exactly one field of the query — the
 * path. Two lists of commits with two filter bars is how they drift into two answers for "which
 * commits touched this".
 *
 * # The filter bar is a controlled view of a value the host owns
 *
 * Nothing here holds a filter. The three controls read `filter` and call `onFilter` with the
 * whole next value, and the debounce, the local narrow and the wire request all happen in
 * `LogTab`. Keeping the state out is what lets this file be server-rendered against ten fixed
 * stories, and it is also the only arrangement in which the box and the request cannot disagree
 * about what is currently being asked — a box with its own `useState` would be the source of
 * truth for one frame per keystroke, which is exactly long enough to paint a stale answer.
 */
import { useEffect, useRef, useState } from 'react'
import type {
  CommitPage,
  CommitRow,
  GraphRow,
  RefChip,
  RepoInfo,
  RevisionChange,
  RevisionRange,
} from '@/ipc/generated'
/*
 * From the component's own module, **not** the `@/icons` barrel.
 *
 * The barrel re-exports `useIconTheme`, which reads the workspace store — and the store's module
 * graph reaches `layout/paneHosts` and therefore xterm, whose addons touch `self` at import time.
 * That is fatal under `check-log-render.mjs`, which loads this file in node: the whole point of
 * keeping `LogView` free of the store is undone by one convenience import. `FileIcon` itself is
 * pure and takes its theme as a prop, which is why the split works at all.
 */
import { FileIcon } from '@/icons/FileIcon'
import type { IconTheme } from '@/icons/iconFor'
import { fileRows, INDENT, type ChangedFile } from './fileRows'
import { isDiffShortcut } from './fileMenu'
import {
  branchOverflowNote,
  branchRows,
  commitChoice,
  moveHighlight,
  type BranchRow,
} from './branchFilter'
import {
  branchFromValue,
  branchValue,
  changeCounts,
  chipTarget,
  chipTitle,
  clearFilters,
  compareTitle,
  isFiltered,
  laneColor,
  missingRefNote,
  moreLabel,
  orderChips,
  RANGE_IDENTICAL,
  rangeTruncatedNote,
  repoChipFor,
  repoStripOn,
  revealFindable,
  revealNote,
  revLabel,
  rowTitle,
  when,
  type LogFilter,
  type LogReveal,
  type SelectMods,
} from './logModel'
import { Icon } from '@/icons/Icon'

import styles from './LogView.module.css'

/**
 * The row context menu, opened and drawn by the host.
 *
 * The same shape and the same reason as `sidebar/GitPanel/GitPanel.tsx`'s [`TreeMenu`]:
 * `useContextMenu` reads the window's live keymap out of `@/store/workspace`, which imports
 * `layout/paneHosts`, which calls `document.createElement` at module scope — so a component that
 * called the hook could not be server-rendered, and `check-log-render.mjs` is the only thing
 * standing between this view and the failure the git panel already had once (compiling, mounting
 * and drawing nothing). The hook therefore lives in `LogTab` and this file takes the handle.
 */
export interface LogRowMenu {
  /** Goes on the scroller, not on each row: the list is one surface and the rows are inside it. */
  onContextMenu: (e: React.MouseEvent) => void
  /** Must be rendered somewhere; it portals to the app root, so where does not matter. */
  menu: React.ReactNode
}

export interface LogViewProps {
  rows: readonly CommitRow[]
  /** Parallel to `rows` when the graph is on, empty when it is off. */
  graph: readonly GraphRow[]
  /** Why the graph is off, when it is and the reason is worth saying. */
  graphOff: string | null
  /** The empty/loading/failed sentence, or `null` when there are rows to show. */
  status: string | null
  page: CommitPage | null
  selected: string | null
  /**
   * The second selected row, or `null` for an ordinary one-commit selection. (M20)
   *
   * A separate prop rather than turning `selected` into an array, and the reason is that the two
   * are not interchangeable: `selected` is the *anchor* — the row a plain click landed on, the row
   * Shift extends from, and the only row the reveal effect below scrolls to. An array would make
   * "which one does a reveal scroll to" a question with no answer in the type, and the answer
   * would end up being `[0]` by accident.
   *
   * Both rows are drawn `aria-selected`, because both of them are: the pane beside the list is
   * showing the range between them, and a list that highlighted one end of what it is showing
   * would be lying about half of it.
   */
  secondary: string | null
  busy: boolean
  now: number
  /** What the three filter controls show. Owned by the host; see the header. */
  filter: LogFilter
  /**
   * The names the branch control offers, deduplicated across every repository in the project.
   *
   * A union and not a per-repository list, because under a merged scope `LogRefs::Branch`
   * resolves per repository and a root that lacks the branch answers `NoSuchRef` — which the
   * note under the list reports by name. Hiding a branch that only one root has would make the
   * monorepo case the filter exists for unreachable.
   */
  branches: readonly string[]
  /** Every repository in the project, in root order. More than one draws the repo strip. */
  repos: readonly RepoInfo[]
  /**
   * What became of a request to reveal one commit — a click on a blame line. (M19)
   *
   * A prop and not something this view derives, for the reason every other value here is one:
   * three of the four outcomes need a round trip (`git_commit_detail`) and one of them needs the
   * loaded page, both of which are `LogTab`'s. What is here is the two things a *view* owes the
   * gesture — the sentence when the commit is not on screen, and the scroll when it is.
   */
  reveal: LogReveal
  /**
   * A click on a row, with the modifiers it carried.
   *
   * The modifiers travel rather than being resolved here, because what they *mean* —
   * Ctrl adds the second endpoint, Shift extends and keeps the ends — is
   * `logModel::selectRow`'s rule and needs the page's order, which this component has as `rows`
   * but the host has as the unnarrowed page. Deciding here would give a different answer while
   * the local text narrow is live, which is the one moment the two lists disagree.
   */
  onSelect: (oid: string, mods: SelectMods) => void
  onMore: () => void
  onFilter: (next: LogFilter) => void
  onRefresh: () => void
  /** *Clear filters and find it* — offered only for a commit known to exist. */
  onFindCommit: () => void
  /** The details pane's body — the message and the changed files. */
  details: React.ReactNode
  /**
   * Absent in a render with no window — the SSR check, and any fixture harness.
   *
   * Optional where the six callbacks above are required, and the asymmetry is deliberate: a
   * missing `onSelect` is a row that has silently stopped being clickable, which is the class of
   * defect `logSmoke.tsx` keeps required props for. A missing menu is a *window* that cannot
   * raise one, which is a real state — `check-log-render.mjs` renders in exactly it.
   */
  rowMenu?: LogRowMenu | undefined
  /**
   * The commit list's share of the panel, in per mille.
   *
   * Per **tab**: `ToolWindowState::log_split` for the Log tab, `HistoryTab::split` for a history
   * one, defaulting to `LOG_SPLIT_DEFAULT` and `LOG_SPLIT_HISTORY`. The two want different
   * numbers because they show different things on the right — a commit's summary against one
   * file's diff — which is the whole argument on the Rust field.
   */
  split: number
}

export function LogView({
  rows,
  graph,
  graphOff,
  status,
  page,
  selected,
  secondary,
  busy,
  now,
  filter,
  branches,
  repos,
  reveal,
  onSelect,
  onMore,
  onFilter,
  onRefresh,
  onFindCommit,
  details,
  rowMenu,
  split,
}: LogViewProps) {
  const filtered = isFiltered(filter)
  const strip = repoStripOn(repos)
  const refNote = missingRefNote(page, filter)
  const note = revealNote(reveal)

  /*
   * Bring the revealed row into view.
   *
   * The one piece of imperative DOM in this file, and it is deliberately here rather than in
   * `LogTab`: the list is this component's markup and the host has no handle on it. It costs the
   * server render nothing — `react-dom/server` runs no effects and never touches the ref — which
   * is what keeps `check-log-render.mjs` working.
   *
   * Keyed on the whole `reveal` object and not on its oid, because clicking the *same* blame line
   * twice is two gestures and must scroll twice: the user may have scrolled away in between, and
   * an oid-keyed effect would silently do nothing the second time. `LogTab` mints a fresh object
   * per request and `NO_REVEAL` is a shared constant, so an untouched tab re-renders without
   * re-running this.
   *
   * `block: 'nearest'` and not `'center'`: a row that is already visible must not move at all.
   * `editor/revealRequest.ts` draws the same distinction between a reveal the user asked for by
   * name and one that merely has to be on screen.
   */
  const revealedRow = useRef<HTMLDivElement | null>(null)
  useEffect(() => {
    if (reveal.kind !== 'found') return
    revealedRow.current?.scrollIntoView({ block: 'nearest' })
  }, [reveal])

  return (
    <div
      className={styles.log}
      data-audit="log"
      /*
       * The divider, as two `fr` units rather than a fixed details width.
       *
       * The column used to be `var(--w-log-details)` — 420px, whatever the tab was for. A
       * History tab's right half is one file's diff, which is the thing the tab exists to show,
       * so it wants half the panel; the Log tab's is a message and a file list beside a wide
       * commit list, which is what 450‰ is. One number, supplied per tab, is what lets both be
       * right — see `HistoryTab::split`.
       *
       * Inline because the value lives in `workspace.json` and a stylesheet cannot read it, the
       * same reason `layout/` writes `gridTemplateColumns` directly. `minmax(0, …)` on both
       * tracks so a long path in either half cannot push the other off the panel.
       */
      style={{ gridTemplateColumns: `minmax(0, ${split}fr) minmax(0, ${1000 - split}fr)` }}
    >
      <div className={styles.pane}>
        <FilterBar
          filter={filter}
          filtered={filtered}
          branches={branches}
          busy={busy}
          onFilter={onFilter}
          onRefresh={onRefresh}
        />
        {/*
         * The context menu is armed on the scroller and not on each row.
         *
         * One listener for a page of rows rather than one per row, and — more importantly — the
         * gesture keeps working in the space *below* the last row: `useContextMenu` calls `items`
         * with the element under the pointer, the host walks up to the nearest `[data-row-id]`,
         * finds none, and returns `[]`, which declines to open at all. An empty box at the
         * pointer reads as a broken surface; no box reads as nothing to offer here.
         */}
        <div
          className={styles.list}
          role="listbox"
          aria-label="Commits"
          tabIndex={0}
          onContextMenu={rowMenu?.onContextMenu}
        >
          {status !== null && (
            <div className={styles.status} data-audit="logStatus">
              <span data-audit="logStatusText">{status}</span>
              {/*
               * The empty state carries its own way out. A user who has narrowed the list to
               * nothing is looking at the sentence, not at the bar above it, and making them
               * find the control that caused it is the difference between "no results" and
               * "this thing is broken".
               */}
              {filtered && (
                <button
                  type="button"
                  className={styles.inlineClear}
                  data-audit="logClearEmpty"
                  onClick={() => onFilter(clearFilters(filter))}
                >
                  Clear filters
                </button>
              )}
            </div>
          )}
          {rows.map((row, i) => {
            const { shown, more } = orderChips(row.refs)
            const chip = strip ? repoChipFor(repos, row.repo) : null
            // Both ends of a comparison are selected rows. `secondary` is `null` for every
            // one-commit selection, so this is exactly the old predicate until a pair exists.
            const picked = row.oid === selected || row.oid === secondary
            return (
              <div
                key={`${row.repo}:${row.oid}`}
                role="option"
                aria-selected={picked}
                // Only the selected row is held, and only so the reveal effect above can scroll
                // to it. A ref on every row would be a map to keep in step with the page.
                ref={row.oid === selected ? revealedRow : null}
                title={rowTitle(row)}
                data-audit="logRow"
                /*
                 * The row's identity, for the context menu to resolve what was right-clicked.
                 *
                 * `repo:oid` and not the oid alone, because a merged walk interleaves several
                 * repositories and two roots can — a submodule vendored from the same history,
                 * a fork — hold the same commit. It is the same key React is given above, and
                 * the same `[data-row-id]` convention `sidebar/GitPanel` uses, so the host's
                 * `closest()` lookup reads identically in both panels.
                 *
                 * Unconditional, not gated on `rowMenu`: it is a fact about the row rather than
                 * a feature of the menu, and an attribute that appears only when a handler was
                 * passed is one the SSR check cannot see.
                 */
                data-row-id={`${row.repo}:${row.oid}`}
                className={picked ? `${styles.row} ${styles.rowActive}` : styles.row}
                /*
                 * `metaKey` folded into `ctrl` here and nowhere else.
                 *
                 * ⌘ is the multi-select modifier on macOS and Ctrl is everywhere else, which is a
                 * fact about the *platform's pointer conventions* rather than about the log — so
                 * it is resolved at the event, the way `sidebar/clickSemantics.ts` resolves its
                 * own `primary` from `ctrl || meta`. `logModel::selectRow` then has one modifier
                 * to reason about instead of two that must never disagree.
                 */
                onClick={(e) =>
                  onSelect(row.oid, { ctrl: e.ctrlKey || e.metaKey, shift: e.shiftKey })
                }
              >
                {/* The graph gutter. Absent entirely when the graph is off, rather than drawn
                    empty: a blank column of the same width would read as a history with no
                    branches in it, which is a claim this view has not checked. */}
                {graph.length > 0 && <GraphCell row={graph[i]} />}
                {/* The repo strip. Before the oid rather than after the subject, because it is
                    the column the eye groups by when several repositories are interleaved —
                    the same position the graph gutter would occupy if a merged walk had one. */}
                {chip !== null && (
                  <span
                    className={styles.repo}
                    data-audit="logRepo"
                    style={{ borderColor: chip.color, color: chip.color }}
                    title={chip.name}
                  >
                    {chip.name}
                  </span>
                )}
                <span className={styles.oid} data-audit="logOid">
                  {row.shortOid}
                </span>
                {shown.map((chipRef) => (
                  <RefChipCell
                    key={chipRef.full}
                    chip={chipRef}
                    filter={filter}
                    onFilter={onFilter}
                  />
                ))}
                {/*
                 * The overflow chip stays a `<span>`, and that is a decision rather than an
                 * omission: `+38` names a count, not a ref, so there is nothing for it to
                 * re-root the walk on. Making it *expand* the row into its full list of refs
                 * would be a reasonable feature and a different one.
                 */}
                {more > 0 && (
                  <span className={styles.chip} data-audit="logChipMore" title={`${more} more refs`}>
                    +{more}
                  </span>
                )}
                <span className={styles.subject} data-audit="logSubject">
                  {row.summary}
                </span>
                <span className={styles.author} data-audit="logRowAuthor">
                  {row.author}
                </span>
                <span className={styles.when} data-audit="logWhen">
                  {when(row.authored, now)}
                </span>
              </div>
            )
          })}
          {/*
           * A ref that only some repositories have. Under a merged scope the page still has rows
           * — the roots that *do* have the branch answered — so `logStatus` is `null` and this is
           * the only place the refusal can be seen. Drawn under the rows and not as the status,
           * because it is a footnote to a list rather than a reason there is not one.
           */}
          {refNote !== null && (
            <div className={styles.note} data-audit="logRefNote">
              {refNote}
            </div>
          )}
          {graphOff !== null && rows.length > 0 && (
            <div className={styles.note} data-audit="logGraphOff">
              {graphOff}
            </div>
          )}
          {page !== null && page.resume !== null && (
            <button
              type="button"
              className={styles.more}
              data-audit="logMore"
              disabled={busy}
              onClick={onMore}
            >
              {moreLabel(page)}
            </button>
          )}
        </div>
      </div>
      <div className={styles.details} data-audit="logDetails">
        {/*
         * The reveal's answer, above the commit it is about.
         *
         * In the details pane and not over the list, because that is where the commit itself is:
         * the `outside` case has already fetched `git_commit_detail`, so the message and the
         * changed files are right underneath this sentence and the user can read the commit they
         * clicked without the list containing it. A banner over the rows would be a sentence
         * about a commit with the *wrong* commit under it.
         */}
        {note !== null && (
          <div className={styles.reveal} data-audit="logReveal">
            <div className={styles.note} data-audit="logRevealNote">
              {note}
            </div>
            {revealFindable(reveal) && (
              <button
                type="button"
                className={styles.inlineClear}
                data-audit="logRevealFind"
                onClick={onFindCommit}
              >
                Clear filters and find it
              </button>
            )}
          </div>
        )}
        {details}
      </div>
      {/* It portals to the app root, so this position is only about *something* rendering it —
          the tool window's panes are `overflow: hidden` and could not have clipped it anyway. */}
      {rowMenu?.menu}
    </div>
  )
}

/**
 * One ref chip: a `<button>` that re-roots the walk, or a `<span>` when there is nothing to ask.
 * (M21)
 *
 * Which of the two it is comes from `logModel::chipTarget`, and it is `null` from that function
 * that decides — not a second predicate here. A chip that rendered as a control and then declined
 * to act would be the dead-menu-item failure `cide-core::commands` refuses to represent: hovering
 * like a button, doing nothing when pressed, and indistinguishable from a working one in a
 * screenshot. Two spellings of the element is how "inert" becomes visible to the pointer, to a
 * screen reader, and to `check:log-render`.
 *
 * # Keyboard: reachable by pointer only, and that is the honest answer rather than the good one
 *
 * `tabIndex={-1}`, so Tab skips every chip. The commit list is a `role="listbox"` whose *scroller*
 * is the single tab stop; the rows are `role="option"` divs with no roving tabindex and no
 * arrow-key handler, so today nothing inside the list is keyboard-reachable. Leaving these
 * buttons as default tab stops would put up to `MAX_CHIPS` of them on **every** row: a
 * fifty-row page is a hundred and fifty Tab presses between the filter bar and *Load more*, which
 * is a worse regression for a keyboard user than a chip they cannot reach. Focusable
 * programmatically and skipped by Tab is the same trade `chrome/BranchSelector.tsx` makes for its
 * rows, for the same reason.
 *
 * What a keyboard user has instead: the branch `<select>` in the filter bar is a real tab stop
 * and offers every local and remote-tracking branch, so re-rooting on a *branch* has a keyboard
 * path already. **Tags do not** — `LogTab::namesOf` collects `BranchList`'s two sides and tags are
 * in neither, so a tag can be reached by pointer here or by typing it into *Compare with…*, which
 * is a different gesture with a different answer. That gap is real and is not closed by this
 * change; closing it means giving the list a roving tabindex over its rows, which is a piece of
 * work of its own and would make these chips reachable for free once it exists.
 */
function RefChipCell({
  chip,
  filter,
  onFilter,
}: {
  chip: RefChip
  filter: LogFilter
  onFilter: (next: LogFilter) => void
}) {
  const target = chipTarget(chip, filter.branch)
  // `styles[kind]` is the colour; the two tokens together are what `check:log-render` counts, and
  // one token means the kind class resolved to `undefined` — a chip in the wrong colour, which is
  // invisible against a screenshot of a repository with only local branches in it.
  const className = `${styles.chip} ${styles[chip.kind] ?? ''}`
  const title = chipTitle(chip, filter.branch)

  if (target === null) {
    return (
      <span className={className} data-audit="logChip" data-kind={chip.kind} title={title}>
        {chip.name}
      </span>
    )
  }
  return (
    <button
      type="button"
      className={className}
      data-audit="logChip"
      data-kind={chip.kind}
      title={title}
      tabIndex={-1}
      onClick={(e) => {
        /*
         * The row underneath is itself clickable, and both handlers would otherwise run.
         *
         * A chip click would then re-root the walk *and* move the selection — and with Ctrl held
         * it would add this row as the second endpoint of a comparison. That is two unrelated
         * things from one gesture, and the second one is worse than merely surplus: the row it
         * selects is about to be the first row of a completely different list, so the details
         * pane would be showing a commit the user did not choose from a walk they did not have
         * yet. React's synthetic events bubble to the row's `onClick`, so stopping here is what
         * keeps the chip a control in its own right.
         */
        e.stopPropagation()
        /*
         * The other two filter boxes are kept, and the branch field is replaced — exactly what
         * the `<select>` beside it does with the same field. The two controls set one value and a
         * chip that also cleared the author box would make "which control did I use" change what
         * a re-root means. A user who has typed an author and clicks `v1.0` is asking for that
         * author's commits from v1.0 down, which is the composable reading and the useful one.
         * (*Clear filters and find it* clears everything, and that is a different gesture with a
         * sentence above it saying so — see `logModel::findCommitFilter`.)
         */
        onFilter({ ...filter, branch: target })
      }}
    >
      {chip.name}
    </button>
  )
}

/**
 * One changed file, in the details pane. (M20)
 *
 * Extracted from `LogTab`'s single-commit list rather than written a second time for the range
 * list, and that is the whole point of it existing: the *click rule* on these rows is conditional
 * — one click re-points a revision diff that is already open and otherwise only selects, two
 * clicks always open a kept tab — and a second copy of that would be a second answer to the bug
 * report `sidebar/clickSemantics.ts` was written for. The rule itself stays in the host, which is
 * where `gitTreeClick` and `diffOpenMode` are consulted; what is shared here is the row that
 * feeds them, so the two lists cannot end up with different gestures.
 *
 * `counts` is `null` for the one-commit list, which prints its totals once above the list
 * instead — `CommitDetail.total.partial` means a large merge has *no* per-file counts to print,
 * and a column of `+0 −0` would be a number the reader could believe. A range always has them.
 */
export function ChangedFileRow({
  path,
  oldPath,
  label,
  counts,
  depth,
  title,
  theme,
  selected,
  onSelect,
  onOpen,
}: {
  path: string
  /** The pre-rename path. Drawn as `old → new`, and passed through to whatever opens the diff. */
  oldPath: string | null
  /**
   * What the row shows, which is not always the path.
   *
   * Grouped under a directory heading it is the basename; flat it is the whole path; a rename is
   * an arrow either way. The rule is `fileRows.ts`'s, because it has three cases that are only
   * visible once a rename crosses a directory — see `labelInDir`.
   */
  label: string
  /** `+12 −3`, or `null` for a list whose counts are stated once at the top. */
  counts: string | null
  /** Indent level. `1` under a directory heading, `0` flat or at the repository root. */
  depth: number
  title: string
  /**
   * Which set of icon files to link. A prop and not `useIconTheme()`, because that hook reads the
   * workspace store and this file is server-rendered by `check-log-render.mjs`. `FileIcon` takes
   * the same prop for the same reason — the whole `icons/` surface is pure below the hook.
   */
  theme: IconTheme
  selected: boolean
  onSelect?: ((path: string) => void) | undefined
  /**
   * Takes `MouseEvent.detail` — the click count — rather than a gesture, so the rule that turns
   * one into the other stays in one place. See `onOpenFile` in `LogTab`.
   */
  onOpen?: ((path: string, oldPath: string | null, detailCount: number) => void) | undefined
}) {
  return (
    <button
      type="button"
      className={selected ? `${styles.fileRow} ${styles.fileRowOn}` : styles.fileRow}
      data-audit="logFile"
      /* The row is the selection, so it reports it. Without this a screen reader is told the
         list has a highlighted row by CSS alone, which is to say not told at all. */
      aria-current={selected ? 'true' : undefined}
      /*
       * How the context menu finds which file was right-clicked.
       *
       * `useContextMenu` hands its `items` builder the DOM target, exactly as the commit list's
       * menu resolves a row, so the host walks up to this attribute rather than this component
       * taking an `onMenu` prop. That keeps the menu — which needs the workspace store, the IPC
       * client and four commands — out of the file `check-log-render.mjs` server-renders.
       */
      data-file-path={path}
      title={title}
      /*
       * One handler for both gestures, keyed on `MouseEvent.detail`.
       *
       * `onDoubleClick` is deliberately absent rather than sitting beside this: the browser fires
       * `click` with `detail` 1 and then 2 for a double-click and would also fire `dblclick`, so
       * keeping both would open the tab twice for one gesture.
       */
      /*
       * One click selects; two open. Both, in that order, from one handler.
       *
       * `onSelect` runs on every click including the second, which is deliberate: a double-click
       * has to leave the row it opened looking selected, and the browser delivers `detail` 1 and
       * then 2, so the first of the pair has already selected it. Skipping it on `detail === 2`
       * would be an extra branch that changes nothing.
       *
       * `onOpen` still takes the count rather than a gesture — `sidebar/clickSemantics.ts` owns
       * the rule that turns one into the other, and this row must not grow a second copy of it.
       */
      onClick={(e) => {
        onSelect?.(path)
        onOpen?.(path, oldPath, e.detail)
      }}
      /*
       * Ctrl+D does what a double-click does, which is why it passes `2`.
       *
       * The click count is the channel — see `onOpen` — so the keyboard has to speak it rather
       * than reach past it into a second entry point. `PRIMARY_FILE_ACTION` records that these
       * two gestures are one action, so editing one of them is visibly editing both.
       *
       * Not a registered command: a binding in `cide-core::commands` is claimed by a window
       * capture listener and would take Ctrl+D from every terminal pane in every window. It is
       * scoped to a focused row here, which is the same reasoning `git.blame` records for
       * shipping unbound.
       */
      onKeyDown={(e) => {
        if (!isDiffShortcut(e)) return
        e.preventDefault()
        onOpen?.(path, oldPath, 2)
      }}
    >
      {/*
       * An empty twisty slot, on a row that has nothing to disclose.
       *
       * It is the indentation — the same arrangement `sidebar/FileTree.tsx` uses, and its note
       * says why: *"the span is drawn on every row — it is the indentation, so a file has one
       * too"*. Without it a file's icon sits where a directory's *twisty* is rather than where
       * its icon is, and the nesting reads as a flat list with a slight offset.
       */}
      <span className={styles.twisty} style={{ marginLeft: depth * INDENT }} aria-hidden="true" />
      <FileIcon row={{ name: baseName(path), kind: 'file' }} theme={theme} />
      <span className={styles.fileLabel} data-audit="logFileLabel">
        {label}
      </span>
      {counts !== null && (
        <span className={styles.counts} data-audit="logFileCounts">
          {counts}
        </span>
      )}
    </button>
  )
}

/** The last path segment, for the icon table — which matches on a filename, not a path. */
function baseName(path: string): string {
  const at = path.lastIndexOf('/')
  const name = at === -1 ? path : path.slice(at + 1)
  return name === '' ? path : name
}

/**
 * The changed-file list, with the control that decides how it is arranged. (M21)
 *
 * One component for both lists — the single commit's and the two-revision range's — because they
 * differ only in what their summary line says and whether the rows carry counts. Two spellings
 * would be two places for the grouping, the indent and the two gestures to drift apart, and the
 * gestures are the part that must not: `sidebar/clickSemantics.ts` owns "one click is not two"
 * for the whole application.
 *
 * The toggle is a header control rather than a setting in a dialog because the choice is not a
 * preference so much as a reading: grouped answers "which areas did this touch", flat answers
 * "what is the full path of the thing I am looking for", and a reader wants each of them within
 * seconds of the other. It persists per project on `ToolWindowState.files_as_tree` — the same
 * place and the same argument as the divider beside it.
 */
export function ChangedFileList({
  files,
  asTree,
  summary,
  theme,
  selected,
  collapsed,
  onSelect,
  onToggleDir,
  onToggleTree,
  onOpen,
  titleFor,
}: {
  files: readonly ChangedFile[]
  asTree: boolean
  /** Which icon set to link. See [`ChangedFileRow.theme`] for why it is a prop. */
  theme: IconTheme
  /** The selected path, or `null`. One at a time — this is a reading surface, not a staging one. */
  selected: string | null
  /** Directories whose files are hidden. Ignored in the flat reading — see [`fileRows`]. */
  collapsed?: ReadonlySet<string> | undefined
  onSelect?: ((path: string) => void) | undefined
  /** Fold or unfold one directory. Absent leaves the headings non-interactive. */
  onToggleDir?: ((dir: string) => void) | undefined
  /** The line above the list: `12 files · +34 −5`, or a sentence when there is nothing to count. */
  summary: string
  /**
   * Absent when the host cannot persist the choice, in which case the control is not drawn at
   * all rather than drawn dead. A toggle that forgets on every render is worse than no toggle.
   */
  onToggleTree?: ((next: boolean) => void) | undefined
  onOpen?: ((path: string, oldPath: string | null, detailCount: number) => void) | undefined
  titleFor: (file: ChangedFile) => string
}) {
  const rows = fileRows(files, asTree, collapsed)
  return (
    <>
      <div className={styles.fileHead} data-audit="logFileHead">
        <span className={styles.note} data-audit="logFileSummary">
          {summary}
        </span>
        {onToggleTree !== undefined && files.length > 0 && (
          <button
            type="button"
            className={asTree ? `${styles.treeToggle} ${styles.treeToggleOn}` : styles.treeToggle}
            data-audit="logTreeToggle"
            /*
             * `aria-pressed` and not `aria-selected`: this is a two-state control that stays
             * where it is, not one of a set of alternatives. It is the same distinction the
             * activity rail's tool-window button records.
             */
            aria-pressed={asTree}
            aria-label="Group files by directory"
            title={
              asTree
                ? 'Grouped by directory. Click to list full paths instead.'
                : 'Full paths. Click to group by directory instead.'
            }
            onClick={() => onToggleTree(!asTree)}
          >
            {/* A folder tree, drawn rather than typed: the glyphs that mean this — ⊞, ⊟, 🗀 —
                are either absent from the desktop's UI fonts or render as emoji at a size
                nothing else in this bar uses. Same finding as the activity rail's Git icon. */}
            <svg
              aria-hidden="true"
              viewBox="0 0 24 24"
              width={13}
              height={13}
              fill="none"
              stroke="currentColor"
              strokeWidth={2}
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M5 4v14M5 9h5M5 15h5M13 7h6M13 12h6M13 17h6" />
            </svg>
          </button>
        )}
      </div>
      {rows.map((row) =>
        row.kind === 'dir' ? (
          /*
           * A button, since M21. It was a label, on the reasoning that a commit's file list is
           * short enough that there is nothing worth folding — true of a three-file commit and
           * plainly false of the merge that touches four hundred, where one directory is the
           * thing you are looking for and the rest is noise.
           */
          <button
            key={row.id}
            type="button"
            className={styles.fileDir}
            data-audit="logFileDir"
            /* `aria-expanded` and not `aria-pressed`: this row *owns* the rows under it, which
               is the difference between a disclosure and a toggle. The grouping control in the
               header above is the other case and correctly uses `aria-pressed`. */
            aria-expanded={!row.collapsed}
            title={row.collapsed ? `Show ${row.count} files in ${row.dir}` : `Hide ${row.dir}`}
            onClick={() => onToggleDir?.(row.dir)}
          >
            <span className={styles.twisty} aria-hidden="true">
              <Icon name={row.collapsed ? 'chevron-right' : 'chevron-down'} size={1} />
            </span>
            {/* The folder icon comes from the same table the file tree's does, so a directory
                heading here and the same directory over there are the same picture. `iconFor`
                takes `kind: 'dir'` and answers from the folder table. */}
            <FileIcon row={{ name: baseName(row.label), kind: 'dir' }} theme={theme} />
            <span className={styles.fileLabel} data-audit="logFileDirLabel">
              {row.label}
            </span>
            {/* Folded, the count is the only thing saying the directory still has contents. A
                silent folded row is indistinguishable from an empty one. */}
            {row.collapsed && (
              <span className={styles.counts} data-audit="logFileDirCount">
                {row.count}
              </span>
            )}
          </button>
        ) : (
          <ChangedFileRow
            key={row.id}
            path={row.file.path}
            oldPath={row.file.oldPath}
            label={row.label}
            counts={row.file.counts}
            depth={row.depth}
            title={titleFor(row.file)}
            theme={theme}
            selected={row.file.path === selected}
            {...(onSelect === undefined ? {} : { onSelect })}
            {...(onOpen === undefined ? {} : { onOpen })}
          />
        ),
      )}
    </>
  )
}

/**
 * The details pane when two revisions are selected: a header, ⇄ Swap, and the range's files. (M20)
 *
 * Here and not in `LogTab.tsx`, where the single-commit details are, and the reason is
 * `check-log-render.mjs`: it server-renders this file and nothing else, because `LogTab` imports
 * the IPC client and the workspace store. A compare header built in the host would be a header no
 * check could look at, and "the swap control is present" is precisely the kind of claim that is
 * true right up until a conditional above it changes.
 *
 * Every value is a prop, including both labels. The pane does not know whether `newer` is an oid
 * or the working tree, and it must not: *Compare with working tree* has no hash for its newer
 * side, and a component that tried to abbreviate one would print `working` and a stray `tree`.
 *
 * `onSwap` is `null` — not a disabled button — for the working-tree comparison, because
 * `RevSide::WorkingTree` is legal only as the *newer* side and `cide_git::revision` refuses the
 * other arrangement by name. A swap control there would be a control whose only outcome is a
 * typed error from Rust.
 */
export function RangeDetails({
  older,
  newer,
  range,
  note,
  onSwap,
  asTree,
  theme,
  selectedFile,
  onSelectFile,
  collapsedDirs,
  onToggleDir,
  onToggleTree,
  onOpenFile,
}: {
  /** The left-hand label: a full oid, which is abbreviated here, or a name that is not. */
  older: string
  newer: string
  /** `null` while the request is in flight, or when `note` says why there will not be one. */
  range: RevisionRange | null
  /** A sentence in the file list's place: a refusal, or a cross-repository pair. */
  note: string | null
  onSwap: (() => void) | null
  /** Group the files by directory. Persisted — see `ToolWindowState::files_as_tree`. */
  asTree: boolean
  theme: IconTheme
  selectedFile: string | null
  onSelectFile?: ((path: string) => void) | undefined
  collapsedDirs?: ReadonlySet<string> | undefined
  onToggleDir?: ((dir: string) => void) | undefined
  onToggleTree?: ((next: boolean) => void) | undefined
  onOpenFile?: ((path: string, oldPath: string | null, detailCount: number) => void) | undefined
}) {
  const truncated = range === null ? null : rangeTruncatedNote(range.truncated)
  return (
    <>
      <div className={styles.rangeHead}>
        <strong data-audit="logRangeHeader">{compareTitle(older, newer)}</strong>
        {onSwap !== null && (
          <button
            type="button"
            className={styles.swap}
            data-audit="logRangeSwap"
            aria-label="Swap sides"
            title="Read the two revisions the other way round"
            onClick={onSwap}
          >
            <Icon name="arrow-left-right" size={1} /> Swap
          </button>
        )}
      </div>
      {/* Which commits these are, in the same old → new direction as the header. The summaries
          come from the range itself (`RevisionRange::newSummary`) rather than from the loaded
          page, because *Compare with…* can name a revision that is not on the page at all — and
          an empty string is what the working-tree side honestly has. */}
      {range !== null && (range.oldSummary !== '' || range.newSummary !== '') && (
        <div className={styles.note} data-audit="logRangeSummaries">
          {range.oldSummary} → {range.newSummary}
        </div>
      )}
      {note !== null && (
        <div className={styles.note} data-audit="logRangeNote">
          {note}
        </div>
      )}
      {/* The count, or the sentence that replaces it when there is nothing to count. Two
          revisions really can have identical trees — a revert and its target, a cherry-pick and
          its source — and an empty pane under a "Comparing …" header is indistinguishable from a
          request that failed. */}
      {range !== null && (
        <ChangedFileList
          files={range.files.map((f: RevisionChange) => ({
            path: f.path,
            oldPath: f.oldPath,
            counts: changeCounts(f.additions, f.deletions),
          }))}
          asTree={asTree}
          theme={theme}
          selected={selectedFile}
          {...(collapsedDirs === undefined ? {} : { collapsed: collapsedDirs })}
          {...(onSelectFile === undefined ? {} : { onSelect: onSelectFile })}
          {...(onToggleDir === undefined ? {} : { onToggleDir })}
          summary={
            range.files.length === 0
              ? RANGE_IDENTICAL
              : `${range.files.length} file${range.files.length === 1 ? '' : 's'}`
          }
          {...(onToggleTree === undefined ? {} : { onToggleTree })}
          {...(onOpenFile === undefined ? {} : { onOpen: onOpenFile })}
          /* All three gestures named, because two of them are not discoverable by trying: the
             single click is conditional — with no revision diff on screen it does nothing
             visible — and the shortcut is not written anywhere else on the surface. */
          titleFor={(f) =>
            `${f.path}\n`
            + `Double-click or Ctrl+D to open this file's diff between these two revisions.\n`
            + 'A single click re-points a revision diff that is already open.\n'
            + 'Right-click for more.'
          }
        />
      )}
      {truncated !== null && (
        <div className={styles.note} data-audit="logRangeTruncated">
          {truncated}
        </div>
      )}
    </>
  )
}

/**
 * The branch filter box and its list.
 *
 * # Two values, and keeping them apart is the whole component
 *
 * `filter.branch` is what the log is *walking*. `query` is what the user has *typed*. They are
 * not the same thing and must not share a state: typing `fea` has to narrow the list without
 * re-walking the log on every keystroke, and abandoning the box with Escape has to leave the walk
 * exactly where it was. So the input shows the query while it is focused and the chosen branch's
 * name when it is not, and only choosing a row calls `onFilter`.
 *
 * That is also why the query is cleared on open rather than seeded with the current branch. A box
 * pre-filled with `main` shows a list filtered to names containing "main" — which is a list of
 * one — so the gesture that is supposed to reveal every branch would reveal the one already
 * chosen.
 *
 * # Blur closes it, and the mousedown guard is why the list is clickable at all
 *
 * `onBlur` fires before `click`, so a list that closed on blur would unmount the row under the
 * pointer between press and release and the click would land on nothing. `onMouseDown` with
 * `preventDefault` keeps focus in the input, which is the standard fix and the reason it is not a
 * `<select>`'s problem.
 */
function BranchBox({
  filter,
  branches,
  onFilter,
}: {
  filter: LogFilter
  branches: readonly string[]
  onFilter: (next: LogFilter) => void
}) {
  const [query, setQuery] = useState('')
  const [open, setOpen] = useState(false)
  const [highlight, setHighlight] = useState(-1)

  const value = branchValue(filter.branch)
  const chosenLabel =
    filter.branch.kind === 'rev'
      ? revLabel(filter.branch.spec)
      : filter.branch.kind === 'all'
        ? 'All branches'
        : filter.branch.kind === 'branch'
          ? filter.branch.name
          : 'Current branch'

  const rows = branchRows(branches, query, value, chosenLabel)
  const note = branchOverflowNote(branches, query)

  const choose = (row: BranchRow) => {
    onFilter({ ...filter, branch: branchFromValue(row.value) })
    setOpen(false)
    setQuery('')
    setHighlight(-1)
  }

  return (
    <div className={styles.branchBox}>
      <input
        className={styles.branch}
        data-audit="logBranch"
        /*
         * The chosen value on an attribute, exactly as the `<select>` carried it.
         *
         * It was there because a `<select>`'s selection lives on one of its `<option>`s and a
         * server-rendered check would have had to work out which one won. The input has a `value`
         * of its own now — but that is the *typed* text, which is a different fact, so the
         * attribute is still the only place the control's chosen branch is legible from outside.
         */
        data-value={value}
        type="text"
        role="combobox"
        aria-expanded={open}
        aria-controls="log-branch-list"
        aria-autocomplete="list"
        aria-label="Branch"
        placeholder="Branch"
        value={open ? query : chosenLabel}
        onFocus={() => {
          setOpen(true)
          setQuery('')
          setHighlight(-1)
        }}
        onBlur={() => setOpen(false)}
        onChange={(e) => {
          setQuery(e.target.value)
          setOpen(true)
          // Back to "nothing chosen" on every edit. Keeping the index would leave the highlight
          // on whatever row happens to be at that position in the new, shorter list — a
          // different branch than the one the user was looking at.
          setHighlight(-1)
        }}
        onKeyDown={(e) => {
          if (e.key === 'Escape') {
            // Closes without choosing, and the walk is untouched — see the header.
            setOpen(false)
            setQuery('')
            e.stopPropagation()
            return
          }
          if (e.key === 'Enter') {
            const picked = commitChoice(rows, highlight)
            if (picked !== null) {
              choose(picked)
              e.preventDefault()
            }
            return
          }
          const moved = moveHighlight(rows.length, highlight, e.key)
          if (moved !== highlight) {
            setHighlight(moved)
            setOpen(true)
            // Or the caret jumps to the end of the text on every ↓, which reads as the box
            // eating the arrow keys.
            e.preventDefault()
          }
        }}
      />
      {open && (
        <ul className={styles.branchList} id="log-branch-list" role="listbox" data-audit="logBranchList">
          {rows.map((row, i) => (
            <li key={row.value}>
              <button
                type="button"
                role="option"
                aria-selected={row.value === value}
                className={
                  i === highlight ? `${styles.branchRow} ${styles.branchRowOn}` : styles.branchRow
                }
                data-audit="logBranchRow"
                // See the header: blur beats click, so the press must not move focus.
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => choose(row)}
              >
                {row.label}
              </button>
            </li>
          ))}
          {rows.length === 0 && (
            <li className={styles.branchNote} data-audit="logBranchEmpty">
              No branch matches
            </li>
          )}
          {/* How many were left out, which the row count cannot say — it stops at the cap by
              construction and includes the two synthetic entries. A list that silently shows
              fifty of four hundred is the failure `LogStop::Budget` exists to prevent one
              surface over. */}
          {note !== null && (
            <li className={styles.branchNote} data-audit="logBranchMore">
              {note}
            </li>
          )}
        </ul>
      )}
    </div>
  )
}

/**
 * A branch box that filters as you type, plus the author, text and refresh controls.
 *
 * # Not a `<select>`, and not `chrome/BranchSelector.tsx` either
 *
 * It was a native `<select>` and that was wrong for the ordinary case: a shared repository has
 * hundreds of branches, the list is ordered by commit date rather than by name, and picking one
 * meant scrolling and reading. Every other picker in this app is a filter box; this was the one
 * place that made the user hunt.
 *
 * It is still not the popup `chrome/BranchSelector.tsx` draws, and the reason has not changed:
 * this component has to server-render — `check-log-render.mjs` is the only thing that looks at
 * it — and a popup is a portal, a focus trap and a `document` listener. So the list is an
 * ordinary absolutely-positioned element inside this component's own box, closed on blur. The
 * second reason also stands: that popup's job is *checkout*, a gesture that moves the working
 * tree and needs a refusal surface, while this one re-roots a read-only walk and cannot fail
 * destructively.
 *
 * Everything decidable is in `branchFilter.ts` so a check can run it: what an empty query means,
 * where the cap applies, what Enter does with one match and no highlight.
 */
function FilterBar({
  filter,
  filtered,
  branches,
  busy,
  onFilter,
  onRefresh,
}: {
  filter: LogFilter
  filtered: boolean
  branches: readonly string[]
  busy: boolean
  onFilter: (next: LogFilter) => void
  onRefresh: () => void
}) {
  return (
    <div className={styles.filters} data-audit="logFilters">
      <BranchBox filter={filter} branches={branches} onFilter={onFilter} />
      <input
        className={styles.input}
        data-audit="logAuthor"
        type="text"
        aria-label="Author"
        placeholder="Author"
        value={filter.author}
        onChange={(e) => onFilter({ ...filter, author: e.target.value })}
      />
      <input
        className={styles.input}
        data-audit="logText"
        type="text"
        aria-label="Message or hash"
        placeholder="Message or hash"
        value={filter.text}
        onChange={(e) => onFilter({ ...filter, text: e.target.value })}
      />
      {filtered && (
        <button
          type="button"
          className={styles.clear}
          data-audit="logClear"
          onClick={() => onFilter(clearFilters(filter))}
        >
          Clear filters
        </button>
      )}
      {/*
       * Refresh is always offered, and it is not the same gesture as changing a filter: the log
       * is a view of a repository that a `git commit` in a bash pane moves under it.
       *
       * `LogTab` now watches `FsChange.git` and re-fetches on a ref move by itself, so this is no
       * longer the *only* way to see that — and it is still here rather than deleted, because the
       * watcher covers only what it can see. A ref moved inside a container, on a network mount
       * with no inotify, or while the watcher was degraded to polling arrives late or not at all,
       * and a read-only panel with no manual refresh gives a user in that position nothing to do.
       *
       * Disabled while a walk is in flight so a held key cannot stack requests the generation
       * counter would only have to throw away.
       */}
      <button
        type="button"
        className={styles.refresh}
        data-audit="logRefresh"
        aria-label="Refresh"
        title="Refresh"
        disabled={busy}
        onClick={onRefresh}
      >
        <Icon name="refresh-cw" size={1} />
      </button>
    </div>
  )
}

/**
 * One row's slice of the graph.
 *
 * Drawn as absolutely positioned spans rather than an SVG per row: every edge is a straight
 * vertical segment in a fixed column, `Enter` and `Exit` being the top and bottom halves of one,
 * so there is no curve to describe and an SVG per row would be one more element and a viewBox to
 * keep in step with the row height. The dot is the only thing that is not a line.
 */
function GraphCell({ row }: { row: GraphRow | undefined }) {
  if (row === undefined) return <span className={styles.graph} />
  const at = (lane: number) => ({ left: `${lane * 10 + 5}px` })
  return (
    <span className={styles.graph} data-audit="logGraph">
      {row.edges.map((edge, i) => {
        if (edge.kind === 'bundle') {
          return (
            <span
              key={i}
              className={styles.bundle}
              style={at(edge.lane)}
              title={`${edge.count} more lanes`}
            >
              ⋯
            </span>
          )
        }
        const lane = edge.kind === 'pass' ? edge.lane : edge.kind === 'enter' ? edge.top : edge.bottom
        const cls =
          edge.kind === 'pass'
            ? styles.pass
            : edge.kind === 'enter'
              ? styles.enter
              : styles.exit
        return (
          <span
            key={i}
            className={`${cls} ${edge.kind === 'exit' && edge.dangling ? styles.dangling : ''}`}
            style={{ ...at(lane), background: laneColor(edge.color) }}
          />
        )
      })}
      <span
        className={styles.dot}
        style={{ ...at(row.lane), background: laneColor(row.color) }}
      />
    </span>
  )
}
