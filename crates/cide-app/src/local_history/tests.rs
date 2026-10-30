//! The capture worker against real files, real walks and a real store, with no window.
//!
//! Each failure mode here has a line in `docs/checks.md`, because each one is silent: nothing
//! errors, and a version simply is not there on the day it is needed.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use cide_fs::budget::Budget;
use cide_fs::{Filter, Visibility};
use cide_ipc::FsChange;
use cide_local_history::{Head, MAX_ROWS, Policy, RootId, Store, VersionKind, VersionSource};

use super::worker::{Capture, Limits, RestoreRequest, Restored, Scope, Timing, TreeRequest};
use super::{PendingSave, Registry};

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "cide-lh-app-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

/// Every delay zero, so `settle` means "done" and not "done after a timer".
fn no_delays() -> Timing {
    Timing {
        delete_recheck: Duration::ZERO,
        scan_gap: Duration::ZERO,
        backstop: Duration::from_secs(3600),
        compact_every: Duration::from_secs(3600),
    }
}

fn limits() -> Limits {
    Limits {
        max_file_size: 1 << 20,
        policy: Policy {
            days_to_keep: 5,
            max_storage: 1 << 30,
            max_rows: MAX_ROWS,
        },
    }
}

/// `show ignored files` on, as shipped, because that is the setting that must not leak in.
fn scope_of(roots: &[PathBuf], rules: &[String]) -> Scope {
    let shown = Visibility {
        hidden: true,
        ignored: true,
    };
    let filter = Filter::build(roots, roots.iter().map(PathBuf::as_path), shown);
    Scope::new(
        roots.to_vec(),
        Arc::new(filter),
        rules,
        Budget::standalone(),
    )
    .unwrap()
}

fn start_with(store: &Path, root: &Path, limits: Limits) -> Arc<Capture> {
    Capture::start(
        store.to_path_buf(),
        root.to_path_buf(),
        scope_of(&[root.to_path_buf()], &[]),
        limits,
        no_delays(),
        false,
        Box::new(|| {}),
        None,
    )
}

fn start(store: &Path, root: &Path) -> Arc<Capture> {
    start_with(store, root, limits())
}

fn stop(capture: Arc<Capture>) {
    if let Some(thread) = capture.stop_and_take() {
        thread.join().unwrap();
    }
}

fn changed(capture: &Capture, paths: &[&Path]) {
    capture.changed(&FsChange {
        paths: paths.iter().map(|p| p.to_path_buf()).collect(),
        truncated: false,
        git: false,
    });
}

/// Single-root tests all use the first id. Asserted, because a root that was never recorded would
/// otherwise look like a store with nothing in it.
fn root_id(store: &Store) -> RootId {
    assert!(!store.roots().is_empty(), "the worker recorded its root");
    RootId(0)
}

fn head(capture: &Capture, rel: &str) -> Option<Head> {
    capture
        .with_store(|store| store.head(root_id(store), rel))
        .flatten()
}

fn kinds(capture: &Capture, rel: &str) -> Vec<VersionKind> {
    capture
        .with_store(|store| {
            let mut versions: Vec<_> = store
                .versions(root_id(store), rel)
                .iter()
                .map(|version| version.kind)
                .collect();
            versions.reverse();
            versions
        })
        .unwrap_or_default()
}

fn text_of(capture: &Capture, rel: &str) -> String {
    capture
        .with_store(|store| {
            let id = store.head(root_id(store), rel).unwrap().id;
            String::from_utf8(store.blob(id).unwrap().read().unwrap()).unwrap()
        })
        .unwrap()
}

#[test]
fn the_first_pass_baselines_project_files_and_nothing_else() {
    let (root, store) = (scratch("baseline"), scratch("baseline-store"));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("target/debug")).unwrap();
    fs::create_dir_all(root.join(".cide/worktrees/run")).unwrap();
    fs::create_dir_all(root.join(".github")).unwrap();
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join(".gitignore"), "target/\n").unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(root.join(".github/ci.yml"), "on: push\n").unwrap();
    fs::write(root.join("target/debug/x.o"), "object").unwrap();
    fs::write(root.join(".cide/config.json"), "{}").unwrap();
    fs::write(root.join(".cide/worktrees/run/main.rs"), "copy").unwrap();
    fs::write(root.join(".git/HEAD"), "ref").unwrap();

    let capture = start(&store, &root);
    capture.settle();

    assert_eq!(kinds(&capture, "src/main.rs"), [VersionKind::Baseline]);
    assert_eq!(text_of(&capture, "src/main.rs"), "fn main() {}\n");
    assert_eq!(kinds(&capture, ".github/ci.yml"), [VersionKind::Baseline]);
    assert_eq!(kinds(&capture, ".gitignore"), [VersionKind::Baseline]);
    for refused in [
        "target/debug/x.o",
        ".cide/config.json",
        ".cide/worktrees/run/main.rs",
        ".git/HEAD",
    ] {
        assert!(head(&capture, refused).is_none(), "{refused}");
    }
    assert!(
        capture
            .with_store(|store| store.baseline_done(root_id(store)))
            .unwrap()
    );
    stop(capture);
}

/// Files that were simply there when the project opened must never read as created. A folder
/// restore deletes on `Created`, so this is the difference between a restore and data loss.
#[test]
fn a_walk_sighting_is_never_a_creation_but_a_watched_arrival_is() {
    let (root, store) = (scratch("created"), scratch("created-store"));
    fs::write(root.join("old.rs"), "old").unwrap();
    let capture = start(&store, &root);
    capture.settle();

    fs::write(root.join("new.rs"), "new").unwrap();
    changed(&capture, &[&root.join("new.rs")]);
    capture.settle();

    assert_eq!(kinds(&capture, "old.rs"), [VersionKind::Baseline]);
    assert_eq!(kinds(&capture, "new.rs"), [VersionKind::Created]);
    stop(capture);
}

#[test]
fn a_change_the_watcher_reports_is_a_version_and_its_bytes_are_the_new_ones() {
    let (root, store) = (scratch("modified"), scratch("modified-store"));
    let file = root.join("a.rs");
    fs::write(&file, "one").unwrap();
    let capture = start(&store, &root);
    capture.settle();

    fs::write(&file, "two two").unwrap();
    changed(&capture, &[&file]);
    capture.settle();

    assert_eq!(
        kinds(&capture, "a.rs"),
        [VersionKind::Baseline, VersionKind::Modified]
    );
    assert_eq!(text_of(&capture, "a.rs"), "two two");
    stop(capture);
}

