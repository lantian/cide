# ADR 0015 — A companion device reads the mirror, and is never told the tree

**Status:** accepted (M72)
**Date:** 2026-09-20

## Context

Every socket cide had before M72 was loopback or a `0600` unix socket, and each was addressed to
a process on this machine: the IDE server to a `claude` cide forked, the hook socket to a binary
cide installed, the agent RPC to a child whose identity comes out of its own environment. M72
opens the first listener another machine may reach, so that a phone can see which sessions are
running, watch one, type into it, and be told when a turn ends.

Three decisions in that feature are the ones a later refactor would undo, each because the
tidier-looking alternative is the one that breaks.

## Decision 1 — the device polls the vt100 mirror; it never attaches a sink

`cide-pty` already fans one child's bytes out to N consumers, and a remote consumer looks exactly
like a detached window's sink. It is not one, and must not become one.

**Back-pressure is structural and deliberate.** A full sink channel blocks the coalescer, then
the reader thread, then the kernel buffer, then `claude` itself. That is correct for a local
window — a webview that cannot keep up *should* slow the producer rather than lose bytes — and it
is catastrophic for a phone on a train, where a three-second stall would stall the agent being
watched. Polling can skip a tick; a sink cannot skip a byte.

Four supporting reasons, any one of which would be enough on its own:

* **`ack` from inside `Sink::deliver` deadlocks the coalescer** — the single thread every byte of
  every session flows through. `cide-pty`'s own comment records M18 doing exactly that. A remote
  sink would have to acknowledge from another thread across a network, which is the same trap
  with more latency in it.
* **A choked sink silently changes its content type**: at its credit limit it begins receiving
  rendered screens in place of the byte stream, which `cide-pty`'s trait doc notes "silently
  corrupts any consumer that is parsing rather than painting". A phone is exactly that consumer.
* **There is no emulator on the far end.** React Native has no xterm.js, and shipping one inside
  a WebView would be a second rendering stack with its own fonts and gesture handling.
* **The coalescer's 8 ms / 8 KiB is tuned for the wrong transport** — a local IPC feeding xterm,
  where a frame is a memcpy. A device wants ten changed rows at ten frames a second.

So `cide_pty::screen` reads the mirror the desktop already maintains and returns styled rows, and
`cide-remote` diffs them per watcher. A device watching nothing costs nothing: no timer runs and
no grid is read.

The cost is stated where it is paid: this is a screen *model*, so OSC 8 hyperlinks, OSC 52, DEC
2026 synchronised framing, sixel and byte fidelity are all gone. Blink and strikethrough are not
lost but unavailable — `vt100::Cell` never recorded them.

## Decision 2 — a device is served projections, never `Workspace`

The obvious design forwards `cide://workspace-changed` to the socket: it is the tree, it is a few
kilobytes, and every window already gets it.

**It carries `Settings`.** `Settings` carries `LlmSettings`, which holds provider API keys in
plaintext — the whole reason `LlmProvider` has a hand-written `Debug` — and `ProxySettings`,
whose URLs carry passwords, which is why `persist`'s own test asserts `workspace.json` is `0600`.
Forwarding that payload would put every credential on the machine onto the LAN, in the one
feature whose premise is that the listener is reachable from another device.

So `cide_ipc::remote` carries `RemoteProject` and `RemoteSession` and cannot name the tree;
`workspace-changed` is teed as a bare revision, and a device answers a revision by asking for a
projection. The protection is structural — the mistake is not expressible in the types — and
`cide-app`'s `a_projection_carries_no_credential` plants a key in a fixture and greps every frame
the host can produce, because "cannot name it" stops being true the first time somebody adds a
convenient field. `contract/protocol.ts` is the transitive closure of the two frame types for the
same reason: a reviewer reading that file is reading the whole wire, and `xtask`'s own test
refuses a closure that reaches `Workspace`, `Settings` or a provider.

## Decision 3 — events reach a device through a tee on `emit.rs`, not a second surface

`AppHandle::emit` reaches this process's webviews and nothing else, so a device cannot subscribe
to any `cide://` event. The tempting fix is to notify the remote server from wherever each event
happens to be produced.

