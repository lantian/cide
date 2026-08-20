/**
 * The `/rename` names, mirrored into the webview and refreshed on the gesture that needs them.
 *
 * `claudeSessions.ts` decides what the *Send this to Claude ▸* submenu says; this is where the
 * one fact it cannot derive comes from. The names live in the CLI's own files under
 * `~/.claude/sessions`, `cide_claude::roster` reads them and `claude_session_names` answers
 * with a `{conversationId: name}` map.
 *
 * # Why this is polled and not pushed, when everything else in this app is pushed
 *
 * There is no event to push. `/rename` is typed into a `claude` that tells cide nothing about
 * it: the only trace is a file rewritten under a directory cide does not own. The two ways to
 * turn that into an event both cost more than the string is worth:
 *
 * * **watch the directory.** Those records carry a `status` that moves several times per turn
 *   for every session on the machine, so a watcher would wake, re-read and broadcast to every
 *   window continuously — to keep a label that is read when a context menu opens.
 * * **poll on a timer.** Same traffic, spread out, and still wrong the moment the interval is
 *   longer than the gap between a rename and the next right-click.
 *
 * So it is fetched **when a menu that shows it is opening**. `useCodeMenu` calls [`refresh`]
 * as it builds its items, and the submenu is built later still — when the row is hovered — so
 * an ordinary open has the answer well before anything can read it. The cost is one round trip
 * per context menu in an editor, against a directory read of a handful of small files.
 *
 * # What the staleness window actually is, stated rather than implied
 *
 * A submenu hovered in the same few milliseconds as the parent menu opened can show the
 * previous answer: the names then fall back to pane titles, or to the name a conversation had
 * before it was renamed *during the life of this one menu*. Nothing is wrong beyond the label
 * — the row still carries the `PaneId`, so the send goes where the row said it would — and the
 * next open is correct. Refusing to draw the submenu until a fetch resolved would trade that
 * for a menu that hangs, which is the worse of the two.
 *
 * # Why a store rather than a module-level variable
 *
 * Nothing subscribes to it today: the menu reads it imperatively at build time, because a
 * submenu that re-rendered under the pointer while the user was reading it would move rows
 * out from under a click. It is a store anyway so that a surface which *does* want to
 * re-render on a rename — a pane title bar, a session switcher — can select from it without a
 * second copy of the fetch policy appearing beside this one.
 */
import { create } from 'zustand'
import { claudeSend, diag } from '@/ipc/client'

interface NamesStore {
  /** Keyed by the conversation id a pane is addressed under. Absent ⇒ nobody has named it. */
  bySession: Record<string, string>
  set: (names: Record<string, string>) => void
}

export const useClaudeNames = create<NamesStore>((set) => ({
  bySession: {},
  set: (names) => set({ bySession: names }),
}))

/**
 * The names as they stand, for a caller that must not re-render to read them.
 *
 * The menu's route. `useClaudeNames(s => s.bySession)` would be the React one and is wrong for
 * a menu: the entries are resolved once, at open time, precisely so the list cannot shuffle
 * under a pointer that is already moving towards a row.
 */
export function claudeNames(): Record<string, string> {
  return useClaudeNames.getState().bySession
}

/** One fetch at a time. A second caller during a flight joins it rather than starting another. */
let inFlight: Promise<void> | null = null

/**
 * Ask Rust for the current names. Fire and forget.
 *
 * Never rejects, and that is deliberate rather than sloppy: `chrome/Failures.tsx` turns an
 * unhandled rejection into a toast, and a toast is the wrong answer to "the labels in a submenu
 * you have not opened yet may be a few seconds old". The command itself answers `{}` for every
 * ordinary way of having nothing to read — no `~/.claude`, a relocated `CLAUDE_CONFIG_DIR`, a
 * machine where `claude` has never run — so anything that gets here is a genuine IPC failure,
 * which is worth a line in the log and nothing on screen.
 *
 * Contrast `claudeSend.lines`, three files away, which deliberately does **not** catch: a send
 * that vanished is indistinguishable from a control wired to nothing and has to be said out
 * loud. This is a label refresh. The two are not the same kind of failure and are not reported
 * the same way.
 */
export function refreshClaudeNames(): void {
  if (inFlight !== null) return
  inFlight = claudeSend
    .names()
    .then((names) => {
      useClaudeNames.getState().set(names)
    })
    .catch((error: unknown) => {
      void diag.log(`editor: could not read Claude session names — ${String(error)}`)
    })
    .finally(() => {
      inFlight = null
    })
}
