use cide_fs::budget::{Budget, SharedBudget};
use cide_fs::testing::scratch;
use cide_fs::{BuildOptions, Index, Root, Visibility};
use cide_ipc::{IndexAllocation, IndexLimit};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[test]
fn reservations_are_atomic_and_live_until_the_last_generation_reader() {
    let shared = SharedBudget::new(4096);
    let one = Budget::new(Arc::clone(&shared), 3072);
    let two = Budget::new(Arc::clone(&shared), 3072);
    std::thread::scope(|s| {
        for budget in [&one, &two] {
            s.spawn(move || {
                for _ in 0..100 {
                    budget.reserve(64);
                }
            });
        }
    });
    assert!(one.used() <= 3072 && two.used() <= 3072);
    assert!(shared.used() <= 4096);
    let reader = Arc::clone(&one);
    let charged = one.used();
    drop(one);
    drop(two);
    assert_eq!(shared.used(), charged);
    drop(reader);
    assert_eq!(shared.used(), 0);
}

#[test]
fn shrinking_a_shared_budget_refuses_more_allocations_without_unaccounting_live_data() {
    let shared = SharedBudget::new(8192);
    let budget = Budget::new(shared.clone(), 8192);
    assert!(budget.reserve(4096));
    shared.set_limit(2048);
    assert!(!budget.reserve(1));
    assert_eq!(budget.reason(), Some(IndexLimit::TotalBudget));
    assert_eq!(shared.used(), 4096);
    drop(budget);
    assert_eq!(shared.used(), 0);
}

#[test]
fn diagnostics_preserve_the_first_refusal_and_its_usage() {
    let shared = SharedBudget::new(8192);
    let budget = Budget::new(shared.clone(), 4096);
    assert!(budget.reserve(4000));
    let path = std::path::Path::new("project/cache/large.rs");
    assert!(!budget.reserve_for(100, IndexAllocation::Symbols, Some(path)));
    let first = budget.refusal().unwrap();
    assert_eq!(first.reason, IndexLimit::ProjectBudget);
    assert_eq!(first.allocation, IndexAllocation::Symbols);
    assert_eq!(first.path.as_deref(), Some("project/cache/large.rs"));
    assert_eq!(first.folder.as_deref(), Some("project/cache"));
    assert_eq!(first.requested_bytes, 100);
    assert_eq!(first.usage.project_bytes, 4000);
    assert_eq!(first.usage.total_bytes, 4000);
    shared.set_limit(1);
    assert!(!budget.reserve_for(
        9000,
        IndexAllocation::Watcher,
        Some(std::path::Path::new("other"))
    ));
    assert_eq!(budget.refusal(), Some(first));
    assert_eq!(budget.usage().total_limit_bytes, 1);
    let next = Budget::new(SharedBudget::new(8192), 4096);
    assert_eq!(next.refusal(), None);
}

#[test]
fn diagnostics_explain_combined_limit_without_inventing_a_path() {
    let shared = SharedBudget::new(4096);
    let first = Budget::new(shared.clone(), 8192);
    let second = Budget::new(shared, 8192);
    assert!(first.reserve(4000));
    assert!(!second.reserve(100));
    let refusal = second.refusal().unwrap();
    assert_eq!(refusal.reason, IndexLimit::TotalBudget);
    assert_eq!(refusal.allocation, IndexAllocation::Overhead);
    assert_eq!(refusal.path, None);
    assert_eq!(refusal.folder, None);
    assert_eq!(
        refusal.usage.project_bytes, 0,
        "failed reservation is rolled back"
    );
    assert_eq!(refusal.usage.total_bytes, 4000);
}

#[test]
fn diagnostic_path_reservations_identify_the_provider() {
    for allocation in [
        IndexAllocation::Files,
        IndexAllocation::Libraries,
        IndexAllocation::Watcher,
    ] {
        let budget = Budget::new(SharedBudget::new(4096), 1);
        assert!(!budget.reserve_path_for(std::path::Path::new("root/file"), "file", allocation));
        assert_eq!(budget.refusal().unwrap().allocation, allocation);
    }
}

