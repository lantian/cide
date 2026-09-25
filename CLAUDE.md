# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

cide is an IDE whose centre of gravity is a live Claude Code session rather than a text buffer:
a pinned, non-closable Claude tab per project hosting a tiling grid of panes. Rust + Tauri 2
backend, React 19 + Vite 8 frontend, Linux-first (developed on KDE/Wayland; macOS builds, see
`docs/platforms.md`).

## Where things are written down

| read | when |
| --- | --- |
| [`docs/running.md`](docs/running.md) | launching, profiles, the sibling forks, `openspec`, the one-instance guard, `run.sh` flags |
| [`docs/checks.md`](docs/checks.md) | what to run after touching a surface, and the silent failures each surface has paid for |
| [`docs/architecture.md`](docs/architecture.md) | crate graph, state loop, wire contract, command registry, pane hosts, PTY/Claude hosting, on-disk files |
| [`docs/adr/`](docs/adr/) | decisions a refactor would otherwise undo — 0001–0003 before anything structural, 0009 before conflicts, 0010 before `ui/src/ext/` |
| [`docs/journal.md`](docs/journal.md) | the honest milestone record, including what was never confirmed on a display. **Read before assuming a feature works end to end; append to it (not the README) when finishing a milestone.** Take the next `Mnn` from its own headings |
| [`docs/platforms.md`](docs/platforms.md) | what is known off Linux, and type-checking macOS arms from Linux |
| [`docs/forks.md`](docs/forks.md) | the rust-analyzer, salsa and gopls forks (`../forks/*`); each fork has its own `CLAUDE.md` — read it before touching that repo |
| [`docs/worktree-isolation.md`](docs/worktree-isolation.md) | for project authors: what concurrent runs share, `agents.isolateEnv`, `verifyExclusive`, reading a refused verify |
| [`docs/packaging.md`](docs/packaging.md) | `cargo xtask package`, `./build.sh`, releases |
| [`docs/ui-kit.md`](docs/ui-kit.md) | **before drawing any UI**: the kit's components, tokens and state rules, and how to add a component; the live page is <http://localhost:1420/kit.html> |
| `CONTRIBUTING.md` / `README.md` | prerequisites and build / the landing page |

## Running it

```sh
pnpm --dir ui install     # once
./run.sh                  # the `dev` profile; --profile <name>, --fresh, --release, --inspect, audits…
```

- **Use `./run.sh`, never `./target/debug/cide`** — a debug build loads the UI from Vite on
  `localhost:1420` and shows a white window without it.
- **`./run.sh` runs the `dev` profile**, isolated from the real instance (`cide_core::profile`,
  set by `CIDE_PROFILE`). A new profile is factory-fresh: no extensions, no global roles.
- **One cide per profile, and `cide --help` is not a launch.** Never `pkill -x cide` or
  `pkill -f mount_cide` — they kill the live IDE and every `claude` in it. Stop a profile by the
  pid in its `instance.lock`.
- Per-machine overrides (`CIDE_RA_PATH`, `CIDE_GOPLS_PATH`, `CIDE_OPENSPEC_PATH`, …) go in a
  gitignored `.env`, which `run.sh` sources.
- `run.sh` must stay bash-3.2-clean (macOS): `./scripts/check-bash32.sh`.
- Inspect state without a GUI: `./target/debug/cide-headless tree|tasks|agents|spec|docker|remote|properties|ext|…`.

## Checking it

Everything CI runs, in CI's order:

```sh
cargo fmt --all --check
cargo --locked xtask contract-check
cargo build --locked --workspace
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo --locked xtask codegen --check
pnpm --dir ui exec tsc --noEmit
pnpm --dir ui run check:<name>          # every check:* script in ui/package.json
pnpm --dir ui build
```

- One Rust test: `cargo test -p cide-git branches`. Keep `--locked`; global flags lead the
  `xtask` alias: `cargo --locked xtask codegen`.
- **`#[ignore]`d tests are deliberate** — most spawn the real `claude` and spend the user's
  quota. Run them only on purpose. Some are free and worth running on every change to their
  surface; `docs/checks.md` says which.
- **The frontend has no test runner**: `ui/scripts/check-*.mjs` are the suite. Adding a
  `check:foo` script to `ui/package.json` is enough for CI. **A check that greps source must strip
  comments first** — house-style comments name the failure, so the needle is in the prose.
  Keep modules a check compiles standalone import-free.
