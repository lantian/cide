//! `git stash`, kept separate from [the shelf](crate::shelf).
//!
//! These are two features, not one with two names, and IDEA ships both for the same reason:
//! a stash is a real git object that `git stash pop` in a terminal can find and that survives
//! cide being uninstalled; a shelf entry is a patch file that can hold part of a working tree.
//! Merging them would mean either losing partial stashing or writing something to `refs/stash`
//! that plain git does not understand.
//!
//! Thin by design: libgit2's stash API is git's, so there is nothing to add.

use std::path::Path;

use cide_ipc::git::{GitError, StashEntry};

use crate::{Result, Wrap, repo as repo_mod};

/// Stash the working tree. `include_untracked` matches `git stash -u`.
pub fn save(root: &Path, message: &str, include_untracked: bool) -> Result<String> {
    let mut repo = repo_mod::open(root)?;
    let signature = repo.signature().wrap()?;
    let mut flags = git2::StashFlags::empty();
    if include_untracked {
        flags |= git2::StashFlags::INCLUDE_UNTRACKED;
    }
    // `KEEP_INDEX` is deliberately not offered. In changelist mode the index is a derived
    // artifact, so "keep the index" describes a state cide would rebuild on the next commit
    // anyway — a control that appears to do something and does not.
    let oid = repo
        .stash_save2(&signature, Some(message), Some(flags))
        .wrap()?;
    Ok(oid.to_string())
}

pub fn list(root: &Path) -> Result<Vec<StashEntry>> {
    let mut repo = repo_mod::open(root)?;
    let mut out = Vec::new();
    repo.stash_foreach(|index, message, oid| {
        out.push(StashEntry {
            index: index as u32,
            message: message.to_string(),
            oid: oid.to_string(),
        });
        true
    })
    .wrap()?;
    Ok(out)
}

pub fn pop(root: &Path, index: u32) -> Result<()> {
    let mut repo = repo_mod::open(root)?;
    check_index(&mut repo, index)?;
    repo.stash_pop(index as usize, None).wrap()
}

pub fn apply(root: &Path, index: u32) -> Result<()> {
    let mut repo = repo_mod::open(root)?;
    check_index(&mut repo, index)?;
    repo.stash_apply(index as usize, None).wrap()
}

pub fn drop_entry(root: &Path, index: u32) -> Result<()> {
    let mut repo = repo_mod::open(root)?;
    check_index(&mut repo, index)?;
    repo.stash_drop(index as usize).wrap()
}

/// Reject an out-of-range index before libgit2 does.
///
/// libgit2 answers a bad index with a generic error; the panel needs to tell "that stash is
/// gone" — someone popped it in a terminal — apart from a repository that will not open.
fn check_index(repo: &mut git2::Repository, index: u32) -> Result<()> {
    let mut count = 0usize;
    repo.stash_foreach(|_, _, _| {
        count += 1;
        true
    })
    .wrap()?;
    if index as usize >= count {
        return Err(GitError::NoSuchStash { index });
    }
    Ok(())
}
