//! The fuzzy pickers: Ctrl+P over a project's files, and one-shot ranking for the command
//! palette.
//!
//! # Why the file picker is a poll and not a subscription
//!
//! The matcher is still being filled while the user types. Pushing a frame per keystroke
//! *and* per injected batch would put an unbounded number of events on the IPC channel during
//! a walk; instead the overlay asks, and the frame it gets back says whether to ask again
//! ([`cide_ipc::PickerFrame::running`]). One round trip per keystroke plus a poll while
//! indexing is a fixed, small cost, and it keeps the ordering problem — a frame for an old
//! query arriving after a newer one — solvable by echoing the query back.

use cide_fs::FsError;
use cide_ipc::{PickerFrame, PickerItem, ProjectId};
use cide_search::{Candidate, Matcher};
use tauri::State;

use crate::files::FsRegistry;

/// Rows one query may return. The overlay draws about twelve.
const DEFAULT_LIMIT: u32 = 50;

/// Query a project's file picker.
///
/// Answerable from the instant `fs.index` starts: `total` climbing between two calls with
/// the same query is the walk still running, which is what the `6 of 2,418` counter shows.
///
/// `async` over the blocking pool like every handler in `cmd::fs`, and here the reason is
/// sharper than "it might be slow": [`cide_search::Matcher::frame`] deliberately spends up
/// to 10 ms inside `nucleo::tick`, and a keystroke costing 10 ms of the main thread while a
/// walk is injecting is the freeze this milestone is about.
///
/// `libraries` widens the answer to the resolved dependency packages — see
/// [`crate::files::ProjectFs::index_libraries`] for what that set is and what it is not. Off is
/// the default, and off costs exactly what it did before this parameter existed: one matcher,
/// one frame, no merge.
///
/// `libraries` is `Option<bool>` and it has to be: Tauri looks each parameter up by key and a
/// key the frontend did not send reaches an `Option` as `None` but reaches a plain `bool` as
/// `Err("missing required key")`. `session_spawn::wants_fork` documents the same trap at
/// length, and it cost every pane spawn in the app for a release.
#[tauri::command(rename_all = "camelCase")]
pub async fn picker_query(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    query: String,
    limit: Option<u32>,
    libraries: Option<bool>,
) -> Result<PickerFrame, FsError> {
    query_project(&registry, project, query, limit, libraries.unwrap_or(false)).await
}

/// Start the library walk for a project, if one has not run.
///
/// Returns immediately after claiming it — the walk itself is the blocking pool's, and the
/// overlay watches it fill through `picker_query`'s `running` flag and its climbing counter,
/// exactly as it watches the project walk. The same shape as `symbols_index`, and for the same
/// reason: a command that waited would put a `cargo metadata` plus a 130 ms walk in front of the
/// keystroke that asked for it.
///
/// Idempotent. A second press of the toggle, a second window, or a re-open of the overlay finds
/// the walk running or finished and starts nothing.
#[tauri::command(rename_all = "camelCase")]
pub async fn picker_index_libraries(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
) -> Result<(), FsError> {
    let fs = registry.get(project).ok_or(FsError::NoIndex)?;
    // Not awaited. `spawn_blocking` hands the job to the pool and this returns; the walk holds
    // the `Arc`, so the project can be closed underneath it without the matcher being freed
    // while a walker thread is still pushing into it.
    tauri::async_runtime::spawn_blocking(move || {
        fs.index_libraries();
    });
    Ok(())
}

