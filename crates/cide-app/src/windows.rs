//! Window creation.
//!
//! `tauri.conf.json` declares **zero** windows on purpose: which windows exist is a
//! function of the workspace and the `windowMode` setting, so it is decided in Rust at
//! runtime rather than baked into config.
//!
//! Every window loads the same `index.html` and reads its role from `?window=<label>`.
//! There is exactly one webview per OS window — see `docs/adr/0001-no-multiwebview.md`.

use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

use cide_ipc::{SessionId, Theme, WindowLabel};
use tauri::window::Color;
use tauri::{AppHandle, LogicalSize, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

// Gated to match its only consumer, exactly. The sole use of `emit` in this module is
// `emit::mouse_nav` inside `install_mouse_nav`, which is `cfg`-ed to the five targets that have a
// GTK window to hang a button handler on; the two other occurrences of the name are string
// literals inside a source assertion, which do not count as a use. Leaving the import
// unconditional made `clippy --all-targets -D warnings` fail on macOS with `unused import` — the
// fourth step of the advisory macOS CI job — and it is what the user's first build reported. The
// same fix, for the same reason, is already applied to `use crate::srcgrep::without_comments` in
// the test module below; the precedent is deliberate and this is its second instance.
#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
use crate::emit;
use crate::workspace_state::WorkspaceState;

/// `--bg` for each theme, as `tokens.css` defines it.
///
/// Duplicated from CSS on purpose, and the only duplication of a token value in Rust: the
/// native window is painted by the toolkit *before* the webview has parsed a stylesheet, so
/// this colour cannot be read from the place that owns it. Wrong values here cost one frame
/// of the wrong colour at launch, which is precisely the bug this is here to fix — keep
/// them in step with the `--bg` of each theme block.
const BG_LIGHT: Color = Color(0xfb, 0xfb, 0xfc, 0xff);
const BG_DARK: Color = Color(0x11, 0x11, 0x13, 0xff);

/// The theme the next window should open in.
///
/// Read from the saved workspace rather than passed in, because every caller — startup,
/// restore, a detach, a window-mode flip — wants the same answer and none of them has a
/// reason to hold it. `try_state` because `create` is reachable from `setup` before the
/// managed state is registered in some orders; the default is what a fresh install gets.
///
/// It takes the workspace lock, which is `parking_lot` and therefore not reentrant, so
/// [`create`] must not be called from inside a `WorkspaceState::update`/`with` closure.
/// Today's callers all commit the mutation first and open the window after — `detach_pane`
/// deliberately so, since it rolls the workspace back if the window fails.
fn theme(app: &AppHandle) -> Theme {
    app.try_state::<WorkspaceState>()
        .map(|state| state.with(|ws| ws.settings.theme))
        .unwrap_or_default()
}

/// The saved chrome font size, for the same first-frame reason [`theme`] is read here.
///
/// Already inside the band on the way out — `apply_patch` clamps on write — but clamped again
/// rather than trusted, because this one is read from state that a `workspace.json` a user
/// edited by hand can reach, and it becomes a divisor in `theme-boot.js`.
fn ui_font_size(app: &AppHandle) -> f32 {
    let stored = app
        .try_state::<WorkspaceState>()
        .map(|state| state.with(|ws| ws.settings.ui_font_size))
        .unwrap_or(cide_ipc::DEFAULT_UI_FONT_SIZE);
    cide_ipc::clamp_ui_font_size(stored)
}

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
    // `CIDE_INPUT_PROBE=1` traces every keyboard emitter into the log — see
    // `ui/src/terminal/inputHost.ts`, which is the only way to tell which of xterm's two
    // input emitters an input method is firing.
    //
    // **Opt-in rather than on for every dev build, and that is a performance decision.** The
    // probe writes one `diag_log` per keydown, keyup, `input` and `onData`; `diag_log` is a
    // synchronous command, so each line is a main-thread IPC round trip and a file append.
    // Four of those per character, on the thread that receives the keystrokes, in exactly the
    // configuration `run.sh` launches — a diagnostic for a lag report that would itself be a
    // cause of lag. It is worth having and it is not worth having by default.
    let input_probe = if std::env::var_os("CIDE_INPUT_PROBE").is_some() {
        "&inputprobe=1"
    } else {
        ""
    };
    // `CIDE_RENDERER=webgl` opts back in to the pooled WebGL renderer. **The DOM renderer is
    // the default, and that is a measurement rather than a preference.**
    //
    // `@xterm/addon-webgl` 0.19.0 sets `TextureAtlas._requestClearModel` to `true` in two
    // places — an atlas page merge, and an oversized-glyph overflow — and **never sets it back
    // to `false` anywhere**; `clearTexture()` does not reset it and `beginFrame()` reads it
    // without consuming it. `WebglRenderer.ts:346` branches on that read, so from the first
    // page merge onward every frame runs `_clearModel(true)` and rebuilds all rows instead of
    // the changed ones, permanently, for the life of that terminal. Any session long enough to
    // fill one atlas page trips it, and nothing untrips it.
    //
    // Confirmed against the running app, not inferred: typing was reported as "very very slow",
    // and with `CIDE_RENDERER=dom` the same build was reported fast. WebKitGTK is what makes it
    // undiagnosable from inside — `WEBGL_debug_renderer_info` is masked, so a software
    // rasterizer is indistinguishable from a GPU, and a full per-frame texture upload on a
    // software path is precisely that symptom.
    //
    // What the default costs: WebGL's advantage is throughput on a flood of output, which is
    // M2's `cat` a 1 GiB file criterion. That number was measured on the WebGL path and has not
    // been re-measured on this one. Latency on every keystroke of every session is worth more
    // than peak throughput on a case that is rare and already flow-controlled, so the default
    // goes to the renderer that is correct all the time — and the flag is how the other is
    // measured rather than argued about.
    let renderer = if std::env::var("CIDE_RENDERER").is_ok_and(|v| v == "webgl") {
        "&renderer=webgl"
    } else {
        ""
    };
    // `CIDE_CONSOLE_BRIDGE=1` forwards the webview's `console.error`/`warn` into the Rust log.
    //
    // There is otherwise no way to read this app's console: a WebKitGTK webview on Wayland has
    // no terminal attached, and opening devtools needs a GUI window on the machine being
    // debugged. Opt-in rather than always on for the reason written on the input probe above —
    // `diag_log` is synchronous, so each forwarded line is a main-thread round trip.
    let console_bridge = if std::env::var_os("CIDE_CONSOLE_BRIDGE").is_some() {
        "&console=1"
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
    // The saved theme, on the URL, because a webview cannot ask Rust anything before its
    // first frame — `app_get_bootstrap` is a round trip and the flash happens long before it
    // answers. `public/theme-boot.js` reads this parameter in <head> and writes `data-theme`
    // while the parser is still above <body>.
    let theme = theme(app);
    let theme_param = match theme {
        Theme::Dark => "&theme=dark",
        Theme::Light => "&theme=light",
    };
    // The chrome font size rides along for exactly the reason the theme does, and it is not
    // cosmetic polish. `installThemeSync` reads the size out of the bootstrap snapshot, and
    // that snapshot is a round trip — the same round trip that is too slow for the theme. The
    // theme arriving late is a flash of the wrong palette; this arriving late is a *reflow* of
    // every row, tab and panel in the window, on every launch, at any size but the default.
    let ui_param = format!("&ui={}", ui_font_size(app));
    let url = WebviewUrl::App(
        format!(
            "index.html?window={}{theme_param}{ui_param}{bench}{audit}{panes}{wins}{input_probe}{renderer}{console_bridge}",
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

    let builder = WebviewWindowBuilder::new(app, label.as_str(), url)
        .title(title)
        // We draw the titlebar ourselves — traffic lights, project tabs and the icon
        // cluster all live in one 34px bar. The cost is that the window manager no longer
        // provides resize borders, snap or double-click-to-maximize, so the frontend
        // implements eight grips and `core:window:allow-start-resize-dragging` is granted.
        .decorations(false)
        .inner_size(initial_width, initial_height)
        .min_inner_size(MIN_WIDTH, MIN_HEIGHT);

    // Not transparent: on Linux `transparent(true)` needs a running compositor and renders
    // opaque black or garbage without one. The frontend paints --bg.
    //
    // Split out of the chain because the method does not exist on macOS. tauri declares it
    // `#[cfg(any(not(target_os = "macos"), feature = "macos-private-api"))]`
    // (tauri-2.11.5 src/webview/webview_window.rs:1075), so a Mac build of the unsplit chain
    // fails with E0599 — which is what the first build on a Mac reported.
    //
    // The feature is deliberately NOT the answer. `macos-private-api` turns on private Apple
    // SPI and is grounds for App Store rejection, it additionally requires `macOSPrivateApi` in
    // `tauri.conf.json`, and we are passing `false` — so it would buy exactly nothing. Nor is
    // deleting the call, even though `WindowConfig::default()` already sets `transparent: false`
    // (tauri-utils-2.9.2 src/config.rs:2320) and the call therefore only asserts the default:
    // stating it is what keeps the reasoning above attached to a line of code that a future
    // tauri default-flip would break loudly rather than silently.
    //
    // macOS consequence, written down rather than left to be inferred: the window is opaque
    // there too, which is the same answer this asserts on Linux — the platforms differ in how
    // the value is set, not in what it ends up being. `background_color` below still runs on
    // every target, so the first-frame fill is unaffected.
    #[cfg(not(target_os = "macos"))]
    let builder = builder.transparent(false);

    let window = builder
        // The toolkit's own fill, for the frames between the window being mapped and the
        // webview having any content — before this it was the toolkit default, which is
        // white on GTK and was the last visible white frame a dark-theme user saw.
        //
        // Set from the saved theme rather than from `tauri.conf.json`, which has nowhere to
        // put it: `app.windows` is empty by design (see the module comment), and a config
        // value could only name one theme anyway.
        .background_color(match theme {
            Theme::Dark => BG_DARK,
            Theme::Light => BG_LIGHT,
        })
        // Mapped immediately rather than hidden-then-shown on first paint.
        //
        // The hidden-then-reveal trick is the usual way to hide a mismatched first frame,
        // and it is the right pattern on Windows and macOS. On Wayland it is actively
        // broken: a surface that is never mapped is never *configured*, so it has no size
        // — the window reported `visible=false outer=0x0` indefinitely, and because JS
        // still runs in an unmapped webview the app looked alive while every PTY sat at
        // the fallback 80x24 with no layout to measure.
        //
        // So there is nothing to hide behind and every frame has to be correct on its own.
        // Three things make that true, in the order they paint: `background_color` above
        // covers the pre-webview frames; `:root` in tokens.css is the LIGHT palette, which
        // is the default, so a webview with no attribute yet is already right for most
        // users; and `public/theme-boot.js` writes `data-theme=dark` from `?theme=` in
        // <head>, before the first frame, for the users for whom it is not. No step waits
        // on React.
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

    // Every window, shell and detached alike: the thumb buttons have to be taken off wry even in
    // a window that has nothing to navigate, or `window.history.back()` fires there — see
    // `install_mouse_nav`, whose whole subject is the handler that runs before ours.
    install_mouse_nav(app, &window);

    Ok(window)
}

/// GDK's number for the mouse's "back" thumb button.
///
/// X11 delivers the thumb buttons as 8 and 9, and GTK3's Wayland backend produces the same
/// numbers — `pointer_handle_button` in `gdk/wayland/gdkdevice-wayland.c` computes
/// `button - BTN_LEFT + 5` for anything above `BTN_MIDDLE`, so `BTN_SIDE` (0x113) is 8 and
/// `BTN_EXTRA` (0x114) is 9. The same values on both backends is what keeps this independent of
/// ADR 0006's graphics ladder. (This comment previously quoted the formula as `- BTN_LEFT + 1 +
/// 3`, which yields 7 for `BTN_SIDE`; the constants were right and the arithmetic was not.)
#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
const GDK_BUTTON_BACK: u32 = 8;
#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
const GDK_BUTTON_FORWARD: u32 = 9;

/// What one GDK event that reached the webview means for mouse navigation.
///
/// A value rather than three branches inside the signal handler, and that split is the whole
/// reason this feature is testable at all: the previous version made every decision inside a
/// closure GTK owns, where no test in this workspace can reach it, and it shipped broken for a
/// milestone. See [`nav_action`].
#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavAction {
    /// Not ours. GTK carries on and the web process sees the event exactly as before.
    Ignore,
    /// Ours, and it is the press that means something: swallow it and send `button`.
    Emit(&'static str),
    /// Ours, but not a press to act on. Swallow it and say nothing.
    Swallow,
}

/// The rule, as a pure function of the two things GDK tells us about an event.
///
/// # Why the double and triple presses are swallowed but not acted on
///
/// GDK emits `ButtonPress`, `DoubleButtonPress`, `ButtonPress`, `TripleButtonPress` for a triple
/// press, so treating every press-shaped event as a step would walk three history entries for two
/// physical clicks. WebKit peeks ahead for the same reason (`WebKitWebViewBase.cpp`). They still
/// have to be swallowed: an unhandled `DoubleButtonPress` on button 8 is one more event for wry's
/// shim (below) to turn into a `window.history.back()`.
///
/// # Why the release is swallowed too
///
/// A release with no matching press leaves WebKit's event handler believing a drag is in
/// progress — which is how a swallowed press turns into a selection that will not let go.
#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
fn nav_action(kind: gtk::gdk::EventType, button: Option<u32>) -> NavAction {
    use gtk::gdk::EventType;

    let Some(name) = button.and_then(nav_button) else {
        return NavAction::Ignore;
    };
    match kind {
        EventType::ButtonPress => NavAction::Emit(name),
        EventType::DoubleButtonPress | EventType::TripleButtonPress | EventType::ButtonRelease => {
            NavAction::Swallow
        }
        // `gdk_event_get_button` answers `None` for every other type, so this arm is defensive
        // rather than reachable — but `EventType` is `#[non_exhaustive]` and "a new GDK event
        // type is silently claimed as a thumb press" is not a failure worth leaving open.
        _ => NavAction::Ignore,
    }
}

/// Route the mouse's thumb buttons into the key gate, and stop them reaching the web process.
///
/// # Why this is in GTK: wry claims buttons 8 and 9 before the DOM ever hears about them
///
/// This is **not** the reason that stood here until M15, and the wrong reason is what made the
/// feature dead on arrival for a whole milestone, so the true one is worth stating in full.
///
/// `wry-0.55.1/src/webkitgtk/synthetic_mouse_events.rs` connects its own `button-press-event`
/// and `button-release-event` handlers to the WebKitWebView, from `attach_handlers`
/// (`webkitgtk/mod.rs:463`) inside `create_webview` — i.e. **before `WebviewWindowBuilder::build`
/// returns**. For buttons 8 and 9 it returns `Propagation::Stop` and instead
/// `run_javascript`s a synthetic DOM `mousedown`/`mouseup` with `button: 3` / `button: 4` at
/// `document.elementFromPoint()`, then, if nothing called `preventDefault`, runs
/// `window.history.back()` / `.forward()`. In a single-entry SPA session history that is a no-op,
/// which is precisely the symptom: *the thumb buttons do nothing*.
///
/// GTK3's `button-press-event` uses the `true-handled` accumulator, so the **first** handler
/// returning `TRUE` ends the emission. wry connects during `build()`; [`install_mouse_nav`] runs
/// after it and additionally defers itself onto the GTK loop with `run_on_main_thread`. A
/// `connect_button_press_event` here is therefore always second, and never ran. Measured against
/// the real GTK 3 on the development machine:
///
/// ```text
/// two button-press-event handlers, first returns True  -> ['A(first, True)']
/// an `event` handler connected SECOND, returns False   -> ['cide-event', 'wry-press(True)']
/// an `event` handler connected SECOND, returns True    -> ['cide-event']
/// ```
///
/// So the handler goes on the **generic `event` signal** instead. `gtk_widget_event_internal`
/// emits `event` first and only emits `button-press-event` if that returned `FALSE`, whatever
/// order anything was connected in. Returning `Propagation::Stop` there means wry's handler never
/// runs, no synthetic DOM event is injected, and no `history.back()` fires.
///
/// The cost of the generic signal is that it carries motion too, which is why [`nav_action`] is a
/// single cheap function: `gdk_event_get_button` is one C call that switches on the event type and
/// answers `FALSE` for everything that is not a button, so a drag pays one call per motion event
/// and nothing else.
///
/// # What is *not* true, and was written here before
///
/// It is not the case that both thumb buttons reach the DOM as `button === 0`. WebKitGTK's
/// `buttonForEvent` would indeed flatten them, but wry intercepts long before WebKit's event
/// factory ever sees the press and synthesizes `button: 3` / `button: 4`. There was consequently
/// never a "phantom left click": `pathLinks.ts`'s gate tests `ev.button !== 0` and would have
/// rejected a synthetic 3 or 4 anyway. A DOM-only implementation was in fact possible — reading
/// wry's synthetic events — and it lost because it inherits wry's `elementFromPoint` dispatch and
/// would die silently the day wry drops the shim, which is the failure mode this whole batch is
/// about.
///
/// # Why the handler goes on the webview widget rather than the toplevel
///
/// `webkitWebViewBaseButtonPressEvent` has no button filter and returns `GDK_EVENT_STOP` for
/// every press, so the event never propagates up to the GTK window — a handler on
/// `gtk_window()` would never see one.
///
/// # Not `with_webview`
///
/// That route needs the `webkit2gtk` crate. `Cargo.toml` pins `gtk` to exactly what tauri pins
/// precisely so this stays a `gtk::Widget` problem — a second version of either crate forks the
/// webview stack — so the widget is found by walking the container tree and matching the type
/// name.
#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
fn install_mouse_nav(app: &AppHandle, window: &WebviewWindow) {
    let handle = app.clone();
    let label = window.label().to_owned();
    let window = window.clone();
    // On the GTK thread, because that is where widgets may be touched at all. The same shape
    // `cmd::project::show_folder_picker` uses, and for the same reason.
    let queued = app.run_on_main_thread(move || {
        use gtk::prelude::*;

        let Some(webview) = gtk_window_webview(&window) else {
            // Survivable and worth a line: the thumb buttons simply do nothing, which is what
            // they did before this existed. Silently degrading is what would make a WebKitGTK
            // rename look like a mouse problem.
            tracing::warn!(window = %label, "no WebKitWebView widget; mouse back/forward is off");
            return;
        };

        // A positive line, not only the failure above. Whether the widget walk found its target
        // is the one thing about this feature no check script can reach, and "the warning did not
        // appear" is a weaker thing to read in a log than "it installed".
        tracing::debug!(window = %label, "mouse back/forward handler installed");

        // Stated rather than assumed. `WebKitWebViewBase`'s own realize already asks for both,
        // and wry adds `BUTTON_PRESS_MASK` on top — but "the events arrive because a dependency
        // happens to want them" is exactly the reasoning that produced the bug above, and
        // `gtk_widget_add_events` on a realized widget merges into the GdkWindow's mask.
        webview.add_events(
            gtk::gdk::EventMask::BUTTON_PRESS_MASK | gtk::gdk::EventMask::BUTTON_RELEASE_MASK,
        );

        let probe = std::env::var_os("CIDE_INPUT_PROBE").is_some();
        let press_app = handle.clone();
        let press_label = label.clone();
        webview.connect_event(move |_, event| {
            let kind = event.event_type();
            let button = event.button();
            if probe && button.is_some() {
                tracing::info!(
                    ?button,
                    event_type = ?kind,
                    "CIDE_INPUT_PROBE: gdk button event"
                );
            }
            match nav_action(kind, button) {
                NavAction::Ignore => gtk::glib::Propagation::Proceed,
                NavAction::Swallow => gtk::glib::Propagation::Stop,
                NavAction::Emit(name) => {
                    // `state()` is `None` on an event GDK carries no modifier field for, which
                    // cannot happen for a button event — but the default is "no modifiers held",
                    // which is the reading that makes an unmodified thumb press still navigate.
                    let state = event.state().unwrap_or_else(gtk::gdk::ModifierType::empty);
                    emit::mouse_nav(
                        &press_app,
                        &press_label,
                        name,
                        (
                            state.contains(gtk::gdk::ModifierType::CONTROL_MASK),
                            state.contains(gtk::gdk::ModifierType::MOD1_MASK),
                            state.contains(gtk::gdk::ModifierType::SHIFT_MASK),
                            state.contains(gtk::gdk::ModifierType::SUPER_MASK),
                        ),
                    );
                    gtk::glib::Propagation::Stop
                }
            }
        });
    });
    if let Err(error) = queued {
        tracing::warn!(%error, "could not install the mouse back/forward handler");
    }
}

/// The key token for a GDK button number, or `None` for a button we do not claim.
#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
fn nav_button(button: u32) -> Option<&'static str> {
    match button {
        GDK_BUTTON_BACK => Some("mouseback"),
        GDK_BUTTON_FORWARD => Some("mouseforward"),
        _ => None,
    }
}

/// Find the `WebKitWebView` inside a window's widget tree.
///
/// By type name rather than by downcast, because the concrete type lives in the `webkit2gtk`
/// crate this crate deliberately does not depend on. A depth-first walk: tauri nests the
/// webview inside a `GtkBox` inside the `GtkApplicationWindow`, and that nesting is an
/// implementation detail of a dependency, so nothing here assumes a depth.
#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
fn gtk_window_webview(window: &WebviewWindow) -> Option<gtk::Widget> {
    use gtk::glib::object::ObjectExt;
    use gtk::prelude::*;

    fn find(widget: &gtk::Widget) -> Option<gtk::Widget> {
        if widget.type_().name() == "WebKitWebView" {
            return Some(widget.clone());
        }
        let container = widget.downcast_ref::<gtk::Container>()?;
        container.children().iter().find_map(find)
    }

    find(window.gtk_window().ok()?.upcast_ref::<gtk::Widget>())
}

/// Nothing to install off the GTK platforms.
///
/// A stub rather than a `cfg` at the call site, so [`create`] reads the same on every target and
/// a Windows or macOS build cannot silently lose the call. Both platforms deliver the thumb
/// buttons to the DOM as `button` 3 and 4 with no mangling, so when either becomes a target the
/// answer there is a listener in `ui/src/keys/`, not this.
#[cfg(not(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
)))]
fn install_mouse_nav(_app: &AppHandle, _window: &WebviewWindow) {}

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

