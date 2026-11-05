# ADR 0016 — Sessions no pane names are ended after a timeout

**Status:** accepted (M196)
**Date:** 2026-11-05

## Context

`closeTab` keeps the tab's children alive so Ctrl+Shift+T can re-adopt them. The undo stack holds
sixteen closes, shared across projects; a child pushed off it had no route back and no owner, and
closing the project did not catch it either, because `sessions_of` walks the tree. A ten-hour
instance measured 54 `claude` processes at about 350 MB each.

## Decision 1 — detached only

`reaper` ends a session only when **no pane names it**, read from the tree with
`workspace::session_panes`. A session idling in an open tab is left alone, however long it has
been idle. `PtySession::sink_count` is not the test: a parked tab or an evicted host has no sink
and is still open.

## Decision 2 — a finished turn holds nothing

Only `running::pane_is_busy` with `Busy::Claimed` holds a session: mid-turn, a permission prompt,
frozen, on a splash, or a prompted child that has not reported yet. `AwaitingInput` and the
`windows::awaiting` set look as if they belong on that list and must not be added.
`cide_claude::next_state` turns every `Stop` out of `Busy` into `AwaitingInput`, so that state is
*the* state of a finished session in a closed tab, which is the population this feature exists for.
Nobody can clear that marker either, because its tab is not open. The accepted cost: a Codex
`request_user_input` also reads `AwaitingInput`, so a question asked in a tab closed for the whole
timeout goes unanswered, and the conversation resumes from its transcript.

## Decision 3 — the undo stack is not exempt, and a reopen resumes

The first version exempted `ClosedTabs`, because restoring a pane onto an ended child showed a dead
pane. That left sixteen closed tabs alive indefinitely, so the reopen was fixed instead. A reopened
pane whose child has gone now resumes without being asked, from both routes back
(`tab_reopen_closed`, and `tab_reopen_file` for a File tab with a Claude split):

- `claude_tab::resume_closed` spawns `--resume` before the tab is back in the tree. Without it, a
  pane naming a session nobody holds spawns a **fresh** `claude`, which is worse than a dead pane
  because it looks fine.
- **A user's tab resumes with no stance args.** `stance_args` is the unattended stance for tabs cide
  opened, and reusing it would raise a user tab's permissions on reopen.
- `cide://panes-retired` destroys the parked host in every window, because its sink is tied to the
  dead child. It also writes the new id to the ledger, which is what skips the Resume splash; a
  stale restore-plan entry would otherwise offer to start a child that is already running. A reopen
  emits it before the reinsert's broadcast. This also frees the terminal's memory.
- `reaper::wait_out` holds a reopen until an in-progress ladder finishes.

The cost: Ctrl+Shift+T after the timeout replays a `--resume` rather than re-adopting a live child,
and a conversation with no transcript, or a spawn that fails, comes back as a fresh session.

## Decision 4 — anything a run names is left to the run

`AgentRegistry::names_session`, not `owns_session`. The latter goes false when a run ends, and a
finished run's retained entry is what its History row adopts; losing it spawned a fresh `claude`
over an opencode run's rendered log. The cost is that a run's exited entries stay until quit.

## Decision 5 — the graceful ladder, then removal

`lifecycle::stop_children`, then `SessionRegistry::remove`, as `stop_closed_sessions` does. The
removal matters as much as the kill: `sessionIsHeld` adopts a retained entry, so leaving it would
show `— exited —` where a `--resume` belongs. A child that outlives the ladder stays registered,
because removing the handle to a running process is how one outlives the app.

## Consequences

- Default on at 30 minutes; `0` turns it off, exited entries included.
- A shell's foreground job holds the session once it has run past the job-notification delay
  (`lifecycle::watch_jobs` reports it as `Busy`). A background job started from a shell in a closed
  tab is not seen, so the shell reads as idle and is ended with the job still in it.
- Claude's own background tasks are not visible in its state, so one still running after `Stop` is
  ended too.
- Only Claude is lossless. A codex or opencode console that never bound a conversation id has
  nothing to resume from.
