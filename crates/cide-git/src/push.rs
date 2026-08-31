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
use cide_ipc::git::{
    GitError, PulledCommit, PushBlock, PushOutcome, PushPreview, PushRequest, RepoInfo,
};
use git2::{Direction, Oid, Remote, Repository};

use crate::{Result, Wrap, pull, repo as repo_mod, status};

/// How many commits [`preview`] lists before it starts counting instead.
///
/// Deliberately not [`crate::pull::PULL_COMMIT_CAP`], which is 10. That cap is right for a
/// four-line toast; this list is the whole content of a scrollable dialog whose one job is
/// showing the set about to leave the machine, and a branch with fifteen commits on it is
/// ordinary. 100 matches [`crate::pull::REBASE_COMMIT_CAP`] — the other place cide decided how
/// many commits a person may reasonably be asked to read at once — and the overflow is reported
/// through [`PushPreview::more`] rather than silently dropped.
pub const PUSH_COMMIT_CAP: usize = 100;

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
pub fn push(root: &Path, request: &PushRequest, proxy: &ProxyEnv) -> Result<PushOutcome> {
    let repo = repo_mod::open(root)?;
    let branch = status::branch_info(&repo)?;
    if branch.unborn {
        return Err(GitError::Unborn);
    }

    let remote_name = match request.remote.as_deref() {
        Some(name) => name.to_string(),
        None => default_remote(&repo, &branch.upstream),
    };
    let refspec = match request.refspec.as_deref() {
        Some(spec) => spec.to_string(),
        None => format!("refs/heads/{0}:refs/heads/{0}", branch.head),
    };
    let set_upstream = request.set_upstream;

    // Read **before** the push, and that ordering is the whole design of these three numbers.
    // Afterwards the remote-tracking ref has moved to the tip and `graph_ahead_behind` would
    // answer zero for every successful push. The alternative — parsing them out of git's
    // output — is refused by this module's own rule: on the binary route that text is the
    // remote server's, it is shown verbatim, and nothing in cide may branch on it.
    let report = ahead_of_remote(&repo, &remote_name, &refspec);

    let route = route(&repo, &remote_name);
    let outcome = match route {
        Route::Binary => push_via_binary(
            root,
            &remote_name,
            &refspec,
            set_upstream,
            request.force,
            proxy,
        ),
        // No proxy is applied here, and none is needed: this arm is reached only for a local
        // or `file://` remote, and libgit2 would ignore the environment even if it were. See
        // the module docs, which cite the source for both halves.
        Route::Libgit2 => push_via_libgit2(
            &repo,
            &remote_name,
            &refspec,
            set_upstream,
            request.force,
            &branch.head,
        ),
    };
    outcome.map(|out| PushOutcome {
        ..report.into_outcome(out)
    })
}

/// What the remote actually has, against what our remote-tracking ref says it has.
///
/// The libgit2 route's `--force-with-lease`. Refuses with [`GitError::PushLeaseStale`] when the
/// two disagree, which is the whole safeguard: a `+` refspec sent past a remote that moved
/// deletes commits nobody in this process has ever seen.
///
/// # The three states, and why only one of them refuses
///
/// * **Both sides have the ref and they match** — the ordinary force push. Allowed.
/// * **Neither side has it** — we are publishing a branch the remote has never had. There is
///   nothing to overwrite, so there is nothing to lease. Allowed, exactly as `git push
///   --force-with-lease` allows it.
/// * **The remote has it and we have no tracking ref for it** — somebody created that branch
///   since our last fetch, or we have never fetched. This is the case that reads as harmless and
///   is not: it is precisely "a ref appeared that this process has not seen", which is what the
///   lease exists to catch. Refused, with an empty `expected` saying we held no opinion.
///
/// A destination that is not `refs/heads/<name>` — a tag, a deletion — is left alone. Those are
/// not reachable from the dialog, and inventing a lease for them here would be guessing at a
/// gesture nothing in cide makes.
fn check_lease(
    repo: &Repository,
    remote: &mut Remote<'_>,
    remote_name: &str,
    refspec: &str,
) -> Result<()> {
    let (_, dst) = split_refspec(refspec);
    let Some(name) = dst.strip_prefix("refs/heads/") else {
        return Ok(());
    };

    let tracked = repo
        .find_reference(&format!("refs/remotes/{remote_name}/{name}"))
        .ok()
        .and_then(|r| r.target());

    // `connect` rather than `ls_remote`-by-another-name: git2 has no standalone listing, and the
    // connection is dropped by the `disconnect` below before the push opens its own.
    remote.connect(Direction::Push).wrap()?;
    let advertised = remote
        .list()
        .wrap()
        .map(|heads| {
            heads
                .iter()
                .find(|head| head.name() == dst)
                .map(|head| head.oid())
        })
        // The listing borrows the connection, so the answer is copied out before it closes.
        .inspect_err(|_| {
            let _ = remote.disconnect();
        })?;
    let _ = remote.disconnect();

    match (tracked, advertised) {
        (_, None) => Ok(()),
        (Some(ours), Some(theirs)) if ours == theirs => Ok(()),
        (ours, Some(theirs)) => Err(GitError::PushLeaseStale {
            branch: name.to_string(),
            expected: ours.map(short_oid).unwrap_or_default(),
            actual: short_oid(theirs),
        }),
    }
}

