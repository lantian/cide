//! What a paired device is talking to: this machine, under this profile. (M72)
//!
//! Three facts, none of which cide had any reason to know until a second device started asking:
//! what this instance is *called*, what identifies it across restarts and address changes, and
//! which addresses a phone could reach it on.
//!
//! It lives in `cide-core` rather than in the server crate because `cide-headless` prints the
//! answers (`cide-headless remote`) and the Settings panel shows them, and neither of those
//! should have to link a socket to ask what this machine is called.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;

use crate::persist;

/// The name a device lists this instance under.
///
/// `[DEV] thinkpad` and `thinkpad`, which is deliberately the same vocabulary the OS window
/// titles already use ([`crate::profile::title_prefix`]) — two surfaces onto one instance should
/// not disagree about what to call it, and a user with a real instance and a dev instance both
/// paired is exactly the case the prefix was invented for.
///
/// It is a **default**, not a name: the panel lets it be edited, and a device shows whatever the
/// instance last told it. Nothing here is an identifier — see [`instance_id`].
pub fn display_name() -> String {
    format!("{}{}", crate::profile::title_prefix(), hostname())
}

/// This machine's hostname, or `"cide"` when the system will not say.
///
/// `gethostname` rather than `/etc/hostname` or `$HOSTNAME`: the file is Linux-specific and is
/// not what a running system necessarily believes, and the variable is a shell convention that a
/// desktop launch does not set — the same class of mistake as reading `PATH` from a GUI process
/// and wondering where the toolchain went (`cide_core::toolchain`'s header has that story).
///
/// The fallback is a name and not an error because every caller of this is drawing a label. A
/// machine that cannot state its own name still has sessions worth watching.
#[cfg(unix)]
pub fn hostname() -> String {
    // POSIX permits truncation without a terminator at exactly HOST_NAME_MAX, so the buffer is
    // one longer than it needs to be and the result is read up to the first NUL. Reading to the
    // end of the buffer instead is how a truncated name acquires trailing garbage.
    let mut buf = vec![0u8; 256];
    // SAFETY: `buf` is a live allocation of `buf.len()` bytes and `gethostname` writes at most
    // that many.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast::<libc::c_char>(), buf.len() - 1) };
    if ok != 0 {
        return FALLBACK_NAME.to_owned();
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..end]).trim().to_owned();
    if name.is_empty() {
        FALLBACK_NAME.to_owned()
    } else {
        name
    }
}

#[cfg(not(unix))]
pub fn hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| FALLBACK_NAME.to_owned())
}

const FALLBACK_NAME: &str = "cide";

/// Where this profile's instance identity is kept.
pub fn instance_id_path() -> PathBuf {
    persist::state_dir().join("remote-id")
}

/// A stable identity for this profile, minted on first use and never changed.
///
/// A device keys its saved entry on this, and that is the whole job: a re-pair of a cide the
/// device already knows must update that entry rather than add a second row for the same
/// machine. Nothing else can do it — an address changes with the network, a port is derived
/// from a profile name that can be reused, and a hostname is shared by both profiles on one
/// laptop.
///
/// **Its own file rather than a field of the device list**, which is where it would naturally
/// sit. The device list is the thing a user empties: revoking every phone, or a repair of a file
/// that failed to parse, must not change which machine this is, or every previously paired
/// device would list it a second time the next time it connected. An identity that can be
/// cleared by a housekeeping gesture is not an identity.
///
/// `0600` because it is in the same directory as everything else here and there is no reason for
/// it to be looser, not because it is a secret — it identifies, it does not authorise. Every
/// failure falls back to an ephemeral id and a warning: a machine that cannot write its state
/// directory has worse problems, and refusing to serve a phone over it would be the wrong end to
/// fail at.
pub fn instance_id() -> String {
    let path = instance_id_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        let existing = text.trim();
        if !existing.is_empty() {
            return existing.to_owned();
        }
    }
    let minted = format!("i-{}", uuid::Uuid::new_v4());
    if let Err(error) =
        persist::write_atomic_with_mode(&path, minted.as_bytes(), persist::PRIVATE_MODE)
    {
        tracing::warn!(%error, path = %path.display(), "remote: could not record the instance id");
    }
    minted
}