/// Repaint every window's *native* fill after a theme switch.
///
/// The webview repaints itself — `data-theme` on `<html>` re-resolves every token — and
/// this is the half of the window the webview does not own: the toolkit's fill, which shows
/// through during a resize and behind the webview on any frame it has not drawn. Without
/// this a window switched from dark to light keeps flashing #111113 at its edges while
/// being dragged.
///
/// Call it from wherever the theme setting is committed, after the workspace has the new
/// value; new windows need nothing, since [`create`] reads the same setting.
pub fn apply_theme(app: &AppHandle, theme: Theme) {
    let color = match theme {
        Theme::Dark => BG_DARK,
        Theme::Light => BG_LIGHT,
    };
    for window in app.webview_windows().values() {
        let _ = window.set_background_color(Some(color));
    }
}

/// Reveal a window after its frontend reports first paint.
pub fn reveal(app: &AppHandle, label: &str) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Bring a window that is already on screen to the front and give it the keyboard.
///
/// Not [`reveal`], and the difference is the `show()`. Making a window visible is first
/// paint's job — `app_ready` calls `reveal` precisely once per window, and a window that has
/// not painted has nothing worth bringing forward — so a second caller that shows windows
/// could put an unpainted webview on screen, which is the blank frame `create` exists to
/// avoid. Here the window is already up: it is behind another one, or minimized.
///
/// Best-effort, and every result is dropped on purpose. A compositor is entitled to refuse a
/// focus steal — most Wayland compositors do, absent an activation token from the gesture
/// that asked — and the caller has no better answer than the one it has already given: the
/// right tab is in front, the right pane is focused, and the window is asking politely. A
/// `Result` here would only offer callers a failure they cannot act on.
pub fn raise(app: &AppHandle, label: &WindowLabel) {
    let Some(window) = app.get_webview_window(label.as_str()) else {
        // Not an error for the same reason `destroy` says it is not: the workspace is the
        // record of what should exist, and a window it names that the desktop has lost is
        // something for `reconcile` to clear up.
        tracing::warn!(%label, "no window to raise");
        return;
    };
    // Unminimize first. `set_focus` on a minimized window is a no-op on X11 and merely sets
    // the urgency hint on some compositors, so a pane whose window the user had minimized
    // would stay minimized and the reveal would be exactly the silent no-op it is fixing.
    if window.is_minimized().unwrap_or(false) {
        let _ = window.unminimize();
    }
    let _ = window.set_focus();
}

