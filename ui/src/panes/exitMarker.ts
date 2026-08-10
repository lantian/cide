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
 * * `undefined` — this pane learned about the exit from `session.hasExited`, which answers a
 *   boolean. That is the rehydration path: a host evicted and re-created after its child had
 *   already gone, where the `cide://session-state` event fired before anything was listening.
 *   No number exists to show.
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
 */
export function exitMarkerBytes(code?: number): string {
  return `\r\n\x1b[2m${exitMarkerText(code)}\x1b[0m\r\n`
}