#[cfg(unix)]
#[test]
fn wine_like_external_links_plateau_and_preserve_sources_in_every_root() {
    let root = scratch("budget-wine");
    let other = scratch("budget-other-root");
    let outside = scratch("budget-external-drive");
    std::fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(root.join(".gitignore"), ".cache/\n").unwrap();
    std::fs::write(other.join("important.rs"), "fn important() {}\n").unwrap();
    std::fs::create_dir_all(root.join(".cache/dosdevices")).unwrap();
    for i in 0..3000 {
        std::fs::write(outside.join(format!("file{i}.rs")), "fn linked() {}\n").unwrap();
    }
    std::os::unix::fs::symlink(outside.path(), root.join(".cache/dosdevices/z:")).unwrap();
    let budget = Budget::new(SharedBudget::new(256 * 1024), 128 * 1024);
    let started = Instant::now();
    let mut index = Index::build_controlled(
        vec![Root::new(root.path()), Root::new(other.path())],
        BuildOptions {
            visibility: Visibility {
                hidden: true,
                ignored: true,
            },
            ..BuildOptions::default()
        },
        budget.clone(),
        &|_| {},
    );
    assert!(index.contains(&root.join("main.rs")));
    assert!(index.contains(&other.join("important.rs")));
    assert!(
        index
            .file_paths()
            .iter()
            .any(|p| p.starts_with(root.join(".cache/dosdevices/z:")))
    );
    assert!(index.files() < 3000);
    assert_eq!(budget.reason(), Some(IndexLimit::ProjectBudget));
    assert!(budget.used() <= 128 * 1024);
    let charged = budget.used();
    let files = index.files();
    let dirs = index.dir_paths();
    let filter = cide_fs::Filter::build(
        &[root.path().into(), other.path().into()],
        dirs.iter().map(|p| p.as_path()),
        index.visibility(),
    );
    for _ in 0..3 {
        index.refresh_subtree(root.path(), &filter);
    }
    assert_eq!(budget.used(), charged);
    assert_eq!(
        index.files(),
        files,
        "a partial rescan must not delete unvisited children"
    );
    eprintln!(
        "bounded fixture: {} files, {} accounted bytes, {:?}",
        files,
        charged,
        started.elapsed()
    );
}

#[test]
fn cancellation_in_the_sink_stops_before_another_batch() {
    let root = scratch("budget-cancel");
    for i in 0..2000 {
        std::fs::write(root.join(format!("file{i}")), "").unwrap();
    }
    let budget = Budget::new(SharedBudget::new(8 * 1024 * 1024), 4 * 1024 * 1024);
    let started = Instant::now();
    let index = Index::build_controlled(
        vec![Root::new(root.path())],
        BuildOptions {
            batch: 16,
            ..BuildOptions::default()
        },
        budget.clone(),
        &|_| budget.cancel(),
    );
    assert!(index.files() <= 16);
    assert!(started.elapsed() < Duration::from_secs(1));
    eprintln!("cancel latency: {:?}", started.elapsed());
}

#[test]
fn deep_wide_directories_do_not_require_a_whole_directory_buffer() {
    let root = scratch("budget-deep-wide");
    let deep = root.join("a/b/c/d/e/f/g/h/i/j/k/l");
    std::fs::create_dir_all(&deep).unwrap();
    for i in 0..1000 {
        std::fs::write(deep.join(format!("{i}.txt")), "").unwrap();
    }
    let budget = Budget::new(SharedBudget::new(64 * 1024), 64 * 1024);
    let index = Index::build_controlled(
        vec![Root::new(root.path())],
        BuildOptions::default(),
        budget.clone(),
        &|_| {},
    );
    assert!(index.files() > 0 && index.files() < 1000);
    assert!(budget.used() <= 64 * 1024);
}
