//! The content search over a real directory tree: what it skips, what it streams, and that
//! it makes the same ignore decision the file tree does.
//!
//! The unit tests in `content.rs` cover the matching semantics against strings. Everything
//! here needs inodes: a `.gitignore` only means something to a walk, "binary" is a property
//! of a file rather than of a `&str`, and the streaming claim is about the *order* work
//! happens in.
//!
//! # How the streaming claim is asserted
//!
//! Not by a clock. `cide-fs/tests/large_repo.rs` makes the same point about the picker and
//! explains why timing is the wrong instrument: a machine under load turns a real ordering
//! property into a flaky one. The structural version here is
//! [`a_search_streams_its_first_hits_while_the_walk_is_still_running`], which cancels *from
//! inside the sink* on the first batch. An implementation that collected every hit and called
//! the sink once at the end cannot pass it — by the time its sink runs there is no walk left
//! to stop, so it reports every hit in the corpus instead of one file's worth.
//!
//! Nothing here writes outside `cide_fs::testing::scratch`, which removes itself on drop
//! including while unwinding.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use cide_fs::testing::{Scratch, scratch};
use cide_ipc::{SearchHit, SearchMode, SearchQuery};
use cide_search::content::{
    Globs, Limits, Outcome, Search, SearchRoot, compile, compile_globs, resolve_scope, within_scope,
};

/// The string every corpus below is searched for. Chosen so it cannot occur by accident in a
/// path, a `.gitignore` or the filler text.
const NEEDLE: &str = "zqneedle";

fn literal(pattern: &str) -> SearchQuery {
    SearchQuery {
        pattern: pattern.to_string(),
        mode: SearchMode::Literal,
        case_sensitive: true,
        whole_word: false,
        scope: String::new(),
        include: String::new(),
    }
}

/// Run a search to completion and return every hit in the order the sink saw them.
fn run(roots: &[SearchRoot], query: &SearchQuery, limits: Limits) -> (Vec<SearchHit>, Outcome) {
    let regex = compile(query).expect("the test's own pattern must compile");
    let cancel = AtomicBool::new(false);
    let mut hits = Vec::new();
    let outcome = Search {
        roots,
        regex: &regex,
        limits,
        admits: None,
        include: None,
        cancel: &cancel,
        progress: None,
    }
    .run(&mut |batch| hits.extend_from_slice(batch));
    (hits, outcome)
}

/// The same, narrowed the way the panel narrows it: one folder, one glob list, or both.
///
/// Separate from [`run`] rather than adding two arguments to it, so the twenty call sites that
/// search everything keep reading as "search everything".
fn run_narrowed(
    roots: &[SearchRoot],
    query: &SearchQuery,
    scope: Option<&Path>,
    include: Option<&Globs>,
) -> (Vec<SearchHit>, Outcome) {
    let regex = compile(query).expect("the test's own pattern must compile");
    let cancel = AtomicBool::new(false);
    let mut hits = Vec::new();
    // The composition `cmd::search` performs: the shared ignore decision AND the scope. The
    // test does the same thing rather than passing the scope some other way, because getting
    // that composition wrong is the failure this is here to catch.
    let admits = |path: &Path, is_dir: bool| match scope {
        Some(scope) => within_scope(path, is_dir, scope),
        None => true,
    };
    let outcome = Search {
        roots,
        regex: &regex,
        limits: Limits::default(),
        admits: Some(&admits),
        include,
        cancel: &cancel,
        progress: None,
    }
    .run(&mut |batch| hits.extend_from_slice(batch));
    (hits, outcome)
}

fn write(path: PathBuf, contents: impl AsRef<[u8]>) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create a corpus directory");
    }
    std::fs::write(path, contents).expect("write a corpus file");
}

