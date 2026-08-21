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
//!
//! # The proxy, and the question that had to be answered rather than assumed
//!
//! [`push`] and [`crate::branch::fetch`] take a [`ProxyEnv`], which is cide's answer for
//! `ProxyScope::git`. Applying it to the forked `git` is easy; the interesting part was
//! whether it is *sufficient* — whether libgit2, running in this process, could reach a proxy
//! by a route the environment of a child process does not control.
//!
//! **It cannot, twice over, and both halves were read out of the vendored source rather than
//! recalled.** `libgit2-sys 0.18.7+1.9.6` is libgit2 1.9.6.
//!
//! 1. libgit2 consults the environment only under `GIT_PROXY_AUTO`. `lookup_proxy`
//!    (`src/libgit2/transports/http.c:303`) switches on `proxy_opts.type`: `SPECIFIED` uses
//!    the given URL, `AUTO` calls `git_remote__http_proxy` — which is the function that reads
//!    `http_proxy`/`https_proxy`, their upper-case twins and `no_proxy`
//!    (`src/libgit2/remote.c:1139`) — and **`default:` returns 0 with `*out_use = false`**.
//!    `GIT_PROXY_NONE` is the first member of `git_proxy_t` (`include/git2/proxy.h:33`) and so
//!    is 0, `GIT_PROXY_OPTIONS_INIT` is `{GIT_PROXY_OPTIONS_VERSION}` (`proxy.h:90`) and so
//!    leaves the kind at 0, git2-rs's `ProxyOptions` is `#[derive(Default)]` with only
//!    `.auto()`/`.url()` moving it off 0, and this crate constructs one nowhere at all. The
//!    `fetch` below passes `None` for its options, which sends `git_remote_fetch` through
//!    `GIT_REMOTE_CONNECT_OPTIONS_INIT` — kind 0 again.
//! 2. And it would not matter if it did: [`route`] never sends network traffic to libgit2.
//!    Only `file://`, an absolute path, a relative path or an empty URL take the git2 route;
//!    every `http(s)://`, every `ssh://`, every `user@host:path`, and anything at all in a
//!    repository with a credential helper, forks the binary. That is asserted by
//!    [`crate::push::tests`] rather than left as prose, because it is the load-bearing half:
//!    it is what makes a child's environment the *whole* of cide's influence over git's
//!    network behaviour.
//!
//! The consequence is the one that shapes [`cide_ipc::ProxyTarget`]: because the forked `git`
//! inherits **cide's own** environment, a cide launched from a shell that exports
//! `HTTPS_PROXY` already proxies every push. "cide does not add a proxy" is therefore not the
//! same sentence as "git does not use one", and only an explicit `env_remove` — which is what
//! `ProxyTarget::Direct` resolves to — can make the second one true.
//!
//! What no amount of environment can promise: `git` still honours `http.proxy` from
//! `~/.gitconfig`, and cide does not edit the user's gitconfig. The UI says so.

use std::path::Path;
use std::process::Command;

use cide_core::proxy::ProxyEnv;
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
///
/// `proxy` is what cide will do to the forked `git`'s proxy environment — see the module
/// docs. An explicit parameter rather than a global or a `WorkspaceState` lookup, because
/// this crate takes no configuration and reading one would give it a dependency on the app's
/// state for the sake of one variable list. [`ProxyEnv::default`] is "touch nothing", which
/// is what every caller outside the app wants and what this crate did before it existed.
pub fn push(
    root: &Path,
    remote: Option<&str>,
    refspec: Option<&str>,
    set_upstream: bool,
    proxy: &ProxyEnv,
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

    // Read **before** the push, and that ordering is the whole design of these three numbers.
    // Afterwards the remote-tracking ref has moved to the tip and `graph_ahead_behind` would
    // answer zero for every successful push. The alternative — parsing them out of git's
    // output — is refused by this module's own rule: on the binary route that text is the
    // remote server's, it is shown verbatim, and nothing in cide may branch on it.
    let report = ahead_of_remote(&repo, &remote_name, &refspec);

    let route = route(&repo, &remote_name);
    let outcome = match route {
        Route::Binary => push_via_binary(root, &remote_name, &refspec, set_upstream, proxy),
        // No proxy is applied here, and none is needed: this arm is reached only for a local
        // or `file://` remote, and libgit2 would ignore the environment even if it were. See
        // the module docs, which cite the source for both halves.
        Route::Libgit2 => {
            push_via_libgit2(&repo, &remote_name, &refspec, set_upstream, &branch.head)
        }
    };
    outcome.map(|out| PushOutcome {
        ..report.into_outcome(out)
    })
}

