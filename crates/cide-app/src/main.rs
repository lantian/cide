// Hide the console window on Windows release builds. No effect on Linux, which is the
// current target, but the attribute has to be present from the start or it is forgotten.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // First, before anything touches GTK or the webview. These variables are read when the
    // webview is created and setting them later silently does nothing — and without them
    // the app does not start at all on a stock KDE Wayland desktop. See `graphics.rs`.
    let applied = cide_app::graphics::apply();
    for (k, v) in &applied {
        eprintln!("[cide] graphics workaround: {k}={v}");
    }

    cide_app::run()
}