// --- "Awaiting: X" -----------------------------------------------------------------------

/// Sessions that have finished processing and are waiting for the user.
///
/// A process-wide `static` rather than managed Tauri state, and that is a deliberate trade.
/// Managed state is the house style and would be the right shape if this were ever read
/// outside this process — but registering it means a line in `lib.rs`'s builder, and every
/// reader of this set is already in this module or in `cmd/window.rs`. A `OnceLock<Mutex<…>>`
/// keeps the whole mechanism in the two files that own windows, at the cost of not being
/// injectable in a test; [`title_with`] below is split out so the part that can be wrong is
/// testable anyway.
///
/// `BTreeSet` rather than `HashSet` purely so [`awaiting_sessions`] hands the frontend a
/// stable order — the broadcast is compared against the last one in three windows, and an
/// order that reshuffles on every insert would make every report look like a change.
///
/// **The frontend decides membership, not this process.** "Finished a turn and is waiting"
/// needs one bit of history that `SessionState` does not carry — see
/// `ui/src/panes/awaitingRule.ts`. What Rust owns is the arithmetic that needs the workspace:
/// which of these sessions is shown in which window.
fn awaiting() -> &'static Mutex<BTreeSet<SessionId>> {
    static AWAITING: OnceLock<Mutex<BTreeSet<SessionId>>> = OnceLock::new();
    AWAITING.get_or_init(|| Mutex::new(BTreeSet::new()))
}

