//! Who may connect, and the one-time codes that let a new device become one of them. (M72)
//!
//! # The file
//!
//! `remote-devices.json` in the profile's state directory, mode `0600`, written atomically.
//! **Not in `workspace.json`**, and the four reasons are each independently sufficient:
//!
//! 1. `SettingsPatch` patches per *top-level field*, so a Settings screen editing anything in a
//!    group sends the whole group back. A write-only credential is therefore not representable
//!    there — the screen would have to hold the real token in order to rename a device.
//! 2. `workspace.json` is broadcast whole to every window on every accepted mutation. Anything
//!    in it is in every webview's memory.
//! 3. `persist::load` moves a workspace it cannot parse aside and returns a default — deliberately,
//!    so a broken layout is never a launch loop. A device list silently emptied by an unrelated
//!    corrupt tree is a phone that stops working with nothing anywhere saying why.
//! 4. `run.sh --fresh` moves `workspace.json` to `.bak`. Losing a layout is the point of that
//!    flag; losing your paired phone is not.
//!
//! # The key, and why it is stored whole
//!
//! 32 bytes from the OS CSPRNG. The argument against a UUID is `cide_ide_mcp::lockfile`'s and is
//! worth repeating, because a UUID is the obvious thing to reach for — a UUID's type contract is
//! *uniqueness*, not unpredictability, so a later "let us use v7 so they sort" turns the
//! credential into a timestamp and a counter.
//!
//! It is kept **whole** rather than as a hash, and that is a real cost written down rather than
//! hidden: a bearer token can be stored hashed because it is *presented* and compared, and this
//! is not presented at all. It is a pre-shared key mixed into every frame's encryption
//! ([`crate::seal`]), so the instance must be able to derive with it. What is bought for that
//! cost is that **no credential is ever sent after pairing** — a device proves who it is by
//! being understandable, and there is nothing on the wire for anybody to take.
//!
//! # The code
//!
//! Eight characters of Crockford base32 with `I`, `L`, `O` and `U` removed — about 40 bits, which
//! is strong against *one* guess and nothing more. So: one outstanding code at a time, two
//! minutes to use it, single use, and **a wrong code burns the code**. A retry budget is exactly
//! what would make a short code guessable, and offering one is how a mechanism that looks careful
//! stops being careful. Repeated failures lock pairing out entirely for a while, and the panel
//! says so in words rather than silently refusing.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

use crate::RemoteError;

/// How long a pairing code is good for.
pub const CODE_TTL: Duration = Duration::from_secs(120);
/// Characters in a pairing code.
pub const CODE_LEN: usize = 8;
/// Failed redemptions inside [`FAILURE_WINDOW`] before pairing is locked out.
pub const FAILURE_LIMIT: usize = 3;
pub const FAILURE_WINDOW: Duration = Duration::from_secs(60);
pub const LOCKOUT: Duration = Duration::from_secs(300);

/// Crockford base32 without `I`, `L`, `O` and `U`.
///
/// The first three because they are read back as `1`, `1` and `0` off a screen at arm's length,
/// and `U` because Crockford excludes it to keep the alphabet from spelling things at the user.
const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

const KEY_BYTES: usize = 32;

/// One paired device, as the file keeps it.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: String,
    pub platform: String,
    /// Hex of the 32-byte pre-shared key this device seals its frames with.
    ///
    /// Whole, not hashed. See the module header for what that costs and what it buys.
    pub seal_key: String,
    pub created_unix_ms: u64,
    #[serde(default)]
    pub last_seen_unix_ms: u64,
    #[serde(default)]
    pub last_addr: String,
}

/// Hand-written so no `tracing` call and no `{:?}` can put a credential in a log.
///
/// The hash is not the token, but it is the only thing standing between a log and the token, and
/// a log is the one copy of a secret that outlives the process and is not mode-protected. This is
/// [`cide_ipc::LlmProvider`]'s rule and `cide_ide_mcp::lockfile::Lockfile`'s, in a third place.
impl fmt::Debug for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Device")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("platform", &self.platform)
            .field("seal_key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    #[serde(default = "one")]
    schema: u32,
    #[serde(default)]
    devices: Vec<Device>,
    /// The port this instance last managed to bind.
    ///
    /// Remembered so a collision is self-healing: the derived port is a preference, and once a
    /// second instance has walked the band to a free one, every device it paired knows that
    /// number and it must keep it.
    #[serde(default)]
    port: Option<u16>,
}

fn one() -> u32 {
    1
}

struct Pending {
    code: String,
    minted: Instant,
}

#[derive(Default)]
struct Failures {
    recent: Vec<Instant>,
    locked_until: Option<Instant>,
}

