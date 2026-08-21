//! Resolving a conflict without leaving cide. (M20)
//!
//! # What this module is for
//!
//! Until M20 the sentence *"cide has no conflict-resolution surface"* appeared in six places in
//! this workspace, and every operation that could conflict was written around it: `pull`
//! fast-forwarded or refused, `replay` composed cherry-picks in memory and refused, and
//! `worktree::integrate` did the same for the agent merge. This module is the surface those
//! refusals were waiting for, and [`crate::pull`]'s header carries the argument for why the
//! operations now write **real git state** — `MERGE_HEAD`, `.git/rebase-merge`, an index with
//! stages 1/2/3 — rather than a private model.
//!
//! # The three stages, and why every side is optional
//!
//! A conflicted index holds up to three entries per path: stage 1 the merge base, stage 2
//! ours, stage 3 theirs. Any of them can be absent — a delete/modify conflict has no `ours`,
//! its mirror has no `theirs`, and a file added independently on both sides has no base.
//! [`crate::repo::conflicts_of`] already handles that for *paths*; [`read`] handles it for
//! *content*, and the distinction matters to the user: a pane rendering a missing side as an
//! empty document says *"the other branch deleted every line"*, which is a different fact from
//! *"the other branch deleted the file"*, and the difference decides what they click.
//!
//! # What "resolved" means, and where it is remembered
//!
//! git does not record it. Resolving a path *is* collapsing its three stages into one, and
//! afterwards nothing in `.git` distinguishes a resolved file from any other staged one — the
//! index of a clean merge stages every merged file the same way.
//!
//! So the set is remembered in the repository's sidecar, snapshotted **lazily**: the first time
//! [`state`] is asked while an operation is in progress, whatever is conflicted right then
//! becomes the operation's roster. `resolved` is then `roster − still conflicted`, which is a
//! definition that costs nothing and stays true across a restart.
//!
//! Lazily rather than at the point the merge is started, because the merge is not always
//! started by cide: `git merge` typed into a pane is the ordinary case in this app, and a
//! roster written only by our own entry points would leave that conflict with no resolved
//! column at all. The cost of the lazy version is exactly its honest limit — a file resolved
//! in a terminal *before* cide first looked is simply not in the roster, so it is not listed
//! as resolved rather than being listed wrongly.

use std::path::Path;

use cide_ipc::git::{
    ConflictEntry, ConflictFile, ConflictSide, ContinueOutcome, GitError, MergeState, MergeStep,
};
use git2::{Index, IndexEntry, Oid, Repository};
use serde::{Deserialize, Serialize};

use crate::{Result, Wrap, changelist, commit, repo as repo_mod, sidecar, status};

/// The largest side a text resolver will load, in bytes.
///
/// Matched to the diff pane's own `MAX_DIFF_CHARS`, and for the same reason it has one: three
/// CodeMirror documents laid out side by side force `height: auto`, which defeats the viewport
/// windowing that makes a large file survivable at all. Over this the row still offers
/// *Yours* and *Theirs*, which are the two answers that need no editor.
pub const MAX_SIDE_BYTES: u64 = 1024 * 1024;

// --- the roster ------------------------------------------------------------------------------

/// The paths one in-progress operation started with.
///
/// Keyed by the operation's name so that finishing a merge and starting a rebase does not
/// inherit the merge's roster. Not keyed by anything finer: git offers no operation id, and a
/// second merge cannot begin before the first ends.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Roster {
    operation: String,
    paths: Vec<String>,
}

fn roster_path(root: &Path) -> std::path::PathBuf {
    sidecar::repo_dir(root).join("conflicts.json")
}

fn read_roster(root: &Path) -> Option<Roster> {
    let bytes = sidecar::read_opt(&roster_path(root)).ok().flatten()?;
    serde_json::from_slice(&bytes).ok()
}