/// [`picker_query`] without Tauri's argument extraction.
///
/// Split out for the same reason as `cmd::fs::index_project`: it is half of the pair the
/// streaming test drives, and `State` cannot be built outside a Tauri app.
pub(crate) async fn query_project(
    registry: &FsRegistry,
    project: ProjectId,
    query: String,
    limit: Option<u32>,
    libraries: bool,
) -> Result<PickerFrame, FsError> {
    let fs = registry.get(project).ok_or(FsError::NoIndex)?;
    // The `Arc` is moved into the job, not borrowed: the project can be closed and removed
    // from the registry while this frame is being taken, and the matcher has to outlive the
    // registry entry for as long as the query is in flight.
    blocking(move || {
        // Read *before* the frame, and the order is the whole correctness argument.
        //
        // `running` is what the overlay uses to decide whether to ask again, but
        // `Matcher::frame` fills it from `nucleo::tick`, which answers a narrower question:
        // "does the scorer still have queued work". Those come apart at the one moment that
        // matters. `FsRegistry::claim` registers the project *before* the walk reads an
        // inode — deliberately, so the picker is answerable from the first instant — so a
        // Ctrl+P in the gap before the first batch lands finds an empty, idle matcher and
        // `nucleo` says `running: false`. The overlay believed it, stopped polling, and left
        // the user an empty picker over a repository that was mid-walk until they typed
        // another character. Opening the picker right after opening a project is the ordinary
        // way to hit that.
        //
        // Reading the flag first is the conservative order: if the walk finishes between here
        // and the frame, this costs one extra poll that comes back settled. Reading it after
        // would let a walk that ended just after the snapshot was taken report `running:
        // false` for a frame that is missing the items it was still pushing.
        let indexing = fs.is_indexing();
        let limit = limit.unwrap_or(DEFAULT_LIMIT) as usize;
        let matcher = fs.matcher();
        matcher.query(&query);

        if !libraries {
            let mut frame = matcher.frame(limit);
            frame.running |= indexing;
            return frame;
        }

        // Read before both frames, for the reason spelled out above: a flag read afterwards can
        // report `running: false` for a frame that is missing the items a walk was still
        // pushing, and the overlay believes it and stops polling.
        let walking = fs.is_indexing_libraries();
        let libraries = fs.library_matcher();
        libraries.query(&query);
        // `limit` rows from each side, merged down to `limit`. Asking each for the full limit is
        // what makes the merge honest: taking half from each would drop a row the project side
        // scored above everything the library side had, in the common case where one side has
        // nothing to offer at all.
        let mut frame = cide_search::merge(
            matcher.frame_scored(limit),
            libraries.frame_scored(limit),
            limit,
        );
        frame.running |= indexing || walking;
        frame
    })
    .await
}

/// Rank a list the caller supplies — the command palette, and anything else with a fixed,
/// small candidate set.
///
/// Same scoring as the file picker on purpose. Two overlays that rank the same query
/// differently reads as a bug even when both answers are defensible on their own.
#[tauri::command(rename_all = "camelCase")]
pub async fn picker_rank(
    query: String,
    items: Vec<PickerItem>,
    limit: Option<u32>,
) -> Result<PickerFrame, FsError> {
    let candidates: Vec<Candidate> = items
        .into_iter()
        .map(|i| Candidate::new(i.text, i.value))
        .collect();
    blocking(move || {
        cide_search::rank(&query, &candidates, limit.unwrap_or(DEFAULT_LIMIT) as usize)
    })
    .await
}

/// The blocking pool, as in `cmd::fs`. A join failure is reported rather than re-panicked;
/// see the note there.
async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> T + Send + 'static,
) -> Result<T, FsError> {
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|error| FsError::Io {
            path: "picker".to_string(),
            message: format!("the worker running this query failed: {error}"),
        })
}

#[cfg(test)]
mod tests {
    //! The library scope, driven end to end from the command layer — without forking `cargo`.
    //!
    //! Every behavioural test here pre-fulfils the *External Libraries* group with directories
    //! this test wrote, which is what `cargo metadata` would otherwise produce. That is not a
    //! shortcut around the interesting part: `index_libraries`'s decision is *what to walk*
    //! given a resolved group, and the resolution itself is `cide-deps`'s, has its own tests,
    //! and forks two toolchains — which is why every behavioural test of `libraries::fill` is
    //! `#[ignore]`d.
    //!
    //! What is asserted here is the half no other test can see: that the group's entries become
    //! picker candidates, that they carry their package's label, that the walk happens at most
    //! once, and that a project scope is *unchanged* by the feature being present.

    use super::*;
    use crate::files::FsEvents;
    use cide_fs::groups::Entry;
    use cide_fs::testing::scratch;
    use cide_ipc::{FsChange, FsStatus};
    use std::sync::Arc;

    struct Silent;
    impl FsEvents for Silent {
        fn status(&self, _project: ProjectId, _status: &FsStatus) {}
        fn changed(&self, _project: ProjectId, _change: &FsChange) {}
    }

