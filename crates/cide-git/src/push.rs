//! Pushing, by two routes.
//!
//! libgit2 cannot drive an interactive credential helper. Its `credentials` callback is a
//! pull-based "give me a username and password" and there is no way to hand control to
//! `git-credential-libsecret`, a GPG pinentry or a browser-based OAuth helper from inside it.
//! So: **if a credential helper is configured, or the remote speaks HTTP(S), the `git`
//! binary does the push.** It inherits the user's whole configuration — helpers, `ssh`
//! options, proxies, `url.*.insteadOf` — and its stderr is what the user is used to reading.
//!
//! git2 keeps the remaining case, a local or `file://` remote, where there is nothing to
//! authenticate. That is not a token gesture: it is the path the tests exercise, and it means
//! a push inside a monorepo of local clones does not fork a process.

use std::path::Path;
use std::process::Command;

use cide_ipc::git::{GitError, PushOutcome};
use git2::{Remote, Repository};

use crate::{Result, Wrap, repo as repo_mod, status};

/// Which mechanism a push will use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Fork `git push`.
    Binary,
    /// Drive libgit2 directly.
    Libgit2,
}

/// Push `refspec` to `remote`.
///
/// `remote` defaults to the branch's upstream remote, else `origin`. `refspec` defaults to
/// pushing the current branch to a branch of the same name.
pub fn push(
    root: &Path,
    remote: Option<&str>,
    refspec: Option<&str>,
    set_upstream: bool,
) -> Result<PushOutcome> {
    let repo = repo_mod::open(root)?;
    let branch = status::branch_info(&repo)?;
    if branch.unborn {
        return Err(GitError::Unborn);
    }

    let remote_name = match remote {
        Some(name) => name.to_string(),
        None => default_remote(&repo, &branch.upstream),
    };
    let refspec = match refspec {
        Some(spec) => spec.to_string(),
        None => format!("refs/heads/{0}:refs/heads/{0}", branch.head),
    };

    let route = route(&repo, &remote_name);
    match route {
        Route::Binary => push_via_binary(root, &remote_name, &refspec, set_upstream),
        Route::Libgit2 => {
            push_via_libgit2(&repo, &remote_name, &refspec, set_upstream, &branch.head)
        }
    }
}

/// Which route a push to `remote` would take.
///
/// Exposed so the UI can say "this will run `git push`" before it does, and so the tests can
/// assert the decision without performing a push.
pub fn route(repo: &Repository, remote: &str) -> Route {
    if has_credential_helper(repo) {
        return Route::Binary;
    }
    let url: String = repo
        .find_remote(remote)
        .ok()
        .map(|r| String::from_utf8_lossy(r.url_bytes()).into_owned())
        .unwrap_or_default();
    if url.starts_with("http://") || url.starts_with("https://") {
        return Route::Binary;
    }
    // ssh:// and `user@host:path` reach an agent or an on-disk key, both of which libgit2
    // *can* use — but a passphrase prompt is interactive and it cannot. The binary handles
    // both, so anything that is not plainly local goes to it.
    if url.starts_with("file://") || url.starts_with('/') || url.starts_with('.') || url.is_empty()
    {
        return Route::Libgit2;
    }
    Route::Binary
}

/// Whether any `credential.helper` is configured at any level.
///
/// `Config::get_entry` reports only the highest-priority value; the multivar iteration is
/// what catches a helper set in `/etc/gitconfig` and left alone by the user.
fn has_credential_helper(repo: &Repository) -> bool {
    let Ok(config) = repo.config() else {
        return false;
    };
    let Ok(entries) = config.multivar("credential.helper", None) else {
        return false;
    };
    let mut found = false;
    // `for_each` rather than collecting: the callback returns `()` and any entry at all
    // answers the question.
    let _ = entries.for_each(|_| found = true);
    found
}

fn default_remote(repo: &Repository, upstream: &Option<String>) -> String {
    if let Some(upstream) = upstream
        && let Some((remote, _)) = upstream.split_once('/')
        && repo.find_remote(remote).is_ok()
    {
        return remote.to_string();
    }
    "origin".to_string()
}

fn push_via_binary(
    root: &Path,
    remote: &str,
    refspec: &str,
    set_upstream: bool,
) -> Result<PushOutcome> {
    let mut command = Command::new("git");
    command.current_dir(root).arg("push");
    if set_upstream {
        command.arg("--set-upstream");
    }
    command.arg(remote).arg(refspec);
    // Porcelain output on stdout, human messages on stderr. Both are shown, because the
    // remote's own text — a review URL, a pre-receive rejection — arrives on stderr and it is
    // the whole reason a user reads a push result.
    let output = command.output().wrap()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return Err(GitError::Push { output: text });
    }
    Ok(PushOutcome {
        remote: remote.to_string(),
        refspec: refspec.to_string(),
        shelled_out: true,
        output: text,
    })
}

fn push_via_libgit2(
    repo: &Repository,
    remote_name: &str,
    refspec: &str,
    set_upstream: bool,
    branch: &str,
) -> Result<PushOutcome> {
    let mut remote: Remote<'_> = repo.find_remote(remote_name).wrap()?;
    let mut messages = String::new();
    {
        let mut callbacks = git2::RemoteCallbacks::new();
        // The remote's own text: `remote: ...` lines, which is where a server-side hook
        // explains a rejection.
        callbacks.sideband_progress(|bytes| {
            messages.push_str(&String::from_utf8_lossy(bytes));
            true
        });
        let mut options = git2::PushOptions::new();
        options.remote_callbacks(callbacks);
        remote
            .push(&[refspec], Some(&mut options))
            .map_err(|e| GitError::Push {
                output: format!("{:?}: {}", e.class(), e.message()),
            })?;
    }

    if set_upstream {
        set_branch_upstream(repo, branch, remote_name, refspec)?;
    }
    Ok(PushOutcome {
        remote: remote_name.to_string(),
        refspec: refspec.to_string(),
        shelled_out: false,
        output: messages,
    })
}

/// Record `branch.<name>.remote` / `.merge`, which is what `--set-upstream` means.
///
/// libgit2's push does not do this, and `Branch::set_upstream` needs the remote-tracking ref
/// to already exist locally — which it does not until the next fetch. Writing the two config
/// keys is what `git push -u` itself does.
fn set_branch_upstream(repo: &Repository, branch: &str, remote: &str, refspec: &str) -> Result<()> {
    let destination = refspec
        .split_once(':')
        .map(|(_, dst)| dst)
        .unwrap_or(refspec);
    let mut config = repo.config().wrap()?;
    config
        .set_str(&format!("branch.{branch}.remote"), remote)
        .wrap()?;
    config
        .set_str(&format!("branch.{branch}.merge"), destination)
        .wrap()
}
