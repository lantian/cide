//! Real temporary repositories, a deterministic generator, and the `git` binary.
//!
//! Everything here runs against **git 2.54.0 on this machine** — the property tests exist to
//! compare our patch synthesis against the real thing, so the reference implementation has to
//! be the real thing and not a second model of it.
//!
//! The test process is isolated before the first repository is created: `HOME` and
//! `XDG_STATE_HOME` are pointed at a scratch directory so neither the user's `~/.gitconfig`
//! (a global `core.autocrlf` would silently change every result) nor their real
//! `$XDG_STATE_HOME/cide/repos/` sidecars are touched.

#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::Once;

/// Point `HOME` and `XDG_STATE_HOME` at a scratch directory, once per test process.
///
/// libgit2 resolves its config search path from `HOME` when it first needs it, so this has to
/// happen before any repository is opened — which is why every constructor calls it.
pub fn isolate() -> PathBuf {
    static ONCE: Once = Once::new();
    let home = std::env::temp_dir().join(format!("cide-git-tests-{}", std::process::id()));
    ONCE.call_once(|| {
        std::fs::create_dir_all(home.join("state")).expect("scratch home");
        // SAFETY: called exactly once, before any test thread has spawned work that reads the
        // environment. There is no other way to redirect libgit2's config search path.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("XDG_STATE_HOME", home.join("state"));
            std::env::set_var("XDG_CONFIG_HOME", home.join("config"));
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        }
    });
    home
}

/// A repository in a temporary directory, removed when the handle drops.
pub struct TempRepo {
    pub root: PathBuf,
}

impl TempRepo {
    pub fn new(tag: &str) -> Self {
        isolate();
        let root = std::env::temp_dir().join(format!(
            "cide-git-{}-{tag}-{}",
            std::process::id(),
            next_id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp repo");
        let repo = Self { root };
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.name", "cide tests"]);
        repo.git(&["config", "user.email", "tests@cide.invalid"]);
        // Every generated file's bytes must survive a round trip through the index unchanged
        // unless a test asks otherwise, so the eol machinery is off by default and switched on
        // explicitly by the CRLF cases.
        repo.git(&["config", "core.autocrlf", "false"]);
        repo
    }

    pub fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("running git {args:?}: {e}"));
        assert!(
            output.status.success(),
            "git {args:?} failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Run git and hand back the exit status instead of asserting on it.
    pub fn try_git(&self, args: &[&str]) -> (bool, String) {
        self.try_git_stdin(args, &[])
    }

    pub fn try_git_stdin(&self, args: &[&str], stdin: &[u8]) -> (bool, String) {
        use std::io::Write;
        let mut child = Command::new("git")
            .current_dir(&self.root)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("spawning git {args:?}: {e}"));
        child
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(stdin)
            .expect("writing stdin");
        let output = child.wait_with_output().expect("git output");
        (
            output.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        )
    }

    pub fn write(&self, path: &str, bytes: &[u8]) {
        let full = self.root.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("parent dir");
        }
        std::fs::write(full, bytes).expect("write file");
    }

    pub fn read(&self, path: &str) -> Vec<u8> {
        std::fs::read(self.root.join(path)).expect("read file")
    }

    pub fn remove(&self, path: &str) {
        let _ = std::fs::remove_file(self.root.join(path));
    }

    pub fn commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
    }

    /// `git ls-files --stage`: mode, oid, stage and path for every index entry.
    ///
    /// This *is* the index for comparison purposes — everything else in the file is stat data
    /// that says nothing about what a commit would contain. Read with the `git` binary rather
    /// than with libgit2 so the comparison is not made by the library under test.
    pub fn index_state(&self) -> String {
        self.git(&["ls-files", "--stage"])
    }

    pub fn index_path(&self) -> PathBuf {
        self.root.join(".git/index")
    }

    /// Snapshot `.git/index` so two applications of the same patch start from one state.
    pub fn save_index(&self) -> Vec<u8> {
        std::fs::read(self.index_path()).unwrap_or_default()
    }

    pub fn restore_index(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            let _ = std::fs::remove_file(self.index_path());
        } else {
            std::fs::write(self.index_path(), bytes).expect("restore index");
        }
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        // Leave the directory behind when a test is failing, so the repository that produced
        // a bad patch can be inspected by hand. That is worth more than a tidy /tmp.
        if std::thread::panicking() {
            eprintln!(
                "[cide-git tests] leaving {} for inspection",
                self.root.display()
            );
            return;
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

// --- generation ---------------------------------------------------------------------------

/// SplitMix64.
///
/// A named, seeded generator rather than a crate: the seed is printed on failure, so a bad
/// case is reproduced by re-running with the same number instead of by re-rolling until it
/// happens again.
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(1))
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }

    pub fn chance(&mut self, numerator: u32, denominator: u32) -> bool {
        (self.next() % u64::from(denominator)) < u64::from(numerator)
    }

    pub fn flip(&mut self) -> bool {
        self.next() & 1 == 1
    }
}

