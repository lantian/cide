//! Directory aliases must agree across the tree, streaming picker and physical watches.
#![cfg(unix)]

use std::collections::HashSet;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use cide_fs::testing::scratch;
use cide_fs::{
    BuildOptions, Filter, Index, Root, Visibility, WalkItem, WatchConfig, WatchEvent, Watcher,
};
use cide_ipc::{FsChange, TreeRowKind};

fn build(root: &Path) -> Index {
    Index::build(vec![Root::new(root)], BuildOptions::default(), &|_| {})
}

#[test]
fn library_candidate_walk_does_not_change_the_synthetic_trees_link_policy() {
    let library = scratch("symlink-library");
    let target = scratch("symlink-library-target");
    std::fs::write(target.join("external.rs"), "external").unwrap();
    symlink(target.path(), library.join("linked")).unwrap();
    let streamed = Mutex::new(Vec::<WalkItem>::new());
    Index::walk_roots(
        &[Root::new(library.path())],
        BuildOptions::default(),
        true,
        &|batch| {
            streamed.lock().unwrap().extend_from_slice(batch);
        },
    );
    let items = streamed.into_inner().unwrap();
    assert!(
        items
            .iter()
            .any(|item| item.path == library.join("linked") && item.symlink && !item.is_dir)
    );
    assert!(
        !items
            .iter()
            .any(|item| item.path == library.join("linked/external.rs"))
    );
}

fn filter(index: &Index) -> Filter {
    Filter::build(
        &index
            .roots()
            .iter()
            .map(|r| r.path.clone())
            .collect::<Vec<_>>(),
        index.dir_paths().iter().map(PathBuf::as_path),
        Visibility::CONSERVATIVE,
    )
}

fn change(path: PathBuf) -> FsChange {
    FsChange {
        paths: vec![path],
        truncated: false,
        git: false,
    }
}

#[test]
fn links_are_directories_and_the_picker_keeps_every_logical_path() {
    let root = scratch("symlink-tree");
    let external = scratch("symlink-external");
    std::fs::create_dir(root.join("real")).unwrap();
    std::fs::write(root.join("real/inside.rs"), "inside").unwrap();
    std::fs::write(external.join("outside.rs"), "outside").unwrap();
    std::fs::write(external.join(".gitignore"), "ignored.rs\n").unwrap();
    std::fs::write(external.join("ignored.rs"), "ignored").unwrap();
    std::fs::create_dir(external.join(".git")).unwrap();
    std::fs::write(external.join(".git/config"), "git").unwrap();
    symlink("real", root.join("relative")).unwrap();
    symlink(external.path(), root.join("external")).unwrap();
    symlink(external.path(), root.join("second")).unwrap();
    symlink(".", root.join("cycle")).unwrap();
    symlink("missing", root.join("broken")).unwrap();
    std::fs::write(root.join(".gitignore"), "ignored-link/\nignored-broken\n").unwrap();
    symlink(".", root.join("ignored-link")).unwrap();
    symlink("missing", root.join("ignored-broken")).unwrap();
    symlink(".", root.join(".hidden-link")).unwrap();
    symlink("real/inside.rs", root.join("file.rs")).unwrap();
    symlink(external.path(), root.join("real/nested")).unwrap();
    let streamed = Mutex::new(Vec::<WalkItem>::new());
    let mut index = Index::build(
        vec![Root::new(root.path())],
        BuildOptions::default(),
        &|batch| {
            streamed.lock().unwrap().extend_from_slice(batch);
        },
    );
    for name in ["relative", "external", "second", "cycle", "real/nested"] {
        assert_eq!(
            index.kind_of(&root.join(name)),
            Some(TreeRowKind::Dir),
            "{name}"
        );
    }
    index.set_all_expanded(true, |_| true);
    let rows = index.rows(0, index.count());
    let cycle = rows.iter().find(|r| r.name == "cycle").unwrap();
    assert!(cycle.symlink);
    assert!(!cycle.has_children);
    assert!(
        rows.iter()
            .any(|r| r.name == "broken" && r.symlink && !r.has_children)
    );
    assert!(
        !index
            .watch_dirs(&filter(&index))
            .contains(&root.join("cycle"))
    );
    for path in [
        "real/inside.rs",
        "relative/inside.rs",
        "external/outside.rs",
        "second/outside.rs",
        "file.rs",
        "real/nested/outside.rs",
    ] {
        assert!(
            streamed
                .lock()
                .unwrap()
                .iter()
                .any(|item| item.path == root.join(path) && !item.is_dir),
            "{path}"
        );
        assert!(index.reveal(&root.join(path)).is_some(), "reveal {path}");
    }
    assert!(!index.contains(&root.join("external/ignored.rs")));
    assert!(!index.contains(&root.join("external/.git/config")));
    for name in ["ignored-link", "ignored-broken", ".hidden-link"] {
        assert!(
            !index.contains(&root.join(name)),
            "error recovery must respect visibility: {name}"
        );
    }
    index.refresh_subtree(root.path(), &filter(&index));
    assert!(
        !index.contains(&root.join("cycle/real")),
        "refresh must not unfold a cycle"
    );
}

