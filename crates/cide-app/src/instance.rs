//! One application per profile, enforced at the only moment it can be.
//!
//! # What a second instance costs
//!
//! A profile is a set of file paths — `workspace.json`, `positions.json`, `agent-runs.json`, the
//! four directories Tauri owns — and `cide_core::profile`'s header already spells out what two
//! instances sharing them do: every write is atomic, so nothing is ever corrupt, and the last one
//! to flush its layout simply wins. That is what profiles were invented to stop *between* the
//! installed build and `./run.sh`. It was never enforced *within* one profile, because starting
//! cide twice by hand is not a thing anybody does on purpose.
//!
//! It is a thing that happens by accident. `child_env::editor_env` puts this binary's path in
//! `EDITOR` for every PTY child, so every `claude`, shell pane and agent run in a running cide
//! knows how to execute it; before [`crate::cli`] existed, any argument at all started a whole
//! second IDE. On 2026-09-04 that happened seven times in one morning, and twice the cleanup
//! (`pkill -f mount_cide`, matching the AppImage path every session carries in its `--settings`
//! argv) took down every `claude` in every project of the first instance instead.
//!
//! [`crate::cli`] closes the door the accident came through. This module is the second lock:
//! whatever gets a second cide started, it stops before it can restore a workspace it will later
//! overwrite.
//!
//! # Not a mutex
//!
//! There is no lock file to hold, because a held lock does not survive what this has to survive.
//! A `SIGKILL`ed cide — the OOM killer, `kill -9`, a compositor tearing the session down — runs
//! no `Drop`, and an `flock` released by the kernel would be indistinguishable from a clean exit
//! only until the *next* start found a file nobody owns. So this works the way
//! `cide_claude::orphans` and `cide_ide_mcp::lockfile` already work in this workspace: the file
//! records a pid, and liveness is the question asked of it. A stale file is not an error; it is
//! the ordinary state after a hard kill and is simply taken over.
//!
//! # The bias, stated once
//!
//! **Every doubt starts the application.** A wrong refusal is an IDE that will not launch, with
//! the fix hidden in an environment variable; a wrong start is the state cide shipped in for
//! thirty-odd milestones. So an unreadable file, an unparseable one, a pid that cannot be
//! checked, a platform with no `/proc`, a directory that cannot be written — every one of them
//! is a start, not a stop. The only refusal is a pid that is *established* alive and
//! *established* to be another cide.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

/// Set to any non-empty value to start anyway. Named in [`crate::cli::usage`].
///
/// Its use is `./run.sh --profile default` and nothing else: two builds deliberately pointed at
/// one profile's state, by somebody who knows what that trades. `run.sh` does not need it — it
/// stops the running instance of the profile first and waits for the process to go.
pub const OVERRIDE_VAR: &str = "CIDE_ALLOW_SECOND_INSTANCE";

/// The file, beside `workspace.json` in the profile's state directory.
pub const LOCK_FILE: &str = "instance.lock";

/// Whether *this* process is the one the file names. Read by [`release`], which must not delete
/// a file it never wrote — the refused second instance would otherwise clear the first's claim on
/// its way out and leave the profile unguarded.
static HELD: AtomicBool = AtomicBool::new(false);

/// What a lock file says. JSON like everything else in the state directory, and hand-readable:
/// the first thing anybody does with an unexpected refusal is `cat` this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The process claiming the profile.
    pub pid: u32,
    /// Its executable, for the human reading the file. Never compared: two *different* builds on
    /// one profile is exactly the collision this refuses, so the paths differing proves nothing.
    #[serde(default)]
    pub exe: String,
    /// The profile it claimed, for the same reason. `None` is the default profile.
    #[serde(default)]
    pub profile: Option<String>,
}

/// What to do about a lock file that is already there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Write our own record and start.
    Take,
    /// Refuse, naming the process that has the profile.
    Refuse(u32),
}

/// The profile's lock file.
pub fn lock_path() -> PathBuf {
    cide_core::persist::state_dir().join(LOCK_FILE)
}

/// Claim the profile, or report the pid that already has it.
///
/// Called from `main`, before the graphics ladder and before anything touches GTK: a refusal must
/// not flash a window, and it has to work on a machine with no display.
pub fn acquire() -> Result<(), u32> {
    let path = lock_path();
    let existing = std::fs::read_to_string(&path).ok();
    let overridden = std::env::var_os(OVERRIDE_VAR).is_some_and(|value| !value.is_empty());
    let me = std::process::id();

    if let Verdict::Refuse(holder) = verdict(existing.as_deref(), me, overridden, is_another_cide) {
        return Err(holder);
    }

    let record = Record {
        pid: me,
        exe: std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        profile: cide_core::profile::active().map(str::to_string),
    };
    match serde_json::to_vec_pretty(&record)
        .map_err(Into::into)
        .and_then(|json| cide_core::persist::write_atomic(&path, &json))
    {
        Ok(()) => {
            HELD.store(true, Ordering::SeqCst);
        }
        // A state directory that cannot be written is a real problem, and it is not this one:
        // the workspace writer will report it far more usefully than a launch refusal would.
        // Starting unguarded is the same behaviour as every version before this module.
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "could not claim the profile; a second instance would not be noticed");
        }
    }
    Ok(())
}

