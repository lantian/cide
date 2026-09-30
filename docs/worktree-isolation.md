# Shared global state across worktrees

For project authors whose roles run tests. This page covers what cide isolates between
concurrent runs, what it doesn't, the three settings in `.cide/config.json` that close the gap,
and the `CIDE_CPUS` variable a project's scripts should size their parallelism by.

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

### What a role may do past codex's sandbox (M119)

Three keys in a role's front matter, also on the Settings → Agents form and on
`cide_agent_create`/`cide_agent_update`:

```yaml
allow-commands: [blender -b, tools/ci/runners/e2e.sh]
needs: [display, audio, gpu]
writable-dirs: [~/.cache/godot]
```

- **`allow-commands`** — prefixes that run without a prompt. On codex, a command that consists of
  one of these alone runs **outside** the sandbox. It stays sandboxed under `timeout`, after
  `cd … &&`, in a pipe or inside `sh -c`, and the run's brief says so. cide writes the rules into
  the run's worktree only. A run in the project root does not get them.
- **`needs`** — `display`, `audio`, `network`, `gpu`. On codex any of them turns the sandbox's
  network on, because its filter refuses unix sockets while the network is off. `display` also
  points GL at Mesa. `gpu` (M125) binds `/dev/dri` and `/dev/nvidia*` into the sandbox, so
  Vulkan runs on the real GPU rather than llvmpipe. codex's own `/dev` has neither, and it has
  no setting for them. cide puts a `bwrap` shim first on codex's `PATH` that adds them to the
  sandbox it builds.
- **`writable-dirs`** — extra writable roots, absolute or `~/…`.

When a run ends, cide ends everything it started, including a codex sandbox's own sessions. When
a worktree is retired, anything still running in it that nobody owns is ended too.

## The `XDG_CONFIG_HOME` caveat

`XDG_CONFIG_HOME` is allowed but rarely what you want. Tools keep credentials and settings there
(`gh`, git credential helpers, cloud CLIs, editors), and any tool not in the shared list is
**logged out in every run**, usually silently: a push that asks for a password nobody will type,
or an API call that fails as unauthenticated. `gh` and `git` are shared by default. Share
anything else your runs need with `isolateEnvShare`, or leave `XDG_CONFIG_HOME` off the list.
`XDG_CACHE_HOME` has a milder cost: a cold cache per worktree, which is slower but correct.

## `CIDE_CPUS`: a run's share of the machine (M132)

cide exports `CIDE_CPUS` to every run in a worktree, to the verify of its branch, and to every
session opened in a checkout. It is **this machine's cores divided by `agents.maxConcurrent`, at
least 1** (`AgentsConfig::cpu_share`). Every run and its verify see the same number.

Use it in the project's scripts wherever they choose a degree of parallelism, instead of `nproc`
or a hard-coded count:

```sh
jobs=${CIDE_CPUS:-$(nproc)}
make -j"$jobs"
cargo test -- --test-threads="$jobs"
./tools/ci/run-tests.sh --shards "$jobs"
```

The `${CIDE_CPUS:-…}` fallback keeps the script working outside cide.

Why it exists: selfcraft's check script took half the machine each time it ran, so three
concurrent runs plus a verify asked for twice the cores there were. The project's global host
locks were papering over that, and they serialised exactly the runs cide was trying to run side
by side. With `CIDE_CPUS` each run
asks for its share, and the sum fits the machine.

## Running several game instances at once

A game's tests usually start the game, and two runs starting it at once meet on three things a
worktree does not isolate: the per-user data directory, the display, and the CPU. Close all three
and parallel instances need no locks.

- **Data: `isolateEnv`.** Godot's `user://` is `$XDG_DATA_HOME/godot/app_userdata/<project>`, so
  with `XDG_DATA_HOME` on the list each worktree's game writes its saves, logs and
  `.recovery_mode_lock` under its own directory. That was this page's original incident. Add
  `XDG_CACHE_HOME` for the shader cache and `TMPDIR` for everything else. Share installed export
  templates with `isolateEnvShare: ["godot/export_templates"]`.
- **Display: a private X server per instance, with `Xvfb -displayfd`.** Do not use a fixed
  `:99`, and do not use `xvfb-run -a`: both let two runs pick the same number, or race between
  "is it free?" and "take it". With `-displayfd` the server chooses a free display itself and
  writes its number to the descriptor once it is ready to accept clients:

  ```sh
  fifo=$(mktemp -u) && mkfifo "$fifo"
  Xvfb -displayfd 3 -screen 0 1280x720x24 -nolisten tcp 3>"$fifo" &
  xvfb=$!
  trap 'kill "$xvfb" 2>/dev/null' EXIT
  read -r display <"$fifo" && rm -f "$fifo"
  export DISPLAY=":$display"
  godot --path . res://tests/boot_smoke.tscn
  ```

  `read` returns only after the server is up, so there is no `sleep` to tune. On codex, a role that
  starts a display needs `needs: [display]` (see [M119](#what-a-role-may-do-past-codexs-sandbox-m119)).
- **CPU: `CIDE_CPUS`**, above.

**Then remove the global host locks**: a `flock /tmp/game.lock` around the test runner, a "one
Godot at a time" guard, a fixed-port mutex. They were the right fix while the instances really
shared state. Once the state is private they only serialise runs, and on a busy board a lock like
that turns `maxConcurrent: 3` into one game at a time with two runs waiting. Keep a lock only for
something that is truly one per machine (a device, a licence server), and reach for
`verifyExclusive` below before writing your own.

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
