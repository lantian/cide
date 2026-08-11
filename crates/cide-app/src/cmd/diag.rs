//! Diagnostics — the M0 GO/NO-GO instrumentation.
//!
//! Two things are measured here, and both can invalidate the architecture, which is why
//! they are built before anything else:
//!
//! 1. **Which IPC path we actually got.** Tauri's fast path degrades silently. If
//!    WebKitGTK is older than 2.40 or a CSP rule blocks the `ipc:` scheme,
//!    `ipc-protocol.js` catches the fetch rejection, sets `customProtocolIpcFailed = true`
//!    *permanently*, logs one `console.warn` and falls back to string `postMessage`.
//!    Throughput collapses with no crash and no Rust-side signal, so the frontend has to
//!    report which path it is on.
//! 2. **How fast that path is.** The PTY transport assumes raw-`Response` pull can move a
//!    terminal's worth of bytes. If it cannot, the whole streaming design changes to
//!    Rust-side VT parsing with dirty-row diffs — a decision that has to be made before
//!    code is written on top of it.

use cide_ipc::IpcHealth;
use tauri::ipc::{Channel, InvokeResponseBody, Response};

/// Return `len` bytes with no JSON encoding, for the pull-path benchmark.
///
/// `Response::new(Vec<u8>)` travels as `application/octet-stream` over the custom
/// protocol: one round trip, no eval, no JSON, no base64.
#[tauri::command]
pub fn diag_echo_bytes(len: usize) -> Response {
    // Not zeroes: a run of identical bytes is exactly what any compression in the path
    // would flatter, and we want to measure the honest case.
    let mut buf = vec![0u8; len];
    for (i, b) in buf.iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }
    Response::new(buf)
}

/// Push `count` frames of `len` bytes down a channel, for the push-path benchmark.
///
/// This mirrors how PTY output actually reaches the webview, so its number is the one the
/// terminal design depends on.
#[tauri::command]
pub fn diag_push_bytes(len: usize, count: usize, sink: Channel<InvokeResponseBody>) {
    let mut buf = vec![0u8; len];
    for (i, b) in buf.iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }
    std::thread::spawn(move || {
        for _ in 0..count {
            if sink.send(InvokeResponseBody::Raw(buf.clone())).is_err() {
                break;
            }
        }
    });
}

/// Forward a frontend diagnostic into the app's log.
///
/// The webview's own console is not reachable from a shell, and on Wayland the devtools
/// window is as hard to raise as the app window itself. A one-line command that puts frontend
/// state next to the Rust log is worth more than either.
///
/// **Through `tracing`, not `eprintln!`, and that was a real defect rather than a tidy-up.**
/// This is documented as the one way a message gets out of the webview, and the instruction
/// that goes with it is "read the log file" — but stderr does not go to the log file. It goes
/// to whatever the process was launched from, which for a desktop launcher is nowhere and for
/// `run.sh` is a terminal nobody is reading. Every frontend diagnostic this app has ever
/// emitted — the input probe's per-keystroke trace, the console bridge, write failures —
/// landed somewhere the person who asked for it was not looking.
///
/// `target:` names the frontend explicitly, so a filter can raise or silence the webview's
/// output without touching the Rust modules' own levels.
#[tauri::command]
pub fn diag_log(message: String) {
    tracing::info!(target: "cide::ui", "{message}");
}

/// Print a finished benchmark report to stdout and exit if this was a headless bench run.
///
/// Exists so `CIDE_BENCH=1 cide` is a complete, scriptable GO/NO-GO check rather than
/// something a human has to click and read off a screen.
#[tauri::command]
pub fn diag_bench_report(app: tauri::AppHandle, report: String) {
    println!("\n{report}\n");
    if std::env::var_os("CIDE_BENCH").is_some() {
        app.exit(0);
    }
}

/// Record what the frontend measured, and raise the alarm if the transport has degraded.
///
/// The `cide://ipc-degraded` broadcast is the part that took until now to exist, and its
/// absence was the whole problem: the doc comment above promised it from M0, `contract/events.json`
/// never had it, and so the one failure this instrumentation was built to catch was the one
/// failure nothing could report. A degraded transport does not crash — it makes every payload
/// JSON-encoded and `eval`'d, permanently, and the app just feels slow.
///
/// **Called on every probe, not only at boot.** `customProtocolIpcFailed` flips at any later
/// moment (`tauri-2.11.5/scripts/ipc-protocol.js`, in the rejection handler that also catches a
/// body-decode failure *after* a command has run), so a boot-only report answers the one state
/// that was never in doubt.
#[tauri::command]
pub fn diag_report_ipc(app: tauri::AppHandle, health: IpcHealth) {
    if !health.custom_protocol {
        crate::emit::ipc_degraded(&app, &health);
    }
    if health.custom_protocol {
        tracing::info!(
            webkit = %health.webkit_version,
            mib_per_sec = health.mib_per_sec,
            "ipc: custom protocol active"
        );
    } else {
        tracing::warn!(
            webkit = %health.webkit_version,
            mib_per_sec = health.mib_per_sec,
            "ipc: DEGRADED to postMessage — every payload is now JSON-encoded and eval'd"
        );
    }
}
