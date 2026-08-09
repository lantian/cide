# ADR 0001 — One webview per OS window; panes are DOM

**Status:** accepted (M0)
**Date:** 2026-08-07

## Context

The design mock's project console is a 2×2 grid of panes — a live Claude session, a second
idle Claude session, a shell, a read-only diff — separated by 6px splitters, each pane in a
`--panel` box with a 26px title bar, the focused one outlined `1px solid var(--accent-dim)`.

Tauri 2 has an `unstable`-gated multiwebview API. On paper it is the obvious implementation:
one `Webview` per pane, `set_bounds` on drag, and each pane gets an isolated JavaScript
realm. It would also have made "detach a pane into its own window" nearly free.

It does not work on this platform, and the way it fails is the problem.

`tauri-runtime-wry` packs child webviews into the window's `GtkBox`. wry only honours
`set_bounds` for `GtkFixed` parents or X11 child windows. Under Wayland with a `GtkBox`
parent, a `set_bounds` call **returns `Ok(())` and does nothing**: splitter drags would be
no-ops that report success, and the panes would stack vertically in creation order. wry's own
documentation marks child webviews "Linux (X11 only) … won't work on Wayland". The
development and target platform is Wayland on KDE.

A second consideration, independent of the bug: `unstable` features are excluded from Tauri's
semver guarantees, so the pane grid — the application's headline surface — would sit on an
API with no compatibility promise.

## Decision

The pane grid is **CSS grid and DOM inside a single webview per OS window**. `LayoutNode` is a
binary split tree in `cide-core`; `ui/src/layout/SplitTree.tsx` renders it as nested CSS grids
and `Splitter.tsx` writes `gridTemplateColumns` directly to the DOM during a drag, committing
`pane.set_ratio` only on `pointerup`.

Tauri's multiwebview API is not enabled, and `wry`/`tao` are not direct dependencies of this
workspace — Tauri 2.11.5 pins `wry ^0.55` / `tao ^0.35` and, because those are 0.x, a direct
dependency would silently fork the graph into two incompatible webview stacks.

Detaching a pane is therefore a *new OS window*, not a reparented webview, and continuity
comes from the session registry rather than from the DOM (see ADR 0002).

## Consequences

- Pixel-exact 6px splitters, the `--accent-dim` focus ring and one shared tab strip are
  trivial, because they are one document. The multiwebview version of the focus ring would
  have had to be drawn by the host window underneath the children.
- One JavaScript main thread serves every pane in a window. This is the real cost, and it is
  why PTY bytes never enter React, why terminal output is coalesced to ≥8 KiB or 8 ms in
  Rust, and why the WebGL pool is capped (WebKitGTK caps concurrent contexts at roughly
  8–16, and `@xterm/addon-canvas` was removed in xterm 6, so the DOM renderer is the only
  fallback).
- Panes must never be unmounted, because a terminal is a live DOM node with scrollback and an
  alt-screen state that no store holds. `ui/src/layout/paneHosts.ts` keeps hosts at module
  scope and `PaneSlot` *moves* them between slots; inactive tabs use `visibility: hidden`,
  never `display: none`, which zeroes every measurement and makes `fitAddon.fit()` compute
  garbage.
- A detached pane is a separate JavaScript realm with no shared memory, so state that has to
  survive detaching cannot live in the webview. That constraint is ADR 0002.
