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

/// The ref namespace agent branches live in. `cide/` rather than a bare name so `git branch`
/// groups them, and so a user's own `developer` branch is never the one an agent commits to.
const BRANCH_PREFIX: &str = "cide";

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
