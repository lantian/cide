//! One git worktree per agent: `.cide/worktrees/<agent>` on branch `cide/<agent>`. (M18)
//!
//! # Why a worktree at all
//!
//! Several subagents run at once against one project. Sharing a checkout means two of them
//! writing one file with nobody told, and the loser's edit is gone before either finishes its
//! turn. A worktree gives each its own working directory and its own `HEAD` while sharing the
//! object database, so the isolation costs one checkout rather than one clone.
//!
//! # One worktree per *task*, not per run — and the name is the caller's composite
//!
//! This header used to argue "one worktree per agent, not per run": the role was the unit, its
//! queue was serial, and per-run worktrees "would leave the question of which of five stale
//! checkouts holds the work nobody merged". Half of that argument survives — per-**run**
//! checkouts really would orphan work — but the serial queue it bought was the cost a real
//! workstream refused to pay: a role with `max-concurrent: 2` ran one task at a time and the
//! user watched the second queue for no reason they could see. The unit that keeps both
//! halves is the **task**: `cide_agents::checkout_name` composes `<role>-<task>` (the bare role
//! is only the *legacy* base branch's name now — since M40 a run with no task stands in the
//! project root and takes no checkout, `cide_agents::run_checkout` being the rule), so a role's
//! tasks parallelise in their own checkouts, the
//! merge unit is a task's branch — which answers exactly "which checkout holds the work" —
//! and a re-dispatch of the same task lands where its earlier commits already are.
//!
//! Nothing in *this* module changed for that: every function here takes a **name** and builds
//! `.cide/worktrees/<name>` on `cide/<name>` from it. The composition lives with the registry,
//! which is the one place that knows both halves of the pair.
//!
//! A consequence worth writing down: `claude` files its transcript under the directory it
//! started in, so a run's cwd *is* its worktree and resume follows it, and paths an agent
//! prints are outside the project root — which is why `terminal_open_path`'s containment gate
//! asks about them. That is correct behaviour, not a bug.
//!
//! # Integration is explicit and refuses rather than half-does
//!
//! [`integrate`] merges `cide/<agent>` into whatever the project root has checked out, and on
//! a conflict it reports the paths **having changed nothing**. Auto-merging when a task turns
//! `Done` was the tempting wrong move: a conflict would then surface as a broken checkout the
//! user did not ask for, with no task explaining it. A refusal they can read beats a working
//! tree they have to repair.
//!
//! # No cached state, as everywhere else in this crate
//!
//! Every function here opens what it needs and drops it; see the crate header.

use std::path::{Path, PathBuf};

use cide_ipc::git::GitError;
use git2::{BranchType, Repository, Tree, WorktreeAddOptions, WorktreePruneOptions};

use crate::{Result, Wrap, branch, repo as repo_mod};

/// Where agent checkouts live, relative to the project root.
///
/// Under `.cide/` because that is the directory this feature already owns (the task board is
/// its neighbour), and because one directory is one line in a `.gitignore` rather than a
/// pattern somebody has to maintain.
const WORKTREES_DIR: &str = ".cide/worktrees";

/// Where `agent`'s checkout is — `<root>/.cide/worktrees/<agent>` — whether or not it exists.
///
/// The one spelling of the path, shared by [`ensure`], [`remove`] and the agent registry, which
/// needs it for a run whose child is long gone: a conversation is filed under the directory the
/// child started in, and re-opening it from anywhere else finds nothing. Pure, and deliberately
/// so — the registry asks under a lock and must not open a repository there. (M42)
pub fn path_of(root: &Path, agent: &str) -> PathBuf {
    root.join(WORKTREES_DIR).join(agent)
}

/// The project root `checkout` is an agent worktree of — `<root>` for
/// `<root>/.cide/worktrees/<name>` — or `None` for any other directory. Pure, like [`path_of`],
/// which it inverts: a pane standing in a run's checkout asks it to find the project whose
/// `.cide/config.json` says how that checkout's children are isolated.
#[must_use]
pub fn root_of_checkout(checkout: &Path) -> Option<PathBuf> {
    let parent = checkout.parent()?;
    checkout.file_name()?;
    parent
        .ends_with(WORKTREES_DIR)
        .then(|| {
            parent
                .parent()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
        })
        .flatten()
}

/// The git metadata a checkout writes when it commits, outside the checkout itself. (M118)
///
/// A linked worktree's `.git` is a *file* pointing at `<repo>/.git/worktrees/<name>`, where its
/// index, `HEAD` and per-worktree refs live; branches, objects and `packed-refs` live in the
/// common `<repo>/.git`. A sandbox whose writable root is the checkout sees neither as writable,
/// and codex's `workspace-write` sandbox re-binds any `.git` it finds read-only on top — so a
/// codex run needs scoped grants or an approved operation for `git commit`, `merge` or
/// `rebase` (`index.lock: Read-only file system`, selfcraft t-572). The Codex launchers use
/// these paths in an invocation-local permission profile; legacy launches use `--add-dir`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitDirs {
    /// `git rev-parse --git-dir`: the checkout's own admin directory.
    pub git_dir: PathBuf,
    /// `git rev-parse --git-common-dir`: refs, objects and config shared by every checkout.
    pub common_dir: PathBuf,
}

/// [`GitDirs`] of the repository `checkout` is a working tree of. Canonicalised, so a sandbox
/// given these paths binds what git will actually open. For a plain (non-linked) checkout the
/// two are the same `.git`.
pub fn git_dirs(checkout: &Path) -> Result<GitDirs> {
    let repo = Repository::open(checkout).wrap()?;
    Ok(GitDirs {
        git_dir: repo_mod::canonical(repo.path()),
        common_dir: repo_mod::canonical(repo.commondir()),
    })
}

/// The ref namespace agent branches live in. `cide/` rather than a bare name so `git branch`
/// groups them, and so a user's own `developer` branch is never the one an agent commits to.
pub const BRANCH_PREFIX: &str = "cide";

/// Where an agent's checkout lives and what branch it is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentWorktree {
    /// The agent id, which is also the worktree's registration name in `.git/worktrees/`.
    pub agent: String,
    /// Absolute, canonical path of the checkout.
    pub path: PathBuf,
    /// What `HEAD` is on **now**, which is normally `cide/<agent>` and is read rather than
    /// assumed: a user is free to check something else out in there, and reporting the branch
    /// cide *meant* would be a claim this function never checked.
    pub branch: String,
    /// The full hex commit id `HEAD` resolves to; empty on an unborn `HEAD`, which a worktree
    /// cide created never has (it is checked out from a commit) but a hand-edited one can.
    pub head: String,
}

/// What [`integrate`] did, or refused to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Integration {
    /// Nothing to do — the agent branch has no commits the base lacks.
    UpToDate,
    /// Merged, with the resulting commit id. A fast-forward reports this too, naming the
    /// commit it moved to: from the caller's side "your branch now contains that work" is the
    /// same answer, and a second variant would mean two UI paths saying one thing.
    Merged { commit: String, files: usize },
    /// Refused *before touching anything*, listing the conflicting paths.
    Conflicts { paths: Vec<String> },
}

/// Every agent worktree cide has created under this root.
///
/// Deliberately **not** every worktree the repository has: a user's own
/// `git worktree add ../hotfix` is theirs, and a panel listing agents must not offer to
/// integrate or remove it. The filter is the path — anything under `<root>/.cide/worktrees/`
/// is cide's, anything else is not.
///
/// A registration whose directory has been deleted is skipped rather than reported: it is not
/// a place anything can run, and [`ensure`] repairs it on the next dispatch.
pub fn list(root: &Path) -> Result<Vec<AgentWorktree>> {
    let root = repo_mod::canonical(root);
    let repo = repo_mod::open(&root)?;
    let dir = repo_mod::canonical(&root.join(WORKTREES_DIR));

    let mut out = Vec::new();
    // `iter()` yields `Result<Option<&str>>`: a name that is not UTF-8 is not one cide wrote,
    // and skipping it is the same answer as the path filter below would give.
    let names = repo.worktrees().wrap()?;
    for name in names.iter().filter_map(|name| name.ok().flatten()) {
        let Ok(worktree) = repo.find_worktree(name) else {
            continue;
        };
        if !repo_mod::canonical(worktree.path()).starts_with(&dir) {
            continue;
        }
        if worktree.validate().is_err() {
            tracing::debug!(
                agent = name,
                "skipping an agent worktree whose directory is gone"
            );
            continue;
        }
        match describe(name, worktree.path()) {
            Ok(entry) => out.push(entry),
            Err(error) => {
                tracing::warn!(agent = name, %error, "an agent worktree could not be read")
            }
        }
    }
    // By agent, so the panel's order does not depend on readdir.
    out.sort_by(|a, b| a.agent.cmp(&b.agent));
    Ok(out)
}

