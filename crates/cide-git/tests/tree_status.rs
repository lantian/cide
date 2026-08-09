//! The file tree's status column, against real temporary repositories.
//!
//! Every case here is a claim `ui/src/sidebar/FileTree.tsx` now makes on screen, so each one
//! builds an actual repository and drives it with the `git` binary rather than asserting
//! against a second model of git's rules. Nothing in this file touches the user's own
//! checkout: `support::TempRepo` isolates `HOME`, `XDG_STATE_HOME` and `XDG_CONFIG_HOME`
//! before the first repository exists, so a global `core.excludesFile` cannot change a
//! result either.
//!
//! The last test is the important one. Everything above it checks what `cide-git` computes;
//! `every_key_lands_on_a_row_the_file_index_actually_produced` checks that what it computes
//! can be *found* — it runs the real `cide-fs` walk over the same repository and compares the
//! two crates' path strings directly. That is the only claim neither crate can make alone,
//! and it is the one whose failure is silent.

mod support;

use std::path::{Path, PathBuf};

use cide_git::tree_status::tree_status;
use cide_ipc::git::TreeStatus;
use support::TempRepo;

/// The status of one path under one root, as the tree would look it up.
fn at(map: &cide_ipc::git::TreeStatusMap, root: &Path, rel: &str) -> Option<TreeStatus> {
    let key = root.join(rel).to_string_lossy().into_owned();
    map.statuses.get(&key).copied()
}

fn roots(repo: &TempRepo) -> Vec<PathBuf> {
    vec![repo.root.clone()]
}

#[test]
fn a_clean_repository_produces_an_empty_map() {
    let repo = TempRepo::new("tree-clean");
    repo.write("src/main.rs", b"fn main() {}\n");
    repo.commit_all("base");

    let map = tree_status(&roots(&repo));
    assert!(
        map.statuses.is_empty(),
        "a clean tree must ship nothing at all, not one `clean` entry per file: {:?}",
        map.statuses
    );
    assert!(!map.truncated);
}

#[test]
fn the_three_letters_the_mock_draws_are_all_reachable() {
    let repo = TempRepo::new("tree-letters");
    repo.write("src/edited.rs", b"one\n");
    repo.write("src/untracked-later.rs", b"one\n");
    repo.write("src/removed.rs", b"one\n");
    repo.commit_all("base");

    // M: edited in the working tree.
    repo.write("src/edited.rs", b"two\n");
    // A: staged as new.
    repo.write("src/staged.rs", b"new\n");
    repo.git(&["add", "src/staged.rs"]);
    // D: dropped from the index while still on disk. This is the only shape in which a file
    // row can be `deleted` at all — a path removed from the working tree has no row to tag.
    repo.git(&["rm", "--cached", "-q", "src/removed.rs"]);
    // Untracked: on disk, unknown to git.
    repo.write("src/fresh.rs", b"new\n");

    let map = tree_status(&roots(&repo));
    let root = &repo.root;

    assert_eq!(at(&map, root, "src/edited.rs"), Some(TreeStatus::Modified));
    assert_eq!(at(&map, root, "src/staged.rs"), Some(TreeStatus::Added));
    assert_eq!(at(&map, root, "src/removed.rs"), Some(TreeStatus::Deleted));
    assert_eq!(at(&map, root, "src/fresh.rs"), Some(TreeStatus::Untracked));
    assert_eq!(at(&map, root, "src/untracked-later.rs"), None);
}

#[test]
fn a_working_tree_deletion_tags_the_folder_it_left() {
    let repo = TempRepo::new("tree-deleted");
    repo.write("src/keep.rs", b"one\n");
    repo.write("src/gone.rs", b"one\n");
    repo.commit_all("base");
    repo.remove("src/gone.rs");

    let map = tree_status(&roots(&repo));
    let root = &repo.root;

    // The file itself has no row — `cide-fs` walks the filesystem and the path is not on it.
    // The folder above it is the only place the tree can report the deletion, and it does.
    assert_eq!(at(&map, root, "src"), Some(TreeStatus::Modified));
    assert_eq!(at(&map, root, "src/keep.rs"), None);
}

