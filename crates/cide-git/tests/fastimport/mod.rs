//! Building exact histories with `git fast-import`.
//!
//! # Why not just run `git commit`
//!
//! Three reasons, and each of them was a problem before this existed.
//!
//! **Cost.** One process per commit puts a 250-commit paging test and a 4000-file commit out of
//! reach; one process for the whole history puts them at a few milliseconds.
//!
//! **The clock.** Every commit here gets its own second, because `git log` breaks a
//! committer-time tie by its own insertion order while our heap breaks it by oid — so a
//! differential test over commits written inside one second would be comparing two tie-break
//! conventions rather than two walks. `git commit` takes its timestamp from the wall clock and
//! from the environment; `fast-import` takes it from the stream.
//!
//! **Merges whose tree is neither parent's.** An *evil merge* — a change that exists on no side —
//! is the one merge a path's history has to show, and driving `git merge` into one means
//! provoking a conflict and resolving it by hand, which is fragile and unreadable. fast-import
//! builds a commit's tree from its **first** parent and then applies the operations, so the tree
//! is whatever the fixture says it is.
//!
//! It is still the real `git` binary writing real objects into a real object database, which is
//! the property that matters: these tests compare our walk against `git log`, so the fixture must
//! not be a second model of git.

#![allow(dead_code)]

use crate::support::TempRepo;

/// 2020-09-13T12:26:40Z. Nothing depends on the value beyond its being comfortably in the past
/// and far from a 32-bit boundary.
pub const EPOCH: i64 = 1_600_000_000;

/// One file operation inside a commit.
pub enum Op<'a> {
    /// mode (`100644`, `100755`, `120000`), path, contents.
    Set(&'a str, &'a str, &'a str),
    Delete(&'a str),
}

/// A `git fast-import` stream under construction.
pub struct Import {
    text: Vec<u8>,
    marks: usize,
}

impl Default for Import {
    fn default() -> Self {
        Self::new()
    }
}

impl Import {
    pub fn new() -> Self {
        Self {
            text: Vec::new(),
            marks: 0,
        }
    }

    /// Append one commit and return its mark.
    ///
    /// `seconds` is an offset from [`EPOCH`], and it is the caller's job to keep them distinct —
    /// see the module header.
    pub fn commit(
        &mut self,
        branch: &str,
        seconds: i64,
        message: &str,
        from: Option<usize>,
        merge: Option<usize>,
        ops: &[Op<'_>],
    ) -> usize {
        self.marks += 1;
        let mark = self.marks;
        let when = EPOCH + seconds;
        self.push(&format!("commit refs/heads/{branch}\nmark :{mark}\n"));
        self.push(&format!(
            "author cide tests <tests@cide.invalid> {when} +0000\n"
        ));
        self.push(&format!(
            "committer cide tests <tests@cide.invalid> {when} +0000\n"
        ));
        self.data(message.as_bytes());
        if let Some(from) = from {
            self.push(&format!("from :{from}\n"));
        }
        if let Some(merge) = merge {
            self.push(&format!("merge :{merge}\n"));
        }
        for op in ops {
            match op {
                Op::Set(mode, path, body) => {
                    self.push(&format!("M {mode} inline {path}\n"));
                    self.data(body.as_bytes());
                }
                Op::Delete(path) => self.push(&format!("D {path}\n")),
            }
        }
        self.push("\n");
        mark
    }

    /// The same, with raw bytes for one file — for the binary cases, which are the whole point of
    /// `LineCount::Binary` existing.
    pub fn commit_bytes(
        &mut self,
        branch: &str,
        seconds: i64,
        message: &str,
        from: Option<usize>,
        files: &[(&str, &[u8])],
    ) -> usize {
        self.marks += 1;
        let mark = self.marks;
        let when = EPOCH + seconds;
        self.push(&format!("commit refs/heads/{branch}\nmark :{mark}\n"));
        self.push(&format!(
            "author cide tests <tests@cide.invalid> {when} +0000\n"
        ));
        self.push(&format!(
            "committer cide tests <tests@cide.invalid> {when} +0000\n"
        ));
        self.data(message.as_bytes());
        if let Some(from) = from {
            self.push(&format!("from :{from}\n"));
        }
        for (path, body) in files {
            self.push(&format!("M 100644 inline {path}\n"));
            self.data(body);
        }
        self.push("\n");
        mark
    }

    /// Point another branch at an existing mark, so `a..b` has two names to work with.
    pub fn reset(&mut self, branch: &str, mark: usize) {
        self.push(&format!("reset refs/heads/{branch}\nfrom :{mark}\n\n"));
    }

    pub fn run(self, repo: &TempRepo) {
        let (ok, out) = repo.try_git_stdin(&["fast-import", "--quiet", "--force"], &self.text);
        assert!(ok, "fast-import failed:\n{out}");
    }

    fn push(&mut self, text: &str) {
        self.text.extend_from_slice(text.as_bytes());
    }

    /// fast-import's counted-payload form. The length is in **bytes** and a miscount is silently
    /// catastrophic — the stream resynchronises mid-command and reports something like
    /// `unsupported command: m :2` many lines later — so it is computed here and never by hand.
    fn data(&mut self, body: &[u8]) {
        self.push(&format!("data {}\n", body.len()));
        self.text.extend_from_slice(body);
        self.push("\n");
    }
}