/// Snapshot the roster if this operation has none yet, and return it.
///
/// Best-effort on the way in and on the way out: a sidecar that cannot be read or written costs
/// the resolved column and nothing else, and refusing to *show* a conflict because a
/// convenience file is unwritable would be the wrong trade.
fn roster_for(root: &Path, operation: &str, current: &[String]) -> Vec<String> {
    if let Some(existing) = read_roster(root)
        && existing.operation == operation
    {
        return existing.paths;
    }
    let fresh = Roster {
        operation: operation.to_string(),
        paths: current.to_vec(),
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(&fresh) {
        let _ = sidecar::write_atomic(&roster_path(root), &bytes);
    }
    fresh.paths
}

fn clear_roster(root: &Path) {
    let _ = sidecar::remove(&roster_path(root));
}

/// Take the roster now, if this operation has none.
///
/// Called at the top of every mutating verb as well as by [`state`], and that is not
/// belt-and-braces: resolving is what *removes* a path from the conflicted set, so a roster
/// first taken afterwards is empty and the panel would show no rows at all — including the one
/// the user just resolved. The snapshot has to happen before the first resolution, and the only
/// way to guarantee that without knowing which call comes first is for all of them to ensure it.
fn ensure_roster(root: &Path, repo: &Repository) {
    let Some(operation) = repo_mod::operation_in_progress(repo) else {
        return;
    };
    let Ok(index) = repo.index() else { return };
    let Ok(conflicted) = repo_mod::conflicts_of(&index) else {
        return;
    };
    let _ = roster_for(root, &operation, &conflicted);
}

// --- what the bar draws ------------------------------------------------------------------

/// The operation in progress, or `None` when the repository is clean.
pub fn state(root: &Path) -> Result<Option<MergeState>> {
    let repo = repo_mod::open(root)?;
    let Some(operation) = repo_mod::operation_in_progress(&repo) else {
        // Nothing in flight. The roster is dead and keeping it would make the *next*
        // operation's lazy snapshot read somebody else's paths.
        clear_roster(root);
        return Ok(None);
    };

    let index = repo.index().wrap()?;
    let conflicted = repo_mod::conflicts_of(&index)?;
    let roster = roster_for(root, &operation, &conflicted);

    let mut entries: Vec<ConflictEntry> = roster
        .iter()
        .map(|path| ConflictEntry {
            path: path.clone(),
            resolved: !conflicted.contains(path),
            binary: is_binary_path(&repo, &index, path),
        })
        .collect();
    // A path that conflicted after the roster was taken — a rebase's later step — is added
    // rather than hidden. The roster is a floor, not a whitelist.
    for path in &conflicted {
        if !roster.contains(path) {
            entries.push(ConflictEntry {
                path: path.clone(),
                resolved: false,
                binary: is_binary_path(&repo, &index, path),
            });
        }
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    let ours = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().ok().map(str::to_string))
        .unwrap_or_else(|| "HEAD".to_string());

    Ok(Some(MergeState {
        theirs: describe_theirs(&repo, &operation),
        step: rebase_step(&repo),
        operation,
        ours,
        entries,
    }))
}

/// What is being merged in, written for a person and never parsed.
fn describe_theirs(repo: &Repository, operation: &str) -> String {
    if operation == "rebase" {
        // The branch the rebase started from — `orig_head_name` is what `git status` prints.
        if let Ok(rebase) = repo.open_rebase(None)
            && let Ok(Some(name)) = rebase.orig_head_name()
        {
            return name.trim_start_matches("refs/heads/").to_string();
        }
        return "the upstream".to_string();
    }
    let mut heads = Vec::new();
    // `mergehead_foreach` needs `&mut`, and this borrow is read-only, so a second handle is
    // opened rather than threading mutability through every caller of `state`.
    if let Ok(mut fresh) = Repository::open(repo.path()) {
        let _ = fresh.mergehead_foreach(|oid| {
            heads.push(*oid);
            true
        });
    }
    match heads.first() {
        Some(oid) => name_for(repo, *oid),
        None => "the other branch".to_string(),
    }
}

/// A commit as a person would name it: a ref that points at it, else a short oid and summary.
fn name_for(repo: &Repository, oid: Oid) -> String {
    if let Ok(refs) = repo.references() {
        for reference in refs.flatten() {
            if reference.target() == Some(oid)
                && let Ok(name) = reference.shorthand_bytes().pipe_utf8()
                && name != "HEAD"
            {
                return name;
            }
        }
    }
    let short: String = oid.to_string().chars().take(8).collect();
    match repo
        .find_commit(oid)
        .ok()
        .and_then(|c| c.summary().ok().flatten().map(str::to_string))
    {
        Some(summary) if !summary.is_empty() => format!("{short} ({summary})"),
        _ => short,
    }
}

/// A tiny extension so the reference walk above reads as one expression.
trait PipeUtf8 {
    fn pipe_utf8(self) -> std::result::Result<String, ()>;
}
impl PipeUtf8 for &[u8] {
    fn pipe_utf8(self) -> std::result::Result<String, ()> {
        std::str::from_utf8(self)
            .map(str::to_string)
            .map_err(|_| ())
    }
}

/// A rebase's position through its todo list, one-based.
///
/// `None` for a merge, which is one step by construction. Deliberately not faked as `(1, 1)`:
/// a bar that says *"1 of 1"* on every merge is noise that teaches the reader to skip the
/// counter on the rebases where it carries the only information they need.
fn rebase_step(repo: &Repository) -> Option<MergeStep> {
    let mut rebase = repo.open_rebase(None).ok()?;
    let total = rebase.len() as u32;
    // `operation_current` is a zero-based index into the todo list, and `None` before the
    // first `next()`. Both map onto a one-based "3 of 7" the same way.
    let done = rebase.operation_current().map_or(0, |n| n as u32 + 1);
    Some(MergeStep { done, total })
}

/// Whether any side of a conflicted path is binary.
///
/// Asked of the *blobs*, not of the working-tree file: the working tree holds git's marker
/// soup, which is text even when both sides are not.
fn is_binary_path(repo: &Repository, index: &Index, path: &str) -> bool {
    let Ok(conflicts) = index.conflicts() else {
        return false;
    };
    for entry in conflicts.flatten() {
        let matches = [&entry.our, &entry.their, &entry.ancestor]
            .iter()
            .any(|side| side.as_ref().is_some_and(|e| e.path == path.as_bytes()));
        if !matches {
            continue;
        }
        return [entry.our, entry.their, entry.ancestor]
            .iter()
            .flatten()
            .any(|e| repo.find_blob(e.id).map(|b| b.is_binary()).unwrap_or(false));
    }
    false
}

// --- reading one file ---------------------------------------------------------------------

/// The three sides of one conflicted path, for the resolver.
pub fn read(root: &Path, path: &str) -> Result<ConflictFile> {
    let repo = repo_mod::open(root)?;
    let index = repo.index().wrap()?;
    let entry = find_conflict(&index, path)?;

    let operation = repo_mod::operation_in_progress(&repo).unwrap_or_default();
    let (ours_label, theirs_label) = side_labels(&repo, &operation);

    let sides = [&entry.ancestor, &entry.our, &entry.their];
    let biggest = sides
        .iter()
        .flat_map(|s| s.as_ref())
        .filter_map(|e| repo.find_blob(e.id).ok().map(|b| b.content().len() as u64))
        .max()
        .unwrap_or(0);
    let binary = sides
        .iter()
        .flat_map(|s| s.as_ref())
        .any(|e| repo.find_blob(e.id).map(|b| b.is_binary()).unwrap_or(false));

    // Both guards refuse **instead of** filling the sides in, so a caller that ignores the
    // flags renders nothing rather than mojibake or a locked-up editor.
    if binary || biggest > MAX_SIDE_BYTES {
        return Ok(ConflictFile {
            path: path.to_string(),
            base: None,
            ours: None,
            theirs: None,
            our_label: ours_label,
            their_label: theirs_label,
            binary,
            too_large: (biggest > MAX_SIDE_BYTES).then_some(biggest),
        });
    }

    let text = |side: &Option<IndexEntry>| -> Option<String> {
        let e = side.as_ref()?;
        let blob = repo.find_blob(e.id).ok()?;
        String::from_utf8(blob.content().to_vec()).ok()
    };
    Ok(ConflictFile {
        path: path.to_string(),
        base: text(&entry.ancestor),
        ours: text(&entry.our),
        theirs: text(&entry.their),
        our_label: ours_label,
        their_label: theirs_label,
        binary: false,
        too_large: None,
    })
}

/// What to call stage 2 and stage 3 — **and a rebase swaps them.**
///
/// This is git's own confusion and it has to be handled rather than inherited. In a merge,
/// stage 2 is the branch you are standing on and stage 3 is the branch coming in, which is what
/// everybody expects. In a **rebase**, `HEAD` is the *new base* while each of your commits is
/// replayed on top — so stage 2 is the upstream and stage 3 is **your own commit**. A resolver
/// that labelled the panes `HEAD (main)` and `origin/main` during a rebase would have them
/// exactly backwards, and *Accept Yours* would take the other branch's work.
///
/// So the labels come from the operation, not from a fixed idea of which side is whose, and
/// they are written for a person: during a rebase the left pane says what is being rebased onto
/// and the right pane names the commit being replayed.
fn side_labels(repo: &Repository, operation: &str) -> (String, String) {
    if operation == "rebase" {
        let onto = repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .map(|c| name_for(repo, c.id()))
            .unwrap_or_else(|| "the new base".to_string());
        let replaying = repo
            .find_reference("REBASE_HEAD")
            .ok()
            .and_then(|r| r.target())
            .map(|oid| name_for(repo, oid))
            .unwrap_or_else(|| "your commit".to_string());
        return (
            format!("Rebasing onto {onto}"),
            format!("Your commit {replaying}"),
        );
    }
    let ours = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().ok().map(str::to_string))
        .map(|b| format!("HEAD ({b})"))
        .unwrap_or_else(|| "HEAD".to_string());
    (ours, describe_theirs(repo, operation))
}