/// Every agent worktree of every repository at `roots`, in root order, each once.
///
/// The set the Git panel's and the Log's worktree selectors offer, and — through [`find`] and
/// [`resolve`] — the *only* set a command keyed by a worktree's `RepoId` or path will act on.
/// A project root is resolved to its work tree first ([`repo_mod::discover_root`]), so a root
/// that is a subdirectory of its repository still finds the checkouts `ensure` made beside the
/// repository's own `.cide/`. A root that is not a repository, or whose worktrees cannot be
/// read, contributes nothing rather than failing the rest: the same answer [`repo_mod::discover`]
/// gives for an unmounted root.
pub fn list_for(roots: &[PathBuf]) -> Vec<AgentWorktree> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for root in roots {
        let Some(work) = repo_mod::discover_root(root) else {
            continue;
        };
        if !seen.insert(work.clone()) {
            continue;
        }
        match list(&work) {
            Ok(found) => out.extend(found),
            Err(error) => {
                tracing::debug!(root = %work.display(), %error, "no agent worktrees read")
            }
        }
    }
    out
}

/// The agent worktree under `roots` whose `RepoId` is `repo`, as the `RepoInfo` discovery would
/// have built for it — or `None`.
///
/// The fallback behind every `(project, RepoId)` command once the panel is looking at a
/// worktree: [`repo_mod::find`] only knows roots and submodules, so without this a checkout's
/// id — a perfectly good `repo_id` of its canonical work tree — is `NoSuchRepo` to stage,
/// commit, diff and the log alike. Only what [`list`] reports is matched, which is the trust
/// boundary: a user's own `git worktree add ../hotfix`, or any other directory whose id a caller
/// could compute, is never resolved through here.
pub fn find(roots: &[PathBuf], repo: cide_ipc::RepoId) -> Option<cide_ipc::git::RepoInfo> {
    list_for(roots)
        .into_iter()
        .find(|wt| repo_mod::repo_id(&wt.path) == repo)
        .map(|wt| info(&wt))
}

/// `path` canonicalised, when it is one of the agent worktrees under `roots`; `None` otherwise.
///
/// For the calls that take a whole tree rather than one repository (`git_status` with a
/// worktree selected): the UI names the checkout by path, and this is the check that the path
/// is one cide made rather than anything the webview cares to send.
pub fn resolve(roots: &[PathBuf], path: &Path) -> Option<PathBuf> {
    let wanted = repo_mod::canonical(path);
    list_for(roots)
        .into_iter()
        .find(|wt| wt.path == wanted)
        .map(|wt| wt.path)
}

/// The `RepoInfo` a worktree stands as: a top-level repository of its own, named after its
/// checkout. Not a submodule and not parented — it shares the object database with the root, but
/// its index, `HEAD` and changelists are its own, which is everything the panel keys on.
pub fn info(wt: &AgentWorktree) -> cide_ipc::git::RepoInfo {
    cide_ipc::git::RepoInfo {
        id: repo_mod::repo_id(&wt.path),
        root: wt.path.clone(),
        name: wt.agent.clone(),
        parent: None,
        is_submodule: false,
    }
}

/// Idempotent: create `.cide/worktrees/<agent>` on `cide/<agent>` from the project's current
/// `HEAD`, or return the existing one. Safe to call before every dispatch.
///
/// # The half-made states, and what each one does
///
/// A crash between the branch and the checkout, an `rm -rf` of a directory that looked like
/// scratch, a previous run that got as far as committing — each leaves a different half-state
/// and each has a different right answer:
///
/// * **A valid registration** — return it. That is what makes this callable before every
///   dispatch instead of once, which is the only shape that survives a crash between the two.
/// * **A registration whose directory was deleted** (`git2` calls this *prunable*) — prune the
///   admin files and build it again. Not "return the registration": there is nowhere to run.
///   Not "fail": a deleted directory is a state `rm -rf` produces in one keystroke, and an
///   agent that can never be dispatched again because of it is a worse outcome than a rebuild.
///   The prune is the admin half only; the working tree is already gone.
/// * **A registration pointing somewhere else** — refuse. Adopting a checkout cide did not
///   make would put an agent's commits in a directory the user is using for something.
/// * **A directory with no registration** — an empty one is removed (libgit2 creates the
///   worktree directory with `O_EXCL` and fails outright if it exists), a non-empty one is
///   refused. cide deletes no directory it cannot prove it made; the files in there may be the
///   only copy of something.
/// * **A branch left by a previous run** — reuse it, never recreate it. Its commits *are* the
///   agent's work, and re-pointing the ref at today's `HEAD` would abandon them where only
///   the reflog remembers.
pub fn ensure(root: &Path, agent: &str) -> Result<AgentWorktree> {
    validate_agent(agent)?;
    let root = repo_mod::canonical(root);
    let repo = repo_mod::open(&root)?;
    // Best effort: a checkout that cannot be hidden from `git status` is noise, not a reason to
    // refuse the run.
    if let Err(error) = exclude_worktrees(repo.commondir()) {
        tracing::warn!(%error, "could not add .cide/worktrees/ to info/exclude");
    }
    let path = path_of(&root, agent);
    let wanted_branch = branch_name(agent);

    if let Ok(existing) = repo.find_worktree(agent) {
        // The path check comes before everything, including the prune: a registration by this
        // name that points somewhere else is the user's, and neither adopting it nor tidying
        // its admin files is ours to do. `Path`'s equality is by component, which is what
        // makes this survive libgit2 handing back a directory with a trailing separator.
        let registered = repo_mod::canonical(existing.path());
        if registered != repo_mod::canonical(&path) {
            return Err(GitError::Io {
                detail: format!(
                    "a worktree named {agent} is already registered at {}, not at {}",
                    registered.display(),
                    path.display()
                ),
            });
        }
        if existing.validate().is_ok() {
            return describe(agent, existing.path());
        }
        // Default prune options refuse a *valid* worktree and refuse a locked one, and do not
        // touch the working tree — so this can only ever remove the admin files of a
        // registration that is already broken, which is exactly `git worktree prune`.
        tracing::info!(agent, "pruning an agent worktree whose directory is gone");
        existing
            .prune(Some(&mut WorktreePruneOptions::new()))
            .wrap()?;
    }

    if std::fs::symlink_metadata(&path).is_ok() {
        let empty = std::fs::read_dir(&path)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false);
        if !empty {
            return Err(GitError::Io {
                detail: format!(
                    "{} already exists and is not a worktree cide made; move it aside",
                    path.display()
                ),
            });
        }
        std::fs::remove_dir(&path).wrap()?;
    }

    if repo.find_branch(&wanted_branch, BranchType::Local).is_err() {
        // From `HEAD`, via the crate's own creator so the name goes through the same
        // `check-ref-format` gate every other branch does, and an unborn `HEAD` answers
        // `Unborn` rather than a libgit2 error class.
        branch::create(&root, &wanted_branch, None)?;
    }

    // libgit2 makes the worktree directory itself with `GIT_MKDIR_EXCL` and does **not** make
    // its parents, so `.cide/worktrees/` has to exist first or the add fails with `ENOENT` on
    // a path the caller never named.
    std::fs::create_dir_all(root.join(WORKTREES_DIR)).wrap()?;

    let reference = repo
        .find_branch(&wanted_branch, BranchType::Local)
        .wrap()?
        .into_reference();
    let mut options = WorktreeAddOptions::new();
    options.reference(Some(&reference));
    // Naming the worktree after the agent is what makes `find_worktree(agent)` above the
    // whole of the "does it exist" question, with no side table to keep in sync.
    let created = repo.worktree(agent, &path, Some(&options)).wrap()?;
    describe(agent, created.path())
}

/// Remove one agent's worktree and prune the admin files.
///
/// **Does not delete the branch.** The work is the point, and after the checkout is gone the
/// branch is the only thing still holding it — `remove` is how a role is retired or a stuck
/// checkout is reset, and neither is a request to throw away commits. Deleting `cide/<agent>`
/// is a separate gesture, and [`branch::delete`] already refuses to do it silently.
///
/// Uncommitted changes *inside* the checkout do not survive; that is what removing a
/// working tree means, and it is why the caller should be asking the user rather than doing
/// this on a timer.
///
/// Removing something that is not there succeeds: the caller wants the state, not the event.
pub fn remove(root: &Path, agent: &str) -> Result<()> {
    validate_agent(agent)?;
    let root = repo_mod::canonical(root);
    let repo = repo_mod::open(&root)?;
    let path = path_of(&root, agent);

    if let Ok(existing) = repo.find_worktree(agent) {
        if repo_mod::canonical(existing.path()) != repo_mod::canonical(&path) {
            return Err(GitError::Io {
                detail: format!(
                    "the worktree named {agent} is at {}, which is not cide's to remove",
                    existing.path().display()
                ),
            });
        }
        // `valid` because the usual case is a perfectly healthy checkout, and `working_tree`
        // because leaving the directory behind would make the next `ensure` refuse it as "a
        // directory cide did not make".
        let mut options = WorktreePruneOptions::new();
        options.valid(true).working_tree(true);
        existing.prune(Some(&mut options)).wrap()?;
    }
    // A directory with no registration is the other half of the same half-made state
    // `ensure` handles; it is under a path cide owns and named by a validated agent id, so
    // removing it is removing our own scratch checkout and nothing else.
    if std::fs::symlink_metadata(&path).is_ok() {
        std::fs::remove_dir_all(&path).wrap()?;
    }
    Ok(())
}

