# ADR 0013 — Docker is read through the Engine API, and compose is the exception

**Status:** accepted (M41)
**Date:** 2026-09-10

## Context

M41 makes a Docker daemon's containers and images a first-class surface in cide. Three questions
had to be answered before a line was written, and the first one decides the other two.

### Why this is not an extension

The request was for "a Docker extension", and cide has an extension system. It cannot host this,
and not by omission. ADR 0010 puts extension code in a Web Worker whose entire capability surface
is a fixed table in `ui/src/ext/protocol.ts`:

```ts
export const REQUIRES: Record<HostRequest['kind'], CapabilityName> = {
  readFile: 'fs:read', activeText: 'editor:read',
  reveal: 'editor:write', outline: 'editor:write', diagnostics: 'editor:write',
}
```

There is no request that runs a process and none that opens a socket. `Capability::ProcessSpawn`
exists and unlocks exactly one thing — the declarative `contributes.languageServers` list, a bare
binary name with frozen args, spawned by cide's LSP supervisor and speaking LSP over stdio. And
`tauri.conf.json`'s CSP deliberately omits `cide-ext:` from `connect-src`, so a worker cannot
`fetch` at all. An extension can parse a `docker-compose.yml` and draw a tree of what it says; it
cannot ask the daemon a single question.

Widening that surface was considered and rejected here rather than deferred. A `docker:read`
capability would be a new `HostRequest`, a new grant on the install sheet, and a permanent
commitment to a vocabulary — and it would exist to serve one built-in feature. ADR 0010's
consequence section already answers this case: *the answer to a genuinely missing panel kind is
to add it to `viewModel.ts`* — that is, to cide — *not to open a hole for arbitrary markup.*

## Decision

**Speak the Engine API over the socket. Do not shell out to `docker`.**

This is the opposite call from ADR 0012, where `cide-spec` runs `openspec --json` rather than
parsing its markdown, and the two cases look alike from a distance. The difference is what the CLI
*is*. `openspec --json` is an adapter its authors maintain: every command emits a document, and
reimplementing their format would have been a second parser that drifts. Docker's CLI is not that.
`docker ps --format json` flattens ports and mounts into display strings built for a terminal;
`docker logs` through a pipe loses the stdout/stderr split the daemon actually sends; and `docker
inspect` — the one faithful surface — is the raw API object with a CLI in front of it. Shelling
out would mean parsing a human interface to recover a machine one that was there all along.

The cost is a dependency on API compatibility, and it is paid by negotiating rather than assuming:
an unversioned `GET /version` first, then pin the lower of what the server offers and what cide was
built against. That first request is also the reachability check, so a daemon that is not up fails
with its endpoint named instead of failing inside whichever call the user happened to make first.

**`bollard`, held to the `lsp-types` rule.** A dependency of exactly one crate, never re-exported;
everything crossing `cide-docker`'s boundary is a `cide_ipc::docker` DTO. Hand-rolling the client
lost on the ground the `zip` entry already argues in the root manifest: chunked transfer-encoding,
the 8-byte stdcopy demux frame, connection hijacking for exec and `DOCKER_HOST` parsing are each a
small correctness trap against a stream cide does not control, and there are four of them before
anything useful happens. TLS is deliberately not compiled in, and an `https://` endpoint refuses by
name rather than connecting in the clear.

**Resolve the connection through Docker *contexts*, never a hardcoded socket path.** This is the
decision that would otherwise be discovered as a bug report. On the machine M41 was written on
there is no `/var/run/docker.sock` at all:

```
$ cat ~/.docker/config.json     → "currentContext": "colima"
$ docker context ls             → colima *, colima-ozon, default
```

The ladder is `CIDE_DOCKER_HOST` (alone, not first — a broken override refuses rather than falling
through), then `DOCKER_HOST`, then the current context's endpoint, then the well-known sockets with
the per-user ones ahead of `/var/run`. Contexts are enumerable, so the panel gets a connection
switcher for free.

**Compose is the exception, and it is a real one.** There is no Engine API for compose: `docker
compose up` is a CLI plugin and the daemon has never heard of a stack. What the API does give is
labels — `com.docker.compose.project`, `.service`, `.config_files`, `.project.working_dir` — so
*grouping* containers into stacks is pure API and only *acting* on one needs the plugin. That lane
is quarantined behind its own discovery ladder (M43), and when the plugin is missing the stacks
still list: only the buttons go quiet, with a sentence.

## Consequences

- **Every failure is a screen, never an `Err`.** `DockerBoard` has `Absent`, `Unusable` and `Ready`
  arms for `SpecBoard`'s reason: a panel handed an error has nothing to draw and no way to say what
  to do next, while a board that says the daemon is not running has a sentence and a Retry. The
  sentence is composed where the reason is known — `cide_docker::connect::Refusal` distinguishes a
  broken override, an unsupported scheme and nothing-found, and each names a different remedy.
- **An unsupported scheme refuses by name and never falls back.** `ssh://` is a real and common
  `DOCKER_HOST`, and cide does not tunnel. Silently connecting to a *local* daemon instead would
  show another machine's containers under that name and let somebody stop one; that is the single
  most dangerous thing this crate could do, so it is a refusal with the reason in it.