fn find_conflict(index: &Index, path: &str) -> Result<git2::IndexConflict> {
    let conflicts = index.conflicts().wrap()?;
    for entry in conflicts.flatten() {
        let hit = [&entry.our, &entry.their, &entry.ancestor]
            .iter()
            .any(|side| side.as_ref().is_some_and(|e| e.path == path.as_bytes()));
        if hit {
            return Ok(entry);
        }
    }
    Err(GitError::NotConflicted {
        path: path.to_string(),
    })
}

// --- resolving ------------------------------------------------------------------------------

/// Write `content` as the resolution of `path`, and mark it resolved.
///
/// Two steps, in this order: the working-tree file, then the index. `Index::add_path` reads the
/// file from disk and adds it at stage 0, and **adding a path at stage 0 is what removes its
/// conflict stages** — which is exactly what `git add` does and why there is no separate
/// "mark resolved" call.
///
/// The working tree first, because the index entry is a claim about the file: an index that
/// says stage 0 over a file still full of `<<<<<<<` is a merge that will commit marker soup.
pub fn resolve(root: &Path, path: &str, content: &[u8]) -> Result<()> {
    let repo = repo_mod::open(root)?;
    ensure_roster(root, &repo);
    let index = repo.index().wrap()?;
    // Refused rather than accepted-as-a-no-op. Two windows can have the panel open, and the
    // ordinary cause of this is one of them acting on a row the other already resolved.
    find_conflict(&index, path)?;
    drop(index);

    let workdir = repo.workdir().ok_or_else(|| GitError::Bare {
        path: root.display().to_string(),
    })?;
    let file = workdir.join(path);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).wrap()?;
    }
    std::fs::write(&file, content).wrap()?;

    let mut index = repo.index().wrap()?;
    index.add_path(Path::new(path)).wrap()?;
    index.write().wrap()?;
    let _ = changelist::record_index(root, &repo);
    Ok(())
}