/// A small tree with one hit in `src`, and three files that must never be reported: one under
/// a gitignored directory, one under a dotted directory, and one binary.
fn corpus(tag: &str) -> Scratch {
    let dir = scratch(tag);
    write(dir.join(".gitignore"), "target/\n");
    write(
        dir.join("src/main.rs"),
        format!("fn main() {{ // {NEEDLE}\n}}\n"),
    );
    write(dir.join("src/lib.rs"), "fn lib() {}\n");
    write(
        dir.join("target/debug/build.log"),
        format!("{NEEDLE} in target\n"),
    );
    write(
        dir.join(".hidden/notes.md"),
        format!("{NEEDLE} in a dotfile dir\n"),
    );
    // A NUL in the head is what makes it binary; the needle is plain text after it, so a
    // search that skipped the check would find it.
    let mut binary = b"\x7fELF\0\0\0\0".to_vec();
    binary.extend_from_slice(format!("{NEEDLE} in a binary\n").as_bytes());
    write(dir.join("src/a.out"), binary);
    dir
}

#[test]
fn a_hit_carries_the_path_the_line_number_and_the_range() {
    let dir = corpus("search-basic");
    let roots = [SearchRoot::new(dir.path())];
    let (hits, outcome) = run(&roots, &literal(NEEDLE), Limits::default());

    assert_eq!(hits.len(), 1, "{hits:#?}");
    let hit = &hits[0];
    assert_eq!(hit.path, dir.join("src/main.rs").to_str().unwrap());
    assert_eq!(
        hit.rel, "src/main.rs",
        "a single-root project has no label prefix"
    );
    assert_eq!(hit.line, 1, "line numbers are 1-based");
    assert_eq!(
        &hit.text[hit.start as usize..hit.end as usize],
        NEEDLE,
        "the range indexes the shipped line"
    );
    assert_eq!(outcome.files, 1);
    assert_eq!(outcome.hits, 1);
    assert!(!outcome.truncated);
    // `src/main.rs` and `src/lib.rs`. The binary was opened but not scanned, and neither
    // excluded directory was descended into.
    assert_eq!(
        outcome.scanned, 2,
        "the skipped files are not counted as scanned"
    );
}

#[test]
fn an_ignored_directory_produces_no_hits() {
    let dir = corpus("search-ignored");
    let (hits, _) = run(
        &[SearchRoot::new(dir.path())],
        &literal(NEEDLE),
        Limits::default(),
    );
    for hit in &hits {
        assert!(
            !hit.rel.starts_with("target/") && !hit.rel.starts_with(".hidden/"),
            "a search that reports hits inside target/ is noise: {hit:?}"
        );
    }
    // And the file really is there — otherwise this passes on an empty tree.
    assert!(dir.join("target/debug/build.log").is_file());
}

#[test]
fn a_binary_file_is_skipped() {
    let dir = corpus("search-binary");
    let (hits, _) = run(
        &[SearchRoot::new(dir.path())],
        &literal(NEEDLE),
        Limits::default(),
    );
    assert!(
        hits.iter().all(|h| !h.rel.ends_with("a.out")),
        "a NUL in the head takes the file out: {hits:#?}"
    );
}

#[test]
fn a_file_past_the_size_cap_is_never_opened() {
    let dir = scratch("search-huge");
    write(dir.join("small.txt"), format!("{NEEDLE}\n"));
    write(
        dir.join("huge.txt"),
        format!("{NEEDLE}\n{}", "x".repeat(4096)),
    );
    let limits = Limits {
        max_file_bytes: 1024,
        ..Limits::default()
    };
    let (hits, _) = run(&[SearchRoot::new(dir.path())], &literal(NEEDLE), limits);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].rel, "small.txt");
}

