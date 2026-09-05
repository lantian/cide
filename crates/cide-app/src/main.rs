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

    // Then every other argument, on the same terms and for a sharper reason: before this,
    // `cide --help` started the full IDE. This binary's path is in `$EDITOR` inside every pane,
    // so anything probing it launched a second instance on a live profile — see `cli`'s header
    // for the morning that cost. Above the graphics ladder because a refusal must not create a
    // window, and below `edit_wait` because `--wait` is that module's flag.
    if let Some(code) = cide_app::cli::cli() {
        std::process::exit(code);
    }

    // And the second lock: whatever else starts a second cide, it stops here rather than
    // restoring a workspace the running instance will overwrite on its way out. Also before the
    // graphics ladder — the refusal is a sentence on stderr and no display is needed for it.
    if let Err(holder) = cide_app::instance::acquire() {
        eprintln!("{}", cide_app::instance::refusal(holder));
        std::process::exit(1);
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