// --- where the listener lives -------------------------------------------------------------

/// The first port of the band cide's remote listener uses.
///
/// Unassigned by IANA, above the ephemeral range on Linux (`32768–60999`), and not a number
/// anything else is likely to have taken. Nothing depends on the value except that it is stable:
/// a device saves the port it paired against.
pub const PORT_BASE: u16 = 17_643;

/// How many ports past [`PORT_BASE`] may be walked.
pub const PORT_BAND: u16 = 64;

/// The port this profile prefers.
///
/// Derived rather than configured, because the case that must work without anybody thinking
/// about it is the one the user actually has: a real instance and a `[DEV]` one, on one machine,
/// both wanting a stable address. The production profile takes the base and a named profile takes
/// a hash of its name, so the two differ from the first launch and keep differing across
/// restarts — which is what lets a device save a port at all.
///
/// A *preference*, not a claim. The port actually bound is remembered by the device store and
/// preferred next time; see `cide-remote`'s `DeviceStore::remembered_port` for why that is what
/// makes a collision self-healing.
pub fn port_for_profile(profile: Option<&str>) -> u16 {
    let Some(name) = profile.filter(|p| !p.is_empty()) else {
        return PORT_BASE;
    };
    // FNV-1a. Not for its distribution — there are 63 slots and a handful of profiles — but
    // because it is four lines and deterministic across builds, which a `DefaultHasher` is
    // explicitly not (`std` reserves the right to change it, and a port that moved when the
    // toolchain did would unpair every device).
    let mut hash: u32 = 0x811c_9dc5;
    for byte in name.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    PORT_BASE + 1 + (hash % u32::from(PORT_BAND - 1)) as u16
}

/// What binding to an address would mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindVerdict {
    /// Loopback, a private range, or link-local. Reachable from this machine and this network.
    Private,
    /// `0.0.0.0` or `::` — every interface this machine has.
    ///
    /// Allowed under *this network*, because the address itself promises nothing: what actually
    /// reaches the listener is decided by the network the machine is on and by the device list,
    /// and a user who cannot bind the wildcard cannot reach cide from a phone whose address
    /// changes.
    Unspecified,
    /// Routable from the internet.
    ///
    /// Refused unless separately and explicitly allowed. Binding a public address is a different
    /// decision from binding a LAN one — it is the difference between "the people in my house"
    /// and "everyone" — and it must not ride the same switch.
    Public,
}

/// Classify a bind address.
///
/// Deliberately a function over an address rather than a check inside the bind: the Settings
/// screen has to say what will happen *before* anything is bound, and a rule that only exists at
/// the moment of binding cannot be shown to anybody.
pub fn bind_policy(addr: IpAddr) -> BindVerdict {
    match addr {
        IpAddr::V4(v4) => {
            if v4.is_unspecified() {
                BindVerdict::Unspecified
            } else if v4.is_loopback() || v4.is_private() || v4.is_link_local() {
                BindVerdict::Private
            } else {
                BindVerdict::Public
            }
        }
        IpAddr::V6(v6) => {
            if v6.is_unspecified() {
                BindVerdict::Unspecified
            } else if v6.is_loopback() || is_unique_local(v6) || is_link_local_v6(v6) {
                BindVerdict::Private
            } else {
                // A v4-mapped v6 address is really a v4 address, and classifying it as public
                // would refuse `::ffff:192.168.1.10` — which is what a dual-stack listener sees
                // a LAN peer as.
                match v6.to_ipv4_mapped() {
                    Some(v4) => bind_policy(IpAddr::V4(v4)),
                    None => BindVerdict::Public,
                }
            }
        }
    }
}

/// `fc00::/7`. `Ipv6Addr::is_unique_local` is unstable, so it is spelled out.
fn is_unique_local(addr: Ipv6Addr) -> bool {
    addr.octets()[0] & 0xfe == 0xfc
}

