//! What the last `claude` to complete the IDE handshake **on this machine** called itself.
//!
//! # Why this exists, and what it is not
//!
//! `cide_ide_mcp::protocol::SUPPORTED_CLI` is a property of the *source tree*: it answers "has
//! anyone checked this version of the protocol", it is evidence-backed and human-updated, and
//! `cargo xtask verify-cli` is what produces the evidence. It says nothing about the machine
//! the app is running on.
//!
//! This answers the other question — "did the CLI on **this** machine actually talk to us" —
//! and it is the only one of the two that can honestly be shown to a user. It is a
//! *per-machine observation*, not a claim about the world, so it does not have the "green on
//! one machine says nothing about another" problem that would make a build-wide flag
//! worthless.
//!
//! # Why it is a sidecar file and not a setting
//!
//! `Workspace.settings` is the user's preferences, persisted to `workspace.json`. This is
//! neither: it is something the app observed. Putting it there would (i) make an observation
//! look like a preference in a screen full of controls the user is expected to change,
//! (ii) make it part of a document that has a schema and migrations for no reason, and
//! (iii) — the one that actually decides it — mean the natural way to write a test for the
//! feature is a test that mutates the developer's own settings file.
//!
//! So it is `state_dir()/claude-handshake.json`, beside `recent.json`, on
//! [`crate::persist::recent_path`]'s own argument: written by the app, not edited by the user,
//! and nobody wants it in a dotfiles repository. Every function here takes its path, so a test
//! drives a temporary directory and the developer's real file is never opened.
//!
//! # Why the record carries the range it was measured against
//!
//! A record of "2.1.231 handshook" is not interpretable a month later, when the build has
//! moved on. Storing `verified_range` alongside it makes a stale record legible **as** stale:
//! the screen can say "this was recorded against 2.1.224–2.1.227, and this build says
//! 2.1.224–2.1.231", which is a different and more useful sentence than silence.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::persist::{state_dir, write_atomic};

/// How stale a record may get before a fresh handshake is worth writing over it.
///
/// # The number is a compromise between two failure modes, both real
///
/// Write on every `ide_connected` and this is a file write per Claude pane per launch — for a
/// value that is almost always identical to the one already there.
///
/// Write only when the *version* changes and the timestamp stops meaning "last handshake" and
/// starts meaning "first time this version ever handshook", which is a much weaker claim: a
/// user whose diffs stopped working today would be shown a date from three weeks ago and have
/// no way to tell whether the CLI still connects at all. That is precisely the question the
/// screen is being read for.
///
/// An hour makes the displayed time honest to within an hour and costs at most one small
/// atomic write per hour of use.
pub const REFRESH_AFTER: Duration = Duration::from_secs(60 * 60);

/// One observation: a CLI completed the whole discovery-and-handshake chain here.
///
/// Reaching this point means a real `claude` read our lockfile, chose the WebSocket transport
/// on `transport == "ws"`, presented the auth header and the `mcp` subprotocol, accepted our
/// `initialize` reply, and announced itself with `ide_connected`. Nothing short of the whole
/// chain produces it. That is what makes it worth recording, and it is why the record is
/// written from the `Connected` handler and not from a spawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Handshake {
    /// What the CLI called itself in `initialize`'s `clientInfo.version`.
    ///
    /// `None` when it named no version. Recorded as an observation in its own right rather
    /// than dropped: "something connected and would not say what it was" is a real state, and
    /// a screen that showed nothing for it would be indistinguishable from a machine where
    /// nothing has ever connected.
    pub version: Option<String>,
    /// When, as Unix milliseconds.
    ///
    /// A number rather than an RFC 3339 string because the only reader is a frontend that
    /// wants to render "3 hours ago", and formatting a timestamp in Rust so the frontend can
    /// parse it back is two conversions to get to the same subtraction.
    pub at_unix_ms: u64,
    /// [`cide_ide_mcp::protocol::verified_range`] as it stood when this was written, so a
    /// stale record is legible as stale rather than as a claim about this build.
    pub verified_range: String,
}

impl Handshake {
    /// A record of an observation made now.
    pub fn now(version: Option<String>, verified_range: String) -> Self {
        Self {
            version,
            at_unix_ms: unix_ms(SystemTime::now()),
            verified_range,
        }
    }
}

/// Unix milliseconds, saturating rather than failing.
///
/// A clock before the epoch is a misconfigured machine, not something the caller can act on,
/// and refusing to record a handshake over it would be a strange way to react.
fn unix_ms(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Where the record lives.
pub fn handshake_path() -> PathBuf {
    state_dir().join("claude-handshake.json")
}

/// Read the record, or `None` when there is not one.
///
/// Never fails, on the same argument as [`crate::persist::load`]: a corrupt or unreadable
/// sidecar must not stop anything, and there is nothing here that cannot be observed again the
/// next time a `claude` connects. Unlike `workspace.json` it is not even quarantined — there
/// is nothing in it a user could want back.
pub fn load(path: &Path) -> Option<Handshake> {
    let raw = std::fs::read(path).ok()?;
    match serde_json::from_slice(&raw) {
        Ok(record) => Some(record),
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "unreadable handshake record");
            None
        }
    }
}

