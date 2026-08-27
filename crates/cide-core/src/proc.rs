//! Who forked whom: a pid's ancestry, and the nearest ancestor something owns.
//!
//! # The bug this exists for
//!
//! cide attributes a connected `claude` to a pane by **pid equality**: `pane_bind_session`
//! records the pid of the process cide forked into the pty, the CLI announces itself over the
//! IDE socket with `ide_connected {pid: process.pid}`, and the two are joined by a hash lookup
//! (`cide_ide_mcp::server`'s `pane_of_pid`). That join is correct exactly when the process that
//! opened the socket **is** the process cide forked, and there are three ordinary ways for it
//! not to be:
//!
//! * A **launcher**. `claude` on a given machine may be a wrapper that *spawns* the real CLI
//!   rather than `exec`ing it — a version manager, a `bbin agent claude`-style shim, an npm
//!   launcher that forks a platform binary. The wrapper is the pty child; the CLI is its child.
//!   This is the reported case, and it is why the failure looked platform-specific: the same
//!   user's Linux box has `~/.local/bin/claude` as a symlink straight to the executable, so
//!   `execve` produced one process and the pids matched.
//! * A **shell pane**. A user who types `claude` into a cide shell gets a grandchild of the
//!   pane's `$SHELL`. `cide_ide_mcp::server`'s own notes already name this case as one that
//!   received no addressed notification ever. What that case gains here is *attribution* — the
//!   pane a diff is credited to; whether an `@`-mention may be routed to a shell pane is
//!   `cmd::file::mention_candidates`' decision and is deliberately unchanged.
//! * A **re-exec through a proxy** — the shape `~/.cargo/bin/rust-analyzer` has, and nothing
//!   says the CLI may not grow one.
//!
//! In all three the announced pid is a *descendant* of a pid cide knows. So the join stops
//! being "is this pid one of ours" and becomes "is one of this pid's ancestors one of ours",
//! which is what [`owning_ancestor`] answers.
//!
//! # Why the walk is split from the lookup
//!
//! [`ancestry`] takes the parent lookup as a closure and [`parent_of_pid`] is the real one.
//! That is not ceremony: the platform half cannot be tested on the platform that matters (the
//! reported failure is a Mac and CI's Linux runner cannot produce it), while every rule that
//! can actually be got wrong — the hop cap, stopping at `init`, refusing a cycle, preferring
//! the *nearest* owned ancestor — is pure and is tested here on every host. The same split
//! `cide_core::toolchain::extra_dirs_in` uses, for the same reason.
//!
//! # What this is not
//!
//! Not a process table. There is no listing, no name, no cwd and no command line here — only
//! parentage, because parentage is the only fact the join needs and every other one invites a
//! caller to guess. `cmd::session`'s `cwd_of_pid` is the other single-fact reader, deliberately
//! separate and deliberately Linux-only.

/// How far up the process tree a descendant may be and still be attributed to its pane.
///
/// Generous rather than tight: the three real cases above are one hop (`wrapper → claude`,
/// `shell → claude`) and a version manager that shells out through `env` or `exec-wrapper` is
/// two or three. The cap is there so a lookup that starts misbehaving cannot become an
/// unbounded loop, not because hop eight is meaningfully different from hop seven — and the
/// walk stops at `init` long before it on any healthy machine.
pub const MAX_ANCESTRY_HOPS: usize = 8;

/// Whether [`parent_of_pid`] can answer on this platform at all.
///
/// `false` means every ancestry walk is the single-element chain and the pid join degrades to
/// exactly what it was before this module existed — a wrapper's `claude` is unaddressable, and
/// the sentence `cmd::file::not_connected` prints is the honest one. Written as a constant, and
/// read by the log line in `cide_app::ide`, for the reason [`crate::child_env`]'s
/// `PARENT_DEATH_IS_ENFORCED` is: a platform that silently does less must say so somewhere a
/// reader can find, rather than leave an empty arm that looks like a working one.
pub const PARENT_LOOKUP_WORKS: bool = cfg!(any(target_os = "linux", target_os = "macos"));

/// A pid's parent, or `None` when there is none to have, the process is gone, or this platform
/// cannot say.
///
/// The three answers are deliberately one: a caller that walks upwards stops on all of them,
/// and distinguishing "no such process" from "no such platform" would be a second error path
/// for a question with one useful answer. [`PARENT_LOOKUP_WORKS`] is where the platform fact is
/// stated instead.
///
/// pid 1 (or 0) is reported as `None` rather than walked through. Reaching `init` means the
/// chain has left everything cide forked, and a subreaper's pid is not an ancestor anybody
/// should attribute a conversation to.
#[cfg(target_os = "linux")]
pub fn parent_of_pid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Field 2 is `comm` — the executable's name, in parentheses — and it may itself contain
    // both spaces and a closing parenthesis, which is why this splits on the **last** `)`
    // rather than on whitespace. A `bash` renamed `my ) shell` is not hypothetical enough to
    // ignore: `proc(5)` documents the field as unsanitised and every correct parser in the
    // wild does this.
    let after = stat.rsplit_once(')')?.1;
    // What follows is ` <state> <ppid> …`.
    let ppid: u32 = after.split_whitespace().nth(1)?.parse().ok()?;
    (ppid > 1).then_some(ppid)
}