#[test]
fn directories_roll_up_and_stop_at_the_root() {
    let repo = TempRepo::new("tree-rollup");
    repo.write("a/b/c/deep.rs", b"one\n");
    repo.write("a/sibling.rs", b"one\n");
    repo.commit_all("base");
    repo.write("a/b/c/deep.rs", b"two\n");

    let map = tree_status(&roots(&repo));
    let root = &repo.root;

    assert_eq!(at(&map, root, "a/b/c/deep.rs"), Some(TreeStatus::Modified));
    assert_eq!(at(&map, root, "a/b/c"), Some(TreeStatus::Modified));
    assert_eq!(at(&map, root, "a/b"), Some(TreeStatus::Modified));
    assert_eq!(at(&map, root, "a"), Some(TreeStatus::Modified));
    assert_eq!(
        map.statuses.get(&root.to_string_lossy().into_owned()),
        Some(&TreeStatus::Modified),
        "the root row itself carries the rollup, so a multi-root project can see which root \
         is dirty without expanding it"
    );
    assert_eq!(
        at(&map, root, "a/sibling.rs"),
        None,
        "a sibling of a changed file is not itself changed"
    );

    let above = root
        .parent()
        .expect("a temp repo has a parent")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        map.statuses.get(&above),
        None,
        "the rollup must stop at the project root and not climb out to /tmp"
    );
}

#[test]
fn an_untracked_directory_is_reported_once_and_not_recursed() {
    let repo = TempRepo::new("tree-untracked-dir");
    repo.write("src/main.rs", b"fn main() {}\n");
    repo.commit_all("base");
    for i in 0..50 {
        repo.write(&format!("src/feature/f{i}.rs"), b"new\n");
    }

    let map = tree_status(&roots(&repo));
    let root = &repo.root;

    assert_eq!(
        at(&map, root, "src/feature"),
        Some(TreeStatus::Untracked),
        "the directory itself carries the status"
    );
    assert_eq!(
        at(&map, root, "src/feature/f0.rs"),
        None,
        "and its 50 children are NOT enumerated — the frontend resolves them from the \
         nearest ancestor, which is what keeps a fresh 100k-file checkout off the wire",
    );
    assert_eq!(
        at(&map, root, "src"),
        Some(TreeStatus::Modified),
        "the folder above a new directory still shows that something appeared under it"
    );
}

#[test]
fn an_ignored_directory_is_named_and_does_not_tint_its_parents() {
    // `cide-fs` walks with `parents(false)`: `.gitignore` files *above* a project root are
    // not consulted. So opening `outer/inner` as a project shows rows that git considers
    // ignored, which is what makes `TreeStatus::Ignored` reachable rather than dead code.
    let repo = TempRepo::new("tree-ignored");
    repo.write(".gitignore", b"inner/build/\n");
    repo.write("inner/src/main.rs", b"fn main() {}\n");
    repo.commit_all("base");
    repo.write("inner/build/artifact.o", b"\x00\x01");

    let root = repo.root.join("inner");
    let map = tree_status(std::slice::from_ref(&root));

    assert_eq!(
        at(&map, &root, "build"),
        Some(TreeStatus::Ignored),
        "the ignored directory is reported relative to the project root, not the work tree"
    );
    assert_eq!(
        at(&map, &root, "build/artifact.o"),
        None,
        "and is not recursed into"
    );
    assert_eq!(
        map.statuses.get(&root.to_string_lossy().into_owned()),
        None,
        "an ignored directory is not a change, so it must not tint the folders above it"
    );
}

#[test]
fn a_root_below_the_work_tree_only_sees_its_own_changes() {
    let repo = TempRepo::new("tree-subroot");
    repo.write("inside/a.rs", b"one\n");
    repo.write("outside/b.rs", b"one\n");
    repo.commit_all("base");
    repo.write("inside/a.rs", b"two\n");
    repo.write("outside/b.rs", b"two\n");

    let root = repo.root.join("inside");
    let map = tree_status(std::slice::from_ref(&root));

    assert_eq!(at(&map, &root, "a.rs"), Some(TreeStatus::Modified));
    assert!(
        map.statuses.keys().all(|k| Path::new(k).starts_with(&root)),
        "a change elsewhere in the repository has no row under this root and must not \
         produce a key that can never match one: {:?}",
        map.statuses,
    );
}