/// `src:dst`, or one name meaning both, with any leading `+` stripped from the source.
fn split_refspec(refspec: &str) -> (&str, &str) {
    match refspec.split_once(':') {
        Some((src, dst)) => (src.trim_start_matches('+'), dst),
        None => {
            let one = refspec.trim_start_matches('+');
            (one, one)
        }
    }
}

/// Hide every remote-tracking ref of `remote` from `walk`.
///
/// The rule shared by [`unseen_by_remote`] and [`preview`]: a branch cut from `main` an hour ago
/// is the three commits the remote does not have, not the four thousand reachable from its tip.
/// That is also what git itself sends. Two copies of this would be a dialog listing one set and
/// a notice counting another.
fn hide_remote_refs(repo: &Repository, walk: &mut git2::Revwalk<'_>, remote: &str) {
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
}

/// The commits reachable from `tip` that no branch of `remote` already has, newest first.
///
/// [`crate::pull::taken_commits`]'s answer for the case it cannot express: there is no single
/// `hidden` oid and therefore no `graph_ahead_behind` to take the total from, so the overflow is
/// counted by continuing the walk rather than by arithmetic. Capped at [`PUSH_COMMIT_CAP`].
fn unseen_commits(repo: &Repository, tip: Oid, remote: &str) -> Result<(Vec<PulledCommit>, u32)> {
    let mut walk = repo.revwalk().wrap()?;
    walk.push(tip).wrap()?;
    hide_remote_refs(repo, &mut walk, remote);

    let mut commits = Vec::new();
    let mut more = 0u32;
    for oid in walk {
        let oid = oid.wrap()?;
        if commits.len() >= PUSH_COMMIT_CAP {
            more = more.saturating_add(1);
            continue;
        }
        commits.push(pull::summarise(&repo.find_commit(oid).wrap()?));
    }
    Ok((commits, more))
}

/// Every remote this repository has, `origin` first and the rest in name order.
///
/// Ordered rather than left in libgit2's order because it is drawn in a picker: `origin` is what
/// nearly every row means, and a list whose first entry moves between repositories is one the
/// user has to read every time.
fn remote_names(repo: &Repository) -> Vec<String> {
    let Ok(names) = repo.remotes() else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for name in names.iter() {
        if let Ok(Some(name)) = name {
            out.push(name.to_string());
        }
    }
    out.sort_by(|a: &String, b: &String| {
        (a != "origin", a.as_str()).cmp(&(b != "origin", b.as_str()))
    });
    out
}