/// Drop the claim, if it is ours. Idempotent, and a no-op in a process that never took one.
///
/// Called from `lifecycle`'s teardown. A `SIGKILL` skips it and leaves the file behind, which is
/// the case [`verdict`] treats as ordinary rather than exceptional.
pub fn release() {
    if !HELD.swap(false, Ordering::SeqCst) {
        return;
    }
    release_at(&lock_path(), std::process::id());
}

/// [`release`] against an explicit path, so a test never touches the real profile's file.
///
/// **Re-reads rather than deleting outright.** Between the claim and here, a start that decided
/// we were dead (a `SIGSTOP`ped cide answers no liveness question this module can ask, and a
/// human with `CIDE_ALLOW_SECOND_INSTANCE` answers none at all) may have written its own record
/// over ours. Deleting that would hand the profile to a third process — and the same rule is what
/// stops a *refused* second instance clearing the first one's claim on its way out, which would
/// turn one accidental launch into an unguarded profile.
fn release_at(path: &Path, me: u32) {
    let ours = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| parse(&text))
        .is_some_and(|record| record.pid == me);
    if ours && let Err(error) = std::fs::remove_file(path) {
        tracing::debug!(%error, path = %path.display(), "could not remove this instance's lock file");
    }
}

/// The whole decision, over injected inputs. `alive` answers "is this pid another live cide?".
///
/// Every arm that is not a live, identified cide is a [`Verdict::Take`] — see the module header
/// on which direction doubt resolves in.
pub fn verdict(
    existing: Option<&str>,
    me: u32,
    overridden: bool,
    alive: impl Fn(u32) -> bool,
) -> Verdict {
    if overridden {
        return Verdict::Take;
    }
    // No file, an unreadable one, or bytes that are not a record: nobody has claimed this.
    let Some(record) = existing.and_then(parse) else {
        return Verdict::Take;
    };
    // Our own pid, from a re-entered `acquire` or a pid that wrapped all the way round to us.
    // Either way the holder is this process and there is nothing to refuse.
    if record.pid == me || record.pid == 0 {
        return Verdict::Take;
    }
    if alive(record.pid) {
        Verdict::Refuse(record.pid)
    } else {
        Verdict::Take
    }
}

/// Is `pid` a live process running the same program this one is?
///
/// **Two questions, and the second is why this is not just `pid_is_alive`.** Pids are reused. A
/// lock file left by a `SIGKILL`ed cide names a number that some unrelated process will
/// eventually own, and reading that as "cide is running" is a refusal to launch with no way for
/// the user to see why — the exact wrong-refusal this module's header rules out. So the holder
/// must also *look like* cide.
///
/// Identity is `/proc/<pid>/comm` against our own, rather than the executable path: the two cides
/// that most want to collide on one profile are an installed AppImage and `./target/debug/cide`,
/// whose paths differ and whose `comm` is `cide` for both. Off Linux there is no `/proc` and this
/// answers `false`, which starts the application — the guard simply does not exist there, like
/// `PR_SET_PDEATHSIG` in `cide_core::child_env`. `docs/platforms.md` is where that is recorded.
fn is_another_cide(pid: u32) -> bool {
    if !cide_ide_mcp::lockfile::pid_is_alive(pid) {
        return false;
    }
    let Some(mine) = comm(Path::new("/proc/self/comm")) else {
        return false;
    };
    comm(&PathBuf::from(format!("/proc/{pid}/comm"))).is_some_and(|theirs| theirs == mine)
}

