//! *External Libraries*: the policy for one synthetic group, and the thread that fills it in.
//!
//! [`crate::groups::ProjectGroups`] is the state — one row space, one lock, one field per
//! policy. This module is the *policy*: which id and label the group has, when it is shown at
//! all, what the placeholder says while `cargo` is running, and what a failure reads like.
//! [`crate::scratches`] is the same shape for the other group and shares nothing with this one
//! but the row space.
//!
//! # Nothing happens when a project opens
//!
//! Not one `stat`. [`ProjectGroups::probe_libraries`] runs from
//! [`ProjectGroups::prepare`](crate::groups::ProjectGroups::prepare) on the **first tree read**
//! — which is the first thing the Explorer does after `project.open`, so in wall-clock terms it
//! is the same moment, but it is the moment where the cost is paid by a caller that is already
//! on the blocking pool and already reading the tree. It is a `has_marker` probe: a handful of
//! `stat`s and at most a `read_dir` per directory to depth two, skipping `target/`,
//! `node_modules/` and dotfiles. That decides whether the header row exists. Nothing is spawned.
//!
//! Resolution happens when the user expands the header, and never before. A project whose
//! *External Libraries* nobody opens costs one probe for the life of the process.
//!
//! # The thread, and why it is not `on_spawn_thread`
//!
//! [`spawn_resolve`] creates a `cide-deps` thread per resolution. It must not use
//! `cide_core::child_env::on_spawn_thread`, which runs its closure on **one** process-global
//! thread and blocks the caller: a job that spawns *and waits for* `cargo metadata` would hold
//! that thread for the whole run — 0.22 s warm, seconds cold, unbounded behind cargo's
//! package-cache lock while a terminal pane builds — and every pane spawn and language-server
//! start in the process would queue behind a file-tree expansion.
//!
//! `arm`'s actual requirement (`PR_SET_PDEATHSIG` fires when the **thread** that forked exits) is
//! met by construction instead: `cide_deps::resolve` forks and waits on this thread, so the
//! thread's lifetime is the child's lifetime plus epsilon. See that function's own note.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use cide_fs::groups::{Entry, Expanded, GroupState};

use crate::files::{FsEvents, ProjectFs};
use crate::groups::ProjectGroups;
use cide_ipc::ProjectId;

/// The group's id. Part of the `cide://group/<id>` sentinel a row carries, so it reaches the
/// frontend and back — `ui/src/sidebar/groupRows.ts` names the same string.
pub const GROUP_ID: &str = "externalLibraries";

/// What the header row says. IDEA's wording, because it is the wording people already look for.
pub const GROUP_LABEL: &str = "External Libraries";

/// What the group shows while a resolver is running.
///
/// A row and not an absence: an empty group is indistinguishable from a broken one, and the
/// quarter-second `cargo metadata` takes is long enough to see.
const RESOLVING: &str = "Resolving dependencies…";

impl ProjectGroups {
    /// Decide whether this project has an *External Libraries* row at all.
    ///
    /// Called once, from `prepare`, which owns the guard — see the module header for what the
    /// probe costs.
    pub(crate) fn probe_libraries(&self, roots: &[PathBuf]) {
        if cide_deps::project_kinds(roots).is_empty() {
            // No Cargo and no Go project under any root. **No header row at all**, which is the
            // deliberate difference from an empty one: a group that promises dependencies and
            // then has none to show is a worse answer than no group, and nothing was promised
            // here. A project that gains a `Cargo.toml` later gets the row on the next relaunch;
            // re-probing on every watcher burst would be a `read_dir` per burst for a row that
            // almost never appears.
            //
            // *Scratches* is deliberately the other way round — see that module. The difference
            // is what the header promises: this one promises somebody else's source, and there
            // is none; that one promises a drawer, and an empty drawer is still a drawer.
            return;
        }
        self.rows.write().show(GROUP_ID, GROUP_LABEL);
    }

