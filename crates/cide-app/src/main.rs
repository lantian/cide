// Hide the console window on Windows release builds. No effect on Linux, which is the
// current target, but the attribute has to be present from the start or it is forgotten.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // **Before the graphics ladder, and before anything else at all.** `cide --wait <file>` is
    // this binary acting as a client of an already-running cide: it opens no window, needs no
    // display, and must still work on a machine that has neither. Every line below it sets an
    // environment variable for a webview this mode never creates. See `edit_wait`.
    if let Some(code) = cide_app::edit_wait::cli() {
        std::process::exit(code);
    }

    // First, before anything touches GTK or the webview. These variables are read when the
    // webview is created and setting them later silently does nothing — and without them
    // the app does not start at all on a stock KDE Wayland desktop. See `graphics.rs`.
    // The user's own choices go in first: `graphics::apply` only ever sets a variable that
    // has no value yet, so anything pinned here wins, and an explicit choice on any rung
    // stands its automatic ladder down. See `cmd::settings::apply_graphics_overrides`.
    let mut applied = cide_app::cmd::settings::apply_graphics_overrides();
    applied.extend(cide_app::graphics::apply());
    for (k, v) in &applied {
        eprintln!("[cide] graphics workaround: {k}={v}");
    }

    cide_app::run()
}