/// `fe80::/10`. `Ipv6Addr::is_unicast_link_local` is unstable, so it is spelled out.
fn is_link_local_v6(addr: Ipv6Addr) -> bool {
    let o = addr.octets();
    o[0] == 0xfe && o[1] & 0xc0 == 0x80
}

/// The addresses a device could plausibly reach this machine on, most useful first.
///
/// The pairing payload has to carry a host, and a machine has several. There is **no `std` road**
/// to this — `std::net` can connect and bind and cannot enumerate — so the options were a new
/// dependency or one syscall family, and this is one syscall family: `libc` is already here for
/// `PR_SET_PDEATHSIG`, `getifaddrs` exists on Linux and macOS alike, and the precedent for
/// hand-writing a platform call rather than taking a crate for it is `cide_core::proc`, which
/// does exactly this for `/proc` and `kinfo_proc`.
///
/// Loopback is **excluded**: it is the one address that is guaranteed not to help a phone, and
/// including it would put the least useful entry at the top of a list somebody is reading off a
/// screen. Link-local addresses are excluded for the neighbouring reason — a `fe80::` address
/// needs a scope identifier that means nothing on the other machine.
#[cfg(unix)]
pub fn local_addresses() -> Vec<IpAddr> {
    let mut out: Vec<IpAddr> = Vec::new();
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `getifaddrs` writes a pointer to a list it allocates, and `freeifaddrs` below
    // releases it. Every dereference between the two checks for null first.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return out;
    }
    let mut cursor = head;
    while !cursor.is_null() {
        // SAFETY: `cursor` is non-null and points into the list `getifaddrs` allocated.
        let entry = unsafe { &*cursor };
        cursor = entry.ifa_next;
        if entry.ifa_addr.is_null() {
            continue;
        }
        // SAFETY: `ifa_addr` is non-null; `sa_family` is the one field every `sockaddr` has.
        let family = unsafe { (*entry.ifa_addr).sa_family };
        let addr = match i32::from(family) {
            libc::AF_INET => {
                // SAFETY: the family says this is a `sockaddr_in`.
                let raw = unsafe { &*(entry.ifa_addr.cast::<libc::sockaddr_in>()) };
                IpAddr::V4(Ipv4Addr::from(u32::from_be(raw.sin_addr.s_addr)))
            }
            libc::AF_INET6 => {
                // SAFETY: the family says this is a `sockaddr_in6`.
                let raw = unsafe { &*(entry.ifa_addr.cast::<libc::sockaddr_in6>()) };
                IpAddr::V6(Ipv6Addr::from(raw.sin6_addr.s6_addr))
            }
            _ => continue,
        };
        if addr.is_loopback() || matches!(addr, IpAddr::V6(v6) if is_link_local_v6(v6)) {
            continue;
        }
        if !out.contains(&addr) {
            out.push(addr);
        }
    }
    // SAFETY: `head` came from `getifaddrs` and is freed exactly once.
    unsafe { libc::freeifaddrs(head) };

    // IPv4 first: it is what a person can read off a screen and type into a phone, and it is
    // what a home network hands out. The v6 addresses follow rather than being dropped, because
    // a machine on a v6-only network has nothing else.
    out.sort_by_key(|addr| u8::from(addr.is_ipv6()));
    out
}

#[cfg(not(unix))]
pub fn local_addresses() -> Vec<IpAddr> {
    Vec::new()
}