/// Resolve `path` by taking one side whole — the panel's one-click buttons.
///
/// A side that is absent is a **deletion**: choosing *theirs* when their side deleted the file
/// removes it, which is the answer the button promises. `remove_path` also clears the conflict
/// stages, so there is nothing else to do for that case.
pub fn take_side(root: &Path, path: &str, side: ConflictSide) -> Result<()> {
    let repo = repo_mod::open(root)?;
    ensure_roster(root, &repo);
    let index = repo.index().wrap()?;
    let entry = find_conflict(&index, path)?;
    drop(index);

    let chosen = match side {
        ConflictSide::Base => entry.ancestor,
        ConflictSide::Ours => entry.our,
        ConflictSide::Theirs => entry.their,
    };

    let workdir = repo.workdir().ok_or_else(|| GitError::Bare {
        path: root.display().to_string(),
    })?;
    let file = workdir.join(path);

    match chosen {
        Some(e) => {
            let blob = repo.find_blob(e.id).wrap()?;
            let content = blob.content().to_vec();
            drop(blob);
            resolve(root, path, &content)
        }
        None => {
            // Absent on the chosen side: that side deleted it.
            if file.exists() {
                std::fs::remove_file(&file).wrap()?;
            }
            let mut index = repo.index().wrap()?;
            index.remove_path(Path::new(path)).wrap()?;
            index.write().wrap()?;
            let _ = changelist::record_index(root, &repo);
            Ok(())
        }
    }
}

