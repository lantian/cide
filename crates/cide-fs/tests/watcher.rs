//! The watcher against a real filesystem.
//!
//! These are the properties the milestone is graded on — one notification for a burst, and
//! nothing at all for an ignored directory — and neither can be shown with a fake clock.

use std::sync::mpsc;
use std::time::Duration;

use cide_fs::testing::scratch;
use cide_fs::{
    BuildOptions, Filter, Index, Root, Visibility, WalkItem, WatchConfig, WatchEvent, Watcher,
};

/// Long enough that a loaded machine does not split a burst, short enough that a failing
/// test finishes.
const QUIET: Duration = Duration::from_millis(400);

fn start(dir: &std::path::Path, max_paths: usize) -> (Watcher, mpsc::Receiver<WatchEvent>) {
    let (watcher, rx, status) = start_with(dir, |config| {
        config.max_paths = max_paths;
    });
    assert_eq!(status.backend, cide_ipc::WatchBackend::Native);
    assert!(status.watched_dirs >= 1, "{status:?}");
    (watcher, rx)
}

fn start_with(
    dir: &std::path::Path,
    tweak: impl FnOnce(&mut WatchConfig),
) -> (Watcher, mpsc::Receiver<WatchEvent>, cide_ipc::WatchStatus) {
    let index = Index::build(
        vec![Root::new(dir)],
        BuildOptions::default(),
        &|_: &[WalkItem]| {},
    );
    let dirs = index.dir_paths();
    let filter = std::sync::Arc::new(Filter::build(
        &[dir.to_path_buf()],
        dirs.iter().map(|p| p.as_path()),
        Visibility::CONSERVATIVE,
    ));
    let mut config = WatchConfig {
        roots: vec![dir.to_path_buf()],
        dirs,
        debounce: Duration::from_millis(100),
        quiet: QUIET,
        max_wait: Duration::from_secs(30),
        ..WatchConfig::default()
    };
    tweak(&mut config);

    let (tx, rx) = mpsc::channel();
    let watcher = Watcher::start(config, filter, move |event| {
        let _ = tx.send(event);
    });
    // The first event is always the backend the watcher settled on.
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(WatchEvent::Status(status)) => (watcher, rx, status),
        other => panic!("expected a status first, got {other:?}"),
    }
}

fn next_change(rx: &mpsc::Receiver<WatchEvent>, within: Duration) -> Option<cide_ipc::FsChange> {
    let deadline = std::time::Instant::now() + within;
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(WatchEvent::Changed(change)) => return Some(change),
            Ok(WatchEvent::Status(_)) => continue,
            Err(_) => return None,
        }
    }
    None
}

#[test]
fn five_thousand_touched_files_produce_one_notification() {
    let dir = scratch("watch-burst");
    std::fs::write(dir.join("seed.txt"), "").unwrap();
    let (_watcher, rx) = start(dir.path(), 8192);

    for i in 0..5_000 {
        std::fs::write(dir.join(format!("f{i}.txt")), "x").unwrap();
    }

    let change = next_change(&rx, Duration::from_secs(20)).expect("the burst should arrive");
    assert!(
        change.paths.len() >= 4_900,
        "expected the whole burst in one change, got {}",
        change.paths.len()
    );
    assert!(!change.git);
    assert!(
        next_change(&rx, QUIET * 3).is_none(),
        "a second notification means the burst was not coalesced"
    );
}

#[test]
fn writes_under_an_ignored_directory_are_never_reported() {
    let dir = scratch("watch-ignored");
    std::fs::create_dir_all(dir.join("target/debug")).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join(".gitignore"), "target/\n").unwrap();
    std::fs::write(dir.join("src/main.rs"), "").unwrap();

    let (_watcher, rx) = start(dir.path(), 8192);
    for i in 0..200 {
        std::fs::write(dir.join(format!("target/debug/o{i}.o")), "x").unwrap();
    }

    assert!(
        next_change(&rx, QUIET * 4).is_none(),
        "a build storm under target/ reached the UI"
    );

    // And a real edit still does.
    std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
    let change = next_change(&rx, Duration::from_secs(10)).expect("a real edit should arrive");
    assert_eq!(change.paths, vec![dir.join("src/main.rs")]);
}