/// Line terminator style for a generated file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eol {
    Lf,
    Crlf,
}

impl Eol {
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Eol::Lf => b"\n",
            Eol::Crlf => b"\r\n",
        }
    }
}

/// A generated text file: `count` lines, optionally without a trailing terminator.
pub fn text(rng: &mut Rng, count: usize, eol: Eol, trailing: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for index in 0..count {
        let word = WORDS[rng.below(WORDS.len())];
        out.extend_from_slice(format!("{index:03} {word}").as_bytes());
        if index + 1 < count || trailing {
            out.extend_from_slice(eol.bytes());
        }
    }
    out
}

/// Randomly insert, delete and rewrite lines. The result is a plausible edit, not noise:
/// runs of untouched lines are what produce hunks with context, and a diff with no context is
/// not the diff the staging code will meet in practice.
pub fn mutate(rng: &mut Rng, original: &[u8], eol: Eol, trailing: bool) -> Vec<u8> {
    let terminator = std::str::from_utf8(eol.bytes()).unwrap();
    let text = String::from_utf8_lossy(original);
    let mut lines: Vec<String> = text.split(terminator).map(str::to_owned).collect();
    // `split` on a terminated file leaves an empty tail; drop it and re-add on join.
    if trailing {
        lines.pop();
    }

    let edits = 1 + rng.below(4);
    for _ in 0..edits {
        if lines.is_empty() {
            lines.push("new line".into());
            continue;
        }
        let at = rng.below(lines.len());
        match rng.below(3) {
            0 => {
                lines.insert(at, format!("+++ {}", WORDS[rng.below(WORDS.len())]));
            }
            1 => {
                lines.remove(at);
            }
            _ => {
                lines[at] = format!("~~~ {}", WORDS[rng.below(WORDS.len())]);
            }
        }
    }

    let mut out = lines.join(terminator).into_bytes();
    if trailing && !out.is_empty() {
        out.extend_from_slice(eol.bytes());
    }
    out
}

const WORDS: &[&str] = &[
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet",
    "kilo", "lima", "mike", "november",
];

/// Bytes that libgit2 and git both call binary: a NUL in the first 8000.
pub fn binary_blob(rng: &mut Rng, len: usize) -> Vec<u8> {
    (0..len)
        .map(|_| (rng.next() % 256) as u8)
        .collect::<Vec<u8>>()
}

/// A fresh clone of `from`, on `main`, tracking `origin/main`.
///
/// Hoisted out of `branches.rs` when `pull.rs` needed the same three lines. A *local* remote on
/// purpose: `cide_git::push::route` sends those to libgit2, so the tests exercise the route
/// that needs no credentials and no network — which is the whole reason that route was kept.
pub fn clone_of(from: &TempRepo, tag: &str) -> TempRepo {
    let work = TempRepo::new(tag);
    work.git(&[
        "remote",
        "add",
        "origin",
        from.root.to_str().expect("utf-8 path"),
    ]);
    work.git(&["fetch", "-q", "origin"]);
    work.git(&["checkout", "-q", "-b", "main", "origin/main"]);
    work
}

/// Every tracked file's path and contents, hashed into one comparable string.
///
/// The oracle for *"the working tree is byte-identical"*, which is what every refusal in
/// `pull.rs` has to promise. `git status --porcelain` alone is not enough: it says a file is
/// modified, not what it now holds, so a refusal that rewrote a file and left it modified
/// would pass.
pub fn worktree_hash(repo: &TempRepo) -> String {
    let mut out = Vec::new();
    walk(&repo.root, &repo.root, &mut out);
    out.sort();
    out.join("\n")
}

fn walk(root: &std::path::Path, at: &std::path::Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(at) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == ".git") {
            continue;
        }
        if path.is_dir() {
            walk(root, &path, out);
        } else if let Ok(bytes) = std::fs::read(&path) {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            out.push(format!("{} {:x?}", rel.display(), simple_hash(&bytes)));
        }
    }
}

fn simple_hash(bytes: &[u8]) -> u64 {
    // FNV-1a. Not cryptographic and does not need to be: this compares a tree against itself a
    // few milliseconds later, and the only adversary is a bug.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// Assert that no sequencer state exists — by `RepositoryState` **and** by the files.
///
/// Both, because they can disagree: libgit2 reads the state from the files, so a stale
/// `MERGE_HEAD` that `cleanup_state` failed to remove would make one of them wrong and the
/// other right, and it is exactly the kind of leftover that locks every other action in
/// `cide-git` until somebody runs a `git` command by hand.
pub fn assert_no_sequencer_state(repo: &TempRepo, when: &str) {
    let git_dir = repo.root.join(".git");
    for name in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "REBASE_HEAD",
        "rebase-merge",
        "rebase-apply",
    ] {
        assert!(
            !git_dir.join(name).exists(),
            "{when}: .git/{name} was left behind"
        );
    }
    let state = git2::Repository::open(&repo.root).expect("open").state();
    assert_eq!(
        state,
        git2::RepositoryState::Clean,
        "{when}: repository state is not clean"
    );
}
