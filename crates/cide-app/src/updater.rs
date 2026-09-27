//! Self-update from GitHub releases: the start-up check, and the install a user asks for.
//!
//! # The shape of it
//!
//! A few seconds after the windows are up, [`start`] asks the release manifest — `latest.json`,
//! an asset of the newest *stable* GitHub release — whether there is a version newer than this
//! build. If there is, and it is not the one the user said to skip, every window is told
//! ([`crate::emit::update_available`]) and draws a notice with three answers:
//!
//! - **Update** runs [`install`]: download, verify the minisign signature against the public
//!   key in `tauri.conf.json`, and replace the AppImage or the `.app` on disk. It does *not*
//!   restart; the user is asked, because a restart stops every session and run this cide hosts.
//! - **Skip this version** writes `settings.update.skipped_version`, and only that version is
//!   silenced (see [`announce`]).
//! - The notice's own dismiss button is *not now*: nothing is written, and the next start
//!   announces it again.
//!
//! # Why `tauri-plugin-updater` and not a hand-rolled download
//!
//! The plugin checks a signature made with a key only the release workflow holds. A hand-rolled
//! updater has the `SHA256SUMS` file at best, and that file comes from the same server as the
//! artefact — anyone who can replace one can replace both. The plugin also already knows the two
//! awkward installs: swapping an AppImage that is running (rename the old one aside on the same
//! filesystem, write the new one, restore on failure), and replacing a `.app` in a folder that
//! needs an administrator's password on macOS.
//!
//! # Why "stable only" costs nothing here
//!
//! The endpoint is `releases/latest/download/latest.json`, and GitHub's `latest` never resolves
//! to a draft or a prerelease. A prerelease can therefore never be announced, with no version
//! filtering in this module to get wrong.
//!
//! # Which builds can replace themselves
//!
//! [`installability`]. The dangerous case is a build that is *not* an AppImage on Linux: the
//! plugin falls back to its AppImage installer for any Linux build it cannot classify, which
//! would write an AppImage over a tarball's `cide` binary. So [`install`] refuses anything
//! [`installability`] did not call [`Installability::InPlace`], and the notice offers those
//! builds the release page instead.

use std::path::{Path, PathBuf};
use std::time::Duration;

use cide_ipc::{UpdateCheck, UpdateInfo, UpdateProgress};
use parking_lot::Mutex;
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::UpdaterExt;

use crate::workspace_state::WorkspaceState;

/// Where a release's page lives. The release workflow publishes to this repository, and the
/// endpoint in `tauri.conf.json` names the same one.
const RELEASES: &str = "https://github.com/lantian/cide/releases";

/// Overrides the manifest URL. For testing an update end to end against a local server — see
/// `docs/packaging.md` — and nothing else: a URL here must be `https` unless the config allows
/// otherwise, which it does not.
const ENDPOINT_ENV: &str = "CIDE_UPDATE_ENDPOINT";

/// How long after the windows are restored the start-up check runs. Late enough that it does
/// not compete with the restore for the first second of a launch, early enough that the notice
/// arrives while the user is still orienting rather than halfway into a thought.
const START_DELAY: Duration = Duration::from_secs(5);

/// How long one manifest request may take. GitHub answers in well under a second; a check that
/// hangs on a captive portal should give up rather than hold a thread for the life of the app.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// Whether this build can replace itself on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installability {
    /// The file (Linux) or bundle (macOS) the plugin will replace.
    InPlace(PathBuf),
    /// Not replaceable, and why — the reason goes to the log, the user sees *Open release page*.
    LinkOnly(&'static str),
}

/// What the check left behind for the windows and for [`install`].
#[derive(Default)]
struct Found {
    info: Option<UpdateInfo>,
    /// The plugin's handle for the same release, which carries the verified download URL and
    /// signature. Kept rather than re-checked at install time so the version the user agreed to
    /// is the version that is installed, even if a newer release appeared in between.
    handle: Option<tauri_plugin_updater::Update>,
    /// Set while a download is running, so a second click (or a second window) does not start
    /// a second download writing over the same file.
    installing: bool,
    /// The version now on disk, once an install finished.
    installed: Option<String>,
}

/// Managed state. One per process.
#[derive(Default)]
pub struct UpdaterState {
    found: Mutex<Found>,
}

impl UpdaterState {
    /// What a window mounting now should show: the update, unless it is already installed.
    pub fn status(&self) -> Option<UpdateInfo> {
        let found = self.found.lock();
        if found.installed.is_some() {
            return None;
        }
        found.info.clone()
    }
}