/// The shared ignore decision, using the real [`cide_fs::Filter`] the file tree consults.
///
/// Both halves matter. The first is agreement: a `Filter` built over this root refuses
/// `target/`, and so does the search. The second is that the filter is consulted at all — a
/// closure that rejects everything must produce no hits, which distinguishes "the walk
/// happened to ignore the same thing" from "the filter was asked".
#[test]
fn the_shared_filter_is_what_decides_what_is_searched() {
    let dir = corpus("search-filter");
    let roots = [SearchRoot::new(dir.path())];
    let root_paths = vec![dir.to_path_buf()];
    let dirs = [dir.path(), &dir.join("src")];
    let filter = cide_fs::Filter::build(
        &root_paths,
        dirs.iter().copied(),
        cide_fs::Visibility::CONSERVATIVE,
    );

    let regex = compile(&literal(NEEDLE)).unwrap();
    let cancel = AtomicBool::new(false);
    let mut hits = Vec::new();
    let admits = |path: &Path, is_dir: bool| filter.admits(path, is_dir);
    Search {
        roots: &roots,
        regex: &regex,
        limits: Limits::default(),
        admits: Some(&admits),
        include: None,
        cancel: &cancel,
        progress: None,
    }
    .run(&mut |batch| hits.extend_from_slice(batch));
    assert_eq!(
        hits.len(),
        1,
        "the same one file the tree would show: {hits:#?}"
    );
    assert!(!filter.admits(&dir.join("target/debug/build.log"), false));

    // Now a decision that only the filter can make: nothing is admitted.
    let nothing = |_: &Path, _: bool| false;
    let cancel = AtomicBool::new(false);
    let mut none = Vec::new();
    let outcome = Search {
        roots: &roots,
        regex: &regex,
        limits: Limits::default(),
        admits: Some(&nothing),
        include: None,
        cancel: &cancel,
        progress: None,
    }
    .run(&mut |batch| none.extend_from_slice(batch));
    assert!(none.is_empty(), "the filter has the final say");
    assert_eq!(outcome.scanned, 0, "a rejected file is not even opened");
}

/// The filter is consulted on *files*, not only on the directories above them.
///
/// The `admits` closure here says yes to every directory, so the walk descends everywhere and
/// `ignore`'s own `.gitignore` handling has nothing to exclude — the only thing that can keep
/// `secret.txt` out of the results is the per-file question. Deleting that call site leaves
/// [`the_shared_filter_is_what_decides_what_is_searched`] green, because its "nothing is
/// admitted" closure rejects the root directory and the walk never reaches a file at all; a
/// review mutated the file-level check away and found nothing failed.
#[test]
fn the_filter_is_asked_about_every_file_and_not_only_about_directories() {
    let dir = scratch("search-file-filter");
    write(dir.join("keep.txt"), format!("{NEEDLE}\n"));
    write(dir.join("secret.txt"), format!("{NEEDLE}\n"));
    write(dir.join("sub/keep.txt"), format!("{NEEDLE}\n"));

    let asked = Mutex::new(Vec::<PathBuf>::new());
    let admits = |path: &Path, is_dir: bool| {
        asked.lock().unwrap().push(path.to_path_buf());
        // Every directory is fine; one file is not.
        is_dir || path.file_name().is_none_or(|n| n != "secret.txt")
    };

    let regex = compile(&literal(NEEDLE)).unwrap();
    let cancel = AtomicBool::new(false);
    let scanned = AtomicU32::new(0);
    let mut hits = Vec::new();
    Search {
        roots: &[SearchRoot::new(dir.path())],
        regex: &regex,
        limits: Limits::default(),
        admits: Some(&admits),
        include: None,
        cancel: &cancel,
        progress: Some(&scanned),
    }
    .run(&mut |batch| hits.extend_from_slice(batch));

    let mut rels: Vec<&str> = hits.iter().map(|h| h.rel.as_str()).collect();
    rels.sort_unstable();
    assert_eq!(
        rels,
        ["keep.txt", "sub/keep.txt"],
        "the refused file is the only one missing: {hits:#?}"
    );
    assert_eq!(scanned.load(Ordering::Acquire), 2, "it was never opened");
    let asked = asked.lock().unwrap();
    assert!(
        asked.iter().any(|p| p.ends_with("secret.txt")),
        "the filter has to have been asked about it: {asked:#?}"
    );
}