/// Merge `cide/<agent>` into the branch the project root has checked out.
///
/// # It must never leave the user's checkout half-merged
///
/// The merge is computed **in memory** first — `merge_commits` produces an index nothing on
/// disk has seen — and [`Index::has_conflicts`] is asked before a single file is written. A
/// conflict returns [`Integration::Conflicts`] with the paths and changes nothing, so the
/// answer is a list the user (or the task's comment thread) can act on rather than a working
/// tree with markers in it that they now have to unpick.
///
/// After the check the order is the one `branch::checkout` uses and for the same reason: tree
/// first, then the ref. A `SAFE` checkout refuses to overwrite a locally modified file, so a
/// dirty base repository fails *before* any commit exists rather than after; the other order
/// leaves `HEAD` and the working tree disagreeing.
pub fn integrate(root: &Path, agent: &str) -> Result<Integration> {
    validate_agent(agent)?;
    let root = repo_mod::canonical(root);
    let repo = repo_mod::open(&root)?;

    // Merging into a tree that is already rebasing is how somebody loses work: the operation
    // in flight owns `HEAD`, the index and `MERGE_HEAD`, and a second merge writing through it
    // leaves a state neither `git rebase --continue` nor `git merge --abort` can undo.
    if let Some(operation) = repo_mod::operation_in_progress(&repo) {
        return Err(GitError::OperationInProgress { operation });
    }
    // The same argument one step weaker: an index with unresolved entries is a merge somebody
    // is in the middle of by hand.
    let unresolved = repo_mod::conflicted_paths(&repo)?;
    if !unresolved.is_empty() {
        return Err(GitError::Conflicted { paths: unresolved });
    }

    let name = branch_name(agent);
    let theirs = repo
        .find_branch(&name, BranchType::Local)
        .map_err(|_| GitError::NoSuchBranch { name: name.clone() })?
        .into_reference()
        .peel_to_commit()
        .wrap()?;

    let mut head = repo.head().map_err(|_| GitError::Unborn)?;
    let ours = head.peel_to_commit().wrap()?;
    // "The branch the project root has checked out" — a detached `HEAD` has none, and moving
    // a detached `HEAD` onto a merge commit puts the work somewhere no ref remembers.
    if repo.head_detached().unwrap_or(false) {
        return Err(GitError::DetachedHead {
            head: ours.id().to_string()[..8].to_string(),
        });
    }

    let annotated = repo.find_annotated_commit(theirs.id()).wrap()?;
    let (analysis, _preference) = repo.merge_analysis(&[&annotated]).wrap()?;
    if analysis.is_up_to_date() {
        return Ok(Integration::UpToDate);
    }

    let ours_tree = ours.tree().wrap()?;
    if analysis.is_fast_forward() {
        // A fast-forward is still a merge as far as the caller is concerned: the base branch
        // now contains the agent's commits, and the answer names the commit it moved to.
        let theirs_tree = theirs.tree().wrap()?;
        checkout(&repo, &theirs_tree)?;
        let files = changed(&repo, &ours_tree, &theirs_tree)?;
        head.set_target(theirs.id(), &format!("cide: fast-forward to {name}"))
            .wrap()?;
        return Ok(Integration::Merged {
            commit: theirs.id().to_string(),
            files,
        });
    }

    // In memory. Nothing below this line touches the working tree until the conflict question
    // has been answered.
    let mut index = repo.merge_commits(&ours, &theirs, None).wrap()?;
    if index.has_conflicts() {
        return Ok(Integration::Conflicts {
            paths: repo_mod::conflicts_of(&index)?,
        });
    }

    // `write_tree_to` adds objects to the odb and nothing else: unreferenced trees are what
    // `git gc` collects, and the checkout below is still free to fail without having claimed
    // anything.
    let tree_id = index.write_tree_to(&repo).wrap()?;
    let tree = repo.find_tree(tree_id).wrap()?;
    checkout(&repo, &tree)?;

    let signature = repo
        .signature()
        // A machine with no `user.email` anywhere is ordinary — a fresh container, a new
        // laptop — and "you cannot integrate an agent's work until you configure git" is a
        // dead end for a merge cide is making on the user's behalf. The identity is cide's
        // own so the commit says plainly who wrote it; the user's own commits are unaffected
        // because `commit::commit` still demands a real signature.
        .or_else(|_| git2::Signature::now("cide", "cide@localhost"))
        .wrap()?;
    let message = format!("Merge {name} into {}", head.shorthand().unwrap_or("HEAD"));
    let commit = repo
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            &message,
            &tree,
            &[&ours, &theirs],
        )
        .wrap()?;

    Ok(Integration::Merged {
        commit: commit.to_string(),
        files: changed(&repo, &ours_tree, &tree)?,
    })
}

/// Several agent branches merged together **without moving anything**. (M132)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Composed {
    /// The project's `HEAD` the merges started from.
    pub base: String,
    /// The combined commit — `None` when every branch was already in or refused.
    pub head: Option<String>,
    /// The agents whose branches are in `head`, in order.
    pub merged: Vec<String>,
    /// The agents whose branches had nothing `base` lacked.
    pub up_to_date: Vec<String>,
    /// The agents whose branches conflicted with what came before them, with the paths. Skipped;
    /// the ones after them were still tried.
    pub conflicts: Vec<(String, Vec<String>)>,
}

/// Merge `cide/<agent>` for each of `agents`, in order, on top of the project's `HEAD` —
/// **in memory, moving no ref and touching no file**. (M132)
///
/// A batch review merges three or four finished branches and verifies the result once. Doing
/// that with [`integrate`] would move the user's branch after the first merge, before anything
/// verified the combination; this computes the whole chain the way `integrate` computes one
/// merge (`merge_commits`, `has_conflicts`, `write_tree_to`), writes the merge commits as
/// unreferenced objects, and hands back the head. The caller verifies that head in a scratch
/// checkout and only then moves the branch with [`advance_to`]. A red verify moves nothing, and
/// the unreferenced commits are what `git gc` collects.
///
/// A branch that conflicts with the chain so far is skipped and named, and the rest still go:
/// one bad branch must not hold three good ones back.
pub fn compose_merges(root: &Path, agents: &[&str]) -> Result<Composed> {
    let root = repo_mod::canonical(root);
    let repo = repo_mod::open(&root)?;
    if let Some(operation) = repo_mod::operation_in_progress(&repo) {
        return Err(GitError::OperationInProgress { operation });
    }
    if repo.head_detached().unwrap_or(false) {
        let head = repo
            .head()
            .ok()
            .and_then(|h| h.target())
            .map(|o| o.to_string());
        return Err(GitError::DetachedHead {
            head: head.unwrap_or_default().chars().take(8).collect(),
        });
    }
    let base = repo
        .head()
        .map_err(|_| GitError::Unborn)?
        .peel_to_commit()
        .wrap()?;
    let head_name = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(str::to_owned).ok())
        .unwrap_or_else(|| "HEAD".to_string());
    let signature = repo
        .signature()
        .or_else(|_| git2::Signature::now("cide", "cide@localhost"))
        .wrap()?;
    let mut out = Composed {
        base: base.id().to_string(),
        ..Composed::default()
    };
    let mut current = base.clone();
    for agent in agents {
        validate_agent(agent)?;
        let name = branch_name(agent);
        let theirs = repo
            .find_branch(&name, BranchType::Local)
            .map_err(|_| GitError::NoSuchBranch { name: name.clone() })?
            .into_reference()
            .peel_to_commit()
            .wrap()?;
        if theirs.id() == current.id()
            || repo.graph_descendant_of(current.id(), theirs.id()).wrap()?
        {
            if current.id() == base.id() {
                out.up_to_date.push((*agent).to_string());
            } else {
                // In because an earlier branch of the batch already carried it.
                out.merged.push((*agent).to_string());
            }
            continue;
        }
        if repo.graph_descendant_of(theirs.id(), current.id()).wrap()? {
            current = theirs;
            out.merged.push((*agent).to_string());
            continue;
        }
        let mut index = repo.merge_commits(&current, &theirs, None).wrap()?;
        if index.has_conflicts() {
            out.conflicts
                .push(((*agent).to_string(), repo_mod::conflicts_of(&index)?));
            continue;
        }
        let tree_id = index.write_tree_to(&repo).wrap()?;
        let tree = repo.find_tree(tree_id).wrap()?;
        let message = format!("Merge {name} into {head_name}");
        let id = repo
            .commit(
                None,
                &signature,
                &signature,
                &message,
                &tree,
                &[&current, &theirs],
            )
            .wrap()?;
        current = repo.find_commit(id).wrap()?;
        out.merged.push((*agent).to_string());
    }
    if current.id() != base.id() {
        out.head = Some(current.id().to_string());
    }
    Ok(out)
}