    /// A registry holding one walked project, plus a resolved library group beside it.
    ///
    /// The group is filled directly rather than resolved. See the module note.
    async fn fixture(tag: &str) -> (FsRegistry, ProjectId, cide_fs::testing::Scratch) {
        let dir = scratch(tag);
        std::fs::create_dir_all(dir.path().join("project/src")).unwrap();
        std::fs::write(dir.path().join("project/src/lib.rs"), []).unwrap();
        std::fs::write(dir.path().join("project/src/main.rs"), []).unwrap();

        // Two packages, both with a `lib.rs` — which is the failure the provenance chip exists
        // for: three rows reading `RS lib.rs src/lib.rs` and no way to tell them apart.
        for (name, version) in [("serde", "1.0.229"), ("syn", "2.0.87")] {
            let pkg = dir.path().join(format!("registry/{name}-{version}/src"));
            std::fs::create_dir_all(&pkg).unwrap();
            std::fs::write(pkg.join("lib.rs"), []).unwrap();
        }

        let registry = FsRegistry::default();
        let project = ProjectId::new();
        let roots = vec![dir.path().join("project")];
        let events: Arc<dyn FsEvents> = Arc::new(Silent);
        crate::cmd::fs::index_project(
            events,
            &registry,
            project,
            roots,
            cide_fs::Visibility::CONSERVATIVE,
        )
        .await
        .expect("the project index");

        let fs = registry.get(project).expect("the project is registered");
        {
            let mut rows = fs.groups().rows.write();
            rows.show(crate::libraries::GROUP_ID, crate::libraries::GROUP_LABEL);
            rows.fulfil(
                crate::libraries::GROUP_ID,
                vec![
                    Entry::directory("serde", dir.path().join("registry/serde-1.0.229"))
                        .with_detail(Some("1.0.229".into())),
                    Entry::directory("syn", dir.path().join("registry/syn-2.0.87"))
                        .with_detail(Some("2.0.87".into())),
                    // A note row: `rust-src` is not installed. It has no directory, so it
                    // contributes no candidates and must not be mistaken for a package.
                    Entry::note("Standard library sources are not installed."),
                ],
                Some("2".into()),
            );
        }
        (registry, project, dir)
    }

    fn texts(frame: &PickerFrame) -> Vec<String> {
        frame.items.iter().map(|r| r.text.clone()).collect()
    }