`emit.rs`'s header promises that **every** `cide://` event goes out through one file, and
`cargo xtask contract-check` reads that file textually to enumerate them. A parallel set of
notifications emitted from convenient places would quietly falsify both — the promise and the
gate that keeps it.

The tee is therefore inside `emit.rs`, one line after each `app.emit`, and it is:

* **nameless** — it introduces no `cide://` string, so the contract file is untouched by the
  mechanism and moves only for the genuinely new `cide://remote-changed`;
* **non-blocking** — a bounded channel and `try_send`, because the threads that reach here are
  the hook applier and the GTK main loop, and neither may be made to wait on a phone's socket. A
  full queue is not an error: the device is sent `Desync` and re-reads. That is `cide-pty`'s
  credit protocol's argument, one consumer further out;
* **lazy** — the event is built by a closure, so an installation with the feature off does not
  clone an awaiting set several times a second in order to drop it;
* **absent by default**, which is what makes the previous point the common case.

## Consequences

* A remote client's view can lag the desktop's by up to a tick, and a burst faster than the tick
  is coalesced. Both are wanted.
* `cide-pty` gains a second reader of its mirror, so the scrollback walk's viewport restore is now
  load-bearing for a surface that is not on screen: it sits behind a drop guard and is asserted on
  the *bytes* `contents_formatted` produces, not on `scrollback() == 0`.
* The feature is off by default, binds loopback until told otherwise, refuses a public address
  behind a separate toggle, and listens at all only when there is a paired device or an open
  pairing window. A cide with the feature on and nobody paired is not on the network.
* The finished-unseen marker gains a third surface rather than a second mechanism: a device
  consumes the authoritative set and reports acknowledgements one session at a time, through the
  same `report_awaiting` a pointerdown in a pane title bar takes.

## Alternatives considered

**A second event surface for remote clients.** Rejected above: it breaks the one-file promise and
the textual gate that depends on it.

**Forwarding raw PTY bytes and parsing them on the device.** Rejected with Decision 1. It needs a
terminal emulator on the phone, and the only credible one is xterm.js inside a WebView.

**TLS with a pinned self-signed certificate.** Rejected, and replaced by Decision 4 below.
React Native's `WebSocket` exposes no certificate hook on either platform, so pinning needs an
Android network security config *and* an iOS native module — and pinning per paired instance,
which is what this feature actually requires, needs the native module on both. It would also have
put the first testable version behind a paid Apple account.

## Decision 4 — the frames are sealed, not the socket

Every frame is an XChaCha20-Poly1305 box. The handshake is in the clear because everything in it
is public — an ephemeral public key and a device id — and what it establishes is not:

* `ikm = psk ‖ X25519(e, S)`. The ephemeral–static exchange authenticates the **instance**, whose
  public half the device received when it paired; the pre-shared key authenticates the **device**.
* A client with the wrong key is not *told* it is wrong. It derives a different channel, its first
  frame does not open, and the connection ends. **No credential is ever sent after pairing**,
  which is a stronger statement than any bearer-token scheme can make.
* HKDF yields a key *and a 16-byte nonce prefix* each way. The prefix is derived rather than sent:
  it costs no round trip, and because it is a function of a transcript containing a fresh
  ephemeral key, two connections under one long-lived key can never reuse a nonce. That is the one
  place a hand-rolled framing is silently catastrophic, and there is a test whose whole job is to
  watch it.
* The counter is checked for **exact** succession rather than mere increase. A WebSocket delivers
  in order and drops nothing, so a gap is a bug or an attack and there is no reason to be lenient
  about which. A replay is a counter already used, and the same line refuses it.

The one thing said in the clear after the handshake is a refusal to a device this instance does
not know. That is deliberate: a revoked phone would otherwise get a socket that opens and then
goes silent, which is indistinguishable from a bad network, and *you were removed* is a sentence
somebody can act on. It reveals nothing — the device id was in the handshake the client sent.

The honest costs, since they are real: the device's key is at rest in `remote-devices.json`
rather than only its hash, because it is mixed into every frame rather than presented and
compared; there is no forward secrecy across a re-pair; cide is writing its own framing; and
frame sizes and timing leak to whoever is watching the network. The mitigations are standard
primitives only, a written spec beside the code, and cross-language test vectors when the
companion app exists.