/// Spawn the start-up check. Call once, after the windows are restored.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("cide-update-check".into())
        .spawn(move || {
            std::thread::sleep(START_DELAY);
            let settings = app
                .state::<WorkspaceState>()
                .with(|ws| ws.settings.update.clone());
            if !settings.check_on_start {
                tracing::debug!("update check off in settings");
                return;
            }
            match tauri::async_runtime::block_on(check(&app)) {
                Ok(UpdateCheck::Available { update }) => {
                    if announce(&update.version, settings.skipped_version.as_deref()) {
                        crate::emit::update_available(&app, &update);
                    } else {
                        tracing::info!(version = %update.version, "update available, skipped by the user");
                    }
                }
                Ok(UpdateCheck::UpToDate { current }) => {
                    tracing::info!(%current, "cide is up to date");
                }
                // Info, not warn: an offline laptop, a captive portal and GitHub's rate limit
                // are all ordinary, and none of them is something the user asked about.
                Ok(UpdateCheck::Unavailable { reason }) => {
                    tracing::info!(%reason, "no update check");
                }
                Err(reason) => tracing::info!(%reason, "update check failed"),
            }
        })
    {
        tracing::warn!(%error, "no thread for the update check");
    }
}

/// Ask the manifest. Records what it found in [`UpdaterState`] either way.
///
/// `Err` is a check that was attempted and failed (the network, a malformed manifest);
/// [`UpdateCheck::Unavailable`] is one that was never attempted, for a reason about this build.
pub async fn check(app: &AppHandle) -> Result<UpdateCheck, String> {
    let current = app.package_info().version.to_string();
    if let Some(reason) = never_checks(&current, cfg!(debug_assertions), &configured_pubkey(app)) {
        return Ok(UpdateCheck::Unavailable {
            reason: reason.into(),
        });
    }

    let mut builder = app.updater_builder().timeout(CHECK_TIMEOUT);
    if let Some(url) = std::env::var_os(ENDPOINT_ENV) {
        let url = url.to_string_lossy();
        let url = url
            .parse()
            .map_err(|error| format!("{ENDPOINT_ENV} is not a URL: {error}"))?;
        builder = builder
            .endpoints(vec![url])
            .map_err(|error| format!("{ENDPOINT_ENV}: {error}"))?;
    }
    let updater = builder.build().map_err(|error| error.to_string())?;
    let found = updater.check().await.map_err(|error| error.to_string())?;

    let state = app.state::<UpdaterState>();
    let Some(handle) = found else {
        let mut slot = state.found.lock();
        slot.info = None;
        slot.handle = None;
        return Ok(UpdateCheck::UpToDate { current });
    };

    let in_place = match installability(app) {
        Installability::InPlace(_) => true,
        Installability::LinkOnly(reason) => {
            tracing::info!(
                reason,
                "this build cannot replace itself; offering the release page"
            );
            false
        }
    };
    let info = UpdateInfo {
        version: handle.version.clone(),
        current,
        notes: handle.body.clone().filter(|notes| !notes.trim().is_empty()),
        release_url: release_url(&handle.version),
        in_place,
    };
    {
        let mut slot = state.found.lock();
        // An install already underway keeps the handle it started with; a manual re-check
        // mid-download must not swap the release out from under it.
        if !slot.installing {
            slot.handle = Some(handle);
        }
        slot.info = Some(info.clone());
    }
    Ok(UpdateCheck::Available { update: info })
}

/// Download, verify and install the release the last check found. Returns the installed
/// version. Does not restart.
pub async fn install(app: &AppHandle) -> Result<String, String> {
    if let Installability::LinkOnly(reason) = installability(app) {
        return Err(format!(
            "This copy of cide cannot update itself ({reason})."
        ));
    }
    let state = app.state::<UpdaterState>();
    let handle = {
        let mut slot = state.found.lock();
        if slot.installing {
            return Err("An update is already downloading.".into());
        }
        let Some(handle) = slot.handle.clone() else {
            return Err("There is no update to install — check for updates first.".into());
        };
        slot.installing = true;
        handle
    };

    let version = handle.version.clone();
    let mut received: u64 = 0;
    let mut last_percent: Option<u64> = None;
    let progress_app = app.clone();
    let result = handle
        .download_and_install(
            |chunk, total| {
                received += chunk as u64;
                // One event per whole percent. With no length there is no percent, so one per
                // MiB instead — enough to show the download is alive.
                let step = match total {
                    Some(total) if total > 0 => received * 100 / total,
                    _ => received / (1024 * 1024),
                };
                if last_percent != Some(step) {
                    last_percent = Some(step);
                    crate::emit::update_progress(
                        &progress_app,
                        UpdateProgress {
                            received_kib: kib(received),
                            total_kib: total.map(kib),
                        },
                    );
                }
            },
            || {},
        )
        .await;

    let mut slot = state.found.lock();
    slot.installing = false;
    match result {
        Ok(()) => {
            slot.installed = Some(version.clone());
            drop(slot);
            tracing::info!(%version, "update installed; takes effect on restart");
            crate::emit::update_ready(app, &version);
            Ok(version)
        }
        Err(error) => {
            tracing::warn!(%error, %version, "update install failed");
            Err(format!("The update could not be installed: {error}"))
        }
    }
}

