# IPC transport benchmark — the M0 GO/NO-GO gate

Run it yourself:

```sh
cargo build -p cide-app
CIDE_BENCH=1 ./target/debug/cide
```

The app opens a window, runs the measurement on first paint, prints the report to stdout
and exits. It is deliberately scriptable rather than a button: on a compositor with
focus-stealing prevention a shell-launched window may never come to the front to be
clicked, and this number needs to be reproducible in CI.

## Why this gate exists

The whole terminal design rests on one assumption — that raw bytes can cross the Tauri IPC
boundary fast enough to carry a PTY under load. If they cannot, the correct design is
Rust-side VT parsing shipping dirty-row diffs, and that has to be decided *before* code is
written on top of the current assumption. Hence: first milestone, before anything else.

**Floor: 30 MiB/s on the raw pull path.** Below that, stop and change the transport.

## Result — 2026-08-07

Machine: openSUSE Tumbleweed, KDE Plasma on Wayland, WebKitGTK 2.52.3, Tauri 2.11.5,
`@xterm/xterm` 6.0.0, debug build (`opt-level = 1`, deps at 2).

```
custom protocol : YES (fast path)
webkit          : 605.1.15

| transport            | payload  | iters | MiB/s   | mean ms | p99 ms |
|----------------------|----------|-------|---------|---------|--------|
| pull (Response raw)  | 1 KiB    | 200   |     9.8 |   0.100 |  2.000 |
| pull (Response raw)  | 8 KiB    | 200   |    78.1 |   0.100 |  1.000 |
| pull (Response raw)  | 64 KiB   | 200   |   173.6 |   0.360 |  1.000 |
| pull (Response raw)  | 256 KiB  | 200   |   195.3 |   1.280 |  3.000 |
| push (Channel raw)   | 1 KiB    | 8192  |     9.4 |   0.104 |      — |
| push (Channel raw)   | 8 KiB    | 1024  |    62.0 |   0.126 |      — |
| push (Channel raw)   | 64 KiB   | 128   |   228.6 |   0.273 |      — |
| push (Channel raw)   | 256 KiB  | 32    |   228.6 |   1.094 |      — |
```

**GO** — raw pull peaks at 195.3 MiB/s, roughly 6.5x the floor. Push peaks at 228.6 MiB/s.
The PTY transport keeps its current shape: raw bytes over `Channel<InvokeResponseBody>`,
with xterm.js owning the VT state.

## What the shape of the curve says

The interesting result is not the peak — it is the **8x cliff between 1 KiB and 8 KiB**.

Tauri routes raw `Channel` payloads below `MAX_RAW_DIRECT_EXECUTE_THRESHOLD` (1024 bytes)
through `webview.eval`, with the bytes spelled out as a JSON array of decimal numbers and
dispatched on the GTK main loop. An unbatched interactive PTY yields 20-200 byte reads, so
without coalescing *every frame* takes that path — and this table is what that costs:
9.4 MiB/s instead of 228.6.

That is the empirical justification for `cide-pty`'s flush rule:

```rust
pub const FLUSH_BYTES: usize    = 8 * 1024;              // ≥8 KiB, or…
pub const FLUSH_INTERVAL: Duration = Duration::from_millis(8);   // …8 ms, whichever first
pub const MAX_FRAME: usize      = 64 * 1024;             // hard cap
```

8 KiB is the first size comfortably clear of the threshold (78 MiB/s pull / 62 MiB/s push),
and 64 KiB is where the curve flattens — past it, latency grows without buying throughput.
The 8 ms timer exists so an idle prompt still appears promptly rather than waiting for a
buffer that will never fill.

## Caveats

- `webkit: 605.1.15` is the AppleWebKit compatibility token in the user-agent string, not
  the engine version. The real engine here is WebKitGTK **2.52.3** (`pkg-config
  --modversion webkit2gtk-4.1`). The report records the UA token because that is what the
  webview will admit to; treat it as a fingerprint, not a version.
- These are debug-build numbers. Release will be faster, so the gate is conservative.
- No number here says anything about a machine where `custom protocol : NO`. That is the
  silent-degradation case (WebKitGTK < 2.40, or a CSP that blocks the `ipc:` scheme), and
  the app detects it by shape rather than by timing: over the custom protocol a raw
  response arrives as an `ArrayBuffer`, over `postMessage` the same value arrives as a JSON
  array of numbers.

## Re-run this when

- Tauri, WebKitGTK or xterm.js takes a major version bump.
- `FLUSH_BYTES`, `FLUSH_INTERVAL` or `MAX_FRAME` change.
- Anyone proposes removing the coalescer "because it adds latency".