/// Record whether one session is waiting on the user. Returns whether the set changed.
pub fn set_awaiting(session: SessionId, waiting: bool) -> bool {
    let mut set = awaiting()
        .lock()
        .expect("the awaiting set is never poisoned");
    if waiting {
        set.insert(session)
    } else {
        set.remove(&session)
    }
}

/// The waiting set, for the broadcast that lets a window opened *after* the fact paint its
/// marker.
pub fn awaiting_sessions() -> Vec<SessionId> {
    awaiting()
        .lock()
        .expect("the awaiting set is never poisoned")
        .iter()
        .copied()
        .collect()
}

/// Forget a session entirely.
///
/// Called when a child dies. Without it a `claude` killed mid-turn stays counted for the life
/// of the process and every window title keeps an `Awaiting: 1` that nothing can clear — the
/// pane is gone, so no gesture is left that could acknowledge it.
pub fn forget_awaiting(session: SessionId) -> bool {
    awaiting()
        .lock()
        .expect("the awaiting set is never poisoned")
        .remove(&session)
}

/// Whether any of `sessions` is waiting, counted once per session.
pub fn awaiting_among(sessions: &BTreeSet<SessionId>) -> usize {
    let set = awaiting()
        .lock()
        .expect("the awaiting set is never poisoned");
    sessions.iter().filter(|s| set.contains(s)).count()
}

/// The OS window title for a base name and a count of waiting sessions.
///
/// `Awaiting: N` **leads**. We draw our own titlebar, so this string is only ever read in a
/// task switcher or a window list, and that is the one place where the left of the string is
/// the part that survives truncation. Zero adds nothing at all: a permanent `Awaiting: 0` in
/// every task bar entry is noise that would teach the user to stop reading the field.
pub fn title_with(base: &str, count: usize) -> String {
    if count == 0 {
        base.to_string()
    } else {
        format!("Awaiting: {count} — {base}")
    }
}

// --- which windows the desktop is looking at ----------------------------------------------