/// Whether the start-up check should tell the user about `version`.
///
/// Only the exact skipped version is silenced. Compared as parsed versions when both parse, so
/// `0.11.0` and `v0.11.0` are the same skip; as strings otherwise, so a value this build cannot
/// parse still silences exactly what it said.
pub fn announce(version: &str, skipped: Option<&str>) -> bool {
    let Some(skipped) = skipped else {
        return true;
    };
    match (release_triple(version), release_triple(skipped)) {
        (Some(a), Some(b)) => a != b,
        _ => version.trim() != skipped.trim(),
    }
}

/// `major.minor.patch` of a release version, or `None` for anything else — a prerelease tag
/// included, since GitHub's `latest` never serves one and a skip of one is compared verbatim.
/// Hand-rolled rather than a `semver` dependency: equality of three numbers is the whole need,
/// and the plugin already does the real *newer than* comparison with its own copy.
fn release_triple(version: &str) -> Option<(u64, u64, u64)> {
    let version = version.trim().trim_start_matches('v');
    let version = version.split('+').next()?;
    let mut parts = version.split('.').map(|part| part.parse::<u64>().ok());
    let triple = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(triple)
}

/// Why this build never asks, or `None` when it does.
///
/// A `-dev` version is the trunk between releases (`release.yml`'s `bump` job writes it): it is
/// *older* than the release it is heading for and *newer* than the one before, so either answer
/// the comparison gave would be wrong. A debug build is `./run.sh`, a Vite-served development
/// window that has nothing on disk worth replacing. An empty public key is a config nobody has
/// finished setting up (`docs/packaging.md`), and every download would fail its signature check.
fn never_checks(current: &str, debug: bool, pubkey: &str) -> Option<&'static str> {
    if debug {
        return Some("a debug build does not update itself");
    }
    if current.contains('-') {
        return Some("a development version does not update itself");
    }
    if pubkey.trim().is_empty() {
        return Some("no update signing key is configured in this build");
    }
    None
}

fn configured_pubkey(app: &AppHandle) -> String {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|updater| updater.get("pubkey"))
        .and_then(|key| key.as_str())
        .unwrap_or_default()
        .to_owned()
}

fn release_url(version: &str) -> String {
    format!("{RELEASES}/tag/v{}", version.trim_start_matches('v'))
}

fn kib(bytes: u64) -> u32 {
    u32::try_from(bytes / 1024).unwrap_or(u32::MAX)
}

/// Whether this build can replace itself, and what it would replace.
pub fn installability(app: &AppHandle) -> Installability {
    #[cfg(target_os = "linux")]
    {
        linux_installability(app.env().appimage.as_deref().map(Path::new))
    }
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        match std::env::current_exe() {
            Ok(exe) => macos_installability(&exe),
            Err(_) => Installability::LinkOnly("the running executable could not be located"),
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = app;
        Installability::LinkOnly("self-update is built for Linux and macOS only")
    }
}

/// An AppImage whose file *and* folder are writable.
///
/// `APPIMAGE` is the only evidence: it is set by the AppImage runtime to the path of the
/// `.AppImage` file, and tauri already cross-checks it against the mount point before exposing it
/// as `Env::appimage`. The folder matters as much as the file: the plugin renames the running
/// image aside inside a temporary directory on the same filesystem before writing the new one,
/// and a read-only folder (`/opt`, owned by root) fails that rename halfway into an update.
#[cfg(any(target_os = "linux", test))]
fn linux_installability(appimage: Option<&Path>) -> Installability {
    let Some(image) = appimage else {
        return Installability::LinkOnly("not running from an AppImage");
    };
    if !image.is_file() {
        return Installability::LinkOnly("the AppImage file is gone");
    }
    let Some(folder) = image.parent() else {
        return Installability::LinkOnly("the AppImage has no folder");
    };
    if !writable(image) || !writable(folder) {
        return Installability::LinkOnly("the AppImage or its folder is not writable");
    }
    Installability::InPlace(image.to_path_buf())
}

