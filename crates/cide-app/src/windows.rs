//! Window creation.
//!
//! `tauri.conf.json` declares **zero** windows on purpose: which windows exist is a
//! function of the workspace and the `windowMode` setting, so it is decided in Rust at
//! runtime rather than baked into config.
//!
//! Every window loads the same `index.html` and reads its role from `?window=<label>`.
//! There is exactly one webview per OS window — see `docs/adr/0001-no-multiwebview.md`.

use cide_ipc::WindowLabel;
use tauri::{AppHandle, LogicalSize, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// Logical size of the design mock. Used as the first-run default so the app opens at the
/// geometry the screenshot comparison in M3 is specified against.
pub const DEFAULT_WIDTH: f64 = 1440.0;
pub const DEFAULT_HEIGHT: f64 = 900.0;

/// Default size of a window holding one torn-out pane, when the caller cannot say how big
/// the pane was. Smaller than a shell because such a window carries no rail, no sidebar and
/// no tab strip — one pane and the bar it is dragged by.
pub const DETACHED_WIDTH: f64 = 900.0;
pub const DETACHED_HEIGHT: f64 = 640.0;

/// Smallest size at which the chrome still lays out: the 42px rail plus a 252px sidebar
/// plus a usable content column, and enough height for header + tab strip + status bar.
pub const MIN_WIDTH: f64 = 720.0;
pub const MIN_HEIGHT: f64 = 420.0;

pub fn create_shell(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let label = WindowLabel::shell();
    create(app, &label, "cide", None)
}

/// Build the window the workspace already calls `label`.
///
/// The label is an argument rather than something minted here because `cide-core` names a
/// window before one exists — the shell on first launch, the target of a detach, every
/// shell a mode flip asks for. A window built under a second, freshly minted label is one
/// no lookup finds: `app_get_bootstrap` falls back to an empty shell and the window paints
/// nothing.
///
/// `size` is a logical inner size to force after the build. `None` leaves whatever
/// `tauri-plugin-window-state` restored in place, which is what a shell wants; a detached
/// pane passes its measured rect, because that plugin keys every `pane:<uuid>` window on
/// the one `pane` prefix and would otherwise give them all the size of the last one closed.
pub fn create(
    app: &AppHandle,
    label: &WindowLabel,
    title: &str,
    size: Option<(f64, f64)>,
) -> tauri::Result<WebviewWindow> {
    // `CIDE_BENCH=1` makes the window run the IPC benchmark on first paint and print the
    // report to stdout. The M0 GO/NO-GO gate has to be runnable from a script and from CI,
    // not only by clicking a button — and on a compositor with focus-stealing prevention,
    // a shell-launched window may never come to the front to be clicked at all.
    let bench = if std::env::var_os("CIDE_BENCH").is_some() {
        "&bench=1"
    } else {
        ""
    };
    // `CIDE_AUDIT=1` measures the chrome against the design mock's stated dimensions and
    // prints the table to stdout. M3's acceptance criterion is a screenshot diff, and a
    // screenshot cannot be diffed here — the mock needs a runtime we do not have, and KDE
    // will not raise a shell-launched window for a capture tool. Measuring the live DOM is
    // the half of that check which can be made to answer for itself.
    let audit = if std::env::var_os("CIDE_AUDIT").is_some() {
        "&audit=1"
    } else {
        ""
    };
    // `CIDE_AUDIT_PANES=1` runs the pane lifecycle audit: 100 split/close/maximize/switch
    // cycles asserting `term.open()` happened exactly once per pane. That is M4's stated
    // verification, and it only means anything driven against real Rust-owned state.
    let panes = if std::env::var_os("CIDE_AUDIT_PANES").is_some() {
        "&panes=1"
    } else {
        ""
    };
    // `CIDE_AUDIT_WINDOWS=1` runs M5's acceptance check: detach and re-dock round trips and
    // window-mode flips, asserting no session is lost to any of them.
    let wins = if std::env::var_os("CIDE_AUDIT_WINDOWS").is_some() {
        "&windows=1"
    } else {
        ""
    };
    let url = WebviewUrl::App(
        format!(
            "index.html?window={}{bench}{audit}{panes}{wins}",
            label.as_str()
        )
        .into(),
    );

    // The milestone states the comparison at 1440x900, and `tauri-plugin-window-state`
    // restores whatever size the window was last left at — so an audit run would silently
    // measure some other viewport and still report PASS.
    //
    // Clamped to the minimum here as well as by `min_inner_size`: a caller reporting the
    // rect of a pane in a narrow split would otherwise ask for a window the chrome cannot
    // lay out, and the correction would arrive as a resize the user watches happen.
    let forced = size
        .map(|(w, h)| (w.max(MIN_WIDTH), h.max(MIN_HEIGHT)))
        .or_else(|| {
            std::env::var_os("CIDE_AUDIT")
                .is_some()
                .then_some((DEFAULT_WIDTH, DEFAULT_HEIGHT))
        });
    let (initial_width, initial_height) = forced.unwrap_or((DEFAULT_WIDTH, DEFAULT_HEIGHT));

    let window = WebviewWindowBuilder::new(app, label.as_str(), url)
        .title(title)
        // We draw the titlebar ourselves — traffic lights, project tabs and the icon
        // cluster all live in one 34px bar. The cost is that the window manager no longer
        // provides resize borders, snap or double-click-to-maximize, so the frontend
        // implements eight grips and `core:window:allow-start-resize-dragging` is granted.
        .decorations(false)
        .inner_size(initial_width, initial_height)
        .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
        // Not transparent: on Linux `transparent(true)` needs a running compositor and
        // renders opaque black or garbage without one. The frontend paints --bg #111113.
        .transparent(false)
        // Mapped immediately rather than hidden-then-shown on first paint.
        //
        // The hidden-then-reveal trick avoids a white flash before the dark theme lands,
        // and it is the right pattern on Windows and macOS. On Wayland it is actively
        // broken: a surface that is never mapped is never *configured*, so it has no size
        // — the window reported `visible=false outer=0x0` indefinitely, and because JS
        // still runs in an unmapped webview the app looked alive while every PTY sat at
        // the fallback 80x24 with no layout to measure.
        //
        // The flash is instead avoided by painting `--bg` from the very first frame; the
        // background is set on `html`/`body` in tokens.css, not by a React render.
        .visible(true)
        // `CIDE_ON_TOP=1` keeps the window above others so a screenshot tool can see it.
        // Not a debugging hack: M3's acceptance test is a screenshot diff against the
        // design mock at 1440x900, and KDE's focus-stealing prevention means a
        // shell-launched window does not reliably come to the front on its own.
        //
        // Note there is no `.center()` — Wayland does not let a client position itself,
        // and asking for it is at best ignored.
        .always_on_top(std::env::var_os("CIDE_ON_TOP").is_some())
        .build()?;

    if let Some((width, height)) = forced {
        // After build, so it wins over any geometry the window-state plugin restored.
        let _ = window.set_size(LogicalSize::new(width, height));
    }

    Ok(window)
}

/// Destroy the OS window called `label`, if it exists.
///
/// `destroy` rather than `close`: `close` emits `CloseRequested`, which the app answers by
/// running the very command that got here. A window closed as part of re-docking a pane
/// would ask to re-dock it again, against a workspace that no longer holds it.
///
/// A label with no window is not an error. The workspace is the record of what should
/// exist, and reconciling it against a window that has already gone is the normal case
/// after a native close.
pub fn destroy(app: &AppHandle, label: &WindowLabel) {
    let Some(window) = app.get_webview_window(label.as_str()) else {
        return;
    };
    if let Err(error) = window.destroy() {
        tracing::error!(%label, %error, "could not destroy a window");
    }
}

/// Reveal a window after its frontend reports first paint.
pub fn reveal(app: &AppHandle, label: &str) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.show();
        let _ = w.set_focus();
    }
}
