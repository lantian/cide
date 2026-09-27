//! Everything a run started, and ending all of it. (M119)
//!
//! # The bug this exists for
//!
//! Ending a run was `PtySession::kill`: portable-pty's `ChildKiller`, one `SIGHUP` to the pid
//! cide forked. For a `claude` that is enough — its tools run in its process group and the pty
//! hang-up reaches them. For **codex** it reaches nothing but codex. Every command a codex run
//! executes goes through `codex-linux-sandbox`, which starts **its own session**, then
//! `bwrap --unshare-pid --die-with-parent --as-pid-1`, then the command. Neither the signal nor
//! the pty hang-up crosses that session boundary, the wrapper has no death signal of its own,
//! and when codex exits it is reparented to `systemd --user` and keeps `bwrap` — and so the
//! whole pid namespace — alive for ever. selfcraft, 2026-09-26: three Godot `-s` probes burning
//! 1.5 % CPU each for seven hours, three Blender workers holding memory in a worktree that had
//! already been deleted.
//!
//! # Why one kill of the wrapper is enough
//!
//! `bwrap --die-with-parent` dies when the wrapper does; the process it runs `--as-pid-1` is the
//! namespace's init, and when a pid namespace's init dies the kernel `SIGKILL`s every other
//! process in it, whatever session or group it put itself in. So this module does not have to
//! understand codex's layout: killing **every descendant** the run had is a superset of killing
//! the wrapper, and it is equally right for a `claude` whose tool called `setsid`.
//!
//! # Why the tree is read *before* the kill
//!
//! After the child exits, its orphans' `ppid` is the subreaper's, and the chain that said "this
//! was the run's" is gone. `on_exit` fires after `waitpid`, which is too late. So a run that cide
//! ends is [`capture`]d first and [`Tree::finish`]ed after a grace; a run that ended by itself —
//! codex crashing, or a command codex abandoned after its own tool timeout — is found by where
//! it stands instead ([`orphans_in`]), which is why that one is asked only about a run's own
//! worktree and never about a project root, where the user's own processes stand.
//!
//! # What was not done
//!
//! A cgroup per run (`systemd-run --user --scope`) would catch everything with no `/proc` walk.
//! It needs a systemd user manager, a D-Bus round trip per spawn and a second road for every
//! machine without them; the walk below is Linux-only too but has no moving parts outside the
//! kernel. `PR_SET_CHILD_SUBREAPER` on cide itself would keep the orphans cide's children, but
//! then cide must reap *every* grandchild of every pane, which `std::process` and portable-pty
//! both assume they alone do.
//!
//! [`crate::proc`] stays the parentage reader it says it is; this is the process *table* that
//! module deliberately is not.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// One row of the process table: enough to find a subtree and to know a pid is still the
/// process it was when it was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcEntry {
    pub pid: u32,
    pub ppid: u32,
    /// `/proc/<pid>/stat` field 22, in clock ticks since boot. Compared, never interpreted: a
    /// pid the kernel handed to somebody else in the meantime has a different one, and that is
    /// the only question it answers ([`signal_listed`]).
    pub start_time: u64,
}

/// `(ppid, start_time)` out of one `/proc/<pid>/stat` line.
///
/// Split on the **last** `)`, for [`crate::proc::parent_of_pid`]'s reason: `comm` is
/// unsanitised. After it the fields are numbered from 3 (`state`), so `ppid` (4) is index 1 and
/// `starttime` (22) is index 19.
pub fn parse_stat(stat: &str) -> Option<(u32, u64)> {
    let after = stat.rsplit_once(')')?.1;
    let mut fields = after.split_whitespace();
    let ppid = fields.nth(1)?.parse().ok()?;
    let start_time = fields.nth(17)?.parse().ok()?;
    Some((ppid, start_time))
}