#[test]
fn a_deletion_is_recorded_and_a_file_that_came_back_is_a_change_instead() {
    let (root, store) = (scratch("delete"), scratch("delete-store"));
    let (gone, back) = (root.join("gone.rs"), root.join("back.rs"));
    fs::write(&gone, "1").unwrap();
    fs::write(&back, "1").unwrap();
    let capture = start(&store, &root);
    capture.settle();

    fs::remove_file(&gone).unwrap();
    changed(&capture, &[&gone]);
    capture.settle();
    assert_eq!(
        head(&capture, "gone.rs").unwrap().kind,
        VersionKind::Deleted
    );

    // An editor's save-by-rename, as the recheck sees it: gone when the event was read, and
    // there again with new bytes when it looks a second time.
    fs::rename(&back, root.join("back.rs~")).unwrap();
    fs::write(&back, "2").unwrap();
    changed(&capture, &[&back, &root.join("back.rs~")]);
    capture.settle();
    assert_eq!(
        kinds(&capture, "back.rs"),
        [VersionKind::Baseline, VersionKind::Modified]
    );
    stop(capture);
}

/// `mv src lib` reports two directories and not one file. The heads under the old name would
/// stay alive at dead paths for ever, and the files under the new one would never be seen.
#[test]
fn an_external_directory_rename_reaches_every_file_in_it() {
    let (root, store) = (scratch("mv"), scratch("mv-store"));
    fs::create_dir_all(root.join("src/deep")).unwrap();
    fs::write(root.join("src/a.rs"), "a").unwrap();
    fs::write(root.join("src/deep/b.rs"), "b").unwrap();
    let capture = start(&store, &root);
    capture.settle();

    fs::rename(root.join("src"), root.join("lib")).unwrap();
    changed(&capture, &[&root.join("src"), &root.join("lib")]);
    capture.settle();

    for gone in ["src/a.rs", "src/deep/b.rs"] {
        assert_eq!(
            head(&capture, gone).unwrap().kind,
            VersionKind::Deleted,
            "{gone}"
        );
    }
    for new in ["lib/a.rs", "lib/deep/b.rs"] {
        assert_eq!(
            head(&capture, new).unwrap().kind,
            VersionKind::Created,
            "{new}"
        );
    }
    stop(capture);
}

#[test]
fn a_truncated_burst_is_caught_up_by_a_pass() {
    let (root, store) = (scratch("truncated"), scratch("truncated-store"));
    let file = root.join("a.rs");
    fs::write(&file, "one").unwrap();
    let capture = start(&store, &root);
    capture.settle();

    fs::write(&file, "changed by a burst too large to itemise").unwrap();
    capture.changed(&FsChange {
        paths: vec![root.clone()],
        truncated: true,
        git: false,
    });
    capture.settle();

    assert_eq!(
        kinds(&capture, "a.rs"),
        [VersionKind::Baseline, VersionKind::Modified]
    );
    stop(capture);
}

#[test]
fn what_changed_while_closed_is_found_on_the_next_open() {
    let (root, store) = (scratch("closed"), scratch("closed-store"));
    let (edited, removed) = (root.join("edited.rs"), root.join("removed.rs"));
    fs::write(&edited, "before").unwrap();
    fs::write(&removed, "here").unwrap();
    let capture = start(&store, &root);
    capture.settle();
    stop(capture);

    fs::write(&edited, "after, while closed").unwrap();
    fs::remove_file(&removed).unwrap();
    fs::write(root.join("added.rs"), "new").unwrap();

    let capture = start(&store, &root);
    capture.settle();
    assert_eq!(
        kinds(&capture, "edited.rs"),
        [VersionKind::Baseline, VersionKind::Modified]
    );
    assert_eq!(
        head(&capture, "removed.rs").unwrap().kind,
        VersionKind::Deleted
    );
    assert_eq!(
        kinds(&capture, "added.rs"),
        [VersionKind::Baseline],
        "a walk cannot know this is new, and a restore must not be told that it is"
    );
    let source = capture
        .with_store(|store| store.versions(root_id(store), "edited.rs")[0].source)
        .unwrap();
    assert_eq!(source, VersionSource::Detected);
    stop(capture);
}

#[test]
fn an_unchanged_project_reopens_without_a_single_new_row() {
    let (root, store) = (scratch("idle"), scratch("idle-store"));
    fs::write(root.join("a.rs"), "a").unwrap();
    let capture = start(&store, &root);
    capture.settle();
    let rows = capture.with_store(|store| store.stats().rows).unwrap();
    stop(capture);

    let capture = start(&store, &root);
    capture.settle();
    assert_eq!(
        capture.with_store(|store| store.stats().rows).unwrap(),
        rows
    );
    stop(capture);
}

/// A disk that did not mount looks like a directory with nothing in it. Marking everything
/// deleted would take the eviction protection off the only copies of those files.
#[test]
fn a_root_that_looks_empty_deletes_nothing() {
    let (root, store) = (scratch("unmounted"), scratch("unmounted-store"));
    fs::write(root.join("a.rs"), "a").unwrap();
    fs::write(root.join("b.rs"), "b").unwrap();
    let capture = start(&store, &root);
    capture.settle();
    stop(capture);

    fs::remove_file(root.join("a.rs")).unwrap();
    fs::remove_file(root.join("b.rs")).unwrap();
    let capture = start(&store, &root);
    capture.settle();
    assert_eq!(head(&capture, "a.rs").unwrap().kind, VersionKind::Baseline);
    assert_eq!(head(&capture, "b.rs").unwrap().kind, VersionKind::Baseline);
    stop(capture);
}

#[test]
fn a_root_that_is_gone_deletes_nothing() {
    let (parent, store) = (scratch("gone-root"), scratch("gone-root-store"));
    let root = parent.join("project");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.rs"), "a").unwrap();
    let capture = start(&store, &root);
    capture.settle();
    stop(capture);

    fs::rename(&root, parent.join("away")).unwrap();
    let capture = start(&store, &root);
    capture.settle();
    assert_eq!(head(&capture, "a.rs").unwrap().kind, VersionKind::Baseline);
    stop(capture);
}

