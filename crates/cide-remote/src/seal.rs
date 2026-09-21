//! Every frame between cide and a paired device is an AEAD box. (M72)
//!
//! # Why the frames and not the socket
//!
//! ADR 0015 has the argument; the short version is that React Native's `WebSocket` exposes no
//! certificate hook on either platform, so pinning a self-signed certificate needs an Android
//! network-security config *and* an iOS native module — and pinning **per paired instance**,
//! which is what this feature actually requires, needs the native module on both. It would also
//! have put the first testable version behind a paid Apple developer account.
//!
//! Sealing the frames instead is pure Rust here and pure JS there, works identically on both
//! platforms, and is *pinning by construction*: only the instance holding the pairing key can
//! produce a frame this opens. There is no PKI, no expiry, no IP SAN, and no "the user tapped
//! Trust once and now everything is trusted".
//!
//! The honest costs, stated here and in the ADR: the key is at rest in `remote-devices.json`
//! rather than only its hash; there is no forward secrecy across a re-pair; cide is writing its
//! own framing; and frame sizes and timing leak to whoever is watching the network.
//!
//! # The shape
//!
//! ```text
//! server → client   "cide-hail" 02 | S_pub[32]                               (once, in the clear)
//! client → server   "cide-seal" 02 | mode | e_pub[32] | len | device-id      (once, in the clear)
//! both ways         counter[8] | XChaCha20-Poly1305(key, prefix ‖ counter, json)
//! ```
//!
//! Both opening messages are in the clear because everything in them is public: two public keys
//! and a device id. What they establish is not. Neither waits for the other — the handshake does
//! not depend on `S` and the greeting does not depend on `e` — so the pair costs no round trip
//! over sending the handshake alone.
//!
//! **The greeting is a convenience and never evidence.** A device that paired by QR already has
//! `S` and compares the greeting against it, refusing by name on a mismatch; a device pairing by
//! a *typed* address has no `S` to compare and adopts the one it is given, which is trust on
//! first use and is exactly as weak as it sounds. What closes that hole is the short
//! authentication string below — not the greeting, which an attacker in the middle writes as
//! easily as cide does.
//!
//! * `ikm = psk ‖ X25519(e, S)` — the ephemeral–static exchange authenticates the **server**
//!   (only the instance holds `S`, whose public half the device got when it paired), and the
//!   pre-shared key authenticates the **device**. A client without the right token derives a
//!   different key, its first frame does not open, and that is the whole of authentication: no
//!   credential is ever sent.
//! * `salt` is a hash of the transcript, so a frame from one handshake cannot be replayed into
//!   another.
//! * HKDF gives four values: a key each way and a 16-byte nonce **prefix** each way. The prefix
//!   is derived rather than sent, so both ends have it without another round trip — and because
//!   it is per connection, two connections under one long-lived key never reuse a nonce. That is
//!   the one place a hand-rolled framing is silently catastrophic, and
//!   [`two_connections_never_reuse_a_nonce`] is what watches it.
//! * The counter is checked for **exact** succession rather than mere increase. A WebSocket
//!   delivers in order and drops nothing, so a gap is a bug or an attack and there is no reason
//!   to be lenient about which.
//! * A sixth value falls out of the same expansion: a **short authentication string**, six
//!   digits, shown on both screens so a person can compare them. It answers the one question the
//!   exchange cannot answer for itself — *is the far end cide, or somebody relaying?* An attacker
//!   in the middle holds two conversations, with its own `S` towards the device and its own `e`
//!   towards cide, so the two transcripts differ and so do the two numbers. It is expanded from
//!   the HKDF output rather than hashed from the transcript alone, which costs nothing and binds
//!   it to the Diffie-Hellman result as well as to what was said.
//!
//!   It is shown **only where the key was not already pinned**, which is the typed road. A QR
//!   carries `S`, so a scanned pairing is authenticated by the key itself, and asking for a
//!   comparison there would be theatre — worse than theatre, because a confirmation people are
//!   asked for when it cannot fail is one they have stopped reading by the time it can.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroize;

use crate::RemoteError;

/// The first bytes of a handshake, so a frame that is not one is refused rather than parsed.
pub const MAGIC: &[u8] = b"cide-seal";