/// Put a resolved path back into conflict.
///
/// Only possible while the index still holds the three stages, which it does not once the path
/// has been resolved — collapsing them *is* resolving, and git keeps no copy. So this rebuilds
/// them from the commits the operation is between, and refuses with [`GitError::StagesGone`]
/// when it cannot find them. Saying that plainly is better than a button that silently produces
/// an empty conflict.
pub fn unresolve(root: &Path, path: &str) -> Result<()> {
    let repo = repo_mod::open(root)?;
    let operation = repo_mod::operation_in_progress(&repo).ok_or_else(|| GitError::StagesGone {
        path: path.to_string(),
    })?;

    let ours = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .ok_or_else(|| GitError::StagesGone {
            path: path.to_string(),
        })?;
    let theirs_oid = other_side(&repo, &operation).ok_or_else(|| GitError::StagesGone {
        path: path.to_string(),
    })?;
    let theirs = repo.find_commit(theirs_oid).wrap()?;
    let base = repo
        .merge_base(ours.id(), theirs.id())
        .ok()
        .and_then(|oid| repo.find_commit(oid).ok());

    let stage = |commit: Option<&git2::Commit<'_>>, n: u16| -> Option<IndexEntry> {
        let tree = commit?.tree().ok()?;
        let found = tree.get_path(Path::new(path)).ok()?;
        let blob = repo.find_blob(found.id()).ok()?;
        Some(IndexEntry {
            ctime: git2::IndexTime::new(0, 0),
            mtime: git2::IndexTime::new(0, 0),
            dev: 0,
            ino: 0,
            mode: found.filemode() as u32,
            uid: 0,
            gid: 0,
            file_size: blob.content().len() as u32,
            id: found.id(),
            // The stage number lives in the high bits of `flags`; libgit2 spells it
            // `GIT_INDEX_ENTRY_STAGE_SHIFT`, which is 12.
            flags: n << 12,
            flags_extended: 0,
            path: path.as_bytes().to_vec(),
        })
    };

    let entries: Vec<IndexEntry> = [
        stage(base.as_ref(), 1),
        stage(Some(&ours), 2),
        stage(Some(&theirs), 3),
    ]
    .into_iter()
    .flatten()
    .collect();
    // Nothing on either side: the path is not part of this operation at all.
    if entries.len() < 2 {
        return Err(GitError::StagesGone {
            path: path.to_string(),
        });
    }

    let mut index = repo.index().wrap()?;
    index.remove_path(Path::new(path)).wrap()?;
    for entry in &entries {
        index.add(entry).wrap()?;
    }
    index.write().wrap()?;
    let _ = changelist::record_index(root, &repo);
    Ok(())
}