/// `settle` times out rather than hanging if this regresses: opening a FIFO for reading blocks
/// until a writer arrives, and nothing else in the project would be recorded after it.
#[test]
fn a_fifo_in_the_tree_does_not_wedge_the_worker() {
    let (root, store) = (scratch("fifo"), scratch("fifo-store"));
    let fifo = root.join("pipe");
    let name = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    fs::write(root.join("after.rs"), "after").unwrap();

    let capture = start(&store, &root);
    capture.settle();
    changed(&capture, &[&fifo]);
    capture.settle();

    assert_eq!(kinds(&capture, "after.rs"), [VersionKind::Baseline]);
    assert!(head(&capture, "pipe").is_none());
    stop(capture);
}

#[test]
fn a_link_is_never_followed_and_what_it_points_at_is_never_touched() {
    let (root, store) = (scratch("link"), scratch("link-store"));
    let outside = scratch("link-outside");
    fs::write(outside.join("secret.rs"), "outside").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.rs"), root.join("file-link")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("dir-link")).unwrap();

    let capture = start(&store, &root);
    capture.settle();
    changed(
        &capture,
        &[&root.join("file-link"), &root.join("dir-link/secret.rs")],
    );
    capture.settle();

    assert!(head(&capture, "file-link").is_none());
    assert!(head(&capture, "dir-link/secret.rs").is_none());
    assert!(head(&capture, "secret.rs").is_none());
    assert_eq!(
        fs::read_to_string(outside.join("secret.rs")).unwrap(),
        "outside"
    );
    stop(capture);
}

/// A project opened through a link and through its target are one set of files, and a spelling
/// must not split them into two histories.
#[test]
fn one_file_reached_two_ways_has_one_history() {
    let (real, store) = (scratch("alias"), scratch("alias-store"));
    let alias_parent = scratch("alias-parent");
    let alias = alias_parent.join("via-link");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    fs::write(real.join("a.rs"), "one").unwrap();

    let capture = start(&store, &alias);
    capture.settle();
    assert_eq!(kinds(&capture, "a.rs"), [VersionKind::Baseline]);

    fs::write(real.join("a.rs"), "two").unwrap();
    changed(&capture, &[&alias.join("a.rs")]);
    capture.settle();
    assert_eq!(
        kinds(&capture, "a.rs"),
        [VersionKind::Baseline, VersionKind::Modified]
    );
    stop(capture);
}

#[test]
fn two_roots_keep_apart_files_with_the_same_name() {
    let (one, two, store) = (
        scratch("root-one"),
        scratch("root-two"),
        scratch("roots-store"),
    );
    fs::write(one.join("same.rs"), "first root").unwrap();
    fs::write(two.join("same.rs"), "second root").unwrap();
    let capture = Capture::start(
        store,
        one.clone(),
        scope_of(&[one, two], &[]),
        limits(),
        no_delays(),
        false,
        Box::new(|| {}),
        None,
    );
    capture.settle();
    capture
        .with_store(|store| {
            let texts: Vec<_> = (0..2)
                .map(|root| {
                    let id = store.head(RootId(root), "same.rs").unwrap().id;
                    String::from_utf8(store.blob(id).unwrap().read().unwrap()).unwrap()
                })
                .collect();
            assert_eq!(texts, ["first root", "second root"]);
        })
        .unwrap();
    stop(capture);
}

#[test]
fn a_save_is_one_version_however_many_times_it_is_reported() {
    let (root, store) = (scratch("save"), scratch("save-store"));
    let file = root.join("a.rs");
    fs::write(&file, "one").unwrap();
    let capture = start(&store, &root);
    capture.settle();

    let before = capture.before_save(&file);
    assert!(before.is_none(), "an ordinary save reads nothing extra");
    fs::write(&file, "two").unwrap();
    capture.saved(file.clone(), before, b"two".to_vec());
    changed(&capture, &[&file]);
    capture.settle();

    assert_eq!(
        kinds(&capture, "a.rs"),
        [VersionKind::Baseline, VersionKind::Modified]
    );
    let source = capture
        .with_store(|store| store.versions(root_id(store), "a.rs")[0].source)
        .unwrap();
    assert_eq!(source, VersionSource::Saved);
    stop(capture);
}

/// The stale Ctrl+S: an agent changed the file, the watcher has not caught up, and the buffer
/// is about to overwrite it unconditionally. This is the last moment that version exists.
#[test]
fn a_save_over_an_unseen_external_edit_keeps_the_external_edit() {
    let (root, store) = (scratch("stale-save"), scratch("stale-save-store"));
    let file = root.join("a.rs");
    fs::write(&file, "original").unwrap();
    let capture = start(&store, &root);
    capture.settle();

    fs::write(&file, "the agent's edit").unwrap();
    let before = capture.before_save(&file);
    assert!(before.is_some());
    fs::write(&file, "stale buffer").unwrap();
    capture.saved(file.clone(), before, b"stale buffer".to_vec());
    capture.settle();

    let texts = capture
        .with_store(|store| {
            store
                .versions(root_id(store), "a.rs")
                .iter()
                .map(|v| String::from_utf8(store.blob(v.id).unwrap().read().unwrap()).unwrap())
                .collect::<Vec<_>>()
        })
        .unwrap();
    assert_eq!(texts, ["stale buffer", "the agent's edit", "original"]);
    stop(capture);
}

/// A write landing just after a save must still be read: a stamp taken at save time could cover
/// it, and the watcher's event for it would then be skipped as unchanged.
#[test]
fn a_write_right_after_a_save_is_not_hidden_by_the_save() {
    let (root, store) = (scratch("after-save"), scratch("after-save-store"));
    let file = root.join("a.rs");
    fs::write(&file, "one").unwrap();
    let capture = start(&store, &root);
    capture.settle();

    fs::write(&file, "saved").unwrap();
    capture.saved(file.clone(), None, b"saved".to_vec());
    fs::write(&file, "right after").unwrap();
    changed(&capture, &[&file]);
    capture.settle();

    assert_eq!(text_of(&capture, "a.rs"), "right after");
    stop(capture);
}