    /// Poll the way the overlay does, until the answer settles.
    async fn settled(registry: &FsRegistry, project: ProjectId, libraries: bool) -> PickerFrame {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let frame = query_project(registry, project, "lib.rs".to_string(), Some(50), libraries)
                .await
                .expect("the picker answers for an indexed project");
            if !frame.running || std::time::Instant::now() >= deadline {
                return frame;
            }
            tokio::task::yield_now().await;
        }
    }

    /// **Off by default, and the default costs nothing.**
    ///
    /// The first half of the request. Asserted as an absence *and* as an absence of work: a
    /// build that walked the libraries eagerly and merely hid them would pass a row assertion
    /// and would have spent 130 ms and 12 MB on every project open.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_library_scope_is_off_and_unwalked_until_it_is_asked_for() {
        let (registry, project, _dir) = fixture("picker-libs-default").await;
        let frame = settled(&registry, project, false).await;
        assert_eq!(texts(&frame), vec!["src/lib.rs"]);
        assert_eq!(frame.total, 2, "the project's two files and nothing else");

        let fs = registry.get(project).unwrap();
        assert!(
            !fs.libraries_walked(),
            "asking the picker with the scope off must not walk 593 packages. The whole reason \
             library sources are outside the index is that the cargo registry is hundreds of \
             thousands of files"
        );
        assert_eq!(fs.library_packages(), 0);
        drop(registry.remove(project));
    }

    /// Switched on, the rows arrive, carrying their package.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_scope_adds_library_rows_that_name_the_package_they_came_from() {
        let (registry, project, _dir) = fixture("picker-libs-on").await;
        let fs = registry.get(project).unwrap();
        assert!(fs.index_libraries(), "the first walk runs");

        let frame = settled(&registry, project, true).await;
        assert_eq!(
            frame.items[0].text, "src/lib.rs",
            "the project's own file first. All three score identically for `lib.rs`, so this \
             row is here because of the tie-break and for no other reason — which is the case \
             that decides what an empty query shows too"
        );
        assert_eq!(
            frame.items[0].source, None,
            "and it is not labelled as anybody's library"
        );

        // The two library rows tie with each other, and nucleo's order among equal scores is
        // its own business — asserting one would be pinning an implementation detail of a crate
        // this project cannot upgrade. What has to be true is *which rows* and *what they say*.
        let mut rest: Vec<(String, Option<String>)> = frame.items[1..]
            .iter()
            .map(|r| (r.text.clone(), r.source.clone()))
            .collect();
        rest.sort();
        assert_eq!(
            rest,
            vec![
                (
                    "serde-1.0.229/src/lib.rs".to_string(),
                    Some("serde 1.0.229".to_string())
                ),
                (
                    "syn-2.0.87/src/lib.rs".to_string(),
                    Some("syn 2.0.87".to_string())
                ),
            ],
            "each library path is prefixed with the package directory a user can narrow by \
             typing, and each row carries name AND version. Without the version, two rows \
             reading `lib.rs` are still indistinguishable, which is the failure this is for"
        );
        assert_eq!(
            frame.total, 4,
            "the counter names the set actually being searched: two project files plus two \
             library files. A count that ignored the libraries would say `3 of 2` "
        );
        assert_eq!(
            fs.library_packages(),
            2,
            "the note row is not a package — it has no directory to walk"
        );
        drop(registry.remove(project));
    }

    /// The same query with the scope off, *after* a walk, still answers with the project alone.
    ///
    /// This is the assertion that makes it a toggle rather than a one-way door. nucleo is
    /// append-only, so the only way back is a second matcher — and a build that merged
    /// unconditionally would pass every test above.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn switching_the_scope_back_off_takes_the_library_rows_away_again() {
        let (registry, project, _dir) = fixture("picker-libs-toggle").await;
        let fs = registry.get(project).unwrap();
        fs.index_libraries();
        assert_eq!(settled(&registry, project, true).await.items.len(), 3);

        let off = settled(&registry, project, false).await;
        assert_eq!(texts(&off), vec!["src/lib.rs"]);
        assert_eq!(
            off.total, 2,
            "and the counter goes back too — it is the size of the set being searched, not a \
             running total of everything ever indexed"
        );
        drop(registry.remove(project));
    }

    /// Idempotent, like every other walk in this crate.
    ///
    /// A second press of ⌥L, a second window, or a re-open of the overlay must not re-walk:
    /// `NucleoMatcher::clear` is what a re-walk would begin with, and it would empty the list
    /// the first caller is reading from.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_second_request_does_not_walk_again() {
        let (registry, project, _dir) = fixture("picker-libs-idempotent").await;
        let fs = registry.get(project).unwrap();
        assert!(fs.index_libraries(), "the first call walks");
        assert!(!fs.index_libraries(), "the second declines");
        assert!(!fs.index_libraries(), "and so does the third");

        let frame = settled(&registry, project, true).await;
        assert_eq!(
            frame.total, 4,
            "and nothing was injected twice — a duplicate walk would double the count and put \
             every library file in the list twice"
        );
        drop(registry.remove(project));
    }

    /// A stale lockfile drops the candidates and allows a fresh walk.
    ///
    /// `cargo update` moves versions and `cargo remove` deletes a directory the matcher still
    /// offers, so keeping the old set would have Ctrl+P opening files that no longer exist.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn forgetting_the_libraries_empties_them_and_lets_a_walk_run_again() {
        let (registry, project, _dir) = fixture("picker-libs-stale").await;
        let fs = registry.get(project).unwrap();
        fs.index_libraries();
        assert_eq!(settled(&registry, project, true).await.total, 4);

        fs.forget_libraries();
        assert!(!fs.libraries_walked());
        assert_eq!(fs.library_packages(), 0);
        let after = settled(&registry, project, true).await;
        assert_eq!(
            after.total, 2,
            "the stale candidates are gone rather than lingering until a relaunch"
        );

        assert!(
            fs.index_libraries(),
            "and the next request walks again rather than finding the flag still set"
        );
        assert_eq!(settled(&registry, project, true).await.total, 4);
        drop(registry.remove(project));
    }

    /// Forgetting a project that never walked is free and not an error.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn forgetting_libraries_nobody_walked_does_nothing() {
        let (registry, project, _dir) = fixture("picker-libs-forget-unwalked").await;
        let fs = registry.get(project).unwrap();
        fs.forget_libraries();
        assert!(!fs.libraries_walked());
        assert_eq!(settled(&registry, project, false).await.total, 2);
        drop(registry.remove(project));
    }
}
