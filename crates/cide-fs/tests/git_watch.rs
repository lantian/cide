//! The watcher against **real** git repositories. (M18)
//!
//! `tests/watcher.rs` covers the watcher's own properties with hand-made `.git` directories,
//! which is right for "does a burst coalesce" and wrong for everything here: every case below
//! is about a layout only git produces. A slash in a branch name is a *directory* under
//! `refs/heads/`; a fetch writes a ref under `refs/remotes/origin/`; `git pack-refs` deletes
//! every loose ref and leaves one file; a linked worktree splits its state across two
//! directories; a submodule puts its whole git directory somewhere else again. A fixture that
//! modelled those by hand would be a second model of git, and it would agree with the code
//! rather than with git.
//!
//! So these drive the real `git` binary — the same choice, for the same reason, that
//! `cide-git/tests/support` makes.
//!
//! # Every one of these builds the `Filter` *after* the repository is in its final shape
//!
//! That is not incidental, it is the bug. A watch list derived at index time from directories
//! that already exist is what a restart produces, and the defect these were written for —
//! `refs/heads` watched non-recursively — is invisible in the session that *created*
//! `refs/heads/feature/`, because `crate::watch`'s event loop watches any new directory it is
//! told about. It only fails on the next launch. "Works until you restart" is why the setup
//! order matters.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cide_fs::filter::FilterInput;
use cide_fs::testing::scratch;
use cide_fs::{
    BuildOptions, Filter, Index, Root, Visibility, WalkItem, WatchConfig, WatchEvent, Watcher,
};

/// Long enough that a loaded machine does not split a burst, short enough that a failing test
/// finishes. The same value `tests/watcher.rs` settled on.
const QUIET: Duration = Duration::from_millis(400);

/// How long a positive assertion waits. Generous on purpose: these run a `git` subprocess
/// first, and the cost of being wrong here is a flaky test in CI.
const WITHIN: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------------------
// git, without the developer's own configuration in it
// ---------------------------------------------------------------------------------------

/// Run `git` in `dir`, asserting it succeeded.
///
/// The environment is scrubbed per command rather than by pointing `HOME` at a scratch
/// directory once per process, the way `cide-git/tests/support` does: that needs an `unsafe`
/// `set_var` before any thread has started, and these tests run alongside others in the same
/// binary. A global `core.autocrlf`, a `commit.gpgsign`, an `init.defaultBranch` or a
/// `core.excludesFile` would each change a result here for reasons that have nothing to do
/// with the watcher.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "cide tests")
        .env("GIT_AUTHOR_EMAIL", "tests@cide.invalid")
        .env("GIT_COMMITTER_NAME", "cide tests")
        .env("GIT_COMMITTER_EMAIL", "tests@cide.invalid")
        .output()
        .unwrap_or_else(|e| panic!("running git {args:?} in {}: {e}", dir.display()));
    assert!(
        out.status.success(),
        "git {args:?} in {} failed:\n{}\n{}",
        dir.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A repository with one commit on `main`.
fn repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("a.txt"), "a\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "one"]);
}