/// The same storm, with the tree *showing* `target/`. Still silent. (M18)
///
/// This is the property the whole *Show ignored files* design rests on, and it is the one that
/// cannot be established by reading: the walk, the watch list and the event loop each have to
/// treat "shown" and "watched" as different questions, and getting any one of them wrong turns a
/// `cargo build` into a repaint every two seconds. So the tree is built with the setting on, the
/// rows are asserted to be there, and then 200 object files are written into them.
#[test]
fn a_shown_target_directory_is_still_never_watched() {
    let dir = scratch("watch-ignored-shown");
    std::fs::create_dir_all(dir.join("target/debug")).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join(".gitignore"), "target/\n").unwrap();
    std::fs::write(dir.join("src/main.rs"), "").unwrap();

    let shown = Visibility {
        hidden: true,
        ignored: true,
    };
    let index = Index::build(
        vec![Root::new(dir.path())],
        BuildOptions {
            visibility: shown,
            ..BuildOptions::default()
        },
        &|_: &[WalkItem]| {},
    );
    let filter = std::sync::Arc::new(Filter::build(
        &[dir.to_path_buf()],
        index.dir_paths().iter().map(|p| p.as_path()),
        shown,
    ));
    let rows: Vec<String> = index.rows(0, 64).into_iter().map(|r| r.name).collect();
    assert!(
        rows.contains(&"target".to_string()),
        "the setting is meant to be on for this test: {rows:?}"
    );

    let (tx, rx) = mpsc::channel();
    let config = WatchConfig {
        roots: vec![dir.to_path_buf()],
        // Exactly what `Indexing::run` hands the watcher.
        dirs: index.watch_dirs(&filter),
        debounce: Duration::from_millis(100),
        quiet: QUIET,
        max_wait: Duration::from_secs(30),
        ..WatchConfig::default()
    };
    let watcher = Watcher::start(config, filter, move |event| {
        let _ = tx.send(event);
    });

    for i in 0..200 {
        std::fs::write(dir.join(format!("target/debug/o{i}.o")), "x").unwrap();
    }
    assert!(
        next_change(&rx, QUIET * 4).is_none(),
        "a build storm under a *shown* target/ reached the UI — the rows are the walk's \
         snapshot on purpose, and watching them costs a descriptor per directory plus one \
         event per object file"
    );

    std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
    let change = next_change(&rx, Duration::from_secs(10)).expect("a real edit should arrive");
    assert_eq!(change.paths, vec![dir.join("src/main.rs")]);
    drop(watcher);
}

#[test]
fn a_git_head_change_is_flagged_as_git() {
    let dir = scratch("watch-git");
    std::fs::create_dir_all(dir.join(".git/refs/heads")).unwrap();
    std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(dir.join("a.rs"), "").unwrap();

    let (_watcher, rx) = start(dir.path(), 8192);
    std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/other\n").unwrap();

    let change = next_change(&rx, Duration::from_secs(10)).expect("HEAD should be watched");
    assert!(change.git, "{change:?}");
    assert!(
        change.paths.iter().any(|p| p.ends_with("HEAD")),
        "{change:?}"
    );
}

/// The degraded path, which is what an exhausted `fs.inotify.max_user_watches` falls into.
///
/// Forcing it rather than exhausting the kernel's watch limit — a test that did that would
/// affect every other process on the machine. The code path is the same one `MaxFilesWatch`
/// takes: `Backend::start_polling`, a `PollWatcher`, and a reason carried to the UI.
#[test]
fn polling_mode_still_reports_changes_and_says_why() {
    let dir = scratch("watch-polling");
    std::fs::write(dir.join("a.rs"), "").unwrap();

    let (_watcher, rx, status) = start_with(dir.path(), |config| {
        config.polling = true;
        config.poll_interval = Duration::from_millis(150);
    });
    assert_eq!(status.backend, cide_ipc::WatchBackend::Polling);
    let reason = status.reason.expect("a degraded backend must say why");
    assert!(
        !reason.is_empty(),
        "the banner needs a sentence, not a flag"
    );
    assert_eq!(
        status.watched_dirs, 0,
        "polling watches roots, not directories"
    );

    std::fs::write(dir.join("b.rs"), "new").unwrap();
    let change = next_change(&rx, Duration::from_secs(10)).expect("polling should notice");
    assert!(
        change.paths.iter().any(|p| p.ends_with("b.rs")),
        "{change:?}"
    );
}