/// The commit the operation is merging in, when one can be named.
fn other_side(repo: &Repository, operation: &str) -> Option<Oid> {
    if operation == "rebase" {
        // The commit being replayed, which is `REBASE_HEAD`.
        return repo
            .find_reference("REBASE_HEAD")
            .ok()
            .and_then(|r| r.target());
    }
    let mut heads = Vec::new();
    let mut fresh = Repository::open(repo.path()).ok()?;
    let _ = fresh.mergehead_foreach(|oid| {
        heads.push(*oid);
        true
    });
    heads.first().copied()
}

// --- finishing ------------------------------------------------------------------------------

/// Conclude the operation, or advance a rebase to its next stop.
///
/// A merge is concluded by [`crate::commit::commit`], which has known how since M10 — it
/// collects `MERGE_HEAD`, writes the extra parents and calls `cleanup_state` — so this does not
/// reimplement it. A rebase is `Rebase::commit` followed by driving `next()` until something
/// conflicts or the list runs out.
pub fn cont(root: &Path, message: Option<&str>) -> Result<ContinueOutcome> {
    let repo = repo_mod::open(root)?;
    let operation = repo_mod::operation_in_progress(&repo).ok_or_else(|| {
        // Nothing in progress. Its own sentence rather than a libgit2 class name, because the
        // ordinary cause is a second window having finished it a moment ago.
        GitError::NotConflicted {
            path: root.display().to_string(),
        }
    })?;

    let index = repo.index().wrap()?;
    let unresolved = repo_mod::conflicts_of(&index)?;
    if !unresolved.is_empty() {
        return Err(GitError::Conflicted { paths: unresolved });
    }
    drop(index);

    if operation != "rebase" {
        let oid = finish_merge(root, message)?;
        clear_roster(root);
        return Ok(ContinueOutcome { oid, state: None });
    }

    let signature = repo.signature().wrap()?;
    let mut rebase = repo.open_rebase(None).wrap()?;
    let mut last = String::new();
    match rebase.commit(None, &signature, message) {
        Ok(oid) => last = oid.to_string(),
        // The resolution left nothing to commit — the user took `ours` for every hunk, so this
        // step is now empty. git's `--empty=drop`, and the same skip `pull::rebase_pull` makes.
        Err(e) if e.code() == git2::ErrorCode::Applied => {}
        Err(e) => return Err::<ContinueOutcome, git2::Error>(e).wrap(),
    }

    while let Some(operation) = rebase.next() {
        operation.wrap()?;
        let index = repo.index().wrap()?;
        let conflicts = repo_mod::conflicts_of(&index)?;
        if !conflicts.is_empty() {
            drop(index);
            drop(rebase);
            let _ = changelist::record_index(root, &repo);
            return Ok(ContinueOutcome {
                oid: last,
                state: state(root)?,
            });
        }
        drop(index);
        match rebase.commit(None, &signature, None) {
            Ok(oid) => last = oid.to_string(),
            Err(e) if e.code() == git2::ErrorCode::Applied => {}
            Err(e) => return Err::<ContinueOutcome, git2::Error>(e).wrap(),
        }
    }
    rebase.finish(Some(&signature)).wrap()?;
    drop(rebase);
    clear_roster(root);
    let _ = changelist::record_index(root, &repo);
    settle(root, &repo);
    Ok(ContinueOutcome {
        oid: last,
        state: None,
    })
}