#[test]
fn rescans_reconcile_links_retargeting_kind_changes_and_cycles() {
    let root = scratch("symlink-rescan");
    let first = scratch("symlink-first");
    let second = scratch("symlink-second");
    std::fs::write(first.join("old.rs"), "old").unwrap();
    std::fs::write(first.join(".gitignore"), "new.rs\n").unwrap();
    std::fs::write(second.join("new.rs"), "new").unwrap();
    std::fs::write(second.join(".gitignore"), "old.rs\nignored.rs\n").unwrap();
    std::fs::write(second.join("old.rs"), "ignored in the new target").unwrap();
    let mut index = build(root.path());
    let filter = filter(&index);
    let link = root.join("alias");
    symlink(first.path(), &link).unwrap();
    let added = index.apply(&change(link.clone()), &filter);
    assert!(added.iter().any(|i| i.path == link.join("old.rs")));
    index.expand(&link);
    assert!(
        index.apply(&change(link.clone()), &filter).is_empty(),
        "an unchanged branch must not duplicate picker entries"
    );
    std::fs::remove_file(&link).unwrap();
    symlink(second.path(), &link).unwrap();
    index.apply(&change(root.path().to_path_buf()), &filter);
    assert!(!index.contains(&link.join("old.rs")));
    assert!(index.contains(&link.join("new.rs")));
    std::fs::write(second.join("ignored.rs"), "still ignored after a rescan").unwrap();
    index.apply(&change(link.join("ignored.rs")), &filter);
    assert!(!index.contains(&link.join("ignored.rs")));
    assert!(!filter.watchable(&link.join("ignored.rs"), false));
    std::fs::remove_file(&link).unwrap();
    symlink(root.path(), &link).unwrap();
    index.apply(&change(link.clone()), &filter);
    index.refresh_subtree(root.path(), &filter);
    assert_eq!(index.kind_of(&link), Some(TreeRowKind::Dir));
    assert!(!index.contains(&link.join("alias")));
    std::fs::remove_file(&link).unwrap();
    std::fs::write(&link, "now a file").unwrap();
    index.apply(&change(link.clone()), &filter);
    assert_eq!(index.kind_of(&link), Some(TreeRowKind::File));
    assert!(
        !index
            .rows(0, index.count())
            .iter()
            .find(|r| r.path == link)
            .unwrap()
            .symlink
    );
}

#[test]
fn a_linked_project_root_and_a_second_root_keep_their_own_rows_and_watches() {
    let fixture = scratch("symlink-project-root");
    let real = fixture.join("real");
    let alias = fixture.join("project");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(real.join("file.rs"), "initial").unwrap();
    symlink(&real, &alias).unwrap();
    let mut index = Index::build(
        vec![Root::new(&alias), Root::new(&real)],
        BuildOptions::default(),
        &|_| {},
    );
    let rows = index.rows(0, index.count());
    assert!(
        rows.iter()
            .any(|row| row.path == alias && row.symlink && row.root == 0)
    );
    assert!(
        rows.iter()
            .any(|row| row.path == real && !row.symlink && row.root == 1)
    );
    assert!(index.reveal(&alias.join("file.rs")).is_some());
    let filter = Arc::new(filter(&index));
    let config = WatchConfig {
        roots: vec![alias.clone(), real.clone()],
        dirs: index.watch_dirs(&filter),
        links: index.watch_links(&filter),
        debounce: Duration::from_millis(30),
        quiet: Duration::from_millis(80),
        ..WatchConfig::default()
    };
    let (tx, rx) = mpsc::channel();
    let _watcher = Watcher::start(config, filter, move |event| {
        let _ = tx.send(event);
    });
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(10)).unwrap(),
        WatchEvent::Status(_)
    ));
    std::fs::write(real.join("file.rs"), "changed").unwrap();
    wait_paths(&rx, &[alias.join("file.rs"), real.join("file.rs")]);
}

#[test]
fn file_operations_follow_children_but_preserve_the_directory_link() {
    let root = scratch("symlink-ops");
    let external = scratch("symlink-ops-target");
    let link = root.join("alias");
    symlink(external.path(), &link).unwrap();
    cide_fs::ops::create_in(&[root.path().to_path_buf()], &link, "created.rs", false).unwrap();
    assert!(external.join("created.rs").is_file());
    let renamed = root.join("renamed");
    cide_fs::ops::rename(&link, &renamed).unwrap();
    assert!(renamed.is_symlink());
    let copied = cide_fs::copy::paste(
        &[root.path().to_path_buf()],
        std::slice::from_ref(&renamed),
        root.path(),
        cide_ipc::PasteMode::Copy,
    )
    .unwrap();
    assert!(copied[0].dest.is_symlink());
    assert_eq!(
        std::fs::read_link(&copied[0].dest).unwrap(),
        external.path()
    );
    let data_home = root.join("data");
    let trashed = cide_fs::trash::move_to_trash_in(&data_home, &renamed).unwrap();
    assert!(trashed.is_symlink());
    assert!(external.join("created.rs").is_file());
    let source = root.join("source");
    std::fs::create_dir_all(source.join("child")).unwrap();
    let destination = root.join("linked-destination");
    symlink(source.join("child"), &destination).unwrap();
    let result = cide_fs::copy::paste(
        &[root.path().to_path_buf()],
        std::slice::from_ref(&source),
        &destination,
        cide_ipc::PasteMode::Copy,
    );
    assert!(
        result.is_err(),
        "a destination linked into the source must refuse recursion"
    );
    assert!(!source.join("child/source").exists());
}

