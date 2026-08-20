//! Creating tags from a log row. (M19)
//!
//! # Two kinds, chosen by which value is present
//!
//! [`TagRequest::message`] is `None` for a lightweight tag — a ref and nothing else — and
//! `Some` for an annotated one, which is a real object with a tagger and a date. The
//! distinction is not cosmetic: `git describe` and most release tooling ignore lightweight
//! tags, so a release marked with the wrong kind is a release the build cannot name. Expressing
//! it as *which field is present* rather than as an `annotated: bool` beside an optional
//! message removes the state where the two disagree.
//!
//! # Allowed while an operation is in progress, deliberately
//!
//! [`crate::reset`], [`crate::replay`] and [`crate::branch::checkout_detached`] all refuse when
//! [`crate::repo::operation_in_progress`] says something is half-finished, because they move
//! `HEAD`, the index or the working tree. This writes **one ref** and touches none of those,
//! `git tag` works perfectly well in the middle of a rebase, and marking the commit you are
//! standing on before you continue is a normal thing to want. The asymmetry is pinned by
//! `a_tag_can_still_be_created_during_a_rebase` in `tests/commit_actions.rs`, so a later
//! tidy-up that made the guards uniform would fail rather than silently take the gesture away.
//!
//! # Pushing the tag is not here
//!
//! Exactly [`crate::branch::delete`]'s argument about deleting a branch on the remote: a push
//! of `refs/tags/<name>` is not undoable by the person who ran it, and it must not ride along
//! on a control labelled *Create tag*. It belongs behind a gesture that says *Push tag to
//! origin* and can be refused on its own terms.

use std::path::Path;

use cide_ipc::git::GitError;
use cide_ipc::history::{TagOutcome, TagRequest};
use git2::{Commit, Repository};

use crate::{Result, Wrap, repo as repo_mod};

/// Create (or, with [`TagRequest::force`], move) a tag.
pub fn create(root: &Path, request: &TagRequest) -> Result<TagOutcome> {
    let repo = repo_mod::open(root)?;
    // No `operation_in_progress` check. See the module header — this is the deliberate half of
    // the asymmetry, not an omission.

    validate_name(&request.name)?;
    let target = resolve(&repo, &request.target)?;

    let full = format!("refs/tags/{}", request.name);
    let existing = repo.find_reference(&full).ok();
    if let Some(reference) = &existing
        && !request.force
    {
        /*
         * The refusal carries **where the tag points now**, not just that the name is taken.
         *
         * A tag is a published promise about which commit a release is, and the question the
         * user is actually being asked is "is the thing already called v1.2.0 the same thing
         * you meant?". "That name is in use" cannot be answered; "v1.2.0 is a1b2c3d4, two
         * commits behind this one" can. `force` is the second, confirmed click after reading
         * that sentence.
         *
         * Peeled to a commit so an annotated tag reports the commit rather than the tag
         * object, which is the oid the log row beside it is showing.
         */
        let oid = reference
            .peel_to_commit()
            .map(|commit| commit.id().to_string())
            .or_else(|_| reference.target().map(|oid| oid.to_string()).ok_or(()))
            .unwrap_or_default();
        return Err(GitError::TagExists {
            name: request.name.clone(),
            oid,
        });
    }
    let moved = existing.is_some();
    drop(existing);

    let annotated = request.message.is_some();
    match &request.message {
        Some(message) => {
            // An annotated tag needs a tagger, which is `user.name`/`user.email`. A repository
            // with neither configured fails here rather than writing an object signed by
            // nobody, and that failure is git's own.
            let tagger = repo.signature().wrap()?;
            repo.tag(
                &request.name,
                target.as_object(),
                &tagger,
                message,
                request.force,
            )
            .wrap()?;
        }
        None => {
            repo.tag_lightweight(&request.name, target.as_object(), request.force)
                .wrap()?;
        }
    }

    Ok(TagOutcome {
        name: request.name.clone(),
        oid: target.id().to_string(),
        annotated,
        moved,
    })
}

/// Reject a name libgit2 would reject, before libgit2 sees it.
///
/// The twin of [`crate::branch`]'s `validate_name`, and it exists for the same reason: libgit2
/// answers a bad tag name with a generic reference error, and the dialog has an input box that
/// wants to say *"tag names cannot contain a space"* while the user is still typing.
///
/// Both halves are checked. [`git2::Reference::is_valid_name`] is `git check-ref-format`, and
/// on top of it come the exclusions [`git2::Repository::tag`] documents in its own
/// contract — `~ ^ : ? [ * \`, the sequences `..` and `@{`, and a trailing `.lock` — which are
/// the characters revparse gives a second meaning to. They overlap heavily; checking both is
/// cheap and means a change on either side cannot open a hole.
fn validate_name(name: &str) -> Result<()> {
    let refused = || GitError::InvalidTagName {
        name: name.to_string(),
    };
    if name.trim().is_empty() {
        return Err(refused());
    }
    const FORBIDDEN: &[char] = &['~', '^', ':', '?', '[', '*', '\\'];
    if name.contains(FORBIDDEN)
        || name.contains("..")
        || name.contains("@{")
        || name.ends_with(".lock")
    {
        return Err(refused());
    }
    if !git2::Reference::is_valid_name(&format!("refs/tags/{name}")) {
        return Err(refused());
    }
    Ok(())
}

/// The commit a tag will point at. `NoSuchCommit`, because the target comes from a log row.
fn resolve<'repo>(repo: &'repo Repository, rev: &str) -> Result<Commit<'repo>> {
    repo.revparse_single(rev)
        .ok()
        .and_then(|object| object.peel_to_commit().ok())
        .ok_or_else(|| GitError::NoSuchCommit {
            rev: rev.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names `git check-ref-format` rejects, pinned here so the dialog's complaint can be
    /// written and tested without a repository. The end-to-end half — that libgit2 agrees, and
    /// that a good name really does produce a tag — is in `tests/commit_actions.rs`.
    #[test]
    fn a_name_git_would_reject_is_refused_before_libgit2_sees_it() {
        for bad in [
            "",
            " ",
            "v 1.0",
            "a..b",
            "~x",
            "x.lock",
            "a@{b}",
            "f[o]o",
            "a^b",
            "a:b",
            "a?b",
            "a*b",
            "a\\b",
            "/leading",
            "trailing/",
        ] {
            assert!(
                validate_name(bad).is_err(),
                "{bad:?} should be refused as a tag name"
            );
        }
        for good in ["v1.2.0", "release/2026-01", "v1.2.0-rc.1", "a.lockfile"] {
            assert!(validate_name(good).is_ok(), "{good:?} is a legal tag name");
        }
    }
}
