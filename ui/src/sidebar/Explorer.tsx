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
import { useEffect } from 'react'
import { FileTree } from './FileTree'
import { useFileTree } from './treeStore'
import { useGitStatus } from './gitStatusStore'
import { groupDigits } from '@/overlays/format'
import { events, fsEvents, type ProjectId } from '@/ipc/client'
import styles from './FileTree.module.css'

export interface ExplorerProps {
  /** The active project, or `null` when none is open. */
  project: ProjectId | null
  onOpenFile?: ((path: string) => void) | undefined
}

export function Explorer({ project, onOpenFile }: ExplorerProps) {
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
    void fsEvents
      .onChanged((changed) => {
        // Only this project's events. A second project's watcher firing must not blank the
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
   * Keeping the tags true as things change, without polling.
   *
   * Three things move a path's status and none of them is a scroll: a commit or a stage cide
   * made itself, a `git add` in a bash pane, and an edit — by the user or by Claude. The first
   * arrives as `cide://git-status`; the other two arrive as `cide://fs-changed`, because the
   * watcher explicitly watches `HEAD`, `index` and the refs alongside the working tree.
   *
   * Both are folded into one debounced refresh in the store rather than one call each: a
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
            †
          </span>
        )}
      </div>
      <FileTree onOpen={onOpenFile} />
    </div>
  )
}
