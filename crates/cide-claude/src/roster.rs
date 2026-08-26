//! What the CLI calls its own conversations: the `/rename` name, read off disk.
//!
//! # The gap this fills
//!
//! A Claude pane's `Pane::title` is minted once, by cide, as `<project> : claude`. It is the
//! same string for every Claude pane in a project, which is fine for a tab strip and useless
//! the moment a user is asked *which* conversation to send a selection to: a menu offering
//! four lines that all read `cide : claude` is a menu with no information in it.
//!
//! The CLI already holds the answer. `/rename` writes a `name` into a per-process file under
//! `~/.claude/sessions/<pid>.json`, alongside the `sessionId` that names the conversation —
//! which is the same id cide passes to `--session-id` and keys its registry by. So the name
//! the user typed is one directory listing away, and this module is that listing and nothing
//! else.
//!
//! # This is an internal Claude Code detail and is treated like one
//!
//! `~/.claude/sessions/` is not a documented interface, in the same way
//! `<projects>/<encoded cwd>/<id>.jsonl` is not — see `cide_app::lifecycle::transcript_exists`,
//! which makes the same bargain in the same words. The rules are therefore the same:
//!
//! * **Read-only.** Nothing here opens a file for writing, creates a directory, or removes
//!   anything. A stale entry is skipped, never deleted: those files belong to the CLI.
//! * **Every way of being wrong answers "no name".** A relocated home, a renamed directory, a
//!   field that changes shape, a file half-written by a CLI that is mid-save — each of them
//!   ends as an absent name, and an absent name costs a menu row its label and nothing else.
//!   There is no failure mode here that can cost a send.
//! * **`CLAUDE_CONFIG_DIR` is honoured**, because it relocates `~/.claude` wholesale and a
//!   user who sets it would otherwise get no names at all with nothing on screen to say why.
//!
//! # Why the newest file wins, and why liveness is asked as well
//!
//! The directory is keyed by **pid**, not by conversation, and the CLI does not appear to
//! remove an entry when it exits — a development machine that has been running for a day has
//! several files naming the same `sessionId` from processes that are long gone. Two rules
//! sort that out, and both are needed:
//!
//! * a dead pid is skipped ([`cide_ide_mcp::lockfile::pid_is_alive`], which is shared rather
//!   than re-implemented precisely so the `EPERM`-is-not-death asymmetry is written once); and
//! * among what is left, the largest `updatedAt` wins, so a resumed conversation is described
//!   by the process that is actually running it.
//!
//! Liveness alone is not enough — pids are recycled, and two live CLIs can legitimately share
//! a conversation id after a resume — and recency alone is not enough either, because a dead
//! process's file keeps whatever `updatedAt` it died with and can outrank nothing at all.

use std::collections::HashMap;
use std::path::PathBuf;

/// One `~/.claude/sessions/<pid>.json`, as much of it as anything here reads.
///
/// Deliberately a small subset of a much larger record — the file also carries `cwd`,
/// `startedAt`, `status`, a socket path and the CLI's version. Naming only what is used is
/// what makes a field added or removed upstream a non-event: `serde` ignores unknown keys,
/// and every field below except `session_id` is optional, so a release that drops one costs
/// this module a name rather than a parse.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    /// The conversation the process is on. Matches `--session-id`, and therefore matches
    /// `Pane::conversation` when the CLI has moved on and `Pane::session` when it has not.
    session_id: String,
    /// The process holding it. Absent in no build seen so far, but optional anyway: a record
    /// with no pid cannot be checked for liveness and is skipped rather than trusted.
    pid: Option<u32>,
    /// What `/rename` set. Absent for a conversation nobody has named.
    name: Option<String>,
    /// Milliseconds since the epoch, written when the *name* was set — not when the record
    /// was last touched.
    ///
    /// The only field here that says anything about *which conversation a name was given to*,
    /// and the whole reason the caller can tell a live name from one the CLI carried across a
    /// `/clear`. See [`Named::since`].
    #[serde(default)]
    name_since: u64,
    /// Milliseconds since the epoch, written on every status change. The tiebreak between two
    /// live processes naming one conversation.
    #[serde(default)]
    updated_at: u64,
}