/// A refused directory is pruned, not walked and then rejected file by file.
///
/// The distinction is invisible in the results — both produce no hits — and it is the whole
/// difference between skipping `target/` and reading every file in it. Asserting it needs the
/// closure to record what it was asked, which is why it is a test rather than a comment.
#[test]
fn a_refused_directory_is_never_descended_into() {
    let dir = scratch("search-prune");
    write(dir.join("keep.txt"), format!("{NEEDLE}\n"));
    for f in 0..8 {
        write(dir.join(format!("heavy/f{f}.txt")), format!("{NEEDLE}\n"));
    }

    let asked = Mutex::new(Vec::<PathBuf>::new());
    let admits = |path: &Path, _is_dir: bool| {
        asked.lock().unwrap().push(path.to_path_buf());
        !path.ends_with("heavy")
    };

    let regex = compile(&literal(NEEDLE)).unwrap();
    let cancel = AtomicBool::new(false);
    let mut hits = Vec::new();
    Search {
        roots: &[SearchRoot::new(dir.path())],
        regex: &regex,
        limits: Limits::default(),
        admits: Some(&admits),
        include: None,
        cancel: &cancel,
        progress: None,
    }
    .run(&mut |batch| hits.extend_from_slice(batch));

    assert_eq!(hits.len(), 1, "only the file outside the pruned tree");
    let asked = asked.lock().unwrap();
    // `heavy` itself is asked about — that is the refusal. What must not appear is anything
    // *under* it, so the directory itself is excluded from the check by `p != heavy`.
    let heavy = dir.join("heavy");
    assert!(
        !asked.iter().any(|p| *p != heavy && p.starts_with(&heavy)),
        "the filter was asked about something inside a directory it had already refused, \
         which means the subtree was walked rather than pruned: {asked:#?}"
    );
}

#[test]
fn a_multi_root_project_labels_every_hit_with_its_root() {
    let dir = scratch("search-multi");
    write(dir.join("alpha/a.txt"), format!("{NEEDLE}\n"));
    write(dir.join("beta/b.txt"), format!("{NEEDLE}\n"));
    let roots = [
        SearchRoot::new(dir.join("alpha")),
        SearchRoot::new(dir.join("beta")),
    ];
    let (hits, outcome) = run(&roots, &literal(NEEDLE), Limits::default());
    let mut rels: Vec<&str> = hits.iter().map(|h| h.rel.as_str()).collect();
    rels.sort_unstable();
    assert_eq!(rels, ["alpha/a.txt", "beta/b.txt"]);
    assert_eq!(outcome.files, 2);
}

#[test]
fn the_total_cap_truncates_rather_than_growing_without_bound() {
    let dir = scratch("search-cap");
    for f in 0..20 {
        write(
            dir.join(format!("f{f}.txt")),
            format!("{NEEDLE}\n").repeat(10),
        );
    }
    let limits = Limits {
        max_hits: 25,
        ..Limits::default()
    };
    let (hits, outcome) = run(&[SearchRoot::new(dir.path())], &literal(NEEDLE), limits);
    assert_eq!(
        hits.len(),
        25,
        "exactly the cap, not the batch that crossed it"
    );
    assert_eq!(outcome.hits, 25);
    assert!(outcome.truncated, "the panel has to be able to say so");
}

/// `truncated` means "there was more", and a result set that exactly fills the cap had no
/// more.
///
/// The panel prints `truncated` as a `+` on the count, so the two halves here are two
/// different sentences to the user: exactly 25 hits and nothing else in the repository is
/// `25`, while 25 of at least 26 is `25+`. The cap test above cannot tell them apart — its
/// corpus has 200 hits, so it is truncated under either reading — and `batch.len() >= room`
/// gets the boundary case wrong in the direction that lies about a *complete* search.
#[test]
fn a_result_set_that_exactly_fills_the_cap_is_complete_and_not_truncated() {
    let dir = scratch("search-cap-exact");
    // Five files, five hits each, and no sixth file: the batch carrying the twenty-fifth hit
    // is the last one there is.
    for f in 0..5 {
        write(
            dir.join(format!("f{f}.txt")),
            format!("{NEEDLE}\n").repeat(5),
        );
    }
    let root = [SearchRoot::new(dir.path())];

    let limits = Limits {
        max_hits: 25,
        ..Limits::default()
    };
    let (hits, outcome) = run(&root, &literal(NEEDLE), limits);
    assert_eq!(hits.len(), 25, "every hit in the corpus");
    assert_eq!(outcome.hits, 25);
    assert_eq!(outcome.files, 5);
    assert!(
        !outcome.truncated,
        "a search that found all 25 hits there are reported itself as `25+`"
    );

    // One below, so the same corpus really does truncate — otherwise the assertion above
    // would also pass on a build that never sets the flag at all.
    let limits = Limits {
        max_hits: 24,
        ..Limits::default()
    };
    let (hits, outcome) = run(&root, &literal(NEEDLE), limits);
    assert_eq!(hits.len(), 24, "trimmed to the cap exactly");
    assert!(outcome.truncated, "and the twenty-fifth hit was dropped");
}