/// What a push from `info` would send, without sending anything.
///
/// The push dialog's whole payload for one repository. It answers with a [`PushPreview`] in every
/// case it can — including the three it cannot push at all, which arrive as
/// [`PushPreview::blocked`] rather than as an error, because the dialog draws a project's
/// repositories together and one unborn submodule must not blank the row beside it.
///
/// Every decision here is [`push`]'s, made by the same code: [`default_remote`] picks the remote,
/// the refspec is spelled the same way, and [`route`] answers whether the lease will be git's or
/// cide's. A preview that resolved any of those independently would be describing a different
/// push from the one the button performs.
pub fn preview(info: &RepoInfo) -> Result<PushPreview> {
    let repo = repo_mod::open(&info.root)?;
    let head = status::branch_info(&repo)?;
    let remotes = remote_names(&repo);
    let remote_name = default_remote(&repo, &head.upstream);

    let blocked = |reason: PushBlock| PushPreview {
        repo: info.clone(),
        head: head.clone(),
        remote: remote_name.clone(),
        remotes: remotes.clone(),
        refspec: String::new(),
        publish: false,
        commits: Vec::new(),
        more: 0,
        diverged: false,
        shells_out: false,
        blocked: Some(reason),
    };

    if head.unborn {
        return Ok(blocked(PushBlock::Unborn));
    }
    if head.detached {
        return Ok(blocked(PushBlock::Detached));
    }
    if repo.find_remote(&remote_name).is_err() {
        return Ok(blocked(PushBlock::NoRemote));
    }

    let Some(local) = repo
        .revparse_single(&format!("refs/heads/{}", head.head))
        .ok()
        .and_then(|o| o.peel_to_commit().ok())
        .map(|c| c.id())
    else {
        // `branch_info` said born and attached, so this is a repository that changed under us
        // between the two reads. Nothing to push is the honest answer, and it is the same answer
        // an unborn one gives.
        return Ok(blocked(PushBlock::Unborn));
    };

    let tracking = repo
        .find_reference(&format!("refs/remotes/{}/{}", remote_name, head.head))
        .ok()
        .and_then(|r| r.target());

    let (commits, more, publish, diverged) = match tracking {
        Some(old) => {
            let (ahead, behind) = repo.graph_ahead_behind(local, old).unwrap_or((0, 0));
            let (commits, more) =
                pull::taken_commits(&repo, local, old, ahead as u32, PUSH_COMMIT_CAP)?;
            // Measured against the **remote-tracking ref**, not `BranchInfo::behind`, which is
            // measured against the configured upstream. They are usually the same ref and the
            // times they are not are exactly the times this flag decides whether a force push is
            // offered — a branch pushed to a remote it does not track, say.
            (commits, more, false, behind > 0)
        }
        None => {
            let (commits, more) = unseen_commits(&repo, local, &remote_name)?;
            // Nothing on the remote to be behind of. `git push` fast-forwards from nothing.
            (commits, more, true, false)
        }
    };

    Ok(PushPreview {
        repo: info.clone(),
        head: head.clone(),
        remote: remote_name.clone(),
        remotes,
        refspec: format!("refs/heads/{0}:refs/heads/{0}", head.head),
        publish,
        commits,
        more,
        diverged,
        shells_out: route(&repo, &remote_name) == Route::Binary,
        blocked: None,
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
    // A leading `+` is a force push, which changes nothing about what is being counted.
    let (src, dst) = split_refspec(refspec);
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
    hide_remote_refs(repo, &mut walk, remote);
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
    force: bool,
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
    /*
     * `--force-with-lease`, bare, and never `--force`.
     *
     * Bare rather than `--force-with-lease=<dst>:<oid>`, which is the form that looks safer and
     * is not. The lease has to be measured against the remote-tracking ref **at push time**: an
     * oid captured when the dialog opened is stale the moment anything else fetches — another
     * cide window, a terminal pane, an agent — and pinning the lease to it would either refuse a
     * push that is fine or, worse, assert an expectation the user never actually read.
     *
     * git's own default is exactly this reading, and it comes with git's own caveat: the lease
     * is only as good as the last fetch, so a `git fetch` run by something that was not looking
     * at the remote ref can still make it vacuous. cide does not try to improve on that — it
     * would mean parsing or second-guessing git, which this module's header forbids.
     */
    if force {
        command.arg("--force-with-lease");
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
    force: bool,
    branch: &str,
) -> Result<PushOutcome> {
    let mut remote: Remote<'_> = repo.find_remote(remote_name).wrap()?;
    /*
     * The lease, performed rather than delegated.
     *
     * libgit2 has no `--force-with-lease`: `git_push_options` carries no expectation at all, and
     * the only force it understands is the `+` on a refspec, which is `--force` whole. So the
     * check is done here — connect, read what the remote actually advertises, and compare it
     * against what our remote-tracking ref claims it was at. A mismatch means somebody pushed
     * since our last fetch and the `+` would delete their work, so nothing is sent.
     *
     * This route is reached only for a local or `file://` remote (see [`route`]), where the
     * connection is a directory read and the extra round trip costs nothing. On the binary route
     * git does the same comparison itself and refuses first; the two must agree, which is why
     * this is a lease and not a plain force with a warning in the dialog.
     */
    if force {
        check_lease(repo, &mut remote, remote_name, refspec)?;
    }
    let refspec: &str = &if force {
        format!("+{}", refspec.trim_start_matches('+'))
    } else {
        refspec.to_string()
    };
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
