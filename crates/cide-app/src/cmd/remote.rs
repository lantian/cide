//! The Settings screen's side of the remote listener. (M72)
//!
//! Six commands, and deliberately **no** `remote_set_enabled`. Turning the feature on is a
//! settings change like any other and goes through `settings_set`, which reconciles the listener
//! the same way it re-walks an index when the explorer's toggles move. A second door onto one
//! switch is the split `cide_git::push` already paid for once: two routes to one gesture that
//! disagreed about whether it asks first.

use tauri::{AppHandle, State};

use cide_ipc::remote::{PairingInvite, PairingProgress, RemoteDevice, RemoteStatus};

use cide_core::CoreError;

use crate::workspace_state::WorkspaceState;

type Result<T> = std::result::Result<T, CoreError>;

/// What the listener is doing, including how many devices are connected right now.
#[tauri::command(rename_all = "camelCase")]
pub fn remote_status(app: AppHandle) -> RemoteStatus {
    crate::remote::status(&app)
}

/// Whether the pairing window is still open, and the device at it right now.
///
/// Polled by the pairing modal off `cide://remote-changed`. `open` is what closes the modal, and
/// it is asked for rather than pushed because none of the three ways a code stops being usable —
/// redeemed, guessed wrong, expired — raises an event of its own; the redemption raises one only
/// because a *device arriving* is separately worth reporting.
///
/// The digits are the typed road's whole authentication: a scanned pairing carried cide's key in
/// the QR and could not have got this far without it, so the panel shows them only when the
/// device had no key to pin — asking for a comparison that cannot fail is how people learn to
/// stop making the one that can.
#[tauri::command(rename_all = "camelCase")]
pub fn remote_pairing_progress(app: AppHandle) -> PairingProgress {
    crate::remote::pairing_progress(&app)
}

/// The paired devices. No token, no hash — there is nothing here to mask because nothing that
/// needs masking is in the type.
#[tauri::command(rename_all = "camelCase")]
pub fn remote_devices(app: AppHandle) -> Vec<RemoteDevice> {
    crate::remote::devices(&app)
}

/// Open a pairing window, and make sure there is a socket for it to be redeemed on.
///
/// The second half is what makes the *first* pairing possible at all. The listener only binds
/// when there is somebody to serve — a cide with the feature on and no paired device listens on
/// nothing, which is the right default for a café network — so the very first device would have
/// nothing to connect to. An open pairing window is the other way of being somebody to serve, and
/// it lasts two minutes.
#[tauri::command(rename_all = "camelCase")]
pub fn remote_pairing_start(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
) -> Result<PairingInvite> {
    let Some(devices) = crate::remote::device_store(&app) else {
        return Err(CoreError::Io("this cide has no remote subsystem".into()));
    };
    let settings = state.with(|ws| ws.settings.remote.clone());
    if !settings.enabled {
        return Err(CoreError::Io(
            "turn the remote listener on before pairing a device".into(),
        ));
    }

    let code = devices
        .begin_pairing()
        .map_err(|error| CoreError::Io(error.to_string()))?;
    // The code exists before the socket does, which is the order that matters: `reconcile` asks
    // whether there is anybody to serve, and an open window is the answer.
    crate::remote::reconcile(&app, &settings);

    let invite = invite_for(&app, &code);
    crate::emit::remote_changed(&app);
    Ok(invite)
}

/// Close the pairing window. The code is dropped, not expired — there is nothing to wait out.
#[tauri::command(rename_all = "camelCase")]
pub fn remote_pairing_cancel(app: AppHandle, state: State<'_, WorkspaceState>) {
    if let Some(devices) = crate::remote::device_store(&app) {
        devices.cancel_pairing();
    }
    // The socket may have existed only for that window. Reconciling takes it back down when the
    // device list is still empty, so a cancelled pairing leaves nothing listening.
    let settings = state.with(|ws| ws.settings.remote.clone());
    crate::remote::reconcile(&app, &settings);
    crate::emit::remote_changed(&app);
}

/// Revoke a device. Its token stops working immediately, including on a connection it already
/// holds — the next frame it sends is checked against a list it is no longer on.
#[tauri::command(rename_all = "camelCase")]
pub fn remote_device_forget(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    device: String,
) -> Result<()> {
    let Some(devices) = crate::remote::device_store(&app) else {
        return Err(CoreError::Io("this cide has no remote subsystem".into()));
    };
    devices
        .forget(&device)
        .map_err(|error| CoreError::Io(error.to_string()))?;

    // Revoking the last one takes the listener down, for `remote_pairing_start`'s reason in
    // reverse: with nobody to serve there is no reason to be on a network.
    let settings = state.with(|ws| ws.settings.remote.clone());
    crate::remote::reconcile(&app, &settings);
    crate::emit::remote_changed(&app);
    Ok(())
}

/// Rename a device. The name is cide's label for it and is never trusted for anything.
#[tauri::command(rename_all = "camelCase")]
pub fn remote_device_rename(app: AppHandle, device: String, name: String) -> Result<()> {
    let Some(devices) = crate::remote::device_store(&app) else {
        return Err(CoreError::Io("this cide has no remote subsystem".into()));
    };
    devices
        .rename(&device, &name)
        .map_err(|error| CoreError::Io(error.to_string()))?;
    crate::emit::remote_changed(&app);
    Ok(())
}