/// The handshake's own version, separate from the protocol's.
///
/// They move for different reasons: a frame can be added without changing how frames are sealed,
/// and the sealing could change without the vocabulary moving. One number for both would force a
/// re-pair for the wrong half of the reason.
///
/// **2 since M74**, when the server began greeting with its public key so that a device pairing
/// by a typed address has something to derive against. The number is inside the transcript, so
/// this bump changes every key and `contract/seal-vectors.json` moves with it — which is the
/// intended signal, and the reason both ends check the constant rather than assume it.
pub const SEAL_VERSION: u8 = 2;

/// The first bytes of a greeting. A different tag from [`MAGIC`] on purpose: the two opening
/// messages cross in flight, so a client that read its own handshake echoed back — or a server
/// handed a greeting — must fail as *the wrong message* rather than as a corrupt one.
pub const HAIL: &[u8] = b"cide-hail";

/// What the server says first: which sealing it speaks, and the key to derive against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Greeting {
    pub seal_version: u8,
    pub server_public: [u8; 32],
}

impl Greeting {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HAIL.len() + 33);
        out.extend_from_slice(HAIL);
        out.push(self.seal_version);
        out.extend_from_slice(&self.server_public);
        out
    }

    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let rest = bytes.strip_prefix(HAIL)?;
        let (&seal_version, rest) = rest.split_first()?;
        // Exactly, not at least — [`Handshake::decode`]'s rule, for its reason.
        if rest.len() != 32 {
            return None;
        }
        Some(Self {
            seal_version,
            server_public: rest.try_into().ok()?,
        })
    }
}

/// What a connection is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A device with no key yet, redeeming a pairing code.
    ///
    /// The channel still authenticates the *server* — only the instance can complete the
    /// exchange against the public key in the pairing payload — which is what keeps the token it
    /// hands back off the wire in the clear. The device is authenticated by the code, inside.
    Pair,
    /// A paired device, proving it by being able to speak at all.
    Resume,
}

impl Mode {
    fn byte(self) -> u8 {
        match self {
            Self::Pair => 0,
            Self::Resume => 1,
        }
    }

    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Pair),
            1 => Some(Self::Resume),
            _ => None,
        }
    }
}

/// The instance's long-lived key.
///
/// Its public half goes in the pairing payload; its private half never leaves this file. It is
/// what makes a device able to tell *this* cide from anything else that answers on that address.
pub struct StaticKey {
    secret: StaticSecret,
    public: PublicKey,
}

impl std::fmt::Debug for StaticKey {
    /// [`crate::Device`]'s rule. The log is the one copy of a secret that outlives the process
    /// and is not mode-protected.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticKey")
            .field("public", &self.public_base64())
            .finish_non_exhaustive()
    }
}

impl StaticKey {
    /// Read the instance's key, minting one the first time.
    ///
    /// `0600`, atomically, with the mode on the file that is *created* so it travels with the
    /// inode through the rename — [`crate::devices`]' rule, for its reason.
    ///
    /// A file that exists and is not 32 bytes is **refused**, not replaced. Minting a new key
    /// there would silently unpair every device, with the symptom that each one fails its
    /// handshake and none of them can say why.
    pub fn load_or_mint(path: &std::path::Path) -> Result<Self, RemoteError> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let raw: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
                    RemoteError::Devices(format!(
                        "{} is not a key — move it aside and every paired device will have to \
                         pair again",
                        path.display()
                    ))
                })?;
                let secret = StaticSecret::from(raw);
                let public = PublicKey::from(&secret);
                Ok(Self { secret, public })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut raw = [0u8; 32];
                getrandom::fill(&mut raw)
                    .map_err(|e| RemoteError::Devices(format!("no entropy: {e}")))?;
                crate::devices::write_private(path, &raw).map_err(|e| {
                    RemoteError::Devices(format!("could not write {}: {e}", path.display()))
                })?;
                let secret = StaticSecret::from(raw);
                let public = PublicKey::from(&secret);
                raw.zeroize();
                Ok(Self { secret, public })
            }
            Err(e) => Err(RemoteError::Devices(format!(
                "{} could not be read: {e}",
                path.display()
            ))),
        }
    }

    /// An in-memory key, for tests and for an instance with no state directory.
    pub fn ephemeral() -> Self {
        let mut raw = [0u8; 32];
        getrandom::fill(&mut raw).expect("the OS has entropy");
        let secret = StaticSecret::from(raw);
        let public = PublicKey::from(&secret);
        raw.zeroize();
        Self { secret, public }
    }

    /// The half that goes in the pairing payload, URL-safe and unpadded so it survives a query
    /// string without escaping.
    pub fn public_base64(&self) -> String {
        base64url(self.public.as_bytes())
    }

    pub fn public_bytes(&self) -> [u8; 32] {
        *self.public.as_bytes()
    }
}