#[test]
fn a_saved_gitignored_file_is_not_copied_into_the_store() {
    let (root, store) = (scratch("ignored-save"), scratch("ignored-save-store"));
    fs::write(root.join(".gitignore"), ".env\n").unwrap();
    let file = root.join(".env");
    fs::write(&file, "TOKEN=1").unwrap();
    let capture = start(&store, &root);
    capture.settle();

    let before = capture.before_save(&file);
    assert!(before.is_none());
    capture.saved(file.clone(), before, b"TOKEN=2".to_vec());
    capture.settle();
    assert!(head(&capture, ".env").is_none());
    stop(capture);
}

#[test]
fn cides_own_rename_carries_the_history_to_the_new_name() {
    let (root, store) = (scratch("rename"), scratch("rename-store"));
    fs::write(root.join("old.rs"), "1").unwrap();
    let capture = start(&store, &root);
    capture.settle();
    fs::write(root.join("old.rs"), "2").unwrap();
    changed(&capture, &[&root.join("old.rs")]);
    capture.settle();

    fs::rename(root.join("old.rs"), root.join("new.rs")).unwrap();
    capture.moved(vec![(root.join("old.rs"), root.join("new.rs"))]);
    changed(&capture, &[&root.join("old.rs"), &root.join("new.rs")]);
    capture.settle();

    assert_eq!(head(&capture, "old.rs").unwrap().kind, VersionKind::Deleted);
    assert_eq!(
        kinds(&capture, "new.rs"),
        [
            VersionKind::Baseline,
            VersionKind::Modified,
            VersionKind::Created
        ]
    );
    stop(capture);
}

#[test]
fn renaming_a_directory_through_cide_moves_every_files_history() {
    let (root, store) = (scratch("rename-dir"), scratch("rename-dir-store"));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.rs"), "1").unwrap();
    let capture = start(&store, &root);
    capture.settle();

    fs::rename(root.join("src"), root.join("lib")).unwrap();
    capture.moved(vec![(root.join("src"), root.join("lib"))]);
    capture.settle();

    assert_eq!(
        kinds(&capture, "lib/a.rs"),
        [VersionKind::Baseline, VersionKind::Created]
    );
    stop(capture);
}

/// Cut and paste reports the destination first. The row must still land, or the file's new name
/// has no road back to its old history.
#[test]
fn a_move_reported_after_its_destination_still_links_the_history() {
    let (root, store) = (scratch("late-move"), scratch("late-move-store"));
    fs::write(root.join("old.rs"), "1").unwrap();
    let capture = start(&store, &root);
    capture.settle();

    fs::rename(root.join("old.rs"), root.join("new.rs")).unwrap();
    changed(&capture, &[&root.join("new.rs")]);
    capture.settle();
    capture.moved(vec![(root.join("old.rs"), root.join("new.rs"))]);
    capture.settle();

    assert!(kinds(&capture, "new.rs").contains(&VersionKind::Baseline));
    stop(capture);
}

#[test]
fn a_file_over_the_limit_is_a_row_with_no_bytes_and_still_counts_as_existing() {
    let (root, store) = (scratch("large"), scratch("large-store"));
    let file = root.join("big.bin");
    fs::write(&file, vec![1; 2048]).unwrap();
    let small = Limits {
        max_file_size: 1024,
        ..limits()
    };
    let capture = start_with(&store, &root, small);
    capture.settle();
    assert!(head(&capture, "big.bin").unwrap().omitted);

    fs::write(&file, vec![2; 2048]).unwrap();
    changed(&capture, &[&file]);
    capture.settle();
    assert_eq!(
        kinds(&capture, "big.bin"),
        [VersionKind::Baseline],
        "one row saying no copy is kept is enough; a log file would otherwise add one per burst"
    );
    stop(capture);
}

/// Past the baseline's share of the cap, new files still get a row with no bytes. Without one a
/// later edit would read as a new file, which a folder restore is entitled to delete.
#[test]
fn a_baseline_that_reaches_the_cap_stops_copying_and_still_finishes() {
    let (root, store) = (scratch("capped"), scratch("capped-store"));
    for i in 0..600_u64 {
        // Xorshift: unique per file and incompressible, so the bytes really reach the pack.
        let mut state = (i + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        let noise: Vec<u8> = (0..4096)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect();
        fs::write(root.join(format!("f{i}.bin")), noise).unwrap();
    }
    let capped = Limits {
        policy: Policy {
            max_storage: 64 << 10,
            ..limits().policy
        },
        ..limits()
    };
    let capture = start_with(&store, &root, capped);
    capture.settle();

    let (rows, kept, done) = capture
        .with_store(|store| {
            let id = root_id(store);
            let heads = store.heads(id);
            let kept = heads.iter().filter(|(_, head)| !head.omitted).count();
            (heads.len(), kept, store.baseline_done(id))
        })
        .unwrap();
    assert_eq!(rows, 600, "every file has a row");
    assert!(kept < 600, "the cap stopped the copying");
    assert!(done, "and the scan still completed");
    stop(capture);
}

#[test]
fn a_transient_save_file_leaves_no_trace() {
    let (root, store) = (scratch("transient"), scratch("transient-store"));
    let capture = start(&store, &root);
    capture.settle();

    let temp = root.join(".a.rs.1234.5.tmp");
    fs::write(&temp, "half a save").unwrap();
    changed(&capture, &[&temp]);
    capture.settle();
    assert!(head(&capture, ".a.rs.1234.5.tmp").is_none());
    stop(capture);
}

#[test]
fn a_second_process_on_a_store_reads_and_records_nothing() {
    let (root, store_dir) = (scratch("second"), scratch("second-store"));
    fs::write(root.join("a.rs"), "a").unwrap();
    let held = Store::open(&store_dir).unwrap();
    assert!(held.is_writable());

    let capture = start(&store_dir, &root);
    capture.settle();
    assert_eq!(capture.with_store(|store| store.stats().rows).unwrap(), 0);
    stop(capture);
}

#[test]
fn the_registry_starts_one_worker_for_two_projects_on_one_store() {
    use cide_ipc::ProjectId;
    let (root, store) = (scratch("shared"), scratch("shared-store"));
    fs::write(root.join("a.rs"), "a").unwrap();
    let registry = Registry::default();
    let (one, two) = (ProjectId::new(), ProjectId::new());
    let attach = |project| {
        registry.ensure(
            project,
            "key".to_owned(),
            store.clone(),
            root.clone(),
            scope_of(std::slice::from_ref(&root), &[]),
            limits(),
            1,
            false,
            false,
            Box::new(|| {}),
        );
    };
    attach(one);
    attach(two);
    let first = registry.capture(one).unwrap();
    let second = registry.capture(two).unwrap();
    assert!(Arc::ptr_eq(&first, &second));

    assert!(
        registry.detach(one).is_none(),
        "the other project still needs it"
    );
    assert!(registry.capture(two).is_some());
    let last = registry.detach(two);
    assert!(last.is_some());
    assert!(registry.capture(two).is_none());
}

/// A quick off-and-on. Without the wait for the old worker, the new one finds the store's lock
/// still held and comes up read-only for the rest of the session, recording nothing in silence.
#[test]
fn turning_a_project_off_and_on_again_does_not_race_its_own_lock() {
    use cide_ipc::ProjectId;
    let (root, store) = (scratch("toggle"), scratch("toggle-store"));
    fs::write(root.join("a.rs"), "a").unwrap();
    let registry = Registry::default();
    let project = ProjectId::new();
    let attach = || {
        registry.ensure(
            project,
            "key".to_owned(),
            store.clone(),
            root.clone(),
            scope_of(std::slice::from_ref(&root), &[]),
            limits(),
            1,
            false,
            false,
            Box::new(|| {}),
        );
    };
    for _ in 0..5 {
        attach();
        registry.detach(project);
    }
    attach();
    let capture = registry.capture(project).unwrap();
    // The production timing rate-limits a pass, so this looks for the store instead of settling.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while capture.with_store(Store::is_writable) != Some(true) {
        assert!(std::time::Instant::now() < deadline, "came up read-only");
        std::thread::sleep(Duration::from_millis(10));
    }
    registry.detach(project);
}

