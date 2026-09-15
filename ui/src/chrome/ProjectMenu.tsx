/**
 * The header's project chooser: `+` opens a folder, `▾` reopens one you had open before.
 *
 * # Why this component wires itself
 *
 * Every other surface in `chrome/` is a pure render target that `App.tsx` wires. This one is
 * not, and the exception is deliberate rather than a shortcut:
 *
 * * **The picker had to move to Rust.** `App.tsx` opened the folder dialog with
 *   `@tauri-apps/plugin-dialog`'s `open()`, and that path cannot be parented on Linux — the
 *   plugin's own command guards `set_parent` behind `cfg(windows | macos)`. That is the whole
 *   of the "the popup opens behind the cide window" report. The fix lives in
 *   `project_pick`, and a control that reached it only if someone remembered to stop passing
 *   `onNew` would be a fix that ships off.
 * * **A recent-projects list has no prop shape.** It is a menu built from an async command at
 *   open time, and threading `recents`, `onOpenRecent` and `onForgetRecent` through `AppHeader`
 *   would be three more props in `App.tsx` — the exact arrangement that has already left three
 *   controls in this app wired to nothing.
 *
 * The layout audit is unaffected: it measures `header`, `projectTab`, `projectDot` and
 * `projectPath`, none of which is in here, and it measures the live DOM of the running app
 * rather than this component in a fixture.
 */
import { useCallback, useRef } from 'react'
import { useContextMenu } from '@/menus'
import { projectMenu, type RecentEntry } from '@/ipc/client'
import { recentEntries } from './menuModel'
import { browseForProject } from './projectOpen'
import { Icon } from '@/icons/Icon'

import styles from './AppHeader.module.css'

export function ProjectMenu() {
  /*
   * The fetched list, in a ref rather than in state.
   *
   * `useContextMenu`'s `items` is called synchronously at open time, and the open follows the
   * fetch in the same async function — a `setState` there would not have been committed yet, so
   * the first press of `▾` would build the menu from the *previous* answer. A ref is written and
   * read in one turn, and nothing else renders from it: the menu is rebuilt from scratch on
   * every open, so there is no stale copy for a missing re-render to leave on screen.
   */
  const recents = useRef<readonly RecentEntry[]>([])
  /*
   * The popup hangs off the whole split button, not off the caret that opened it.
   *
   * `openFor` anchors a menu at its target's left/bottom corner. Anchored on the caret — the
   * right-hand half — the list dropped from a point a few pixels wide and stretched away to the
   * right, reading as a menu belonging to nothing. Anchored here it drops flush with the `+`,
   * under the control it came from, which is where every other dropdown in this app appears.
   */
  const anchor = useRef<HTMLDivElement>(null)

  // The picker itself lives in `chrome/projectOpen.ts`, because the palette's *Open project…*
  // is the same gesture and two spellings of it is how one of them ends up on the plugin
  // dialog that cannot be parented on Linux. That module's doc is where the why lives.
  const browse = useCallback(() => {
    void browseForProject()
  }, [])

  const { openFor, isOpen, menu } = useContextMenu({
    label: 'Recent projects',
    items: () =>
      recentEntries(recents.current, {
        browse,
        reopen: (path) => {
          // `void` on purpose: a rejection — the folder went away between the fetch and the
          // click — is caught by `Failures`, which is the surface that exists to say so.
          // The reopened project arrives on the mutation's own broadcast; nothing here
          // reads the mirror afterwards, so there is no re-read and nothing to wait for.
          void projectMenu.reopen(path)
        },
        forgetMissing: () => {
          // Sequential rather than `Promise.all`: each call is a read-modify-write of one file,
          // and Rust serialises them on one lock anyway. Racing them would only mean waiting on
          // the same lock from four tasks instead of one.
          void (async () => {
            for (const entry of recents.current.filter((e) => !e.exists)) {
              recents.current = await projectMenu.forget(entry.project.path)
            }
          })()
        },
        clear: () => {
          void projectMenu.forget(null).then((left) => {
            recents.current = left
          })
        },
      }),
  })

  const openRecents = useCallback(() => {
    void (async () => {
      recents.current = await projectMenu.recent()
      const under = anchor.current
      if (under) openFor(under)
    })()
  }, [openFor])

  return (
    /*
     * One bordered pill holding both halves.
     *
     * The wrapper is not decoration for its own sake: it is what takes this control out of the
     * header's `.tabs` box — see `AppHeader.tsx` — and it is what the popup anchors to. It also
     * draws the separation the two buttons were faking with a negative margin, which is the
     * "bad styling" in the report.
     */
    <div className={styles.projectMenu} ref={anchor}>
      <button type="button" className={styles.add} title="Open project" onClick={browse}>
        +
      </button>
      {/*
       * A separate control rather than a long-press or a right-click on `+`. Both of those are
       * gestures nobody discovers, and the request was for a *selector* — something visible that
       * says a list exists.
       */}
      <button
        type="button"
        className={styles.recent}
        title="Recent projects"
        aria-label="Recent projects"
        aria-haspopup="menu"
        aria-expanded={isOpen}
        onClick={openRecents}
      >
        <Icon name="chevron-down" size={1} />
      </button>
      {menu}
    </div>
  )
}