/// What a client sends first, in the clear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    pub mode: Mode,
    pub ephemeral: [u8; 32],
    /// Empty in [`Mode::Pair`]: there is no device yet.
    pub device: String,
}

impl Handshake {
    /// The bytes. Written here as well as read here, so the two spellings cannot drift — and so
    /// the tests can be a client.
    pub fn encode(&self) -> Vec<u8> {
        let id = self.device.as_bytes();
        let mut out = Vec::with_capacity(MAGIC.len() + 35 + id.len());
        out.extend_from_slice(MAGIC);
        out.push(SEAL_VERSION);
        out.push(self.mode.byte());
        out.extend_from_slice(&self.ephemeral);
        // One byte of length, so a device id longer than 255 is refused rather than truncated.
        // Every id cide mints is `d-` plus a uuid.
        out.push(u8::try_from(id.len()).unwrap_or(0));
        out.extend_from_slice(id);
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let rest = bytes.strip_prefix(MAGIC)?;
        let (&version, rest) = rest.split_first()?;
        if version != SEAL_VERSION {
            return None;
        }
        let (&mode, rest) = rest.split_first()?;
        let mode = Mode::from_byte(mode)?;
        if rest.len() < 33 {
            return None;
        }
        let ephemeral: [u8; 32] = rest[..32].try_into().ok()?;
        let len = usize::from(rest[32]);
        let id = rest.get(33..33 + len)?;
        // Exactly, not at least: trailing bytes mean this is not the frame it claims to be.
        if rest.len() != 33 + len {
            return None;
        }
        Some(Self {
            mode,
            ephemeral,
            device: String::from_utf8(id.to_vec()).ok()?,
        })
    }
}

/// One direction's cipher and counter.
struct Lane {
    cipher: XChaCha20Poly1305,
    prefix: [u8; 16],
    counter: u64,
}

impl Lane {
    fn nonce(&self, counter: u64) -> XNonce {
        let mut raw = [0u8; 24];
        raw[..16].copy_from_slice(&self.prefix);
        raw[16..].copy_from_slice(&counter.to_be_bytes());
        *XNonce::from_slice(&raw)
    }
}

/// The sending half.
pub struct Sealer(Lane);

impl Sealer {
    /// `counter ‖ ciphertext`.
    ///
    /// The counter is sent as well as used, although the receiver knows what to expect, because
    /// a frame that names its own position is a frame whose rejection says *which* one was wrong.
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, SealError> {
        let counter = self.0.counter;
        let nonce = self.0.nonce(counter);
        let sealed = self
            .0
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad: &counter.to_be_bytes(),
                },
            )
            .map_err(|_| SealError::Refused)?;
        // Wrapping is not a case that can arise — at a thousand frames a second it is six hundred
        // million years — but a silent wrap would be nonce reuse, so it ends the connection.
        self.0.counter = counter.checked_add(1).ok_or(SealError::Exhausted)?;
        let mut out = Vec::with_capacity(8 + sealed.len());
        out.extend_from_slice(&counter.to_be_bytes());
        out.extend_from_slice(&sealed);
        Ok(out)
    }
}

/// The receiving half.
pub struct Opener(Lane);