#[test]
fn a_save_outside_every_capture_does_nothing() {
    let registry = Registry::default();
    let save = PendingSave::begin(&registry, Path::new("/nowhere/a.rs"));
    save.finish(b"bytes".to_vec());
}

#[test]
fn nothing_is_written_for_a_project_that_never_captured() {
    let store = scratch("never").join("store");
    assert!(!cide_local_history::exists(&store));
    assert!(!store.exists());
}

fn request(path: &Path, version: u64) -> RestoreRequest {
    RestoreRequest {
        path: path.to_path_buf(),
        version,
        open_roots: Vec::new(),
        caches: Vec::new(),
    }
}

/// The version that held this text.
fn version_of(capture: &Capture, rel: &str, text: &str) -> u64 {
    capture
        .with_store(|store| {
            store
                .versions(root_id(store), rel)
                .iter()
                .find(|v| {
                    store
                        .blob(v.id)
                        .is_some_and(|blob| blob.read().unwrap() == text.as_bytes())
                })
                .map(|v| v.id)
                .expect("a version with that text")
        })
        .unwrap()
}

fn all_texts(capture: &Capture, rel: &str) -> Vec<String> {
    capture
        .with_store(|store| {
            store
                .versions(root_id(store), rel)
                .iter()
                .filter_map(|v| store.blob(v.id))
                .map(|blob| String::from_utf8(blob.read().unwrap()).unwrap())
                .collect()
        })
        .unwrap()
}

#[test]
fn a_restore_makes_the_file_that_version_and_can_itself_be_undone() {
    let (root, store) = (scratch("restore"), scratch("restore-store"));
    let file = root.join("a.rs");
    fs::write(&file, "good").unwrap();
    let capture = start(&store, &root);
    capture.settle();
    fs::write(&file, "broken by an agent").unwrap();
    changed(&capture, &[&file]);
    capture.settle();

    let good = version_of(&capture, "a.rs", "good");
    assert_eq!(
        capture.restore(request(&file, good)),
        Ok(Restored::Rewritten)
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "good");
    assert_eq!(
        all_texts(&capture, "a.rs"),
        ["good", "broken by an agent", "good"]
    );
    let source = capture
        .with_store(|store| store.versions(root_id(store), "a.rs")[0].source)
        .unwrap();
    assert_eq!(source, VersionSource::Restored);

    let broken = version_of(&capture, "a.rs", "broken by an agent");
    capture.restore(request(&file, broken)).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "broken by an agent");
    stop(capture);
}

/// The file changed and no event has arrived yet: the restore's own copy is the only one of it.
#[test]
fn a_restore_saves_a_change_nothing_has_reported_yet() {
    let (root, store) = (scratch("restore-unseen"), scratch("restore-unseen-store"));
    let file = root.join("a.rs");
    fs::write(&file, "v1").unwrap();
    let capture = start(&store, &root);
    capture.settle();

    fs::write(&file, "unreported work").unwrap();
    let v1 = version_of(&capture, "a.rs", "v1");
    capture.restore(request(&file, v1)).unwrap();
    assert!(all_texts(&capture, "a.rs").contains(&"unreported work".to_owned()));
    stop(capture);
}

/// Past the size limit the safety copy is still taken: the limit is about routine capture, and
/// this is the one moment bytes are destroyed on purpose.
#[test]
fn a_restore_saves_the_current_file_even_when_it_is_over_the_limit() {
    let (root, store) = (scratch("restore-big"), scratch("restore-big-store"));
    let file = root.join("a.txt");
    fs::write(&file, "small").unwrap();
    let small = Limits {
        max_file_size: 16,
        ..limits()
    };
    let capture = start_with(&store, &root, small);
    capture.settle();

    let large = "x".repeat(4096);
    fs::write(&file, &large).unwrap();
    let old = version_of(&capture, "a.txt", "small");
    capture.restore(request(&file, old)).unwrap();
    assert!(all_texts(&capture, "a.txt").contains(&large));
    stop(capture);
}

#[test]
fn a_restore_stops_if_the_current_file_cannot_be_saved_first() {
    use std::os::unix::fs::PermissionsExt;
    let (root, store) = (scratch("restore-abort"), scratch("restore-abort-store"));
    let file = root.join("a.rs");
    fs::write(&file, "old").unwrap();
    let capture = start(&store, &root);
    capture.settle();
    fs::write(&file, "unsaved elsewhere").unwrap();
    let old = version_of(&capture, "a.rs", "old");

    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    let outcome = capture.restore(request(&file, old));
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();

    // Skipped as root, where the permission bits above stop nobody.
    if unsafe { libc::geteuid() } != 0 {
        assert!(outcome.is_err(), "{outcome:?}");
        assert_eq!(fs::read_to_string(&file).unwrap(), "unsaved elsewhere");
    }
    stop(capture);
}

