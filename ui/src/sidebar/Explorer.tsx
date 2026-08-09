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
import { groupDigits } from '@/overlays/format'
import { fsEvents, type ProjectId } from '@/ipc/client'
import styles from './FileTree.module.css'

export interface ExplorerProps {
  /** The active project, or `null` when none is open. */
  project: ProjectId | null
  onOpenFile?: ((path: string) => void) | undefined
}

export function Explorer({ project, onOpenFile }: ExplorerProps) {
  const count = useFileTree((s) => s.count)

  useEffect(() => {
    void useFileTree.getState().attach(project)
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

  return (
    <div className={styles.panel} data-audit="sidebarFiles">
      <div className={styles.header} data-audit="explorerHeader">
        <span className={styles.headerTitle}>Explorer</span>
        {/* The mock's right-aligned mono meta. Withheld at zero rather than shown as `0`,
            which during the first walk would read as "this repository is empty". */}
        {count > 0 && <span className={styles.headerMeta}>{groupDigits(count)}</span>}
      </div>
      <FileTree onOpen={onOpenFile} />
    </div>
  )
}
