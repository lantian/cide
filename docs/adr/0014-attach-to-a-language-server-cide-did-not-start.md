# ADR 0014 — A language server may be attached to over a loopback socket, never started that way

**Status:** accepted (M59)
**Date:** 2026-09-17

## Context

M59 adds Godot support as a marketplace extension. The syntax half is data (ADR 0010: three
`contributes.languages` entries and fifteen regex rules). The language-server half is not: Godot's
language server is the **running Godot editor**, listening on `127.0.0.1:6005`, and there is no
binary that speaks LSP on stdio. `cide-lsp` had exactly one transport — spawn a `Command`, own
its three pipes, arm it with `PR_SET_PDEATHSIG`, walk a signal ladder on the way out — and every
one of those steps is about a process cide owns.

Three ways to reach Godot were on the table.

1. **A stdio↔TCP bridge declared as the binary** — `socat - TCP:127.0.0.1:6005`. No core change,
   and wrong in every way that matters: when Godot is not open the bridge exits at once, which
   the supervisor reads as a *crash* and answers with the backoff ladder, the crash budget and a
   permanent *"exited 3 times in five minutes"* — the sentence a user would read every time they
   opened a Godot project before opening Godot. It also puts a third-party program between cide
   and every document the user has open, on the strength of a manifest.
2. **Spawn Godot headless** — `godot --headless --editor --lsp-port N --path P` — and connect
   to it. Real, and the VS Code plugin offers it as an option. But a headless *editor* imports
   the project (minutes on a large one, writing `.godot/`), competes with the GUI editor the user
   has open anyway, and turns "attach to the thing that is already running" into "own a second
   copy of it". Deferred: the same definition with `args` added is how it would be spelled, and
   the manifest refuses that combination today so the door is closed rather than ajar.
3. **Attach.** A `connect` endpoint on the server definition, a `Target::Tcp` beside
   `Target::Process`, and a supervisor arm that treats *nothing is listening* as *not running*.

## Decision

**cide attaches to a language server over a loopback socket when its definition says so, and
treats that server as somebody else's process in every rule that touches it.**

- `LanguageServerDef.connect: Option<TcpEndpoint { host, port }>`. `binary` stays required and is
  the server's *name* — it keys the registry's merge, the Problems panel's source id and every
  log line — and for a spawned server it is also the program. Nothing is looked up for a connect
  server. `connect` with `args` is refused.
- **Loopback only, as a literal address.** `TcpEndpoint::loopback_addr` is the one producer of
  the rule: the host must parse as an `IpAddr` and answer `is_loopback()`; a name, `localhost`
  included, is refused, because connecting takes a `SocketAddr` and resolving a name is an
  `/etc/hosts` question a manifest must not get to ask. The rule is asked twice — in
  `cide_ext::manifest` and again in `cide_lsp::discover::locate` — because a builtin definition
  never passes the manifest, and the two crates cannot share a verdict. This is the analogue of
  `binary`'s *never an absolute path*: `didOpen` carries the whole document, so a definition that
  could name a host could send the user's files to it.
- **A connect server must declare `projectMarkers`.** Empty means every root, and with the retry
  below that is a `connect()` against the port every few seconds in every project the user ever
  opens. `sync_servers` gates on `applicable`, so the marker is the only thing that scopes it.
- **A new capability, `lsp:connect`** — *connect to a language server already running on your
  machine* — instead of `process:spawn`. The install sheet is the one place the words matter,
  and *run programs on your machine* is the wrong sentence for an extension that runs none.
  Declared before `ProcessSpawn` so the sorted list keeps the widest blast radius last.
- **Never `exit` a server cide did not start.** The socket ladder is `Session::detach` — the
  `shutdown` request alone — then any reply to that id or EOF, then `shutdown(Both)` and a join.
  No `try_wait`, no `SIGTERM`, no `kill`. `processId` in `initialize` is `null`: cide is not the
  parent, and a server that watched cide's pid would exit for the wrong process.
- **Not listening is not broken, and a dropped socket is not a crash.** A guarded arm ahead of
  the two existing failure arms in `supervise_lives`: emit `Unavailable` (which the panel dims)
  once per outage, wait `ATTACH_RETRY`, try again. No ladder to advance, no budget to spend. A
  socket that closes after the handshake is the user restarting their editor and takes the same
  road.
- **Open documents are replayed on every new life**, for every server. `LspEvent::Handshook`
  fires once per life, and the app replays a `didOpen` for each cached document the server owns.
  This closes a gap every server had — a crash-restart, the watchdog and the Restart button all
  started a life that knew no documents — which rust-analyzer and gopls hid by reading the disk
  and Godot, publishing only on open/change/save, would not have.

## Consequences

- `codec.rs` is untouched: it was generic over `BufRead`/`Write` from the start, and a
  `BufReader<TcpStream>` and a boxed `TcpStream` are what the pump holds now, without knowing.
- The crate has its first non-`#[ignore]`d end-to-end tests. A socket needs no binary on PATH,
  so a fake server on `127.0.0.1:0` drives the whole supervisor: `Ready`, the drop path's
  `shutdown`-and-never-`exit`, the retry finding a listener that appeared later, the reconnect
  after a closed socket, and the two sliced-wait bugs for the new arm.
- `Provenance::Attached` is excluded from both `config.rs` gates by construction and pinned so:
  an attached server gets no fork options and no environment.
- The memory watchdog never fires for an attached server (`pid()` is `None`); the Builtin/System
  setting has no row for one. Both are right: the process is the user's editor.
- Not done: spawn-and-connect (option 2), passing the marker's directory rather than the project
  root as `rootUri` for a nested `game/project.godot`, and any transport other than TCP on
  loopback. Each is a later decision, and the manifest rules above are what keep a manifest
  from pre-empting it.