/// Move the project's checked-out branch from `base` to `head` — a commit [`compose_merges`]
/// built on top of it — tree first, then the ref, with a `SAFE` checkout. (M132)
///
/// Refused when the branch is no longer at `base`: somebody committed or merged while the batch
/// verified, and moving the ref now would drop their commit. The caller composes again.
pub fn advance_to(root: &Path, base: &str, head: &str) -> Result<usize> {
    let root = repo_mod::canonical(root);
    let repo = repo_mod::open(&root)?;
    if let Some(operation) = repo_mod::operation_in_progress(&repo) {
        return Err(GitError::OperationInProgress { operation });
    }
    let mut reference = repo.head().map_err(|_| GitError::Unborn)?;
    let ours = reference.peel_to_commit().wrap()?;
    if ours.id().to_string() != base {
        return Err(GitError::Io {
            detail: format!(
                "the branch moved while the batch was verified (it was at {}, it is at {}); \
                 nothing was merged",
                &base[..base.len().min(8)],
                &ours.id().to_string()[..8]
            ),
        });
    }
    let target = repo.find_commit(git2::Oid::from_str(head).wrap()?).wrap()?;
    let ours_tree = ours.tree().wrap()?;
    let tree = target.tree().wrap()?;
    checkout(&repo, &tree)?;
    let files = changed(&repo, &ours_tree, &tree)?;
    reference
        .set_target(target.id(), "cide: merge a reviewed batch")
        .wrap()?;
    Ok(files)
}

/// A scratch checkout of `commit` at `.cide/worktrees/<name>`, on a branch `cide/<name>` forced to
/// it — where a combined head is verified before anything moves. (M132) Any previous checkout by
/// that name is removed first: it is cide's scratch, never a run's.
pub fn scratch_at(root: &Path, name: &str, commit: &str) -> Result<PathBuf> {
    validate_agent(name)?;
    let root = repo_mod::canonical(root);
    remove(&root, name)?;
    let repo = repo_mod::open(&root)?;
    let target = repo
        .find_commit(git2::Oid::from_str(commit).wrap()?)
        .wrap()?;
    repo.branch(&branch_name(name), &target, true).wrap()?;
    Ok(ensure(&root, name)?.path)
}

/// The subjects of the commits on `HEAD` since `from` (exclusive), newest first, at most
/// `limit` — "what landed since the last plan", for the planner's facts. (M132) Empty when
/// `from` is unknown to this repository.
pub fn subjects_since(root: &Path, from: &str, limit: usize) -> Result<Vec<String>> {
    let repo = repo_mod::open(&repo_mod::canonical(root))?;
    let Ok(from) = git2::Oid::from_str(from) else {
        return Ok(Vec::new());
    };
    if repo.find_commit(from).is_err() {
        return Ok(Vec::new());
    }
    let mut walk = repo.revwalk().wrap()?;
    walk.push_head().wrap()?;
    walk.hide(from).wrap()?;
    let mut out = Vec::new();
    for id in walk.take(limit) {
        let commit = repo.find_commit(id.wrap()?).wrap()?;
        let short: String = commit.id().to_string().chars().take(8).collect();
        out.push(format!(
            "{short} {}",
            commit.summary().ok().flatten().unwrap_or_default()
        ));
    }
    Ok(out)
}

/// How many commits `cide/<agent>` has that the project's `HEAD` does not, or `None` when there
/// is no such branch or everything on it is already in.
///
/// The question behind "may this task be set to done": accepting a run's work means taking it
/// into this branch, and a task closed over a branch that never landed is work the board says is
/// finished and the product does not have. That happened (terrastrike's t-1062): the reviewer's
/// `cide_agent_integrate` was denied by the tab's own permission check — not a conflict, so the
/// review prompt's "a conflict means not done" did not obviously cover it — the reviewer set the
/// task to done anyway with "somebody should merge this" in its last comment, and nothing on the
/// board ever looked at that task again. A prompt rule had already failed once, so the answer is
/// a fact the tracker checks rather than a sentence it asks for.
///
/// Read-only and cheap — one revwalk bounded by the merge base — so it can sit in front of every
/// status change. A missing branch is `None` rather than an error: a task-less run, a role with
/// `worktree: false`, or a branch somebody deleted after merging all mean "nothing is waiting".
pub fn unmerged(root: &Path, agent: &str) -> Result<Option<usize>> {
    validate_agent(agent)?;
    let repo = repo_mod::open(&repo_mod::canonical(root))?;
    let Ok(branch) = repo.find_branch(&branch_name(agent), BranchType::Local) else {
        return Ok(None);
    };
    let theirs = branch.into_reference().peel_to_commit().wrap()?.id();
    let ours = repo
        .head()
        .map_err(|_| GitError::Unborn)?
        .peel_to_commit()
        .wrap()?
        .id();
    let mut walk = repo.revwalk().wrap()?;
    walk.push(theirs).wrap()?;
    walk.hide(ours).wrap()?;
    let ahead = walk.count();
    if ahead == 0 {
        return Ok(None);
    }
    // Commits ahead whose *content* is already in: terrastrike's `cide/qa-t-22` is one merge
    // commit of `cide/developer-t-22`, which master took directly. Counting commits alone calls
    // that unmerged for ever, and a gate that fires on work that is plainly in teaches whoever
    // meets it to route around it. So the merge is computed in memory, as `integrate` does,
    // and a clean result equal to `HEAD`'s tree means there is nothing waiting. A conflict is
    // by definition content that is not in.
    let ours_commit = repo.find_commit(ours).wrap()?;
    let theirs_commit = repo.find_commit(theirs).wrap()?;
    let mut index = repo
        .merge_commits(&ours_commit, &theirs_commit, None)
        .wrap()?;
    if !index.has_conflicts() && index.write_tree_to(&repo).wrap()? == ours_commit.tree_id() {
        return Ok(None);
    }
    Ok(Some(ahead))
}

/// What [`remove_if_integrated`] did — and, when it kept the checkout, why. (M89)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Retired {
    /// The checkout is gone; `cide/<agent>` is kept, exactly as [`remove`] keeps it.
    Removed,
    /// There was no checkout to remove.
    Absent,
    /// `cide/<agent>` still holds this many commits the project's `HEAD` lacks.
    Unmerged { commits: usize },
    /// The checkout has this many uncommitted or untracked paths, which removing would destroy.
    Dirty { paths: usize },
    /// The checkout's `HEAD` is not on `cide/<agent>` — somebody checked something else out in
    /// there, and "the branch is merged" says nothing about what that is.
    Elsewhere { branch: String },
}

/// Remove an agent's worktree **only when nothing in it would be lost**. (M89)
///
/// The reason it exists: integrating a task's branch left `.cide/worktrees/<role>-<task>` on
/// disk for ever. Every merged task was a full checkout nobody would open again, and the Agents
/// panel's History offered Integrate beside each one because its worktree was still there.
///
/// Three facts are checked, and any one of them keeps the checkout:
///
/// * **the branch is in** — [`unmerged`] answers `None`, the same content-aware test the task
///   tracker's `done` gate uses, so a branch whose commits were taken some other way counts;
/// * **the tree is clean** — no modified, staged or untracked file (ignored files do not
///   count: they are build output, and `target/` in a Rust checkout is the usual reason this
///   directory is large at all). Uncommitted work is the one thing the branch does *not* hold,
///   and [`remove`]'s own doc says it does not survive;
/// * **`HEAD` is on `cide/<agent>`** — otherwise "merged" is a claim about a branch the checkout
///   is not on.
///
/// Whether a *process* is still standing in the directory is not this crate's question — it has
/// no registry. The caller asks that first (`cide_app::agents`), because deleting the cwd of a
/// live, idle `claude` would leave it running with no directory under it.
pub fn remove_if_integrated(root: &Path, agent: &str) -> Result<Retired> {
    validate_agent(agent)?;
    let root = repo_mod::canonical(root);
    let path = path_of(&root, agent);
    if !path.is_dir() {
        return Ok(Retired::Absent);
    }
    if let Some(commits) = unmerged(&root, agent)? {
        return Ok(Retired::Unmerged { commits });
    }
    let described = describe(agent, &path)?;
    if described.branch != branch_name(agent) {
        return Ok(Retired::Elsewhere {
            branch: described.branch,
        });
    }
    let repo = repo_mod::open(&path)?;
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(false)
        .include_ignored(false)
        .include_unmodified(false);
    let dirty = repo.statuses(Some(&mut opts)).wrap()?.len();
    if dirty > 0 {
        return Ok(Retired::Dirty { paths: dirty });
    }
    // Closed before the prune: libgit2 holds files under the worktree open on some platforms.
    drop(repo);
    remove(&root, agent)?;
    Ok(Retired::Removed)
}

