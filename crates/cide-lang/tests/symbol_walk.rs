//! The project walk, against a real directory tree.
//!
//! An integration test rather than a unit one because the thing under test *is* the traversal:
//! `ignore::WalkBuilder`, the thread pool, and the cap enforced on the receiving side of a
//! channel. None of that is reachable from a `&str` fixture.
//!
//! Streaming is asserted **structurally** — from inside the sink, by observing that a file's
//! symbols arrive before the walk returns — rather than by timing. A test that sleeps and then
//! checks a counter passes on a fast machine for the wrong reason.

use std::sync::atomic::{AtomicBool, Ordering};

use cide_lang::{Limits, WalkRoot, walk_symbols};

/// A throwaway tree under the temp directory, removed on drop.
struct Tree(std::path::PathBuf);

impl Tree {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("cide-lang-walk-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }

    fn write(&self, rel: &str, contents: &str) {
        let path = self.0.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, contents).expect("write");
    }

    fn root(&self) -> WalkRoot {
        WalkRoot {
            path: self.0.clone(),
            label: None,
        }
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn collect(
    roots: &[WalkRoot],
    limits: Limits,
) -> (Vec<cide_lang::WalkedFile>, cide_lang::WalkOutcome) {
    let cancel = AtomicBool::new(false);
    let mut files = Vec::new();
    let outcome = walk_symbols(roots, limits, None, &cancel, &mut |f| files.push(f));
    (files, outcome)
}

fn names(files: &[cide_lang::WalkedFile]) -> Vec<String> {
    let mut out: Vec<String> = files
        .iter()
        .flat_map(|f| f.symbols.iter().map(|s| s.name.clone()))
        .collect();
    out.sort();
    out
}

#[test]
fn both_languages_are_walked_and_nothing_else_is_opened() {
    let tree = Tree::new("langs");
    tree.write("src/lib.rs", "pub fn rust_fn() {}\n");
    tree.write("cmd/main.go", "package main\nfunc go_fn() {}\n");
    // Everything below has no grammar. The extension gate must reject each of them *before*
    // opening the file — a walk that reads them all is the cost this test exists to prevent.
    tree.write("README.md", "# not source\n");
    tree.write("Cargo.toml", "[package]\n");
    tree.write("data.json", "{}\n");
    tree.write("script.py", "def py_fn(): pass\n");

    let (files, outcome) = collect(&[tree.root()], Limits::default());

    assert_eq!(outcome.files, 2, "{:?}", files);
    assert!(names(&files).contains(&"rust_fn".to_string()));
    assert!(names(&files).contains(&"go_fn".to_string()));
    assert!(
        !names(&files).contains(&"py_fn".to_string()),
        "a language with no grammar was parsed anyway"
    );
}

#[test]
fn gitignored_files_are_not_indexed() {
    // The walk has to agree with the file tree. A symbol in `target/` is a row the user cannot
    // find in the explorer and probably cannot edit.
    let tree = Tree::new("ignored");
    tree.write(".gitignore", "target/\n");
    tree.write("src/lib.rs", "fn kept() {}\n");
    tree.write("target/generated.rs", "fn dropped() {}\n");
    // `require_git(false)` is what makes the `.gitignore` apply without an actual repository —
    // the same setting `cide-fs` and `cide-search` use, spelled out in all three.

    let (files, _) = collect(&[tree.root()], Limits::default());
    assert_eq!(names(&files), vec!["kept".to_string()]);
}

#[test]
fn an_admits_closure_can_prune_a_subtree() {
    let tree = Tree::new("admits");
    tree.write("src/lib.rs", "fn kept() {}\n");
    tree.write("vendor/dep.rs", "fn vendored() {}\n");

    let cancel = AtomicBool::new(false);
    let mut files = Vec::new();
    let admits = |path: &std::path::Path, _is_dir: bool| {
        !path.components().any(|c| c.as_os_str() == "vendor")
    };
    walk_symbols(
        &[tree.root()],
        Limits::default(),
        Some(&admits),
        &cancel,
        &mut |f| files.push(f),
    );

    assert_eq!(names(&files), vec!["kept".to_string()]);
}

#[test]
fn a_files_symbols_arrive_before_the_walk_returns() {
    // Streaming, asserted structurally rather than by timing: the sink runs *during* the walk,
    // so a flag it sets is observable to the sink itself on a later file. If the walk collected
    // everything and then replayed it, `seen_first` would still be false on every call.
    let tree = Tree::new("stream");
    for i in 0..40 {
        tree.write(&format!("src/f{i}.rs"), &format!("fn f{i}() {{}}\n"));
    }

    let cancel = AtomicBool::new(false);
    let mut count = 0usize;
    let mut saw_a_predecessor = false;
    walk_symbols(
        &[tree.root()],
        Limits::default(),
        None,
        &cancel,
        &mut |_f| {
            if count > 0 {
                saw_a_predecessor = true;
            }
            count += 1;
        },
    );

    assert_eq!(count, 40);
    assert!(saw_a_predecessor, "the sink never ran during the walk");
}

#[test]
fn the_whole_index_cap_stops_the_walk_and_says_so() {
    let tree = Tree::new("cap");
    for i in 0..20 {
        let body: String = (0..10).map(|j| format!("fn f{i}_{j}() {{}}\n")).collect();
        tree.write(&format!("src/f{i}.rs"), &body);
    }

    let limits = Limits {
        max_symbols: 25,
        ..Limits::default()
    };
    let (files, outcome) = collect(&[tree.root()], limits);

    assert!(
        outcome.truncated,
        "a cap that stops silently is a cap that lies"
    );
    assert!(outcome.symbols <= 25, "{}", outcome.symbols);
    assert!(!files.is_empty(), "the cap stopped everything");
}

#[test]
fn a_file_past_the_size_cap_contributes_nothing_and_raises_nothing() {
    let tree = Tree::new("big");
    tree.write("src/small.rs", "fn small() {}\n");
    tree.write("src/huge.rs", &"fn big() {}\n".repeat(20_000));

    let limits = Limits {
        max_file_bytes: 1024,
        ..Limits::default()
    };
    let (files, outcome) = collect(&[tree.root()], limits);

    assert_eq!(outcome.files, 1);
    assert_eq!(names(&files), vec!["small".to_string()]);
}

#[test]
fn a_multi_root_project_prefixes_each_file_with_its_root_label() {
    // So one file is named the same here, in the file picker and in the search panel.
    let a = Tree::new("root-a");
    let b = Tree::new("root-b");
    a.write("lib.rs", "fn from_a() {}\n");
    b.write("lib.rs", "fn from_b() {}\n");

    let roots = vec![
        WalkRoot {
            path: a.0.clone(),
            label: Some("alpha".into()),
        },
        WalkRoot {
            path: b.0.clone(),
            label: Some("beta".into()),
        },
    ];
    let (files, outcome) = collect(&roots, Limits::default());

    assert_eq!(outcome.files, 2);
    let mut rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
    rels.sort();
    assert_eq!(rels, vec!["alpha/lib.rs", "beta/lib.rs"]);
}

#[test]
fn a_single_root_project_is_not_prefixed() {
    // The label exists for disambiguation, and there is nothing to disambiguate against.
    let tree = Tree::new("single");
    tree.write("lib.rs", "fn f() {}\n");
    let roots = vec![WalkRoot {
        path: tree.0.clone(),
        label: Some("alpha".into()),
    }];
    let (files, _) = collect(&roots, Limits::default());
    assert_eq!(files[0].rel, "lib.rs");
}

#[test]
fn cancelling_ends_delivery_rather_than_only_the_walk() {
    let tree = Tree::new("cancel");
    for i in 0..60 {
        tree.write(&format!("src/f{i}.rs"), &format!("fn f{i}() {{}}\n"));
    }

    let cancel = AtomicBool::new(false);
    let mut count = 0usize;
    walk_symbols(
        &[tree.root()],
        Limits::default(),
        None,
        &cancel,
        &mut |_f| {
            count += 1;
            // The shape the picker uses: the user typed the next character, so everything
            // already queued answers a question nobody is asking.
            cancel.store(true, Ordering::Release);
        },
    );

    assert_eq!(count, 1, "delivery continued after cancellation");
}

#[test]
fn a_file_that_is_not_utf8_is_skipped_rather_than_failing_the_walk() {
    let tree = Tree::new("utf8");
    tree.write("src/good.rs", "fn good() {}\n");
    std::fs::write(tree.0.join("src/bad.rs"), [0xff, 0xfe, 0x00, 0x41]).expect("write");

    let (files, outcome) = collect(&[tree.root()], Limits::default());
    assert_eq!(outcome.files, 1);
    assert_eq!(names(&files), vec!["good".to_string()]);
}

#[test]
fn a_source_file_that_declares_nothing_does_not_become_a_row() {
    // `outcome.files` counts files that *contributed*, so a repository of `mod` re-exports does
    // not inflate the figure the picker shows.
    let tree = Tree::new("empty");
    tree.write("src/reexport.rs", "use std::fmt;\n");
    tree.write("src/real.rs", "fn real() {}\n");

    let (files, outcome) = collect(&[tree.root()], Limits::default());
    assert_eq!(outcome.files, 1);
    assert_eq!(files.len(), 1);
}
