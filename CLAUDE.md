# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

cide is an IDE whose centre of gravity is a live Claude Code session rather than a text
buffer: a pinned, non-closable Claude tab per project hosting a tiling grid of panes. Rust +
Tauri 2 backend, React 19 + Vite 8 frontend, Linux-first (developed on KDE/Wayland).

`README.md` is the landing page — short, screenshotted, aimed at somebody deciding whether to
try cide. The honest record lives in `docs/journal.md`: the milestone-by-milestone write-up,
including which acceptance criteria were never confirmed on a display. Read that before assuming
a feature works end to end, and append to it — not to the README — when finishing a milestone.
`docs/platforms.md` is the same discipline for what is known off Linux, and `CONTRIBUTING.md`
holds build, run, check and packaging.

## Running it

```sh
pnpm --dir ui install     # once
./run.sh
```

**Use `./run.sh`, not the binary.** A debug build does not load `ui/dist` — `tauri.conf.json`
bakes `devUrl: http://localhost:1420` into the binary, so launching `./target/debug/cide`
with nothing on that port gives a white window and a connection error. `run.sh` builds
`cide-app`, starts Vite, waits for the port, stops any previous instance (SIGTERM, so the
workspace flushes), reaps orphaned `claude` children, and refuses to start when the saved
workspace would open more than eight windows.

**`./run.sh` launches the `dev` profile, not your real instance.** cide is developed inside
cide, so the two run side by side; before profiles they shared one `workspace.json` and one
Tauri app directory, and whichever exited last stamped its layout over the other's. A profile
moves the whole footprint — `$XDG_STATE_HOME/cide-dev` and `$XDG_CONFIG_HOME/cide-dev`, and a
suffixed bundle identifier so the WebKit storage, the log and the remembered window geometry
move with it — and prefixes the OS window title with `[DEV] ` so a task switcher tells them
apart. `cide_core::profile` is the whole rule; `CIDE_PROFILE` is how it is set, and children
inherit it, so `cide-headless tree` in a profiled pane inspects that profile's workspace.

**The rust-analyzer fork** (ADR 0011) lives in `../forks/rust-analyzer`, branch `cide` —
never a workspace member — and since Phase 1 it drags its sibling **`../forks/salsa`**, a
patched 0.28.2 wired through `[patch.crates-io]` (the manifest's `../salsa` resolves relative
to the fork, so the pair must stay siblings of each other), which the disk
index's tracked-struct restore cannot exist without — and, since Phase 3, its LRU-evicted-memo
disk tier (`memos.redb` beside the snapshot, reloading only what shallow-verifies — ADR 0011,
Decision 5). A dev build has no bundled sidecar, so it
runs the PATH rust-analyzer with zero setup; to run the fork instead, build it once in the
sibling and launch with `CIDE_RA_PATH=$HOME/work/forks/rust-analyzer/target/release/rust-analyzer
./run.sh` — the override outranks both the bundled binary and PATH, and refuses (rather than
falls through) when it points at nothing executable. To carry the overrides on every launch
without retyping them, put plain `KEY=value` lines in **`.env`** at the repo root — run.sh
sources it (allexport) when present, a missing file is simply an empty one, and it is
gitignored because the paths in it are one machine's. Packaging any bundle requires all three
forks at the revisions `packaging/*.lock` pin — `./scripts/clone-forks.sh` checks them out, and
preflight names whichever is missing or off its pin. **`docs/forks.md` is the whole story**: what
each fork is, why the two-sibling rule is load-bearing, how a pin moves, and what CI checks about
one. **Each fork also carries its own `CLAUDE.md`** — the rust-analyzer one is the merge-survival
guide (what must outlive an upstream rebase, and the six invariants that break with no compile
error), the salsa one is a porting guide: it is a real fork of `salsa-rs/salsa` since 2026-08-28,
so a version bump is a `git rebase` onto the next release tag, but never a mechanical one — each
patch is re-argued against what upstream's persistence has grown, and retiring one of ours in
favour of theirs is the good outcome. Read them before touching either repo.

**The gopls checkout** (ADR 0011, Decision 6) is the third fork: `../forks/tools`
(golang/tools — gopls is its `gopls/` module), branch `cide` from the tag
`packaging/gopls.lock` pins, built as the `cide-gopls` sidecar with the host's `go`
(`GOTOOLCHAIN=auto` fetches the toolchain the module demands). **No cide patches yet** — it
builds stock gopls under the cide name; its `CLAUDE.md` records the planned patch surface
(persist the metadata graph, cap the in-memory layer) and the measure-first gate. Dev
override: `CIDE_GOPLS_PATH=$HOME/work/forks/tools/gopls/cide-gopls ./run.sh`. The shipped
gopls is configured through the **env lane** (`cide_lsp::config::extra_env` — `GOPLSCACHE`
into the per-profile cache dir), not `initializationOptions`.

**The `openspec` CLI** (ADR 0012) is the one dependency cide neither ships nor bundles. M28's
OpenSpec support reads `openspec/` by running `openspec … --json`, so a machine without it gets a
panel that says so and names `npm install -g @fission-ai/openspec` — and nothing else about the
feature appears. It is installed by `npm -g`, which puts it in a Node directory a *shell* rc file
adds to `PATH` and a desktop launcher does not, while `toolchain::extra_dirs` is `~/.cargo/bin`
and `~/go/bin` and its header forbids widening that list. So `cide_spec::discover` carries its own
rungs (`CIDE_OPENSPEC_PATH` alone and first, then `which`, then `$NVM_BIN`/Volta/pnpm/npm-global
and `~/.nvm/versions/node/*` newest first), and `child_env::run_filter_with` hands the chosen
directory back to the child — because `openspec` is a `#!/usr/bin/env node` script, so without it
`execve` succeeds and the shebang dies with `env: node: No such file or directory`. To see which
answer a given launch gets, run `./target/debug/cide-headless spec <root>`.

**One cide per profile, and `cide --help` is not a launch.** Both are new, and both exist
because of the same morning: this binary's path is in `$EDITOR` inside every pane, cide parsed
no argument but `--wait`, and an agent probing the CLI started a whole second IDE on the live
profile — then took down every `claude` in the first instance trying to clean it up.
`cide_app::cli` answers `--help`/`--version` and refuses an unknown flag (a bare path is still
ignored, not refused); `cide_app::instance` writes a pid into
`$XDG_STATE_HOME/cide-<profile>/instance.lock` and refuses a start while that pid is a live
cide. Every doubt — no file, a corrupt one, a dead or unidentifiable pid — starts the
application, because an IDE that will not launch is the worse failure.
`CIDE_ALLOW_SECOND_INSTANCE=1` is the deliberate override; `run.sh` needs it for nothing, since
it stops the profile's running instance first.

A profile starts **factory-fresh** — settings live inside `workspace.json`, so a new one has no
installed extensions, no global agent roles and the default keymap. That is the point, and it
is the first thing to remember before filing "my extensions are gone".

```sh
./run.sh --release        # embeds ui/dist, needs no dev server (run `pnpm --dir ui build` first)
./run.sh --profile <name> # run under another profile; `default` shares the real instance's state
./run.sh --fresh          # start from an empty workspace (moves the old one to .bak)
./run.sh --bench          # CIDE_BENCH=1        IPC transport gate (M0), prints and exits
./run.sh --audit-chrome   # CIDE_AUDIT=1        48 chrome dimensions vs the design mock, both themes
./run.sh --audit-panes    # CIDE_AUDIT_PANES=1  100 split/close/maximize cycles over the real domain
./run.sh --audit-windows  # CIDE_AUDIT_WINDOWS=1 detach/re-dock and window modes
./run.sh --inspect        # console into the Rust log + WebKit inspector on 127.0.0.1:9222
./run.sh --on-top         # CIDE_ON_TOP=1, for screenshots (KDE won't raise a shell-launched window)
```