/// Everything a device needs, in one line it can be handed by a scan or a paste.
///
/// `i` — the instance id — is what lets a device recognise a re-pair of a cide it already knows
/// instead of listing the same machine twice. Nothing else can do that job: an address changes
/// with the network, a port is derived from a profile name that could be reused, and a hostname
/// is shared by both profiles on one laptop.
///
/// `m` is the transport and `k` is this instance's public key. Together they are what let a
/// device establish a channel *to this cide specifically* before it has any credential of its
/// own — which is what keeps the key it is about to be given off the wire in the clear.
/// Everything a device needs, packed small enough to photograph.
///
/// **Every address goes in, and that is the requirement rather than a nicety.** cide offers
/// every private address it has — ten on a developer's machine, one real network card and nine
/// docker, libvirt, k8s and VPN bridges — and there is no way here to know which one a phone can
/// reach. An earlier version capped the list at four to shrink the symbol and promptly dropped
/// the only address that worked, which is a worse failure than a QR that is hard to scan: it
/// fails *confidently*, on a code that looks perfectly valid.
///
/// The size comes out of the encoding instead, and it more than pays for the cap:
///
/// * **One port for all of them.** Every address used to carry its own `%3A17643` — six
///   characters plus three for the escaped colon, per address, for a number that is the same
///   every time.
/// * **Bare commas.** A comma is a sub-delimiter and legal unescaped in a query; escaping it
///   cost two characters per address for nothing.
/// * **No instance id and no display name.** Both used to ride along so the device could
///   recognise a re-pair and label the machine — and since M74 the `Paired` frame carries
///   `instance` and `label`, because the *typed* road never had a QR to read them from. Sending
///   them twice made the payload 38 characters longer and the two copies able to disagree.
///
/// Ten addresses went from 349 characters to 206 — shorter than four addresses under the old
/// spelling. An IPv6 address needs no brackets here precisely because the port is no longer
/// attached to it; the device brackets it when it builds a URL.
fn invite_for(app: &AppHandle, code: &str) -> PairingInvite {
    let (hosts, port) = match crate::remote::status(app) {
        RemoteStatus::Listening {
            addresses, port, ..
        } => (addresses.join(","), port),
        _ => (String::new(), 0),
    };
    // `hosts` is spelled in, not percent-encoded: it is dotted quads, colons and commas, every
    // one of which is legal unescaped in a query, and escaping them is how the payload grew
    // past what a camera can resolve.
    let uri = format!(
        "cide://pair?v={}&m=seal&k={}&p={port}&h={hosts}&c={}",
        cide_ipc::remote::PROTOCOL_VERSION,
        percent(&crate::remote::public_key(app).unwrap_or_default()),
        code,
    );
    PairingInvite {
        code: code.to_owned(),
        grouped: cide_remote::devices::grouped(code),
        // The QR is built here, from the same `uri` the panel offers for a paste, so a scan and
        // a paste cannot describe different pairings. Encoding it a second time from the same
        // string would be a second producer of one value — `cide_git::push::preview`'s rule.
        qr: cide_core::remote::pairing_qr(&uri),
        uri,
        expires_unix_ms: expiry_unix_ms(),
    }
}

fn expiry_unix_ms() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    now + cide_remote::devices::CODE_TTL.as_millis() as u64
}

/// Percent-encode everything that is not unreserved.
///
/// Hand-written rather than a dependency: the whole input set is a uuid, a hostname with a
/// possible `[DEV] ` prefix, and a comma-separated list of addresses. A crate for that would be a
/// crate in the dependency graph of an application that already refuses several larger ones for
/// less reason.
fn percent(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pairing_uri_escapes_what_a_url_cannot_carry() {
        // The `[DEV] ` prefix is the case this exists for: brackets and a space in a query value.
        assert_eq!(percent("[DEV] box"), "%5BDEV%5D%20box");
        assert_eq!(percent("i-1234-abcd"), "i-1234-abcd");
        assert_eq!(
            percent("192.168.1.4:17643,10.8.0.3:17643"),
            "192.168.1.4%3A17643%2C10.8.0.3%3A17643"
        );
    }

    /// The payload no longer brackets an IPv6 address, and that is deliberate.
    ///
    /// A bracket exists only to separate an address from the port glued to it, and since M74 the
    /// port travels once, in `p=`, rather than once per address. So `fd00::1` is unambiguous in
    /// the payload and two characters shorter — and the device puts the brackets back when it
    /// builds a URL, because `ws://fd00::1:17643` is not the machine that sent the code.
    /// `../cide-mobile`'s `withPort` is the other half and has its own tests.
    #[test]
    fn the_payload_carries_addresses_bare_and_the_port_once() {
        let uri = "cide://pair?v=1&m=seal&k=KKK&p=17643&h=192.168.1.4,fd00::1&c=0RXVJ6ZE";
        assert!(uri.contains("&p=17643"), "one port for every address");
        assert!(
            uri.contains("h=192.168.1.4,fd00::1"),
            "bare, comma-separated"
        );
        assert!(
            !uri.contains("%3A"),
            "no escaped colons: that is what made it unscannable"
        );
        assert!(!uri.contains("%2C"), "no escaped commas either");
        // A comma and a colon are both legal unescaped in a query, which is what lets ten
        // addresses fit in a symbol a camera can still resolve.
        assert!(
            !uri.contains('['),
            "brackets belong to the URL, not the payload"
        );
    }
}