/// What the push is about to send, measured against the remote-tracking ref.
///
/// Empty for a refspec this cannot describe — a deletion, a tag, anything whose destination is
/// not `refs/heads/<name>`. That is deliberately silent: the caller still gets a `PushOutcome`
/// and the frontend still has git's own text; what it loses is a count it could not have
/// stated truthfully anyway.
#[derive(Default)]
struct PushReport {
    branch: String,
    pushed: u32,
    old_oid: String,
    new_oid: String,
}

impl PushReport {
    fn into_outcome(self, base: PushOutcome) -> PushOutcome {
        PushOutcome {
            branch: self.branch,
            pushed: self.pushed,
            old_oid: self.old_oid,
            new_oid: self.new_oid,
            ..base
        }
    }
}

fn ahead_of_remote(repo: &Repository, remote: &str, refspec: &str) -> PushReport {
    // `src:dst`, or one name meaning both. A leading `+` is `--force`, which changes nothing
    // about what is being counted.
    let (src, dst) = match refspec.split_once(':') {
        Some((src, dst)) => (src.trim_start_matches('+'), dst),
        None => {
            let one = refspec.trim_start_matches('+');
            (one, one)
        }
    };
    // A deletion pushes an empty source. Nothing to count and nothing to name.
    let Some(name) = dst.strip_prefix("refs/heads/") else {
        return PushReport::default();
    };
    let Some(local) = repo
        .revparse_single(src)
        .ok()
        .and_then(|o| o.peel_to_commit().ok())
    else {
        return PushReport::default();
    };

    let tracking = repo
        .find_reference(&format!("refs/remotes/{remote}/{name}"))
        .ok()
        .and_then(|r| r.target());

    match tracking {
        Some(old) => {
            let (ahead, _) = repo.graph_ahead_behind(local.id(), old).unwrap_or((0, 0));
            PushReport {
                branch: name.to_string(),
                pushed: ahead as u32,
                old_oid: short_oid(old),
                new_oid: short_oid(local.id()),
            }
        }
        // No remote-tracking ref: `git push -u` publishing a branch the remote has never
        // seen. `old_oid` stays empty, which is how the frontend tells *published* from
        // *pushed* without a second boolean.
        //
        // Counted by hiding **every** remote-tracking ref of this remote rather than walking
        // the whole history: a branch cut from `main` an hour ago is three commits the remote
        // does not have, not the four thousand reachable from its tip. That is also what git
        // itself sends.
        None => PushReport {
            branch: name.to_string(),
            pushed: unseen_by_remote(repo, local.id(), remote),
            old_oid: String::new(),
            new_oid: short_oid(local.id()),
        },
    }
}

/// How many commits reachable from `tip` no branch of `remote` already has.
fn unseen_by_remote(repo: &Repository, tip: git2::Oid, remote: &str) -> u32 {
    let Ok(mut walk) = repo.revwalk() else {
        return 0;
    };
    if walk.push(tip).is_err() {
        return 0;
    }
    let prefix = format!("refs/remotes/{remote}/");
    if let Ok(refs) = repo.references() {
        for reference in refs.flatten() {
            let Ok(name) = reference.name() else { continue };
            if !name.starts_with(&prefix) {
                continue;
            }
            if let Some(oid) = reference.target() {
                let _ = walk.hide(oid);
            }
        }
    }
    walk.count() as u32
}