/// How many modified, staged or untracked paths a worktree has — [`remove_if_integrated`]'s own
/// test of "would anything be lost", on its own. `0` when there is no such checkout.
pub fn dirty(root: &Path, agent: &str) -> Result<usize> {
    validate_agent(agent)?;
    let path = path_of(&repo_mod::canonical(root), agent);
    if !path.is_dir() {
        return Ok(0);
    }
    let repo = repo_mod::open(&path)?;
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(false)
        .include_ignored(false)
        .include_unmodified(false);
    Ok(repo.statuses(Some(&mut opts)).wrap()?.len())
}

/// Commit everything uncommitted in a worktree onto its branch, and answer the new commit —
/// `None` when there was nothing to commit. (OpenSpec sessions)
///
/// For Publish and Integrate on a change applied in `.cide/worktrees/spec-<change>`: an agent
/// told to commit as it goes still leaves the last ticked box uncommitted often enough, and a
/// merge or a push of the branch alone would silently leave that edit behind. Everything the
/// worktree's own `.gitignore` does not exclude is taken — the same set `git add -A` takes —
/// because the tree belongs to the run, not to a person with half-staged work in it.
///
/// Refused while the checkout's `HEAD` is not on `cide/<agent>`, [`remove_if_integrated`]'s
/// reason: committing onto whatever somebody checked out in there is not committing the change.
pub fn commit_all(root: &Path, agent: &str, message: &str) -> Result<Option<String>> {
    validate_agent(agent)?;
    let path = path_of(&repo_mod::canonical(root), agent);
    let described = describe(agent, &path)?;
    if described.branch != branch_name(agent) {
        return Err(GitError::Git {
            detail: format!(
                ".cide/worktrees/{agent} is on {}, not {}; commit there yourself",
                described.branch,
                branch_name(agent)
            ),
        });
    }
    let repo = repo_mod::open(&path)?;
    let mut index = repo.index().wrap()?;
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .wrap()?;
    // `add_all` adds and modifies; a deletion is only staged by `update_all`.
    index.update_all(["*"].iter(), None).wrap()?;
    index.write().wrap()?;
    let tree_id = index.write_tree().wrap()?;
    let head = repo.head().wrap()?.peel_to_commit().wrap()?;
    if head.tree_id() == tree_id {
        return Ok(None);
    }
    let tree = repo.find_tree(tree_id).wrap()?;
    let signature = repo
        .signature()
        .or_else(|_| git2::Signature::now("cide", "cide@localhost"))
        .wrap()?;
    let commit = repo
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &[&head],
        )
        .wrap()?;
    Ok(Some(commit.to_string()))
}

/// Move the project root's **untracked** copies of files `cide/<agent>` adds under `dir` out of
/// the way, into `park`, so a merge of that branch can write them. Answers what was moved, as
/// paths relative to the root, for [`unpark`]. (OpenSpec sessions)
///
/// The case it exists for: an OpenSpec change is proposed in the root and, as it usually is, not
/// committed; Apply in a worktree copies the change's folder in (a worktree is cut from `HEAD`)
/// and the session commits it there with its ticks. Merging that branch back then meets the
/// root's own untracked copy of every one of those files, and git refuses the checkout — "N
/// conflicts prevent checkout" — over files the branch is the continuation of. Only a path that
/// is untracked in the root **and** added by the branch is moved; anything tracked, anything the
/// branch does not have, and anything outside `dir` is left exactly where it is.
pub fn park_untracked(root: &Path, agent: &str, dir: &Path, park: &Path) -> Result<Vec<PathBuf>> {
    validate_agent(agent)?;
    let root = repo_mod::canonical(root);
    let repo = repo_mod::open(&root)?;
    let Ok(branch) = repo.find_branch(&branch_name(agent), BranchType::Local) else {
        return Ok(Vec::new());
    };
    let tree = branch.into_reference().peel_to_tree().wrap()?;
    let index = repo.index().wrap()?;
    let mut moved = Vec::new();
    let mut stack = vec![root.join(dir)];
    while let Some(at) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(rel) = path.strip_prefix(&root) else {
                continue;
            };
            let rel = rel.to_path_buf();
            let tracked = index.get_path(&rel, 0).is_some();
            if tracked || tree.get_path(&rel).is_err() {
                continue;
            }
            let target = park.join(&rel);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| GitError::Io {
                    detail: e.to_string(),
                })?;
            }
            std::fs::rename(&path, &target).map_err(|e| GitError::Io {
                detail: e.to_string(),
            })?;
            moved.push(rel);
        }
    }
    Ok(moved)
}

/// Put back what [`park_untracked`] moved — after a merge that did not land. A path the merge has
/// since written is left as the merge wrote it, and its parked copy stays in `park`.
pub fn unpark(root: &Path, park: &Path, moved: &[PathBuf]) {
    let root = repo_mod::canonical(root);
    for rel in moved {
        let at = root.join(rel);
        if at.exists() {
            continue;
        }
        if let Some(parent) = at.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::rename(park.join(rel), &at);
    }
}

/// The line [`ensure`] keeps in the repository's `info/exclude`.
///
/// Anchored with the leading `/`, so it matches `.cide/worktrees/` **at the root of each
/// checkout** and nothing below it — in the project root that is every agent checkout, and in a
/// checkout it is nothing at all (there is no `.cide/worktrees/` inside one), so what a run
/// commits on its branch is exactly what it committed before. `info/exclude` and not
/// `.gitignore`: it is cide's rule, local to this clone, and a `.gitignore` edit is a change
/// somebody would commit. Git reads a linked worktree's excludes from the **common** directory,
/// so one line covers them all. Without it the root's `git status` lists the checkouts as
/// untracked, and a `git add -A` there commits whole copies of the tree.
pub const WORKTREES_EXCLUDE: &str = "/.cide/worktrees/";

/// Add [`WORKTREES_EXCLUDE`] to `<common>/info/exclude` unless it is there.
fn exclude_worktrees(common: &Path) -> std::io::Result<()> {
    let exclude = common.join("info").join("exclude");
    let current = match std::fs::read_to_string(&exclude) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    if current.lines().any(|line| line.trim() == WORKTREES_EXCLUDE) {
        return Ok(());
    }
    if let Some(parent) = exclude.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = current;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("# cide: agent and OpenSpec worktrees\n");
    text.push_str(WORKTREES_EXCLUDE);
    text.push('\n');
    std::fs::write(&exclude, text)
}

// --- internals ------------------------------------------------------------------------------

/// `cide/<agent>`.
fn branch_name(agent: &str) -> String {
    format!("{BRANCH_PREFIX}/{agent}")
}

/// Reject anything that is not `[a-z0-9][a-z0-9-]{0,63}`.
///
/// **Worktree names are not paths.** The name comes from a config file in the user's project
/// and from whatever wrote it — including a model — and it is about to be `Path::join`ed and
/// turned into a ref name. `../../etc` joined onto `.cide/worktrees/` puts a checkout wherever
/// it likes, and `..` in a ref name is a revision range. This is the same class of refusal
/// `cide-app`'s `cmd/file.rs` makes about untrusted paths — shape first, before anything
/// resolves it — and it is deliberately a whitelist, because a blacklist of `..` and `/` still
/// admits a leading `-` that the next `git` invocation reads as a flag, a backslash, a NUL, a
/// name that only differs from another by case (one path on macOS, two here — the thing
/// `check:casing` exists for), and whatever the next filesystem calls special.
///
/// The length is 64 where an agent id alone is capped at 32, because a name here may be
/// `cide_agents::checkout_name`'s composite — a 32-char role plus a task slug — and that
/// composer keeps itself under this cap by construction. The charset is exactly the agent-id
/// grammar, which is what lets one whitelist serve both shapes.
///
/// The refusal reuses [`GitError::InvalidBranchName`] because the name *is* the second
/// segment of `cide/<name>`, so the sentence the UI already shows is true; a worktree-shaped
/// variant would be a wire-contract change for a message that reads the same.
fn validate_agent(agent: &str) -> Result<()> {
    let mut chars = agent.chars();
    let ok = (1..=64).contains(&agent.len())
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !ok {
        return Err(GitError::InvalidBranchName {
            name: agent.to_string(),
        });
    }
    Ok(())
}

/// Read a checkout's own `HEAD` rather than assuming cide's.
fn describe(agent: &str, path: &Path) -> Result<AgentWorktree> {
    let repo = repo_mod::open(path)?;
    let branch = repo
        .head()
        .ok()
        .and_then(|head| head.shorthand().map(str::to_owned).ok())
        .unwrap_or_else(|| "HEAD".to_string());
    let head = repo
        .head()
        .ok()
        .and_then(|head| head.peel_to_commit().ok())
        .map(|commit| commit.id().to_string())
        .unwrap_or_default();
    Ok(AgentWorktree {
        agent: agent.to_string(),
        // Canonical, because libgit2 hands back a prettified directory path (with a trailing
        // separator) and the caller is going to compare this with a path it built itself.
        path: repo_mod::canonical(path),
        branch,
        head,
    })
}