/// Cancellation from inside the sink, which is only possible if the sink runs during the
/// walk. See the module header for why this is the streaming assertion rather than a clock.
#[test]
fn a_search_streams_its_first_hits_while_the_walk_is_still_running() {
    let dir = scratch("search-stream");
    // Enough files that a walk of all of them is many batches, and enough bytes in each that
    // the walkers cannot race to the end of the tree while the first batch is in the channel.
    const FILES: usize = 400;
    let filler = "let x = 1;\n".repeat(400);
    for f in 0..FILES {
        write(
            dir.join(format!("pkg{f:04}/src/file.rs")),
            format!("{filler}// {NEEDLE}\n"),
        );
    }

    let regex = compile(&literal(NEEDLE)).unwrap();
    let cancel = AtomicBool::new(false);
    let batches = AtomicU32::new(0);
    let scanned = AtomicU32::new(0);
    let first_batch = Mutex::new(Vec::new());
    let outcome = Search {
        roots: &[SearchRoot::new(dir.path())],
        regex: &regex,
        limits: Limits::default(),
        admits: None,
        include: None,
        cancel: &cancel,
        progress: Some(&scanned),
    }
    .run(&mut |batch| {
        if batches.fetch_add(1, Ordering::Relaxed) == 0 {
            first_batch.lock().unwrap().extend_from_slice(batch);
            // The whole test. A run that had already finished walking by the time the sink
            // was first called has nothing left to stop here.
            cancel.store(true, Ordering::Release);
        }
    });

    let first = first_batch.lock().unwrap();
    assert_eq!(first.len(), 1, "one file's hits arrive as one batch");
    assert!(first[0].rel.ends_with("src/file.rs"));
    assert_eq!(
        outcome.hits, 1,
        "cancelling on the first batch stopped a walk that had {FILES} files of hits left; \
         an implementation that only called the sink after the walk would report all of them"
    );
    assert!(
        outcome.scanned < FILES as u32,
        "the walk stopped early: scanned {} of {FILES}",
        outcome.scanned
    );
}

// --- narrowing the search: one folder, one glob list -------------------------------------

/// A corpus with the needle in three different directories and three different extensions, so
/// a scope and a glob each have something to exclude that the other would not.
fn narrowing_corpus(tag: &str) -> Scratch {
    let dir = scratch(tag);
    write(dir.join("crates/git/src/log.rs"), format!("// {NEEDLE}\n"));
    write(dir.join("crates/git/Cargo.toml"), format!("# {NEEDLE}\n"));
    write(dir.join("crates/fs/src/index.rs"), format!("// {NEEDLE}\n"));
    write(dir.join("ui/src/App.tsx"), format!("// {NEEDLE}\n"));
    dir
}

#[test]
fn a_scope_searches_one_folder_and_leaves_the_rest_of_the_project_alone() {
    let dir = narrowing_corpus("search-scope-one-folder");
    let roots = vec![SearchRoot::new(dir.path())];
    let scope = dir.join("crates/git");

    let (hits, _) = run_narrowed(&roots, &literal(NEEDLE), Some(&scope), None);
    let mut rels: Vec<&str> = hits.iter().map(|h| h.rel.as_str()).collect();
    rels.sort_unstable();
    assert_eq!(rels, ["crates/git/Cargo.toml", "crates/git/src/log.rs"]);
}

