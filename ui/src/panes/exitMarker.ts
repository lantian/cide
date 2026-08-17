/**
 * The line a pane prints once its child is gone.
 *
 * A pure module with no React and no xterm in it, for one reason: it is the only place the
 * exit code becomes something a user reads, and `check-exit-marker.mjs` can compile and run
 * this on its own. Inlined in `TerminalPane` it would be untestable, which is how the code
 * came to be plumbed all the way out of `wait()` and then dropped on the floor one line
 * before the screen.
 *
 * Deliberately *not* a React node. It belongs at the end of the transcript, where a shell
 * would print it, so it goes through `term.write` and then survives scrollback, re-mounts
 * and detach the same way every other byte in the pane does. See `markExited`.
 */

/**
 * Whether `code` is worth showing the user.
 *
 * Three cases, and only one of them is a number anybody wants on screen:
 *
 * * `undefined` — nothing can say. That is the rehydration path: a host evicted and re-created
 *   after its child had already gone, where the `cide://session-state` event fired before
 *   anything was listening. It used to mean "this pane asked `session.hasExited`, which
 *   answers a boolean" — a code that existed and was never requested. {@link markFor} narrows
 *   it to the one case where no number exists anywhere: a session the registry has never
 *   heard of, restored from `workspace.json` after the process that owned it died with the
 *   app. Every other rehydration now arrives through `session_exit` with its status intact.
 * * `0` — it finished, and it worked. `— exited (0) —` is noise on the ordinary case, and
 *   noise on the ordinary case is how a marker stops being read at all.
 * * negative — `cide_pty::UNKNOWN_EXIT_CODE`, meaning `wait()` itself failed and this build
 *   genuinely cannot say. Printing `(-1)` invites a user to look up a status that does not
 *   exist; the tracing warning carries the real story for anyone reading the log.
 *
 * Everything else is the shell's own convention and is shown verbatim: `1` for an ordinary
 * failure, `137` for a SIGKILL (the OOM reaper), `143` for the SIGTERM this app sends on
 * quit. Those three used to be indistinguishable, which is what the whole change was for.
 */
export function showsCode(code?: number): boolean {
  return code !== undefined && Number.isInteger(code) && code > 0
}

/** The marker's text, without the SGR framing. */
export function exitMarkerText(code?: number): string {
  return showsCode(code) ? `— exited (${code}) —` : '— exited —'
}

/**
 * The bytes written into the terminal.
 *
 * SGR 2 is dim — the ANSI analogue of the `--faint` token the design uses for spent text —
 * and SGR 0 closes it so the child's own colours are not inherited by whatever a user types
 * next. The leading CRLF is what puts this on its own line even when the child died
 * mid-line, which a killed process usually does.
 *
 * **The opening SGR 0 is not redundant with the closing one.** SGR 2 *adds* dim to whatever
 * attributes are already active; it does not clear them. A child that died inside a coloured
 * prompt — which is most of them, since the shell's prompt is where a pane spends its life —
 * leaves a background colour set, and this line was then drawn on it, full width, right up
 * against the edge of the pane. That is the same defect as the restored-shell banner
 * (`cide_app::lifecycle::restore_notice`, which opens the same way and says why): an
 * attribute set by someone else and never reset.
 */
export function exitMarkerBytes(code?: number): string {
  return `\r\n\x1b[0m\x1b[2m${exitMarkerText(code)}\x1b[0m\r\n`
}

/**
 * What the registry can still say about a session whose pane was not there to hear it die.
 *
 * The rehydration path — a host evicted and re-created after its child had already gone —
 * asks a question rather than listening for an event, and the question it used to ask was
 * `session_has_exited`, which answers a boolean. That is why the marker on that path never
 * carried a number: the number existed in the registry the whole time and was simply not on
 * the wire. `session_exit` now carries it, and `TerminalPane` hands its answer straight to
 * {@link markFor}.
 *
 * **This type is declared here rather than imported from the generated bindings**, which is a
 * deliberate cost. `check-exit-marker.mjs` compiles this module standalone with `tsc`, with no
 * path aliases and no bundler, because it is the only place the exit code becomes something a
 * user reads and that decision deserves a test that does not need a DOM. So the equivalence
 * with Rust's `SessionExit` is enforced at the call site instead: `TerminalPane` passes the
 * generated type into {@link markFor}, and structural typing fails the build if the two ever
 * diverge. A variant added in Rust and missed here is a type error, not a silent fallthrough.
 *
 * Four answers, because `cide-pty` genuinely has four states to report and collapsing any two
 * of them loses the code again:
 *
 * * `running` — the child is alive. The pane was rehydrated over a session that outlived it,
 *   which is the ordinary case and the reason the question is asked at all.
 * * `reaping` — `Session::has_exited()` is true but `Session::exit_status()` is still `None`.
 *   EOF on the pty master and the reaper's `wait()` are two events on two threads, and this is
 *   the window between them. The code exists and is milliseconds away on
 *   `cide://session-state`.
 * * `exited` — reaped, with the status `wait()` returned.
 * * `unknown` — the registry never held this id. A `SessionId` restored from `workspace.json`
 *   after a restart is exactly this: the process that owned it is gone and no code survives
 *   anywhere. This is the only answer for which a bare `— exited —` is the truth.
 *
 * Mirrors `SessionExit` in `cide-ipc`, which is the command's return type.
 */
