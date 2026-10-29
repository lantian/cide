use cide_fs::budget::{Budget, SharedBudget};
use cide_fs::testing::scratch;
use cide_fs::{BuildOptions, Filter, Index, Root, Visibility};
use cide_ipc::FsChange;

#[test]
fn nested_subtrees_are_inclusive_and_diagnostics_do_not_read_disk() {
    let root = scratch("diagnostics-nested");
    let child = root.join("ui/node_modules");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::write(child.join("one.js"), "x").unwrap();
    std::fs::write(root.join("ui/main.ts"), "x").unwrap();
    let index = Index::build(
        vec![Root::new(root.path())],
        BuildOptions::default(),
        &|_| {},
    );
    // Removing the entire subtree on disk cannot change an in-memory diagnostic snapshot.
    std::fs::remove_dir_all(root.join("ui")).unwrap();
    let rows = index.largest_folders();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].relative_path, "ui");
    assert_eq!((rows[0].files, rows[0].dirs), (2, 2));
    assert_eq!((rows[1].files, rows[1].dirs), (1, 1));
    let cost = |rel: &str| Budget::path_bytes(&root.join(rel), rel);
    assert_eq!(
        rows[1].bytes,
        cost("ui/node_modules") + cost("ui/node_modules/one.js")
    );
    assert_eq!(
        rows[0].bytes,
        rows[1].bytes + cost("ui") + cost("ui/main.ts")
    );
}

#[test]
fn top_ten_are_deterministic_and_deleted_nodes_are_not_ranked() {
    let root = scratch("diagnostics-ranking");
    for n in 0..15 {
        std::fs::create_dir(root.join(format!("d{n:02}"))).unwrap();
    }
    let mut index = Index::build(
        vec![Root::new(root.path())],
        BuildOptions::default(),
        &|_| {},
    );
    let rows = index.largest_folders();
    assert_eq!(rows.len(), 10);
    assert_eq!(rows[0].relative_path, "d00");
    assert_eq!(rows[9].relative_path, "d09");
    std::fs::remove_dir(root.join("d00")).unwrap();
    let filter = Filter::build(
        &[root.path().into()],
        std::iter::empty(),
        Visibility::CONSERVATIVE,
    );
    index.apply(
        &FsChange {
            paths: vec![root.join("d00")],
            truncated: false,
            git: false,
        },
        &filter,
    );
    let rows = index.largest_folders();
    assert_eq!(rows[0].relative_path, "d01");
    assert_eq!(rows[9].relative_path, "d10");
}

#[test]
fn multiple_roots_and_partial_coverage_keep_paths_unambiguous() {
    let a = scratch("diagnostics-root-a");
    let b = scratch("diagnostics-root-b");
    for root in [a.path(), b.path()] {
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
    }
    let index = Index::build(
        vec![Root::new(a.path()), Root::new(b.path())],
        BuildOptions::default(),
        &|_| {},
    );
    let rows = index.largest_folders();
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .all(|r| r.relative_path == "src" && r.files == 1)
    );
    assert_ne!(rows[0].root, rows[1].root);
    for row in &rows {
        let root = Root::new(&row.root);
        assert_eq!(
            row.bytes,
            Budget::path_bytes(&root.path.join("src"), &format!("{}/src", root.label))
                + Budget::path_bytes(
                    &root.path.join("src/main.rs"),
                    &format!("{}/src/main.rs", root.label)
                )
        );
    }
    let budget = Budget::new(SharedBudget::new(1), 1);
    let partial = Index::build_controlled(
        vec![Root::new(a.path())],
        BuildOptions::default(),
        budget.clone(),
        &|_| {},
    );
    assert!(budget.refusal().is_some());
    assert!(partial.largest_folders().is_empty());
}

#[cfg(unix)]
#[test]
fn linked_subtrees_use_the_logical_index_path() {
    let root = scratch("diagnostics-link");
    let external = scratch("diagnostics-target");
    std::fs::write(external.join("file.rs"), "").unwrap();
    std::os::unix::fs::symlink(external.path(), root.join("linked")).unwrap();
    let index = Index::build(
        vec![Root::new(root.path())],
        BuildOptions::default(),
        &|_| {},
    );
    let rows = index.largest_folders();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].path, root.join("linked").to_string_lossy());
    assert_eq!(rows[0].files, 1);
}