#[test]
fn a_symlinked_root_is_keyed_the_way_the_file_tree_spells_it() {
    // The failure this pins: git resolves the work tree to its canonical path, `cide-fs`
    // walks whatever path the project was configured with. Conflating the two produces a map
    // whose every key is right and whose every key matches no row, i.e. a tree with no tags
    // and nothing anywhere saying why.
    let repo = TempRepo::new("tree-symlink");
    repo.write("src/main.rs", b"fn main() {}\n");
    repo.commit_all("base");
    repo.write("src/main.rs", b"fn main() { }\n");

    let link = repo.root.with_extension("link");
    let _ = std::fs::remove_file(&link);
    if std::os::unix::fs::symlink(&repo.root, &link).is_err() {
        eprintln!("symlinks unavailable here; skipping");
        return;
    }

    let map = tree_status(std::slice::from_ref(&link));
    assert_eq!(
        at(&map, &link, "src/main.rs"),
        Some(TreeStatus::Modified),
        "keys must be spelled with the configured root, not the canonical one"
    );
    let _ = std::fs::remove_file(&link);
}

#[test]
fn two_roots_in_one_repository_are_both_tagged() {
    // `repo::discover` deduplicates by work tree, so a naive `discover(&roots)` would
    // attribute the whole repository's entries to whichever root came first and leave the
    // other silently untagged.
    let repo = TempRepo::new("tree-two-roots");
    repo.write("one/a.rs", b"x\n");
    repo.write("two/b.rs", b"x\n");
    repo.commit_all("base");
    repo.write("one/a.rs", b"y\n");
    repo.write("two/b.rs", b"y\n");

    let one = repo.root.join("one");
    let two = repo.root.join("two");
    let map = tree_status(&[one.clone(), two.clone()]);

    assert_eq!(at(&map, &one, "a.rs"), Some(TreeStatus::Modified));
    assert_eq!(at(&map, &two, "b.rs"), Some(TreeStatus::Modified));
}

#[test]
fn a_root_with_no_repository_is_empty_rather_than_an_error() {
    let dir = std::env::temp_dir().join(format!("cide-tree-status-plain-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("plain dir");
    std::fs::write(dir.join("src/a.txt"), b"hello").expect("write");

    let map = tree_status(std::slice::from_ref(&dir));
    assert!(map.statuses.is_empty());
    assert!(!map.truncated);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_conflicted_file_reads_as_modified() {
    let repo = TempRepo::new("tree-conflict");
    repo.write("f.txt", b"base\n");
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "other"]);
    repo.write("f.txt", b"other\n");
    repo.commit_all("other");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("f.txt", b"main\n");
    repo.commit_all("main");
    let (ok, _) = repo.try_git(&["merge", "other"]);
    assert!(!ok, "the merge is supposed to conflict");

    let map = tree_status(&roots(&repo));
    assert_eq!(
        at(&map, &repo.root, "f.txt"),
        Some(TreeStatus::Modified),
        "a conflict is shown as modified: the one-glyph column is not a merge tool, and the \
         commit panel is where conflicts are actually resolved"
    );
}

#[test]
fn a_submodule_inside_a_root_contributes_its_own_changes() {
    let inner = TempRepo::new("tree-submodule-inner");
    inner.write("lib.rs", b"one\n");
    inner.commit_all("base");

    let outer = TempRepo::new("tree-submodule-outer");
    outer.write("main.rs", b"fn main() {}\n");
    outer.commit_all("base");
    let (ok, out) = outer.try_git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "-q",
        inner.root.to_str().expect("utf-8 path"),
        "vendor/lib",
    ]);
    if !ok {
        eprintln!("submodules unavailable here ({out}); skipping");
        return;
    }
    outer.commit_all("add submodule");
    outer.write("vendor/lib/lib.rs", b"two\n");

    let map = tree_status(&roots(&outer));
    assert_eq!(
        at(&map, &outer.root, "vendor/lib/lib.rs"),
        Some(TreeStatus::Modified),
        "a submodule's own file is dirty and the tree shows it — the superproject only ever \
         reports the gitlink, which is not a row the user can act on"
    );
}

// --- the join itself ------------------------------------------------------------------------