/// A `.app` bundle cide is running from, unless replacing it would be pointless.
///
/// Two places a `.app` runs from that must not be "updated": the mounted `.dmg` (`/Volumes/…`,
/// read-only, and gone when it is ejected) and App Translocation (`…/AppTranslocation/…`), the
/// randomised read-only copy macOS runs a quarantined app from until it is moved. Writability
/// is not checked: `/Applications` is admin-writable, and the plugin asks for an administrator's
/// password when it needs one.
#[cfg(any(target_os = "macos", test))]
fn macos_installability(exe: &Path) -> Installability {
    let Some(bundle) = exe
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
    else {
        return Installability::LinkOnly("not running from a .app bundle");
    };
    let text = bundle.to_string_lossy();
    if text.starts_with("/Volumes/") {
        return Installability::LinkOnly("running from the disk image; move cide to Applications");
    }
    if text.contains("/AppTranslocation/") {
        return Installability::LinkOnly(
            "macOS is running a translocated copy; move cide to Applications",
        );
    }
    Installability::InPlace(bundle.to_path_buf())
}

#[cfg(any(target_os = "linux", test))]
fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c` is a valid NUL-terminated path for the duration of the call.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_skip_silences_that_version_only() {
        assert!(announce("0.11.0", None));
        assert!(!announce("0.11.0", Some("0.11.0")));
        assert!(!announce("0.11.0", Some("v0.11.0")));
        // The next release is newer than the skip, and is announced.
        assert!(announce("0.11.1", Some("0.11.0")));
        // Unparseable values compare verbatim rather than silencing everything.
        assert!(!announce("weird", Some("weird")));
        assert!(announce("0.11.0", Some("weird")));
        assert!(announce("0.11.0", Some("0.11.0-rc.1")));
        assert!(!announce("0.11.0+build.7", Some("0.11.0")));
    }

    #[test]
    fn dev_and_debug_builds_never_check() {
        assert!(never_checks("0.10.2-dev", false, "key").is_some());
        assert!(never_checks("0.11.0", true, "key").is_some());
        assert!(never_checks("0.11.0", false, "  ").is_some());
        assert_eq!(never_checks("0.11.0", false, "key"), None);
    }

    /// The plugin deserializes `plugins.updater` in its `setup`, and a block it refuses — a
    /// missing `pubkey`, a plain-`http` endpoint — is a plugin that fails to start, which is a cide
    /// that fails to start. Parsed here with the plugin's own type, so the refusal is a red test
    /// rather than a window that never opens.
    #[test]
    fn the_checked_in_updater_config_is_one_the_plugin_accepts() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let block = conf
            .pointer("/plugins/updater")
            .expect("plugins.updater must exist, even with an empty key")
            .clone();
        let parsed: tauri_plugin_updater::Config = serde_json::from_value(block).unwrap();
        assert!(
            parsed
                .endpoints
                .iter()
                .all(|url| url.as_str().starts_with(RELEASES)),
            "the endpoint names the repository RELEASES points at"
        );
    }

    #[test]
    fn a_release_page_has_one_v() {
        assert_eq!(release_url("0.11.0"), format!("{RELEASES}/tag/v0.11.0"));
        assert_eq!(release_url("v0.11.0"), format!("{RELEASES}/tag/v0.11.0"));
    }

    #[test]
    fn only_a_writable_appimage_replaces_itself() {
        assert!(matches!(
            linux_installability(None),
            Installability::LinkOnly(_)
        ));
        let dir = std::env::temp_dir().join(format!("cide-updater-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let image = dir.join("cide_0.10.1_amd64.AppImage");
        let _ = std::fs::remove_file(&image);
        assert!(matches!(
            linux_installability(Some(&image)),
            Installability::LinkOnly(_)
        ));
        std::fs::write(&image, b"not really").unwrap();
        assert_eq!(
            linux_installability(Some(&image)),
            Installability::InPlace(image.clone())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dmg_or_translocated_app_is_not_replaced() {
        let installed = Path::new("/Applications/cide.app/Contents/MacOS/cide");
        assert_eq!(
            macos_installability(installed),
            Installability::InPlace(PathBuf::from("/Applications/cide.app"))
        );
        for exe in [
            "/Volumes/cide/cide.app/Contents/MacOS/cide",
            "/private/var/folders/x/AppTranslocation/ABC/d/cide.app/Contents/MacOS/cide",
            "/usr/local/bin/cide",
        ] {
            assert!(
                matches!(
                    macos_installability(Path::new(exe)),
                    Installability::LinkOnly(_)
                ),
                "{exe}"
            );
        }
    }
}