/// The windows the desktop currently has focused.
///
/// A second process-wide `static` beside [`awaiting`], for the same trade and with a stronger
/// reason: this is read from the window-event callback on the main thread *and* from
/// [`crate::cmd::window::retitle`] on a command thread and on `cide-pty`'s reaper thread. A
/// mutex over a tiny set is the cheap answer; asking Tauri would not be.
///
/// **Driven by the event, never by asking the toolkit**, and that is not a preference. Tao
/// raises `WindowEvent::Focused` from GTK's `focus-in-event`/`focus-out-event` with a
/// `Propagation::Proceed` handler connected *before* `GtkWindow`'s own default handler, which
/// is what updates `is-active` — so a window asked "are you focused?" from inside its own
/// focus-out still answers yes. The event carries the answer; a query would race it.
///
/// Absence means "not focused", and that default is only safe because the event is reliable:
/// tao connects both handlers in `Window::new`, before the window is ever mapped, so the
/// first `Focused(true)` cannot be missed. It matters more than a default usually does —
/// believing an actually-focused window is unfocused is how [`announce`] would ask for
/// attention into it, which the window manager drops *and* which leaves the toolkit's cached
/// hint stuck up, so the next request writes nothing. That is the very failure this exists to
/// end; see [`demand_attention`].
fn focus_set() -> &'static Mutex<BTreeSet<WindowLabel>> {
    static FOCUSED: OnceLock<Mutex<BTreeSet<WindowLabel>>> = OnceLock::new();
    FOCUSED.get_or_init(|| Mutex::new(BTreeSet::new()))
}

/// Record that the desktop gave one window the focus, or took it away.
pub fn set_focused(label: &WindowLabel, has_focus: bool) {
    let mut set = focus_set().lock().expect("the focus set is never poisoned");
    if has_focus {
        set.insert(label.clone());
    } else {
        set.remove(label);
    }
}

/// Whether the desktop currently has this window focused.
pub fn is_focused(label: &WindowLabel) -> bool {
    focus_set()
        .lock()
        .expect("the focus set is never poisoned")
        .contains(label)
}

/// Forget a window that has gone.
///
/// Labels are minted per window and a closed one never comes back, but a stale `focused`
/// entry would silently exempt a window from ever asking for the user again if its label were
/// ever reused — a failure with no symptom to trace. One `remove` on `Destroyed` costs
/// nothing and removes the question.
pub fn forget_focused(label: &WindowLabel) {
    focus_set()
        .lock()
        .expect("the focus set is never poisoned")
        .remove(label);
}

/// What one window says to the desktop: its title, and whether it is asking for the user.
///
/// One struct because they are one fact — *does this window hold a session that wants the
/// user* — applied to the two surfaces a desktop offers. Returning them separately is how a
/// window ends up flashing with `Awaiting: 0` in its title.
pub struct Announcement {
    /// The OS window title, `Awaiting: N` and all.
    pub title: String,
    /// Whether to hold the window's urgency hint up.
    pub attention: bool,
}

/// **The decision**, and the whole of it: what a window should be announcing right now.
///
/// Pure, and split out from [`crate::cmd::window::retitle`] for the reason [`title_with`] was
/// split out before it — the part that can be wrong is the arithmetic, and the arithmetic is
/// the part a test can reach without an `AppHandle` and a compositor.
///
/// # `focused` is the fix for "i saw it once only"
///
/// The reported bug was that the attention marker fired on the first finished turn and never
/// again after the user came to the window. Every link in the chain was and is correct — the
/// hook fires, the state machine transitions, the frontend decides, the set moves, `retitle`
/// runs, the title is rewritten — and the signal still died, in the last inch, at the window
/// manager.
///
/// An urgency hint on the **active** window is meaningless and every window manager treats it
/// so: KWin's `demandAttention` opens `if (isActive()) set = false;`, and the request is
/// *dropped* rather than deferred. The old code asked for attention the instant the waiting
/// count moved and never asked again, which works exactly once — for the first turn, the one
/// the user kicked off before walking away. From then on the loop is: come back, read it,
/// type the next thing, and *stay in the window* while it runs. Every turn after the first
/// therefore finished with the window focused, every request was dropped on arrival, and
/// nothing re-asked when the user later walked away. `Focused(false)` was explicitly not
/// handled ("the hint goes back up when the count next changes") — but for a session that is
/// already waiting the count never changes again.
///
/// So the hint stops being an event and becomes what the title already is: a **level**, held
/// against the current facts and recomputed whenever either of them moves. `count` moves on a
/// report; `focused` moves on `WindowEvent::Focused`, which is why `lib.rs` handles both
/// directions of it and routes both into `retitle`.
///
/// # Why this is not a nuisance
///
/// The `!focused` term is also the whole of the "do not interrupt someone who is already
/// here" requirement, and it is now *stated* rather than delegated to a platform behaviour we
/// were relying on to swallow our own request. A window the user is looking at never asks for
/// them. A window they have walked away from asks for exactly as long as it holds a session
/// that has finished its turn and has not been read — which is the same span its title spends
/// saying `Awaiting: N`, because it is the same sentence in the other surface.
pub fn announce(base: &str, count: usize, focused: bool) -> Announcement {
    Announcement {
        title: title_with(base, count),
        attention: count > 0 && !focused,
    }
}

/// Set one window's title, if it still exists.
pub fn set_title(app: &AppHandle, label: &WindowLabel, title: &str) {
    let Some(window) = app.get_webview_window(label.as_str()) else {
        return;
    };
    if let Err(error) = window.set_title(title) {
        // Worth a line but not worth failing a command over: the title is a courtesy, and a
        // window that went away between the lookup and the call is the ordinary case during a
        // close.
        tracing::debug!(%label, %error, "could not set a window title");
    }
}

/// Ask the desktop to mark one window as wanting the user, or stop asking.
///
/// # Why the title was not enough, which is what was actually reported
///
/// [`title_with`] and [`set_title`] were already correct and already running: every report,
/// every detach, every child death recomputes the string and applies it. The report was that
/// nothing changed *in the task bar*, and both halves of that are true at once — because a
/// task bar is not obliged to render a window's title. Plasma's default Task Manager is
/// **Icons-only**, where the title appears in a tooltip and nowhere else; GNOME's overview
/// shows it only when the window is displayed as a thumbnail. Setting a string is a message to
/// a surface the user may not have.
///
/// The signal a task bar *does* render, in every environment, is the urgency hint —
/// `_NET_WM_STATE_DEMANDS_ATTENTION` on X11, `xdg_toplevel` activation under Wayland — which
/// is what makes a task entry light up, bounce, or bold itself. Nothing in this tree ever
/// asked for it. The note beside [`raise`] shows it was known about and only ever met as an
/// accident of `set_focus`.
///
/// # Dumb on purpose: the decision is [`announce`]'s
///
/// This function asks; it does not decide, and it must not start. `wanted` comes from
/// [`announce`], which is the one place that knows both facts the answer needs — how many of
/// this window's sessions are waiting, and whether the user is already in it.
///
/// The version of this comment that this replaces read: *"`request_user_attention` is
/// documented as having no effect while the application is focused, so the window the user is
/// working in cannot flash at them ... and it costs no state of our own to get."* Every word
/// of that is true and it was the bug. Leaning on the platform to swallow an unwanted request
/// means the platform also swallows a request we will want in a moment, and there is no
/// notification when it does: the request is **dropped, not queued**. That is the whole of
/// "i saw it once only" — see [`announce`], which now states the condition instead of hoping
/// something downstream applies it.
///
/// Two consequences worth knowing, both of which the `!focused` term in [`announce`] avoids
/// rather than merely tolerates:
///
/// * GTK caches this. `gtk_window_set_urgency_hint` is a no-op when the flag already holds
///   the value asked for, so a request that a window manager dropped *also* leaves GTK
///   believing the hint is up, and the next request — the one that would have worked — writes
///   no X property and wakes nobody.
/// * A window manager re-reads the hint on a property change, not on a focus change. Nothing
///   re-evaluates a dropped request when the user finally walks away, which is precisely when
///   they needed it.
///
/// [`UserAttentionType::Informational`] rather than `Critical`: a finished turn is news, not
/// an emergency. On Windows the difference is flashing the task bar button rather than the
/// window as well; on Linux the runtime documents both levels as the same hint, so the choice
/// costs nothing there and says the right thing everywhere else.
///
/// # Unsetting is required, not optional
///
/// Tauri's own doc: *"Unsetting the request for user attention might not be done automatically
/// by the WM when the window receives input."* So a hint raised once could otherwise stay lit
/// on a window the user has read, for the life of the process — a permanent "come back here"
/// is worse than no signal at all, because it is the signal that teaches people to stop
/// looking. Both directions come from the same place now: `WindowEvent::Focused` in `lib.rs`
/// records the focus and re-runs `retitle`, so coming to the window lowers the hint and
/// leaving a window that still holds a waiting session raises it again.
pub fn demand_attention(app: &AppHandle, label: &WindowLabel, wanted: bool) {
    let Some(window) = app.get_webview_window(label.as_str()) else {
        return;
    };
    let request = wanted.then_some(tauri::UserAttentionType::Informational);
    if let Err(error) = window.request_user_attention(request) {
        // Same disposition as `set_title` above and for the same reason: this is a courtesy to
        // the desktop, and a window that closed between the lookup and the call is ordinary.
        tracing::debug!(%label, %error, wanted, "could not set a window's urgency hint");
    }
}