/// `git rev-parse` for the two halves of a work tree's state.
///
/// This is deliberately **not** a re-implementation of `cide_git::repo::watch_dirs`; it is the
/// oracle that function is supposed to agree with. `cide-fs` cannot call it — `cide-git`
/// dev-depends on `cide-fs` (see its `Cargo.toml`, which explains why), and pointing the same
/// edge back would make the two crates mutually test-dependent for one three-line helper. The
/// unit tests that pin `watch_dirs`' own answer for both worktree kinds live next to it, in
/// `cide-git/src/repo.rs`; what is being tested here is the half `cide-fs` owns — that a
/// `Filter` given both directories watches all of a linked worktree instead of half of it.
fn git_dirs(work: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for flag in ["--git-dir", "--git-common-dir"] {
        let raw = git(work, &["rev-parse", "--path-format=absolute", flag]);
        let path = PathBuf::from(raw.trim());
        if !out.contains(&path) {
            out.push(path);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// the harness
// ---------------------------------------------------------------------------------------

struct Watching {
    _watcher: Watcher,
    rx: mpsc::Receiver<WatchEvent>,
}

/// Index `work`, build a `Filter` over the git directories `git_dirs` names, and start
/// watching. Everything on disk when this is called is what a restart would have found.
fn watch(work: &Path) -> Watching {
    let index = Index::build(
        vec![Root::new(work)],
        BuildOptions::default(),
        &|_: &[WalkItem]| {},
    );
    let roots = vec![work.to_path_buf()];
    let dirs = index.dir_paths();
    let filter = std::sync::Arc::new(Filter::build_with(FilterInput {
        roots: &roots,
        dirs: &dirs,
        git_dirs: &git_dirs(work),
        visibility: Visibility::CONSERVATIVE,
    }));
    let config = WatchConfig {
        roots,
        dirs: index.watch_dirs(&filter),
        debounce: Duration::from_millis(100),
        quiet: QUIET,
        max_wait: Duration::from_secs(30),
        ..WatchConfig::default()
    };
    let (tx, rx) = mpsc::channel();
    let watcher = Watcher::start(config, filter, move |event| {
        let _ = tx.send(event);
    });
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(WatchEvent::Status(status)) => {
            assert_eq!(status.backend, cide_ipc::WatchBackend::Native, "{status:?}");
        }
        other => panic!("expected a status first, got {other:?}"),
    }
    Watching {
        _watcher: watcher,
        rx,
    }
}

/// Every path reported until one of them satisfies `wanted`, or the deadline passes.
///
/// A predicate rather than "the next change" because a git command touches several watched
/// paths at once and the debouncer is free to split them across bursts: `git commit` writes
/// the index, the ref and `COMMIT_EDITMSG` within a few milliseconds of each other, and which
/// tick they land in is the machine's business. Returning as soon as the interesting path
/// arrives keeps a passing test fast and only a failing one slow.
fn until(w: &Watching, wanted: impl Fn(&Path) -> bool) -> (Vec<PathBuf>, bool) {
    let deadline = Instant::now() + WITHIN;
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut git = false;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match w.rx.recv_timeout(left) {
            Ok(WatchEvent::Changed(change)) => {
                git |= change.git;
                let hit = change.paths.iter().any(|p| wanted(p));
                seen.extend(change.paths);
                if hit {
                    return (seen, git);
                }
            }
            Ok(WatchEvent::Status(_)) => continue,
            Err(_) => break,
        }
    }
    (seen, git)
}

/// Whether any path reported within `WITHIN` satisfies `wanted`, with the whole burst for the
/// failure message.
fn saw(w: &Watching, what: &str, wanted: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let (seen, git) = until(w, &wanted);
    assert!(
        seen.iter().any(|p| wanted(p)),
        "nothing matching {what} was reported; the watcher saw {seen:#?}"
    );
    assert!(
        git,
        "the burst carrying {what} must be flagged `FsChange::git` — that flag is what makes \
         the git panel and the branch readout re-read: {seen:#?}"
    );
    seen
}

/// Whether a path ends in this slash-separated suffix.
fn ends(path: &Path, suffix: &str) -> bool {
    let text = path.to_string_lossy().replace('\\', "/");
    text.ends_with(suffix)
}

fn contains(path: &Path, fragment: &str) -> bool {
    path.to_string_lossy().replace('\\', "/").contains(fragment)
}

// ---------------------------------------------------------------------------------------
// bug 1: a slash in a branch name
// ---------------------------------------------------------------------------------------

/// The defect, in the shape it actually reaches a user.
///
/// `feature/login` is a file inside `.git/refs/heads/feature/`. With `refs/heads` watched
/// non-recursively that directory is on nobody's list, and the `Filter` is rebuilt here —
/// standing in for a restart — *after* it exists, so the event loop's watch-a-new-directory
/// rule cannot paper over it the way it does in the session that created the branch.
///
/// The assertion is on the ref path rather than on `change.git`, and it has to be: `git
/// commit` rewrites `.git/index` too, which was already watched, so the flag alone was true
/// before this fix and told you nothing.
#[test]
fn a_commit_on_a_branch_with_a_slash_marks_the_change_after_a_restart() {
    let dir = scratch("watch-git-slash");
    repo(&dir);
    git(&dir, &["checkout", "-q", "-b", "feature/login"]);
    std::fs::write(dir.join("a.txt"), "b\n").unwrap();
    git(&dir, &["commit", "-qam", "on the branch"]);
    assert!(
        dir.join(".git/refs/heads/feature/login").is_file(),
        "the branch has to be a file inside a directory, or this test proves nothing"
    );

    // Everything above is "the previous session". The watcher starts now.
    let w = watch(dir.path());
    std::fs::write(dir.join("a.txt"), "c\n").unwrap();
    git(&dir, &["commit", "-qam", "another"]);

    saw(&w, "refs/heads/feature/login", |p| {
        ends(p, "refs/heads/feature/login")
    });
}

/// The same hole one directory over: `refs/remotes/origin/` is created at clone time, so no
/// session ever sees it appear and no restart ever puts it on a non-recursive watch list.
///
/// Unlike the two cases either side of it, this one *sometimes* passed before the fix, and
/// the reason is worth knowing rather than rediscovering. `git fetch` reads
/// `refs/remotes/origin/` on its way to writing it, `notify`'s inotify mask includes
/// `IN_OPEN`, and a directory event makes `crate::watch`'s event loop watch the new directory
/// — so whether the ref write is seen came down to whether that watch was installed before
/// git got to the write. A watcher that is correct when it wins a race is not a watcher.
#[test]
fn a_fetch_that_only_moves_remote_tracking_refs_marks_the_change() {
    let dir = scratch("watch-git-fetch");
    let origin = dir.join("origin");
    repo(&origin);

    let clone = dir.join("clone");
    git(
        dir.path(),
        &[
            "-c",
            "protocol.file.allow=always",
            "clone",
            "-q",
            origin.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );

    let w = watch(&clone);

    // A commit in origin, then a fetch in the clone. Nothing in the clone's work tree moves
    // and no local branch moves — the only ref that changes is `refs/remotes/origin/main`.
    std::fs::write(origin.join("a.txt"), "b\n").unwrap();
    git(&origin, &["commit", "-qam", "two"]);
    git(
        &clone,
        &["-c", "protocol.file.allow=always", "fetch", "-q", "origin"],
    );

    saw(&w, "a ref under refs/remotes", |p| {
        contains(p, "refs/remotes/")
    });
}

// ---------------------------------------------------------------------------------------
// gap 3: packed refs
// ---------------------------------------------------------------------------------------

/// `git pack-refs` moves every loose ref into one file, and that file has to be watched.
///
/// The repository is packed **before** the `Filter` is built, because `packed-refs` is guarded
/// by `exists()` — see `GIT_WATCHED`, which says why that guard cannot be dropped — so a
/// repository that had none at index time gets no watch for it until the next index. Packing
/// twice is what makes this a test of the watch rather than of the guard.
#[test]
fn pack_refs_marks_the_change() {
    let dir = scratch("watch-git-pack");
    repo(&dir);
    git(&dir, &["pack-refs", "--all"]);
    assert!(dir.join(".git/packed-refs").is_file());

    let w = watch(dir.path());

    // A new branch, then pack again: `packed-refs` is rewritten and the loose ref that was
    // briefly there is deleted.
    git(&dir, &["branch", "later"]);
    git(&dir, &["pack-refs", "--all"]);

    saw(&w, "packed-refs or a ref", |p| {
        ends(p, "packed-refs") || contains(p, "refs/heads/")
    });
}

/// The shape `packed-refs` exists for: a fresh clone whose refs are **only** packed.
///
/// `git clone` writes `packed-refs` and no loose ref at all, so before this change the file
/// that moves when a branch moves was watched by nothing. The commit below creates
/// `refs/heads/main` as a loose ref for the first time — which is itself only visible because
/// `refs` is watched recursively rather than as three named subdirectories.
#[test]
fn a_clone_whose_refs_are_only_packed_still_reports_a_commit() {
    let dir = scratch("watch-git-clone");
    let origin = dir.join("origin");
    repo(&origin);

    let clone = dir.join("clone");
    git(
        dir.path(),
        &[
            "-c",
            "protocol.file.allow=always",
            "clone",
            "-q",
            origin.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    assert!(clone.join(".git/packed-refs").is_file());
    // A clone writes `packed-refs` *and* a loose `refs/heads/main` for the branch it checks
    // out, which is not the state this test is about. `pack-refs` produces it, and it is also
    // the state any repository reaches on its own after `git gc`.
    git(&clone, &["pack-refs", "--all"]);
    assert!(
        !clone.join(".git/refs/heads/main").exists(),
        "the premise: every ref of this repository now lives in packed-refs and nowhere else"
    );

    let w = watch(&clone);
    std::fs::write(clone.join("a.txt"), "b\n").unwrap();
    git(&clone, &["commit", "-qam", "local"]);

    saw(&w, "refs/heads/main", |p| ends(p, "refs/heads/main"));
}

// ---------------------------------------------------------------------------------------
// bug 2: a linked worktree
// ---------------------------------------------------------------------------------------

/// A linked worktree's state is split, and watching one half watches no refs at all.
///
/// `.git` here is a *file* pointing at `<common>/worktrees/<name>/`, which holds `HEAD` and
/// `index` and no `refs`. The `Filter` gets both directories — the pair
/// `cide_git::repo::watch_dirs` returns, taken here from `git rev-parse` so `cide-fs` need not
/// link `cide-git` — and the assertion is that the commit's *ref* is reported, because the
/// per-worktree half alone would have reported the index write and nothing else.
#[test]
fn a_linked_worktree_commit_marks_the_change() {
    let dir = scratch("watch-git-worktree");
    let main = dir.join("main");
    repo(&main);
    let wt = dir.join("wt");
    git(
        &main,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "side"],
    );
    assert!(
        wt.join(".git").is_file(),
        "a linked worktree's `.git` is a file holding `gitdir:`"
    );

    let dirs = git_dirs(&wt);
    assert_eq!(
        dirs.len(),
        2,
        "the split is the premise of this test: {dirs:?}"
    );
    // `refs/` itself is there — git makes it for the handful of genuinely per-worktree refs,
    // `refs/bisect` and friends — and it is empty. The branches are the point: `refs/heads`
    // does not exist here at all, so a filter built from this directory alone watches a
    // worktree's HEAD and index and not one branch.
    assert!(
        !dirs[0].join("refs/heads").exists(),
        "the per-worktree directory holds no branch refs: {dirs:?}"
    );
    assert!(
        dirs[1].join("refs/heads/side").is_file(),
        "they are in the common directory: {dirs:?}"
    );

    let w = watch(&wt);
    std::fs::write(wt.join("a.txt"), "from the worktree\n").unwrap();
    git(&wt, &["commit", "-qam", "in the worktree"]);

    saw(&w, "refs/heads/side, in the common directory", |p| {
        ends(p, "refs/heads/side")
    });
}

/// A submodule's git directory is `<super>/.git/modules/<name>`, which `git_dir`'s `gitdir:`
/// parsing has always resolved — the case that keeps that parsing in the crate.
///
/// Watched as its own project root, which is how a user opens one.
#[test]
fn a_submodule_commit_marks_the_change() {
    let dir = scratch("watch-git-submodule");
    let upstream = dir.join("upstream");
    repo(&upstream);

    let super_ = dir.join("super");
    repo(&super_);
    git(
        &super_,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            upstream.to_str().unwrap(),
            "sub",
        ],
    );
    git(&super_, &["commit", "-qm", "add sub"]);

    let sub = super_.join("sub");
    assert!(sub.join(".git").is_file());

    let w = watch(&sub);
    std::fs::write(sub.join("a.txt"), "changed\n").unwrap();
    git(&sub, &["commit", "-qam", "in the submodule"]);

    saw(&w, "the submodule's own refs/heads/main", |p| {
        contains(p, "modules/sub") && ends(p, "refs/heads/main")
    });
}

// ---------------------------------------------------------------------------------------
// the prerequisite: GIT_NEVER
// ---------------------------------------------------------------------------------------

/// The guard that has no caller yet, asserted from both sides.
///
/// `Filter::is_watched_path` is a `starts_with` over a list, so the day something puts a whole
/// git directory on that list — "just watch `.git`" is a one-line change that looks right —
/// `admits()` becomes true for `.git/objects/**` and the watcher's new-directory rule takes a
/// descriptor on a 256-way fanout per fetch. Both halves are checked: the filter's answer, and
/// that a storm of writes under `.git/objects` produces no event at all.
///
/// The second half is the watcher's watched set observed the only way a test can observe it —
/// by what it reports. A watched directory reports its writes; this one must report none,
/// while a write the watcher *is* supposed to see still arrives, which is what tells "nothing
/// under objects" apart from "the watcher was not running".
#[test]
fn nothing_under_git_objects_is_ever_admitted_or_watched() {
    let dir = scratch("watch-git-objects");
    repo(&dir);
    // A real object directory with a real fanout in it, so the assertions are about paths that
    // exist rather than about paths that happen not to.
    let fanout = dir.join(".git/objects/ab");
    std::fs::create_dir_all(&fanout).unwrap();
    std::fs::write(fanout.join("cdef"), "loose object").unwrap();
    std::fs::create_dir_all(dir.join(".git/lfs/objects")).unwrap();
    std::fs::create_dir_all(dir.join(".git/worktrees/other")).unwrap();
    std::fs::write(dir.join(".git/worktrees/other/HEAD"), "ref: refs/heads/x\n").unwrap();

    let roots = vec![dir.to_path_buf()];
    let filter = Filter::build_with(FilterInput {
        roots: &roots,
        dirs: &[dir.to_path_buf()],
        git_dirs: &git_dirs(dir.path()),
        visibility: Visibility {
            hidden: true,
            ignored: true,
        },
    });

    for path in [
        dir.join(".git/objects"),
        dir.join(".git/objects/ab"),
        dir.join(".git/objects/ab/cdef"),
        dir.join(".git/lfs"),
        dir.join(".git/lfs/objects"),
        dir.join(".git/worktrees"),
        dir.join(".git/worktrees/other"),
        dir.join(".git/worktrees/other/HEAD"),
    ] {
        assert!(
            !filter.admits(&path, path.is_dir()),
            "{} was admitted, with every visibility switch on",
            path.display()
        );
        assert!(
            !filter.watchable(&path, path.is_dir()),
            "{} was watchable",
            path.display()
        );
        assert!(
            !filter.is_git_path(&path),
            "{} counted as a git change",
            path.display()
        );
    }
    assert!(
        filter.watched_paths().iter().all(|w| {
            let text = w.path.to_string_lossy().replace('\\', "/");
            !text.contains("/objects") && !text.contains("/lfs") && !text.contains("/worktrees")
        }),
        "no explicit watch may land inside one of them: {:?}",
        filter.watched_paths()
    );
    // And the entries that *should* be there still are, so the guard is not simply refusing
    // everything under the git directory.
    assert!(filter.admits(&dir.join(".git/HEAD"), false));
    assert!(filter.admits(&dir.join(".git/refs/heads/main"), false));

    // Now the watcher's side of the same question.
    let w = watch(dir.path());
    for i in 0..200 {
        let bucket = dir.join(format!(".git/objects/{:02x}", i % 256));
        std::fs::create_dir_all(&bucket).unwrap();
        std::fs::write(bucket.join(format!("obj{i}")), "x").unwrap();
    }
    // Everything reported in the window, rather than "no change arrived": a recursive watch
    // is set up by walking, and `notify`'s inotify mask includes `IN_OPEN`, so `refs/heads`
    // and `refs/tags` report themselves once as the walk reads them. That noise predates this
    // change — those two directories were separately watched before, with the same mask — and
    // it is harmless (`Index::apply` moves no row for a git path). What must be absent is any
    // path under the three refused directories.
    let quiet = drain(&w, QUIET * 4);
    assert!(
        quiet.iter().all(|p| {
            !contains(p, "/objects") && !contains(p, "/lfs") && !contains(p, "/worktrees")
        }),
        "a fetch's worth of loose objects reached the UI: {quiet:#?}"
    );

    // Not "the watcher was dead": a real ref still arrives.
    git(&dir, &["branch", "proof"]);
    saw(&w, "refs/heads/proof", |p| ends(p, "refs/heads/proof"));
}

/// Every path reported within `within`. For the negative assertions.
fn drain(w: &Watching, within: Duration) -> Vec<PathBuf> {
    let deadline = Instant::now() + within;
    let mut seen = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match w.rx.recv_timeout(left) {
            Ok(WatchEvent::Changed(change)) => seen.extend(change.paths),
            Ok(WatchEvent::Status(_)) => continue,
            Err(_) => break,
        }
    }
    seen
}
