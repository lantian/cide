# ADR 0006 — Linux window creation and the graphics ladder

**Status:** accepted (M0)
**Date:** 2026-08-07

## Context

Two independent failures during M0 each made the app *look* like it was working while it
was not. Both are Linux/Wayland-specific, both fail silently, and neither is documented in
Tauri's own guidance. They are recorded here because the fixes are one-liners that a future
refactor would plausibly "clean up".

Reference environment: openSUSE Tumbleweed, KDE Plasma on Wayland, WebKitGTK 2.52.3,
GTK 3.24.52, Tauri 2.11.5.

## Finding 1 — the DMABUF renderer makes the app unlaunchable

Stock configuration exits immediately with:

```
Gdk-Message: Error 71 (Protocol error) dispatching to Wayland display.
```

Measured across combinations:

| environment                          | result                                        |
|--------------------------------------|-----------------------------------------------|
| default (Wayland)                    | `Error 71`, process exits                     |
| `GDK_BACKEND=x11`                    | runs, spams `Failed to create GBM buffer of size 1440x900` |
| `WEBKIT_DISABLE_DMABUF_RENDERER=1`   | **runs cleanly on native Wayland**            |
| `WEBKIT_DISABLE_COMPOSITING_MODE=1`  | runs, but accelerated compositing is off      |

### Decision

`crates/cide-app/src/graphics.rs` applies `WEBKIT_DISABLE_DMABUF_RENDERER=1` and
`__NV_DISABLE_EXPLICIT_SYNC=1` **at the top of `main`**, before anything touches GTK. These
variables are read when the webview is created; setting them later does nothing at all.

Both respect a value already present in the environment, so a user who knows their hardware
is never overridden, and `CIDE_NO_GRAPHICS_WORKAROUNDS=1` disables the lot for bisecting.

`WEBKIT_DISABLE_COMPOSITING_MODE` is deliberately *not* applied by default: it also fixes
the crash, but it disables accelerated compositing for the entire webview, which a
four-terminal grid feels immediately. It belongs in Settings for the machines rung 1 does
not rescue (M11).

## Finding 2 — `visible(false)` never gets a size on Wayland

The conventional pattern is to build a window hidden and reveal it on first paint, so the
user never sees a white flash before a dark theme lands. On Wayland this is broken: a
surface that is never *mapped* is never *configured*, so it has no size. The window
reported

```
visible=false outer=0x0
```

indefinitely, and `show()` from the reveal path did not rescue it.

The failure is nasty because JavaScript runs perfectly well in an unmapped webview. The IPC
benchmark completed, commands round-tripped, sessions spawned — everything looked healthy
while every PTY silently sat at its fallback 80x24 because there was no layout to measure.

### Decision

Windows are built `.visible(true)`. The flash is avoided instead by painting `--bg` from
the very first frame: `html`/`body` carry the background in `tokens.css`, so it is applied
by the stylesheet rather than by a React render.

Related: there is no `.center()` in the builder. Wayland does not let a client position
itself, so asking is at best ignored.

## Finding 3 — a shell-launched window will not come to the front

KDE's focus-stealing prevention means a window launched from a terminal stays behind the
user's other windows even with `set_focus()`. This is correct behaviour, and it makes any
screenshot-based verification unautomatable by default.

### Decision

`CIDE_ON_TOP=1` builds the window `always_on_top`. This is not a debugging hack: M3's
acceptance criterion is a screenshot diff against the design mock at 1440x900, and that
test needs a deterministic way to put the window where a capture tool can see it.

Similarly, `CIDE_BENCH=1` runs the IPC benchmark on first paint, prints the report to
stdout and exits — so the M0 gate is a scriptable command rather than a button a human has
to find and click.

## Consequences

- Three environment variables (`CIDE_NO_GRAPHICS_WORKAROUNDS`, `CIDE_ON_TOP`, `CIDE_BENCH`)
  are part of the app's contract and are documented in the README.
- The graphics ladder must surface in Settings → Appearance (M11) with each rung labelled
  by its cost, because this heuristic will be wrong on some machines.
- Anyone porting to macOS or Windows should revisit `visible(true)`: the hidden-then-reveal
  pattern is the right one there, so this needs to become platform-conditional rather than
  unconditional.