    /// Re-`stat` the manifests, and say whether the answer on screen is out of date.
    ///
    /// Called from `fs_tree_count`, which the file tree issues on **every watcher burst** — and a
    /// `cargo build` in a terminal pane produces those continuously. So the cost is the design
    /// constraint, and it is met three ways: two early returns that are one atomic and one lock
    /// each, and `cide_deps::restamp` rather than `cide_deps::stamp` — the latter re-walks each
    /// root to depth two to *find* the manifests, this re-`stat`s the two-to-six files the last
    /// resolution actually read.
    ///
    /// A stamp rather than a watcher subscription, for a reason no amount of care about the
    /// watcher would fix: `Cargo.lock` is gitignored in plenty of repositories, `cide_fs::Filter`
    /// excludes it, and **no `cide://fs-changed` would ever mention it**.
    ///
    /// Answers `None` while a resolver is running or before anything has been resolved: the
    /// stamp is only meaningful against an answer that exists.
    pub fn stale(&self) -> Option<Expanded> {
        if self.resolving.load(Ordering::Acquire) {
            return None;
        }
        if self.rows.read().state(GROUP_ID) != Some(GroupState::Ready) {
            return None;
        }
        {
            let mut held = self.stamp.lock();
            let fresh = cide_deps::restamp(&held);
            if *held == fresh {
                return None;
            }
            *held = fresh;
        }
        self.rows.write().invalidate(GROUP_ID)
    }

    /// Whether this path is one *External Libraries* would hold once it is resolved.
    ///
    /// Cheap and I/O-free: two textual containment tests against
    /// `cide_core::toolchain::dependency_roots`. It exists for [`Self::resolve_now`], which needs
    /// to tell "the reveal is asking about a dependency source we have not listed yet" from "the
    /// reveal is asking about a gitignored project file", **without** running `cargo metadata` to
    /// find out.
    ///
    /// `roots` wins, exactly as it does in `read_only_reason_in`: a vendored crate the user opened
    /// as a project root is their own code, and the index — not this group — is what should have
    /// answered for it.
    /// # Only while the group is still `Unresolved`
    ///
    /// The gate below used to be "does the group exist", which made [`Self::resolve_now`]'s
    /// promise of *at most once per project per process* false in the one case that matters. A
    /// `Ready` group has already listed everything it is ever going to list; a dependency-cache
    /// path it does not contain — a Go module cache path in a Cargo project, a registry crate at
    /// a version belonging to some other project, or **any** such path when the resolution itself
    /// failed — is not going to appear by resolving a second time.
    ///
    /// So every Ctrl+Shift+E over such a tab re-forked `cargo metadata` synchronously, and
    /// `fulfil` replaced the rows, which collapsed whatever the user had expanded. The gesture
    /// that means *show me where this is* threw away the tree it was pointing at.
    ///
    /// `Resolving` is excluded for a different reason: somebody is already doing the work, and
    /// `resolve_now`'s own `claim()` would refuse anyway — asking is just a wasted containment
    /// test and a misleading `true`.
    pub fn is_unlisted_library_path(&self, path: &Path, roots: &[PathBuf]) -> bool {
        if self.rows.read().state(GROUP_ID) != Some(GroupState::Unresolved) {
            return false;
        }
        cide_core::toolchain::read_only_reason(path, roots).is_some()
    }

    /// Resolve the group **on this thread**, so a caller that has just been asked a question can
    /// answer it. Returns false when somebody else is already resolving.
    ///
    /// The one caller is `fs_reveal`, and the case is *Select Opened File* over a tab that Go to
    /// definition opened in `~/.cargo/registry`. The group has never been expanded, so it holds
    /// no packages, so `Groups::reveal` cannot find the file and the command would report **"that
    /// file is not in this project's file tree"** — about a file the user is looking at. That is
    /// a lie, and it is exactly the class of answer this milestone exists to stop giving.
    ///
    /// It blocks, and that is the trade, stated plainly: up to a quarter of a second warm and
    /// longer cold, on a blocking-pool worker, for a gesture that explicitly asked *where is this
    /// file*. Three things make it affordable. It is gated on [`Self::is_unlisted_library_path`],
    /// so the ordinary reveal of a project file never reaches it. It happens at most once per
    /// project per process, because the group is `Ready` afterwards. And the alternative is not
    /// "faster" — it is "wrong".
    ///
    /// It is **not** what an expand does: that one answers the click immediately with a
    /// *Resolving…* row and lets [`spawn_resolve`] finish in the background, because there the
    /// user can see the group and the placeholder is a complete answer.
    pub fn resolve_now(&self, roots: &[PathBuf]) -> bool {
        if !self.claim() {
            return false;
        }
        self.fill(roots);
        true
    }