fn wait_paths(rx: &mpsc::Receiver<WatchEvent>, expected: &[PathBuf]) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = HashSet::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(WatchEvent::Changed(change)) => seen.extend(change.paths),
            Ok(WatchEvent::Status(_)) => {}
            Err(_) => break,
        }
        if expected.iter().all(|p| seen.contains(p)) {
            return;
        }
    }
    panic!("expected {expected:?}, received {seen:?}");
}

fn watch_aliases(polling: bool, recursive: bool) {
    let root = scratch("symlink-watch");
    let external = scratch("symlink-watch-target");
    let replacement = scratch("symlink-watch-replacement");
    std::fs::write(external.join("seed.rs"), "seed").unwrap();
    std::fs::write(replacement.join("other.rs"), "other").unwrap();
    symlink(external.path(), root.join("one")).unwrap();
    symlink(external.path(), root.join("two")).unwrap();
    symlink(".", root.join("cycle")).unwrap();
    symlink(replacement.join("missing/deep"), root.join("broken")).unwrap();
    std::fs::write(root.join("real.rs"), "original").unwrap();
    symlink("real.rs", root.join("file-link.rs")).unwrap();
    let index = build(root.path());
    let filter = Arc::new(filter(&index));
    let config = WatchConfig {
        roots: vec![root.path().to_path_buf()],
        dirs: index.watch_dirs(&filter),
        links: index.watch_links(&filter),
        polling,
        max_dir_watches: if recursive { 0 } else { 8192 },
        poll_interval: Duration::from_millis(70),
        debounce: Duration::from_millis(30),
        quiet: Duration::from_millis(80),
        ..WatchConfig::default()
    };
    let (tx, rx) = mpsc::channel();
    let _watcher = Watcher::start(config, filter, move |event| {
        let _ = tx.send(event);
    });
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(10)).unwrap(),
        WatchEvent::Status(_)
    ));
    let edit = |path: PathBuf, text: &str, seconds: u64| {
        std::fs::write(&path, text).unwrap();
        // notify's polling backend compares whole seconds. Set a distinct timestamp
        // without sleeping, even on a filesystem with coarse mtime resolution.
        let times = std::fs::FileTimes::new()
            .set_modified(std::time::SystemTime::now() + Duration::from_secs(seconds));
        std::fs::File::open(path).unwrap().set_times(times).unwrap();
    };
    edit(
        external.join("seed.rs"),
        "a longer edit from the real target",
        10,
    );
    wait_paths(&rx, &[root.join("one/seed.rs"), root.join("two/seed.rs")]);
    edit(
        root.join("real.rs"),
        "the source and file alias both refresh",
        10,
    );
    wait_paths(&rx, &[root.join("real.rs"), root.join("file-link.rs")]);
    std::fs::create_dir(external.join("new")).unwrap();
    wait_paths(&rx, &[root.join("one/new"), root.join("two/new")]);
    std::fs::write(external.join("new/nested.rs"), "created").unwrap();
    wait_paths(
        &rx,
        &[
            root.join("one/new/nested.rs"),
            root.join("two/new/nested.rs"),
        ],
    );
    std::fs::remove_file(root.join("one")).unwrap();
    symlink(replacement.path(), root.join("one")).unwrap();
    wait_paths(&rx, &[root.join("one")]);
    edit(replacement.join("other.rs"), "replacement changed", 20);
    wait_paths(&rx, &[root.join("one/other.rs")]);
    edit(
        external.join("seed.rs"),
        "the other alias must still be watched",
        30,
    );
    wait_paths(&rx, &[root.join("two/seed.rs")]);
    // Replace the target at exactly the same path: its parent watch must restore it.
    std::fs::remove_dir_all(external.path()).unwrap();
    wait_paths(&rx, &[root.join("two")]);
    std::fs::create_dir(external.path()).unwrap();
    wait_paths(&rx, &[root.join("two")]);
    std::fs::write(external.join("restored.rs"), "restored").unwrap();
    wait_paths(&rx, &[root.join("two/restored.rs")]);
    std::fs::create_dir_all(replacement.join("missing/deep")).unwrap();
    wait_paths(&rx, &[root.join("broken")]);
    std::fs::write(replacement.join("missing/deep/found.rs"), "found").unwrap();
    wait_paths(&rx, &[root.join("broken/found.rs")]);
}

#[test]
fn native_watches_refresh_all_aliases_and_retargeted_links() {
    watch_aliases(false, false);
}

#[test]
fn recursive_watches_refresh_all_aliases_and_retargeted_links() {
    watch_aliases(false, true);
}

#[test]
fn polling_refreshes_all_aliases_and_retargeted_links() {
    watch_aliases(true, false);
}
