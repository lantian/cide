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
    let url = WebviewUrl::App(
        format!(
            "index.html?window={}{theme_param}{bench}{audit}{panes}{wins}{input_probe}{renderer}{console_bridge}",
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
        // renders opaque black or garbage without one. The frontend paints --bg.
        .transparent(false)
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
}