- **Docker's vocabulary travels as `String`.** Container states, health and image kinds are carried
  as written. Docker adds states, and a row that rendered blank because the daemon was newer than
  cide would be a bug with no symptom to search for. `bollard-stubs` models these as closed enums
  with no `#[serde(other)]` arm, which cide cannot fix — but the failure is then a named
  `Unreadable` on one call rather than a silently empty board.
- **No project anywhere in the feature.** A daemon belongs to the machine: one board, one
  coalescer, one `cide://docker-changed` with no id, and `sidebar.docker` is the only sidebar
  command in the registry with no `projectOpen` in its `when` clause.
- **`cide-docker` owns a private tokio runtime and is blocking on the outside**, so `cmd/docker.rs`
  calls it from `spawn_blocking` like every other domain crate and tokio does not become a fact the
  rest of the workspace has to know.
- The connection is held and the board is not. Caching a board would cache the thing that changes;
  reopening a connection per refresh would make a refresh a version negotiation. The handle is
  dropped on any transport failure — but **not** on a daemon *refusal*, or every "you cannot remove
  a running container" would become a reconnect.

## Addendum (M42–M45): what the API turned out not to have

Three gaps were found by building against it, and each shaped a decision rather than being worked
around quietly.

**There is no "stop an exec".** `POST /containers/{id}/exec` and `POST /exec/{id}/start` create
and attach; `inspect` and `resize` are the rest. Nothing ends one. So closing an exec pane drops
the input side and lets the daemon deliver EOF on stdin — which a shell obeys and a program that
ignores stdin does not. The alternative was a *second* exec running `kill`, which needs a shell
and a pid visible inside the container and therefore fails on exactly the distroless images where
it would matter most. The limitation is written into `cide_pty::Transport`'s implementation rather
than left to be discovered: a pane closed over `sleep 600` leaves it running.

**There is no directory listing.** `GET /containers/{id}/archive?path=X` returns a tar of the
whole subtree — asking for `/` tars the container — and `HEAD` describes one entry and nothing
about its children. So M44 splits the two halves: a directory is listed by running `ls -lAp`
through an exec, and a *file* is read from `/archive`, which needs no shell. The consequence is
stated rather than hidden: an image built `FROM scratch` cannot be browsed, and its files can
still be read once their paths are known. A listing that fails is a sentence naming the cause,
never an empty tree, because an empty tree is a lie about a filesystem that is full.

**Compose remains the exception the original decision named**, and building it confirmed the
shape. Grouping is pure API — the labels are there — and `-p <project>` on every CLI invocation is
the load-bearing argument: without it Compose derives the project name from the working directory,
so a stack brought up from a directory that has since been renamed is a *different* project and
`down` reports success having stopped nothing.

### And one thing the API has that cide deliberately does not use

`PUT /containers/{id}/archive` writes a file into a container. cide reads and never writes. An
edit written back is lost the moment the container is recreated — which for anything under compose
is the ordinary way it is restarted — and a silently-lost edit is a worse feature than no feature.


## Addendum (M48): the compose exception, from a file rather than from a label

The original decision described acting on a *stack* — something already running, whose project
name came off a container's `com.docker.compose.project` label. M48 added the other direction: a
compose **file** the user is looking at, in the tree, in the editor, or named by the palette.
Three things about the exception changed shape.

**`-p <project>` is load-bearing when there is a label and wrong when there is not.** The
paragraph above still stands for the panel's road and is unmodified. It does not transfer. A file
carries no label, and Compose's own derivation is better than anything cide could compute: it
reads a `name:` at the top of the file, then `COMPOSE_PROJECT_NAME` from the `.env` beside it, and
only then falls back to the working directory's basename. Passing a guessed `-p` overrides all
three at once and acts on a stack nobody has — the same failure as the renamed directory, reached
from the other side. So `argv`'s project became an `Option`, and the file road passes `None`
deliberately rather than by omission.

**Resolving the binary is not resolving the invocation, and conflating them is silent.** `argv`
omits the `compose` word because whether it is needed depends on which rung of `find_compose`'s
ladder answered — a standalone `docker-compose` takes the verbs directly and would read `compose`
as a file name. `act` resolved `find_docker()` instead and dropped the word on the floor, so every
stack button ran `docker -p … up` and was refused by the CLI with `unknown shorthand flag: 'p'`.
It shipped that way from M43 to M48 with every unit test green, because each asserts on the argv
and the argv was right; only running the binary can see it. `Invocation::program` is private and
`plan`/`act` are the only ways through it, which is the structural half of the fix.

**Output needed somewhere to go, and that unlocked the verbs the original list refused.** The
first cut of `ComposeAction` stopped at `up`/`down`/`restart` and said why: `build`, `pull` and
`run` are long jobs whose progress *is* the answer, and a button that silently spends four minutes
is worse than no button. A compose run from a file is an **ordinary shell pane** —
`SplitIntent::Compose` carries the argv and `session_spawn` does the rest — so there is a place to
watch one now, with no deadline and a Ctrl+C that reaches the child. `is_quick()` is where the two
roads divide: a surface that waits for an answer may only offer bounded verbs, and the panel's
button list is asserted equal to that set in both directions.

The argv is carried on the intent and **stored nowhere**. `workspace.json` outlives the run, and a
restored pane that re-spawned `docker compose up` at launch — against a file that may have
changed, on a machine the user has only just unlocked — is the one outcome here worth designing
against. It restores as the plain shell it is, in the directory the run used.