    /// Put the *Resolving…* row up. Returns false when somebody else already did.
    ///
    /// The claim and the placeholder are one operation on purpose: between "I will resolve" and
    /// "here is a row saying so" there is a window in which the group is expanded and empty, and
    /// that window is exactly the state this feature refuses to have.
    fn claim(&self) -> bool {
        if self.resolving.swap(true, Ordering::AcqRel) {
            return false;
        }
        self.rows.write().start(GROUP_ID, Entry::note(RESOLVING));
        true
    }

    /// Resolve, and install the result. **Blocking, and must run on a thread the caller owns.**
    ///
    /// The `resolving` flag is cleared by a guard rather than at the end of the happy path — a
    /// panic inside `cide_deps` would otherwise leave the flag set for ever, and the consequence
    /// is silent and total: every later expand would decline to start a resolution and the group
    /// would show *Resolving…* until the app was restarted. The same shape as `FsRegistry::claim`'s
    /// `IndexingGuard`, and for the same reason.
    fn fill(&self, roots: &[PathBuf]) {
        struct Guard<'a>(&'a std::sync::atomic::AtomicBool);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        // The stamp is taken *before* the resolution, not after: a `Cargo.lock` rewritten while
        // cargo was reading it must leave the answer looking stale, and a stamp taken afterwards
        // would record the new file against the old answer.
        let stamp = cide_deps::stamp(roots);
        let _guard = Guard(&self.resolving);

        let resolved = cide_deps::resolve(roots);
        let count = resolved.packages.len();
        let mut rows: Vec<Entry> = resolved
            .packages
            .into_iter()
            .map(|package| Entry {
                name: package.name,
                detail: Some(detail_of(package.version, package.note)),
                dir: package.dir.is_some(),
                path: package.dir,
            })
            .collect();
        // Notes last, so a partial answer reads as "here are your dependencies, and here is what
        // went wrong" rather than burying the packages under an error.
        rows.extend(resolved.notes.into_iter().map(Entry::note));

        *self.stamp.lock() = stamp;
        self.rows.write().fulfil(
            GROUP_ID,
            rows,
            // The count on the header, matching how `Explorer` puts the row count on its own.
            // Withheld at zero: `External Libraries  0` beside a row explaining the failure
            // would be two ways of saying the same thing, and one of them looks like a bug.
            (count > 0).then(|| count.to_string()),
        );
    }
}

/// The dim second column of a library row: the version, and the reason if there is one.
///
/// A free function so it can be driven from a test. `fill` around it forks two toolchains, so
/// every behavioural test of *that* has to be `#[ignore]`d — and this is the half a later edit
/// gets subtly wrong with no visible symptom beyond a row that reads oddly.
///
/// Two facts, one column: `serde  1.0.229` and `go-spew  v1.1.1 · not downloaded`. The
/// empty-version arm is not hypothetical tidiness — the SDK row (M15) has no version to show
/// when the probe itself failed, and `Rust   · error: toolchain '1.99.0' is not installed` reads
/// as a missing field rather than as a sentence.
fn detail_of(version: String, note: Option<String>) -> String {
    match (version.is_empty(), note) {
        (true, Some(note)) => note,
        (false, Some(note)) => format!("{version} · {note}"),
        (_, None) => version,
    }
}