/// Move the working tree onto `tree`, refusing to overwrite anything modified locally.
fn checkout(repo: &Repository, tree: &Tree<'_>) -> Result<()> {
    let mut builder = git2::build::CheckoutBuilder::new();
    // `safe`, never `force`. The whole promise of this module is that an integration the user
    // did not watch cannot eat an edit they made while it ran.
    builder.safe();
    repo.checkout_tree(tree.as_object(), Some(&mut builder))
        .wrap()
}

/// How many files a merge touched, for the sentence the caller shows.
fn changed(repo: &Repository, from: &Tree<'_>, to: &Tree<'_>) -> Result<usize> {
    let diff = repo.diff_tree_to_tree(Some(from), Some(to), None).wrap()?;
    Ok(diff.deltas().len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_checkout_names_its_project_root_and_nothing_else_does() {
        let root = Path::new("/work/game");
        assert_eq!(
            root_of_checkout(&path_of(root, "qa-tester-t-301")).as_deref(),
            Some(root)
        );
        assert_eq!(root_of_checkout(root), None);
        assert_eq!(root_of_checkout(&root.join(".cide/worktrees")), None);
        assert_eq!(root_of_checkout(&root.join("src/worktrees/x")), None);
    }

    /// Run `git` in `dir` with the user's own configuration kept out of it.
    ///
    /// The same helper `repo.rs`'s test module keeps, and for the reason its comment gives:
    /// `tests/support` isolates by pointing `HOME` at a scratch directory once per process,
    /// which a unit test inside the library cannot do without racing every other test in this
    /// binary. Per-command environment is the equivalent that needs no `set_var`.
    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "cide tests")
            .env("GIT_AUTHOR_EMAIL", "tests@cide.invalid")
            .env("GIT_COMMITTER_NAME", "cide tests")
            .env("GIT_COMMITTER_EMAIL", "tests@cide.invalid")
            .output()
            .unwrap_or_else(|e| panic!("running git {args:?}: {e}"));
        assert!(
            out.status.success(),
            "git {args:?} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn scratch(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("cide-git-wt-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch");
        // Canonicalised for the reason spelled out on `repo`'s copy of this helper: macOS
        // resolves `$TMPDIR` through a symlink and the product's answers come back resolved.
        std::fs::canonicalize(&path).unwrap_or(path)
    }

    /// A project with one commit on `main`.
    ///
    /// `user.*` is set in the *local* config because the library under test uses libgit2,
    /// which reads the developer's real `$HOME/.gitconfig` and cannot be told not to from
    /// here; local config wins, so every assertion below is about the repository and not
    /// about the machine. `core.autocrlf` is pinned for the same reason.
    fn project(tag: &str) -> PathBuf {
        let root = scratch(tag);
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.name", "cide tests"]);
        git(&root, &["config", "user.email", "tests@cide.invalid"]);
        git(&root, &["config", "core.autocrlf", "false"]);
        write(&root, "base.txt", "base\n");
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-qm", "base"]);
        root
    }

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).expect("write");
    }

    fn commit(dir: &Path, name: &str, body: &str, message: &str) {
        write(dir, name, body);
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-qm", message]);
    }

    fn head_of(dir: &Path) -> String {
        git(dir, &["rev-parse", "HEAD"]).trim().to_string()
    }

    #[test]
    fn a_linked_checkout_names_its_own_admin_dir_and_the_common_one() {
        let root = project("gitdirs");
        let wt = ensure(&root, "developer-t-1").expect("ensure");
        let dirs = git_dirs(&wt.path).expect("git dirs");
        let expect = |args: &[&str]| {
            let out = git(&wt.path, args);
            repo_mod::canonical(&wt.path.join(out.trim()))
        };
        assert_eq!(dirs.git_dir, expect(&["rev-parse", "--git-dir"]));
        assert_eq!(dirs.common_dir, expect(&["rev-parse", "--git-common-dir"]));
        assert_eq!(dirs.common_dir, repo_mod::canonical(&root.join(".git")));
        assert_ne!(
            dirs.git_dir, dirs.common_dir,
            "a linked worktree has its own"
        );

        let plain = git_dirs(&root).expect("the root");
        assert_eq!(plain.git_dir, plain.common_dir);
    }

    /// Publish and Integrate commit what a session left behind — new, modified and deleted
    /// files — onto the checkout's branch, and a clean checkout commits nothing.
    #[test]
    fn commit_all_takes_everything_the_session_left_and_nothing_twice() {
        let root = project("commit-all");
        let wt = ensure(&root, "spec-c1").expect("ensure");
        let project_head = head_of(&root);
        assert_eq!(dirty(&root, "spec-c1").unwrap(), 0);
        assert_eq!(commit_all(&root, "spec-c1", "OpenSpec: c1").unwrap(), None);

        write(&wt.path, "new.txt", "new\n");
        write(&wt.path, "base.txt", "changed\n");
        assert_eq!(dirty(&root, "spec-c1").unwrap(), 2);
        let made = commit_all(&root, "spec-c1", "OpenSpec: c1")
            .unwrap()
            .expect("a commit");
        assert_eq!(head_of(&wt.path), made);
        assert_eq!(dirty(&root, "spec-c1").unwrap(), 0, "nothing left behind");
        assert_eq!(
            git(&root, &["log", "-1", "--format=%s", "cide/spec-c1"]).trim(),
            "OpenSpec: c1"
        );
        assert_eq!(
            head_of(&root),
            project_head,
            "the project's own branch is untouched"
        );

        std::fs::remove_file(wt.path.join("new.txt")).unwrap();
        commit_all(&root, "spec-c1", "OpenSpec: c1")
            .unwrap()
            .expect("a deletion is a change too");
        assert!(
            git(&wt.path, &["ls-files"])
                .lines()
                .all(|line| line != "new.txt"),
            "the deletion was committed"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The OpenSpec case: an uncommitted change folder in the root, carried into the worktree and
    /// committed there. The root's untracked copies are parked so the merge can land, a tracked
    /// file and a file the branch lacks stay, and unparking after a failed merge restores them.
    #[test]
    fn untracked_copies_the_branch_adds_are_parked_and_only_those() {
        let root = project("park");
        let wt = ensure(&root, "spec-c1").expect("ensure");
        std::fs::create_dir_all(wt.path.join("openspec/changes/c1")).unwrap();
        write(
            &wt.path.join("openspec/changes/c1"),
            "tasks.md",
            "- [x] one\n",
        );
        git(&wt.path, &["add", "-A"]);
        git(&wt.path, &["commit", "-qm", "work"]);

        std::fs::create_dir_all(root.join("openspec/changes/c1")).unwrap();
        write(&root.join("openspec/changes/c1"), "tasks.md", "- [ ] one\n");
        write(&root.join("openspec/changes/c1"), "notes.md", "mine\n");

        let park = root.join(".cide/spec-carried/c1");
        let moved =
            park_untracked(&root, "spec-c1", Path::new("openspec/changes/c1"), &park).unwrap();
        assert_eq!(moved, vec![PathBuf::from("openspec/changes/c1/tasks.md")]);
        assert!(!root.join("openspec/changes/c1/tasks.md").exists());
        assert!(
            root.join("openspec/changes/c1/notes.md").exists(),
            "not on the branch: kept"
        );

        unpark(&root, &park, &moved);
        assert_eq!(
            std::fs::read_to_string(root.join("openspec/changes/c1/tasks.md")).unwrap(),
            "- [ ] one\n"
        );

        let moved =
            park_untracked(&root, "spec-c1", Path::new("openspec/changes/c1"), &park).unwrap();
        assert!(matches!(
            integrate(&root, "spec-c1").unwrap(),
            Integration::Merged { .. }
        ));
        assert_eq!(moved.len(), 1);
        assert_eq!(
            std::fs::read_to_string(root.join("openspec/changes/c1/tasks.md")).unwrap(),
            "- [x] one\n",
            "the branch's copy, with its ticks, landed"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The checkouts are hidden from the root's `git status`, once, and what a checkout commits
    /// is unchanged — the pattern is anchored, so inside a checkout it matches nothing.
    #[test]
    fn ensure_hides_the_checkouts_from_the_root_and_nothing_from_a_checkout() {
        let root = project("exclude");
        let wt = ensure(&root, "spec-c1").expect("ensure");
        ensure(&root, "spec-c2").expect("a second");
        let exclude = std::fs::read_to_string(root.join(".git/info/exclude")).unwrap();
        assert_eq!(
            exclude
                .lines()
                .filter(|l| l.trim() == WORKTREES_EXCLUDE)
                .count(),
            1,
            "written once: {exclude}"
        );
        assert!(
            !git(&root, &["status", "--porcelain"]).contains(".cide/worktrees"),
            "the root does not list its checkouts"
        );
        write(&wt.path, "work.txt", "done\n");
        assert!(commit_all(&root, "spec-c1", "work").unwrap().is_some());
        assert!(
            git(&wt.path, &["ls-files"])
                .lines()
                .any(|l| l == "work.txt"),
            "a checkout's own files commit as before"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ensure_twice_returns_the_same_worktree() {
        let root = project("idempotent");

        let first = ensure(&root, "developer").expect("first ensure");
        let second = ensure(&root, "developer").expect("second ensure");

        assert_eq!(first, second, "ensure is called before every dispatch");
        assert_eq!(first.branch, "cide/developer");
        assert_eq!(
            first.head,
            head_of(&root),
            "branched from the project's HEAD"
        );
        assert_eq!(first.path, root.join(".cide/worktrees/developer"));
        assert!(
            first.path.join("base.txt").is_file(),
            "and it is checked out"
        );
        assert_eq!(
            git(&root, &["worktree", "list", "--porcelain"])
                .matches("worktree ")
                .count(),
            2,
            "the project itself plus one, never a second registration"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_agent_name_that_is_a_path_is_refused_before_anything_is_joined() {
        let root = project("names");

        for name in [
            "../../etc",
            "..",
            "a/b",
            "Developer",
            "dev ops",
            "",
            "-dev",
            "dev\0",
            // 64 is the cap, not 32: a name may be `checkout_name`'s role-plus-task composite.
            &"x".repeat(65),
        ] {
            assert_eq!(
                ensure(&root, name),
                Err(GitError::InvalidBranchName {
                    name: name.to_string()
                }),
                "{name:?} must never reach a Path::join"
            );
            assert!(remove(&root, name).is_err(), "{name:?} in remove");
            assert!(integrate(&root, name).is_err(), "{name:?} in integrate");
        }
        assert!(
            !root.join(".cide").exists(),
            "a refused name creates nothing at all"
        );

        for name in [
            "dev",
            "qa2",
            "a",
            "code-reviewer",
            // A per-task composite and the cap itself, which composites may reach.
            "developer-t-62",
            &"x".repeat(64),
        ] {
            assert!(validate_agent(name).is_ok(), "{name:?} is a fine agent id");
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn list_reports_what_ensure_made_and_nothing_else() {
        let root = project("list");
        assert_eq!(list(&root).expect("empty list"), Vec::new());

        ensure(&root, "qa").expect("qa");
        ensure(&root, "developer").expect("developer");
        // A worktree of the user's own, outside `.cide/`: theirs, and not an agent's.
        let theirs = scratch("list-theirs").join("hotfix-checkout");
        git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                theirs.to_str().unwrap(),
                "-b",
                "hotfix",
            ],
        );

        let listed = list(&root).expect("list");
        assert_eq!(
            listed.iter().map(|w| w.agent.as_str()).collect::<Vec<_>>(),
            ["developer", "qa"],
            "sorted by agent, and the user's own worktree is not an agent"
        );
        assert_eq!(listed[0].branch, "cide/developer");
        assert_eq!(listed[1].path, root.join(".cide/worktrees/qa"));

        let _ = std::fs::remove_dir_all(theirs.parent().expect("parent"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The worktree selectors' trust boundary: a checkout `ensure` made resolves by id and by
    /// path from the project's roots — including a root that is a subdirectory of the repository
    /// — while the root's own id, a user's worktree and an arbitrary directory never do.
    #[test]
    fn find_and_resolve_answer_only_for_agent_worktrees() {
        let root = project("find");
        std::fs::create_dir_all(root.join("sub")).expect("sub");
        let made = ensure(&root, "developer-t-7").expect("ensure");
        let theirs = scratch("find-theirs").join("hotfix");
        git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                theirs.to_str().unwrap(),
                "-b",
                "hf",
            ],
        );

        for roots in [vec![root.clone()], vec![root.join("sub")]] {
            let info = find(&roots, repo_mod::repo_id(&made.path)).expect("found by id");
            assert_eq!(info.root, made.path);
            assert_eq!(info.name, "developer-t-7");
            assert!(info.parent.is_none() && !info.is_submodule);
            assert_eq!(resolve(&roots, &made.path), Some(made.path.clone()));
            assert_eq!(
                list_for(&roots).len(),
                1,
                "one root, one checkout, listed once"
            );
        }
        let roots = vec![root.clone()];
        assert!(
            find(&roots, repo_mod::repo_id(&root)).is_none(),
            "not the root"
        );
        assert!(
            find(&roots, repo_mod::repo_id(&theirs)).is_none(),
            "not theirs"
        );
        assert_eq!(resolve(&roots, &theirs), None);
        assert_eq!(resolve(&roots, &root.join("sub")), None);
        assert_eq!(
            list_for(&[root.clone(), root.clone()]).len(),
            1,
            "a repeated root is one"
        );

        let _ = std::fs::remove_dir_all(theirs.parent().expect("parent"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ensure_repairs_a_registration_whose_directory_was_deleted() {
        let root = project("prunable");
        let made = ensure(&root, "developer").expect("ensure");
        commit(&made.path, "agent.txt", "work\n", "agent work");
        let work = head_of(&made.path);

        // What `rm -rf` leaves: `.git/worktrees/developer` still registered, nothing on disk.
        std::fs::remove_dir_all(&made.path).expect("delete the checkout");
        assert!(
            git(&root, &["worktree", "list", "--porcelain"]).contains("prunable"),
            "the state under test is git's own 'prunable'"
        );

        let again = ensure(&root, "developer").expect("ensure repairs it");
        assert_eq!(again.path, made.path);
        assert_eq!(again.branch, "cide/developer");
        assert_eq!(
            again.head, work,
            "the branch is reused, so the commits made before the directory died survive"
        );
        assert!(again.path.join("agent.txt").is_file());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ensure_refuses_a_directory_it_did_not_make() {
        let root = project("occupied");
        let path = root.join(".cide/worktrees/developer");
        std::fs::create_dir_all(&path).expect("mkdir");
        write(&path, "notes.txt", "somebody's file\n");

        let error = ensure(&root, "developer").expect_err("a non-empty directory is refused");
        assert!(
            matches!(&error, GitError::Io { detail } if detail.contains("move it aside")),
            "{error:?}"
        );
        assert!(path.join("notes.txt").is_file(), "and nothing is deleted");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn remove_keeps_the_branch_the_work_is_on() {
        let root = project("remove");
        let made = ensure(&root, "developer").expect("ensure");
        commit(&made.path, "agent.txt", "work\n", "agent work");
        let work = head_of(&made.path);

        remove(&root, "developer").expect("remove");
        assert!(!made.path.exists(), "the checkout is gone");
        assert_eq!(list(&root).expect("list"), Vec::new());
        assert_eq!(
            git(&root, &["rev-parse", "cide/developer"]).trim(),
            work,
            "and the branch still holds the commits — it is all that does"
        );
        remove(&root, "developer").expect("removing twice is not an error");

        // The work is still integrable, which is the point of keeping the branch.
        assert!(matches!(
            integrate(&root, "developer"),
            Ok(Integration::Merged { .. })
        ));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_merged_clean_worktree_is_retired_and_anything_else_is_kept() {
        let root = project("retire");
        let made = ensure(&root, "developer").expect("ensure");
        commit(&made.path, "agent.txt", "work\n", "agent work");

        assert_eq!(
            remove_if_integrated(&root, "developer").expect("unmerged"),
            Retired::Unmerged { commits: 1 },
            "work the base lacks keeps the checkout"
        );
        assert!(matches!(
            integrate(&root, "developer"),
            Ok(Integration::Merged { .. })
        ));

        write(&made.path, "scratch.txt", "not committed\n");
        assert_eq!(
            remove_if_integrated(&root, "developer").expect("dirty"),
            Retired::Dirty { paths: 1 },
            "an untracked file is work the branch does not hold"
        );
        assert!(made.path.is_dir(), "and nothing was deleted");
        std::fs::remove_file(made.path.join("scratch.txt")).expect("clean up");

        assert_eq!(
            remove_if_integrated(&root, "developer").expect("retire"),
            Retired::Removed
        );
        assert!(!made.path.exists(), "the checkout is gone");
        assert!(
            !git(&root, &["rev-parse", "cide/developer"])
                .trim()
                .is_empty(),
            "and the branch is kept, as `remove` keeps it"
        );
        assert_eq!(
            remove_if_integrated(&root, "developer").expect("twice"),
            Retired::Absent
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn integrate_with_no_commits_is_up_to_date() {
        let root = project("up-to-date");
        ensure(&root, "developer").expect("ensure");
        let before = head_of(&root);

        assert_eq!(
            integrate(&root, "developer").expect("integrate"),
            Integration::UpToDate
        );
        assert_eq!(head_of(&root), before, "and nothing moved");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The gate `cide_task_update` puts in front of `done`: nothing on a missing or fully merged
    /// branch, a count on one that is ahead — including after the base moved on, which is the
    /// case a plain `HEAD == branch` comparison would get wrong in both directions.
    #[test]
    fn unmerged_counts_only_what_the_base_lacks() {
        let root = project("unmerged");
        assert_eq!(unmerged(&root, "developer-t-1").expect("no branch"), None);

        let made = ensure(&root, "developer-t-1").expect("ensure");
        assert_eq!(unmerged(&root, "developer-t-1").expect("fresh"), None);

        commit(&made.path, "a.txt", "a\n", "one");
        commit(&made.path, "b.txt", "b\n", "two");
        commit(&root, "base2.txt", "moved\n", "the base moves on");
        assert_eq!(unmerged(&root, "developer-t-1").expect("ahead"), Some(2));

        assert!(matches!(
            integrate(&root, "developer-t-1").expect("integrate"),
            Integration::Merged { .. }
        ));
        assert_eq!(unmerged(&root, "developer-t-1").expect("merged"), None);

        // Commits ahead whose content the base already has — a branch that only merged
        // another one master took directly — is nothing waiting, not one commit unmerged.
        let qa = ensure(&root, "qa-t-1").expect("ensure qa");
        git(
            &qa.path,
            &["commit", "-q", "--allow-empty", "-m", "nothing new"],
        );
        assert_eq!(unmerged(&root, "qa-t-1").expect("empty commit"), None);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn integrate_fast_forwards_when_the_base_has_not_moved() {
        let root = project("fast-forward");
        let made = ensure(&root, "developer").expect("ensure");
        commit(&made.path, "agent.txt", "work\n", "agent work");
        let tip = head_of(&made.path);

        let outcome = integrate(&root, "developer").expect("integrate");
        assert_eq!(
            outcome,
            Integration::Merged {
                commit: tip.clone(),
                files: 1
            },
            "a fast-forward is still a merge, and names the commit it moved to"
        );
        assert_eq!(head_of(&root), tip);
        assert!(
            root.join("agent.txt").is_file(),
            "the working tree moved too"
        );
        assert_eq!(
            git(&root, &["status", "--porcelain", "--untracked-files=no"]),
            "",
            "and the index came with it"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Three branches, one conflicting with an earlier one: the other two compose, nothing moves
    /// until `advance_to`, and the scratch checkout holds the combined head. (M132)
    #[test]
    fn a_batch_composes_in_memory_and_skips_the_conflict() {
        let root = project("batch");
        let a = ensure(&root, "dev-t-1").expect("ensure");
        commit(&a.path, "a.txt", "a\n", "a");
        let b = ensure(&root, "dev-t-2").expect("ensure");
        commit(&b.path, "b.txt", "b\n", "b");
        let c = ensure(&root, "dev-t-3").expect("ensure");
        commit(&c.path, "a.txt", "not a\n", "c");
        let before = head_of(&root);

        let composed = compose_merges(&root, &["dev-t-1", "dev-t-2", "dev-t-3"]).expect("compose");
        assert_eq!(composed.base, before);
        assert_eq!(composed.merged, ["dev-t-1", "dev-t-2"]);
        assert_eq!(composed.conflicts.len(), 1, "{composed:?}");
        assert_eq!(composed.conflicts[0].0, "dev-t-3");
        assert_eq!(head_of(&root), before, "composing moves nothing");
        assert!(!root.join("a.txt").exists());

        let head = composed.head.expect("a combined head");
        let scratch = scratch_at(&root, "batch-verify", &head).expect("scratch");
        assert!(scratch.join("a.txt").is_file() && scratch.join("b.txt").is_file());

        let files = advance_to(&root, &composed.base, &head).expect("advance");
        assert_eq!(files, 2);
        assert_eq!(head_of(&root), head);
        assert!(root.join("b.txt").is_file());
        assert!(
            advance_to(&root, &composed.base, &head).is_err(),
            "a branch that moved since the compose is refused"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_clean_divergent_change_merges() {
        let root = project("divergent");
        let made = ensure(&root, "developer").expect("ensure");
        commit(&made.path, "agent.txt", "work\n", "agent work");
        commit(&root, "user.txt", "mine\n", "user work");
        let base_before = head_of(&root);

        let Integration::Merged { commit, files } =
            integrate(&root, "developer").expect("integrate")
        else {
            panic!("a divergent but non-overlapping change merges");
        };
        assert_eq!(files, 1, "one file arrived from the agent's side");
        assert_eq!(head_of(&root), commit);
        assert_eq!(
            git(&root, &["rev-list", "--parents", "-n", "1", "HEAD"])
                .split_whitespace()
                .count(),
            3,
            "a real merge commit with two parents"
        );
        assert!(root.join("agent.txt").is_file() && root.join("user.txt").is_file());
        assert_eq!(
            git(&root, &["rev-parse", "HEAD^1"]).trim(),
            base_before,
            "ours first"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn two_agents_editing_one_line_conflict_and_the_base_is_untouched() {
        let root = project("conflict");
        commit(&root, "shared.txt", "one\ntwo\nthree\n", "shared");

        let alpha = ensure(&root, "alpha").expect("alpha");
        let beta = ensure(&root, "beta").expect("beta");
        commit(&alpha.path, "shared.txt", "one\nALPHA\nthree\n", "alpha");
        commit(&beta.path, "shared.txt", "one\nBETA\nthree\n", "beta");

        assert!(matches!(
            integrate(&root, "alpha").expect("alpha integrates"),
            Integration::Merged { .. }
        ));

        let before = head_of(&root);
        let outcome = integrate(&root, "beta").expect("the refusal is not an error");
        assert_eq!(
            outcome,
            Integration::Conflicts {
                paths: vec!["shared.txt".to_string()]
            },
            "the conflicting path is named"
        );

        assert_eq!(head_of(&root), before, "nothing was committed");
        assert_eq!(
            std::fs::read_to_string(root.join("shared.txt")).expect("read"),
            "one\nALPHA\nthree\n",
            "no conflict markers were written into the user's checkout"
        );
        assert_eq!(
            git(&root, &["status", "--porcelain", "--untracked-files=no"]),
            "",
            "and the index is clean — a refusal that touched nothing"
        );
        assert_eq!(
            git(&root, &["rev-parse", "cide/beta"]).trim(),
            head_of(&beta.path),
            "beta's work is still on beta's branch"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn integrate_refuses_while_the_base_is_mid_merge() {
        let root = project("mid-merge");
        commit(&root, "shared.txt", "one\n", "shared");
        git(&root, &["checkout", "-q", "-b", "side"]);
        commit(&root, "shared.txt", "side\n", "side");
        git(&root, &["checkout", "-q", "main"]);
        commit(&root, "shared.txt", "main\n", "main");
        ensure(&root, "developer").expect("ensure");

        // A merge left conflicted on purpose: `git merge` exits non-zero, so not `git()`.
        let merge = std::process::Command::new("git")
            .current_dir(&root)
            .args(["merge", "side"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git merge");
        assert!(!merge.status.success(), "the merge was meant to conflict");

        assert_eq!(
            integrate(&root, "developer"),
            Err(GitError::OperationInProgress {
                operation: "merge".to_string()
            }),
            "a second merge through a half-finished one is how work is lost"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_repository_with_no_configured_identity_can_still_integrate() {
        let root = project("no-identity");
        let made = ensure(&root, "developer").expect("ensure");
        commit(&made.path, "agent.txt", "work\n", "agent work");
        commit(&root, "user.txt", "mine\n", "user work");

        // Empty, not unset: libgit2 reads `$HOME/.gitconfig` and there is no way to point it
        // elsewhere from in-process, but an empty `user.email` makes `signature()` fail on
        // every machine, which is the state a fresh container is in.
        git(&root, &["config", "user.name", ""]);
        git(&root, &["config", "user.email", ""]);

        assert!(matches!(
            integrate(&root, "developer").expect("integration must not need git configured"),
            Integration::Merged { .. }
        ));
        assert_eq!(
            git(&root, &["log", "-1", "--format=%an <%ae>"]).trim(),
            "cide <cide@localhost>",
            "and it says plainly that cide made the commit"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}

/// **The one fact Claude Code subagent discovery rests on.** (M30)
///
/// A run dispatched to a `.claude/agents/` subagent is spawned with `--agent <name>`, and the CLI
/// resolves a *project* subagent by walking up from the child's working directory. That works
/// only because a run's checkout is **inside** the project root: the walk goes
/// `.cide/worktrees/<name>` → `.cide/worktrees` → `.cide` → `<root>`, and finds
/// `<root>/.claude/agents/` on the last step.
///
/// Moving worktrees to a sibling directory, a temp dir or an XDG state path would break every
/// project-scoped subagent with **no error anywhere** — the CLI would simply not find the
/// definition and the run would come up as the default agent, doing plausible work under the
/// wrong brief. Pinned here rather than in `cide-agents` because this is the module that decides
/// it.
#[cfg(test)]
mod discovery_containment {
    use super::*;

    #[test]
    fn a_worktree_lives_under_the_project_root() {
        let root = Path::new("/repo");
        let path = root.join(WORKTREES_DIR).join("code-reviewer-t-1");
        assert!(
            path.starts_with(root),
            "{} must be under {}, or `claude --agent` cannot see `.claude/agents/`",
            path.display(),
            root.display()
        );
        assert!(
            !WORKTREES_DIR.starts_with('/') && !WORKTREES_DIR.contains(".."),
            "`{WORKTREES_DIR}` must stay a relative path inside the root"
        );
    }
}