/// Commit the merge, cherry-pick or revert through the ordinary commit path.
fn finish_merge(root: &Path, message: Option<&str>) -> Result<String> {
    let stored = repo_mod::open(root)?;
    let text = match message {
        Some(text) if !text.trim().is_empty() => text.to_string(),
        /*
         * `MERGE_MSG` is what `git commit` would open an editor on, so using it means a merge
         * concluded from the panel carries the same message as one concluded from a terminal.
         * `cide_git::pull` overwrites libgit2's version of that file with git's own sentence,
         * so this reads the same text whichever side started the merge.
         *
         * Comment lines are stripped, because `git commit` strips them and the file git writes
         * ends in a `#Conflicts:` block listing the very files that have just been resolved.
         * Committing it verbatim would put that block in the history.
         */
        _ => {
            // `MERGE_MSG` for all three: git and libgit2 both write it for a cherry-pick and a
            // revert as well as for a merge, and it holds the message the replayed commit
            // carried — which is the message `git cherry-pick --continue` would offer.
            let raw = std::fs::read_to_string(stored.path().join("MERGE_MSG"))
                .unwrap_or_else(|_| "Merge\n".to_string());
            let body: String = raw
                .lines()
                .filter(|line| !line.trim_start().starts_with('#'))
                .collect::<Vec<_>>()
                .join("\n");
            let trimmed = body.trim_end();
            if trimmed.is_empty() {
                "Merge\n".to_string()
            } else {
                format!("{trimmed}\n")
            }
        }
    };
    let outcome = commit::commit(
        root,
        &cide_ipc::git::CommitRequest {
            message: text,
            amend: false,
            // No changelist and no selections: on a merge the **index is the truth**, built by
            // the merge and then edited by the resolutions above, and `commit::commit` takes
            // its staging-area arm for exactly this case. Naming a changelist here would ask
            // it to rebuild the index from HEAD and throw the merge away.
            changelist: None,
            selections: None,
            // The index guard is waived, and it has to be: a merge is by definition an index
            // cide did not write hunk by hunk, so the "staging changed outside cide" bar would
            // fire on every single merge conclusion.
            force: true,
            amend_of: None,
        },
    )?;
    Ok(outcome.oid)
}

/// `git merge --abort` / `git rebase --abort`.
///
/// Both restore the tree to where the operation started. For a rebase libgit2 does it;
/// for a merge it is `cleanup_state` plus a hard reset to `ORIG_HEAD`, which is what
/// `git merge --abort` is documented to be equivalent to.
pub fn abort(root: &Path) -> Result<()> {
    let repo = repo_mod::open(root)?;
    let Some(operation) = repo_mod::operation_in_progress(&repo) else {
        clear_roster(root);
        return Ok(());
    };

    if operation == "rebase" {
        let mut rebase = repo.open_rebase(None).wrap()?;
        rebase.abort().wrap()?;
        drop(rebase);
    } else {
        let was = repo
            .find_reference("ORIG_HEAD")
            .ok()
            .and_then(|r| r.target())
            .or_else(|| repo.head().ok().and_then(|h| h.target()))
            .ok_or(GitError::Unborn)?;
        let target = repo.find_object(was, None).wrap()?;
        // `HARD`, and this is the one place in the crate that uses it deliberately: the whole
        // promise of Abort is *put it back*, and a `SAFE` reset would refuse over the very
        // marker-filled files the merge wrote.
        repo.reset(&target, git2::ResetType::Hard, None).wrap()?;
        repo.cleanup_state().wrap()?;
    }

    clear_roster(root);
    let _ = changelist::record_index(root, &repo);
    settle(root, &repo);
    Ok(())
}

/// Sweep changelist assignments that no longer name a live path.
///
/// Same trailer, same `let _`, and the same reason as `commit::commit` and `replay::replay`:
/// only explicit assignments are stored, so nothing needs editing — but the dead ones have to
/// go or they accumulate against paths that no longer exist.
fn settle(root: &Path, _repo: &Repository) {
    let live = status::live_paths(root).unwrap_or_default();
    let _ = changelist::update(root, |data| {
        let _ = data.reconcile(&live);
        Ok(())
    });
}
