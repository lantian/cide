/**
 * The 252px explorer panel: the header the mock states, and the tree below it.
 *
 * Split from `FileTree` because the panel is a fixed piece of chrome with a fixed width and
 * the tree is a scrolling list — the layout audit measures the first and the virtualizer
 * owns the second, and one component doing both would make the audit reach through a
 * scroll container to find a border.
 *
 * This is also where the store is pointed at a project and where the watcher is subscribed,
 * so `FileTree` stays a pure render target over `treeStore`.
 */
import { memo, useEffect } from 'react'
import { FileTree } from './FileTree'
import { useFileTree } from './treeStore'
import { useGitStatus } from './gitStatusStore'
import { groupDigits } from '@/overlays/format'
import { events, type ProjectId } from '@/ipc/client'
import { Icon } from '@/icons/Icon'

import styles from './FileTree.module.css'

export interface ExplorerProps {
  /** The active project, or `null` when none is open. */
  project: ProjectId | null
  /** A double-click on a file row, and Enter. A single click only selects — see `FileTree`. */
  onOpenFile?: ((path: string) => void) | undefined
  /**
   * The context menu's *Open to the Side*.
   *
   * Optional because nothing in the sidebar can split a pane and this panel must not pretend
   * otherwise: with no host the item is drawn disabled with the reason on it, which is the
   * house treatment for "why not" beating absence. Wiring it is one line in the shell — see
   * the note in `FileTree.tsx`.
   */
  onOpenFileToSide?: ((path: string) => void) | undefined
  /**
   * The path the active tab is *about*, or `null` when it is about no file.
   *
   * Not used to reveal anything — that is [`onSelectOpened`]'s business, and it re-derives the
   * path from the same `focusedTabPath` the `fileTabActive` context flag does. This is here
   * only so the button can be **disabled with a reason** rather than being enabled and failing:
   * the header knows nothing about tabs, and the alternative is a control that looks live on a
   * Claude console and answers a click with a notice.
   */
  openedFile?: string | null
  /**
   * *Select opened file* — the ⌖ button.
   *
   * Routed through the command, never through `treeStore.reveal` directly. One code path for
   * the button, the ⌃⇧E chord and the palette row, because a second call site with its own copy
   * of the preconditions is how three gestures come to behave in three ways — the same argument
   * `keymap.rs` makes for routing the mouse's thumb buttons through the key gate.
   *
   * Optional, and the button is not drawn without it: a host with no dispatcher cannot honour
   * the click, and the honest shape there is absence rather than a permanently greyed control.
   * `App.tsx` always supplies it in the shell window, which is the only window with a sidebar.
   */
  onSelectOpened?: (() => void) | undefined
  /**
   * *Expand all* / *Collapse all* — the two fold buttons beside ⌖. (M96)
   *
   * Routed through `file.expandAll` / `file.collapseAll` by the host, for [`onSelectOpened`]'s
   * reason, and not drawn without it for the same one.
   */
  onExpandAll?: (() => void) | undefined
  onCollapseAll?: (() => void) | undefined
  /**
   * A **pinned** row was opened — today that is *Project Notes*, and the argument is its id.
   *
   * An id rather than a path, because a pin has no path: the row carries a `cide://group/…`
   * sentinel and the file it stands for is created and named by Rust. The host turns the id into
   * a **command**, exactly as [`onSelectOpened`] above turns its click into `file.reveal`, and
   * for the identical reason — one code path for the double-click, the palette row and any chord
   * somebody binds later, rather than three call sites with three copies of the preconditions.
   */
  onOpenPin?: ((id: string) => void) | undefined
  /**
   * *Show File History* on a tree row — the tool window's per-file tab. (M18)
   *
   * Passed straight through to `FileTree`, which owns the enablement; this panel only carries
   * it. Routed by the host through the **command**, exactly as [`onSelectOpened`] and
   * [`onOpenPin`] are and for the identical reason: four surfaces now offer this act, and four
   * copies of its preconditions is four chances for one of them to drift. See the prop's
   * documentation in `FileTree.tsx`.
   */
  onShowHistory?: ((path: string) => void) | undefined
}

/**
 * Memoised, like every sidebar panel host: the panel is a direct child of `App`, which
 * re-renders on every store notification it subscribes to, and everything this panel draws
 * arrives through its own store subscriptions or through props `App` pins with `useCallback`
 * for exactly this. Without the memo, every App render re-walked the panel — listeners,
 * virtualizer and all — for events that had nothing to do with files.
 */