/// The heading a scoped hit draws is still measured from the **root**, not from the scope.
///
/// This is the whole reason scoping is a filter rather than a narrower root list. Re-basing
/// `rel` on the scope would make one file read `src/log.rs` here and `crates/git/src/log.rs`
/// in the file picker, which is how two surfaces stop agreeing about what a file is called.
#[test]
fn a_scoped_hit_is_still_named_relative_to_its_root() {
    let dir = narrowing_corpus("search-scope-rel");
    let roots = vec![SearchRoot::new(dir.path())];
    let scope = dir.join("crates/git/src");

    let (hits, _) = run_narrowed(&roots, &literal(NEEDLE), Some(&scope), None);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].rel, "crates/git/src/log.rs");
}

/// A scope prunes: the subtree it excludes is skipped at its top, not read file by file.
///
/// Invisible in the results — both spellings report the same two hits — and it is the whole
/// difference between narrowing a search and merely filtering its output. `scanned` counts
/// files actually opened, so it is the instrument.
#[test]
fn a_scope_prunes_rather_than_filtering_what_it_has_already_read() {
    let dir = narrowing_corpus("search-scope-prunes");
    let roots = vec![SearchRoot::new(dir.path())];

    let (_, whole) = run_narrowed(&roots, &literal(NEEDLE), None, None);
    let (_, scoped) = run_narrowed(
        &roots,
        &literal(NEEDLE),
        Some(&dir.join("crates/git")),
        None,
    );
    assert_eq!(whole.scanned, 4);
    assert_eq!(scoped.scanned, 2);
}

#[test]
fn a_glob_keeps_only_the_files_it_names_and_still_descends_everywhere() {
    let dir = narrowing_corpus("search-glob-extension");
    let roots = vec![SearchRoot::new(dir.path())];
    let globs = compile_globs("*.rs")
        .expect("a plain extension glob")
        .expect("not empty");

    let (hits, _) = run_narrowed(&roots, &literal(NEEDLE), None, Some(&globs));
    let mut rels: Vec<&str> = hits.iter().map(|h| h.rel.as_str()).collect();
    rels.sort_unstable();
    // Both of them, from two different directories — the point being that a whitelist glob
    // must not prune the directories it does not itself match. `ignore`'s `!is_dir` guard is
    // what makes that true, and this is the test that notices if it ever stops being.
    assert_eq!(rels, ["crates/fs/src/index.rs", "crates/git/src/log.rs"]);
}

#[test]
fn a_negated_glob_excludes_instead_of_including() {
    let dir = narrowing_corpus("search-glob-negated");
    let roots = vec![SearchRoot::new(dir.path())];
    let globs = compile_globs("!*.toml")
        .expect("a negation")
        .expect("not empty");

    let (hits, _) = run_narrowed(&roots, &literal(NEEDLE), None, Some(&globs));
    assert!(
        hits.iter().all(|h| !h.rel.ends_with(".toml")),
        "the .toml was searched anyway: {:?}",
        hits.iter().map(|h| &h.rel).collect::<Vec<_>>()
    );
    assert_eq!(hits.len(), 3);
}

#[test]
fn a_scope_and_a_glob_compose() {
    let dir = narrowing_corpus("search-scope-and-glob");
    let roots = vec![SearchRoot::new(dir.path())];
    let globs = compile_globs("*.rs")
        .expect("an extension glob")
        .expect("not empty");

    let (hits, _) = run_narrowed(
        &roots,
        &literal(NEEDLE),
        Some(&dir.join("crates/git")),
        Some(&globs),
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].rel, "crates/git/src/log.rs");
}

// --- resolving what the user typed into a directory ----------------------------------------

#[test]
fn a_scope_resolves_relative_to_a_root_and_absolutely() {
    let dir = narrowing_corpus("search-resolve-basics");
    let roots = vec![SearchRoot::new(dir.path())];
    let want = dir.join("crates/git");

    assert_eq!(resolve_scope(&roots, "crates/git"), Ok(want.clone()));
    // A trailing separator is what a path copied out of a shell carries.
    assert_eq!(resolve_scope(&roots, "crates/git/"), Ok(want.clone()));
    assert_eq!(resolve_scope(&roots, "  crates/git  "), Ok(want.clone()));
    assert_eq!(
        resolve_scope(&roots, want.to_str().expect("utf-8 scratch path")),
        Ok(want)
    );
}