/// The device list, and the pairing window when one is open.
pub struct DeviceStore {
    path: PathBuf,
    inner: Mutex<Inner>,
}

struct Inner {
    stored: Stored,
    pending: Option<Pending>,
    failures: Failures,
}

/// Deliberately opaque. See [`Device`]'s `Debug`.
impl fmt::Debug for DeviceStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let inner = self.inner.lock();
        f.debug_struct("DeviceStore")
            .field("path", &self.path)
            .field("devices", &inner.stored.devices.len())
            .field("pairing", &inner.pending.is_some())
            .finish_non_exhaustive()
    }
}

impl DeviceStore {
    /// Read the file, or start empty.
    ///
    /// A file that cannot be parsed is **refused**, not replaced: unlike a workspace, this is not
    /// something the user can rebuild by rearranging panes, and quietly starting empty would
    /// unpair every device with no message. The caller reports it and the panel says so.
    pub fn load(path: PathBuf) -> Result<Self, RemoteError> {
        let stored = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Stored>(&bytes).map_err(|e| {
                RemoteError::Devices(format!("{} is not readable: {e}", path.display()))
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Stored::default(),
            Err(e) => {
                return Err(RemoteError::Devices(format!(
                    "{} could not be read: {e}",
                    path.display()
                )));
            }
        };
        Ok(Self {
            path,
            inner: Mutex::new(Inner {
                stored,
                pending: None,
                failures: Failures::default(),
            }),
        })
    }

    /// An in-memory store, for tests and for a host that has no state directory.
    pub fn ephemeral() -> Self {
        Self {
            path: PathBuf::new(),
            inner: Mutex::new(Inner {
                stored: Stored::default(),
                pending: None,
                failures: Failures::default(),
            }),
        }
    }

    pub fn devices(&self) -> Vec<Device> {
        self.inner.lock().stored.devices.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.lock().stored.devices.is_empty()
    }

    pub fn remembered_port(&self) -> Option<u16> {
        self.inner.lock().stored.port
    }

    pub fn remember_port(&self, port: u16) -> Result<(), RemoteError> {
        let mut inner = self.inner.lock();
        if inner.stored.port == Some(port) {
            return Ok(());
        }
        inner.stored.port = Some(port);
        self.write(&inner.stored)
    }

    /// Open a pairing window, replacing any code already outstanding.
    ///
    /// Replacing rather than refusing: the gesture is *show me a code*, and a user who pressed it
    /// twice wants the one now on screen to work. Two live codes would double the guessing
    /// surface for no benefit.
    pub fn begin_pairing(&self) -> Result<String, RemoteError> {
        let mut inner = self.inner.lock();
        if let Some(until) = inner.failures.locked_until
            && until > Instant::now()
        {
            return Err(RemoteError::Pairing(
                "pairing is locked for a few minutes after repeated bad codes".to_owned(),
            ));
        }
        let code = mint_code()?;
        inner.pending = Some(Pending {
            code: code.clone(),
            minted: Instant::now(),
        });
        Ok(code)
    }

    pub fn cancel_pairing(&self) {
        self.inner.lock().pending = None;
    }

    /// The code now outstanding, if it has not expired.
    pub fn pending_code(&self) -> Option<String> {
        let inner = self.inner.lock();
        let pending = inner.pending.as_ref()?;
        (pending.minted.elapsed() < CODE_TTL).then(|| pending.code.clone())
    }

    /// Redeem a code for a device and its key. The key is returned **once**.
    pub fn redeem(
        &self,
        code: &str,
        name: &str,
        platform: &str,
    ) -> Result<(String, String), RemoteError> {
        let mut inner = self.inner.lock();
        let now = Instant::now();

        if let Some(until) = inner.failures.locked_until
            && until > now
        {
            return Err(RemoteError::Pairing(
                "pairing is locked for a few minutes after repeated bad codes".to_owned(),
            ));
        }

        // Taken, not borrowed: a code is single use, and a wrong guess burns it too. Both are the
        // same statement — after this line there is no outstanding code — and writing it once is
        // what stops a later edit from making the failure path lenient.
        let Some(pending) = inner.pending.take() else {
            note_failure(&mut inner.failures, now);
            return Err(RemoteError::Pairing("no pairing code is open".to_owned()));
        };
        if pending.minted.elapsed() >= CODE_TTL {
            note_failure(&mut inner.failures, now);
            return Err(RemoteError::Pairing(
                "that pairing code has expired".to_owned(),
            ));
        }
        let offered = normalise_code(code);
        if offered
            .as_bytes()
            .ct_eq(pending.code.as_bytes())
            .unwrap_u8()
            != 1
        {
            note_failure(&mut inner.failures, now);
            return Err(RemoteError::Pairing(
                "that pairing code is not right".to_owned(),
            ));
        }

        inner.failures = Failures::default();
        let key = mint_key()?;
        let device = Device {
            id: format!("d-{}", uuid::Uuid::new_v4()),
            name: name.trim().to_owned(),
            platform: platform.trim().to_owned(),
            seal_key: key.clone(),
            created_unix_ms: unix_ms(),
            last_seen_unix_ms: 0,
            last_addr: String::new(),
        };
        let id = device.id.clone();
        inner.stored.devices.push(device);
        self.write(&inner.stored)?;
        Ok((id, key))
    }

    /// The key a device seals with, or `None` when it is not paired — which is also what a
    /// revoked device gets, because revoking is removing.
    ///
    /// Constant-time is not a question here: the answer is a lookup by a public id, and the
    /// secret is never *compared* against anything. A device that has the wrong key does not
    /// fail a check; it produces frames this instance cannot read, which is [`crate::seal`]'s
    /// whole point.
    pub fn seal_key(&self, device: &str) -> Option<[u8; 32]> {
        let inner = self.inner.lock();
        let known = inner.stored.devices.iter().find(|d| d.id == device)?;
        let mut out = [0u8; 32];
        let hex = known.seal_key.as_bytes();
        if hex.len() != 64 {
            return None;
        }
        for (i, pair) in hex.chunks(2).enumerate() {
            let byte = std::str::from_utf8(pair).ok()?;
            out[i] = u8::from_str_radix(byte, 16).ok()?;
        }
        Some(out)
    }

    /// Whether this device is paired at all.
    pub fn knows(&self, device: &str) -> bool {
        self.inner
            .lock()
            .stored
            .devices
            .iter()
            .any(|d| d.id == device)
    }

    /// Record that a device was seen. Best effort — a failed write must not fail a connection.
    pub fn note_seen(&self, device: &str, addr: &str) {
        let mut inner = self.inner.lock();
        let Some(known) = inner.stored.devices.iter_mut().find(|d| d.id == device) else {
            return;
        };
        known.last_seen_unix_ms = unix_ms();
        known.last_addr = addr.to_owned();
        let stored = std::mem::take(&mut inner.stored);
        if let Err(error) = self.write(&stored) {
            tracing::debug!(%error, "remote: could not record a device's last-seen");
        }
        inner.stored = stored;
    }

    pub fn rename(&self, device: &str, name: &str) -> Result<(), RemoteError> {
        let mut inner = self.inner.lock();
        let Some(known) = inner.stored.devices.iter_mut().find(|d| d.id == device) else {
            return Err(RemoteError::Devices("no such device".to_owned()));
        };
        known.name = name.trim().to_owned();
        let stored = std::mem::take(&mut inner.stored);
        let result = self.write(&stored);
        inner.stored = stored;
        result
    }

    /// Revoke a device. Returns whether there was one to revoke.
    pub fn forget(&self, device: &str) -> Result<bool, RemoteError> {
        let mut inner = self.inner.lock();
        let before = inner.stored.devices.len();
        inner.stored.devices.retain(|d| d.id != device);
        let removed = inner.stored.devices.len() != before;
        if removed {
            let stored = std::mem::take(&mut inner.stored);
            let result = self.write(&stored);
            inner.stored = stored;
            result?;
        }
        Ok(removed)
    }

    fn write(&self, stored: &Stored) -> Result<(), RemoteError> {
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        let json = serde_json::to_vec_pretty(stored)
            .map_err(|e| RemoteError::Devices(format!("could not encode the device list: {e}")))?;
        write_private(&self.path, &json).map_err(|e| {
            RemoteError::Devices(format!("could not write {}: {e}", self.path.display()))
        })
    }
}

fn note_failure(failures: &mut Failures, now: Instant) {
    failures
        .recent
        .retain(|at| now.duration_since(*at) < FAILURE_WINDOW);
    failures.recent.push(now);
    if failures.recent.len() >= FAILURE_LIMIT {
        failures.locked_until = Some(now + LOCKOUT);
        failures.recent.clear();
    }
}

/// Uppercase, and with the grouping dash and any stray space removed.
///
/// The code is shown as `XXXX-XXXX` because eight unbroken characters are hard to read off a
/// screen and easy to mistype. Whoever types it back may or may not type the dash, and refusing
/// them over punctuation cide added for legibility would be refusing them over nothing.
pub fn normalise_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// `XXXX-XXXX`, from the OS CSPRNG.
fn mint_code() -> Result<String, RemoteError> {
    let mut bytes = [0u8; CODE_LEN];
    getrandom::fill(&mut bytes).map_err(|e| RemoteError::Devices(format!("no entropy: {e}")))?;
    // Rejection is unnecessary: the alphabet is exactly 32 long, so a byte's low five bits are
    // uniform over it. Taking a modulo of a non-power-of-two alphabet is the version of this that
    // is subtly biased, which is worth a sentence because the fix looks like an accident.
    Ok(bytes
        .iter()
        .map(|b| ALPHABET[usize::from(b & 0x1f)] as char)
        .collect::<String>())
}

fn mint_key() -> Result<String, RemoteError> {
    let mut bytes = [0u8; KEY_BYTES];
    getrandom::fill(&mut bytes).map_err(|e| RemoteError::Devices(format!("no entropy: {e}")))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// How a code is shown to a person.
pub fn grouped(code: &str) -> String {
    let code = normalise_code(code);
    if code.len() == CODE_LEN {
        format!("{}-{}", &code[..4], &code[4..])
    } else {
        code
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Write `0600`, atomically, with the mode on the **temp file**.
///
/// `cide_core::persist::write_atomic_with_mode` does exactly this and this crate cannot call it:
/// `cide-remote` deliberately does not depend on `cide-core`. The rule it copies is the one that
/// matters — the mode goes on the file that is *created*, so it travels with the inode through
/// the rename. A `chmod` after the rename leaves a window in which the file is readable, and that
/// window is the whole reason to care.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    {
        let mut file = create_private(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(unix)]
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::create(path)
}

#[cfg(test)]
impl DeviceStore {
    /// Age the outstanding code past its TTL, so expiry is testable without a two-minute sleep.
    ///
    /// A test-only door rather than an injectable clock: the clock is read in exactly one place
    /// and a `Duration` parameter threaded through [`DeviceStore`] for one assertion would be a
    /// wider surface than the thing it proves.
    pub(crate) fn expire_pending_for_test(&self) {
        let mut inner = self.inner.lock();
        if let Some(pending) = inner.pending.as_mut() {
            pending.minted = Instant::now() - CODE_TTL - Duration::from_secs(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_eight_characters_of_an_alphabet_that_cannot_be_misread() {
        let store = DeviceStore::ephemeral();
        for _ in 0..64 {
            let code = store.begin_pairing().expect("mints");
            assert_eq!(code.len(), CODE_LEN);
            for c in code.chars() {
                assert!(ALPHABET.contains(&(c as u8)), "{code} contains {c}");
                assert!(
                    !"ILOU".contains(c),
                    "{code} contains a character read as another"
                );
            }
            assert_eq!(grouped(&code).len(), CODE_LEN + 1);
        }
    }

    /// The dash exists so eight characters can be read off a screen. Refusing somebody for
    /// typing it, or for not typing it, would be refusing them over punctuation cide added.
    #[test]
    fn a_code_is_accepted_however_it_is_typed_back() {
        let store = DeviceStore::ephemeral();
        let code = store.begin_pairing().expect("mints");
        let typed = format!(" {} ", grouped(&code).to_lowercase());
        assert!(store.redeem(&typed, "Pixel", "android").is_ok());
    }

    #[test]
    fn a_code_works_once() {
        let store = DeviceStore::ephemeral();
        let code = store.begin_pairing().expect("mints");
        assert!(store.redeem(&code, "first", "android").is_ok());
        assert!(store.redeem(&code, "second", "android").is_err());
        assert_eq!(store.devices().len(), 1);
    }

    /// A redeemed code stops being outstanding, and that is what closes cide's pairing modal.
    ///
    /// The panel polls [`DeviceStore::pending_code`] through `remote_pairing_progress` and takes
    /// `None` as *this window is over*. Nothing is emitted when a code is taken — the frame that
    /// wakes the panel is the one about a *device arriving* — so if this went on answering
    /// `Some` the QR would stay on screen after the phone it was for had finished with it, which
    /// is exactly what it did.
    #[test]
    fn a_redeemed_code_is_no_longer_outstanding() {
        let store = DeviceStore::ephemeral();
        let code = store.begin_pairing().expect("mints");
        assert_eq!(store.pending_code().as_deref(), Some(code.as_str()));
        store.redeem(&code, "phone", "android").expect("redeems");
        assert_eq!(store.pending_code(), None);
    }

    /// And so does a wrong guess, for the same reader: a burnt code is not one to keep showing.
    #[test]
    fn a_wrong_guess_leaves_no_code_outstanding() {
        let store = DeviceStore::ephemeral();
        store.begin_pairing().expect("mints");
        assert!(store.redeem("AAAAAAAA", "attacker", "unknown").is_err());
        assert_eq!(store.pending_code(), None);
    }

    #[test]
    fn repeated_bad_codes_lock_pairing_out_and_say_so() {
        let store = DeviceStore::ephemeral();
        for _ in 0..FAILURE_LIMIT {
            store.begin_pairing().expect("mints");
            assert!(store.redeem("AAAAAAAA", "attacker", "unknown").is_err());
        }
        let refusal = store.begin_pairing().expect_err("locked out");
        assert!(refusal.to_string().contains("locked"), "{refusal}");
    }

    /// A successful pairing clears the record, or a user who mistypes twice and then succeeds
    /// would be locked out by their own success.
    #[test]
    fn a_success_forgives_the_earlier_mistypes() {
        let store = DeviceStore::ephemeral();
        for _ in 0..FAILURE_LIMIT - 1 {
            store.begin_pairing().expect("mints");
            assert!(
                store
                    .redeem("AAAAAAAA", "butterfingers", "android")
                    .is_err()
            );
        }
        let code = store.begin_pairing().expect("mints");
        assert!(store.redeem(&code, "Pixel", "android").is_ok());

        for _ in 0..FAILURE_LIMIT - 1 {
            store.begin_pairing().expect("mints");
            assert!(
                store
                    .redeem("AAAAAAAA", "butterfingers", "android")
                    .is_err()
            );
        }
        assert!(store.begin_pairing().is_ok(), "the count was never cleared");
    }

    /// The key is stored **whole**, which is the cost the module header writes down — and in
    /// return nothing is ever presented, so there is no comparison to get wrong and nothing on
    /// the wire to take.
    #[test]
    fn a_key_is_looked_up_by_a_public_id_and_never_compared() {
        let store = DeviceStore::ephemeral();
        let code = store.begin_pairing().expect("mints");
        let (device, key) = store.redeem(&code, "Pixel", "android").expect("pairs");

        assert!(store.knows(&device));
        assert!(!store.knows("d-nobody"));

        let raw = store.seal_key(&device).expect("a key");
        assert_eq!(raw.len(), KEY_BYTES);
        assert_eq!(
            raw.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            key,
            "the stored key is not the one the device was given"
        );
        // Revoking is removing, so a revoked device cannot derive a channel at all.
        assert_eq!(store.seal_key("d-nobody"), None);
        store.forget(&device).expect("writes");
        assert_eq!(store.seal_key(&device), None);
    }

    /// The log is the one copy of a secret that outlives the process and is not mode-protected.
    #[test]
    fn no_debug_rendering_prints_a_credential() {
        let store = DeviceStore::ephemeral();
        let code = store.begin_pairing().expect("mints");
        let (device, key) = store.redeem(&code, "Pixel", "android").expect("pairs");
        let known = store.devices().into_iter().next().expect("one device");

        for rendering in [
            format!("{store:?}"),
            format!("{known:?}"),
            format!("{:?}", store.devices()),
        ] {
            assert!(!rendering.contains(&key), "{rendering}");
            assert!(!rendering.contains(&known.seal_key), "{rendering}");
        }
        assert!(format!("{known:?}").contains(&device));
    }

    /// A device list that will not parse is refused rather than replaced. Starting empty would
    /// unpair every device with nothing anywhere saying why, and unlike a workspace there is no
    /// gesture that rebuilds it.
    #[test]
    fn an_unreadable_device_list_refuses_rather_than_starting_empty() {
        let dir = std::env::temp_dir().join(format!("cide-remote-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("creates");
        let path = dir.join("remote-devices.json");
        std::fs::write(&path, b"{ this is not json").expect("writes");

        let refusal = DeviceStore::load(path.clone()).expect_err("refuses");
        assert!(refusal.to_string().contains("not readable"), "{refusal}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_device_list_survives_a_round_trip_through_the_file() {
        let dir = std::env::temp_dir().join(format!("cide-remote-rt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("creates");
        let path = dir.join("remote-devices.json");

        let store = DeviceStore::load(path.clone()).expect("starts empty");
        let code = store.begin_pairing().expect("mints");
        let (device, key) = store.redeem(&code, "Pixel 9", "android").expect("pairs");
        store.remember_port(17_644).expect("writes");

        let reopened = DeviceStore::load(path.clone()).expect("reads");
        assert_eq!(
            reopened
                .seal_key(&device)
                .expect("a key")
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            key
        );
        assert_eq!(reopened.remembered_port(), Some(17_644));
        assert_eq!(reopened.devices()[0].name, "Pixel 9");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the device list holds a credential's hash");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