#[cfg(test)]
mod tests {
    use super::{BG_DARK, BG_LIGHT};
    use cide_ipc::SessionId;
    use tauri::window::Color;

    /// The one place a token value is copied out of CSS into Rust, so it is the one place
    /// that can silently drift. It costs a frame of the wrong colour at launch — the exact
    /// bug `background_color` was added to fix — and nothing else in the tree notices,
    /// because Rust never reads the stylesheet and the stylesheet never reads Rust.
    ///
    /// Parsed from the file rather than restated here: a second copy of the literal would
    /// only pin this test to itself.
    #[test]
    fn the_native_window_fill_matches_the_bg_token_of_each_theme() {
        let css = include_str!("../../../ui/src/styles/tokens.css");

        // `--bg` appears once per theme block and the light block is first, matching the
        // order the file is written in (`:root, [data-theme='light']` then dark).
        let mut found = css
            .lines()
            .filter_map(|l| l.trim().strip_prefix("--bg:"))
            .map(|v| v.trim().trim_end_matches(';'));
        let light = found.next().expect("tokens.css declares no --bg");
        let dark = found.next().expect("tokens.css declares only one --bg");
        assert_eq!(
            found.next(),
            None,
            "a third --bg appeared; this test picks by order"
        );

        assert_eq!(hex(BG_LIGHT), light, "BG_LIGHT drifted from the light --bg");
        assert_eq!(hex(BG_DARK), dark, "BG_DARK drifted from the dark --bg");
    }

    fn hex(c: Color) -> String {
        format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
    }

    /// The user asked for `Awaiting: X` "to see in the task bar", so the two things that
    /// matter are that the count leads and that zero is silent.
    #[test]
    fn a_window_title_announces_waiting_sessions_and_only_then() {
        use super::title_with;

        assert_eq!(title_with("atlas", 0), "atlas", "zero must add nothing");
        assert_eq!(title_with("atlas", 1), "Awaiting: 1 — atlas");
        assert_eq!(title_with("atlas", 3), "Awaiting: 3 — atlas");
        assert!(
            title_with("atlas", 2).starts_with("Awaiting:"),
            "a task switcher truncates the right-hand end, so the count has to be on the left"
        );
    }

    /// Membership is per session, and forgetting one leaves the others counted.
    ///
    /// The set is a process-wide static, so this test uses ids it mints itself and never
    /// asserts on the set's total — another test in this binary may legitimately hold
    /// entries at the same time.
    #[test]
    fn a_session_that_dies_stops_being_counted_and_takes_nothing_with_it() {
        use super::{awaiting_among, forget_awaiting, set_awaiting};
        use std::collections::BTreeSet;

        let dying = SessionId::new();
        let survivor = SessionId::new();
        let never = SessionId::new();
        let mine: BTreeSet<_> = [dying, survivor, never].into_iter().collect();

        assert!(set_awaiting(dying, true), "a first insert is a change");
        assert!(!set_awaiting(dying, true), "a repeat report is not news");
        set_awaiting(survivor, true);
        assert_eq!(awaiting_among(&mine), 2);

        assert!(forget_awaiting(dying));
        assert_eq!(
            awaiting_among(&mine),
            1,
            "forgetting took the wrong session, or took more than one"
        );

        set_awaiting(survivor, false);
        assert_eq!(awaiting_among(&mine), 0);
    }

    /// The desktop, in the only two respects this feature depends on.
    ///
    /// Both are platform facts rather than choices of ours, and between them they are the
    /// whole reason the shipped code called the user exactly once. Neither is visible to a
    /// test that asks "did we call `request_user_attention(true)`", which the shipped code
    /// did — on time, on every single turn.
    ///
    /// 1. **The toolkit caches the hint.** `gtk_window_set_urgency_hint` is `if (priv->urgent
    ///    != setting)`, so asking for a state the window already holds writes no X property
    ///    and the window manager is told nothing at all.
    /// 2. **The window manager drops a request on the active window**, rather than deferring
    ///    it. KWin's `demandAttention` opens `if (isActive()) set = false;`. It re-reads the
    ///    hint when the property changes; a focus change changes no property, so nothing
    ///    re-examines a request that was dropped.
    ///
    /// Together: raise the hint on a window the user is sitting in and the request is thrown
    /// away *and* the toolkit now believes the hint is up, so the next raise — the one that
    /// would have worked — writes nothing. That is `i saw it once only` in two lines.
    #[derive(Default)]
    struct Desktop {
        /// The toolkit's cached flag.
        hint: bool,
        /// What the task bar entry is actually doing, which is the only thing the user sees.
        lit: bool,
        /// Every state the entry took, so the run can be asserted whole rather than sampled.
        history: Vec<bool>,
    }

    impl Desktop {
        /// `windows::demand_attention`, all the way down to the compositor.
        fn request(&mut self, wanted: bool, active: bool) {
            if wanted != self.hint {
                self.hint = wanted;
                // The property moved, so the window manager looks — and ignores a demand from
                // the window it has already given the user.
                self.lit = wanted && !active;
            }
            self.history.push(self.lit);
        }

        /// The user arriving. A window manager clears its own demands-attention state on
        /// activation; the toolkit's cached flag is untouched by that, which is the trap.
        fn activated(&mut self) {
            self.lit = false;
        }
    }