/// Write the record atomically, at 0600.
///
/// Through [`write_atomic`] rather than `fs::write` for the reason that function's own doc
/// gives: the mode is the part nobody should be re-deciding. There is no password in here, but
/// the alternative was a second copy of the temp-file-and-rename dance that drifts.
pub fn save(path: &Path, record: &Handshake) -> Result<()> {
    write_atomic(path, &serde_json::to_vec_pretty(record)?)
}

/// Whether `fresh` is worth writing over `stored`.
///
/// **A pure function, and that is deliberate.** `ide_connected` fires once per Claude pane, in
/// an async handler, in an app that may have several projects open — exactly the shape of code
/// where a debounce written inline is a rule nothing can test. Everything decidable is decided
/// here:
///
/// * nothing stored — write, obviously;
/// * the version changed — write, because that is the news this record exists to carry, and it
///   includes the self-update case that `check_once`'s latch deliberately cannot see;
/// * the range changed — write, because the record is now describing a different build, and a
///   record whose range disagrees with the running build is the one case the screen has a
///   special sentence for;
/// * older than [`REFRESH_AFTER`] — write, so "last handshake" stays a true description rather
///   than quietly becoming "first handshake";
/// * a stored record from the *future* — write. A clock that went backwards would otherwise
///   freeze the record permanently, since no later handshake would ever look newer.
///
/// Otherwise: leave it alone, and the app performs no I/O for the second, third and fourth
/// pane of a session.
pub fn supersedes(stored: Option<&Handshake>, fresh: &Handshake) -> bool {
    let Some(stored) = stored else {
        return true;
    };
    if stored.version != fresh.version || stored.verified_range != fresh.verified_range {
        return true;
    }
    // `checked_sub` rather than a subtraction: `stored` may be newer than `fresh` if the clock
    // moved, and an underflow here would panic in a debug build and wrap to "ancient" in a
    // release one — a difference in behaviour between profiles, in a file write.
    match fresh.at_unix_ms.checked_sub(stored.at_unix_ms) {
        Some(elapsed) => Duration::from_millis(elapsed) >= REFRESH_AFTER,
        None => true,
    }
}

