/**
 * The popup a held-modifier switcher puts up — for projects (Ctrl+`) and for tabs (Ctrl+Tab).
 *
 * **Presentation only.** It renders `useSwitcher`'s walk and nothing else: no keyboard
 * handling, no listeners, no focus. That is deliberate and it is the reason the gesture is
 * correct in a window where this never mounts — the walk, its capture on the walk key and on
 * Escape, and the release watcher all live in `keys/switcherStore.ts`, so the worst a missing
 * popup can do is switch invisibly. A component that owned the keyup would instead open a walk
 * nothing could close, and the capture would swallow Tab for the rest of the session.
 *
 * It also takes no focus, on purpose. The keystrokes that drive it are being delivered to
 * whatever the user was working in, and moving focus mid-gesture is one of the ways a keyup
 * goes missing — see the `blur` branch of `armRelease`.
 *
 * # One popup, two lists
 *
 * The walk's [`SwitchTarget`] says which, and the difference is only what a row draws: a
 * project's dot, name and path, or a tab's badge and title. The rows themselves come from the
 * same `walk.order`, and the header and hint are derived rather than written twice — the hint
 * names the key that is actually being held, because a user can rebind either chord and a
 * hard-coded "Release Ctrl · Tab to move" would be an instruction to press something that does
 * nothing. `keys/switcher.ts::hintFor` is the pure half of that.
 *
 * # Mounting
 *
 * ```tsx
 * import { Switcher } from '@/chrome/Switcher'
 * …
 * <Switcher />
 * ```
 *
 * Beside `<Failures />` in `App.tsx`, and for the same reason that one takes no props: it
 * reads its own store and renders `null` until there is something to show.
 */
import { useSwitcher } from '@/keys/switcherStore'
import { useWorkspace } from '@/store/workspace'
import { hintFor, selection } from '@/keys/switcher'
import type { Project, Tab } from '@/ipc/client'
import { Icon, type IconName } from '@/icons/Icon'

import styles from './Switcher.module.css'

export function Switcher() {
  const walk = useSwitcher((s) => s.walk)
  const target = useSwitcher((s) => s.target)
  // The workspace records, for names, dots and badges. Subscribed to `boot` — one reference —
  // rather than to a derived object, because zustand compares selector results with `Object.is`
  // and a fresh record every notification would re-render this on every keystroke in the app.
  const boot = useWorkspace((s) => s.boot)

  if (walk === null || target === null) return null
  const projects = boot?.workspace.projects
  if (projects === undefined) return null

  const chosen = selection(walk)
  // For a tab walk, the project whose strip is being walked — captured when the popup opened,
  // so a project activated underneath it cannot change which tabs these ids name.
  const tabs = target.project === null ? undefined : projects[target.project]?.tabs

  return (
    // `aria-live` rather than `role="dialog"`: nothing here is focusable and there is no
    // interaction to trap, so the useful thing to announce is which row is now selected.
    <div className={styles.popup} data-audit="switcher" aria-live="polite">
      <div className={styles.head}>
        {target.kind === 'project' ? 'Switch project' : 'Switch tab'}
      </div>
      <ul className={styles.list}>
        {walk.order.map((id) => {
          const row =
            target.kind === 'project'
              ? projectRow(projects[id])
              : tabRow(tabs?.find((tab) => tab.id === id))
          // A project or tab that closed mid-walk. Skipped rather than drawn as a blank row:
          // the commit reconciles the same way and would not activate it either.
          if (row === null) return null
          const selected = id === chosen
          return (
            <li
              key={id}
              className={selected ? `${styles.row} ${styles.selected}` : styles.row}
              aria-current={selected ? 'true' : undefined}
            >
              {row.dot === null ? (
                <span className={styles.mark} aria-hidden="true">
                  {row.mark === '' ? null : <Icon name={row.mark} size={1} />}
                </span>
              ) : (
                <span className={styles.dot} style={{ background: row.dot }} aria-hidden="true" />
              )}
              <span className={styles.name}>{row.name}</span>
              <span className={styles.path}>{row.detail}</span>
            </li>
          )
        })}
      </ul>
      {/* Spelled out because the gesture is not discoverable: nothing on screen says that
          holding the modifier is what keeps this open. The release is named first and names
          the key — the earlier wording led with the two Tab strokes, and read as a demand for
          one more keypress rather than as "you are already holding the thing that ends it". */}
      <div className={styles.hint}>{hintFor(walk)}</div>
    </div>
  )
}

/**
 * One row's contents. `dot` is a project's colour; a tab draws a `mark` instead.
 *
 * `IconName | ''` rather than `IconName | null` because a project row genuinely has no mark and
 * the empty string is what the dot-or-mark branch below already tests for. The marks were
 * `◆ ▤ ± ⚙ ·` — five characters from five different parts of Unicode, at five apparent weights.
 */
interface Row {
  dot: string | null
  mark: IconName | ''
  name: string
  detail: string
}

function projectRow(project: Project | undefined): Row | null {
  if (project === undefined) return null
  return { dot: project.dot, mark: '', name: project.name, detail: project.displayPath }
}

/**
 * A tab row: what the tab is, and what it is about.
 *
 * The vocabulary is `TabStrip`'s and is deliberately *narrower* than its `viewFor` — that
 * function returns JSX with CSS-module classes bound to the strip's own stylesheet, and
 * importing it here would put two surfaces' geometry in one place for the sake of a badge. What
 * matters is that the words agree, so the console reads "Claude" here as it does there.
 */
function tabRow(tab: Tab | undefined): Row | null {
  if (tab === undefined) return null
  switch (tab.kind.kind) {
    case 'claudeHome':
      return { dot: null, mark: 'message-square', name: 'Claude', detail: 'console' }
    case 'claudeFull':
      return { dot: null, mark: 'message-square', name: tab.kind.title, detail: 'session' }
    case 'file':
      return { dot: null, mark: 'file', name: basename(tab.kind.path), detail: tab.kind.path }
    case 'diff':
      return {
        dot: null,
        mark: 'file-diff',
        name: tab.kind.spec.title,
        detail: tab.kind.spec.newPath,
      }
    case 'settings':
      return { dot: null, mark: 'settings', name: 'Settings', detail: '' }
    case 'docs':
      return { dot: null, mark: 'book-open-text', name: tab.kind.title, detail: 'documentation' }
    default:
      // A `TabKind` variant added later. A row that says *something* beats a popup with a hole
      // in it, and `TabStrip.viewFor` is where the compiler is made to care.
      return { dot: null, mark: 'circle-slash', name: 'Tab', detail: '' }
  }
}

function basename(path: string): string {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  return path.slice(cut + 1)
}