export const Explorer = memo(ExplorerImpl)

function ExplorerImpl({
  project,
  onOpenFile,
  onOpenFileToSide,
  openedFile = null,
  onSelectOpened,
  onExpandAll,
  onCollapseAll,
  onOpenPin,
  onShowHistory,
}: ExplorerProps) {
  const count = useFileTree((s) => s.count)
  const truncated = useGitStatus((s) => s.status.truncated)

  useEffect(() => {
    void useFileTree.getState().attach(project)
  }, [project])

  /*
   * The status map is attached separately from the rows and never awaited alongside them.
   * That separation is the requirement: the tree paints from `treeStore` as soon as
   * `fs_tree_count` answers, and a repository whose `git status` takes two seconds shows an
   * untagged tree that gains tags — never an empty pane.
   */
  useEffect(() => {
    void useGitStatus.getState().attach(project)
  }, [project])

  useEffect(() => {
    if (project === null) return
    /*
     * `cancelled` as well as `unlisten`, because `listen` is asynchronous and this effect
     * re-runs on every project switch. If the cleanup wins the race, `unlisten` is still null
     * and calling it does nothing — the subscription then arrives with nobody left to remove
     * it and refreshes a tree that has moved on, once more per switch. Unsubscribing from
     * inside the `then` is the only place that can see the handle at all.
     */
    let cancelled = false
    let unlisten: (() => void) | null = null
    /*
     * `events.onFsChanged` is the helper that matches the wire: the payload carries
     * `payload.change.paths` (see `emit::FsChanged`). A second helper, `fsEvents.onChanged`,
     * read `payload.paths` and so handed every caller `undefined`. This comment used to end
     * "reported rather than deleted"; it has since been deleted, because the next caller to
     * reach for it by name got a listener that threw on every burst and failed silently.
     *
     * The burst itself is deliberately not inspected. Whether a row moved is a question about
     * the *index*, which applies its own ignore rules to these paths and skips the git files
     * outright, and re-deriving that judgement from path strings in the frontend would be a
     * second, worse copy of `cide_fs::filter`. `refresh` asks the index instead, and answers a
     * burst that moved nothing with no state write at all.
     */
    void events
      .onFsChanged((changed) => {
        // Only this project's events. A second project's watcher firing must not disturb the
        // tree the user is looking at.
        if (changed !== project) return
        void useFileTree.getState().refresh()
      })
      .then((fn) => {
        if (cancelled) fn()
        else unlisten = fn
      })
    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [project])

  /*
   * The walk finishing is news, and nothing else carries it.
   *
   * `attach` reads `fs_tree_count` the instant a project becomes active, which is while
   * `store/workspace.ts`'s `fs.index` is still walking — so the honest answer then is zero
   * rows, and it stays zero until something asks again. `cide://fs-changed` does not fire for
   * the walk (it is the *watcher*, and the watcher only starts once the walk is done), so
   * without this a freshly opened project shows an empty explorer until the user happens to
   * touch a file.
   *
   * Every status for this project triggers a re-read, not only the one with
   * `indexing: false`. The event is emitted at the start of a walk, at its end, and when the
   * watcher degrades to polling — three events per project, not a stream — so re-reading on
   * all three costs nothing. Note that `refresh` is a *revalidation*, not the cache drop it
   * used to be (see `treeStore`'s header): the walk starting no longer blanks the tree, it
   * re-reads the count and the visible rows and writes only if they moved. A caller that
   * genuinely needs the cache thrown away — new roots — wants `attach`, not this.
   */
  useEffect(() => {
    if (project === null) return
    let cancelled = false
    let unlisten: (() => void) | null = null
    void events
      .onFsStatus((changed) => {
        if (changed !== project) return
        void useFileTree.getState().refresh()
      })
      .then((fn) => {
        if (cancelled) fn()
        else unlisten = fn
      })
    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [project])

  /*
   * Keeping the tags true as things change, without polling.
   *
   * Three things move a path's status and none of them is a scroll: a commit or a stage cide
   * made itself, a `git add` in a bash pane, and an edit — by the user or by Claude. The first
   * arrives as `cide://git-status`; the other two arrive as `cide://fs-changed`, because the
   * watcher explicitly watches `HEAD`, `index` and the refs alongside the working tree.
   *
   * Both are folded into one coalesced refresh in the store rather than one call each: a
   * single `git commit` fires the git event *and* rewrites `.git/index` and `HEAD`, so
   * refreshing per event would walk the repository three times for one gesture.
   *
   * `fs-changed` is subscribed here a second time rather than folded into the effect above.
   * That one owns the row cache and this one owns the status map; they invalidate for
   * overlapping but not identical reasons — a project switch has to reset both, an expand only
   * the first — and merging them would tie two lifetimes together for one saved listener.
   */
  useEffect(() => {
    if (project === null) return
    let cancelled = false
    const unlisten: Array<() => void> = []
    const subscribe = (listen: Promise<() => void>) => {
      void listen.then((fn) => {
        if (cancelled) fn()
        else unlisten.push(fn)
      })
    }
    const refresh = (changed: ProjectId) => {
      if (changed === project) useGitStatus.getState().schedule()
    }
    subscribe(events.onFsChanged(refresh))
    subscribe(events.onGitStatus(refresh))
    return () => {
      cancelled = true
      for (const fn of unlisten) fn()
    }
  }, [project])

  return (
    <div className={styles.panel} data-audit="sidebarFiles">
      <div className={styles.header} data-audit="explorerHeader">
        <span className={styles.headerTitle}>Explorer</span>
        {/* The mock's right-aligned mono meta. Withheld at zero rather than shown as `0`,
            which during the first walk would read as "this repository is empty". */}
        {count > 0 && <span className={styles.headerMeta}>{groupDigits(count)}</span>}
        {/*
          * The status map hit its cap, so rows past the cut are untagged rather than clean.
          * Shown rather than only logged: the flag exists precisely so that a tree which has
          * stopped tagging does not read as a tree with nothing to tag, and a marker nobody
          * ever renders is a lie with a boolean in front of it. One glyph with a tooltip,
          * because the panel is 252px wide and this is a footnote, not an error.
          */}
        {truncated && (
          <span
            className={styles.headerWarn}
            title="Too many changes to tag every row; files further down the tree are shown untagged."
          >
            <Icon name="asterisk" size={1} />
          </span>
        )}
        {/*
          * *Select opened file*. IDEA's own crosshair.
          *
          * It was `⌖` (U+2316 POSITION INDICATOR) as a literal character, and this comment used
          * to say that the application bundled no UI icon set and that `ActivityRail.tsx` and
          * `GitPanel/Toolbar.tsx` stated that as policy rather than as a gap. It also recorded
          * that `◎` (U+25CE) was the fallback "if a font somewhere renders it as tofu", and that
          * **nothing in this repository checked glyph coverage** — which is the sentence that
          * made the policy untenable: a fallback nobody can verify is a hope. `check-ui-icons.mjs`
          * exists now, and the mark is drawn rather than typed.
          *
          * `title` and `aria-label` carry the same sentence, and it changes with the state:
          * a disabled control that says only "Select opened file" leaves the user to guess why
          * it is grey. This is the house treatment — see `GitPanel/Toolbar.tsx`, which calls
          * disabled-with-a-tooltip "the honest placeholder", and `menuModel.ts`'s `NO_HOST`.
          */}
        {onSelectOpened !== undefined && (
          <button
            type="button"
            className={styles.headerAction}
            data-audit="explorerSelectOpened"
            disabled={openedFile === null}
            title={
              openedFile === null
                ? 'No file is open in this tab'
                : 'Select opened file — scroll to it in the tree'
            }
            aria-label={
              openedFile === null ? 'No file is open in this tab' : 'Select opened file'
            }
            onClick={onSelectOpened}
          >
            <Icon name="crosshair" size={1} />
          </button>
        )}
        {onExpandAll !== undefined && (
          <button
            type="button"
            className={styles.headerAction}
            data-audit="explorerExpandAll"
            title="Expand all folders"
            aria-label="Expand all folders"
            onClick={onExpandAll}
          >
            <Icon name="chevrons-up-down" size={1} />
          </button>
        )}
        {onCollapseAll !== undefined && (
          <button
            type="button"
            className={styles.headerAction}
            data-audit="explorerCollapseAll"
            title="Collapse all folders"
            aria-label="Collapse all folders"
            onClick={onCollapseAll}
          >
            <Icon name="chevrons-down-up" size={1} />
          </button>
        )}
      </div>
      <FileTree
        project={project}
        onOpen={onOpenFile}
        onOpenToSide={onOpenFileToSide}
        onOpenPin={onOpenPin}
        onShowHistory={onShowHistory}
      />
    </div>
  )
}
