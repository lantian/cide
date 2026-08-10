# ADR 0008 — A `SIGKILL` of cide must not leave `claude` children behind

**Status:** accepted (M11), partially implemented
**Date:** 2026-08-10

## Context

cide spawns two kinds of child process:

* **PTY children** — one per pane, through `cide-pty` and therefore through `portable-pty`.
  These are the `claude` sessions and the shells.
* **One-shot children** — `cide-claude::headless`, for commit-message generation and
  "explain this selection".

It also owns one unix socket, `$XDG_RUNTIME_DIR/cide-hooks-<pid>.sock`, which every child's
hooks report to.

A clean quit is handled: `lifecycle::shutdown` kills the sessions and drops the `HookServer`,
whose `Drop` unlinks the socket. A `SIGKILL` runs none of that, and `SIGKILL` is not an exotic
case here — an OOM kill, `kill -9` after a wedged webview, or a compositor tearing down the
session all produce it.

What is actually left behind after `kill -9`, on this machine:

* Every PTY child keeps running. `portable-pty` calls `setsid()` in its own `pre_exec`, so
  each child is a session leader in its own process group; it does not even receive `SIGHUP`
  when the master fd closes. A `claude` in that state holds a subscription slot, keeps its
  transcript open, and is invisible to the user until they go looking in `ps`.
* One `cide-hooks-<pid>.sock` accumulates per hard kill, for the life of the account.

The kernel has a mechanism for the first: `prctl(PR_SET_PDEATHSIG, sig)`. Three properties of
it decide how it can be used, and all three fail silently if got wrong.

1. **It is cleared for the child of `fork(2)`.** Arming the parent and then spawning does
   nothing at all. It must be set in the child, between `fork` and `exec` — a `pre_exec` hook.
   (The brief for this work asserted the opposite, that the setting is inherited across
   `fork`. It is not; `copy_process()` zeroes `pdeath_signal`. Building on that reading would
   have produced a no-op that passes every test one could write for it.)
2. **It survives `execve(2)`**, except when the image is set-user-ID, set-group-ID, or carries
   file capabilities. That is what makes the `pre_exec` approach work at all, and also why
   this is a degradation rather than a guarantee for an arbitrary `$SHELL`.
3. **The "parent" is the *thread* that forked, not the process.** The signal is delivered when
   that thread exits, with the process still very much alive. A pane spawned from a pooled
   Tauri command worker would be killed seconds after it opened, for no reason a user could
   diagnose.

There is a fourth, smaller trap: if the parent dies between the `fork` and the `prctl`, the
death notification has already happened and the signal is never delivered.

## Decision

**Children are armed with `PR_SET_PDEATHSIG = SIGTERM` from inside a `pre_exec` hook**, and
every spawn that is armed runs on a single process-lifetime thread.

`cide-claude::orphans` holds all of it:

* `arm(&mut Command)` installs the hook, capturing the parent pid *before* the fork so the
  child can detect the race in fact 4 and `raise()` on itself when it has already been
  orphaned.
* `set_parent_death_signal(expected_parent, signal)` is the in-child primitive, public so a
  spawner in another crate can use the same implementation rather than a second copy.
* `on_spawn_thread(f)` runs a spawn on one named thread (`cide-spawn`) that is created once
  and never joined, which is the whole answer to fact 3.

`SIGTERM` rather than `SIGKILL`: it gives `claude` its normal exit path and gives a shell the
chance to finish a `write(2)` into one of the user's files. Neither program ignores `SIGTERM`,
so the guarantee `SIGKILL` would buy is worth less than the truncated file it risks.

**Sockets left by a previous run are swept at startup**, before the new listener binds, by
`orphans::sweep_hook_sockets`. It mirrors `cide_ide_mcp::lockfile::sweep_stale` in shape and
in caution, and *calls that module's `pid_is_alive`* rather than restating the rule: only
`ESRCH` counts as death, because `EPERM` is another user's live cide in a shared `/tmp` and
unlinking its socket would take hooks away from a running application that has no idea we
exist. The bias is towards leaving a file behind.

## Consequences

**One-shot children are covered.** `cide_claude::headless::run` arms its child, and the thread
that forks it is the same thread that then blocks waiting for it — so fact 3 holds by
construction rather than by convention.

**PTY children are not yet covered, and this is the known gap.** `portable-pty` 0.9.0's
`CommandBuilder` exposes no `pre_exec` hook; `PtySystem::spawn_command` installs its own
(`setsid`, `TIOCSCTTY`, fd cleanup, umask) and there is no way to compose with it from
outside the crate. Closing this requires one of:

* a `pre_exec` escape hatch on `SpawnSpec`, threaded through `cide-pty` to a fork of
  `portable-pty`'s spawn path;
* upstreaming a `CommandBuilder::pre_exec` into `portable-pty`;
* a tiny setuid-free launcher binary shipped beside `cide` that arms itself and `execve`s the
  real program — which preserves the child's pid, and the pid is load-bearing here because it
  is the join key between a PTY session and an `ide_connected` notification.

Until one of those lands, a `SIGKILL` still orphans panes. The startup socket sweep and the
CLI-side lockfile pruning both already assume that a hard kill happens, so nothing regresses;
the orphaned processes simply remain until the user ends them.

**Every armed spawn must go through `on_spawn_thread`.** This is the rule most likely to be
broken by a later change, because breaking it produces a pane that opens and then dies a few
seconds later — which reads as a `claude` crash, not as a threading mistake. `session_spawn`
is the call site that has to observe it.

## Alternatives considered

**A supervising thread that `waitpid`s and kills.** Does nothing under `SIGKILL`: the
supervisor dies with everything else. It is the mechanism cide already has for a *clean* quit.

**A process group killed at exit.** Same problem, plus `portable-pty` puts each child in its
own session, so there is no shared group to signal.

**`PR_SET_CHILD_SUBREAPER`.** Solves the opposite problem — reparenting orphaned
grandchildren to us — and does nothing about our own death.

**`SIGKILL` as the death signal.** Rejected above: guaranteed death is not worth a truncated
file in the user's repository.
