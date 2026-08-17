//! Linux graphics workarounds, applied before the webview exists.
//!
//! # Why this module is not optional
//!
//! WebKitGTK's rendering path is the least predictable dependency in the stack, and unlike
//! an Electron app we cannot pin the engine — we get whatever the distribution ships.
//! Measured on the development machine (openSUSE Tumbleweed, KDE Plasma on Wayland,
//! WebKitGTK 2.52.3):
//!
//! | environment                          | result                                      |
//! |--------------------------------------|---------------------------------------------|
//! | default (Wayland)                    | `Gdk-Message: Error 71 (Protocol error)`, exits immediately |
//! | `GDK_BACKEND=x11`                    | runs, but spams `Failed to create GBM buffer of size 1440x900` |
//! | `WEBKIT_DISABLE_DMABUF_RENDERER=1`   | runs cleanly on native Wayland              |
//! | `WEBKIT_DISABLE_COMPOSITING_MODE=1`  | runs, but disables accelerated compositing   |
//!
//! So the app is unlaunchable out of the box on a mainstream KDE Wayland desktop, and the
//! cure is one environment variable that must be set **before** the webview is created —
//! after that it has no effect, which is why this runs at the very top of `main`.
//!
//! # The ladder
//!
//! Each rung is more aggressive and costs more performance than the last. We apply the
//! cheapest one that is known to be needed, and Settings → Appearance exposes the rest for
//! the machines this heuristic gets wrong. Every variable respects an existing value in
//! the environment, so a user who already knows what their hardware needs is never
//! overridden.

/// Whether this platform has a ladder at all.
///
/// Every rung is a WebKitGTK variable, and WebKitGTK is the Linux web view. On macOS the web
/// view is WKWebView and on Windows it is WebView2; neither reads any of these names, so
/// [`apply`] sets nothing there — correctly, and it is worth being explicit about rather than
/// leaving as an inline `cfg!`, because two surfaces downstream depend on the answer:
///
/// * `cmd::settings::graphics_status`, and through it **Settings → Appearance, which off Linux
///   offers three switches that persist and do nothing.** That is a known defect rather than a
///   decision — see `README.md`'s Platforms section. This constant is the seam to gate it on
///   when someone with a Mac can see the screen.
/// * `apply_graphics_overrides`, which puts the user's stored choices into the environment; the
///   same reasoning applies to it and for the same reason it is harmless rather than wrong.
///
/// A named constant also means the two places that branch on the platform cannot drift into
/// disagreeing about what "Linux" meant.
pub const LADDER_APPLIES: bool = cfg!(target_os = "linux");

/// Apply the graphics workarounds this session needs.
///
/// Must be called before `tauri::Builder` runs. Returns the variables it set, for logging.
pub fn apply() -> Vec<(&'static str, &'static str)> {
    let mut applied = Vec::new();

    // An explicit opt-out, for bisecting a rendering bug against stock behaviour.
    if std::env::var_os("CIDE_NO_GRAPHICS_WORKAROUNDS").is_some() {
        return applied;
    }

    if LADDER_APPLIES {
        // Rung 1: the DMABUF renderer. This is the one that matters in practice — it is
        // implicated in the Wayland protocol error above, in blank windows on NVIDIA, and
        // in a family of "the app starts but never paints" reports across compositors.
        // Disabling it costs a little compositing throughput and buys the app starting.
        if set_if_unset("WEBKIT_DISABLE_DMABUF_RENDERER", "1") {
            applied.push(("WEBKIT_DISABLE_DMABUF_RENDERER", "1"));
        }

        // Rung 2, NVIDIA-only: explicit sync is a frequent cause of hangs on the
        // proprietary driver. Cheap enough to set unconditionally on Linux, and inert
        // elsewhere.
        if set_if_unset("__NV_DISABLE_EXPLICIT_SYNC", "1") {
            applied.push(("__NV_DISABLE_EXPLICIT_SYNC", "1"));
        }

        // Rung 3 — `WEBKIT_DISABLE_COMPOSITING_MODE=1` — is deliberately NOT applied by
        // default. It also fixes the startup crash, but it turns off accelerated
        // compositing for the whole webview, which a four-terminal grid feels immediately.
        // Settings offers it for the machines rung 1 does not rescue.
    }

    applied
}

/// Set an environment variable only if the user has not already chosen a value.
fn set_if_unset(key: &'static str, value: &'static str) -> bool {
    if std::env::var_os(key).is_some() {
        return false;
    }
    // SAFETY: called from `main` before any threads are spawned. Rust 2024 marks
    // `set_var` unsafe precisely because a concurrent `getenv` in another thread is a data
    // race; there is no other thread yet at this point.
    unsafe { std::env::set_var(key, value) };
    true
}
