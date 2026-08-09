# ADR 0005 — How cide hosts Claude Code: a real PTY, plus the IDE protocol, plus hooks

**Status:** accepted (M0–M7)
**Date:** 2026-08-07

## Context

cide's centre of gravity is a live Claude Code session. There were three ways to host one, and
the interesting answer is that none of them is sufficient alone.

**A — a plain terminal.** Spawn `claude` in a PTY and render it. Everything works exactly as
it does in any terminal, forever, because the CLI's terminal interface is its only stable
one. But Claude's diffs arrive as ASCII inside the terminal, selections and `@`-mentions have
no path into the conversation, and the status bar has no token or cost figures.

**B — the IDE integration protocol.** `claude --help` documents `--ide`. Reading the constants
out of the shipped binary gives the rest: a lock file at `~/.claude/ide/<port>.lock` carrying
`pid`, `workspaceFolders`, `ideName`, `transport`, `authToken`; the CLI connects to
`127.0.0.1:<port>` with an `X-Claude-Code-Ide-Authorization` header and speaks MCP; the IDE
serves `openDiff`, `close_tab`, `closeAllDiffTabs`, `getDiagnostics`, `openFile` and receives
`selection_changed`, `at_mentioned` and `ide_connected`. This is where every good feature
lives. It is also entirely undocumented, unversioned — there is no protocol-generation field
anywhere — and the CLI self-updates: 2.1.221 → 2.1.224 in four days on the reference machine.

**C — the SDK / `-p` one-shot.** No TUI, no conversation, structured output. Wrong for a live
session; right for generating a commit message.

## Decision

**All three, layered, with A load-bearing and B strictly additive.**

Every pane is a real PTY running the real `claude` (A). The IDE MCP server (B) runs per
project and is a *feature* of that pane, never a precondition for it: if the handshake fails,
the server does not start or the protocol changes shape, the pane keeps working and Claude
prints its diffs as text the way it does in any other terminal. The headless lane (C) is a
separate, non-interactive path in `cide-claude::headless` that allocates no PTY and joins no
session.

The load-bearing detail is `CLAUDE_CODE_SSE_PORT`, which every pane is spawned with. It does
two things at once: it satisfies the CLI's lock-file validity check — bypassing
cwd-containment, pid-liveness and PID-ancestry — and it force-enables auto-connect. Without
it, a `claude` started here falls back to disambiguating among every lock file in the shared
`~/.claude/ide` directory and can bind to another editor entirely. Per-pane attribution then
comes from joining `ide_connected{pid}` against each session's `child_pid`, which
`portable-pty` already gives us: one WebSocket connection ↔ one pid ↔ one pane.

Hooks are registered **inline via `--settings`**, never by editing `~/.claude/settings.json`.
That file is the user's; installing our hooks in it would outlive the app, apply to every
`claude` they run anywhere, and fight any other tool managing it. The statusline is
*chained*, not replaced — `cide-hook statusline` runs the user's own command first and prints
its stdout verbatim before forwarding the JSON to us — because the statusline is the only
supported source for live token and cost figures, and adopting cide must not cost someone the
status line they already had.

Claude's edits come back through three layers, fastest first: `openDiff` (routed to a native
`@codemirror/merge` pane *before* bytes hit disk), the `PostToolUse` hook (an immediate path
list), and the `notify` watcher (the ground-truth backstop for `sed -i`, `cargo fmt`,
`git checkout` and subagent writes).

Two prohibitions are absolute:

- **Never inject `ANTHROPIC_API_KEY` and never read `~/.claude/.credentials.json`.** The key
  outranks subscription OAuth in the CLI's credential precedence, so setting one would
  silently bill a Console organisation for a user on Claude Max. Children inherit their
  authentication by inheriting the environment, which is all they need. The same rule is why
  the headless lane never passes `--bare`: its own help text says *"Anthropic auth is strictly
  `ANTHROPIC_API_KEY` or `apiKeyHelper` via `--settings` (OAuth and keychain are never
  read)"*.
- **Never render anything from `~/.claude/projects/*.jsonl`.** It is documented as internal
  and version-unstable, and uses camelCase where the public wire format uses snake_case. A
  best-effort read for a `/resume` picker *preview*, inside a try/catch, is the limit.

## Consequences

- The whole reverse-engineered surface is a maintenance commitment, not a one-time cost. It is
  confined to `crates/cide-ide-mcp/` and `crates/cide-claude/`, `claude --version` is recorded
  at spawn, and integration tests launch the **real** binary — `cargo test -p cide-ide-mcp --
  --ignored` asserts a full `openDiff` round trip in all three outcomes, including
  accepted-with-edits, where `content[1].text` carries our modified buffer back.
- `openDiff` parks an entire agent turn on our RPC, so every way a human can vanish must
  resolve the future: pane closed, tab closed, window closed, project closed, app quitting,
  socket dropped, the same file already under diff from another pane, the user walking away.
  Miss one and a session hangs forever with no visible cause. `diff_broker` holds a oneshot
  per `request_id` with a registered cancel path, and a table-driven test enumerates the
  disappearance modes.
- Child environment is part of the contract: `TERM=xterm-256color` (plain `xterm` gives 8
  colours, ASCII box-drawing and no alt screen), `COLORTERM=truecolor`,
  `CLAUDE_CODE_SCROLL_SPEED=3`, and `TMUX`/`TMUX_PANE`/`COLUMNS`/`LINES`/`CI` scrubbed.
  Scrubbing `$TMUX` is not paranoia: its presence triggers an unconditional 256-colour clamp
  that visibly desaturates the `#d97757` accent the entire design is built on.
- Fork is possible because of A + hooks: `claude --resume <src> --fork-session --session-id
  <fresh>` was verified against 2.1.226 — the id we pass is honoured, the fork inherits the
  parent's history, and both remain independently resumable.
- A degraded mode is a real, tested state rather than an afterthought: no MCP server, no
  hooks, no statusline still yields working terminals with a live `claude` in them.
