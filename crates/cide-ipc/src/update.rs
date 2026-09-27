//! Self-update: what the backend tells a window about a newer release.
//!
//! The check itself lives in `cide-app`'s `updater` module — it needs `tauri-plugin-updater`,
//! and only `cide-app` may reach tauri. These are the shapes that cross the wire, here so
//! `cargo xtask codegen` exports them to `ui/src/ipc/generated.ts` like every other payload.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A release newer than the running build, as the manifest described it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateInfo {
    /// The version on offer, as the manifest spelled it (`0.11.0`, no `v`).
    pub version: String,
    /// The running build's own version, so the notice can say "you have …" without a second
    /// round trip for `boot.capabilities.version`.
    pub current: String,
    /// The release notes the manifest carried, if any. Plain text, possibly long.
    pub notes: Option<String>,
    /// The release's page on GitHub — where *Open release page* goes, and where a failed install
    /// sends the user to do it by hand.
    pub release_url: String,
    /// Whether this build can replace itself: an AppImage whose file and folder are writable, or
    /// a macOS `.app` that is not running from the disk image or from App Translocation.
    /// Everything else — the tarball, a `.deb`, a Flatpak — is offered the release page instead,
    /// because writing an AppImage over a binary that is not one would leave an unlaunchable
    /// file where the program used to be. `cide-app`'s `updater::installability` has the list.
    pub in_place: bool,
}

/// What a manual *Check for updates* found. The start-up check never produces the last two as a
/// notice: an offline laptop is not news, and neither is a build that is up to date.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum UpdateCheck {
    Available {
        update: UpdateInfo,
    },
    UpToDate {
        current: String,
    },
    /// No check was possible: a development build, no signing key configured, or the request
    /// failed. The sentence says which, for a person.
    Unavailable {
        reason: String,
    },
}

/// Download progress, sent at most once per whole percent so a fast connection does not flood
/// every window with events.
///
/// KiB in a `u32` rather than bytes in a `u64`: ts-rs exports `u64` as `bigint`, which JSON
/// cannot carry, and a 4 TiB ceiling is not one an AppImage will meet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateProgress {
    pub received_kib: u32,
    /// `None` when the server sent no `Content-Length`.
    pub total_kib: Option<u32>,
}