- The audits (`./run.sh --audit-chrome|--audit-panes|--audit-windows`) need a display and are not
  in CI; they are the only check for pane-host and chrome regressions.

### Which check covers what you touched

Run these after touching the surface; follow the link for what breaks **silently** there
before you change it.

| you changed | run (details: [`docs/checks.md`](docs/checks.md)) |
| --- | --- |
| [`keys/`, a default binding, `cide-core::commands`](docs/checks.md#keys-a-default-binding-cide-corecommands) | `check:keys`, `check:commands`, `check:switcher`, `cargo test -p cide-core commands` |
| [A non-Latin keyboard layout](docs/checks.md#a-non-latin-keyboard-layout) | `check:latin`, `check:keys`, `check:terminal-keys` |
| [`sidebar/GitPanel/`, `cide-git`](docs/checks.md#sidebargitpanel-cide-git) | `check:git`, `check:render`, `check:diff`, `check:diff-render`, `check:branches` |
| [The diff surface](docs/checks.md#the-diff-surface) | `check:diff-render`, `check:scheme`, `check:markdown`, `check:merge` |
| [Pull, merge, rebase](docs/checks.md#pull-merge-rebase) | `check:pull-strategy`, `check:branches`, `cargo test -p cide-git --test pull --test conflicts --test merge` |
| [Push](docs/checks.md#push) | `check:push`, `check:branches`, `cargo test -p cide-git --test staging -- preview lease blocked diverged publish` |
| [The conflict resolver](docs/checks.md#the-conflict-resolver) | `check:merge`, `check:render`, `check:theme`, `check:ui-scale` |
| [`toolwindow/`, the activity rail, the bottom panel](docs/checks.md#toolwindow-the-activity-rail-the-bottom-panel) | `check:toolwindow`, `check:toolwindow-render`, `check:sidebar`, `check:menus` |
| [The shell with no project open](docs/checks.md#the-shell-with-no-project-open) | `check:toolwindow-render`, `check:boundary`, `check:sidebar`, `check:commands`, `cargo test -p cide-core -- toolwindow persist` |
| [`gitlog/`, `cide-git`'s `log`/`lanes`/`show`/`revision`](docs/checks.md#gitlog-cide-gits-loglanesshowrevision) | `check:log`, `check:log-render`, `check:diff-render` |
| [Blame](docs/checks.md#blame) | `check:blame`, `check:editor` |
| [The change column](docs/checks.md#the-change-column) | `check:change-bars`, `check:editor`, `check:blame` |
| [The task tracker, `.cide/`, agent definitions](docs/checks.md#the-task-tracker-cide-agent-definitions) | `check:agents`, `check:agents-render`, `check:sidebar`, `check:commands` |
| [A role's colour, or who a comment is signed by](docs/checks.md#a-roles-colour-or-who-a-comment-is-signed-by) | `check:theme`, `check:agents`, `check:agents-render`, `check:ui-scale`, `cargo test -p cide-ipc agents`, `cargo test -p cide-agents`, `check:settings-agents` |
| [The board's payload, or anything that opens a task's card](docs/checks.md#the-boards-payload-or-anything-that-opens-a-tasks-card) | `check:agents`, `check:agents-render`, `check:selectors`, `check:theme`, `cargo test -p cide-tasks`, `cargo xtask codegen --check` |
| [The tracker's storage](docs/checks.md#the-trackers-storage) | `cargo test -p cide-tasks`, `cargo test -p cide-ipc tasks`, `cargo test -p cide-hook`, `cargo test -p cide-app dotcide`, `CIDE_TRACKER=<project> cargo test -p cide-tasks a_real_tracker -- --ignored` |
| [Opening a run](docs/checks.md#opening-a-run) | `cargo test -p cide-app -- agents lifecycle cmd::pane cmd::session`, `cargo test -p cide-agents`, `cargo test -p cide-pty`, `check:agents`, `check:agents-render`, `check:attach`, `check:rows`, `check:detached`, `check:json-log`, `cargo test -p cide-app event_tap`, `check:settings-agents`, `cargo test -p cide-agents --test real_codex -- --ignored --skip a_real_turn`, `cargo test -p cide-agents --test real_served -- --ignored` |
| [What a resumed run's pane opens onto](docs/checks.md#what-a-resumed-runs-pane-opens-onto) | `cargo test -p cide-app -- agents`, `cargo test -p cide-pty`, `check:json-log`, `check:attach` |
| [Stopping a run](docs/checks.md#stopping-a-run) | `cargo test -p cide-app -- agents agent_rpc`, `cargo test -p cide-agents`, `check:agents` |
| [LLM providers, model pools and local overrides](docs/checks.md#llm-providers-model-pools-and-local-overrides) | `cargo test -p cide-ipc llm`, `cargo test -p cide-agents`, `cargo test -p cide-app -- agents settings`, `check:pools`, `check:settings-agents`, `check:selectors`, `check:ui-icons`, `cargo test -p cide-agents --test real_opencode -- --ignored`, `cargo test -p cide-agents --test real_mimo -- --ignored --skip a_real_turn` |
| [The settings MCP tools](docs/checks.md#the-settings-mcp-tools) | `cargo test -p cide-agents tools`, `cargo test -p cide-app agent_rpc`, `cargo test -p cide-ipc llm`, `check:pools` |
| [Task attachments](docs/checks.md#task-attachments) | `cargo test -p cide-tasks`, `cargo test -p cide-agents`, `cargo test -p cide-app agent_rpc`, `check:agents`, `check:agents-render`, `check:selectors`, `check:ui-scale`, `check:motion`, `check:ui-icons` |
| [The role format, or anything that reads `.claude/agents/`](docs/checks.md#the-role-format-or-anything-that-reads-claudeagents) | `cargo test -p cide-agents`, `check:settings-agents`, `check:agents`, `check:agents-render`, `cargo test -p cide-fs` |
| [Docker](docs/checks.md#docker) | `check:docker`, `check:docker-render`, `check:theme`, `check:boundary`, `check:sidebar`, `check:commands`, `check:ui-icons`, `cargo test -p cide-docker`, `./target/debug/cide-headless docker` |
| [An exec or log pane](docs/checks.md#an-exec-or-log-pane) | `cargo test -p cide-pty`, `cargo test -p cide-docker`, `check:attach`, `check:awaiting`, `check:render-stall`, `check:exit`, `cargo test -p cide-docker -- --ignored` |
| [Compose, inspect, or a container's files](docs/checks.md#compose-inspect-or-a-containers-files) | `check:docker`, `check:docker-render`, `check:tab-overflow`, `check:tab-drag`, `check:commands`, `check:ui-icons`, `check:motion`, `cargo test -p cide-docker`, `cargo test -p cide-docker -- --ignored` |
| [The Dockerfile language, or `LanguageServerDef::init_options`](docs/checks.md#the-dockerfile-language-or-languageserverdefinit_options) | `cargo --locked xtask codegen --check`, `check:editor`, `cargo test -p cide-lsp` |
| [OpenSpec](docs/checks.md#openspec) | `check:openspec`, `check:openspec-render`, `check:openspec-config`, `check:agents-render`, `cargo test -p cide-spec`, `cargo test -p cide-spec -- --ignored`, `./target/debug/cide-headless spec <root>` |
| [Which pane a connected `claude` belongs to](docs/checks.md#which-pane-a-connected-claude-belongs-to) | `cargo test -p cide-core proc`, `cargo test -p cide-ide-mcp`, `cargo test -p cide-app ide` |
| [The commit actions](docs/checks.md#the-commit-actions) | `check:log-actions`, `cargo test -p cide-git --test commit_actions` |
| [The file tree, fs ops](docs/checks.md#the-file-tree-fs-ops) | `check:tree-status`, `check:tree-flicker`, `check:fs-clipboard`, `check:new-entry`, `check:scratch`, `check:excalidraw` |
| [The properties card](docs/checks.md#the-properties-card) | `check:properties`, `check:commands`, `check:menu-model`, `check:menus`, `check:theme`, `check:selectors`, `check:ui-scale`, `cargo test -p cide-core properties`, `./target/debug/cide-headless properties <path>` |
| [Pasting an image](docs/checks.md#pasting-an-image) | `check:paste-image`, `check:fs-clipboard`, `check:editor`, `cargo test -p cide-core image`, `cargo test -p cide-fs` |
| [The drawing pane](docs/checks.md#the-drawing-pane) | `check:excalidraw`, `check:commands`, `check:editor`, `check:image`, `cargo test -p cide-core document`, `cargo test -p cide-ipc frame`, `cargo test -p cide-app -- framed_write json_body truncated_frame` |
| [Which shell a pane runs, or what `PATH` a child gets](docs/checks.md#which-shell-a-pane-runs-or-what-path-a-child-gets) | `cargo test -p cide-core` |
| [The command line, or the one-instance guard](docs/checks.md#the-command-line-or-the-one-instance-guard) | `cargo test -p cide-app -- cli:: instance::` |
| [A synthetic tree row](docs/checks.md#a-synthetic-tree-row) | `check:groups`, `check:notes`, `check:scratch`, `check:tree-drag` |
| [`layout/`](docs/checks.md#layout) | `check:rows` |
| [Moving a pane](docs/checks.md#moving-a-pane) | `check:pane-move`, `check:rows`, `check:detached`, `cargo test -p cide-core layout` |
| [A splitter's drag path, `layout/resizeGesture.ts`, or any `ResizeObserver`](docs/checks.md#a-splitters-drag-path-layoutresizegesturets-or-any-resizeobserver) | `check:resize` |
| [`editor/`](docs/checks.md#editor) | `check:editor` |
| [What Tab inserts](docs/checks.md#what-tab-inserts) | `check:editor`, `check:selectors`, `check:completion`, `cargo test -p cide-ipc settings` |
| [`editor/markdown/`, the markdown preview](docs/checks.md#editormarkdown-the-markdown-preview) | `check:markdown`, `check:editor`, `check:ui-scale` |
| [Anything a CodeMirror surface draws](docs/checks.md#anything-a-codemirror-surface-draws) | `check:scheme` |
| [A syntax colour, a `--tk-*`, or `TOKEN_ROLES`](docs/checks.md#a-syntax-colour-a---tk--or-token_roles) | `check:scheme`, `check:editor` |
| [`ColorScheme::normalise`, and anything about an imported scheme's selection](docs/checks.md#colorschemenormalise-and-anything-about-an-imported-schemes-selection) | `cargo test -p cide-ipc theme` |
| [Showing a rejected `invoke` to a user](docs/checks.md#showing-a-rejected-invoke-to-a-user) | `check:scheme` |
| [Subscribing to a `cide://` event](docs/checks.md#subscribing-to-a-cide-event) | `check:unlisten`, `check:attach`, `check:detached` |
| [`cide_core::scheme`'s `SURFACE_KEYS`](docs/checks.md#cide_coreschemes-surface_keys) | `cargo test -p cide-core scheme` |
| [`cide_core::scheme`'s `score`/`NO_WEAK_EVIDENCE`, or `theme::fallback`](docs/checks.md#cide_coreschemes-scoreno_weak_evidence-or-themefallback) | `cargo test -p cide-core scheme` |
| [The VS Code theme importer](docs/checks.md#the-vs-code-theme-importer) | `cargo test -p cide-core scheme` |
| [Menus, header, chrome, settings](docs/checks.md#menus-header-chrome-settings) | `check:menus`, `check:menu-model`, `check:tab-overflow`, `check:sidebar`, `check:theme`, `check:fonts`, `check:proxy` |
| [The header project tab's two chips](docs/checks.md#the-header-project-tabs-two-chips) | `check:running`, `check:awaiting`, `check:theme`, `check:ui-scale`, `check:motion`, `cargo test -p cide-app -- running spinner` |
| [Any `font-size`, or a box drawn around text](docs/checks.md#any-font-size-or-a-box-drawn-around-text) | `check:ui-scale` |
| [A new sidebar panel, or anything `App.tsx` renders as one](docs/checks.md#a-new-sidebar-panel-or-anything-apptsx-renders-as-one) | `check:boundary` |
| [Any `useWorkspace`/`useStore` selector](docs/checks.md#any-useworkspaceusestore-selector) | `check:selectors` |
| [Closing or reopening a project](docs/checks.md#closing-or-reopening-a-project) | `cargo test -p cide-core -- reopen closed`, `cargo test -p cide-app -- lifecycle sessions_of`, `check:selectors`, `check:attach` |
| [Terminal input, session state](docs/checks.md#terminal-input-session-state) | `check:input`, `check:exit`, `check:awaiting`, `check:format` |
| [Rewriting a session's output](docs/checks.md#rewriting-a-sessions-output) | `check:json-log`, `cargo test -p cide-pty`, `cargo test -p cide-core jsonlog`, `cargo test -p cide-app a_shell_renders` |
| [The awaiting signal for a *shell* pane](docs/checks.md#the-awaiting-signal-for-a-shell-pane) | `check:awaiting`, `cargo test -p cide-pty jobs`, `cargo test -p cide-pty -- a_real_shell` |
| [The session sink](docs/checks.md#the-session-sink) | `check:attach`, `check:awaiting`, `check:render-stall` |
| [A pane host's lifecycle, or anything that decides when a terminal paints](docs/checks.md#a-pane-hosts-lifecycle-or-anything-that-decides-when-a-terminal-paints) | `check:render-stall` |
| [Overlays, pickers, search](docs/checks.md#overlays-pickers-search) | `check:picker`, `check:search`, `check:problems` |
| [Terminal file links, `cmd/file.rs`'s refusals](docs/checks.md#terminal-file-links-cmdfilerss-refusals) | `check:paths`, `check:outside-open` |
| [`windows/`, detach and re-dock](docs/checks.md#windows-detach-and-re-dock) | `check:detached`, `check:window-controls` |
| [Which projects or tabs a window draws](docs/checks.md#which-projects-or-tabs-a-window-draws) | `check:detached`, `check:commands`, `check:switcher`, `cargo test -p cide-core workspace`, `cargo test -p cide-app` |
| [The macOS dock menu](docs/checks.md#the-macos-dock-menu) | `cargo test -p cide-app dock` |
| [`cide-lang`, `cide-lsp`, symbol navigation, diagnostics](docs/checks.md#cide-lang-cide-lsp-symbol-navigation-diagnostics) | `check:outline`, `check:problems`, `check:commands`, `check:keys` |
| [Code completion](docs/checks.md#code-completion) | `check:completion`, `check:editor`, `cargo test -p cide-lsp -- --ignored offers_an_import_edit` |
| [The bundled servers](docs/checks.md#the-bundled-servers) | `cargo test -p cide-lsp`, `cargo test -p cide-core`, `cargo test -p xtask`, `cargo test -p cide-lsp -- --ignored a_broken_first_candidate`, `cargo test -p cide-lsp --test real_npm_server -- --ignored` |
| [An attached language server](docs/checks.md#an-attached-language-server) | `cargo test -p cide-lsp`, `cargo test -p cide-lsp --test attach`, `cargo test -p cide-ext`, `cargo test -p cide-ipc lang`, `cargo test -p cide-app -- lsp`, `check:ext`, `check:problems` |
| [Quick documentation](docs/checks.md#quick-documentation) | `cargo test -p cide-lsp`, `cargo test -p cide-app -- lsp cmd::project`, `check:commands`, `check:keys`, `check:editor`, `check:tab-overflow`, `check:tab-drag`, `check:ui-scale` |
| [`ext/`, `sidebar/ExtensionsPanel/`, `cide-ext`, a manifest field](docs/checks.md#ext-sidebarextensionspanel-cide-ext-a-manifest-field) | `check:ext`, `check:ext-render`, `check:boundary`, `check:sidebar`, `cargo test -p cide-ext`, `cargo test -p cide-ext --test marketplace` |
| [A `SettingsSection` variant](docs/checks.md#a-settingssection-variant) | `check:ext` |
| [A `TabKind` variant](docs/checks.md#a-tabkind-variant) | `check:tab-overflow`, `check:tab-drag`, `cargo test -p cide-app` |
| [A language table, a fold spec, a scratch row, `cide_ipc::lang::builtins`](docs/checks.md#a-language-table-a-fold-spec-a-scratch-row-cide_ipclangbuiltins) | `cargo --locked xtask codegen`, `check:editor` |
| [An icon, anywhere](docs/checks.md#an-icon-anywhere) | `check:ui-icons` |
| [A `transition`, or a `@media (prefers-reduced-motion)` block](docs/checks.md#a-transition-or-a-media-prefers-reduced-motion-block) | `check:motion` |
| [A radius, a spacing step, an elevation or a duration](docs/checks.md#a-radius-a-spacing-step-an-elevation-or-a-duration) | `check:menus` |
| [The remote surface](docs/checks.md#the-remote-surface) | `cargo test -p cide-remote`, `cargo test -p cide-pty`, `cargo test -p cide-ipc -- remote screen`, `cargo test -p cide-app -- remote emit windows`, `cargo test -p cide-core -- remote workspace`, `cargo test -p xtask protocol`, `pnpm --dir ui run check:remote`, `cargo --locked xtask codegen --check`, `./target/debug/cide-headless remote`, `cargo test -p cide-remote --test vectors -- --ignored`, `npm run check`, `check:remote`, `cargo test -p cide-core remote`, `cargo test -p cide-pty a_real_child_fills_the_ring -- --ignored` |
| [A Claude tab cide opened by itself](docs/checks.md#a-claude-tab-cide-opened-by-itself) | `cargo test -p cide-app -- spinner agent_rpc`, `cargo test -p cide-agents config`, `cargo test -p cide-core persist`, `check:settings-agents`, `check:agents`, `cargo --locked xtask codegen --check` |
| [The New project wizard](docs/checks.md#the-new-project-wizard) | `check:new-project`, `check:menu-model`, `check:commands`, `check:ui-scale`, `check:theme`, `check:motion`, `check:ui-icons`, `cargo test -p cide-app -- new_project cmd::project`, `cargo test -p cide-core commands`, `cargo --locked xtask codegen --check` |
| [The inbox, milestones and their gates](docs/checks.md#the-inbox-milestones-and-their-gates) | `cargo test -p cide-tasks`, `cargo test -p cide-agents -- milestones tools autodispatch config`, `cargo test -p cide-core check`, `cargo test -p cide-app -- spinner agent_rpc session`, `check:agents`, `check:agents-render`, `cargo --locked xtask codegen --check`, `npm run check` |
| [Isolated directories, exclusive verify, a refused verify](docs/checks.md#isolated-directories-exclusive-verify-a-refused-verify) | `cargo test -p cide-core -- isolated_env check`, `cargo test -p cide-agents -- harness::tests::the_isolated milestones config tools::tests::the_config`, `cargo test -p cide-app -- milestones agent_rpc`, `cargo test -p cide-git worktree`, `cargo --locked xtask codegen --check`, `check:settings-agents` |
| [What a local override actually changes, or when the workspace reaches the disk](docs/checks.md#what-a-local-override-actually-changes-or-when-the-workspace-reaches-the-disk) | `cargo test -p cide-agents`, `cargo test -p cide-app -- agents workspace_state`, `check:agents`, `check:settings-agents` |
| [An agent reviewing a GitLab MR, and its draft comments](docs/checks.md#an-agent-reviewing-a-gitlab-mr-and-its-draft-comments) | `cargo test -p cide-gitlab`, `cargo test -p cide-agents review`, `cargo test -p cide-app -- agent_rpc mr_review`, `check:gitlab`, `check:gitlab-render`, `check:gitlab-dom`, `cargo --locked xtask codegen --check` |
| [The console harness, or anything that spawns codex](docs/checks.md#the-console-harness-or-anything-that-spawns-codex) | `cargo test -p cide-core -- codex_cli workspace`, `cargo test -p cide-claude`, `cargo test -p cide-agents`, `cargo test -p cide-app -- cmd::session agents spec`, `check:awaiting`, `check:claude-cli`, `check:menu-model`, `check:ext`, `cargo --locked xtask codegen --check`, `cargo test -p cide-agents --test real_codex -- --ignored --skip a_real_turn` |
| [The screenshot demo and the feature site](docs/checks.md#the-screenshot-demo-and-the-feature-site) | `check:demo`, `pnpm --dir ui demo:shots` |
| [Added, renamed or moved any file](docs/checks.md#added-renamed-or-moved-any-file) | `check:casing` |
| [The UI kit, or any new UI](docs/checks.md#the-ui-kit-or-any-new-ui) | `check:kit`, `check:ui-scale`, `check:motion`, `check:theme`, `check:ui-icons`, `check:menus` |

## Architecture invariants

Full account in [`docs/architecture.md`](docs/architecture.md). The rules that break silently:

- **Only `cide-app` may depend on tauri.** `cide-headless` is the proof, and CI fails any other
  member whose `cargo tree` reaches `tauri|wry|tao`. Needing an `AppHandle` in domain logic means
  the logic is in the wrong crate.
- **Rust owns all durable state** (ADR 0002). A gesture is `ui/src/ipc/client.ts` →
  `#[tauri::command]` in `cide-app/src/cmd/*` → a validated mutation in `cide-core` (bumps `rev`)
  → `emit::workspace_changed` to every window. `ui/src/store/workspace.ts` is a mirror and never
  edits the tree.
- **Sessions live outside the tree**, keyed by `SessionId` (which *is* `claude --session-id`).
  Panes attach; closing a pane, tab or window never touches the child.
- **The wire contract has three gates**: `ui/src/ipc/generated.ts` and
  `ui/src/editor/builtinLanguages.ts` are generated by `cargo xtask codegen` — never hand-edit;
  a new command/event drifts `contract/*.json` (accept with `cargo xtask contract-check --write`);
  `client.ts` is the only seam to `invoke`/`listen`; every event leaves through `emit.rs`.
- **Commands and keys are one registry** (`cide-core::commands`). Ids are API — add, never
  rename. Every id is handled in `ui/src/keys/dispatch.ts` or carries an `unavailable` reason;
  a `when` flag must be in `CONTEXT_FLAGS` and actually supplied.
- **Pane hosts** (`ui/src/layout/paneHosts.ts`): hidden tabs are `visibility: hidden`, never
  `display: none`; never conditionally render a pane or `replaceChildren` a slot; `term.open()`
  once per pane.
- **PTY coalescing is correctness**, not tuning — small `Channel` payloads go through
  `webview.eval` on the GTK main loop.
- **Claude hosting** (ADR 0005): every early return in `ide.rs::pump` cancels `openDiff` first;
  never read `~/.claude/.credentials.json` or inject `ANTHROPIC_API_KEY`; every new spawn goes
  through `cide-core::child_env` (`prepare_command` *and* `arm`).
- Under a profile every on-disk path is `cide-<profile>`, decided only in
  `cide_core::persist::xdg_dir`.

## Conventions

- **New UI is built from the UI kit** ([`docs/ui-kit.md`](docs/ui-kit.md), `ui/src/kit/`) — the
  New Project wizard's and the GitLab panel's look, rebuilt on the tokens. Import the kit's
  `Button`, `TextInput`, `Badge`, `Dialog`, … rather than writing another `.primary`/`.action`/
  `.row` class; the app had ~45 secondary buttons before M100 because every feature drew its own. A part the
  kit lacks is **added to the kit first** — component, specimen on the kit page, entry in
  `docs/ui-kit.md` — then used; changing a kit part means updating its specimen and entry in the
  same change. `check:kit` fails on a component without either. The app is on the kit since the redesign (M100): a surface whose
  markup a check pins composes the kit's classes instead of rendering the component —
  `docs/ui-kit.md` rule 3.
- **Comments here carry the why, at length, including the option that lost and the bug the
  code prevents.** Match that density; do not "tidy away" a comment that names a failure — most
  of them exist because the failure happened.
- Third-party versions live only in the root `[workspace.dependencies]`; crates say
  `foo = { workspace = true }`. `wry` and `tao` must never become direct dependencies (0.x, so
  a second version forks the webview stack); `gtk` is pinned to exactly what tauri pins.
- `ui/package.json` pins every version exactly, no carets — Vite 8's Rolldown/Oxc pipeline
  makes an unannounced plugin break expensive.
- Agent worktrees live in **`.cide/worktrees/`** (`cide_git::worktree::WORKTREES_DIR`) and are
  gitignored; a blind `git add -A` during a parallel run would otherwise commit several complete
  copies of the tree. This line said `.claude/worktrees/` until M30 and was wrong — newly
  confusing, too, next to a feature that reads `.claude/agents/`. That the checkouts sit **inside**
  the project root is load-bearing rather than incidental: `claude --agent` finds a project
  subagent by walking up from the child's cwd, so moving them out would break every project-scoped
  subagent with no error anywhere (`cide-git`'s `discovery_containment` is the guard).
