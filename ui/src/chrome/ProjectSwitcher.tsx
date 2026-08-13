/**
 * The popup Ctrl+Tab puts up while the modifier is held.
 *
 * **Presentation only.** It renders `useSwitcher`'s walk and nothing else: no keyboard
 * handling, no listeners, no focus. That is deliberate and it is the reason the gesture is
 * correct in a window where this never mounts — the walk, its capture on Tab and Escape, and
 * the release watcher all live in `keys/switcherStore.ts`, so the worst a missing popup can
 * do is switch projects invisibly. A component that owned the keyup would instead open a walk
 * nothing could close, and the capture would swallow Tab for the rest of the session.
 *
 * It also takes no focus, on purpose. The keystrokes that drive it are being delivered to
 * whatever the user was working in, and moving focus mid-gesture is one of the ways a keyup
 * goes missing — see the `blur` branch of `armRelease`.
 *
 * # Mounting
 *
 * ```tsx
 * import { ProjectSwitcher } from '@/chrome/ProjectSwitcher'
 * …
 * <ProjectSwitcher />
 * ```
 *
 * Beside `<Failures />` in `App.tsx`, and for the same reason that one takes no props: it
 * reads its own store and renders `null` until there is something to show.
 */
import { useSwitcher } from '@/keys/switcherStore'
import { useWorkspace } from '@/store/workspace'
import { selection } from '@/keys/switcher'
import type { Project } from '@/ipc/client'
import styles from './ProjectSwitcher.module.css'

export function ProjectSwitcher() {
  const walk = useSwitcher((s) => s.walk)
  // The project records, for names and dots. Subscribed to `boot` — one reference — rather
  // than to a derived object, because zustand compares selector results with `Object.is` and
  // a fresh record every notification would re-render this on every keystroke in the app.
  const boot = useWorkspace((s) => s.boot)

  if (walk === null) return null
  const projects = boot?.workspace.projects
  if (projects === undefined) return null

  const chosen = selection(walk)

  return (
    // `aria-live` rather than `role="dialog"`: nothing here is focusable and there is no
    // interaction to trap, so the useful thing to announce is which project is now selected.
    <div className={styles.popup} data-audit="projectSwitcher" aria-live="polite">
      <div className={styles.head}>Switch project</div>
      <ul className={styles.list}>
        {walk.order.map((id) => {
          const project: Project | undefined = projects[id]
          // A project that closed mid-walk. Skipped rather than drawn as a blank row: the
          // commit reconciles the same way and would not activate it either.
          if (project === undefined) return null
          const selected = id === chosen
          return (
            <li
              key={id}
              className={selected ? `${styles.row} ${styles.selected}` : styles.row}
              aria-current={selected ? 'true' : undefined}
            >
              <span className={styles.dot} style={{ background: project.dot }} aria-hidden="true" />
              <span className={styles.name}>{project.name}</span>
              <span className={styles.path}>{project.displayPath}</span>
            </li>
          )
        })}
      </ul>
      {/* Spelled out because the gesture is not discoverable: nothing on screen says that
          holding the modifier is what keeps this open. The release is named first and names
          the key — the earlier wording led with the two Tab strokes, and read as a demand for
          one more keypress rather than as "you are already holding the thing that ends it". */}
      <div className={styles.hint}>
        Release Ctrl to switch · Tab / Shift+Tab to move · Esc cancels
      </div>
    </div>
  )
}
