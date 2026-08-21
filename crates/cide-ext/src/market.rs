//! Cloning and refreshing a marketplace.
//!
//! # Two routes, and the one that matters is the forked binary
//!
//! `cide-git/src/push.rs` established the policy this file follows exactly, and its argument is
//! not repeated here so much as inherited: **libgit2 cannot drive an interactive credential
//! helper.** Its `credentials` callback is a pull-based "give me a username and password" with no
//! way to hand control to `git-credential-libsecret`, a GPG pinentry or a browser-based OAuth
//! flow. So anything that speaks HTTP(S) or SSH is done by forking the user's own `git`, which
//! inherits their whole configuration — helpers, `ssh` options, proxies, `url.*.insteadOf` — and
//! whose stderr is what they are used to reading.
//!
//! What is left for a library is a **local path**, where there is nothing to authenticate. That
//! case is not a token gesture: it is how a marketplace is developed, it is what the tests use,
//! and it is the one this repository's own sibling marketplace is connected by. And for that case
//! there is no library call either — `git` is already the right tool and is already on `PATH`,
//! because [`route`] having chosen `Binary` for everything else means the binary is not an
//! optional extra. So this module forks `git` for both, and [`Route`] survives as the answer to a
//! different question: *does this connection reach the network and use your credentials?* The
//! panel shows that, because a user should not have to infer it from a URL.
//!
//! # Why no `--depth 1`
//!
//! A shallow clone is smaller and is the obvious thing for a directory nobody edits. It is refused
//! for a reason that is specific rather than principled: an install pins a **commit**, and an
//! update compares against it. Under a shallow clone a pinned commit falls out of the repository
//! the moment the marketplace publishes anything, and the comparison stops being answerable —
//! *"is my copy behind?"* becomes *"unknown"* for every extension at once. Marketplaces are small.
//!
//! # Nothing here writes into a project
//!
//! Every path this module touches is under `$XDG_STATE_HOME/cide/marketplaces/`. It never runs
//! `git` with the user's repository as `current_dir`, never reads their index, and can therefore
//! not be the reason a working tree changed.

use std::path::{Path, PathBuf};
use std::process::Command;

use cide_core::proxy::ProxyEnv;

/// How a marketplace's source is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// A path on this machine. No network, no credentials, nothing to prompt for.
    Local,
    /// A remote. Reaches the network and may use the user's credential helper.
    Remote,
}

/// Which route a source takes.
///
/// The string is classified, not the filesystem: a source is `Remote` because of how it is
/// *written*, so the answer is the same before the first clone as after, and the panel can say
/// what a connection will do before it does it.
#[must_use]
pub fn route(source: &str) -> Route {
    let source = source.trim();
    if source.starts_with("http://")
        || source.starts_with("https://")
        || source.starts_with("ssh://")
        || source.starts_with("git://")
    {
        return Route::Remote;
    }
    if source.starts_with("file://") || source.starts_with('/') || source.starts_with('.') {
        return Route::Local;
    }
    if source.starts_with('~') {
        return Route::Local;
    }
    // `user@host:path`. Checked after the schemes and after the path shapes, so an absolute
    // Windows-looking path is not mistaken for one — and before the fallback, because a bare
    // `github.com:owner/repo` is a remote by anyone's reading.
    if let Some((before, _)) = source.split_once(':')
        && !before.is_empty()
        && !before.contains('/')
    {
        return Route::Remote;
    }
    Route::Local
}

/// Expand a leading `~`, and nothing else.
///
/// Deliberately not a general shell expansion. `$VAR` and globs are not expanded because a
/// marketplace source is recorded verbatim in a file that may be committed to a dotfiles
/// repository, and a value whose meaning depends on the environment that read it is a value that
/// means something different on the next machine — which is the case that file exists for.
#[must_use]
pub fn expand(source: &str) -> PathBuf {
    let source = source.trim();
    if let Some(rest) = source.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    if source == "~"
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home);
    }
    PathBuf::from(source.strip_prefix("file://").unwrap_or(source))
}

/// What a `git` invocation produced.
pub struct GitOutput {
    pub ok: bool,
    /// stdout and stderr, in that order, scrubbed of anything cide put in the environment.
    pub text: String,
}

impl GitOutput {
    /// The output as an error sentence, trimmed to something a row can hold.
    #[must_use]
    pub fn error(&self) -> String {
        let text = self.text.trim();
        if text.is_empty() {
            return "git failed and said nothing.".to_string();
        }
        // The last non-empty line: git puts progress on the way down and the reason at the
        // bottom, and a row that showed the first line would show `Cloning into '…'`.
        text.lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or(text)
            .trim()
            .to_string()
    }
}