    /// **The reported bug**: the marker called the user once and never again.
    ///
    /// > "i saw it once only, and after i've focused - nothing more calling me in title, or
    /// > panel - it seems that after first run it failes to work again"
    ///
    /// Three full turns with an acknowledgement between them, which is the shape the shipped
    /// tests did not have: they drove one turn, and one turn is the turn that works. The
    /// turns after the first are the ones that finish while the user is **sitting in the
    /// window**, because that is what the loop looks like once they have come back to read
    /// the first — come back, read it, type the next thing, stay there while it runs. Then
    /// they walk away again, and that is the moment the window has to start calling them.
    ///
    /// Every function driven here is the production one, in the order its production caller
    /// calls it: `set_awaiting` is `window_set_awaiting`'s, `awaiting_among` and `announce`
    /// are `retitle`'s, and `set_focused` is what `lib.rs` does with `WindowEvent::Focused`.
    /// The only model is [`Desktop`], and it models the desktop rather than any of our code.
    #[test]
    fn a_finished_turn_calls_the_user_back_on_every_turn_and_not_only_the_first() {
        use super::{announce, awaiting_among, is_focused, set_awaiting, set_focused};
        use cide_ipc::WindowLabel;
        use std::collections::BTreeSet;

        // Both statics are process-wide, so this test names a window and a session that
        // nothing else in this binary can be holding.
        let shell = WindowLabel(format!("shell:{}", SessionId::new()));
        let session = SessionId::new();
        let shown: BTreeSet<_> = [session].into_iter().collect();

        // `cmd::window::retitle`, for the one window this test has.
        let retitle = |desk: &mut Desktop| -> String {
            let say = announce("atlas", awaiting_among(&shown), is_focused(&shell));
            desk.request(say.attention, is_focused(&shell));
            say.title
        };
        // The four things that actually happen, each ending in the recompute its production
        // caller ends in: `window_set_awaiting` → `retitle`, and `WindowEvent::Focused` →
        // `cmd::window::focus_changed` → `retitle`.
        let turn_ends = |desk: &mut Desktop| {
            set_awaiting(session, true);
            retitle(desk)
        };
        let reads_the_pane = |desk: &mut Desktop| {
            set_awaiting(session, false);
            retitle(desk)
        };
        let comes_to_the_window = |desk: &mut Desktop| {
            desk.activated();
            set_focused(&shell, true);
            retitle(desk)
        };
        let walks_away = |desk: &mut Desktop| {
            set_focused(&shell, false);
            retitle(desk)
        };

        let desk = &mut Desktop::default();

        // --- turn 1: typed the prompt, then left ------------------------------------------
        assert_eq!(comes_to_the_window(desk), "atlas");
        assert_eq!(walks_away(desk), "atlas", "nothing is waiting yet");
        assert!(
            !desk.lit,
            "a window with nothing waiting never asks for anybody"
        );

        assert_eq!(turn_ends(desk), "Awaiting: 1 — atlas");
        assert!(
            desk.lit,
            "the first finished turn has to call the user — this much always worked, and it \
             is the whole of what the user ever saw"
        );

        // --- they come back, read it, and stay in the window -------------------------------
        assert_eq!(
            comes_to_the_window(desk),
            "Awaiting: 1 — atlas",
            "arriving at the window is not reading the session: a shell holds every tab of \
             every project it shows, so the title keeps saying so until the pane is read"
        );
        assert!(
            !desk.lit,
            "a window the user is looking at must not flash at them, and it must be THIS code \
             declining to ask rather than the window manager quietly dropping the request"
        );

        assert_eq!(
            reads_the_pane(desk),
            "atlas",
            "a session read is not announced"
        );
        assert!(!desk.lit);

        // --- turn 2: they typed the next thing and stayed --------------------------------
        //
        // The whole bug lives here. The count moves, `retitle` runs, the title is rewritten —
        // and the shipped code asked for attention at this instant, into a focused window,
        // where the request was dropped on arrival and never remade.
        assert_eq!(
            turn_ends(desk),
            "Awaiting: 1 — atlas",
            "turn 2 must retitle exactly as turn 1 did"
        );
        assert!(!desk.lit, "still nothing to shout about: the user is here");

        // --- and now they walk away, with a finished turn unread ---------------------------
        //
        // THE assertion, and it is the user's sentence in code. Before this fix nothing
        // recomputed here at all — `Focused(false)` was unhandled, and the comment that
        // justified it ("the hint goes back up when the count next changes") is wrong for the
        // one case that matters: a session already waiting never moves the count again.
        assert_eq!(walks_away(desk), "Awaiting: 1 — atlas");
        assert!(
            desk.lit,
            "the user walked away from a finished turn and nothing called them back — this is \
             \"i saw it once only\", and one turn per test is what let it ship"
        );

        // --- turn 3, the same loop, to pin that it is not two-shot either -----------------
        comes_to_the_window(desk);
        reads_the_pane(desk);
        turn_ends(desk);
        assert!(!desk.lit);
        walks_away(desk);
        assert!(
            desk.lit,
            "every turn — not the first, and not the first two"
        );

        assert_eq!(
            desk.history,
            vec![
                false, false, true, // turn 1: nothing, nothing, then away and called
                false, false, false, // read it, stayed, turn 2 ended with them here
                true,  // they left: called again
                false, false, false, // read it, stayed, turn 3 ended with them here
                true,  // they left: called again
            ],
            "the task entry lights exactly when the user is away from a window holding a \
             finished turn they have not read, and is dark at every other moment"
        );

        // Leave the process-wide sets as they were found.
        set_awaiting(session, false);
        super::forget_focused(&shell);
    }

    /// The half a behavioural test cannot reach: that the focus actually gets *delivered*.
    ///
    /// [`announce`] can be perfect and the feature still dead if nothing tells it the user
    /// left — which is precisely the shape the bug shipped in, since every other link was
    /// already correct. A source assertion, in the same spirit as
    /// `ui/scripts/check-awaiting.mjs`'s: this project's recurring defect is a complete
    /// mechanism whose output reaches no surface, and the wiring is what nothing else can see.
    #[test]
    fn a_window_losing_the_focus_recomputes_what_it_is_announcing() {
        let lib = include_str!("lib.rs");
        let window = include_str!("cmd/window.rs");

        assert!(
            !lib.contains("WindowEvent::Focused(true) =>"),
            "lib.rs handles only the focus arriving. Losing the focus is the moment a window \
             holding an unread finished turn has to start asking for the user, and it is the \
             only moment left: the waiting count does not move again for a session that is \
             already waiting"
        );
        assert!(
            lib.contains("WindowEvent::Focused(has_focus) =>")
                && lib.contains("cmd::window::focus_changed("),
            "lib.rs must route both directions of WindowEvent::Focused into \
             cmd::window::focus_changed, which records where the user is and recomputes"
        );
        assert!(
            window.contains("windows::set_focused(label, has_focus)")
                && window.contains("retitle(app, &state.snapshot())"),
            "focus_changed must record the focus AND recompute; recording it alone leaves the \
             hint at whatever the last count change happened to set"
        );
        assert!(
            window.contains("windows::announce(")
                && !window.contains("demand_attention(app, label, count"),
            "retitle must take its hint from windows::announce, which is the one place that \
             knows both facts. A bare `count > 0` asks a focused window to flash, the window \
             manager drops the request, and nothing ever asks again"
        );
    }