/// macOS: the same fact, out of `libproc`.
///
/// There is no `/proc` on a Mac. `proc_pidinfo` with the `PROC_PIDT_SHORTBSDINFO` flavour
/// fills one [`libc::proc_bsdshortinfo`] for one pid and the parent is `pbsi_ppid`. It is in
/// libSystem, which is already linked, and needs no entitlement.
///
/// **`sysctl(KERN_PROC_PID)` was written first and does not compile**, which is worth leaving
/// in the record because it is the obvious answer and it is a trap: the `kinfo_proc` it fills
/// is not a type the `libc` crate defines for Apple at all, so reaching for it means either
/// hand-declaring a large kernel struct or adding a dependency. The Darwin type-check in
/// README's *Type-checking for macOS from Linux* is what caught that from a Linux machine, and
/// it is exactly the class of thing that check exists for.
///
/// **The short flavour rather than `PROC_PIDTBSDINFO`**, deliberately: it is the variant the
/// kernel will answer for a process the caller does not own, so a `claude` that has changed uid
/// — or any future caller of this function — gets a parent rather than `EPERM`. The long one
/// carries thirty fields to reach the same `ppid`.
///
/// **A short write is a failure and must be checked as one.** `proc_pidinfo` returns the number
/// of bytes it wrote, not zero-on-success, and it returns 0 for a pid that has gone. A version
/// that only tested for a negative return would read a zeroed struct and report pid 0 as
/// everybody's parent — a lookup that never fails and is always wrong, which is the worst shape
/// available here.
#[cfg(target_os = "macos")]
pub fn parent_of_pid(pid: u32) -> Option<u32> {
    let mut info: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdshortinfo>()).ok()?;

    // SAFETY: `info` is a live, correctly sized `proc_bsdshortinfo` and `size` describes it;
    // the flavour is the one that fills that struct; `arg` is unused by it. Nothing here
    // outlives the call.
    let written = unsafe {
        libc::proc_pidinfo(
            libc::c_int::try_from(pid).ok()?,
            libc::PROC_PIDT_SHORTBSDINFO,
            0,
            (&raw mut info).cast::<libc::c_void>(),
            size,
        )
    };
    if written != size {
        return None;
    }

    (info.pbsi_ppid > 1).then_some(info.pbsi_ppid)
}

/// Everywhere else: no answer, and [`PARENT_LOOKUP_WORKS`] says so.
///
/// Written out rather than left to `#[cfg]` on the two arms above, so that a build for a target
/// nobody considered fails to *attribute* rather than fails to compile. The BSD answer is the
/// same `sysctl` with a differently shaped `kinfo_proc`; adding one is a third arm here and
/// nothing else.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn parent_of_pid(_pid: u32) -> Option<u32> {
    None
}

/// `pid` and its ancestors, nearest first, capped and cycle-proof.
///
/// The first element is always `pid` itself — a chain, not a list of *other* processes — so a
/// caller matching against it gets the "no wrapper at all" case for free instead of writing it
/// out separately. At most `MAX_ANCESTRY_HOPS + 1` elements.
///
/// `parent_of` is a parameter so this is testable without a process tree; [`ancestry_of`] is
/// the one that asks the kernel.
///
/// A repeat stops the walk. A pid cannot really be its own ancestor, but this reads a
/// filesystem or a syscall whose answer is not cide's to trust, and "loop for ever inside a
/// notification handler" is not a failure mode worth leaving open for the sake of two lines.
pub fn ancestry(pid: u32, parent_of: impl Fn(u32) -> Option<u32>) -> Vec<u32> {
    let mut chain = vec![pid];
    let mut current = pid;
    for _ in 0..MAX_ANCESTRY_HOPS {
        let Some(parent) = parent_of(current) else {
            break;
        };
        if chain.contains(&parent) {
            break;
        }
        chain.push(parent);
        current = parent;
    }
    chain
}

/// [`ancestry`] against the real process tree.
pub fn ancestry_of(pid: u32) -> Vec<u32> {
    ancestry(pid, parent_of_pid)
}

