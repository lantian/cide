/**
 * The wording and the decision behind the close confirmation.
 *
 * Split out of `CloseConfirm.tsx` because this half is where the mistakes are, and this half
 * can be tested. `ui/scripts/check-close-confirm.mjs` compiles this file on its own and
 * asserts the two rules that matter:
 *
 *  1. The dialog appears **only** when something is genuinely at risk. A confirmation the
 *     user sees on every close is one they learn to dismiss unread, and a guard that is
 *     dismissed unread has stopped being a guard. `atRisk` is the whole of that decision.
 *  2. It **names** what would be lost. "3 items" tells a user nothing they can act on;
 *     `main.rs`, `Cargo.toml`, `notes.md` tells them whether to stop.
 *
 * DOM-free and import-free, so the check script can compile it with nothing but `tsc` — the
 * same shape as `overlays/score.ts` and `store/statusFormat.ts`. The types below are
 * structural subsets of the generated `UnsavedTab` and `SessionSummary`, declared here
 * rather than imported for that reason; TypeScript checks the real ones against them at
 * every call site in `CloseConfirm.tsx`.
 */

/** The subset of the generated `UnsavedTab` this module needs. */
export interface UnsavedLike {
  /** Absolute path of the file. */
  path: string
  /** The basename, as the tab strip draws it. */
  title: string
  projectName: string
}

/**
 * The subset of the generated `SessionSummary` this module needs.
 *
 * `SessionState` is tagged `state`, not `kind` — the one enum in the wire format that is —
 * so `session.state.state` is correct and not a typo.
 */
export interface SessionLike {
  paneTitle: string
  projectName: string
  state: { state: string }
}

/** What the user asked to close. Only the wording depends on it. */
export type CloseScope = 'pane' | 'tab' | 'project' | 'window' | 'app'

/** Everything a close would cost, in the shape `app.quitRequested` answers. */
export interface CloseRisk {
  unsaved: UnsavedLike[]
  sessions: SessionLike[]
}

/**
 * Whether to put a dialog in front of this close.
 *
 * The negative case is the important one: with nothing unsaved and nothing mid-turn the
 * close must go straight through. Rust agrees independently — `close_tab` refuses only a
 * dirty tab — so a bug here that fired the dialog too often would be annoying, and a bug
 * that fired it too rarely would still be caught by the refusal. That asymmetry is the
 * design.
 */
export function atRisk(risk: CloseRisk): boolean {
  return risk.unsaved.length > 0 || risk.sessions.length > 0
}

/** The heading. Names the single file when there is exactly one, because that is clearest. */
export function confirmTitle(scope: CloseScope, risk: CloseRisk): string {
  // A pane close discards one editor's buffer, so it names the file for the same reason a
  // single-file tab close does: "1 file with unsaved changes" tells the user nothing they
  // can act on when they are looking straight at it.
  if (risk.unsaved.length === 1 && (scope === 'tab' || scope === 'pane')) {
    return `${risk.unsaved[0]!.title} has unsaved changes`
  }
  if (risk.unsaved.length > 0) {
    return `${count(risk.unsaved.length, 'file')} with unsaved changes`
  }
  return `${count(risk.sessions.length, 'Claude session')} still working`
}

/**
 * The sentence under the heading.
 *
 * States the consequence in the two different registers the two risks deserve: unsaved
 * edits are *gone*, an interrupted turn *resumes*. Flattening both into "are you sure" is
 * what makes a dialog worth ignoring.
 */
export function confirmBody(scope: CloseScope, risk: CloseRisk): string {
  const what = subject(scope)
  if (risk.unsaved.length > 0 && risk.sessions.length > 0) {
    return `Closing ${what} discards these edits — they exist nowhere else. The listed sessions lose their current turn; their conversations resume next time.`
  }
  if (risk.unsaved.length > 0) {
    return `Closing ${what} discards these edits. They exist nowhere else.`
  }
  return `Closing ${what} ends the current turn. The conversations themselves are kept and resume next time.`
}

/** The destructive button. Says what it destroys, never just "OK". */
export function confirmLabel(risk: CloseRisk): string {
  return risk.unsaved.length > 0 ? 'Discard changes and close' : 'Close anyway'
}

/**
 * One row per unsaved file: the name the tab strip shows, and the directory holding it.
 *
 * The directory is not decoration. Two tabs called `mod.rs` are the normal case in a Rust
 * tree, and a dialog that lists `mod.rs` twice has told the user nothing.
 */
export function unsavedRow(tab: UnsavedLike): { name: string; where: string } {
  return { name: tab.title, where: dirname(tab.path) }
}

/** One row per interrupted session: the pane's title, and what it is doing. */
export function sessionRow(session: SessionLike): { name: string; where: string } {
  return { name: session.paneTitle, where: stateLabel(session.state.state) }
}

/**
 * `busy` / `awaitingPermission` as the status bar words them.
 *
 * An unknown tag falls back to the raw string rather than to "unknown": a state added to
 * `SessionState` in Rust and not here should still show *something* true.
 */
export function stateLabel(state: string): string {
  if (state === 'busy') return 'working'
  if (state === 'awaitingPermission') return 'waiting for permission'
  if (state === 'idle') return 'idle'
  return state
}

/** The directory part of an absolute POSIX path; `/` for a file at the root. */
export function dirname(path: string): string {
  const cut = path.lastIndexOf('/')
  if (cut < 0) return ''
  return cut === 0 ? '/' : path.slice(0, cut)
}

/** `1 file` / `3 files`. Written out because "1 files" in a dialog is unforgivable. */
export function count(n: number, noun: string): string {
  return n === 1 ? `1 ${noun}` : `${n} ${noun}s`
}

/** What the sentence is about: the thing being closed. */
function subject(scope: CloseScope): string {
  switch (scope) {
    case 'pane':
      return 'this pane'
    case 'tab':
      return 'this tab'
    case 'project':
      return 'this project'
    case 'window':
      return 'this window'
    case 'app':
      return 'cide'
  }
}