#[test]
fn a_deleted_file_comes_back_with_its_mode_and_its_folder() {
    use std::os::unix::fs::PermissionsExt;
    let (root, store) = (scratch("undelete"), scratch("undelete-store"));
    fs::create_dir_all(root.join("bin")).unwrap();
    let file = root.join("bin/run.sh");
    fs::write(&file, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    let capture = start(&store, &root);
    capture.settle();
    let id = version_of(&capture, "bin/run.sh", "#!/bin/sh\n");

    fs::remove_dir_all(root.join("bin")).unwrap();
    changed(&capture, &[&root.join("bin")]);
    capture.settle();
    assert_eq!(
        head(&capture, "bin/run.sh").unwrap().kind,
        VersionKind::Deleted
    );

    assert_eq!(capture.restore(request(&file, id)), Ok(Restored::Recreated));
    assert_eq!(fs::read_to_string(&file).unwrap(), "#!/bin/sh\n");
    let mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);
    assert_eq!(
        head(&capture, "bin/run.sh").unwrap().kind,
        VersionKind::Created
    );
    stop(capture);
}

#[test]
fn a_restore_refuses_a_link_and_a_path_that_leaves_the_project() {
    let (root, store) = (scratch("restore-link"), scratch("restore-link-store"));
    let outside = scratch("restore-link-outside");
    let file = root.join("a.rs");
    fs::write(&file, "one").unwrap();
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("sub/b.rs"), "two").unwrap();
    let capture = start(&store, &root);
    capture.settle();
    let a = version_of(&capture, "a.rs", "one");
    let b = version_of(&capture, "sub/b.rs", "two");

    // The file became a link to something outside.
    fs::remove_file(&file).unwrap();
    fs::write(outside.join("secret"), "outside").unwrap();
    std::os::unix::fs::symlink(outside.join("secret"), &file).unwrap();
    assert!(capture.restore(request(&file, a)).is_err());
    assert_eq!(
        fs::read_to_string(outside.join("secret")).unwrap(),
        "outside"
    );

    // The folder became a link to something outside.
    fs::remove_dir_all(root.join("sub")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("sub")).unwrap();
    assert!(capture.restore(request(&root.join("sub/b.rs"), b)).is_err());
    assert!(!outside.join("b.rs").exists());
    stop(capture);
}

#[test]
fn a_version_with_no_copy_refuses_rather_than_emptying_the_file() {
    let (root, store) = (scratch("no-copy"), scratch("no-copy-store"));
    let file = root.join("big.bin");
    fs::write(&file, vec![1; 2048]).unwrap();
    let small = Limits {
        max_file_size: 1024,
        ..limits()
    };
    let capture = start_with(&store, &root, small);
    capture.settle();
    let id = head(&capture, "big.bin").unwrap().id;
    assert!(capture.restore(request(&file, id)).is_err());
    assert_eq!(fs::read(&file).unwrap(), vec![1; 2048]);
    stop(capture);
}

#[test]
fn a_label_lands_in_the_timeline() {
    let (root, store) = (scratch("put-label"), scratch("put-label-store"));
    fs::write(root.join("a.rs"), "a").unwrap();
    let capture = start(&store, &root);
    capture.settle();
    capture.label("Before refactor".to_owned()).unwrap();
    let groups = capture
        .with_store(|store| store.groups(root_id(store), "", None, 10))
        .unwrap();
    assert_eq!(groups[0].label.as_deref(), Some("Before refactor"));
    stop(capture);
}

fn pause_and_wait(capture: &Capture) {
    capture.set_paused(true);
    capture.settle();
}

#[test]
fn switched_off_nothing_is_recorded_and_what_was_recorded_stays() {
    let (root, store) = (scratch("off"), scratch("off-store"));
    let file = root.join("a.rs");
    fs::write(&file, "one").unwrap();
    let capture = start(&store, &root);
    capture.settle();
    pause_and_wait(&capture);

    fs::write(&file, "two").unwrap();
    changed(&capture, &[&file]);
    assert!(capture.before_save(&file).is_none());
    capture.saved(file.clone(), None, b"two".to_vec());
    capture.moved(vec![(file.clone(), root.join("b.rs"))]);
    capture.settle();

    assert_eq!(kinds(&capture, "a.rs"), [VersionKind::Baseline]);
    assert_eq!(
        text_of(&capture, "a.rs"),
        "one",
        "turning off deletes nothing"
    );
    stop(capture);
}

#[test]
fn switched_off_a_restore_still_saves_the_current_file_first() {
    let (root, store) = (scratch("off-restore"), scratch("off-restore-store"));
    let file = root.join("a.rs");
    fs::write(&file, "old").unwrap();
    let capture = start(&store, &root);
    capture.settle();
    let old = version_of(&capture, "a.rs", "old");
    pause_and_wait(&capture);

    fs::write(&file, "work done while off").unwrap();
    capture.restore(request(&file, old)).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "old");
    assert!(all_texts(&capture, "a.rs").contains(&"work done while off".to_owned()));
    stop(capture);
}

#[test]
fn switching_back_on_catches_up_with_the_disk() {
    let (root, store) = (scratch("on-again"), scratch("on-again-store"));
    let file = root.join("a.rs");
    fs::write(&file, "before").unwrap();
    let capture = start(&store, &root);
    capture.settle();
    pause_and_wait(&capture);

    fs::write(&file, "changed while off").unwrap();
    fs::write(root.join("new.rs"), "added while off").unwrap();
    capture.set_paused(false);
    capture.settle();

    assert_eq!(text_of(&capture, "a.rs"), "changed while off");
    assert_eq!(
        kinds(&capture, "new.rs"),
        [VersionKind::Baseline],
        "a pass cannot know when it appeared, so a restore must not treat it as new"
    );
    stop(capture);
}