/// A `/proc/<pid>/comm`, trimmed of its newline. `None` when there is no such file to read.
fn comm(path: &Path) -> Option<String> {
    let name = std::fs::read_to_string(path).ok()?.trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// A lock file's contents, or `None` for anything that is not one.
fn parse(text: &str) -> Option<Record> {
    serde_json::from_str(text).ok()
}

/// The sentence a refused start prints. Names the pid, and both ways out.
///
/// Every line of it is load-bearing to somebody who has just been refused: what happened, why it
/// is not merely fussiness (the shared file), how to get two cides on purpose, and how to get
/// this one anyway. A refusal that says only "already running" sends the user to `pkill`, which
/// is how this whole story started.
pub fn refusal(holder: u32) -> String {
    let profile = cide_core::profile::active().unwrap_or("default");
    format!(
        "cide: another cide (pid {holder}) is already running under the '{profile}' profile.\n\
         Two instances share one workspace.json, so the last one to exit would overwrite the\n\
         other's windows, tabs and panes. This one is not starting.\n\
         \n\
         To run a second cide with state of its own:  CIDE_PROFILE=<name> cide\n\
         To start anyway, sharing that state:         {OVERRIDE_VAR}=1 cide\n\
         The claim is {}, and is ignored once pid {holder} is gone.",
        lock_path().display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(pid: u32) -> String {
        serde_json::to_string(&Record {
            pid,
            exe: "/usr/bin/cide".into(),
            profile: None,
        })
        .unwrap()
    }

    const ALIVE: fn(u32) -> bool = |_| true;
    const DEAD: fn(u32) -> bool = |_| false;

    #[test]
    fn a_live_holder_is_refused() {
        assert_eq!(
            verdict(Some(&record(4242)), 7, false, ALIVE),
            Verdict::Refuse(4242)
        );
    }

    /// The ordinary state after a `SIGKILL`. Must not need a human to clear it.
    #[test]
    fn a_dead_holders_file_is_taken_over() {
        assert_eq!(verdict(Some(&record(4242)), 7, false, DEAD), Verdict::Take);
    }

    #[test]
    fn no_file_is_a_start() {
        assert_eq!(verdict(None, 7, false, ALIVE), Verdict::Take);
    }

    /// Every unreadable shape resolves towards launching. A corrupt lock file must never be the
    /// reason somebody's IDE stops opening.
    #[test]
    fn nonsense_in_the_file_is_a_start() {
        for text in ["", "   ", "not json", "{}", "{\"pid\":\"twelve\"}", "[]"] {
            assert_eq!(
                verdict(Some(text), 7, false, ALIVE),
                Verdict::Take,
                "{text:?} refused a launch"
            );
        }
    }

    #[test]
    fn our_own_pid_is_not_a_stranger() {
        assert_eq!(verdict(Some(&record(7)), 7, false, ALIVE), Verdict::Take);
        // 0 addresses a process group in `kill`, so it is never a holder — the same rule
        // `cide_ide_mcp::lockfile::pid_is_alive` states.
        assert_eq!(verdict(Some(&record(0)), 7, false, ALIVE), Verdict::Take);
    }

    #[test]
    fn the_override_starts_regardless() {
        assert_eq!(verdict(Some(&record(4242)), 7, true, ALIVE), Verdict::Take);
    }

    /// A record written by this build must be readable by it, including the older shape that
    /// carried nothing but a pid — the fields it does not have are the ones a human never sees.
    #[test]
    fn a_record_round_trips_and_tolerates_a_bare_pid() {
        let written = record(99);
        assert_eq!(parse(&written).unwrap().pid, 99);
        assert_eq!(parse("{\"pid\":99}").unwrap().pid, 99);
    }

    fn temp_lock(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cide-instance-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create the temp dir");
        dir.join(LOCK_FILE)
    }

    /// The ordinary exit: we wrote it, so we take it away.
    #[test]
    fn a_claim_of_ours_is_dropped() {
        let path = temp_lock("ours");
        std::fs::write(&path, record(7)).unwrap();
        release_at(&path, 7);
        assert!(!path.exists(), "our own claim outlived the teardown");
    }

    /// The one that matters: a second cide that was *refused* also runs the teardown on its way
    /// out. If release deleted whatever it found, one accidental `cide --help` would leave the
    /// running instance's profile unguarded — the exact door this module closed.
    #[test]
    fn somebody_elses_claim_survives_our_teardown() {
        let path = temp_lock("theirs");
        std::fs::write(&path, record(4242)).unwrap();
        release_at(&path, 7);
        assert!(
            path.exists(),
            "a refused instance deleted the holder's claim"
        );
        assert_eq!(
            parse(&std::fs::read_to_string(&path).unwrap()).unwrap().pid,
            4242
        );
    }

    /// A file already gone, or bytes that are not a record. Neither is worth a failure on a path
    /// whose caller is a process that is exiting anyway.
    #[test]
    fn releasing_nothing_is_quiet() {
        release_at(&temp_lock("absent"), 7);
        let corrupt = temp_lock("corrupt");
        std::fs::write(&corrupt, "not json").unwrap();
        release_at(&corrupt, 7);
        assert!(corrupt.exists(), "an unreadable claim was deleted anyway");
    }

    /// The refusal has to carry the way out, or it sends the reader to `pkill`.
    #[test]
    fn the_refusal_names_both_escapes() {
        let text = refusal(4242);
        assert!(text.contains("4242"));
        assert!(text.contains("CIDE_PROFILE"));
        assert!(text.contains(OVERRIDE_VAR));
    }

    /// Liveness alone is not identity, and a stale file naming a *reused* pid is the case that
    /// proves it. Pid 1 stands in for the reuse: alive on any unix, and not a cide. Asked of
    /// this process instead the question would be trivially true — its `comm` is the one being
    /// compared against — which is why `verdict` answers our own pid before ever calling here.
    #[test]
    fn identity_is_asked_as_well_as_liveness() {
        assert!(cide_ide_mcp::lockfile::pid_is_alive(1));
        assert!(!is_another_cide(1), "init was taken for a running cide");
    }

    /// `pid_is_alive` answers *true* for a pid it cannot even convert, deliberately: its bias is
    /// never to delete a file on a doubt. The identity half is what stops that becoming a
    /// refusal here, and it is the only thing that does.
    #[test]
    fn an_impossible_pid_is_not_a_holder() {
        assert!(cide_ide_mcp::lockfile::pid_is_alive(u32::MAX));
        assert!(!is_another_cide(u32::MAX));
    }
}