#[test]
fn a_new_file_reaches_the_index_and_a_deleted_one_leaves_it() {
    let dir = scratch("watch-apply");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/a.rs"), "").unwrap();

    let mut index = Index::build(
        vec![Root::new(dir.path())],
        BuildOptions::default(),
        &|_: &[WalkItem]| {},
    );
    let filter = Filter::build(
        &[dir.to_path_buf()],
        index.dir_paths().iter().map(|p| p.as_path()),
        Visibility::CONSERVATIVE,
    );
    index.expand(&dir.join("src")).unwrap();
    assert_eq!(index.count(), 2);

    std::fs::write(dir.join("src/b.rs"), "").unwrap();
    let added = index.apply(
        &cide_ipc::FsChange {
            paths: vec![dir.join("src/b.rs")],
            truncated: false,
            git: false,
        },
        &filter,
    );
    assert_eq!(added.len(), 1);
    assert_eq!(added[0].rel, "src/b.rs");
    assert_eq!(index.count(), 3);
    assert_eq!(index.rows(2, 1)[0].name, "b.rs");

    std::fs::remove_file(dir.join("src/a.rs")).unwrap();
    index.apply(
        &cide_ipc::FsChange {
            paths: vec![dir.join("src/a.rs")],
            truncated: false,
            git: false,
        },
        &filter,
    );
    assert_eq!(index.count(), 2);
    assert!(!index.contains(&dir.join("src/a.rs")));
    assert!(index.contains(&dir.join("src/b.rs")));
}

/// The burst the file tree used to repaint itself for, and what it is worth.
///
/// `.git/HEAD`, `.git/index` and the refs are watched on purpose — the git status column has
/// no other way to hear about a `git add` in a terminal pane — so every git command in the
/// project produces a change event. What that event can never do is move a row: the walk put
/// nothing under `.git` in the tree, and `Index::apply` skips those paths by name.
///
/// This is the half of the flicker diagnosis that lives in Rust. The frontend used to answer
/// this event by dropping its whole row cache, so a repository being committed to repainted
/// the explorer from empty on every gesture; `ui/src/sidebar/treeStore.ts` now revalidates
/// instead and writes nothing when nothing moved. Asserted here rather than only there,
/// because "this event cannot change a row" is a property of the index and not of the panel.
#[test]
fn a_git_index_write_is_reported_but_moves_no_row() {
    let dir = scratch("watch-git-noop");
    std::fs::create_dir_all(dir.join(".git/refs/heads")).unwrap();
    std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(dir.join(".git/index"), "before").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/a.rs"), "").unwrap();

    let mut index = Index::build(
        vec![Root::new(dir.path())],
        BuildOptions::default(),
        &|_: &[WalkItem]| {},
    );
    let dirs = index.dir_paths();
    let filter = Filter::build(
        &[dir.to_path_buf()],
        dirs.iter().map(|p| p.as_path()),
        Visibility::CONSERVATIVE,
    );
    index.expand(&dir.join("src")).unwrap();
    let before = index.rows(0, 100);
    assert_eq!(before.len(), 2, "src/ and src/a.rs; .git is not a row");

    let (_watcher, rx) = start(dir.path(), 8192);
    std::fs::write(dir.join(".git/index"), "after").unwrap();

    // The UI hears about it — that is the whole reason the path is watched.
    let change = next_change(&rx, Duration::from_secs(10)).expect("the index write is reported");
    assert!(change.git, "{change:?}");
    assert!(
        change.paths.iter().all(|p| filter.is_git_path(p)),
        "the burst is git metadata and nothing else: {change:?}"
    );

    // And the tree does not move by so much as a row.
    let added = index.apply(&change, &filter);
    assert!(added.is_empty(), "{added:?}");
    assert_eq!(index.count(), before.len());
    assert_eq!(index.rows(0, 100), before);
}

#[test]
fn a_directory_created_after_the_walk_is_watched_and_indexed() {
    let dir = scratch("watch-newdir");
    std::fs::write(dir.join("seed.txt"), "").unwrap();
    let (_watcher, rx) = start(dir.path(), 8192);

    std::fs::create_dir_all(dir.join("fresh/inner")).unwrap();
    let first = next_change(&rx, Duration::from_secs(10)).expect("the new directory");
    assert!(
        first.paths.iter().any(|p| p.ends_with("fresh")),
        "{first:?}"
    );

    // The watch on `fresh/inner` is the point: it did not exist when the walk ran.
    std::fs::write(dir.join("fresh/inner/deep.rs"), "").unwrap();
    let second = next_change(&rx, Duration::from_secs(10)).expect("a write inside a new directory");
    assert!(
        second.paths.iter().any(|p| p.ends_with("deep.rs")),
        "{second:?}"
    );
}