/// Record a handshake if it is news, and answer what the record now says.
///
/// The whole decision in one call, so the application's event handler has no rule in it. A
/// failed write is logged and swallowed: this is an observation about a CLI, and refusing to
/// serve a diff because a sidecar could not be written would be a spectacular
/// over-reaction to a full disk.
pub fn record(path: &Path, fresh: Handshake) -> Handshake {
    let stored = load(path);
    if !supersedes(stored.as_ref(), &fresh) {
        // Unreachable with `stored == None`, so the `expect` cannot fire: `supersedes`
        // answers `true` for a missing record.
        return stored.expect("supersedes answers true when nothing is stored");
    }
    if let Err(error) = save(path, &fresh) {
        tracing::warn!(path = %path.display(), %error, "could not record the CLI handshake");
    }
    fresh
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "cide-handshake-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("a scratch directory");
            Self(path)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn at(ms: u64, version: &str) -> Handshake {
        Handshake {
            version: Some(version.to_string()),
            at_unix_ms: ms,
            verified_range: "2.1.224–2.1.227".into(),
        }
    }

    #[test]
    fn a_first_handshake_is_always_news() {
        assert!(supersedes(None, &at(1_000, "2.1.227")));
    }

    #[test]
    fn a_new_version_is_news_however_recently_the_last_one_was_written() {
        let stored = at(1_000, "2.1.227");
        // One millisecond later, which is well inside the refresh window.
        assert!(supersedes(Some(&stored), &at(1_001, "2.1.231")));
    }

    /// The self-update case, which is the whole reason the record is worth having: the probe
    /// behind the Settings warning is latched for the life of the process and cannot see this.
    #[test]
    fn a_cli_that_updated_under_a_running_app_is_news() {
        let stored = at(0, "2.1.227");
        let fresh = Handshake {
            version: Some("2.1.231".into()),
            at_unix_ms: 60_000,
            verified_range: stored.verified_range.clone(),
        };
        assert!(supersedes(Some(&stored), &fresh));
    }

    #[test]
    fn a_different_build_is_news_even_at_the_same_version() {
        let stored = at(1_000, "2.1.227");
        let fresh = Handshake {
            verified_range: "2.1.224–2.1.231".into(),
            at_unix_ms: 1_001,
            ..stored.clone()
        };
        assert!(
            supersedes(Some(&stored), &fresh),
            "the record now describes a different build's range, and the screen has a \
             sentence for exactly that disagreement"
        );
    }

    /// The debounce, and the reason it exists: `ide_connected` fires once per Claude pane.
    #[test]
    fn the_same_version_again_a_moment_later_is_not_worth_a_write() {
        let stored = at(1_000_000, "2.1.227");
        assert!(!supersedes(Some(&stored), &at(1_000_001, "2.1.227")));
        assert!(!supersedes(Some(&stored), &at(1_000_000, "2.1.227")));
        // Right up to the boundary.
        let just_inside = 1_000_000 + REFRESH_AFTER.as_millis() as u64 - 1;
        assert!(!supersedes(Some(&stored), &at(just_inside, "2.1.227")));
    }

    /// And past it, so "last handshake" stays a true description rather than quietly becoming
    /// "the first time this version ever connected".
    #[test]
    fn the_same_version_an_hour_later_refreshes_the_timestamp() {
        let stored = at(1_000_000, "2.1.227");
        let refreshed = 1_000_000 + REFRESH_AFTER.as_millis() as u64;
        assert!(supersedes(Some(&stored), &at(refreshed, "2.1.227")));
    }

    /// A clock that went backwards must not freeze the record for ever.
    ///
    /// Without the `checked_sub` this is either a panic (debug) or a wrap to "ancient"
    /// (release) — a difference in behaviour between profiles, inside a decision about writing
    /// a file.
    #[test]
    fn a_record_from_the_future_is_replaced_rather_than_believed() {
        let stored = at(9_000_000, "2.1.227");
        assert!(supersedes(Some(&stored), &at(1_000, "2.1.227")));
    }

    /// A CLI that named no version is still an observation, and a different one from silence.
    #[test]
    fn a_connection_that_named_no_version_is_recorded_as_such() {
        let anonymous = Handshake {
            version: None,
            at_unix_ms: 5,
            verified_range: "2.1.224–2.1.227".into(),
        };
        assert!(supersedes(None, &anonymous));
        // And a version arriving afterwards is news over it.
        assert!(supersedes(Some(&anonymous), &at(6, "2.1.227")));
        // As is the reverse, which is what a downgrade to an older CLI looks like.
        assert!(supersedes(Some(&at(6, "2.1.227")), &anonymous));
    }

    #[test]
    fn a_record_round_trips_through_the_file_under_its_wire_names() {
        let dir = TempDir::new("round-trip");
        let path = dir.join("claude-handshake.json");

        assert_eq!(load(&path), None, "no file yet is not an error");

        let record = at(1_700_000_000_000, "2.1.227");
        save(&path, &record).expect("write");
        assert_eq!(load(&path).as_ref(), Some(&record));

        // camelCase on the wire, because the frontend reads this through a hand-mirrored
        // interface in `ui/src/ipc/client.ts` and a snake_case key would arrive as
        // `undefined` and be drawn as "never".
        let raw = fs::read_to_string(&path).expect("read");
        assert!(raw.contains("\"atUnixMs\""), "{raw}");
        assert!(raw.contains("\"verifiedRange\""), "{raw}");
    }

    #[test]
    fn an_unreadable_record_reads_as_no_record_rather_than_failing() {
        let dir = TempDir::new("corrupt");
        let path = dir.join("claude-handshake.json");
        fs::write(&path, b"{ this is not json").expect("write");

        assert_eq!(load(&path), None);

        // And it is overwritten by the next handshake rather than quarantined: there is
        // nothing in this file a user could want back.
        let fresh = at(42, "2.1.227");
        assert_eq!(record(&path, fresh.clone()), fresh);
        assert_eq!(load(&path).as_ref(), Some(&fresh));
    }

    /// `record` is the whole decision, so the pane-storm case is asserted through it rather
    /// than only through `supersedes`.
    #[test]
    fn four_panes_connecting_at_once_write_the_file_once() {
        let dir = TempDir::new("panes");
        let path = dir.join("claude-handshake.json");

        let first = at(1_000_000, "2.1.227");
        assert_eq!(record(&path, first.clone()), first);
        let written = fs::metadata(&path).expect("exists").len();

        for offset in 1..4 {
            let again = at(1_000_000 + offset, "2.1.227");
            assert_eq!(
                record(&path, again),
                first,
                "the stored record is returned unchanged, so the screen shows one time rather \
                 than flickering between four"
            );
        }
        assert_eq!(fs::metadata(&path).expect("exists").len(), written);
    }

    #[test]
    fn the_record_sits_beside_the_other_state_the_app_writes() {
        assert_eq!(handshake_path().parent(), Some(state_dir().as_path()));
        assert!(handshake_path().ends_with("claude-handshake.json"));
    }
}