/// Eight hex digits, the width every other short oid in this crate uses.
fn short_oid(oid: git2::Oid) -> String {
    oid.to_string().chars().take(8).collect()
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

/// The remote a branch's upstream names, else `origin`.
///
/// `pub(crate)` for [`crate::branch::fetch`], which has to pick the same remote a push would:
/// a *Fetch* that reads a different remote from the *Push* beside it in the same menu is a
/// bug the user would diagnose as flaky networking.
pub(crate) fn default_remote(repo: &Repository, upstream: &Option<String>) -> String {
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
    proxy: &ProxyEnv,
) -> Result<PushOutcome> {
    let mut command = Command::new("git");
    // `git` is the child most exposed to a bundled launch after `claude`: it dlopens the
    // host's libcurl and OpenSSL for the network half, and an AppImage's `LD_LIBRARY_PATH`
    // puts eleven bundled libraries ahead of them. See `cide_core::child_env`.
    cide_core::child_env::prepare_command(&mut command);
    // After the bundle scrub, which never touches a proxy name — the two passes answer
    // different questions and the ordering only has to be stated once.
    proxy.apply(&mut command);
    command.current_dir(root).arg("push");
    // These commands are synchronous, so a `git` that blocks on a prompt freezes the window
    // with no way to answer it. When cide was started from a terminal, git finds that
    // terminal on `/dev/tty` — a terminal the user is not looking at — and waits there
    // forever. `GIT_TERMINAL_PROMPT=0` turns that wait into an immediate, readable failure.
    // GUI credential helpers are unaffected: this only disables git's *own* tty prompting,
    // which is the whole reason the binary route exists rather than libgit2's callback.
    command.env("GIT_TERMINAL_PROMPT", "0");
    if set_upstream {
        command.arg("--set-upstream");
    }
    command.arg(remote).arg(refspec);
    // Porcelain output on stdout, human messages on stderr. Both are shown, because the
    // remote's own text — a review URL, a pre-receive rejection — arrives on stderr and it is
    // the whole reason a user reads a push result.
    let output = command.output().wrap()?;
    // Scrubbed of anything cide itself put into this child's environment. `git`'s stderr is
    // shown verbatim — that is the point of the binary route — and curl is quite capable of
    // echoing the proxy it could not reach, credentials and all, into a `407 from proxy after
    // CONNECT` message that then becomes a toast and a screenshot in a ticket. Only secrets
    // *we* handed over are removed; the rest of git's message is not ours to edit.
    let text = proxy.scrub_output(&format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ));
    if !output.status.success() {
        return Err(GitError::Push { output: text });
    }
    Ok(PushOutcome {
        remote: remote.to_string(),
        refspec: refspec.to_string(),
        shelled_out: true,
        output: text,
        // Filled in by `PushReport::into_outcome` from a reading taken before the push. The
        // two route functions cannot take it themselves: they are also the crate's own test
        // surface, and threading a report through them would mean building one in every test.
        ..PushOutcome::blank()
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
    /*
     * Per-ref rejections, which `Remote::push` does **not** report as an error. (M20)
     *
     * This is the half that was missing, and it was missing in the direction that costs the
     * most: `git_remote_push` returns `Ok` when the transport worked, whatever the remote
     * decided about the refs. A non-fast-forward — somebody else pushed first, which is the
     * commonest rejection there is — arrives only through `push_update_reference`, one call per
     * ref, with `Some(reason)` when it was refused. Without this callback the push reported
     * success and the panel said *"Pushed 1 commit to origin/main"* over a remote that had
     * refused it.
     *
     * The binary route never had the bug — `git push` exits non-zero — which is why it showed up
     * only on a local or `file://` remote, and why it showed up as *the palette says one thing
     * and Commit-and-Push says another*.
     */
    let mut rejected: Vec<String> = Vec::new();
    {
        let mut callbacks = git2::RemoteCallbacks::new();
        // The remote's own text: `remote: ...` lines, which is where a server-side hook
        // explains a rejection.
        callbacks.sideband_progress(|bytes| {
            messages.push_str(&String::from_utf8_lossy(bytes));
            true
        });
        callbacks.push_update_reference(|reference, status| {
            if let Some(reason) = status {
                rejected.push(format!("{reference}: {reason}"));
            }
            // `Ok(())` even for a rejection: returning an error here aborts the whole push and
            // loses the *other* refs' outcomes, and the refusal is reported below with all of
            // them rather than as whichever one happened to be first.
            Ok(())
        });
        let mut options = git2::PushOptions::new();
        options.remote_callbacks(callbacks);
        remote
            .push(&[refspec], Some(&mut options))
            .map_err(|e| GitError::Push {
                output: format!("{:?}: {}", e.class(), e.message()),
            })?;
    }

    if !rejected.is_empty() {
        return Err(GitError::Push {
            output: format!("{}{}", messages, rejected.join("\n")),
        });
    }

    if set_upstream {
        set_branch_upstream(repo, branch, remote_name, refspec)?;
    }
    Ok(PushOutcome {
        remote: remote_name.to_string(),
        refspec: refspec.to_string(),
        shelled_out: false,
        output: messages,
        ..PushOutcome::blank()
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