/// A name, and when it was given.
///
/// The pair rather than the string, because a name alone cannot answer the question the
/// caller actually has. The CLI holds its name on the **process**, not on the conversation:
/// `/rename` sets `registeredName` on a per-process singleton, and `/clear` starts a new
/// conversation inside that same process, so the record is rewritten with a new `sessionId`
/// and the *old* name still attached. Read as a bare map, that makes a name the user gave one
/// conversation label the one that replaced it — which is what "`/clear` did not reset the
/// session" looked like from the outside.
///
/// `cide_core::workspace::fresh_claude_names` is where the comparison lives; this module's job
/// ends at reporting both halves honestly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    /// What the user typed, trimmed of nothing — it is their string.
    pub name: String,
    /// `nameSince`: milliseconds since the epoch, or `0` from a CLI that does not write it.
    ///
    /// Zero is deliberately the *oldest possible* value rather than "unknown": a name with no
    /// timestamp loses to any conversation change cide has recorded, so an older CLI degrades
    /// to dropping the name on a cleared pane rather than to showing a stale one.
    pub since: u64,
}

/// `~/.claude/sessions`, or `None` when there is no home to look under.
///
/// `CLAUDE_CONFIG_DIR` relocates the whole of `~/.claude`; honouring it here is the same
/// courtesy `cide_app::lifecycle::claude_projects_dir` pays, and for the same reason — a user
/// who sets it is exactly the user for whom a hardcoded `~/.claude` silently finds nothing.
pub fn sessions_dir() -> Option<PathBuf> {
    let base = match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".claude"),
    };
    Some(base.join("sessions"))
}

/// Conversation id → the name the user gave it, for every live named session on this machine.
///
/// Unnamed conversations are **absent**, not present with an empty string: "nobody has named
/// this" and "somebody named it `""`" are different facts, and the caller's fallback (the
/// pane's own title) is right for the first and wrong for the second.
///
/// Not filtered by project. It could be — the records carry `cwd` — and it deliberately is
/// not: the caller matches by conversation id, which is already narrower than any cwd test
/// could be, and a cwd comparison would additionally have to answer for symlinks, a project
/// with several roots, and a session started in a subdirectory. Matching an id that cide
/// itself minted cannot be wrong in any of those ways.
pub fn names() -> HashMap<String, Named> {
    match sessions_dir() {
        Some(dir) => names_in(&dir),
        None => HashMap::new(),
    }
}