/// The nearest pid in `pid`'s ancestry that `owner_of` claims, with what it claimed.
///
/// **Nearest wins, and that is the whole rule.** A `claude` started in a shell pane inside a
/// project whose console pane is also cide's child has two owned ancestors; the shell is the
/// one it is actually running in. Walking from the announced pid outwards and stopping at the
/// first hit is what makes that true by construction rather than by a tie-break somewhere else.
///
/// Returns the ancestor's pid as well as the owner, because the caller has to be able to undo
/// the attribution: `cide_app::ide` records the derived binding against the pid it was derived
/// *from*, so that reaping that child drops the descendant's binding with it rather than
/// leaving a pid the kernel may hand out again pointing at a dead pane.
pub fn owning_ancestor<T>(
    pid: u32,
    owner_of: impl Fn(u32) -> Option<T>,
    parent_of: impl Fn(u32) -> Option<u32>,
) -> Option<(u32, T)> {
    ancestry(pid, parent_of)
        .into_iter()
        .find_map(|p| owner_of(p).map(|owner| (p, owner)))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// A parent lookup over a literal tree, so the rules below are about the walk and not
    /// about whichever processes happen to exist while the suite runs.
    fn tree(edges: &[(u32, u32)]) -> impl Fn(u32) -> Option<u32> + use<> {
        let map: HashMap<u32, u32> = edges.iter().copied().collect();
        move |pid| map.get(&pid).copied()
    }

    #[test]
    fn a_chain_starts_at_the_pid_itself() {
        // The case that matters most: no wrapper. A caller matching down the chain must find
        // the announced pid at hop zero, or the ordinary spawn would need a special case.
        assert_eq!(ancestry(42, tree(&[])), vec![42]);
    }

    #[test]
    fn a_wrapper_shows_up_as_the_second_element() {
        // `bbin agent claude`: cide forked 100, which forked the CLI as 101.
        assert_eq!(ancestry(101, tree(&[(101, 100)])), vec![101, 100]);
    }

    #[test]
    fn the_walk_stops_at_the_hop_cap() {
        // A chain longer than the cap: 0 <- 1 <- 2 <- … Each pid's parent is the one below it.
        let edges: Vec<(u32, u32)> = (1..40).map(|p| (p, p - 1)).collect();
        let chain = ancestry(39, tree(&edges));
        assert_eq!(chain.len(), MAX_ANCESTRY_HOPS + 1);
        assert_eq!(chain[0], 39);
    }

    #[test]
    fn a_cycle_stops_the_walk_instead_of_hanging() {
        // Impossible from a healthy kernel, reachable from a misread. Without the guard this
        // test would not fail — it would never return, inside a notification handler.
        assert_eq!(
            ancestry(1000, tree(&[(1000, 1001), (1001, 1000)])),
            vec![1000, 1001]
        );
    }

    #[test]
    fn the_nearest_owned_ancestor_wins() {
        // The shell case with a console pane above it: 300 is the shell pane's `$SHELL`, 200
        // is a pane cide also owns, 301 is the `claude` the user typed into the shell. It
        // belongs to the shell, not to the outer pane.
        let owners: HashMap<u32, &str> = [(300, "shell-pane"), (200, "console-pane")]
            .into_iter()
            .collect();
        let found = owning_ancestor(
            301,
            |pid| owners.get(&pid).copied(),
            tree(&[(301, 300), (300, 200)]),
        );
        assert_eq!(found, Some((300, "shell-pane")));
    }

    #[test]
    fn an_unowned_chain_is_no_answer_rather_than_a_guess() {
        // Somebody else's `claude`, connected through the lockfile from a Terminal window. It
        // has no ancestor cide forked and must not be attributed to a pane.
        let found = owning_ancestor(9, |_| None::<()>, tree(&[(9, 8), (8, 7)]));
        assert_eq!(found, None);
    }

    /// The platform half, against this process — the only test here that touches a kernel.
    ///
    /// It proves the `/proc` parse and the `sysctl` call agree with `getppid(2)`, which is the
    /// one thing the pure tests above cannot say and the one thing most likely to be wrong
    /// after a port. Skipped where [`PARENT_LOOKUP_WORKS`] is false, because there the honest
    /// answer *is* `None` and asserting otherwise would fail a platform for behaving as
    /// documented.
    #[test]
    #[cfg(unix)]
    fn the_real_lookup_agrees_with_getppid() {
        if !PARENT_LOOKUP_WORKS {
            assert_eq!(parent_of_pid(std::process::id()), None);
            return;
        }
        // SAFETY: `getppid` takes no arguments and cannot fail.
        let expected = unsafe { libc::getppid() } as u32;
        assert_eq!(parent_of_pid(std::process::id()), Some(expected));

        let chain = ancestry_of(std::process::id());
        assert_eq!(chain[0], std::process::id());
        assert_eq!(chain[1], expected);
    }

    #[test]
    #[cfg(unix)]
    fn a_pid_that_does_not_exist_answers_none() {
        // The reaped-child case, which is what a stale binding would otherwise walk into.
        // `0x7fff_ffff` is above every default `pid_max` and is not a live process anywhere.
        assert_eq!(parent_of_pid(0x7fff_ffff), None);
    }
}