/// Encode a pairing URI as a QR module grid, or `None` if it will not encode.
///
/// `None` is a real answer and not an error, which is why this returns an `Option` and not a
/// `Result`: nothing the caller could do with a reason would help, because the payload is not
/// the user's to shorten. A pairing URI is around 160 characters and encodes comfortably at
/// version 7-ish; the refusal arm exists for the case where the payload grows past what a
/// version-40 symbol holds at this error-correction level, and the honest response to that is
/// to draw the code and the address as text — which the panel does anyway, beside the QR. See
/// the dependency's entry in the root manifest.
///
/// **Medium error correction, deliberately.** A QR on a screen is read by a phone held a
/// hand's width away in a lit room, which is the easiest case a scanner ever gets; `High`
/// would buy robustness nothing here needs and spend it on a denser symbol, and a denser
/// symbol on a small dialog is the one thing that actually stops a scan.
#[must_use]
pub fn pairing_qr(uri: &str) -> Option<cide_ipc::remote::QrMatrix> {
    use qrcode::{EcLevel, QrCode};

    let code = QrCode::with_error_correction_level(uri.as_bytes(), EcLevel::M).ok()?;
    let width = code.width();
    // `to_colors` is row-major and exactly `width * width` long, which is the shape `QrMatrix`
    // promises. Asserting it here rather than trusting it keeps the promise this side of the
    // wire, where a test can see it.
    let colors = code.to_colors();
    debug_assert_eq!(colors.len(), width * width);
    let size = u32::try_from(width).ok()?;

    Some(cide_ipc::remote::QrMatrix {
        size,
        modules: colors
            .into_iter()
            .map(|c| c == qrcode::Color::Dark)
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hostname_is_always_a_name() {
        let name = hostname();
        assert!(!name.is_empty());
        assert_eq!(
            name,
            name.trim(),
            "a trailing NUL or newline reached the label"
        );
        assert!(!name.contains('\0'), "{name:?}");
    }

    /// The prefix is the instance-telling-apart mechanism, and it leads for the same reason it
    /// leads in an OS window title: a task switcher and a phone list both truncate from the
    /// right.
    #[test]
    fn a_display_name_starts_with_the_profile_marker() {
        let name = display_name();
        assert!(name.ends_with(&hostname()), "{name}");
        assert_eq!(
            name.len(),
            crate::profile::title_prefix().len() + hostname().len()
        );
    }

    #[test]
    fn the_production_profile_takes_the_base_and_a_named_one_does_not() {
        assert_eq!(port_for_profile(None), PORT_BASE);
        assert_eq!(port_for_profile(Some("")), PORT_BASE);

        let dev = port_for_profile(Some("dev"));
        assert_ne!(dev, PORT_BASE, "a named profile collided with production");
        assert!((PORT_BASE..PORT_BASE + PORT_BAND).contains(&dev));
    }

    /// The whole point of deriving it: a device saves a port, so the same name must give the same
    /// answer on every launch and every build.
    #[test]
    fn a_profiles_port_never_moves() {
        for name in ["dev", "test", "scratch", "a-very-long-profile-name"] {
            let once = port_for_profile(Some(name));
            assert_eq!(once, port_for_profile(Some(name)));
            assert!(
                (PORT_BASE..PORT_BASE + PORT_BAND).contains(&once),
                "{name} -> {once}"
            );
        }
        assert_ne!(
            port_for_profile(Some("dev")),
            port_for_profile(Some("test"))
        );
    }

    #[test]
    fn a_public_address_is_a_different_decision_from_a_private_one() {
        use std::str::FromStr;

        for private in [
            "127.0.0.1",
            "10.0.0.5",
            "172.16.4.1",
            "172.31.255.254",
            "192.168.1.40",
            "169.254.1.1",
            "::1",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "::ffff:192.168.1.10",
        ] {
            assert_eq!(
                bind_policy(IpAddr::from_str(private).expect("an address")),
                BindVerdict::Private,
                "{private}"
            );
        }

        for public in ["8.8.8.8", "1.1.1.1", "172.32.0.1", "2001:4860:4860::8888"] {
            assert_eq!(
                bind_policy(IpAddr::from_str(public).expect("an address")),
                BindVerdict::Public,
                "{public}"
            );
        }

        for any in ["0.0.0.0", "::"] {
            assert_eq!(
                bind_policy(IpAddr::from_str(any).expect("an address")),
                BindVerdict::Unspecified,
                "{any}"
            );
        }
    }

    /// Whatever this machine has, the answer must be usable: no loopback (it cannot help a
    /// phone), no link-local (its scope means nothing on the other machine), no duplicates, and
    /// the readable ones first.
    #[test]
    fn a_pairing_uri_encodes_to_a_square_grid() {
        let uri = "cide://pair?v=1&i=abc&n=thinkpad&m=seal&k=zo060cy2M-x7cMF4FKXHbs0CloUFDTRHRboFhw5YfVk&h=192.168.1.4:17643&c=0RXVJ6ZE";
        let qr = pairing_qr(uri).expect("a pairing URI must encode");

        // The contract the renderer indexes against. A short grid would draw a code that is
        // structurally a QR and reads as nothing.
        assert_eq!(qr.modules.len(), (qr.size * qr.size) as usize);
        // Version 1 is 21 modules and version 40 is 177; anything outside that is not a QR.
        assert!((21..=177).contains(&qr.size), "size {}", qr.size);
        assert_eq!(qr.size % 4, 1, "every QR version is 4n+17 modules across");
    }

    #[test]
    fn the_three_finder_patterns_are_where_a_scanner_looks_for_them() {
        // This is the assertion that a grid which is merely the right *shape* cannot pass. A
        // finder pattern is a 7x7 ring: dark border, light ring inside it, 3x3 dark core. All
        // three corners carry one, and a scanner finds the symbol by them alone — so if the
        // module order were ever transposed or flipped, everything above would still hold and
        // only this would fail.
        let qr = pairing_qr("cide://pair?v=1&c=0RXVJ6ZE").expect("must encode");
        let n = qr.size;
        for (ox, oy) in [(0, 0), (n - 7, 0), (0, n - 7)] {
            for dy in 0..7 {
                for dx in 0..7 {
                    let ring = dx == 0 || dx == 6 || dy == 0 || dy == 6;
                    let core = (2..=4).contains(&dx) && (2..=4).contains(&dy);
                    assert_eq!(
                        qr.dark(ox + dx, oy + dy),
                        ring || core,
                        "finder at ({ox},{oy}) is wrong at ({dx},{dy})"
                    );
                }
            }
        }
        // And the fourth corner has none — which is what tells a scanner the orientation.
        let bottom_right_is_a_finder =
            (0..7).all(|d| qr.dark(n - 7 + d, n - 1) && qr.dark(n - 1, n - 7 + d));
        assert!(
            !bottom_right_is_a_finder,
            "the fourth corner must not be a finder"
        );
    }

    #[test]
    fn reading_outside_the_grid_is_light_rather_than_a_panic() {
        // The renderer walks a quiet zone around the symbol, so it asks about modules that do
        // not exist. Answering `false` is what makes that loop legal to write.
        let qr = pairing_qr("cide://pair?v=1").expect("must encode");
        assert!(!qr.dark(qr.size, 0));
        assert!(!qr.dark(0, qr.size));
        assert!(!qr.dark(u32::MAX, u32::MAX));
    }

    #[test]
    fn a_payload_too_large_to_encode_is_none_rather_than_a_panic() {
        // The arm that exists so a pairing is never blocked by its own convenience. A
        // version-40 M symbol holds about 2 300 bytes.
        assert!(pairing_qr(&"x".repeat(8_000)).is_none());
    }

    #[test]
    fn the_offered_addresses_are_ones_a_phone_could_use() {
        let found = local_addresses();
        assert!(found.iter().all(|addr| !addr.is_loopback()), "{found:?}");
        assert!(
            found
                .iter()
                .all(|addr| !matches!(addr, IpAddr::V6(v6) if is_link_local_v6(*v6))),
            "{found:?}"
        );
        let mut seen = found.clone();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), found.len(), "a duplicate address was offered");
        let first_v6 = found.iter().position(|addr| addr.is_ipv6());
        let last_v4 = found.iter().rposition(|addr| addr.is_ipv4());
        if let (Some(first_v6), Some(last_v4)) = (first_v6, last_v4) {
            assert!(last_v4 < first_v6, "the v6 addresses did not come last");
        }
    }
}