/// Every process this user can see, one pass over `/proc`. Empty off Linux.
#[cfg(target_os = "linux")]
pub fn snapshot() -> Vec<ProcEntry> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter_map(|pid| {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            let (ppid, start_time) = parse_stat(&stat)?;
            Some(ProcEntry {
                pid,
                ppid,
                start_time,
            })
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
pub fn snapshot() -> Vec<ProcEntry> {
    Vec::new()
}

/// `root`'s descendants in `table`, parents before children. `root` itself is not included —
/// it is the child the caller kills its own way.
pub fn descendants(root: u32, table: &[ProcEntry]) -> Vec<ProcEntry> {
    let mut found = Vec::new();
    let mut frontier = vec![root];
    while let Some(parent) = frontier.pop() {
        for entry in table.iter().filter(|e| e.ppid == parent && e.pid != root) {
            // A table read from a live `/proc` is not a snapshot in time — a pid can be reused
            // between two reads and appear to be its own ancestor. Refuse to walk it twice.
            if !found.iter().any(|f: &ProcEntry| f.pid == entry.pid) {
                found.push(*entry);
                frontier.push(entry.pid);
            }
        }
    }
    found
}

/// Signal every entry that is still the process it was when it was listed. Returns how many
/// were signalled.
#[cfg(target_os = "linux")]
pub fn signal_listed(list: &[ProcEntry], signal: libc::c_int) -> usize {
    let mut sent = 0;
    for entry in list {
        // pid 1 and 0 turn `kill` into something else entirely; nothing listed here is either.
        if entry.pid <= 1 {
            continue;
        }
        let still = std::fs::read_to_string(format!("/proc/{}/stat", entry.pid))
            .ok()
            .and_then(|stat| parse_stat(&stat))
            .is_some_and(|(_, start)| start == entry.start_time);
        if !still {
            continue;
        }
        let Ok(pid) = i32::try_from(entry.pid) else {
            continue;
        };
        // SAFETY: `kill` takes two integers and touches no memory of this process.
        if unsafe { libc::kill(pid, signal) } == 0 {
            sent += 1;
        }
    }
    sent
}

#[cfg(not(target_os = "linux"))]
pub fn signal_listed(_list: &[ProcEntry], _signal: i32) -> usize {
    0
}

/// What a child had running under it at one moment, kept so it can be ended after the child.
#[derive(Debug, Default)]
pub struct Tree(Vec<ProcEntry>);

/// Read `root`'s descendants now, while the parent chain still says whose they are.
pub fn capture(root: u32) -> Tree {
    Tree(descendants(root, &snapshot()))
}

impl Tree {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// `SIGKILL` whatever of the tree is still alive, now. For the shutdown, which has already
    /// spent its graces on the ladder and is about to exit — a thread would not outlive it.
    pub fn end_now(self) -> usize {
        kill_listed(&self.0)
    }

    /// After `grace`, `SIGKILL` whatever of the tree is still alive, on a thread of its own.
    ///
    /// A grace and not an immediate kill because the child is being asked politely at the same
    /// moment: codex answers `SIGHUP` by ending its turn, and a tool it is waiting on that gets
    /// `SIGKILL` first shows up as a failed command in a transcript somebody may read. Most of
    /// the tree is gone by the time the grace ends; this is for the part that nothing asked.
    /// `SIGKILL` rather than `SIGTERM` because what survives the grace is by definition what
    /// did not act on the hang-up — a Godot `-s` script that never calls `quit()`, a Blender
    /// stuck in PulseAudio.
    pub fn finish(self, grace: Duration, what: String) {
        if self.0.is_empty() {
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("cide-tree-end".into())
            .spawn(move || {
                std::thread::sleep(grace);
                let killed = kill_listed(&self.0);
                if killed > 0 {
                    tracing::info!(killed, "{what}: ended processes it left running");
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "no thread to end what a child left running; ending it now");
        }
    }
}

#[cfg(target_os = "linux")]
fn kill_listed(list: &[ProcEntry]) -> usize {
    signal_listed(list, libc::SIGKILL)
}

#[cfg(not(target_os = "linux"))]
fn kill_listed(_list: &[ProcEntry]) -> usize {
    0
}

/// Whether `pid`'s parent is the one an orphan is reparented to: `init`, or a subreaper — on a
/// systemd machine the user manager, `systemd --user`, which is what selfcraft's orphans hung
/// from. Read by name because the subreaper flag itself is not exposed in `/proc`.
#[cfg(target_os = "linux")]
fn adopted(ppid: u32) -> bool {
    ppid <= 1
        || std::fs::read_to_string(format!("/proc/{ppid}/comm"))
            .is_ok_and(|comm| comm.trim() == "systemd")
}

/// Whether a path read out of `/proc` lies inside `dir`. A cwd whose directory has been
/// removed reads back with ` (deleted)` appended, and that is exactly the case that matters:
/// a worktree removed under a Blender still standing in it.
pub fn stands_in(path: &Path, dir: &Path) -> bool {
    let text = path.to_string_lossy();
    let path = Path::new(text.strip_suffix(" (deleted)").unwrap_or(&text));
    path.starts_with(dir)
}

/// Whether a command line is a codex sandbox wrapper confined to somewhere inside `dir`:
/// `codex-linux-sandbox --sandbox-policy-cwd <dir…>`. Its own cwd is the worktree too, but the
/// flag is the stronger evidence and survives a wrapper that `chdir`s.
pub fn sandboxed_in(cmdline: &[String], dir: &Path) -> bool {
    cmdline
        .windows(2)
        .any(|w| w[0] == "--sandbox-policy-cwd" && Path::new(&w[1]).starts_with(dir))
}

/// Processes left running under `dir` that nobody owns any more: a process that was adopted by
/// `init` or the user manager and stands in `dir` (or is a codex sandbox confined to it), with
/// everything under it. Empty off Linux.
///
/// Only an **adopted** root is taken, which is the whole safety of this: a shell the user opened
/// in a worktree has a live parent — the cide pty, a terminal — and is never listed, however
/// long it has been standing there.
#[cfg(target_os = "linux")]
pub fn orphans_in(dir: &Path) -> Vec<ProcEntry> {
    let table = snapshot();
    let own = std::process::id();
    let mut found: Vec<ProcEntry> = Vec::new();
    for entry in &table {
        if entry.pid == own || !adopted(entry.ppid) {
            continue;
        }
        let cwd: Option<PathBuf> = std::fs::read_link(format!("/proc/{}/cwd", entry.pid)).ok();
        let cmdline: Vec<String> = std::fs::read(format!("/proc/{}/cmdline", entry.pid))
            .map(|bytes| {
                bytes
                    .split(|b| *b == 0)
                    .filter(|s| !s.is_empty())
                    .map(|s| String::from_utf8_lossy(s).into_owned())
                    .collect()
            })
            .unwrap_or_default();
        if cwd.as_deref().is_some_and(|cwd| stands_in(cwd, dir)) || sandboxed_in(&cmdline, dir) {
            found.push(*entry);
            found.extend(descendants(entry.pid, &table));
        }
    }
    found.sort_by_key(|e| e.pid);
    found.dedup_by_key(|e| e.pid);
    found
}

#[cfg(not(target_os = "linux"))]
pub fn orphans_in(_dir: &Path) -> Vec<ProcEntry> {
    Vec::new()
}

/// [`orphans_in`], ended. Returns how many were killed; each root is logged with the start of
/// its command line so "what did cide just kill" is answerable from the log.
pub fn end_orphans_in(dir: &Path) -> usize {
    let orphans = orphans_in(dir);
    if orphans.is_empty() {
        return 0;
    }
    for entry in &orphans {
        let head = std::fs::read(format!("/proc/{}/cmdline", entry.pid))
            .map(|b| {
                let text = String::from_utf8_lossy(&b).replace('\0', " ");
                text.chars().take(120).collect::<String>()
            })
            .unwrap_or_default();
        tracing::info!(pid = entry.pid, dir = %dir.display(), "ending a leftover process: {head}");
    }
    kill_listed(&orphans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(pid: u32, ppid: u32) -> ProcEntry {
        ProcEntry {
            pid,
            ppid,
            start_time: u64::from(pid),
        }
    }

    #[test]
    fn a_stat_line_is_read_past_a_hostile_comm() {
        let stat = "4242 (my ) shell) S 17 4242 4242 0 -1 4194560 1 0 0 0 0 0 0 0 20 0 1 0 987654 \
                    1000 100 18446744073709551615";
        assert_eq!(parse_stat(stat), Some((17, 987_654)));
    }

    /// The codex shape: the wrapper and everything under it hang off the run's child, the
    /// namespace's init is a grandchild of the wrapper, and a sibling pane is left alone.
    #[test]
    fn descendants_cross_every_session_and_stop_at_the_subtree() {
        let table = [
            e(10, 1),  // the run's child
            e(11, 10), // codex-linux-sandbox (its own session)
            e(12, 11), // bwrap
            e(13, 12), // the namespace's pid 1
            e(14, 13), // godot
            e(20, 1),  // somebody else's shell
            e(21, 20),
        ];
        let pids: Vec<u32> = descendants(10, &table).iter().map(|e| e.pid).collect();
        assert_eq!(pids.len(), 4);
        for p in [11, 12, 13, 14] {
            assert!(pids.contains(&p), "{pids:?}");
        }
        assert!(!pids.contains(&10) && !pids.contains(&20) && !pids.contains(&21));
    }

    #[test]
    fn a_cycle_in_a_torn_table_does_not_hang() {
        let table = [e(10, 1), e(11, 10), e(12, 11), e(11, 12)];
        assert_eq!(descendants(10, &table).len(), 2);
    }

    #[test]
    fn a_deleted_worktree_still_contains_what_stood_in_it() {
        let dir = Path::new("/p/.cide/worktrees/sprite-artist-t-520");
        assert!(stands_in(
            Path::new("/p/.cide/worktrees/sprite-artist-t-520/tools (deleted)"),
            dir
        ));
        assert!(stands_in(
            Path::new("/p/.cide/worktrees/sprite-artist-t-520 (deleted)"),
            dir
        ));
        assert!(!stands_in(
            Path::new("/p/.cide/worktrees/sprite-artist-t-5200"),
            dir
        ));
        assert!(!stands_in(Path::new("/p"), dir));
    }

    #[test]
    fn a_sandbox_is_placed_by_its_policy_cwd() {
        let dir = Path::new("/p/.cide/worktrees/ui-dev-t-588");
        let args = |cwd: &str| -> Vec<String> {
            [
                "codex-linux-sandbox",
                "--sandbox-policy-cwd",
                cwd,
                "--command-cwd",
                cwd,
            ]
            .map(String::from)
            .to_vec()
        };
        assert!(sandboxed_in(&args("/p/.cide/worktrees/ui-dev-t-588"), dir));
        assert!(!sandboxed_in(&args("/p/.cide/worktrees/ui-dev-t-592"), dir));
    }

    /// The real thing on the real kernel: a child that `setsid`s a grandchild, read before the
    /// child is killed, and the grandchild ended afterwards although it is in another session
    /// and has been adopted by then.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_setsid_grandchild_is_ended_after_its_parent() {
        use std::process::Command;
        let mut child = Command::new("sh")
            .args(["-c", "setsid sleep 60 & sleep 60; :"])
            .spawn()
            .expect("spawn sh");
        let root = child.id();
        let mut tree = Tree::default();
        for _ in 0..50 {
            tree = capture(root);
            if tree.0.len() >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(tree.0.len() >= 2, "{tree:?}");
        let setsid = tree.0.clone();
        child.kill().expect("kill sh");
        child.wait().expect("reap sh");
        let alive = |list: &[ProcEntry]| {
            list.iter()
                .filter(|e| std::path::Path::new(&format!("/proc/{}", e.pid)).exists())
                .count()
        };
        assert!(
            alive(&setsid) > 0,
            "the grandchildren outlived their parent, as codex's do"
        );
        assert!(kill_listed(&setsid) > 0);
        // Reaped by whoever adopted them; give the kernel a moment.
        for _ in 0..50 {
            let zombies_or_gone = setsid.iter().all(|e| {
                std::fs::read_to_string(format!("/proc/{}/stat", e.pid))
                    .map(|s| {
                        s.rsplit_once(')')
                            .is_some_and(|(_, a)| a.trim_start().starts_with('Z'))
                    })
                    .unwrap_or(true)
            });
            if zombies_or_gone {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the setsid'd grandchild survived");
    }

    /// A reused pid is not killed: an entry whose start time no longer matches is skipped.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_pid_that_is_somebody_else_now_is_left_alone() {
        let me = ProcEntry {
            pid: std::process::id(),
            ppid: 0,
            start_time: 1, // not this process's
        };
        assert_eq!(signal_listed(&[me], 0), 0);
    }
}