impl Opener {
    /// Open a frame, or refuse it.
    ///
    /// **Exact succession**, not mere increase. A WebSocket delivers in order and drops nothing,
    /// so a gap is a bug or an attack and there is no reason to be lenient about which. A replay
    /// is a counter that has already been used, and is refused by the same line.
    pub fn open(&mut self, frame: &[u8]) -> Result<Vec<u8>, SealError> {
        if frame.len() < 8 {
            return Err(SealError::Refused);
        }
        let counter = u64::from_be_bytes(frame[..8].try_into().map_err(|_| SealError::Refused)?);
        if counter != self.0.counter {
            return Err(SealError::OutOfOrder);
        }
        let nonce = self.0.nonce(counter);
        let plaintext = self
            .0
            .cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &frame[8..],
                    aad: &counter.to_be_bytes(),
                },
            )
            .map_err(|_| SealError::Refused)?;
        self.0.counter = counter.checked_add(1).ok_or(SealError::Exhausted)?;
        Ok(plaintext)
    }
}

/// Why a frame was not opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealError {
    /// It did not authenticate: a wrong key, a tampered frame, or a truncated one.
    Refused,
    /// It named a position that is not the next one — a gap, a repeat, or a replay.
    OutOfOrder,
    /// The counter ran out. Unreachable in this universe; see [`Sealer::seal`].
    Exhausted,
}

impl std::fmt::Display for SealError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused => write!(f, "that frame did not authenticate"),
            Self::OutOfOrder => write!(f, "that frame arrived out of order"),
            Self::Exhausted => write!(f, "this connection has sent too many frames"),
        }
    }
}

/// What a handshake establishes: a channel, and a number a person can read out.
///
/// One struct rather than a tuple because the third value is not like the other two — the lanes
/// are used by every frame and the [`Sas`] by at most one screen — and a three-element tuple is
/// where a caller starts getting the order wrong silently.
pub struct Derived {
    pub opener: Opener,
    pub sealer: Sealer,
    pub sas: Sas,
}

/// Six digits, for a person to compare against the six on the other screen.
///
/// Six and not four: four is 10 000 pairs and an attacker in the middle who may retry gets one
/// chance in 10 000 per attempt, which against a person who will re-scan after a failure is not
/// a hard problem. Six is a million, and it is still short enough to hold in your head while you
/// look from a phone to a monitor. Not eight, because a number nobody finishes reading is
/// compared by its first digits only.
///
/// Rendered `418 302` — grouped in threes for the reason a pairing code is grouped in fours, and
/// the one place the spacing is decided so two screens cannot space it differently and make a
/// match look like a mismatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sas(u32);

impl Sas {
    /// Eight bytes down to six digits.
    ///
    /// The modulo bias is real and is about one part in 2^44, which is not a number anybody can
    /// exploit and not one worth a rejection-sampling loop whose failure arm could never be
    /// tested. Taking eight bytes rather than four is what makes it that small: from four it
    /// would be one part in 2^12.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 8]) -> Self {
        Self((u64::from_be_bytes(bytes) % 1_000_000) as u32)
    }

    /// The digits with no spacing, for a test or a wire.
    #[must_use]
    pub fn digits(self) -> String {
        format!("{:06}", self.0)
    }

    /// The digits as a person reads them: `418 302`.
    #[must_use]
    pub fn grouped(self) -> String {
        let d = self.digits();
        format!("{} {}", &d[..3], &d[3..])
    }
}

