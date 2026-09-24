# Shared global state across worktrees

For project authors whose roles run tests. This page covers what cide isolates between
concurrent runs, what it doesn't, and the three settings in `.cide/config.json` that close the gap.

## What a worktree does and doesn't isolate

Each run with a task works in its own git worktree, `.cide/worktrees/<role>-<task>`, on branch
`cide/<role>-<task>`. Before `cide_agent_integrate` merges that branch, cide runs the project's
verify command (`milestones.verify`) in the same worktree.

A worktree isolates **the source tree only**. Anything your tools keep in a per-user location is
shared by every run and every verify on the machine:

- `~/.local/share/…` (`$XDG_DATA_HOME`): a Godot game's `user://`, which is
  `$XDG_DATA_HOME/godot/app_userdata/<project>` on Linux, and application databases.
- `~/.cache/…` (`$XDG_CACHE_HOME`): test-runner caches and downloaded toolchains.
- `~/.local/state/…`, `~/.config/…`, `$TMPDIR`.
- Fixed ports, lock files, system services and devices.

One run's tests writing there can therefore fail **another branch's** verify, and integrate
refuses a correct branch. The incident behind this page: a branch that changed only data files
was refused twice. The project's own guard noticed that Godot processes in two other worktrees
had written the shared `user://` (a `.recovery_mode_lock`, and a rotated log).

## `agents.isolateEnv`: a directory of its own per worktree

Set these keys by hand in `.cide/config.json`, or let the orchestrator set them with the
`cide_agents_config` tool. The tool's description explains when to reach for them, and a refused
verify points to that tool when other runs were alive and the project doesn't isolate anything.
The tool refuses `HOME`, unknown variable names and share paths with `..` before writing
anything. `[]` or `null` turns a key off and removes it from the file. The tool warns when
`XDG_CONFIG_HOME` is isolated, and `cide_agents_list` reports the isolation while it is on.
The file is committed, so a change made by the tool goes out with the next commit like any
other.

```json
{
  "agents": {
    "isolateEnv": ["XDG_DATA_HOME", "XDG_CACHE_HOME", "TMPDIR"],
    "isolateEnvShare": ["godot/export_templates"],
    "verifyExclusive": false
  }
}
```

- **What it does.** Each listed variable is pointed at a directory of that worktree's own,
  under cide's cache (`~/.cache/cide/isolated-env/<worktree>/<var>`). This applies to the
  run's child, to the verify of that run's branch, and to any pane opened in that worktree (a
  reopened run, a reviewer tab). All of them compute the directory from the same worktree path,
  so **a run that passes its tests sees the same directories its verify does.**
- **What it may name:** `XDG_DATA_HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME`, `XDG_CONFIG_HOME`,
  `TMPDIR`. Any other name is skipped with a warning in the log. **`HOME` is never overridden**,
  so `~/.claude`, ssh keys and every non-XDG dotfile keep working.
- **Default: off.** An empty list changes nothing, and a project that doesn't set it spawns
  exactly as before.
- **Only runs in a worktree.** A run with no task, or one whose role says `worktree: false`,
  works in the project root, which is the user's own environment, and is not isolated.
- **Shared entries.** Each isolated directory gets a symlink back to the real one for the
  harnesses' own state and logins (`opencode`, `mimocode`, `codex`, `qwen`, `claude`,
  `claude-cli-nodejs`), for `gh` and `git`, and for cide's own directory. Without those links a
  run would start logged out, and an opencode or mimo run's conversation would be filed where
  its resume never looks. `isolateEnvShare` adds more entries, as relative paths that may be
  nested. `godot/export_templates` shares installed export templates while `user://` stays
  private, and `ms-playwright` keeps a cold `XDG_CACHE_HOME` from downloading browsers again
  in every worktree.
- **Cleanup.** The directories are not under the worktree. Anything there would be an untracked
  file, and integrate refuses a checkout with untracked files. They are deleted once their
  worktree is gone, on the next run or verify of any isolating project.

### The `XDG_CONFIG_HOME` caveat

`XDG_CONFIG_HOME` is allowed but rarely what you want. Tools keep credentials and settings there
(`gh`, git credential helpers, cloud CLIs, editors), and any tool not in the shared list is
**logged out in every run**, usually silently: a push that asks for a password nobody will type,
or an API call that fails as unauthenticated. `gh` and `git` are shared by default. Share
anything else your runs need with `isolateEnvShare`, or leave `XDG_CONFIG_HOME` off the list.
`XDG_CACHE_HOME` has a milder cost: a cold cache per worktree, which is slower but correct.

## `agents.verifyExclusive`: one verify at a time

For shared state that environment variables can't reach, such as a fixed port, a system
service, a device or a database server. With it on, a project's verifies **queue** behind each
other rather than running side by side. It never refuses. The integrate answer says how long a
verify waited and whose it waited for.

Role runs are **not** paused while a verify runs, so a run's tests can still collide with a
verify. That is what `isolateEnv` is for. The setting is off by default because it makes a busy
board's reviews wait on each other.

## Reading a refused verify

A failed verify no longer tells the reader to hand the branch back. It says:

- which commit failed, with both roads: hand it back if the failure is in the change, or
  leave the task in review and retry if it came from the environment;
- **when verify ran, and whether it ran for this call** (`fresh`) or answered from a run that had
  already finished on the same commit (`reused`). The prewarm started when the task went to
  review is used once, and after that every integrate runs verify again, so a retry is always a
  real re-run;
- **the other runs that were alive while it ran**, as role, task and worktree, and a pointer to
  `cide_agents_config` with `isolateEnv` when the project doesn't isolate anything.

The full output is in the log the refusal names. For an isolated verify, the log's header lists
the directories it ran with (`# env: XDG_DATA_HOME=…`).