/// A scope naming a **file** is that file's directory.
///
/// The rule behind "…or if a file is selected, its parent". It lives in Rust so that the
/// frontend never has to guess from a tree row that may have been scrolled out of the row
/// cache, and so that a path pasted out of *Copy Relative Path* does something sensible.
#[test]
fn a_scope_naming_a_file_resolves_to_its_directory() {
    let dir = narrowing_corpus("search-resolve-file");
    let roots = vec![SearchRoot::new(dir.path())];
    assert_eq!(
        resolve_scope(&roots, "crates/git/src/log.rs"),
        Ok(dir.join("crates/git/src"))
    );
}

#[test]
fn a_multi_root_scope_may_name_its_root_by_label() {
    let one = narrowing_corpus("search-resolve-multi-a");
    let two = narrowing_corpus("search-resolve-multi-b");
    let roots = vec![
        SearchRoot {
            path: one.path().to_path_buf(),
            label: "one".to_string(),
        },
        SearchRoot {
            path: two.path().to_path_buf(),
            label: "two".to_string(),
        },
    ];

    // The label alone is the root; the label as a prefix selects which root a shared relative
    // path means. Without the second, `crates/git` in a two-root project would always resolve
    // to the first root and silently search the wrong tree.
    assert_eq!(resolve_scope(&roots, "two"), Ok(two.path().to_path_buf()));
    assert_eq!(
        resolve_scope(&roots, "two/crates/git"),
        Ok(two.join("crates/git"))
    );
    assert_eq!(
        resolve_scope(&roots, "crates/git"),
        Ok(one.join("crates/git"))
    );
}

#[test]
fn a_scope_that_names_nothing_is_a_sentence_rather_than_an_empty_result() {
    let dir = narrowing_corpus("search-resolve-missing");
    let roots = vec![SearchRoot::new(dir.path())];

    let err = resolve_scope(&roots, "crates/nope").expect_err("no such directory");
    assert!(err.contains("crates/nope"), "{err}");

    // A real directory outside every root is the other half: the walk starts at roots, so a
    // scope outside them could never produce a hit and answering "no results" would be a dead
    // end the user cannot diagnose.
    let outside = scratch("search-resolve-outside");
    let err = resolve_scope(&roots, outside.path().to_str().expect("utf-8"))
        .expect_err("outside every root");
    assert!(err.contains("outside this project"), "{err}");
}

/// The literal reading of a relative scope beats the label-prefix reading.
///
/// A repository with a directory named after itself is the case, and it is not exotic —
/// `cide/cide/src`, `foo/foo/src`, every Go module laid out that way. Under the other order
/// `cide/src` resolves to `<root>/src` and searches a tree the user did not name, which is a
/// wrong answer that looks like a right one.
#[test]
fn a_literal_relative_scope_beats_the_root_label_reading() {
    let dir = scratch("search-resolve-selfnamed");
    let label = dir
        .path()
        .file_name()
        .and_then(|n| n.to_str())
        .expect("a utf-8 scratch name")
        .to_string();
    write(dir.join("src/a.rs"), "");
    write(dir.join(format!("{label}/src/b.rs")), "");
    let roots = vec![SearchRoot::new(dir.path())];

    assert_eq!(
        resolve_scope(&roots, &format!("{label}/src")),
        Ok(dir.join(format!("{label}/src"))),
    );
    // And with no such literal directory, the prefix reading is still there as the fallback.
    assert_eq!(
        resolve_scope(&roots, &format!("{label}/src/a.rs")),
        Ok(dir.join("src")),
    );
    // The bare label is the root itself, which is what the tree's top-level row is called.
    assert_eq!(resolve_scope(&roots, &label), Ok(dir.path().to_path_buf()));
}
