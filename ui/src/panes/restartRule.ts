/**
 * What a pane whose child has died offers the user.
 *
 * # The defect this is the answer to
 *
 * Double Ctrl+C quits the Claude CLI — that is the CLI's own policy and cide does not touch it
 * — and what was left behind was a **dead end**. The pane printed `— exited —` and then offered
 * nothing at all: `claude.restart` was in the command registry but marked
 * `unavailable("needs a respawn path in the pane host; kill alone is not a restart")`, so it
 * was greyed out in the palette and no key could be bound to it; the terminal's own context
 * menu said in a comment why it was not there; typing into the pane wrote to a dead pty and the
 * error was discarded; and the project console pane cannot be closed. Quitting the application
 * was the only way out of a pane the user had not meant to close.
 *
 * # Why the decision lives in a module of its own
 *
 * Twice in this project a rule that lived inside a React hook or an event handler shipped
 * wrong, because that is the one place no check script can compile it. This is the rule — what
 * is offered, when, and with which words — and `check-restart.mjs` runs every branch of it with
 * no DOM. `TerminalPane` renders whatever comes back and decides nothing.
 *
 * # Fresh is the default, resume is the second button
 *
 * The two are genuinely different and both are wanted. *Resume* is `claude --resume <id>`: the
 * effective session id stays the same, so `pane.session` needs no correction, and the user gets
 * the conversation they lost. *Fresh* mints a new id and starts an empty conversation. Fresh is
 * primary because that is what was asked for; resume is offered whenever Claude Code still
 * holds a transcript for the session this pane was showing, because a user who double-Ctrl+C'd
 * out of a long conversation almost certainly wants it back and one click is a cheap way to
 * find out.
 *
 * Resume is offered **only when the transcript really exists** — the answer comes from Rust,
 * which asks the filesystem (`lifecycle::resumable`). A Resume button that spawns a
 * `claude --resume` for a transcript that is gone is a control that fails after it is pressed,
 * which is the same defect one layer down.
 */
import { exitMarkerText } from './exitMarker'

/**
 * The two ways back.
 *
 * A string union rather than a boolean `fresh`, because it travels through a command id, a
 * button and an IPC argument, and `restart(false)` at a call site is unreadable in exactly the
 * place a mistake would be expensive.
 */
export type RestartMode = 'fresh' | 'resume'

/** Which of the two programs the pane was running. Mirrors `terminal/keys.ts`'s own kind. */
export type RestartPaneKind = 'claude' | 'shell'

export interface ExitedPaneFacts {
  kind: RestartPaneKind
  /** Whether this pane's child is known to have gone. Nothing is offered over a live one. */
  exited: boolean
  /** The child's status, when anything can say. Rendered by `exitMarker.ts`, not here. */
  code?: number | undefined
  /**
   * Whether Claude Code still holds a transcript for the session this pane held.
   *
   * A claim about the filesystem, answered by `session_resumable` in Rust, not a guess. False
   * whenever nothing has answered yet, which is the safe direction: the offer gains a button
   * when the answer arrives rather than losing one.
   */
  resumable: boolean
}

export interface RestartAction {
  mode: RestartMode
  label: string
}

export interface RestartOffer {
  /** What happened, in the pane's own words. The same sentence the transcript carries. */
  status: string
  /** The accented control. Always present — a pane that offers nothing is the bug. */
  primary: RestartAction
  /** The quieter one, or `null` when there is nothing honest to put there. */
  secondary: RestartAction | null
}

/**
 * What this pane should offer, or `null` for "offer nothing".
 *
 * `null` for a live child, and that is the only `null`. Every exited pane gets a way back,
 * including a shell — a `bash` that exits leaves the identical dead pane, and it is less severe
 * only because a shell pane can be closed.
 */
export function restartOffer(facts: ExitedPaneFacts): RestartOffer | null {
  if (!facts.exited) return null

  const primary: RestartAction = {
    mode: 'fresh',
    label: facts.kind === 'claude' ? 'Start a new session' : 'Restart',
  }

  // A shell has no conversation, so it is never resumable however the flag is set. Rust says
  // the same thing (`restore_for` answers `Fresh` for every non-Claude pane); asserting it here
  // too means a caller that passes the wrong facts cannot draw a button that cannot work.
  const canResume = facts.kind === 'claude' && facts.resumable

  return {
    status: exitMarkerText(facts.code),
    primary,
    secondary: canResume ? { mode: 'resume', label: 'Resume this conversation' } : null,
  }
}