/// Every key the map produces, resolved the way `ui/src/sidebar/treeStatus.ts` resolves one.
///
/// Kept in step with that file by hand, which is the cost of the two halves living in two
/// languages. It is a short function and the behaviour it encodes — the nearest ancestor with
/// an entry decides, and only `untracked` and `ignored` are inherited — is pinned separately
/// on the TypeScript side by `ui/scripts/check-tree-status.mjs`.
fn resolve(map: &cide_ipc::git::TreeStatusMap, path: &Path) -> TreeStatus {
    let key = path.to_string_lossy().into_owned();
    if let Some(own) = map.statuses.get(&key) {
        return *own;
    }
    let mut at = path.parent();
    while let Some(dir) = at {
        if let Some(status) = map.statuses.get(&dir.to_string_lossy().into_owned()) {
            return match status {
                TreeStatus::Untracked | TreeStatus::Ignored => *status,
                _ => TreeStatus::Clean,
            };
        }
        at = dir.parent();
    }
    TreeStatus::Clean
}

/// The map's keys are byte-identical to the paths `cide-fs` puts on real rows.
///
/// This is the one claim neither crate can make alone, and it is where the whole feature
/// breaks if it breaks: `cide-git` spells paths from the canonical work tree, `cide-fs` spells
/// them from the configured root, and a mismatch produces a map whose every entry is correct
/// and whose every entry matches no row. The failure mode is a tree with no tags and no error
/// anywhere — so the two real walkers are run over one real repository and the strings are
/// compared directly.
#[test]
fn every_key_lands_on_a_row_the_file_index_actually_produced() {
    let repo = TempRepo::new("tree-join");
    repo.write("src/edited.rs", b"one\n");
    repo.write("src/clean.rs", b"one\n");
    repo.write("docs/readme.md", b"docs\n");
    repo.commit_all("base");
    repo.write("src/edited.rs", b"two\n");
    repo.write("src/added.rs", b"new\n");
    repo.git(&["add", "src/added.rs"]);
    repo.write("fresh/a.rs", b"new\n");
    repo.write("fresh/b.rs", b"new\n");

    let map = tree_status(&roots(&repo));

    // The real walk, with the real defaults the app uses.
    let mut index = cide_fs::Index::build(
        vec![cide_fs::Root::new(&repo.root)],
        cide_fs::BuildOptions::default(),
        &|_| {},
    );
    // Expand everything the assertions name; the tree starts collapsed.
    for dir in ["src", "docs", "fresh"] {
        index
            .expand(&repo.root.join(dir))
            .unwrap_or_else(|| panic!("{dir} should be a directory in the index"));
    }
    let rows = index.rows(0, index.count());
    let by_path: std::collections::HashMap<String, TreeStatus> = rows
        .iter()
        .map(|row| {
            (
                row.path.to_string_lossy().into_owned(),
                resolve(&map, &row.path),
            )
        })
        .collect();

    let of = |rel: &str| {
        let key = repo.root.join(rel).to_string_lossy().into_owned();
        *by_path
            .get(&key)
            .unwrap_or_else(|| panic!("no row for {key}; rows: {:?}", by_path.keys()))
    };

    assert_eq!(of("src/edited.rs"), TreeStatus::Modified);
    assert_eq!(of("src/added.rs"), TreeStatus::Added);
    assert_eq!(of("src/clean.rs"), TreeStatus::Clean);
    assert_eq!(
        of("docs"),
        TreeStatus::Clean,
        "an untouched folder stays clean"
    );
    assert_eq!(of("docs/readme.md"), TreeStatus::Clean);
    assert_eq!(of("src"), TreeStatus::Modified, "the folder rolls up");
    assert_eq!(of("fresh"), TreeStatus::Untracked);
    assert_eq!(
        of("fresh/a.rs"),
        TreeStatus::Untracked,
        "inherited from the directory: git never enumerated this path, and the row exists \
         because `cide-fs` walks the filesystem rather than the index"
    );

    // And the converse: no key names a path the walk never produced. A key that matches no row
    // is invisible, which is exactly how a canonicalisation bug hides.
    let walked: std::collections::HashSet<String> = rows
        .iter()
        .map(|row| row.path.to_string_lossy().into_owned())
        .chain(std::iter::once(repo.root.to_string_lossy().into_owned()))
        .collect();
    let orphans: Vec<&String> = map
        .statuses
        .keys()
        .filter(|k| !walked.contains(*k))
        .collect();
    assert!(
        orphans.is_empty(),
        "these keys can never match a row: {orphans:?}"
    );
}