/// Started paused: a project with history whose switch is off. Nothing runs, nothing is scanned.
#[test]
fn a_store_opened_with_capture_off_is_readable_and_never_scanned() {
    let (root, store) = (scratch("start-off"), scratch("start-off-store"));
    fs::write(root.join("a.rs"), "a").unwrap();
    let capture = start(&store, &root);
    capture.settle();
    stop(capture);

    fs::write(root.join("a.rs"), "changed while closed").unwrap();
    let capture = Capture::start(
        store.clone(),
        root.clone(),
        scope_of(std::slice::from_ref(&root), &[]),
        limits(),
        no_delays(),
        true,
        Box::new(|| {}),
        None,
    );
    capture.settle();
    assert_eq!(text_of(&capture, "a.rs"), "a");
    stop(capture);
}

fn tree(capture: &Capture, prefix: &str, at: u64) -> TreeRequest {
    TreeRequest {
        root: capture.with_store(root_id).unwrap(),
        prefix: prefix.to_owned(),
        at,
        label: "Before restoring".to_owned(),
        open_roots: Vec::new(),
        caches: Vec::new(),
        trash: scratch_trash,
    }
}

/// Never the user's real Trash, for files a test made.
fn scratch_trash(path: &Path) -> cide_fs::Result<PathBuf> {
    let home = std::env::temp_dir().join(format!("cide-lh-trash-{}", std::process::id()));
    cide_fs::trash::move_to_trash_in(&home, path)
}

/// The trash is the user's real one, so these tests only ever ask it to take scratch files.
#[test]
fn a_folder_restore_brings_back_old_content_and_deleted_files() {
    let (root, store) = (scratch("tree"), scratch("tree-store"));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.rs"), "good a").unwrap();
    fs::write(root.join("src/b.rs"), "good b").unwrap();
    let capture = start(&store, &root);
    capture.settle();
    let at = head(&capture, "src/b.rs")
        .unwrap()
        .id
        .max(head(&capture, "src/a.rs").unwrap().id);

    fs::write(root.join("src/a.rs"), "broken").unwrap();
    fs::remove_file(root.join("src/b.rs")).unwrap();
    changed(&capture, &[&root.join("src/a.rs"), &root.join("src/b.rs")]);
    capture.settle();

    let report = capture.restore_tree(tree(&capture, "src", at)).unwrap();
    assert_eq!(report.written.len(), 2, "{report:?}");
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(fs::read_to_string(root.join("src/a.rs")).unwrap(), "good a");
    assert_eq!(fs::read_to_string(root.join("src/b.rs")).unwrap(), "good b");
    assert!(
        all_texts(&capture, "src/a.rs").contains(&"broken".to_owned()),
        "what was replaced is still recoverable"
    );
    stop(capture);
}

/// **The test this whole design answers to.** A file that was simply there when the first scan
/// ran has no row older than that scan, and that must never read as "did not exist then".
#[test]
fn a_folder_restore_never_touches_a_file_that_predates_the_first_scan() {
    let (root, store) = (scratch("tree-old"), scratch("tree-old-store"));
    fs::write(root.join("a.rs"), "one").unwrap();
    let capture = start(&store, &root);
    capture.settle();
    let at = head(&capture, "a.rs").unwrap().id;
    stop(capture);

    // Found by a later pass, so all the store can say is that it exists now.
    fs::write(root.join("found-later.rs"), "precious").unwrap();
    let capture = start(&store, &root);
    capture.settle();
    assert_eq!(kinds(&capture, "found-later.rs"), [VersionKind::Baseline]);

    let report = capture.restore_tree(tree(&capture, "", at)).unwrap();
    assert!(report.removed.is_empty(), "{report:?}");
    assert_eq!(report.left, 1);
    assert_eq!(
        fs::read_to_string(root.join("found-later.rs")).unwrap(),
        "precious"
    );
    stop(capture);
}

#[test]
fn a_folder_restore_refuses_to_start_without_its_undo_point() {
    let (root, store_dir) = (scratch("tree-readonly"), scratch("tree-readonly-store"));
    fs::write(root.join("a.rs"), "one").unwrap();
    let held = Store::open(&store_dir).unwrap();
    let capture = start(&store_dir, &root);
    capture.settle();
    let request = TreeRequest {
        root: RootId(0),
        prefix: String::new(),
        at: 1,
        label: "x".to_owned(),
        open_roots: Vec::new(),
        caches: Vec::new(),
        trash: scratch_trash,
    };
    assert!(capture.restore_tree(request).is_err());
    assert_eq!(fs::read_to_string(root.join("a.rs")).unwrap(), "one");
    drop(held);
    stop(capture);
}

#[test]
fn clearing_forgets_the_versions_and_capture_starts_over() {
    let (root, store) = (scratch("clear"), scratch("clear-store"));
    fs::write(root.join("a.rs"), "one").unwrap();
    let capture = start(&store, &root);
    capture.settle();
    fs::write(root.join("a.rs"), "two").unwrap();
    changed(&capture, &[&root.join("a.rs")]);
    capture.settle();

    capture.clear().unwrap();
    capture.settle();
    assert_eq!(
        kinds(&capture, "a.rs"),
        [VersionKind::Baseline],
        "only the file as it is now, from a fresh first scan"
    );
    assert_eq!(fs::read_to_string(root.join("a.rs")).unwrap(), "two");
    stop(capture);
}

#[test]
fn clearing_with_capture_off_stays_empty() {
    let (root, store) = (scratch("clear-off"), scratch("clear-off-store"));
    fs::write(root.join("a.rs"), "one").unwrap();
    let capture = start(&store, &root);
    capture.settle();
    pause_and_wait(&capture);
    capture.clear().unwrap();
    capture.settle();
    assert_eq!(capture.with_store(|s| s.stats().rows).unwrap(), 0);
    stop(capture);
}