/// Run `git` with cide's standard child treatment.
///
/// Every spawn in this workspace goes through `child_env::prepare_command` — an AppImage's
/// `AppRun` leaves `LD_LIBRARY_PATH` pointing inside a mount, and `git` is the child most exposed
/// to that after `claude` because it dlopens the host's libcurl and OpenSSL for the network half.
/// `GIT_TERMINAL_PROMPT=0` is the other half: these calls are synchronous, and a `git` that
/// blocks on a prompt would find the terminal cide was launched from — one the user is not
/// looking at — and wait there for ever.
fn run(proxy: &ProxyEnv, dir: Option<&Path>, args: &[&str]) -> std::io::Result<GitOutput> {
    let mut command = Command::new("git");
    cide_core::child_env::prepare_command(&mut command);
    proxy.apply(&mut command);
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    command.env("GIT_TERMINAL_PROMPT", "0");
    // Neither of these is cosmetic. Without `--no-pager` a `git log` inherits the user's pager
    // and blocks on a terminal nobody is reading; `core.hooksPath=` disables hooks, because a
    // marketplace is a repository cide clones on the user's behalf and a `post-checkout` in it
    // would be arbitrary code running at connect time, before the user has approved anything.
    command.arg("--no-pager").arg("-c").arg("core.hooksPath=");
    command.args(args);
    let output = command.output()?;
    let text = proxy.scrub_output(&format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ));
    Ok(GitOutput {
        ok: output.status.success(),
        text,
    })
}

/// Clone a marketplace into `dest`, replacing whatever was there.
///
/// The destination is removed first rather than reused. A half-finished clone from an interrupted
/// connect is a directory that looks like a repository and is not one, and every later `fetch`
/// against it fails with a message about the wrong thing.
pub fn clone(proxy: &ProxyEnv, source: &str, dest: &Path) -> std::io::Result<GitOutput> {
    if dest.exists() {
        std::fs::remove_dir_all(dest)?;
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let expanded = expand(source);
    let source_arg = expanded.to_string_lossy().into_owned();
    let dest_arg = dest.to_string_lossy().into_owned();
    run(proxy, None, &["clone", &source_arg, &dest_arg])
}

/// Bring a clone up to date with its remote, discarding anything local.
///
/// `fetch` then a hard reset onto the remote head, rather than `pull`. A pull can produce a merge
/// commit, and a merge commit in a directory the user never edits is a repository that will one
/// day refuse to fast-forward for a reason nobody can reconstruct. This directory is a cache: the
/// remote is always right.
pub fn refresh(proxy: &ProxyEnv, dest: &Path) -> std::io::Result<GitOutput> {
    let fetched = run(proxy, Some(dest), &["fetch", "--prune", "origin"])?;
    if !fetched.ok {
        return Ok(fetched);
    }
    // The remote's default branch, whatever it is called. `origin/HEAD` is written by clone and
    // is the only thing that knows; hard-coding `main` would break every marketplace on `master`
    // and every one that renames later.
    let head = run(
        proxy,
        Some(dest),
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )?;
    let branch = if head.ok && !head.text.trim().is_empty() {
        head.text.trim().to_string()
    } else {
        "origin/HEAD".to_string()
    };
    run(proxy, Some(dest), &["reset", "--hard", &branch])
}

/// The commit a clone is at, or `None` if the directory is not a repository.
pub fn head(proxy: &ProxyEnv, dest: &Path) -> Option<String> {
    let output = run(proxy, Some(dest), &["rev-parse", "HEAD"]).ok()?;
    if !output.ok {
        return None;
    }
    let head = output.text.trim().to_string();
    if head.is_empty() { None } else { Some(head) }
}

/// Whether a directory looks like a clone this module made.
#[must_use]
pub fn is_clone(dest: &Path) -> bool {
    dest.join(".git").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The routing table, asserted rather than left as prose.
    ///
    /// `push.rs` makes the same assertions about the same question and says why: this is the
    /// load-bearing half, because it is what makes a child's environment the *whole* of cide's
    /// influence over git's network behaviour. A source that was classified `Local` and then
    /// reached the network would do so with none of the proxy handling applied.
    #[test]
    fn remotes_are_remote_and_paths_are_local() {
        for source in [
            "https://github.com/owner/repo.git",
            "http://example.invalid/repo",
            "ssh://git@github.com/owner/repo",
            "git://example.invalid/repo",
            "git@github.com:owner/repo.git",
            "github.com:owner/repo",
        ] {
            assert_eq!(route(source), Route::Remote, "{source}");
        }
        for source in [
            "/home/someone/work/cide-marketplace",
            "./cide-marketplace",
            "../cide-marketplace",
            "file:///home/someone/work/cide-marketplace",
            "~/work/cide-marketplace",
        ] {
            assert_eq!(route(source), Route::Local, "{source}");
        }
    }

    #[test]
    fn a_tilde_is_expanded_and_a_variable_is_not() {
        unsafe { std::env::set_var("HOME", "/home/someone") };
        assert_eq!(
            expand("~/work/m"),
            PathBuf::from("/home/someone/work/m"),
            "a leading ~ is the one expansion this does"
        );
        assert_eq!(
            expand("$HOME/work/m"),
            PathBuf::from("$HOME/work/m"),
            "a source is recorded verbatim in a file that may be committed; a value whose \
             meaning depends on the environment that read it means something else on the next \
             machine"
        );
        assert_eq!(expand("file:///srv/m"), PathBuf::from("/srv/m"));
    }
}