/// Derive both directions from a handshake.
///
/// `psk` is the device's token for [`Mode::Resume`] and empty for [`Mode::Pair`]. It is mixed
/// into the input keying material rather than used as the salt, so that a wrong token produces a
/// different key rather than a detectable failure at a known point.
pub fn derive(statik: &StaticKey, handshake: &Handshake, psk: &[u8], server_side: bool) -> Derived {
    let shared = statik
        .secret
        .diffie_hellman(&PublicKey::from(handshake.ephemeral));

    // The transcript. Everything either end committed to before a key existed, so a handshake
    // cannot be replayed into a different conversation.
    let mut transcript = blake3::Hasher::new();
    transcript.update(b"cide-remote/seal/1");
    transcript.update(&[SEAL_VERSION, handshake.mode.byte()]);
    transcript.update(&handshake.ephemeral);
    transcript.update(&statik.public_bytes());
    transcript.update(handshake.device.as_bytes());
    let salt = transcript.finalize();

    let mut ikm = Vec::with_capacity(psk.len() + 32);
    ikm.extend_from_slice(psk);
    ikm.extend_from_slice(shared.as_bytes());

    let hkdf = hkdf::Hkdf::<sha2::Sha256>::new(Some(salt.as_bytes()), &ikm);
    ikm.zeroize();

    // A key and a nonce prefix each way, from one expansion. The prefix is **derived rather than
    // sent**: it costs no round trip, and because it is a function of this connection's
    // transcript, two connections under one long-lived key never share one.
    let mut material = [0u8; 104];
    hkdf.expand(b"cide-remote/seal/keys/1", &mut material)
        .expect("104 bytes is inside HKDF-SHA256's output limit");

    let c2s = Lane {
        cipher: XChaCha20Poly1305::new(Key::from_slice(&material[..32])),
        prefix: material[64..80].try_into().expect("16 bytes"),
        counter: 0,
    };
    let s2c = Lane {
        cipher: XChaCha20Poly1305::new(Key::from_slice(&material[32..64])),
        prefix: material[80..96].try_into().expect("16 bytes"),
        counter: 0,
    };
    let sas = Sas::from_bytes(material[96..104].try_into().expect("8 bytes"));
    material.zeroize();

    let (opener, sealer) = if server_side {
        (Opener(c2s), Sealer(s2c))
    } else {
        (Opener(s2c), Sealer(c2s))
    };
    Derived {
        opener,
        sealer,
        sas,
    }
}

/// A client's side of a handshake, for tests and for any client written in Rust.
///
/// The key is a `StaticSecret` in type only. `EphemeralSecret` is the name that fits, and its
/// `diffie_hellman` consumes `self` — which is wrong here, because the caller has to build the
/// handshake from the public half *before* it knows whether there will be an exchange at all. It
/// is minted per connection and dropped with it, which is what "ephemeral" means; the type is
/// about ownership, not lifetime.
pub fn client_handshake(mode: Mode, device: &str) -> (Handshake, StaticSecret) {
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).expect("the OS has entropy");
    let secret = StaticSecret::from(raw);
    raw.zeroize();
    let handshake = Handshake {
        mode,
        ephemeral: *PublicKey::from(&secret).as_bytes(),
        device: device.to_owned(),
    };
    (handshake, secret)
}

/// The client half of [`derive`], given the server's public key.
pub fn derive_client(
    ephemeral: &StaticSecret,
    server_public: &[u8; 32],
    handshake: &Handshake,
    psk: &[u8],
) -> Derived {
    let shared = ephemeral.diffie_hellman(&PublicKey::from(*server_public));

    let mut transcript = blake3::Hasher::new();
    transcript.update(b"cide-remote/seal/1");
    transcript.update(&[SEAL_VERSION, handshake.mode.byte()]);
    transcript.update(&handshake.ephemeral);
    transcript.update(server_public);
    transcript.update(handshake.device.as_bytes());
    let salt = transcript.finalize();

    let mut ikm = Vec::with_capacity(psk.len() + 32);
    ikm.extend_from_slice(psk);
    ikm.extend_from_slice(shared.as_bytes());
    let hkdf = hkdf::Hkdf::<sha2::Sha256>::new(Some(salt.as_bytes()), &ikm);
    ikm.zeroize();

    let mut material = [0u8; 104];
    hkdf.expand(b"cide-remote/seal/keys/1", &mut material)
        .expect("104 bytes is inside HKDF-SHA256's output limit");

    let c2s = Lane {
        cipher: XChaCha20Poly1305::new(Key::from_slice(&material[..32])),
        prefix: material[64..80].try_into().expect("16 bytes"),
        counter: 0,
    };
    let s2c = Lane {
        cipher: XChaCha20Poly1305::new(Key::from_slice(&material[32..64])),
        prefix: material[80..96].try_into().expect("16 bytes"),
        counter: 0,
    };
    let sas = Sas::from_bytes(material[96..104].try_into().expect("8 bytes"));
    material.zeroize();
    Derived {
        opener: Opener(s2c),
        sealer: Sealer(c2s),
        sas,
    }
}

/// URL-safe base64 without padding, so a key survives a query string unescaped.
///
/// Hand-written for [`crate::devices`]' percent-encoder's reason: the whole input set is two
/// 32-byte keys, and a dependency for that is a dependency in the graph of an application that
/// already refuses larger ones for less.
pub fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let take = chunk.len() + 1;
        for i in 0..take {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 0x3f) as usize] as char);
        }
    }
    out
}

