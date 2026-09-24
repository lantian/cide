# cide

**The IDE built around Claude Code.** A live Claude Code session sits at the centre of the window, and a real IDE surrounds it.

[![cide — the IDE built around Claude Code](docs/img/hero.png)](https://lantian.github.io/cide/)

**→ [See every feature at lantian.github.io/cide](https://lantian.github.io/cide/)** (English and Russian, in dark and light)

## What it is

- **A Claude tab that never closes.** Every project opens on a pinned Claude tab: a tiling grid of Claude sessions, shells and diffs. Sessions belong to the Rust core, not to a window, so they survive closed panes, detached windows and restarts.
- **A real Claude Code IDE.** cide serves Claude Code's IDE-integration MCP server:
  - the agent's edits arrive as diffs in cide's editor;
  - `getDiagnostics` answers from cide's own language servers;
  - `Ctrl+G` opens the plan in a tab.
- **A team of agents.** A task tracker in `.cide/` that Claude calls over MCP, plus roles that each run in their own git worktree. Milestones gate merges on your verify command. Roles can run on Claude Code, opencode, Codex or MiMo.
- **An IDE that works.**
  - CodeMirror 6 with real language servers (a packaged build carries its own rust-analyzer and gopls).
  - IDEA-style git: changelists, a shelf, hunk staging, a log with graph lanes, and a three-pane merge.
  - GitLab MR review, OpenSpec and Docker.
- **Yours to shape.** Dark and light themes, VS Code colour themes imported, a keymap editor over one command registry, and extensions from a git repository.

Rust + Tauri 2 backend, React 19 frontend. **Linux-first**, developed on KDE/Wayland.

## Install

You need **Rust 1.92**, **Node + pnpm**, **WebKitGTK 4.1 / GTK 3**, and the
[`claude` CLI](https://docs.claude.com/en/docs/claude-code), installed and signed in. cide hosts the CLI; it does not ship it.

```sh
git clone https://github.com/lantian/cide && cd cide
pnpm --dir ui install
./run.sh
```

- **Language servers:** a source build uses `rust-analyzer` and `gopls` from your `PATH`.
- **AppImage:** `./build.sh` builds one that carries its own language servers. See [CONTRIBUTING.md](CONTRIBUTING.md) and [docs/forks.md](docs/forks.md).
- **macOS:** [docs/platforms.md](docs/platforms.md) covers it.
- **Credentials:** cide never reads `~/.claude/.credentials.json` and never injects `ANTHROPIC_API_KEY`. Children inherit their auth from the environment.

## Status

**Early, and honest about it.** cide is used every day to build itself. Linux is the only platform it has run on; the macOS build compiles but no one has run it. [docs/journal.md](docs/journal.md) records, milestone by milestone, what has never been confirmed on a display.

## Documentation

| | |
| --- | --- |
| [CONTRIBUTING.md](CONTRIBUTING.md) | build, run, check, package — everything CI runs |
| [docs/architecture.md](docs/architecture.md) | crates, the state loop, the wire contract, pane hosts, Claude hosting |
| [docs/adr/](docs/adr/) | the decisions a refactor would otherwise undo |
| [docs/platforms.md](docs/platforms.md) | what is known, and what is only read, off Linux |
| [docs/journal.md](docs/journal.md) | the milestone record, including what was never verified |
| [docs/forks.md](docs/forks.md) | the rust-analyzer, salsa and gopls forks |

## Licence

MIT — see [LICENSE](LICENSE).
