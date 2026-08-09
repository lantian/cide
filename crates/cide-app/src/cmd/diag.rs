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

/// Forward a frontend diagnostic to the app's stderr.
///
/// The webview's own console is not reachable from a shell, and on Wayland the devtools
/// window is as hard to raise as the app window itself. A one-line command that puts
/// frontend state next to the Rust log is worth more than either.
#[tauri::command]
pub fn diag_log(message: String) {
    eprintln!("[cide/ui] {message}");
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

/// Record what the frontend measured at boot.
///
/// Logged rather than stored for M0; M11 surfaces it in Settings and emits
/// `cide://ipc-degraded` so a user on the slow path finds out from the UI instead of from
/// the app feeling inexplicably sluggish.
#[tauri::command]
pub fn diag_report_ipc(health: IpcHealth) {
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