/// The inverse. `None` for anything that is not the alphabet.
pub fn from_base64url(text: &str) -> Option<Vec<u8>> {
    let mut acc: u32 = 0;
    let mut bits = 0;
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    for c in text.chars() {
        let value = match c {
            'A'..='Z' => c as u32 - 'A' as u32,
            'a'..='z' => c as u32 - 'a' as u32 + 26,
            '0'..='9' => c as u32 - '0' as u32 + 52,
            '-' => 62,
            '_' => 63,
            _ => return None,
        };
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A paired pair: server key, device token, and both ends of one connection.
    fn channel(psk: &[u8]) -> (StaticKey, Opener, Sealer, Opener, Sealer) {
        let statik = StaticKey::ephemeral();
        let (handshake, ephemeral) = client_handshake(Mode::Resume, "d-test");
        let server = derive(&statik, &handshake, psk, true);
        let (server_open, server_seal) = (server.opener, server.sealer);
        let client = derive_client(&ephemeral, &statik.public_bytes(), &handshake, psk);
        let (client_open, client_seal) = (client.opener, client.sealer);
        (statik, server_open, server_seal, client_open, client_seal)
    }

    #[test]
    fn both_ends_derive_the_same_channel() {
        let (_k, mut server_open, mut server_seal, mut client_open, mut client_seal) =
            channel(b"token");

        let sent = client_seal.seal(b"{\"t\":\"ping\"}").expect("seals");
        assert_eq!(server_open.open(&sent).expect("opens"), b"{\"t\":\"ping\"}");

        let back = server_seal.seal(b"{\"t\":\"pong\"}").expect("seals");
        assert_eq!(client_open.open(&back).expect("opens"), b"{\"t\":\"pong\"}");
    }

    /// The whole of authentication: no credential is sent, and a client without the right one
    /// simply cannot be understood.
    #[test]
    fn a_wrong_token_cannot_be_understood_rather_than_being_told_so() {
        let statik = StaticKey::ephemeral();
        let (handshake, ephemeral) = client_handshake(Mode::Resume, "d-test");
        let mut server_open = derive(&statik, &handshake, b"the-real-token", true).opener;
        let mut client_seal =
            derive_client(&ephemeral, &statik.public_bytes(), &handshake, b"a-guess").sealer;

        let frame = client_seal.seal(b"hello").expect("seals");
        assert_eq!(server_open.open(&frame), Err(SealError::Refused));
    }

    /// The device is talking to *this* cide or to nothing. An impostor on the same address has
    /// the device id and the address and not the static key.
    #[test]
    fn a_server_without_the_static_key_cannot_answer() {
        let real = StaticKey::ephemeral();
        let impostor = StaticKey::ephemeral();
        let (handshake, ephemeral) = client_handshake(Mode::Resume, "d-test");

        let mut impostor_seal = derive(&impostor, &handshake, b"token", true).sealer;
        let mut client_open =
            derive_client(&ephemeral, &real.public_bytes(), &handshake, b"token").opener;

        let frame = impostor_seal.seal(b"welcome").expect("seals");
        assert_eq!(client_open.open(&frame), Err(SealError::Refused));
    }

    /// **The one place a hand-rolled framing is silently catastrophic.**
    ///
    /// The counter restarts at zero on every connection, so if the rest of the nonce were fixed
    /// per key, two connections would encrypt different plaintexts under the same nonce — which
    /// for a stream cipher hands an observer their xor. The prefix is derived from the
    /// transcript, and the transcript contains a fresh ephemeral public key, so it cannot repeat.
    #[test]
    fn two_connections_never_reuse_a_nonce() {
        let statik = StaticKey::ephemeral();
        let mut seen = std::collections::HashSet::new();

        for _ in 0..32 {
            let (handshake, _) = client_handshake(Mode::Resume, "d-test");
            let mut sealer = derive(&statik, &handshake, b"one-token", true).sealer;
            // The first four nonces of each connection, which are the ones that would collide.
            for _ in 0..4 {
                let frame = sealer.seal(b"x").expect("seals");
                let counter = u64::from_be_bytes(frame[..8].try_into().expect("8 bytes"));
                assert!(
                    seen.insert((sealer.0.prefix, counter)),
                    "a nonce repeated across two connections"
                );
            }
        }
    }

    /// Exact succession, not mere increase. A WebSocket delivers in order and drops nothing.
    #[test]
    fn a_replayed_or_reordered_frame_is_refused() {
        let (_k, mut server_open, _ss, _co, mut client_seal) = channel(b"token");

        let first = client_seal.seal(b"one").expect("seals");
        let second = client_seal.seal(b"two").expect("seals");

        // Out of order.
        assert_eq!(server_open.open(&second), Err(SealError::OutOfOrder));
        // In order.
        assert_eq!(server_open.open(&first).expect("opens"), b"one");
        assert_eq!(server_open.open(&second).expect("opens"), b"two");
        // And a replay of one already accepted.
        assert_eq!(server_open.open(&first), Err(SealError::OutOfOrder));
    }

    #[test]
    fn a_tampered_frame_is_refused() {
        let (_k, mut server_open, _ss, _co, mut client_seal) = channel(b"token");
        let mut frame = client_seal.seal(b"{\"t\":\"ping\"}").expect("seals");
        let last = frame.len() - 1;
        frame[last] ^= 1;
        assert_eq!(server_open.open(&frame), Err(SealError::Refused));

        // Including the counter, which is authenticated as associated data rather than merely
        // read — otherwise an observer could renumber frames and the cipher would not notice.
        let mut renumbered = client_seal.seal(b"x").expect("seals");
        renumbered[7] = 0;
        assert!(server_open.open(&renumbered).is_err());
    }

    #[test]
    fn a_truncated_frame_is_refused_rather_than_panicking() {
        let (_k, mut server_open, _ss, _co, mut client_seal) = channel(b"token");
        let frame = client_seal.seal(b"hello").expect("seals");
        for cut in 0..frame.len() {
            assert!(server_open.open(&frame[..cut]).is_err(), "cut at {cut}");
        }
    }

    // --- the handshake --------------------------------------------------------------------

    #[test]
    fn a_handshake_round_trips_and_refuses_anything_else() {
        let (handshake, _) = client_handshake(Mode::Pair, "");
        let bytes = handshake.encode();
        assert_eq!(Handshake::decode(&bytes).expect("decodes"), handshake);

        let (resume, _) = client_handshake(Mode::Resume, "d-abc");
        assert_eq!(
            Handshake::decode(&resume.encode()).expect("decodes"),
            resume
        );

        assert!(Handshake::decode(b"not a handshake").is_none());
        assert!(Handshake::decode(&[]).is_none());
        // A wrong version is refused rather than reinterpreted.
        let mut wrong = bytes.clone();
        wrong[MAGIC.len()] = SEAL_VERSION + 1;
        assert!(Handshake::decode(&wrong).is_none());
        // Trailing bytes mean this is not the frame it claims to be.
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(Handshake::decode(&extra).is_none());
        // And a truncation at every length.
        for cut in 0..bytes.len() {
            assert!(Handshake::decode(&bytes[..cut]).is_none(), "cut at {cut}");
        }
    }

    // --- the instance key -----------------------------------------------------------------

    #[test]
    fn an_instance_key_is_minted_once_and_kept() {
        let dir = std::env::temp_dir().join(format!("cide-seal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("creates");
        let path = dir.join("remote-key");

        let first = StaticKey::load_or_mint(&path).expect("mints");
        let again = StaticKey::load_or_mint(&path).expect("reads");
        assert_eq!(first.public_bytes(), again.public_bytes());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the instance key is a secret");
        }

        // A file that is not a key is refused, not replaced: minting over it would unpair every
        // device with no message anywhere.
        std::fs::write(&path, b"nonsense").expect("writes");
        let refusal = StaticKey::load_or_mint(&path).expect_err("refuses");
        assert!(refusal.to_string().contains("pair again"), "{refusal}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_debug_rendering_prints_the_private_half() {
        let key = StaticKey::ephemeral();
        let rendering = format!("{key:?}");
        assert!(rendering.contains(&key.public_base64()));
        // The secret has no accessor at all, so the strongest available statement is that the
        // rendering is the public half and a marker.
        assert!(rendering.contains(".."), "{rendering}");
    }

    // --- base64url ------------------------------------------------------------------------

    #[test]
    fn base64url_round_trips_and_stays_url_safe() {
        for len in 0..40 {
            let mut bytes = vec![0u8; len];
            getrandom::fill(&mut bytes).expect("entropy");
            let text = base64url(&bytes);
            assert!(
                text.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "{text}"
            );
            assert_eq!(from_base64url(&text).expect("decodes"), bytes);
        }
        assert!(from_base64url("not/base64url+").is_none());
    }
    #[test]
    fn an_honest_exchange_agrees_on_the_digits() {
        // The premise. Without this the comparison is noise and people learn to ignore it.
        let statik = StaticKey::ephemeral();
        let (handshake, ephemeral) = client_handshake(Mode::Pair, "");
        let server = derive(&statik, &handshake, b"", true);
        let client = derive_client(&ephemeral, &statik.public_bytes(), &handshake, b"");
        assert_eq!(server.sas, client.sas);
        assert_eq!(server.sas.digits().len(), 6);
    }

    #[test]
    fn somebody_in_the_middle_makes_the_two_screens_disagree() {
        // The whole feature, and the only test that can fail if the short authentication string
        // were derived from something an attacker controls on both sides.
        //
        // The relay runs two exchanges: towards the device it offers its own static key, and
        // towards cide it offers its own ephemeral key. It can read and re-encrypt every frame,
        // so nothing else in this module can see it. What it cannot do is make the two
        // transcripts agree, because each end's transcript names the key it actually spoke to.
        let real = StaticKey::ephemeral();
        let relay = StaticKey::ephemeral();

        // The device's leg: it believes `relay` is cide.
        let (device_handshake, device_ephemeral) = client_handshake(Mode::Pair, "");
        let on_the_phone = derive_client(
            &device_ephemeral,
            &relay.public_bytes(),
            &device_handshake,
            b"",
        )
        .sas;

        // The relay's leg to the real cide, with an ephemeral key of its own.
        let (relay_handshake, _) = client_handshake(Mode::Pair, "");
        let on_the_screen = derive(&real, &relay_handshake, b"", true).sas;

        assert_ne!(
            on_the_phone, on_the_screen,
            "a relayed pairing must not show the same digits on both screens"
        );
    }

    #[test]
    fn the_digits_are_grouped_in_one_place_and_keep_their_leading_zeros() {
        // Two screens spacing it differently would make a match read as a mismatch, and a
        // dropped leading zero would make `042 to 7` — so both are decided here and asserted.
        assert_eq!(Sas::from_bytes([0; 8]).digits(), "000000");
        assert_eq!(Sas::from_bytes([0; 8]).grouped(), "000 000");
        let sas = Sas::from_bytes(u64::to_be_bytes(1_418_302));
        assert_eq!(sas.digits(), "418302");
        assert_eq!(sas.grouped(), "418 302");
        for n in [0u64, 1, 999_999, 1_000_000, u64::MAX] {
            assert_eq!(Sas::from_bytes(u64::to_be_bytes(n)).digits().len(), 6);
        }
    }

    #[test]
    fn a_greeting_round_trips_and_refuses_anything_that_is_not_one() {
        let greeting = Greeting {
            seal_version: SEAL_VERSION,
            server_public: StaticKey::ephemeral().public_bytes(),
        };
        let bytes = greeting.encode();
        assert_eq!(Greeting::decode(&bytes), Some(greeting));

        // A handshake is not a greeting. The two cross in flight, so each end must fail on the
        // other's message as *the wrong message* — a shared tag would make one decode as a
        // truncated version of the other.
        let (handshake, _) = client_handshake(Mode::Pair, "d-1");
        assert_eq!(Greeting::decode(&handshake.encode()), None);
        assert_eq!(Handshake::decode(&bytes), None);

        assert_eq!(Greeting::decode(b""), None);
        assert_eq!(Greeting::decode(&bytes[..bytes.len() - 1]), None, "short");
        let mut long = bytes.clone();
        long.push(0);
        assert_eq!(Greeting::decode(&long), None, "trailing bytes");
    }
}