/// Start a resolution for `project` on a thread of its own, and emit when it lands.
///
/// Returns immediately. The caller — a `fs_expand` on the blocking pool — answers the click with
/// the *Resolving…* row already in place, and the finished rows arrive as a `cide://fs-status`,
/// which `ui/src/sidebar/Explorer.tsx` has turned into a `refresh()` since M8.
///
/// **`fs-status` and not a new event.** A `cide://libraries` event would need a contract entry,
/// an emitter, a client helper and a subscriber in the Explorer — four new things to carry one
/// bit of news that an existing, live subscriber already reacts to correctly. The reuse is not a
/// shortcut: `refresh` re-reads the count and the visible rows and writes nothing when nothing
/// moved, which is exactly the semantics wanted here.
pub fn spawn_resolve(fs: Arc<ProjectFs>, events: Arc<dyn FsEvents>, project: ProjectId) {
    if !fs.groups().claim() {
        return;
    }
    let roots = fs.root_paths();
    let worker = Arc::clone(&fs);
    let started = std::thread::Builder::new()
        .name("cide-deps".into())
        .spawn(move || {
            worker.groups().fill(&roots);
            // The status carries `rows`, which is the composed count — see `cmd::fs::tree_count`
            // — so the scroller resizes in the same frame the rows appear.
            events.status(project, &worker.status());
        });
    if let Err(error) = started {
        // A thread that could not be created leaves the flag set and the group stuck on
        // *Resolving…* with nothing running, which is the one state worse than a failure —
        // so the failure is written into the group as its own row.
        tracing::warn!(%error, "could not start the dependency resolver");
        fs.groups().resolving.store(false, Ordering::Release);
        fs.groups().rows.write().fulfil(
            GROUP_ID,
            vec![Entry::note(format!(
                "cide could not start a thread to resolve dependencies: {error}"
            ))],
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::TreeRowKind;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cide-libraries-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// The rows of the *External Libraries* group alone, so a second group in the same space
    /// cannot make these assertions read the wrong header.
    fn library_rows(groups: &ProjectGroups) -> Vec<cide_ipc::TreeRow> {
        let all = groups.rows(0, 100);
        let start = match all.iter().position(|row| row.name == GROUP_LABEL) {
            Some(at) => at,
            None => return Vec::new(),
        };
        all.into_iter()
            .skip(start)
            .take_while(|row| row.depth > 0 || row.name == GROUP_LABEL)
            .collect()
    }

    /// The rule that keeps a project with nothing to show from growing an empty twisty.
    #[test]
    fn a_project_with_no_manifest_has_no_header_row() {
        let dir = temp("nomanifest");
        std::fs::write(dir.join("notes.txt"), "").expect("write");
        let groups = ProjectGroups::new();
        groups.probe_libraries(std::slice::from_ref(&dir));
        assert!(library_rows(&groups).is_empty());
        assert_eq!(groups.count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cargo_project_gets_one_collapsed_header_and_nothing_is_spawned() {
        let dir = temp("cargo");
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname=\"x\"\n").expect("write");
        let groups = ProjectGroups::new();
        // Through `prepare`, which owns the once-only guard, and twice — two windows attaching
        // to one project is the ordinary case, not an error one.
        groups.prepare(std::slice::from_ref(&dir));
        groups.prepare(std::slice::from_ref(&dir));
        let rows = library_rows(&groups);
        assert_eq!(rows.len(), 1, "probing twice must not draw two headers");
        assert_eq!(rows[0].kind, TreeRowKind::Group);
        assert_eq!(rows[0].name, GROUP_LABEL);
        assert!(!rows[0].expanded);
        assert_eq!(
            groups.rows.read().state(GROUP_ID),
            Some(GroupState::Unresolved),
            "opening a project must not resolve anything"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The window between "I will resolve" and "there is a row saying so" is the one state this
    /// feature refuses to have, so claiming and placing the placeholder are one operation.
    #[test]
    fn claiming_puts_a_row_up_and_a_second_claim_is_refused() {
        let dir = temp("claim");
        std::fs::write(dir.join("Cargo.toml"), "[package]\n").expect("write");
        let groups = ProjectGroups::new();
        groups.probe_libraries(std::slice::from_ref(&dir));
        groups.expand(&cide_fs::groups::group_path(GROUP_ID));

        assert!(groups.claim());
        let rows = library_rows(&groups);
        assert_eq!(rows.len(), 2, "{rows:#?}");
        assert_eq!(rows[1].kind, TreeRowKind::Note);
        assert_eq!(rows[1].name, RESOLVING);

        assert!(
            !groups.claim(),
            "a second window expanding the same header must not fork a second cargo"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_resolution_that_found_nothing_still_says_something() {
        let dir = temp("empty");
        std::fs::write(dir.join("go.mod"), "module x\n").expect("write");
        let groups = ProjectGroups::new();
        groups.probe_libraries(std::slice::from_ref(&dir));
        groups.expand(&cide_fs::groups::group_path(GROUP_ID));
        groups.claim();
        // Runs the real `go list` when go is installed and reports the missing binary when it is
        // not — either way the assertion is the same, and it is the one that matters: a group
        // that resolved to nothing has a row saying why.
        groups.fill(std::slice::from_ref(&dir));
        let rows = library_rows(&groups);
        assert!(rows.len() >= 2, "{rows:#?}");
        // A note *somewhere* under the header, not at a fixed index. Since M15 the SDK row sits
        // above it — `Go  go1.25.5`, which is a true and useful row and is not an answer to
        // "what does this project depend on". The property being pinned is unchanged and is the
        // only one that was ever meant: a group that resolved to no dependencies has a row
        // saying why, rather than being silently empty.
        let note = rows[1..]
            .iter()
            .find(|row| row.kind == TreeRowKind::Note)
            .unwrap_or_else(|| panic!("a resolution with no dependencies must say so: {rows:#?}"));
        assert!(!note.name.is_empty());
        assert_eq!(groups.rows.read().state(GROUP_ID), Some(GroupState::Ready));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The stamp is what notices a `cargo add` in a terminal pane without a watcher.
    #[test]
    fn a_rewritten_lockfile_invalidates_an_open_group_and_demands_a_new_resolution() {
        let dir = temp("stale");
        std::fs::write(dir.join("Cargo.toml"), "[package]\n").expect("write");
        let roots = vec![dir.clone()];
        let groups = ProjectGroups::new();
        groups.probe_libraries(&roots);
        groups.expand(&cide_fs::groups::group_path(GROUP_ID));
        groups.claim();
        groups.fill(&roots);

        assert_eq!(groups.stale(), None, "nothing moved");
        std::fs::write(dir.join("Cargo.lock"), "version = 4\n").expect("write");
        assert_eq!(
            groups.stale(),
            Some(Expanded::Resolve),
            "an open group emptied by an invalidation must be refilled at once, or it is the \
             silently-empty state this feature exists to prevent"
        );
        assert_eq!(groups.stale(), None, "and only once");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The staleness check runs on **every watcher burst**, so it must not re-walk the roots.
    ///
    /// Asserted by adding a manifest for a *second* toolchain after the resolution and checking
    /// that the answer does not change: a check that re-discovered units would find a Go unit
    /// here, produce a longer stamp, and report the answer stale — which is the shape a
    /// `cide_deps::stamp` call here would have.
    #[test]
    fn the_staleness_check_stats_the_files_it_knows_and_does_not_rediscover_them() {
        let dir = temp("restamp");
        std::fs::write(dir.join("Cargo.toml"), "[package]\n").expect("write");
        let roots = vec![dir.clone()];
        let groups = ProjectGroups::new();
        groups.probe_libraries(&roots);
        groups.expand(&cide_fs::groups::group_path(GROUP_ID));
        groups.claim();
        groups.fill(&roots);

        std::fs::write(dir.join("go.mod"), "module x\n").expect("write");
        assert_eq!(
            groups.stale(),
            None,
            "a manifest for a toolchain this answer never covered is not that answer going \
             stale — and noticing it would mean a read_dir per root on every watcher burst"
        );

        // But the files the answer WAS computed from still count.
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname=\"x\"\n").expect("write");
        assert_eq!(groups.stale(), Some(Expanded::Resolve));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_is_stale_before_anything_has_been_resolved() {
        let dir = temp("fresh");
        std::fs::write(dir.join("Cargo.toml"), "[package]\n").expect("write");
        let roots = vec![dir.clone()];
        let groups = ProjectGroups::new();
        groups.probe_libraries(&roots);
        assert_eq!(
            groups.stale(),
            None,
            "a group nobody has opened has no answer to be stale"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The gate on [`ProjectGroups::resolve_now`], which is what keeps *Select Opened File* from
    /// forking `cargo` every time it is pressed over a gitignored project file.
    #[test]
    fn only_a_dependency_source_outside_every_root_asks_for_a_resolution() {
        let dir = temp("claims");
        std::fs::write(dir.join("Cargo.toml"), "[package]\n").expect("write");
        let roots = vec![dir.clone()];
        let groups = ProjectGroups::new();

        let caches = cide_core::toolchain::dependency_roots();
        let crate_source = caches
            .first()
            .expect("this machine has a dependency cache")
            .join("index.crates.io-1949/serde-1.0.229/src/lib.rs");

        assert!(
            !groups.is_unlisted_library_path(&crate_source, &roots),
            "with no group shown at all there is nothing to resolve — a project in another \
             language must not fork cargo because a tab is open on somebody's crate"
        );

        groups.probe_libraries(&roots);
        assert!(
            groups.is_unlisted_library_path(&crate_source, &roots),
            "a registry source under a shown group is what resolve_now exists for"
        );
        assert!(
            !groups.is_unlisted_library_path(&dir.join("src/main.rs"), &roots),
            "a project file is the index's business, gitignored or not: resolving here would \
             put a quarter of a second on every reveal that misses"
        );
        assert!(
            !groups.is_unlisted_library_path(Path::new("/etc/passwd"), &roots),
            "and a path in neither the project nor a dependency cache is neither's business"
        );

        // Once the group is `Ready` it has listed everything it will ever list, so a
        // dependency-cache path it does not contain will not appear by resolving again — and
        // `fulfil` replacing the rows would collapse whatever the user had expanded.
        //
        // This is the case that made `resolve_now`'s "at most once per project per process" a
        // false promise: a Go module-cache path in a Cargo project, a registry crate belonging to
        // some other project, or *any* such path once resolution has failed, re-forked `cargo
        // metadata` synchronously on every single press of Ctrl+Shift+E — and threw away the tree
        // the gesture was pointing at.
        groups.rows.write().fulfil(GROUP_ID, Vec::new(), None);
        assert_eq!(
            groups.rows.read().state(GROUP_ID),
            Some(GroupState::Ready),
            "the fixture only means anything if the group really reached Ready",
        );
        assert!(
            !groups.is_unlisted_library_path(&crate_source, &roots),
            "a resolved group is not asked again: re-forking cargo would cost a quarter of a \
             second per press and collapse the rows the user had open, to learn nothing new"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Every** root, not `caches.first()`. This is the gate that would have caught M15's
    /// second report, and the way the old one was written is itself the finding.
    ///
    /// The test above takes `dependency_roots().first()` and builds a serde path out of it. That
    /// is self-referential: it can only ever exercise a population the function already returns,
    /// so a root that was *missing* — the rustup toolchains directory, which held every byte of
    /// the standard library — could never make it fail. The end-to-end test in `cmd/fs.rs` has
    /// the same shape and is `#[ignore]`d besides. Coverage of the predicate proved only that
    /// the predicate agreed with itself.
    ///
    /// Sweeping the whole list fixes the *form* of the mistake; the containment test in
    /// `cide_core::toolchain` names the rustup path shape from the outside and fixes the
    /// instance. Both are needed, because only the second can notice an absence.
    #[test]
    fn every_dependency_root_reaches_the_resolution_gate_including_the_toolchains_own_library() {
        let dir = temp("all-roots");
        std::fs::write(dir.join("Cargo.toml"), "[package]\n").expect("write");
        let roots = vec![dir.clone()];
        let groups = ProjectGroups::new();
        groups.probe_libraries(&roots);

        let caches = cide_core::toolchain::dependency_roots();
        assert!(
            caches.len() >= 3,
            "at least the two cargo caches and the go module cache; got {caches:?}"
        );
        for cache in &caches {
            // A plausible file under each root. The predicate is textual, so nothing has to
            // exist — which is the point: it must answer for a toolchain this machine does not
            // happen to have installed just as it does for one it does.
            let file = cache.join("some/nested/source.rs");
            assert!(
                groups.is_unlisted_library_path(&file, &roots),
                "{} is a dependency root, so a file under it must reach resolve_now. If this \
                 fails for the rustup toolchains directory, `std` is invisible to Select opened \
                 file AND writable by Ctrl+S — see cide_core::toolchain::dependency_roots",
                cache.display()
            );
        }

        // The one that matters, spelled out rather than left to the loop, because it is the
        // exact path the bug was reported against.
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            let std_file = home.join(
                ".rustup/toolchains/1.92.0-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/\
                 library/core/src/option.rs",
            );
            assert!(
                groups.is_unlisted_library_path(&std_file, &roots),
                "Go to definition lands here and Ctrl+Shift+E used to answer \"that file is not \
                 in this project's file tree\" about it"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_row_with_no_version_reads_as_a_sentence_and_not_as_a_missing_field() {
        assert_eq!(detail_of("1.0.229".into(), None), "1.0.229");
        assert_eq!(
            detail_of("v1.1.1".into(), Some("not downloaded".into())),
            "v1.1.1 · not downloaded",
            "two facts in one dim column, which is what the separator is for"
        );
        assert_eq!(
            detail_of(
                String::new(),
                Some("error: toolchain '1.99.0' is not installed".into())
            ),
            "error: toolchain '1.99.0' is not installed",
            "and with no version there is nothing to separate it FROM: a leading ` · ` in front \
             of the one thing the row has to say reads as a field that failed to render, which \
             is the opposite of the legibility the note row exists for"
        );
    }

    /// The link that was actually missing: a group holding an SDK row can be *revealed into*.
    ///
    /// Everything either side of this was correct — the reveal walks the groups, the group
    /// materialises an unopened chain — but there was no row for `std` to be found under,
    /// because `cargo metadata` cannot report one. Driven over a fixture group rather than a
    /// real toolchain, so it fails on a machine with no rustup rather than being skipped there.
    #[test]
    fn a_file_under_the_sdk_row_has_a_row_in_the_tree() {
        let dir = temp("sdk-reveal");
        let library = dir.join("library");
        std::fs::create_dir_all(library.join("core/src")).expect("mkdir");
        std::fs::write(library.join("core/src/option.rs"), "// std\n").expect("write");
        let project = dir.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        std::fs::write(project.join("Cargo.toml"), "[package]\n").expect("write");
        let roots = vec![project];
        let groups = ProjectGroups::new();
        groups.probe_libraries(&roots);

        groups.rows.write().fulfil(
            GROUP_ID,
            vec![Entry {
                name: "Rust".to_string(),
                detail: Some("1.92.0-x86_64-unknown-linux-gnu".to_string()),
                dir: true,
                path: Some(library.clone()),
            }],
            Some("1".to_string()),
        );

        assert!(
            groups.reveal(&library.join("core/src/option.rs")).is_some(),
            "with an SDK row present the reveal finds a file under it — this is the link the \
             whole report was about, and it needed no change at all once the row existed"
        );
        assert_eq!(
            groups.reveal(&dir.join("elsewhere/x.rs")),
            None,
            "and a path under no row is still no row: the group answers for what it lists"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