The audits need a display and are not in CI; they are the only checks for the pieces that
cannot be verified by reading them. `CIDE_NO_GRAPHICS_WORKAROUNDS=1` skips the Linux graphics
ladder — the app does not start on stock KDE Wayland without that ladder (ADR 0006).

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

- **One Rust test:** `cargo test -p cide-git branches`. `--locked` matters (a re-resolved
  lockfile is the drift class CI exists to catch); `cargo xtask` is a `.cargo/config.toml`
  alias, so global flags lead: `cargo --locked xtask codegen`.
- **`#[ignore]`d tests are ignored on purpose.** Most spawn the real `claude` — they need the
  binary on PATH, an authenticated account, network, and they spend the user's own quota. The
  rest is `cide-pty`'s 1 GiB soak. Run them deliberately: `cargo test --workspace -- --ignored`.
- **The frontend has no test runner.** `ui/scripts/check-*.mjs` *are* the suite: each compiles
  a deliberately import-free module with the TypeScript in `node_modules` (or SSR-bundles an
  entry through Vite) and asserts on the output. Adding a `check:foo` script to
  `ui/package.json` is enough — CI enumerates them rather than listing them. Several modules
  are import-free *so that* their check can compile them standalone; keep them that way.
- Icon drift: `scripts/gen-icons.sh --check`. Packaging: `cargo xtask package [--appimage|--deb|--flatpak|--tarball|--app|--dmg|--src] [--check|--write|--run]` — without `--run` it only prints a plan. Naming no target means everything *this host* is responsible for; naming a bundle the host cannot build (`--dmg` on Linux) is a preflight failure, because nothing here cross-compiles. `--src` is the exception and the cheap one: `git archive` of HEAD into `target/release/bundle/src/cide-<version>-src.tar.gz`, byte-reproducible, buildable on any host but in the **Linux** default set only, so a release matrix uploads one source tarball rather than two that differ. It refuses a dirty tree — a tarball cut from one is a false claim about a commit. `--tarball` is the *binary* one and makes the opposite promise: `target/release/bundle/tarball/cide-<version>-linux-<arch>.tar.gz`, the three binaries (`cide`, `cide-hook`, and since M25 `cide-rust-analyzer` — cide's own rust-analyzer build from the sibling fork pinned by `packaging/rust-analyzer.lock`) plus the desktop entry, icons, README and LICENSE under a `cide-<version>/` prefix. It carries no WebKitGTK, so it needs the host's 4.1 — but the bundled rust-analyzer means it is no longer "a tenth the size of the AppImage"; both archives carry the same ~50 MiB fork. Compiled output, so nothing about its bytes is reproducible. Linux only. **Every packaged target now needs `../forks/rust-analyzer` checked out at the pinned rev** — preflight fails with the clone command when it is absent. `./build.sh` is the wrapper for daily use: one artefact for the host (AppImage on Linux, `.dmg` on macOS), then installs it into `$CIDE_INSTALL_DIR` (default `~/bin`, Linux only — a `.dmg` is opened, not put on PATH). `--all` for the host's whole default set, `--plan` to build nothing, `--no-install` to stop after building. **Releases are `.github/workflows/release.yml`**, dispatched by hand with a version: it cuts `release/v<version>` from master, writes that version into the four files that carry it (Cargo.toml, tauri.conf.json, ui/package.json, and the regenerated flatpak metainfo — they drifted before it existed), tags, builds every artefact through `cargo xtask package`, and publishes one GitHub release with a SHA256SUMS. **Never bump the version by hand for a release, and never bump master to the version you are about to release** — that file then differs by no lines and the workflow's `1 1` numstat guard refuses the run. A final `bump` job puts master on `<next patch>-dev` after publishing, because the release branch is never merged back and the trunk otherwise keeps a version two releases old; `scripts/bump-version.sh` is the same edit by hand, and release.yml now calls it.
- **Linux is the only platform cide has ever run on.** `docs/platforms.md` is the record: what a `cfg` arm does on macOS instead, which guarantees have no equivalent there (`PR_SET_PDEATHSIG` above all), and what still needs a Mac. The `macos` CI job is advisory until it has passed once.
- **The macOS arms type-check from here for nine of the twelve crates, and it is worth doing before touching a `cfg`-gated one.** `docs/platforms.md`'s *Type-checking for macOS from Linux* has the command; it uses `scripts/darwin-cc.sh` and `DOCS_RS=1`. **`cide-app`, `cide-git` and `xtask` are excluded** — `git2` is `vendored-openssl`, so they build OpenSSL for Darwin and that needs a real cross toolchain. So it covers `cide-core`'s `child_env` arms and every other domain crate, and it does *not* cover the crate that links tauri: the two errors a Mac reported in M16 were both in `cide-app` and this would have caught neither. It proves nothing about linking or runtime, and it is not a substitute for the `macos` CI job.

Which check covers what you touched:

| you changed | run |
| --- | --- |
| `keys/`, a default binding, `cide-core::commands` | `check:keys`, `check:commands`, `check:switcher` |
| `sidebar/GitPanel/`, `cide-git` | `check:git`, `check:render`, `check:diff`, `check:diff-render`, `check:branches` |
| **the diff surface — `panes/GitDiffPane*`, `diffRows`, `diffSync`, `diffTokens`, `diffHighlight`, `diffConnector`, `changeNav`** | `check:diff-render` above all, plus `check:scheme`, `check:markdown` and `check:merge`. Four import-free modules are compiled and driven standalone there; keep them that way. Three separate silent-failure classes live here: **colour** is only safe because `diffTokens.lineTokens` compares each row's content against the line its tokens spell — drop that guard and a moved rev paints a *plausible* lie (a keyword-red word on a line with no keyword in it, no gap, no artefact, nothing logged). **The tokenizer must stay behind a dynamic `import()`**: `GitDiffView` is SSR-bundled and run under node by the check, which greps the emitted bundle for a CodeMirror *import statement* — matched on the statement because `diffBlame.ts` says the package name in prose and that comment survives into the bundle. And **`data-current` must stay last** in `cell()`'s attribute spread, because the smoke fixture's row regex is attribute-ordered: move it and the regex matches nothing and every positional digest goes silently empty |
| **pull, merge, rebase — `cide-git`'s `pull`/`merge`/`conflict`, the divergence dialog, the branch popup's "Merge into current branch"** | `check:pull-strategy`, `check:branches`, plus `cargo test -p cide-git --test pull --test conflicts --test merge` (differential against the real `git`, including the merge message byte for byte — and `pull` and `merge` deliberately have *different* message oracles: `git pull`'s comes from `fmt-merge-msg` over `FETCH_HEAD`, `git merge`'s does not) |
| **push — `cide_git::push`'s `preview`/`push`/`check_lease`, `PushDialog`, `pushModel`, `pushStore`, `pushRun`** | `check:push`, `check:branches`, plus `cargo test -p cide-git --test staging -- preview lease blocked diverged publish`. Three surfaces raise the dialog — the palette, the branch popup and *Commit and Push…* — and every one of them must go through `openPushDialog`, because two routes to one gesture that disagreed about whether it asks first is the split M20 already paid for here. Two silent-failure classes live in this surface. The **preview must describe the push that happens**: it resolves the remote with `default_remote` and spells the refspec exactly as `push` does, so a preview that derived either independently would draw a dialog that is internally consistent about a different push — and the count is `commits.length + more`, never `BranchInfo::ahead`, which is measured against the *upstream* and is zero for the publish case the dialog most needs a number for. And **force is a lease**: the binary route hands git `--force-with-lease` bare (an oid captured when the dialog opened is stale the moment anything else fetches), while the libgit2 route has no lease at all and cide performs the comparison itself in `check_lease` — delete it and the `+` refspec silently deletes whatever somebody else pushed, reporting success |
| the conflict resolver — `panes/MergePane*`, `panes/mergeModel.ts`, `sidebar/GitPanel/MergeBar.tsx` | `check:merge`, `check:render`, `check:theme`, `check:ui-scale` |
| `toolwindow/`, the activity rail, the bottom panel | `check:toolwindow`, `check:toolwindow-render`, `check:sidebar`, `check:menus` |
| `gitlog/`, `cide-git`'s `log`/`lanes`/`show`/`revision` | `check:log`, `check:log-render`, `check:diff-render` |
| blame — `editor/blame*`, `cide-git::blame` | `check:blame`, `check:editor` |
| **the change column — `editor/change*`, the `.cm-gutters` reservation** | `check:change-bars`, then `check:editor` and `check:blame`, which pin the same reservation and the same extension array. The markers are diffed **in the webview**, against HEAD's blob and the *live* document — `git_diff_file` reads the file on disk, which is wrong for every unsaved buffer, and that is the state the editor is in whenever the column is most wanted. Four silent-failure classes live here: the HEAD blob must be split **exactly as CodeMirror splits a document** (`splitBaseline`) — on all three break shapes, or a CRLF file keeps a `\r` on every line and paints the **whole file** modified; and *keeping* the empty line after a final newline, because `Text` keeps it and the diff pane's `splitLines` correctly drops it, so borrowing that rule puts a phantom addition on the last line of nearly every file in the repository; the bar rules must stay **three classes deep**, or `highlightActiveLineGutter` paints over the marker on the one line the user is editing; and `revertPlan`'s `eat` decides which surrounding line break a removal consumes, which is the one operation here that **destroys** the user's text rather than mispainting it, and the revert round-trip over sixteen block shapes in `check:change-bars` is its only automated defence |
| the task tracker, `.cide/`, agent definitions — `cide-tasks`, `cide-agents`, `sidebar/TasksPanel/`, `sidebar/AgentsPanel/` | `check:agents`, `check:agents-render`, `check:sidebar`, `check:commands` |
| **task attachments — `cide_tasks::attachments`, `TaskStore::attach`/`attach_bytes`, the `DetachAttachment` edit, `cmd/tasks.rs`'s seven attachment commands, `cide_task_attach` and the `attachments` field of `cide_task_create`/`cide_task_update`/`cide_task_comment`, `TasksPanel/AttachmentStrip`, `AttachmentLightbox`, `attachmentPreviews.ts`, `fileDrop.ts`** | `cargo test -p cide-tasks` (the copy, the cap, the sanitiser, `repair`, the three merge rows), `cargo test -p cide-agents` and `cargo test -p cide-app agent_rpc` (the tool, the worktree-relative path, the render's absolute paths), `check:agents` (`ATTACHMENT_KINDS` scraped from `tasks.rs`, the size ladder, the lightbox walk, the drop model), `check:agents-render` (the five attachment stories; every older story must still digest no tile, no `<img>` and no paperclip), `check:selectors`, `check:ui-scale`, `check:motion`, `check:ui-icons`. **Bytes never cross the IPC**: a file is named by a path and copied by Rust into `.cide/attachments/<task>/<attachment>/<name>`, and a thumbnail is an `<img>` over the asset protocol after `task_attachment_image` has **re-sniffed the bytes** — the stored `AttachmentKind` is a hint in a committed, hand-editable file and must never decide the grant. Five silent-failure classes. `repair` must **drop, never re-mint** an attachment record: the id is a directory name, and a re-minted record leaves the bytes orphaned under the old one with no symptom but a thumbnail that never loads. `reconcile_comment` must **union** attachments rather than take the winner's list, or a text edit on one side silently drops a screenshot attached on the other. A refused source must refuse **before any copy** (`import`'s two passes), or a comment lands naming files it does not have. A relative path in an agent's call resolves against the **run's cwd** (`TaskSink::cwd`, from `AgentRegistry::run_scopes_for`) — its worktree, or the project root for a run dispatched with no task (M40) — never a default: a queued run has no cwd and is filtered out. And the desktop drop rides Tauri's own drag event because the native handler swallows HTML5 DnD; if that event never fires the failure is total silence, which only a display can rule out |
| **the role format, or anything that reads `.claude/agents/`** — `defs.rs`'s `frontmatter`/`render`/`save`, `AgentScope`, `harness/claude.rs`'s `--agent` road | `cargo test -p cide-agents` above all — the **corpus** (`claude_corpus`) is the whole promise that cide does not damage a file it did not write, asserted on each key's *raw source text* rather than its parsed value, because a `hooks:` block flattened to one line has an identical value and a destroyed file; `a_real_subagent_survives_a_real_save` makes the same claim through the disk, where `save`'s find-the-file-that-declares-this-name resolution could otherwise write a second file beside the user's and pass everything that never looked. Then `check:settings-agents` (`AGENT_FIELDS` = `AgentField`, four scopes, cross-family shadowing), `check:agents` (`SCOPES` = `AgentScope`, the badge total), `check:agents-render` (the chip on both Claude scopes and on neither cide one), and `cargo test -p cide-fs`/`-p cide-app` for the watch and the `.claude` route. Two silent-failure classes live here: cide's dialect must go on **refusing** what it always refused (a mode, not a twin parser — `cides_own_format_still_refuses_what_it_always_refused`), and every flag `--agent` makes cide's to *not* send is one that would silently **win** over the definition if it leaked back in |
| **OpenSpec — `cide-spec`, `cmd/spec.rs`, `spec_state`, `spec_triggers`, `sidebar/OpenSpecPanel/`, the card's spec block** | `check:openspec`, `check:openspec-render`, `check:openspec-config` (the `openspec/config.yaml` form and its init wizard, which live in a modal off the **panel header** and not under Settings — cide's Settings is global and that file belongs to one repository), `check:agents-render` (its unchanged digests *are* the claim that a task without a change renders as before), plus `cargo test -p cide-spec` and — the ones that matter — `cargo test -p cide-spec -- --ignored`, which run the real `openspec`. Three CLI facts are silent bugs the other way and only those tests see them: a `--json` failure **exits 0** and reports itself in a `status` array; root resolution **walks ancestors**, so an agent's worktree with no `openspec/` of its own reports the *parent repository's* progress as that run's; and a bare `init` **prompts**, which on a command worker is a hang. A fourth fact is not about the CLI's output but its *installation*: which commands a project has, and **how they are invoked**, is read from that project's directory by `cide_spec::claude` and never spelled in cide — OpenSpec moved its workflow from slash commands (`.claude/commands/opsx/`, `/opsx:propose`) to Claude Code skills (`.claude/skills/openspec-propose/`, `/openspec-propose`), and the nine strings that had the old prefix compiled in made every button in the panel refuse on every correctly set up project, with a sentence recommending `openspec update` that on such a project writes nothing. `SURFACES` is the table, newest first, and each row carries a **rename** map because the prefix was not all that moved — `apply`/`archive`/`sync`/`update` became `apply-change`/`archive-change`/`sync-specs`/`update-change`, so cide's handle is the current name and the older row translates; a table without it answers *no such command* on a legacy project for exactly the commands a dispatch types. A fifth fact is about *timing*: Claude Code reads a project's skills once at launch, so a conversation older than the file cannot know the command — `SessionRegistry::started` against `claude::installed_at` is the comparison, `spec_run_command` refuses on it and the dispatch road degrades to prose. A sixth is that `archive` **moves** a change out of `changes/` and *no* CLI command reads the result, so `cide_spec::archived_change` reads the directory itself — deltas through the byte-range scanner, documents as a listing, `SpecOrigin::Archived` on the answer, and the checklist and verdict deliberately **empty** because neither is recoverable and both render as a lie (`0/0` and a green *Valid* over merged work). `check:openspec` fails on any invocation spelled anywhere in the panel; and `./target/debug/cide-headless spec <root>` prints what a given project resolves to. The write path is guarded by a corpus round-trip (`tests/corpus.rs`) asserting that splicing a requirement back as itself leaves the file byte-identical — front matter, comments, BOM and CRLF included |
| **which pane a connected `claude` belongs to — `cide_core::proc`, `cide-ide-mcp`'s `pane_of_pid`/`unbound_pids`, `ide::resolve_unbound_connections`, `pane_bind_session`** | `cargo test -p cide-core proc`, `cargo test -p cide-ide-mcp` and `cargo test -p cide-app ide` — attribution is by **pid**, and the pid cide forked is not always the pid that connects: `claude` behind a launcher (a version manager, an npm shim, a `bbin agent claude` wrapper) or typed into a shell pane announces a *descendant's* pid, so the join is an ancestry walk and *nearest owned ancestor wins*. It reads platform-specific and is not — a wrapper breaks it identically everywhere, which is why it was reported as "macOS only". Every rule that can be got wrong is pure and runs on any host; the platform half cannot, so `parent_of_pid`'s macOS arm is only ever compiled by the Darwin type-check in `docs/platforms.md`'s *Type-checking for macOS from Linux*, which is what caught that `libc` defines no `kinfo_proc` for Apple. And a binding cide *derived* must be recorded against the pid it was derived from, because `report_exit` reaps the pty child and has never heard of the CLI behind it |
| the commit actions — `cide-git`'s `replay`/`reset`/`tag`, `chrome/logActions.ts` | `check:log-actions`, plus `cargo test -p cide-git --test commit_actions` (differential against the real `git`) |
| the file tree, fs ops | `check:tree-status`, `check:tree-flicker`, `check:fs-clipboard`, `check:new-entry` |
| **pasting an image — `cmd::fs::fs_paste_image`, `cide_core::image`'s `encode_png`, `ops::write_new_bytes`, `editor/pasteImage.ts`** | `check:paste-image`, `check:fs-clipboard`, `check:editor`, plus `cargo test -p cide-core image` and `cargo test -p cide-fs`. **The clipboard is read, encoded and written entirely in Rust** and the frontend only names a directory — `crates/cide-ipc/src/image.rs`'s rule (pixels never cross the IPC) applied to the one direction that had never needed it, and it is also what sidesteps all three objections `ui/src/terminal/clipboard.ts` records against the *webview* `read_image` (an ungranted capability, and a `ResourceId` into the window's resource table that leaks a full bitmap without `core:resources:allow-close`). The **host** API is neither capability-gated nor resource-backed. Four silent-failure classes live here: the paste event's `types` list is the detector and the Rust command is the reader, because a synchronous handler cannot await a clipboard — turn that around and `preventDefault` is decided after the browser has already pasted; `wantsImagePaste` must go on refusing a clipboard holding **text as well**, or a copied file path stops pasting as a path (the same trade `terminal/clipboard.ts` makes, for the same reason); `relativeTo` takes the **document** path and not its directory, and an off-by-one `../` renders as a broken-image glyph with the file present on disk and nothing logged; and a generated name always contains spaces, so a markdown destination must be `<`-wrapped or CommonMark parses it as a link to `Pasted` with prose after it. `FsError::NoClipboardImage` is its own variant because Ctrl+V reaches this command whenever the in-app file clipboard is empty — an empty or text-only system clipboard is the *ordinary* outcome and must say nothing |
| **which shell a pane runs, or what `PATH` a child gets — `cide_core::shell`, `cide_core::login_path`, `toolchain::discovered_dirs`** | `cargo test -p cide-core` (the shell ladder over injected inputs, the marker parser, the additions rule, and `the_path_a_child_searches_is_the_path_which_searched` — now asserted over `discovered_dirs`), and `docs/platforms.md`, which is the record. Two rules. **A shell pane asks for `program: ''`**, meaning *the user's login shell*, and `session_spawn` substitutes it — `/bin/bash` hardcoded in the webview is a shell nobody on macOS configures, so the pane ran without `~/.zshrc` and therefore without nvm or Homebrew. And **`login_path` is the only thing in this workspace that spawns a shell to ask a question**: `toolchain`'s header still says nothing in *it* spawns a process, `discovered_dirs` is where the two lists join, and `child_env::run_filter_bare` exists so the probe is the one child that does not get cide's augmented `PATH` — the ordinary path would wait on the result it is computing. It runs only where `login_path::warm` is called, which is `cide_app::run` and nowhere else, so every test and every other binary behaves as it did before |
| **the command line, or the one-instance guard — `cide_app::cli`, `cide_app::instance`, and `main`'s order** | `cargo test -p cide-app -- cli:: instance::`. Two rules, both written because breaking either is silent and expensive. **An argument this binary does not know must refuse, not launch**: `child_env::editor_env` puts cide's own path in `$EDITOR` for every PTY child, so `cide --help` from a pane started a *second IDE on the same profile* — and the cleanup an agent then reached for (`pkill -f mount_cide`) matched every `claude` in the *first* instance, because their `--settings` argv carries that instance's `cide-hook` path. Every session in every project died, twice in one morning. A **positional** argument still starts the app, deliberately: the bias in both modules is that a wrong refusal to launch is worse than a wrong launch, which is also why a corrupt, unreadable or unidentifiable claim is a start and why the holder must be established both *alive* and *a cide* (`/proc/<pid>/comm` — pids are reused, and off Linux the guard simply does not exist, see `docs/platforms.md`). And the claim must be dropped by whichever exit the teardown takes — `lifecycle`'s `ReleaseClaim` — or the file goes on naming a pid the kernel will one day hand to another cide |
| a synthetic tree row — a group, a note, a pin | `check:groups`, `check:notes`, `check:scratch`, `check:tree-drag` |
| `layout/` — splits, dividers, pane grid | `check:rows` |
| **moving a pane — `layout::move_pane`, `paneMove.ts`, `paneDropZone.ts`, `usePaneDrag.ts`, the grab handle** | `check:pane-move`, `check:rows` (a terminal pane's cluster reserves one action more than the base, and both numbers are derived *and* pinned), `check:detached` (`ClusterPlan` is shape-asserted), plus `cargo test -p cide-core layout`. Three silent-failure classes live here: the hit test must be rooted at the active tab's `[data-audit="paneTree"]`, because `TabContent` lays **every** tab out at full size behind `visibility: hidden` and an unscoped rect scan drops the pane into a tab nobody can see; the drop-zone store's snapshot must stay a **primitive**, or `useSyncExternalStore` re-renders for ever and unmounts the root; and `move_pane` must commit a **clone**, because the destination's legality is only decidable after the removal and a refusal mid-way drops a live session's pane out of the tree entirely |
| **a splitter's drag path, `layout/resizeGesture.ts`, or any `ResizeObserver`** | `check:resize` — a drag is smooth only because the expensive reactions to a size change (xterm's `fit()`, a `session_resize` that reflows the scrollback on the IPC thread, a minimap repaint) are deferred to the end of the gesture. Undo that and nothing throws, nothing changes on screen, and the app simply locks up while somebody drags a divider |
| `editor/` | `check:editor` |
| `editor/markdown/`, the markdown preview | `check:markdown`, `check:editor`, `check:ui-scale` |
| **anything a CodeMirror surface draws — a new pane that mounts an `EditorView`** | `check:scheme` — every surface must pass `polarityExtension`/`watchPolarity` from `editor/cmPolarity.ts`. `@codemirror/view`'s base theme is written against generated `&light`/`&dark` classes, and nothing set `EditorView.darkTheme` until M24, so every editor in cide was a *light-mode* CodeMirror under the dark theme. Forty-odd rules across view, autocomplete, lint, search and merge; most were masked by our stylesheets, and the one that was not — the selection, at specificity (0,6,0) against our (0,3,0) — shipped as a lavender block with the text at 1.02:1 on it. Its colour lives in `editor/highlight.css`, the only *global* stylesheet, because a module's hashed prefix cannot reach that specificity |
| **a syntax colour, a `--tk-*`, or `TOKEN_ROLES`** | `check:scheme`, then `check:editor` — ink is measured against **two** grounds, the background and `--tk-sel`, and the load-bearing assertion is that selecting costs no role more than 20% of its contrast — an absolute floor is not enough and was tried first. Contrast on a selection is `(ink+0.05)/(sel+0.05)`, so a selection's *luminance* taxes every ink at once and no floor is reachable by tuning inks; the dark `--tk-sel` is therefore iso-luminant with the background and visible through chroma alone. Checking one ground and painting on the other is how the dark theme shipped with comments at 1.73:1 on a selection, invisible, from M3 to M24. The role set is spelled in **four** files that cannot import one another (`cide_ipc::theme`, `editor/scheme.ts`, `editor/highlight.css`, `tokens.css`) and every way they disagree is silent: a role declared and read nowhere is a colour nobody sees, and one read and declared nowhere paints *nothing at all*, because an undefined custom property is not a colour. Order in `TOKEN_ROLES` is precedence — `HighlightStyle` resolves a tag through its parent chain — so a role claiming a child tag must precede the role claiming its parent, and the wrong winner looks fine |
| **`ColorScheme::normalise`, and anything about an imported scheme's selection** | `cargo test -p cide-ipc theme` — a theme may omit `editor.selectionBackground` entirely (Min Dark does) or state one that is useless, and both render as *no selection at all*. `normalise` therefore **validates** rather than fills: under `MIN_SELECTION_DISTANCE` redmean units it derives one, tinting towards chroma rather than lightness so the inks keep 93–99% of their contrast. Alpha is **composited** onto the theme's own background, never dropped — `#ffffff20` is a lifted grey, `#ffffff` is a white block over the text |
| **showing a rejected `invoke` to a user** | `ui/src/ipc/errorText.ts`, never `String(e)` — `CoreError` is a tagged enum, so a rejection is `{ kind, detail }` and `String()` of it is `[object Object]`. That is what the colour-scheme import showed instead of the sentence naming the file and what was wrong with it. `check:scheme` pins the helper and greps the two Settings surfaces, because a failure path is only ever seen by the person it is failing |
| **`cide_core::scheme`'s `SURFACE_KEYS`** | `cargo test -p cide-core scheme` — the editor's colours come from `editor.*` keys **only**. The workbench `foreground` is a sidebar/status-bar colour and themes routinely make it far dimmer; reading it as the editor's ink dropped every unroled identifier in an imported theme to a mid grey. Where a theme states neither ground the fallback is **VS Code's** default (`VS_DEFAULT_BG`/`_FG`), not cide's palette: an imported scheme is a rendering of somebody else's theme |
| **`cide_core::scheme`'s `score`/`NO_WEAK_EVIDENCE`, or `theme::fallback`** | `cargo test -p cide-core scheme`, and check a real theme with `CIDE_VSIX=<path> cargo test -p cide-core a_real_package -- --ignored --nocapture`, which prints every role grouped by colour. The matcher scores by **shared dot-segments first**, direction second — a broad rule beating a qualified one is how Min Dark's JSON keys came out the wrong colour. `NO_WEAK_EVIDENCE` refuses the narrower-sibling rule for punctuation/bracket/operator: a rule on `punctuation.separator.key-value` is a special case, not a statement about every comma, and reading it as one painted a whole buffer keyword-red |
| **the VS Code theme importer — `cide_core::scheme`'s `SCOPES`** | `cargo test -p cide-core scheme` — a scope matcher with precedence rules, differential against theme files shaped like real ones. It is Rust and not TypeScript so that it *can* be tested; the stored scheme is the converted one, so editing `SCOPES` never repaints somebody's existing import. The `.vsix` road builds its archive in the test, so nothing there inflates a byte — check a real package with `CIDE_VSIX=<path> cargo test -p cide-core a_real_package -- --ignored --nocapture`, which prints each theme's converted key/string/number |
| menus, header, chrome, settings | `check:menus`, `check:menu-model`, `check:tab-overflow`, `check:sidebar`, `check:theme`, `check:fonts`, `check:proxy` |
| **any `font-size`, or a box drawn around text** | `check:ui-scale` — chrome type is a closed ladder of `--fs-ui-*` rungs over one `--ui-scale`, and a bare `font-size: <n>px` is a label that silently stops following the UI font size. Looks right at the default, which is where you are working |
| a new sidebar panel, or anything `App.tsx` renders as one | `check:boundary` — an unwrapped panel takes the **whole window** down when it throws, and the rail's choice is restored on launch, so it stays down |
| **any `useWorkspace`/`useStore` selector** | `check:selectors` — a selector that *returns* a fresh array or object re-renders for ever and ends at *Maximum update depth exceeded*, which unmounts the whole root. The render checks SSR the pure views, and one server pass runs no updates, so nothing else in the suite can see it |
| **closing or reopening a project — `close_project_here`, `workspace::reopen_project`, `persist::closed.json`, the per-project restore plan** | `cargo test -p cide-core -- reopen closed`, `cargo test -p cide-app -- lifecycle sessions_of`, `check:selectors`, `check:attach`. Closing a project **stops its children and keeps its layout**; reopening the directory puts the same panes back with the same session ids, and the frontend asks `app_restore_plan` for that one project and holds its panes until the answer is in. Three silent failures live here: a stopped session left in the registry is *adopted* by the reopened pane (`sessionIsHeld`) instead of resumed, so the ladder thread removes it; a parked host left in the webview does the same through the ledger, so `closeProject` destroys them; and a pane rendered before its project's plan lands latches the splash decision without it, so `plannedProjects` gates the tree per project |
| terminal input, session state | `check:input`, `check:exit`, `check:awaiting`, `check:format` |
| **rewriting a session's output — `cide_pty::LineRender`/`Rendered`, `render_lines`, `cide_core::jsonlog`, `lifecycle::json_log_render`, and the click-to-expand road — `logring`, `session_log_detail`, `terminal/logLink.ts`, `chrome/LogDetailCard`** | `check:json-log`, `cargo test -p cide-pty`, `cargo test -p cide-core jsonlog` and `cargo test -p cide-app a_shell_renders` — the hook runs **upstream of the vt100 mirror**, which is what makes one rendered stream out of the mirror, every sink and the choked-sink catch-up, and which is also why a line it claims wrongly is *destroyed*: not in the scrollback, not in a reattach snapshot. Three silent failures live here. The detector must go on **refusing** what it refuses — `jq -c` output, a minified `package.json`, an API response with a `message` — which is why two of {time, level, message} are required and why the negative corpus is the real specification. An unterminated tail may be held only while the hook says it could still complete (`holding_only`): held unconditionally, a shell prompt is invisible until the user presses Enter, and the pane reads as hung — and a test whose deadline outlives the child cannot see it, because EOF flushes a held tail too. And `Rendered::Keep` must emit the **original bytes**, terminator included: a line handed back as its own text is re-terminated `\r\n`, which walks every line of a raw-mode TUI back to column 0. The way *back* to the whole event is an **OSC 8 hyperlink over the timestamp and level only** — never the whole line, because xterm discards a later provider's link where it overlaps an earlier one and its OSC provider runs first, so a full-line link silently deletes the file-path link inside `caller=srv/main.go:42`. The handle it carries is a sequence number into a per-session ring, not a hash of the rendering, because two `tick` lines a second apart render to identical characters; `vt100` drops OSC 8 from `state_formatted`, so a reattached pane's *existing* lines are inert and new ones are not |
| **the awaiting signal for a *shell* pane — `cide-pty`'s `jobs`, `lifecycle::watch_jobs`** | `check:awaiting`, plus `cargo test -p cide-pty jobs` and `cargo test -p cide-pty -- a_real_shell` — the pane dot, both badges and the window title were never Claude-specific (they key off a session id), so a shell notifies by *emitting the states it never emitted*: `Busy` when its foreground process group has held the terminal for the job threshold (`terminal.jobNotifyAfterSecs`, two minutes by default — `settings_set` retunes running watches, so it is live), `AwaitingInput` when the shell gets it back. `tcgetpgrp` on the pty master, polled on the coalescer's own tick — from the **output** arm as well as the idle one, or a job that starts and ends inside one noisy build is never seen. The threshold gates the *start* because nothing downstream can un-announce a job (an announced job's `Finished` maps to `AwaitingInput`, which raises the marker unconditionally), so an announced `ls` would notify however fast it ran; a fullscreen TUI is suppressed for its whole life because quitting `vim` is not a job finishing. Never for Claude — hooks are exact and know about permission prompts, and every tool call the CLI forks takes a pgid of its own |
| **the session sink — `panes/sessionSink.ts`, `attachModel.ts`, or when a pane attaches/detaches** | `check:attach`, `check:awaiting`, `check:render-stall` — the sink belongs to the pane *host*, not the React mount, and an unmount is usually a *park*: a project switch unmounts every pane of the outgoing project, and a sink detached there loses every byte printed while the user is away (the recovery snapshot was then refused by the hydration gate — the frozen-after-project-switch pane). The sink ends only in `releaseHost`, `teardown` and `forgetSession`, each through `PaneHost.sinkClose`; `check:attach` pins all three plus the snapshot decision (`hydrationPlan`) and the frame-before-snapshot ordering |
| **a pane host's lifecycle, or anything that decides when a terminal paints** | `check:render-stall` — xterm pauses its own renderer when `.xterm-screen` reports non-intersecting and resumes only on the same observer, so a pause with no DOM change on the way out leaves a frozen picture over a live buffer. `layout/paneHosts.ts`'s watchdog is what notices; the guards in `terminal/renderStall.ts` are what keep it from firing on the four states where painting nothing is correct |
| overlays, pickers, search | `check:picker`, `check:search`, `check:problems` |
| terminal file links, `cmd/file.rs`'s refusals | `check:paths`, `check:outside-open` |
| `windows/`, detach and re-dock | `check:detached`, `check:window-controls` |
| **which projects or tabs a window draws — `windows/windowTabs.ts`, `keys/target.ts`'s `windowProjectsOf`, `AppHeader`'s `projects`, `rebuild_windows`** | `check:detached`, `check:commands`, `check:switcher`, plus `cargo test -p cide-core workspace` and `cargo test -p cide-app` — one invariant, *a thing is drawn by exactly one window*, and every way of breaking it is silent. A strip built from `workspace.projects` rather than `role.projects` renders tabs the window does not draw, and they are inert rather than wrong-looking: `activate_project` only moves `active` on shells whose `projects` holds the id, so the click lands in another window. The mirror image is a role with no OS window — `open_project` mints one in `PerProject` mode and only `reconcile` builds it, so a mutation that rebuilds the window map and does not reconcile is a project with a live `claude` and nothing on screen |
| **the macOS dock menu — `cide-app`'s `dock`, `cmd::window::focus_project`** | `cargo test -p cide-app dock` covers the model; the AppKit half cannot be built here at all. Check it by copying the `macos` module into a throwaway crate with the cide-side types stubbed and running `docs/platforms.md`'s Darwin recipe plus `cargo clippy --all-targets` — the recipe is in *Type-checking for macOS from Linux*, and it caught a hard error and two `-D warnings` clippy lints the first time |
| `cide-lang`, `cide-lsp`, symbol navigation, diagnostics | `check:outline`, `check:problems`, `check:commands`, `check:keys` |
| **code completion — `editor/completion*.ts`, `editor/lspSnippet.ts`, `cide_lsp::convert::completion`, the `completion` handshake key** | `check:completion`, `check:editor` (its composed keymap decides whether Tab is free, and a new keymap missing from that array makes the answer a lie), plus `cargo test -p cide-lsp -- --ignored offers_an_import_edit`. **Do not "simplify" the handshake by dropping `completionItem.resolveSupport`** — it reads like removing an unused option and it is what turns auto-import on: rust-analyzer offers *no* flyimport candidates to a client that cannot resolve `additionalTextEdits` lazily, so the popup keeps working and quietly stops offering anything you have not already imported |
| **the bundled servers — `discover`'s ladder/`BUNDLED` table, the hint rung (`extraPathHints` + `cide_core::node_dirs`), `cide-lsp/src/config.rs` (both lanes: `init_options` and `extra_env`), `toolchain::sibling_binary`, the Builtin/System settings, `packaging/{rust-analyzer,gopls}.lock`** | `cargo test -p cide-lsp` (ladder + config + session + env lane), `cargo test -p cide-core` (sibling lookup, cache dir, node dirs), `cargo test -p xtask` (lock formats, fork/gopls steps, go verdict, manifest), `check:menus`; the real fallback path only under `cargo test -p cide-lsp -- --ignored a_broken_first_candidate`, and a real npm server under `cargo test -p cide-lsp --test real_npm_server -- --ignored`. The `initializationOptions` keys are a **two-repo contract** with `../forks/rust-analyzer`'s config module — one producer each side (`cide_lsp::config::rust_analyzer_options`), and a rename on either side degrades silently to defaults, so move both in one review; `extra_env`'s `GOPLSCACHE` is the same discipline against upstream gopls's own variable (ADR 0011). The hint rung is how an npm-installed server (an extension's `typescript-language-server`) is found from a desktop launch: after PATH miss the manifest's `extraPathHints` and the Node version-manager dirs are searched, and a hit's directory — plus `node`'s — rides `Candidate::child_path_dirs` into `prepare_command_with`, because such a server is a `#!/usr/bin/env node` script whose shebang otherwise dies with `env: node: No such file or directory`. A multi-language server (`vscode-css-language-server`, `["css","scss","less"]`) labels each didOpen with the *document's* language via `Server::language_id_for` — the first-declared id would make it parse SCSS as CSS and report every nested rule as an error |
| `ext/`, `sidebar/ExtensionsPanel/`, `cide-ext`, a manifest field | `check:ext`, `check:ext-render`, `check:boundary`, `check:sidebar`, plus `cargo test -p cide-ext` — the manifest refusal table, the path jail, the git route, and, against `../cide-marketplace`, the whole install road. **The `cide-ext://` URL's spelling is a platform split and lives in `ui/src/ext/assetUrl.ts`**, import-free so `check:ext` can drive it with one user agent per platform: Tauri serves a custom scheme as `http://<scheme>.localhost/…` on **Windows and Android** and as `<scheme>://localhost/…` on **macOS, iOS and Linux** (`tauri-2.11.5/scripts/core.js`'s `convertFileSrc` branches on those two and nothing else). cide shipped with macOS on the Windows side, which is invisible from Linux and kills *every* extension with a worker on a Mac — the fetch 404s and a module worker that fails to load fires a message-less `Event`, so the panel can only name the URL. That split is why the identity pair rides in the **path** and not the host (`cide_ext::assets::split_path`) |
| **a `SettingsSection` variant** | `check:ext` — the nav is the Rust enum's order, and a variant costs three things: the variant, a `SECTIONS` entry and a `case` in `renderSection`. The third is the one that gets forgotten, and it used to compile: the switch returns `ReactNode`, `undefined` is one, so the nav row opened a blank page. There is a `never` guard on it now |
| **a `TabKind` variant** | `check:tab-overflow`, `check:tab-drag`, plus `cargo test -p cide-app` — a variant is four arms, and the two that get forgotten are `chrome/TabStrip.tsx`'s (a `never` there is a compile error, which is the point) and `closed_tabs::remembered`'s (a miss there is a tab Ctrl+Shift+T silently will not reopen) |
| **a language table, a fold spec, a scratch row, `cide_ipc::lang::builtins`** | `cargo --locked xtask codegen`, then `check:editor` — `ui/src/editor/builtinLanguages.ts` is **generated** from Rust and `codegen --check` is the gate. One registry feeds builtins and extensions alike, so a table edited in the wrong place is a language an extension can no longer supersede |
| **an icon, anywhere** | `check:ui-icons` — the set is closed in both directions, every mark must be a value an extension could also supply (`is_svg_path`), each size preset's rendered stroke must land in 1.4–1.8px, and **no Unicode symbol may be drawn as an icon** outside a per-file allowlist with a stated reason. Add a mark by editing `ICONS` in `ui/scripts/vendor-ui-icons.mjs` and re-running it — `src/icons/iconPaths.ts` is generated |
| **a `transition`, or a `@media (prefers-reduced-motion)` block** | `check:motion` — `transition: all` is banned as a word, only paint properties may be transitioned, and four files are fenced off entirely: a transition on a splitter's drag path defeats `resizeGesture`'s gesture-end deferral, and one on a pane host interpolates the 1px `bottom` nudge `repaintHost` uses to un-stall a terminal. Neither failure throws or changes a pixel in any snapshot |
| **a radius, a spacing step, an elevation or a duration** | the `--r-*`, `--sp-*`, `--shadow-*` and `--dur-*` families in `tokens.css`. `--shadow-*` carries colour and must be declared in **both** palette blocks; the rest are theme-independent. `check:menus` resolves every token a menu reads against the palette blocks, so a new family needs its prefix in that script's shape rule |
| **added, renamed or moved any file** | `check:casing` — a name differing from a sibling's only in case is one path on macOS, and it cost a Mac build once (see `docs/platforms.md`) |

## Architecture

### The rule that shapes the crate graph

**Only `cide-app` may depend on tauri.** `cide-headless` is the standing proof: it links
`cide-core`, `cide-ipc`, `cide-pty`, `cide-tasks` and `cide-agents` and renders a live session,
a task board and an agent roster to stdout with zero Tauri;
if it stops compiling, domain logic leaked into the app crate. A CI job enumerates every
workspace member with `cargo tree` and fails on `tauri|wry|tao`. Reaching for an `AppHandle`
inside domain logic is the signal that the logic is in the wrong crate.

```
crates/
  cide-app/       the Tauri shell: windows, the command surface, process lifecycle. Glue only.
  cide-ipc/       wire DTOs (serde + ts-rs). Simultaneously in-memory domain, disk format, wire format.
  cide-core/      behaviour over those DTOs, as free functions: workspace tree, keymap, commands,
                  persist, and the VS Code colour-theme importer (`scheme`).
  cide-pty/       PTY sessions: spawn, coalescing, backpressure, vt100 mirror.
  cide-claude/    spawning and supervising `claude`: env, hooks, resume/fork, headless one-shots.
  cide-ide-mcp/   the Claude Code IDE-integration MCP server (openDiff, getDiagnostics, openFile).
  cide-git/       multi-root git, hunk/line staging, changelists, shelf; and the read-only
                  half — log, graph lanes, file history, blame — plus the commit actions,
                  pull's merge/rebase, and the conflict surface (ADR 0009).
  cide-fs/        gitignore-aware indexing and watching.   cide-search/  fuzzy + content search.
  cide-tasks/     `.cide/tasks.json`: one owning store, a repairing loader, a stale-file merge.
  cide-spec/      OpenSpec, by running its CLI — never by parsing its markdown (ADR 0012).
  cide-agents/    `.cide/` roles and config, and the `cide_task_*` MCP vocabulary. Spawns nothing yet.
  cide-lang/      tree-sitter over Rust and Go: what a file declares. No tauri, no cide-fs.
  cide-lsp/       an LSP *client*. Threads, not tokio (see its lib.rs). Since M22 the server set
                  is a registry, not an enum: builtins plus whatever an extension declares.
  cide-ext/       extensions and the git repositories they come from: manifests, marketplaces,
                  installs, and the merge that decides which contribution wins. Runs no
                  extension code — that is a Worker in the webview (ADR 0010).
  cide-hook/      second binary: bridges a Claude hook to the running IDE over a unix socket.
  cide-headless/  third binary: proves the core links without tauri
                  (`cide-headless tree|commands|keymap|tasks|agents`).
```

`docs/adr/` records the decisions that a refactor would otherwise undo — read 0001 (no
multiwebview), 0002 (Rust owns state), 0003 (xterm owns VT) before changing anything
structural, 0009 (real sequencer state) before touching how a conflict is landed (it reverses an
argument still written out at length in `cide-git/src/replay.rs`), and 0010 (extensions out of
realm) before touching anything under `ui/src/ext/`: it is why a contributed panel is a *view
model* rather than a React component, and why a capability check that moved into the worker would
be a check the extension could delete.

### The state loop

Rust owns everything durable. A gesture goes:

```
ui/src/ipc/client.ts → #[tauri::command] in crates/cide-app/src/cmd/*
                     → validated mutation in cide-core::workspace (bumps `rev`)
                     → emit::workspace_changed → EVERY window
                     → ui/src/store/workspace.ts (a mirror; drops any snapshot with a stale rev)
```

`ui/src/store/workspace.ts` never edits the tree — two windows editing one tree would give two
answers, and a detached pane is a separate JavaScript realm with no shared memory. The webview
owns exactly two things: transient gesture state (which overlay is open, whether a splitter is
mid-drag) and live DOM instances (xterm `Terminal`, CodeMirror `EditorView`) that cannot be
serialised. Splitter drags write `gridTemplateColumns` directly and commit `pane_set_ratio`
once, on `pointerup`, because every mutation costs a round trip.

**Sessions live outside the tree**, in a process-global registry keyed by `SessionId`. A pane
holds an id and merely *attaches*; closing a pane, tab or window never touches the child.
`Session.sinks` is a list, not an `Option`, so a detach is gapless — the new window's sink is
live before the old one drops. That one decision is what makes detach-into-a-window,
survive-a-window-close, and the stacked-vs-per-project window setting the same mechanism.

### The wire contract has three gates

1. DTOs are `#[derive(TS)]` types in `crates/cide-ipc`. `cargo test -p cide-ipc` writes
   `crates/cide-ipc/bindings/*.ts`; `cargo xtask codegen` concatenates them into
   **`ui/src/ipc/generated.ts`, which is generated — never hand-edit it.** `codegen --check`
   fails the build when a Rust field rename never reached TypeScript.
2. Adding or removing a `#[tauri::command]` (registered in `crates/cide-app/src/lib.rs`'s
   `generate_handler!`) or a `cide://` event (all of which go out through
   `crates/cide-app/src/emit.rs` and nowhere else) drifts `contract/{commands,events}.json`.
   Accept it with `cargo xtask contract-check --write`, and move `ui/src/ipc/client.ts` with it.
3. `ui/src/ipc/client.ts` is the frontend's only seam to `invoke`/`listen`; that is what keeps
   the surface greppable. `ui/src/chrome/WindowFrame.tsx` is the one documented exception, for
   window controls, which are not commands.

`cargo xtask codegen` writes a **second** generated file since M22:
**`ui/src/editor/builtinLanguages.ts`**, from `cide_ipc::lang::builtins()`. A language's *routing*
— which extensions it claims, what the status bar calls it, how it folds, whether the scratch
picker offers it — has one home, in Rust, because the same table is merged with what an extension
contributes and two copies would drift. What is *not* there is the tokenizer: a builtin's grammar
contains a function (Rust's lifetimes, Markdown's headings) and stays in
`ui/src/editor/languages/<id>.ts`. A contributed one carries `rules` instead — see
`cide_ipc::lang::GrammarRule`.

### Commands and keys are one registry

`cide-core::commands` is the single table behind both the palette and the keymap. Two tables
would let a command be bindable but unlistable, and both drift silently.

- **Ids are API.** A user's `keymap.json` names them. Add freely, never rename.
- Every id is either handled by a `case` in `ui/src/keys/dispatch.ts` or carries an
  `unavailable` reason. Listed-and-silently-inert is the state `check:commands` makes
  unrepresentable — it once described 24 of 37 commands.
- A `when` clause may only name a flag in `CONTEXT_FLAGS`, and every flag must actually be
  supplied by the webview; a flag nobody sets is false for ever and the command silently
  vanishes from the palette.
- Key resolution is layered in Rust (defaults → platform → user `keymap.json`) and normalised;
  `ui/src/keys/keymap.ts` only indexes what it was handed.

### Pane hosts: the load-bearing thirty lines

`ui/src/layout/paneHosts.ts` keeps terminal DOM in a module-level map outside React; `PaneSlot`
merely `appendChild`s it. A destroyed node loses scrollback, selection, renderer and in-flight
turn, and xterm has no restore that survives it. The rules, each a bug if broken: hidden tabs
use `visibility: hidden`, never `display: none` (measurements read zero and `fit()` corrupts
the child's terminal size); never conditionally render a pane or `replaceChildren` a slot;
`term.open()` runs exactly once per pane; `releaseHost` is for a pane leaving this window with
its session alive, `destroyHost` for one that is finished. Only `CIDE_AUDIT_PANES=1` can catch
a regression here.

### PTY and Claude hosting

`cide-pty` owns explicit threads, not tokio (portable-pty's handles are blocking): reader
thread → bounded channel (cap 16) → coalescer → vt100 mirror + sinks. Backpressure is
structural — a full channel blocks the reader, then the kernel buffer, then the child.
Coalescing is a correctness requirement, not tuning: Tauri routes `Channel` payloads under
1024 bytes through `webview.eval` as a JSON array of decimal numbers on the GTK main loop.

Claude is hosted three ways at once (ADR 0005): a real PTY, the IDE-integration MCP server
(one per project, advertised via `~/.claude/ide/<port>.lock`), and hooks through the
`cide-hook` binary. Three invariants:

- **`openDiff` blocks the agent's turn.** Every early return in `cide-app/src/ide.rs::pump`
  must cancel the request first, or the `claude` that asked waits for ever with nothing on
  screen.
- **Never read `~/.claude/.credentials.json`, never inject `ANTHROPIC_API_KEY`** — it outranks
  subscription OAuth and would silently bill a Console org. Children inherit auth from the
  environment.
- **No child inherits the bundle's environment, and every child is armed.** Both rules live in
  `cide-core::child_env`, which is the module every spawn site passes through.
  `prepare_command` (ADR 0007, and since M17 one pass more): an AppImage's `AppRun` leaves
  `PYTHONHOME` and `LD_LIBRARY_PATH` pointing inside a mount, and a `claude` that inherits them
  cannot start a single stdio MCP server — it reports `CONNECTION_CLOSED` from three processes
  below anything cide logs. It also *appends* `cide-core::toolchain::extra_dirs` to the child's
  `PATH`, because a child that inherits a GUI launch's `PATH` cannot find its own toolchain —
  which is what made `gopls` answer `no views` on a Mac where cide had found and started it.
  `arm` (ADR 0008, moved here from `cide-claude` in M12) hands the kernel a pid to kill when cide
  dies, and its contract is that **the forking thread must outlive the child** — which is what
  `on_spawn_thread` is for, and why a language server is never spawned from a Tauri command
  worker. A new `Command::new`/`SpawnSpec` anywhere in the workspace needs both lines.

A `SessionId` *is* the value passed to `claude --session-id`, which is what makes resume free.
Shutdown is a ladder — SIGHUP, SIGTERM, SIGKILL — so a `claude` finishes writing the transcript
that resume depends on; signals arrive through a self-pipe because taking the workspace lock in
a handler deadlocks whenever the interrupted thread already held it.

### On disk

Under a profile every path below has `cide-<profile>` where it says `cide` — one leaf, decided
in `cide_core::persist::xdg_dir` and nowhere else, so an instance's whole footprint moves or
none of it does. Inspect a profile's with `CIDE_PROFILE=<name> ./target/debug/cide-headless tree`.

- `$XDG_STATE_HOME/cide/workspace.json` — the tree. Written atomically, 500 ms debounce; reads
  never fail, because a broken layout must not become a launch loop. Inspect without a GUI:
  `./target/debug/cide-headless tree`.
- `$XDG_CONFIG_HOME/cide/keymap.json` — user binding overrides only (diffs; defaults are
  compiled in).
- `$XDG_CONFIG_HOME/cide/schemes/<id>.json` — imported editor colour schemes, one file each.
  The *converted* scheme, not the source VS Code theme: re-converting at launch would let a
  change to `cide_core::scheme::SCOPES` repaint a buffer somebody was happy with. `cide` is
  not a file — it is what `tokens.css` declares, so selecting it clears the properties.

## Conventions

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
