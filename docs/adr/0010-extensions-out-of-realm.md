# ADR 0010 — Extension code runs out of the window's realm; panels are data

**Status:** accepted (M22)
**Date:** 2026-08-21

## Context

M22 makes four things contributable from outside the tree: languages, language servers, UI
panels, and logic over open files. The first two are data and were never in question. The other
two are code, and the question is *where that code runs*.

Three arrangements were considered, and the first is the one every comparable product has
chosen.

**Trusted ES module in the main realm.** cide dynamically imports an extension's module into the
window and hands it React, the DOM and a registration API. It is what VS Code's renderer-side
contributions amount to, it is the least code to write, and an extension can draw anything.

It loses on ADR 0001. There is one webview per OS window and **one JavaScript main thread serving
every pane**; PTY bytes are coalesced in Rust to ≥8 KiB or 8 ms specifically so that thread stays
free, and the WebGL pool is capped because WebKitGTK allows roughly 8–16 contexts. Extension code
on that thread competes with xterm for it, and the failure is not a slow panel — it is a terminal
that stutters while somebody else's `for` loop runs.

It loses a second time on the seam. `ui/src/ipc/client.ts` is the only file permitted to import
`@tauri-apps/api`, which is what keeps the command surface greppable and lets
`cargo xtask contract-check` reason about it. An extension in the same realm can reach
`window.__TAURI__` directly, so "granted capabilities" would be a convention rather than a
mechanism — a check the extension could delete.

**An iframe per panel.** Isolated, and an extension can draw anything. It is a second document per
panel, which is the direction ADR 0001 rejected for panes and rejects here for the same reasons;
and the theme would have to be re-established by hand inside every frame, so `check:theme` and
`check:ui-scale` would stop covering the panels most likely to get it wrong.

**An out-of-process host**, one child per extension speaking JSON-RPC over stdio, the way
`cide-lsp` and `cide-ide-mcp` already work. Genuinely isolated, any implementation language, and
it reuses `child_env::prepare_command` and `arm`. It costs a process per extension and — the
deciding objection — it makes an extension a *binary or a script with a runtime*, which turns
"clone a repository and press Install" into a build step. That is the wrong price for a syntax
definition and a list of statements.

## Decision

**Extension code runs in a dedicated Web Worker per extension, and a panel is a view model that
cide renders.**

- The worker has no DOM, no `@tauri-apps/api` and no `invoke` — not hidden, genuinely absent,
  because a worker is a different realm. Everything it can do goes through a fixed table of
  requests in `ui/src/ext/protocol.ts`.
- Every request is checked against the extension's granted capabilities in `ui/src/ext/host.ts`,
  **on the main thread**. That is the whole design in one sentence: a capability check inside the
  worker is a check the extension could delete.
- A panel is a `PanelView` — a tree, a list, a table, some prose — drawn by
  `ui/src/ext/ExtPanelView.tsx` with cide's own components and tokens.
- The worker's module is fetched over a custom `cide-ext://` scheme handled in
  `cide-app/src/ext_assets.rs`, path-jailed by `cide_ext::assets` and refused outright for a
  disabled extension.
- Marketplaces are git repositories, reached by forking the user's own `git` —
  `cide-git/src/push.rs`'s `Route` policy, inherited whole, including the reason: libgit2 cannot
  drive an interactive credential helper.
- Installed code is **copied** out of the clone at a pinned commit, never symlinked or run in
  place.

## Consequences

- The main thread stays free, which is what ADR 0001 spends its whole length protecting.
- A contributed panel is correct in both palettes and at every UI font size for free, because the
  pixels are drawn by components `check:theme` and `check:ui-scale` already cover.
- A broken extension costs its own panels. It cannot unmount the React root: there is no
  extension code on the main thread, every message is validated by `isWorkerNote` before it is
  destructured, and nothing in the host awaits a worker — so a hung extension cannot make cide
  wait either.
- **An extension cannot draw something cide has no component for.** A chart, a canvas, a custom
  gesture: not possible, and not by omission. The answer to a genuinely missing panel kind is to
  add it to `viewModel.ts`, where it gets a theme, a check and a keyboard story — not to open a
  hole for arbitrary markup.
- `connect-src` in `tauri.conf.json` deliberately does **not** include `cide-ext:`. A worker may
  `import` modules from its own installed directory, because that is `script-src`/`worker-src`,
  and it may not `fetch` anything at all. An extension that needs data ships it as a module.
- A `git pull` on a marketplace cannot change code under a running app, and an update is a gesture
  with a version behind it. The cost is disk: an installed extension exists twice, once in the
  clone and once in the install.
- Capability grants live in `$XDG_CONFIG_HOME/cide/extensions.json` as the list the user was
  *shown*, and an install whose manifest asks for more than that list is refused rather than
  upgraded. Without it a marketplace could add `process:spawn` in a commit and every machine that
  ran a refresh would grant it silently.
- Contributed *bottom* tabs are not persisted in `workspace.json`, unlike the Log and History
  tabs beside them. There is nothing durable in one — its content is rebuilt by its worker on
  every launch — so a stored `active` would restore a tab whose extension may since have been
  uninstalled, and go on restoring it.
