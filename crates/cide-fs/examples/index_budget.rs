//! Isolated memory regression benchmark: no application state or live projects are read.
//! `cargo build --locked -p cide-fs --example index_budget`, then compare:
//! `/usr/bin/time -l target/debug/examples/index_budget 10000 8`
//! `/usr/bin/time -l target/debug/examples/index_budget 100000 8`
use cide_fs::budget::{Budget, MIB, SharedBudget};
use cide_fs::{BuildOptions, Index, Root, Visibility};
use cide_search::{Candidate, NucleoMatcher};
use std::sync::Arc;

fn main() {
    let count: usize = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "10000".into())
        .parse()
        .unwrap();
    let mib: u64 = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "8".into())
        .parse()
        .unwrap();
    let cancel = std::env::args().nth(3).as_deref() == Some("cancel");
    let root = cide_fs::testing::scratch("index-budget-bench");
    let external = cide_fs::testing::scratch("index-budget-bench-drive");
    std::fs::create_dir_all(root.join(".cache/dosdevices")).unwrap();
    std::fs::write(root.join(".gitignore"), ".cache/\n").unwrap();
    std::fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();
    for i in 0..count {
        std::fs::write(external.join(format!("file{i:07}.rs")), "fn example() {}\n").unwrap();
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(external.path(), root.join(".cache/dosdevices/z:")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(external.path(), root.join(".cache/dosdevices/z-drive"))
        .unwrap();
    let budget = Budget::new(SharedBudget::new(mib * MIB), mib * MIB);
    budget.reserve(2 * MIB); // matcher pools and bounded batches
    let matcher = NucleoMatcher::with_owner(budget.clone());
    let started = std::time::Instant::now();
    let index = Index::build_controlled(
        vec![Root::new(root.path())],
        BuildOptions {
            visibility: Visibility {
                hidden: true,
                ignored: true,
            },
            ..BuildOptions::default()
        },
        Arc::clone(&budget),
        &|batch| {
            for item in batch.iter().filter(|i| !i.is_dir) {
                matcher.push(Candidate::new(&item.rel, item.path.to_str().unwrap()));
            }
            if cancel && matcher.candidate_count() >= 512 {
                budget.cancel();
            }
        },
    );
    println!(
        "fixture={count} files={} accounted={} limit={} reason={:?} cancelled={} elapsed={:?}",
        index.files(),
        budget.used(),
        mib * MIB,
        budget.reason(),
        budget.cancelled(),
        started.elapsed()
    );
    assert!(budget.used() <= mib * MIB);
    if cancel {
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }
}
