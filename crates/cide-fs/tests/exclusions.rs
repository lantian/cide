use cide_fs::budget::{Budget, SharedBudget};
use cide_fs::testing::scratch;
use cide_fs::{BuildOptions, Exclusions, Filter, Index, Root, Visibility};
use cide_ipc::{ExplorerSettings, FsChange};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

const ALL: Visibility = Visibility {
    hidden: true,
    ignored: true,
};
fn policy(root: &Path, rules: &[&str]) -> Arc<Exclusions> {
    Arc::new(
        Exclusions::compile(
            vec![root.into()],
            &rules.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
        .unwrap(),
    )
}

#[test]
fn rules_are_root_relative_component_safe_and_validated() {
    let p = policy(Path::new("/project"), &[".cide/tasks", "**/node_modules"]);
    assert!(
        policy(Path::new("/project"), &["./build/"])
            .matching_rule(Path::new("/project/build/a.rs"), false)
            .is_some()
    );
    for path in [
        "/project/.cide/tasks/a.md",
        "/project/node_modules/pkg/a.js",
        "/project/ui/node_modules/pkg/a.js",
    ] {
        assert!(p.matching_rule(Path::new(path), false).is_some(), "{path}");
    }
    for path in [
        "/project/.cide/tasks.json",
        "/project/.cide/tasks-old/a",
        "/project/sub/.cide/tasks/a",
        "/outside/.cide/tasks/a",
        "/project/node_modules",
    ] {
        assert!(p.matching_rule(Path::new(path), false).is_none(), "{path}");
    }
    for invalid in [
        "", "/tmp", "../a", "a/../b", "!a", "C:/a", "a\\b", "[abc", "a//b",
    ] {
        assert!(
            Exclusions::compile(vec![], &[invalid.into()]).is_err(),
            "{invalid}"
        );
    }
    assert!(
        policy(Path::new("/project"), &[])
            .matching_rule(Path::new("/project/.cide/tasks"), true)
            .is_none()
    );
}

#[test]
fn lazy_pages_reveal_refresh_and_collapse_never_feed_the_index() {
    let dir = scratch("excluded-pages");
    let folder = dir.join(".cide/tasks");
    std::fs::create_dir_all(folder.join("nested")).unwrap();
    std::fs::write(folder.join("nested/deep.rs"), "fn invisible() {}").unwrap();
    std::fs::write(dir.join(".cide/tasks.json"), "{}").unwrap();
    for n in 0..1200 {
        std::fs::write(folder.join(format!("file{n:04}.rs")), "").unwrap();
    }
    let exclusions = policy(&dir, &[".cide/tasks"]);
    let budget = Budget::new(SharedBudget::new(64 * 1024 * 1024), 64 * 1024 * 1024);
    let candidates = Mutex::new(Vec::new());
    let mut index = Index::build_controlled(
        vec![Root::new(dir.path())],
        BuildOptions {
            visibility: ALL,
            exclusions: exclusions.clone(),
            ..Default::default()
        },
        budget.clone(),
        &|batch| candidates.lock().unwrap().extend_from_slice(batch),
    );
    assert_eq!(index.files(), 1);
    let library_candidates = Mutex::new(Vec::new());
    Index::walk_roots(
        &[Root::new(folder.clone())],
        BuildOptions {
            visibility: ALL,
            exclusions: exclusions.clone(),
            ..Default::default()
        },
        false,
        &|batch| library_candidates.lock().unwrap().extend_from_slice(batch),
    );
    assert!(library_candidates.lock().unwrap().is_empty());
    assert!(
        candidates
            .lock()
            .unwrap()
            .iter()
            .all(|i| !i.path.starts_with(&folder))
    );
    let filter = Filter::build(
        &[dir.path().into()],
        index.dir_paths().iter().map(|p| p.as_path()),
        ALL,
    )
    .with_exclusions(exclusions);
    let charged = budget.used();
    let counts = (index.files(), index.dirs());
    index.expand(&dir.join(".cide"));
    index.set_all_expanded(true, |_| true);
    assert!(
        !index
            .rows(0, 4096)
            .iter()
            .find(|r| r.path == folder)
            .unwrap()
            .expanded
    );
    index.expand(&folder);
    let rows = index.rows(0, 4096);
    let marker = rows.iter().find(|r| r.path == folder).unwrap();
    assert!(marker.excluded_from_index && marker.browse_page.as_ref().unwrap().next);
    assert_eq!(
        rows.iter()
            .filter(|r| r.path.parent() == Some(folder.as_path()))
            .count(),
        512
    );
    assert!(!index.contains(&folder.join("nested/deep.rs")));
    index.browse_page(&folder, 1);
    assert!(!index.contains(&folder.join("file0000.rs")));
    index.browse_page(&folder, -1);
    assert!(index.contains(&folder.join("file0000.rs")));
    assert!(index.reveal(&folder.join("file1199.rs")).is_some());
    assert!(index.reveal(&folder.join("nested/deep.rs")).is_some());
    std::fs::write(folder.join("new.rs"), "").unwrap();
    assert!(
        index
            .apply(
                &FsChange {
                    paths: vec![folder.join("new.rs")],
                    truncated: false,
                    git: false
                },
                &filter
            )
            .is_empty()
    );
    assert!(index.refresh_subtree(&folder, &filter).is_empty());
    assert_eq!((index.files(), index.dirs()), counts);
    assert_eq!(budget.used(), charged);
    assert!(index.file_paths().iter().all(|p| !p.starts_with(&folder)));
    assert!(
        index
            .largest_folders()
            .iter()
            .all(|p| !Path::new(&p.path).starts_with(&folder))
    );
    assert!(!filter.admits(&folder.join("new.rs"), false));
    assert!(index.browse_cache_bytes() > 0);
    index.collapse(&dir.join(".cide"));
    assert_eq!(index.browse_cache_bytes(), 0);
    assert!(!index.contains(&folder.join("new.rs")));
    assert!(index.contains(&folder));
    index.expand(&folder);
    assert!(index.browse_cache_bytes() > 0);
    index.set_all_expanded(false, |_| true);
    assert_eq!(index.browse_cache_bytes(), 0);
    std::fs::remove_dir_all(&folder).unwrap();
    index.apply(
        &FsChange {
            paths: vec![folder.clone()],
            truncated: false,
            git: false,
        },
        &filter,
    );
    assert!(
        !index.contains(&folder),
        "deleting an excluded directory removes its marker"
    );
}

#[test]
fn new_excluded_folders_are_markers_and_maintenance_watch_is_narrow() {
    let dir = scratch("excluded-updates");
    std::fs::create_dir_all(dir.join(".cide")).unwrap();
    let exclusions = Arc::new(
        Exclusions::compile(
            vec![dir.path().into()],
            &ExplorerSettings::default().index_excluded_folders,
        )
        .unwrap(),
    );
    let mut index = Index::build(
        vec![Root::new(dir.path())],
        BuildOptions {
            visibility: ALL,
            exclusions: exclusions.clone(),
            ..Default::default()
        },
        &|_| {},
    );
    let filter = Filter::build(
        &[dir.path().into()],
        index.dir_paths().iter().map(|p| p.as_path()),
        ALL,
    )
    .with_exclusions(exclusions);
    std::fs::create_dir_all(dir.join(".cide/worktrees/worker/src")).unwrap();
    std::fs::write(dir.join(".cide/worktrees/worker/src/main.rs"), "").unwrap();
    let added = index.refresh_subtree(&dir, &filter);
    assert!(added.is_empty());
    assert!(index.contains(&dir.join(".cide/worktrees")));
    assert!(!index.contains(&dir.join(".cide/worktrees/worker")));
    // A refresh can arrive while the initial walk is still filling a separate arena.
    let mut pending = Index::empty_controlled(
        vec![Root::new(dir.path())],
        ALL,
        Budget::standalone(),
        policy(&dir, &[".cide/worktrees"]),
    );
    assert!(
        pending
            .refresh_subtree(&dir, &filter)
            .iter()
            .all(|item| !item.path.starts_with(dir.join(".cide/worktrees")))
    );
    assert!(!pending.contains(&dir.join(".cide/worktrees/worker")));
    for path in [
        ".cide/tasks.json",
        ".cide/config.json",
        ".cide/agents/worker.md",
        ".cide/tasks/task/attachment.png",
        ".cide/worktrees",
        ".cide/worktrees/worker",
        ".cide/worktrees/worker/openspec/changes/fix/tasks.md",
    ] {
        assert!(filter.watchable(&dir.join(path), false), "{path}");
    }
    assert!(!filter.watchable(&dir.join(".cide/worktrees/worker/src"), true));
    assert!(!filter.watchable(&dir.join(".cide/worktrees/worker/src/main.rs"), false));
}

#[test]
fn each_root_and_an_independently_opened_worktree_have_their_own_boundary() {
    let a = scratch("excluded-root-a");
    let b = scratch("excluded-root-b");
    let rules = ExplorerSettings::default().index_excluded_folders;
    let p = Exclusions::compile(vec![a.path().into(), b.path().into()], &rules).unwrap();
    for root in [a.path(), b.path()] {
        assert!(
            p.matching_rule(&root.join(".cide/tasks/a"), false)
                .is_some()
        );
    }
    let worktree = a.join(".cide/worktrees/worker");
    let own = Exclusions::compile(vec![worktree.clone()], &rules).unwrap();
    assert!(
        own.matching_rule(&worktree.join("src/main.rs"), false)
            .is_none()
    );
    assert!(
        own.matching_rule(&worktree.join(".cide/tasks/a"), false)
            .is_some()
    );
}

#[test]
fn maintenance_watcher_starts_inside_excluded_folders_without_watching_checkout_sources() {
    use cide_fs::{WatchConfig, WatchEvent, Watcher};
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };
    let dir = scratch("exclusions-maintenance-watch");
    for path in [
        ".cide/tasks/one",
        ".cide/agents",
        ".cide/worktrees/worker/openspec/changes/fix",
        ".cide/worktrees/worker/src/deep",
    ] {
        std::fs::create_dir_all(dir.join(path)).unwrap();
    }
    let paths = [
        ".cide/tasks/one/body.md",
        ".cide/agents/worker.md",
        ".cide/worktrees/worker/openspec/changes/fix/tasks.md",
    ];
    for path in paths {
        std::fs::write(dir.join(path), "before").unwrap();
    }
    let source = dir.join(".cide/worktrees/worker/src/deep/main.rs");
    std::fs::write(&source, "before").unwrap();
    let exclusions = Arc::new(
        Exclusions::compile(
            vec![dir.path().into()],
            &ExplorerSettings::default().index_excluded_folders,
        )
        .unwrap(),
    );
    let filter = Arc::new(
        Filter::build(&[dir.path().into()], [dir.path()], Visibility::CONSERVATIVE)
            .with_exclusions(exclusions),
    );
    let (tx, rx) = mpsc::channel();
    let watcher = Watcher::start(
        WatchConfig {
            roots: vec![dir.path().into()],
            dirs: vec![dir.path().into()],
            debounce: Duration::from_millis(50),
            quiet: Duration::from_millis(100),
            ..Default::default()
        },
        filter,
        move |e| {
            let _ = tx.send(e);
        },
    );
    let status = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(status, WatchEvent::Status(_)));
    for path in paths {
        std::fs::write(dir.join(path), "after").unwrap();
    }
    std::fs::write(&source, "after").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut received = std::collections::HashSet::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let Ok(event) = rx.recv_timeout(left) else {
            break;
        };
        if let WatchEvent::Changed(change) = event {
            received.extend(change.paths);
        }
        if paths.iter().all(|p| received.contains(&dir.join(p))) {
            break;
        }
    }
    drop(watcher);
    for path in paths {
        assert!(received.contains(&dir.join(path)), "missing {path}");
    }
    assert!(!received.contains(&source));
}

#[cfg(unix)]
#[test]
fn lazy_symlink_cycles_and_read_errors_are_visible() {
    let dir = scratch("excluded-links");
    let folder = dir.join("excluded");
    std::fs::create_dir_all(&folder).unwrap();
    std::os::unix::fs::symlink(&folder, folder.join("loop")).unwrap();
    let mut index = Index::build(
        vec![Root::new(dir.path())],
        BuildOptions {
            visibility: ALL,
            exclusions: policy(&dir, &["excluded"]),
            ..Default::default()
        },
        &|_| {},
    );
    index.expand(&folder);
    index.expand(&folder.join("loop"));
    let rows = index.rows(0, 100);
    assert!(
        rows.iter()
            .find(|r| r.name == "loop")
            .unwrap()
            .browse_page
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("cycle")
    );
    std::fs::remove_dir_all(&folder).unwrap();
    index.browse_page(&folder, 0);
    assert!(
        index
            .rows(0, 100)
            .iter()
            .find(|r| r.path == folder)
            .unwrap()
            .browse_page
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("Cannot read")
    );
}