/// The one path in the feature that removes a user's file, so it gets every promise spelled out:
/// only what was watched appearing, to the trash and not unlinked, with a copy kept, and undoable.
#[test]
fn a_folder_restore_trashes_only_what_it_saw_appear_and_keeps_a_copy() {
    let (root, store) = (scratch("tree-remove"), scratch("tree-remove-store"));
    fs::write(root.join("a.rs"), "a").unwrap();
    let capture = start(&store, &root);
    capture.settle();
    let at = head(&capture, "a.rs").unwrap().id;

    let added = root.join("added.rs");
    fs::write(&added, "made after that point").unwrap();
    changed(&capture, &[&added]);
    capture.settle();
    assert_eq!(kinds(&capture, "added.rs"), [VersionKind::Created]);

    let report = capture.restore_tree(tree(&capture, "", at)).unwrap();
    assert_eq!(report.removed, std::slice::from_ref(&added), "{report:?}");
    assert!(!added.exists());
    assert_eq!(fs::read_to_string(root.join("a.rs")).unwrap(), "a");

    let history = capture
        .with_store(|store| store.versions(root_id(store), "added.rs"))
        .unwrap();
    assert_eq!(history[0].kind, VersionKind::Deleted);
    assert_eq!(history[0].source, VersionSource::Restored);

    // And back again, from the version the restore kept.
    let id = version_of(&capture, "added.rs", "made after that point");
    assert_eq!(
        capture.restore(request(&added, id)),
        Ok(Restored::Recreated)
    );
    assert_eq!(fs::read_to_string(&added).unwrap(), "made after that point");
    stop(capture);
}

/// An edit the watcher has not reported yet is still the user's work. It is saved before the file
/// goes, so being wrong about a file costs a restore and not its content.
#[test]
fn a_folder_restore_saves_an_unreported_edit_before_removing_the_file() {
    let (root, store) = (scratch("tree-unseen"), scratch("tree-unseen-store"));
    fs::write(root.join("a.rs"), "a").unwrap();
    let capture = start(&store, &root);
    capture.settle();
    let at = head(&capture, "a.rs").unwrap().id;
    let added = root.join("added.rs");
    fs::write(&added, "first").unwrap();
    changed(&capture, &[&added]);
    capture.settle();

    fs::write(&added, "edited, not reported yet").unwrap();
    capture.restore_tree(tree(&capture, "", at)).unwrap();
    assert!(all_texts(&capture, "added.rs").contains(&"edited, not reported yet".to_owned()));
    stop(capture);
}

/// The real seam: a real walk, a real `ProjectFs`, real inotify and the real `FsEvents` calls, with
/// only the window missing. What the tests above stub is exactly what this one is for, which is
/// that the watcher's `Status` really does start capture, and that what it reports really does
/// arrive as versions. It is what `impl FsEvents for AppHandle` does, minus the settings lookup.
struct LiveEvents {
    store: PathBuf,
    capture: parking_lot::Mutex<Option<Arc<Capture>>>,
    watching_calls: AtomicU32,
}

impl crate::files::FsEvents for LiveEvents {
    fn status(&self, _: cide_ipc::ProjectId, _: &cide_ipc::FsStatus) {}

    fn changed(&self, _: cide_ipc::ProjectId, change: &FsChange) {
        if let Some(capture) = &*self.capture.lock() {
            capture.changed(change);
        }
    }

    fn watching(&self, _: cide_ipc::ProjectId, fs: &Arc<crate::files::ProjectFs>) {
        self.watching_calls.fetch_add(1, Ordering::Relaxed);
        let roots = fs.root_paths();
        let scope = Scope::new(roots.clone(), fs.filter(), &[], Arc::clone(&fs.budget)).unwrap();
        let mut slot = self.capture.lock();
        match &*slot {
            Some(capture) => capture.rescope(scope, true),
            None => {
                *slot = Some(Capture::start(
                    self.store.clone(),
                    roots[0].clone(),
                    scope,
                    limits(),
                    no_delays(),
                    false,
                    Box::new(|| {}),
                    None,
                ));
            }
        }
    }
}

/// Wait for something the real watcher will do in its own time: 300 ms debounce, then up to 2 s
/// of coalescing. A timeout rather than a sleep, so a pass costs what it costs.
fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(
            std::time::Instant::now() < deadline,
            "never happened: {what}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_real_watcher_starts_capture_and_its_events_become_versions() {
    use crate::files::{FsEvents, FsRegistry};
    let (root, store) = (scratch("live"), scratch("live-store"));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("target")).unwrap();
    fs::write(root.join(".gitignore"), "target/\n").unwrap();
    fs::write(root.join("src/a.rs"), "one").unwrap();

    let live = Arc::new(LiveEvents {
        store: store.clone(),
        capture: parking_lot::Mutex::new(None),
        watching_calls: AtomicU32::new(0),
    });
    let events: Arc<dyn FsEvents> = live.clone();
    let registry = FsRegistry::default();
    let project = cide_ipc::ProjectId::new();
    let shown = Visibility {
        hidden: true,
        ignored: true,
    };
    registry
        .claim(project, vec![root.clone()], shown)
        .unwrap()
        .run(events, project);

    eventually("the watcher's first status", || {
        live.watching_calls.load(Ordering::Relaxed) > 0
    });
    let capture = live.capture.lock().clone().unwrap();
    eventually("the first scan", || {
        capture
            .with_store(|store| !store.roots().is_empty() && store.baseline_done(RootId(0)))
            .unwrap_or(false)
    });
    assert_eq!(kinds(&capture, "src/a.rs"), [VersionKind::Baseline]);

    fs::write(root.join("src/a.rs"), "two").unwrap();
    eventually("an edit from outside cide", || {
        kinds(&capture, "src/a.rs") == [VersionKind::Baseline, VersionKind::Modified]
    });
    assert_eq!(text_of(&capture, "src/a.rs"), "two");

    fs::write(root.join("src/new.rs"), "new").unwrap();
    eventually("a new file", || {
        kinds(&capture, "src/new.rs") == [VersionKind::Created]
    });

    fs::remove_file(root.join("src/new.rs")).unwrap();
    eventually("a deletion", || {
        head(&capture, "src/new.rs").is_some_and(|head| head.kind == VersionKind::Deleted)
    });

    // A folder moved from outside: two directory events and no file event.
    fs::rename(root.join("src"), root.join("lib")).unwrap();
    eventually("a renamed folder", || {
        head(&capture, "lib/a.rs").is_some()
            && head(&capture, "src/a.rs").is_some_and(|head| head.kind == VersionKind::Deleted)
    });

    fs::write(root.join("target/x.o"), "object").unwrap();
    fs::write(root.join("sentinel.rs"), "after the build output").unwrap();
    eventually("the sentinel", || head(&capture, "sentinel.rs").is_some());
    assert!(
        head(&capture, "target/x.o").is_none(),
        "with show ignored files on, as shipped, build output is still not kept"
    );
    stop(capture);
    drop(registry);
}