    // --- mouse back and forward -----------------------------------------------------------

    /// Rust source with its comments removed, so a source assertion cannot be satisfied by prose.
    ///
    /// This project has been bitten by the other kind: `check-*.mjs` greps that matched a string
    /// inside the very comment explaining why the feature exists, so deleting the feature left
    /// the gate green. Here the hazard is the mirror image and just as real — the assertion below
    /// is a *negative* one, and `install_mouse_nav`'s own documentation has to name
    /// `connect_button_press_event` in order to explain what wry does with it. Without this the
    /// only way to keep the test green would be to stop writing the explanation down.
    ///
    /// The implementation moved to [`crate::srcgrep`] when `lifecycle.rs` needed the same thing
    /// for the run loop's exit arms. It is imported rather than copied for the ordinary reason,
    /// and for one specific to it: a second stripper would be a second set of edge cases, and
    /// the edge cases are the whole of what makes a source assertion trustworthy.
    // Gated to match its only consumer. The test below is `cfg`-ed to the platforms that have a
    // GTK window to attach a button handler to; leaving the import unconditional made
    // `clippy --all-targets -D warnings` fail on macOS with `unused import` — which is the fourth
    // step of the new advisory macOS CI job, so the job would have gone red on its first run for a
    // reason that has nothing to do with the platform support it exists to measure.
    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    use crate::srcgrep::without_comments;

    /// The gate that would have caught the shipped-and-dead mouse buttons.
    ///
    /// Nothing in this repository asserted that a GDK button ever reaches a `Decision`.
    /// `check:keys` sweeps `gate.mouseHandler('mouseback', …)` — one function call *downstream*
    /// of the entire broken segment; `keymap.rs` proves the binding resolves; `contract-check`
    /// records the event's name. Every gate tested one link of a five-link chain and the broken
    /// link was the one owned by a dependency, so no behavioural test could have found it: the
    /// bug is *which GTK signal we connect to*, and the answer is only wrong in the presence of
    /// wry's handler on the same widget.
    ///
    /// A structural assertion is therefore the honest gate, and it is a strong one, because the
    /// failure message carries the whole diagnosis to whoever trips it.
    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    #[test]
    fn the_thumb_buttons_are_taken_on_a_signal_wry_has_not_already_claimed() {
        // Everything above `mod tests` — the module below has to *name* the thing it forbids,
        // both in the assertions and in their failure messages, and a negative source assertion
        // that includes its own text can only ever fail. Caught by writing it the other way
        // round first, which is recorded here because the mistake is not obvious until it fires.
        let source = include_str!("windows.rs")
            .split("\n#[cfg(test)]\nmod tests {")
            .next()
            .expect("windows.rs still has a #[cfg(test)] module at the end");
        let source = without_comments(source);

        assert!(
            !source.contains("connect_button_press_event")
                && !source.contains("connect_button_release_event"),
            "wry connects its OWN button-press-event/button-release-event handlers to the \
             WebKitWebView inside `WebviewWindowBuilder::build()` (wry-0.55.1 \
             src/webkitgtk/synthetic_mouse_events.rs), returns Propagation::Stop for buttons 8 \
             and 9, and spends them on window.history.back(). GTK3's button-press-event uses the \
             true-handled accumulator, so the first handler to return TRUE ends the emission — \
             and anything cide connects after build() is second. Connecting there means this \
             handler NEVER RUNS and the thumb buttons silently do nothing, which is exactly how \
             this shipped for a milestone. Use WidgetExt::connect_event: GTK emits the generic \
             `event` signal before any specific one, whatever the connection order."
        );
        assert!(
            source.contains("webview.connect_event(move |_, event|"),
            "the thumb buttons must be claimed on the generic `event` signal, on the webview \
             widget — that is the only place in this process that runs before wry's handler"
        );
        assert!(
            source.contains("NavAction::Emit(name) =>") && source.contains("emit::mouse_nav("),
            "and a press that resolves to a nav button must still reach emit::mouse_nav — a \
             handler that swallows the event and sends nothing is a thumb button that does \
             nothing, told apart from today's bug only by reading the source"
        );
    }

    /// The rules, driven directly. These are cheap and they are also the half that a rewrite of
    /// the signal plumbing would be most likely to get subtly wrong.
    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    #[test]
    fn only_the_first_press_of_a_thumb_button_walks_the_history() {
        use super::{NavAction, nav_action};
        use gtk::gdk::EventType;

        assert_eq!(
            nav_action(EventType::ButtonPress, Some(8)),
            NavAction::Emit("mouseback")
        );
        assert_eq!(
            nav_action(EventType::ButtonPress, Some(9)),
            NavAction::Emit("mouseforward")
        );

        // GDK sends ButtonPress, DoubleButtonPress, ButtonPress, TripleButtonPress for a triple
        // press. Acting on each would walk three entries for two physical clicks; letting them
        // through would hand wry three more chances to run history.back().
        for kind in [
            EventType::DoubleButtonPress,
            EventType::TripleButtonPress,
            EventType::ButtonRelease,
        ] {
            assert_eq!(
                nav_action(kind, Some(8)),
                NavAction::Swallow,
                "{kind:?} on a thumb button is swallowed and not acted on"
            );
        }

        // Everything else keeps propagating, and this is the load-bearing negative: the handler
        // is on the *generic* event signal, so an over-eager Stop here would swallow every left
        // click, every motion event and every scroll in the whole application.
        for button in [None, Some(1), Some(2), Some(3), Some(4), Some(5), Some(10)] {
            assert_eq!(
                nav_action(EventType::ButtonPress, button),
                NavAction::Ignore,
                "button {button:?} is not cide's"
            );
        }
        assert_eq!(nav_action(EventType::MotionNotify, None), NavAction::Ignore);
        assert_eq!(nav_action(EventType::Scroll, None), NavAction::Ignore);
        assert_eq!(nav_action(EventType::KeyPress, None), NavAction::Ignore);
    }

    /// The other half of the report: a refusal the user cannot see is a dead gesture.
    ///
    /// `jump.ts` writes a sentence for Back-with-nothing-behind-it specifically so that "refused"
    /// can be told from "wired to nothing". Routing it through `unmet` put it in `diag.log`, so
    /// on screen the two were identical — which is why the buttons being *completely dead* went
    /// unnoticed for a milestone by everyone including the person who wrote the sentence.
    #[test]
    fn a_refused_navigation_is_shown_to_the_user_and_not_only_logged() {
        let dispatch = include_str!("../../../ui/src/keys/dispatch.ts");
        let arm = dispatch
            .split("case 'navigate.back':")
            .nth(1)
            .and_then(|rest| rest.split("case 'navigate.nextMember':").next())
            .expect("dispatch.ts still has a navigate.back arm");
        assert!(
            arm.contains("notify(refusal"),
            "the navigate.back/forward arm must surface jump.ts's refusal with notify(...). \
             `unmet` writes one line through diag.log and nothing else, so a Back with an empty \
             history looks exactly like a Back wired to nothing"
        );
        assert!(
            !arm.contains("unmet(command, refusal)"),
            "and it must not ALSO go through unmet — two reports for one refusal, one of them \
             invisible, is how the visible half gets deleted later as a duplicate"
        );
    }
}