/// [`names`], against a directory named explicitly. The half a test can drive.
pub fn names_in(dir: &std::path::Path) -> HashMap<String, Named> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        // No directory at all is the ordinary state on a machine where `claude` has never
        // run, and it is not worth a log line at any level a user would see.
        return HashMap::new();
    };

    // Keyed by conversation, holding the `updatedAt` that won it, so the newest live record
    // for each conversation is what survives the walk.
    let mut best: HashMap<String, (u64, Named)> = HashMap::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension() != Some(std::ffi::OsStr::new("json")) {
            // The directory also holds `<pid>.<hash>.key` files. Skipping by extension rather
            // than parsing and discarding keeps a credential-shaped file from ever being read.
            continue;
        }
        // Unreadable is another user's file, or one being replaced under us. Neither is an
        // error here: the next call sees it.
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<Record>(&text) else {
            continue;
        };
        let Some(name) = record.name.filter(|n| !n.trim().is_empty()) else {
            continue;
        };
        let Some(pid) = record.pid else {
            continue;
        };
        if !cide_ide_mcp::lockfile::pid_is_alive(pid) {
            continue;
        }
        match best.get(&record.session_id) {
            Some((seen, _)) if *seen >= record.updated_at => {}
            _ => {
                let named = Named {
                    name,
                    since: record.name_since,
                };
                best.insert(record.session_id, (record.updated_at, named));
            }
        }
    }

    best.into_iter()
        .map(|(id, (_, named))| (id, named))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The live pid every fixture below claims, so nothing depends on a stranger's process.
    fn me() -> u32 {
        std::process::id()
    }

    fn write(dir: &std::path::Path, file: &str, body: &str) {
        std::fs::write(dir.join(file), body).expect("fixture written");
    }

    /// The four ways a record is skipped, and the one way it counts.
    ///
    /// Written as one test over one directory because that is how the function meets them —
    /// mixed together in a single listing — and because the interesting assertion is about
    /// what is *absent* from the answer, which only means something when the rejected records
    /// were in the same walk as the accepted one.
    #[test]
    fn only_a_live_named_record_contributes_a_name() {
        let dir = tempdir();
        let pid = me();

        write(
            &dir,
            "1.json",
            &format!(r#"{{"pid":{pid},"sessionId":"alive","name":"agents","updatedAt":10}}"#),
        );
        /*
         * Dead, and the number is chosen rather than picked. Two pids are read as *alive* by
         * `pid_is_alive` no matter what: `0`, because `kill(0, …)` addresses the caller's own
         * process group, and anything that does not fit a `pid_t`, because it cannot be
         * passed to `kill` at all. `i32::MAX` avoids both — it fits, and it is two orders of
         * magnitude above the largest `pid_max` Linux permits (2^22), so no process can hold
         * it. Its `updatedAt` is the highest in the directory on purpose: recency must not
         * be able to resurrect a dead record.
         */
        write(
            &dir,
            "2.json",
            r#"{"pid":2147483647,"sessionId":"dead","name":"gone","updatedAt":99}"#,
        );
        write(
            &dir,
            "3.json",
            &format!(r#"{{"pid":{pid},"sessionId":"unnamed","updatedAt":10}}"#),
        );
        write(
            &dir,
            "4.json",
            &format!(r#"{{"pid":{pid},"sessionId":"blank","name":"   ","updatedAt":10}}"#),
        );
        write(&dir, "5.json", "{ this is not json");
        // A `.key` file is never opened at all; if it were parsed it would be skipped anyway,
        // so the assertion that proves the extension gate is that its *contents* are valid.
        write(
            &dir,
            "6.key",
            &format!(r#"{{"pid":{pid},"sessionId":"key","name":"secret","updatedAt":10}}"#),
        );

        let names = names_in(&dir);
        assert_eq!(names.get("alive").map(|n| n.name.as_str()), Some("agents"));
        assert_eq!(names.len(), 1, "{names:?}");
    }

    /// Two live processes on one conversation: the newer record names it.
    ///
    /// This is a resume — the parent's file lingers with the name it had when the user last
    /// renamed it, and the running process's file carries the current one. Taking either "the
    /// first one the directory listed" or "the lowest pid" would make the answer depend on
    /// filesystem ordering, which is exactly the class of bug that shows up as *"it sometimes
    /// shows the old name"*.
    #[test]
    fn the_newest_live_record_wins_a_shared_conversation() {
        let dir = tempdir();
        let pid = me();
        write(
            &dir,
            "10.json",
            &format!(r#"{{"pid":{pid},"sessionId":"s","name":"old","updatedAt":100}}"#),
        );
        write(
            &dir,
            "11.json",
            &format!(r#"{{"pid":{pid},"sessionId":"s","name":"new","updatedAt":200}}"#),
        );
        assert_eq!(
            names_in(&dir).get("s").map(|n| n.name.as_str()),
            Some("new")
        );
    }

    /// `nameSince` is carried through, and its absence reads as the oldest possible name.
    ///
    /// The field is what lets a caller tell a name given to *this* conversation from one the
    /// CLI carried across a `/clear`, so a record that has it must not be flattened to a bare
    /// string on the way out — and one that does not must not come back as "just now", which
    /// would make the stale name win every comparison it should lose.
    #[test]
    fn a_name_carries_when_it_was_given() {
        let dir = tempdir();
        let pid = me();
        write(
            &dir,
            "20.json",
            &format!(
                r#"{{"pid":{pid},"sessionId":"stamped","name":"proto","nameSince":1700,"updatedAt":1800}}"#
            ),
        );
        write(
            &dir,
            "21.json",
            &format!(r#"{{"pid":{pid},"sessionId":"unstamped","name":"legacy","updatedAt":1800}}"#),
        );
        let names = names_in(&dir);
        assert_eq!(
            names.get("stamped"),
            Some(&Named {
                name: "proto".into(),
                since: 1700
            })
        );
        assert_eq!(names.get("unstamped").map(|n| n.since), Some(0));
    }

    /// A directory that is not there answers empty rather than failing.
    ///
    /// The state of every machine on which `claude` has never run, and of every CI runner.
    #[test]
    fn a_missing_directory_is_not_an_error() {
        assert!(names_in(std::path::Path::new("/nonexistent/cide/sessions")).is_empty());
    }

    /// A scratch directory, cleared on the way in, without a dev-dependency for it.
    ///
    /// `tempfile` is not in this crate's tree and adding it for three tests would put a
    /// dependency in the graph for the sake of a `mkdir`. Cleared on entry rather than
    /// removed on exit because a test that fails leaves its fixture behind either way, and
    /// clearing on entry is the half that also survives a panic. The pid keeps concurrent
    /// runs of the same test binary apart.
    fn tempdir() -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("cide-roster-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }
}