export type ExitAnswer =
  | { kind: 'running' }
  | { kind: 'reaping' }
  | { kind: 'exited'; code: number }
  | { kind: 'unknown' }

/**
 * What the one-shot rehydration check should do with that answer.
 *
 * `null` means *do not write the marker*, and it covers two different reasons that must not be
 * merged. `running` is obvious. `reaping` is the interesting one: the pane is about to be told
 * the real code by `cide://session-state`, and `markExited` is one-shot — whichever call
 * arrives first is the one whose code is shown — so marking now would win the race and print
 * the codeless line a fraction of a second before the number turned up. Losing the code to a
 * thread scheduling accident is precisely the failure this whole path exists to end, so the
 * check declines and lets the event do it.
 *
 * Otherwise it is the argument for `exitMarkerBytes`: a real status, or `undefined` for the
 * session nothing can speak for.
 */
export function markFor(answer: ExitAnswer): { code?: number } | null {
  switch (answer.kind) {
    case 'running':
    case 'reaping':
      return null
    case 'exited':
      return { code: answer.code }
    case 'unknown':
      return {}
  }
}

/**
 * The line a pane prints when its child never started.
 *
 * A pane whose spawn fails is *blank*. It looks exactly like a pane that is still connecting,
 * and exactly like one whose terminal failed to paint — three very different problems wearing
 * one appearance, and the only record was a `console.error` in a webview whose console is not
 * reachable from a shell on Wayland. That is how `SessionError::AlreadyOpen` shipped invisible:
 * the domain refused the spawn for a good reason, said so precisely, and nobody could read it.
 *
 * Written into the terminal rather than rendered as chrome, for the same reason as
 * {@link exitMarkerBytes}: it belongs in the transcript, where it survives scrollback, a
 * re-mount and a detach, and where the user is already looking. A toast would be in the
 * corner, describing a pane it cannot point at.
 *
 * The framing is {@link exitMarkerBytes}' — including the leading SGR 0, which is not
 * redundant. A pane that fails to spawn has written nothing, so nothing is set; a pane that
 * *re*-spawns after a child died inside a coloured prompt has a background still active, and
 * this line would otherwise be drawn on it full width.
 */
export function spawnFailureBytes(message: string): string {
  return `\r\n\x1b[0m\x1b[2m— ${message} —\x1b[0m\r\n`
}

/**
 * Turn a rejected `session.spawn` into a sentence for the pane.
 *
 * Errors cross the IPC boundary tagged — `{ kind, message }` — and `message` is the half
 * written for a person, so it is preferred verbatim. `kind` is deliberately not shown: the
 * user does not need to know it is called `alreadyOpen`, and the message already says a
 * conversation cannot be resumed twice.
 *
 * The fallback matters more than it looks. A rejection that is not our tagged error — a
 * command that does not exist because the binary is stale, which has happened in this project
 * — arrives as a bare string or an `Error`, and rendering `[object Object]` into the pane
 * would replace one unreadable state with another.
 */
/**
 * Whether a rejected session command means *the id this pane was holding is gone*.
 *
 * `SessionError` crosses the wire tagged — `{ kind, message }` — precisely so the frontend can
 * branch on the variant instead of matching on prose, and this is the branch that matters.
 * `noSuchSession` is the registry's correct refusal to answer for an id it has never held, and
 * printing it into the pane was reporting an internal bookkeeping fact to a user who can do
 * nothing with it: `— no such session —` on a blank pane, on every launch, for every pane the
 * user had torn into its own window. It is *recoverable* — forget the id and start a session —
 * and every other kind is not, which is what this predicate is for.
 *
 * Deliberately narrow. `alreadyOpen` is a refusal the user must see (a conversation cannot be
 * resumed twice), and `pty` is a real failure to start a process; retrying either would loop.
 * `noClaudeBinary` (M16) is the same: the configured program cannot be executed, so a retry
 * cannot help and a pane that retried would spin. Its `message` is a written sentence naming
 * the value and where to correct it, which `spawnFailureText` below prefers verbatim — so the
 * only thing this predicate has to do about it is stay silent.
 */
export function isRecoverableSessionError(reason: unknown): boolean {
  if (reason === null || typeof reason !== 'object') return false
  return (reason as { kind?: unknown }).kind === 'noSuchSession'
}

export function spawnFailureText(reason: unknown): string {
  if (typeof reason === 'string' && reason.length > 0) return reason
  if (reason instanceof Error && reason.message.length > 0) return reason.message
  if (reason !== null && typeof reason === 'object') {
    const message = (reason as { message?: unknown }).message
    if (typeof message === 'string' && message.length > 0) return message
    const kind = (reason as { kind?: unknown }).kind
    if (typeof kind === 'string' && kind.length > 0) return `session error: ${kind}`
  }
  return 'this pane could not start, and gave no reason'
}
