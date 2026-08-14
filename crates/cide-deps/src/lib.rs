//! What a project depends on, and where each dependency's source sits on this machine.
//!
//! The backend of *External Libraries*. It answers one question — "list this project's external
//! dependencies and the directory each one's source is unpacked into" — by asking the toolchain
//! that already knows, and it is built around four properties that are each one wrong flag away
//! from being false.
//!
//! # 1. It never writes to the user's repository
//!
//! `cargo metadata` **creates or updates `Cargo.lock`** as a side effect. So does
//! `cargo metadata --offline`, which is the flag most people reach for and which was measured
//! doing exactly that: in a crate with no lockfile it printed `Locking 7 packages…` and wrote
//! one. `--no-deps` avoids the write and is useless here, because it reports only workspace
//! members — i.e. nothing this crate is for.
//!
//! **`--frozen`** (`--locked --offline`) is the only combination that both resolves the full
//! graph and is guaranteed not to touch the tree. A missing or stale lockfile becomes exit 101
//! and a legible sentence, which is the *content* of the failure row rather than an obstacle to
//! it:
//!
//! ```text
//! error: the lock file /…/Cargo.lock needs to be updated but --frozen was passed to prevent this
//! ```
//!
//! `go list -m` has the same hazard and the same answer: `-mod=readonly` (the default since Go
//! 1.16, set explicitly so a user's `GOFLAGS=-mod=mod` cannot turn it off) plus `GOPROXY=off`,
//! which makes a fetch impossible rather than merely unlikely. `go list` may still write
//! `$GOCACHE`, which is a cache and not the user's repository.
//!
//! # 2. It never touches the network
//!
//! Same two flags. This matters more than politeness: the group is expanded by a click, and a
//! click that could take thirty seconds on hotel wifi is a click that looks broken. Every
//! failure this crate can report is therefore a *local* failure, and every one of them is
//! reportable in a sentence immediately.
//!
//! # 3. It never blocks the UI
//!
//! Nothing here is asynchronous and nothing here is fast: `cargo metadata --frozen` over this
//! repository is 0.22 s warm and produces 3 MB of JSON. [`resolve`] is blocking, and the caller
//! runs it on a thread of its own — see [`resolve`]'s own note, which is where the
//! `PR_SET_PDEATHSIG` contract is spelled out, because getting *that* wrong is the trap in this
//! feature rather than the latency.
//!
//! # 4. It indexes and watches nothing
//!
//! This crate returns paths. It does not walk them, does not `stat` their contents, and hands
//! nothing to `cide-fs`. `~/.cargo/registry` here is 2.9 GB across 2,223 crate directories; an
//! inotify watch on it or a `nucleo` injection of its files would drown both.

use std::path::{Path, PathBuf};
use std::process::Command;

pub mod cargo;
pub mod go;
pub mod version;

/// A toolchain whose dependency graph cide can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toolchain {
    Cargo,
    Go,
}

impl Toolchain {
    pub fn binary(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Go => "go",
        }
    }

    /// Files whose presence means this toolchain has something to resolve here.
    ///
    /// `go.work` before `go.mod`: in a Go workspace `go list -m all` covers every member
    /// module, so resolving from the workspace file is one process instead of one per module.
    pub fn markers(self) -> &'static [&'static str] {
        match self {
            Self::Cargo => &["Cargo.toml"],
            Self::Go => &["go.work", "go.mod"],
        }
    }

    /// How to install it, in the words the user would type.
    fn install_hint(self) -> &'static str {
        match self {
            Self::Cargo => "rustup",
            Self::Go => "your package manager, or from https://go.dev/dl/",
        }
    }
}

/// One external dependency of a project.
///
/// "External" excludes the workspace's own members: they are the project, and they are already
/// drawn in the tree above this group. A **path dependency outside the roots** is not excluded —
/// a sibling crate the user is developing against is one of the most useful rows the group can
/// have, and it is also the case that justifies the read-only rule being about the *dependency
/// cache* rather than about this group: a path dep is the user's own code and stays writable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    /// What the row draws. The crate name for cargo; the **full module path** for go —
    /// `github.com/go-openapi/jsonpointer`, never `jsonpointer`, because the module cache is
    /// full of colliding last segments and the two are not the same claim.
    pub name: String,
    /// Drawn dim after the name.
    pub version: String,
    /// The unpacked source directory. `None` when there is nothing on disk to open.
    pub dir: Option<PathBuf>,
    /// Why this row has no directory, or why it is not what the user expects.
    ///
    /// Go's `-e` flag is what makes this per-row rather than per-group: a module the graph names
    /// but the cache does not hold answers with an `Error` of its own and every *other* module
    /// still resolves. Cargo has no `-e` equivalent, so a cargo failure is a group-level note.
    pub note: Option<String>,
}

/// Everything one project's resolution produced.
///
/// **Both fields may be non-empty at once**, and that is the shape the group needs: go can
/// report 20 modules and one that is missing from the cache, and a design that made this an
/// `enum` would have to throw one of the two away.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolved {
    pub packages: Vec<Package>,
    /// Sentences the group shows in place of, or beside, the packages.
    ///
    /// Never empty when `packages` is: *a group that is silently empty is indistinguishable
    /// from a broken one*, so [`resolve`] guarantees at least one of the two is non-empty.
    pub notes: Vec<String>,
}

/// A file whose contents decide the answer, and enough of its metadata to tell it moved.
///
/// `(len, mtime-nanos)` rather than a hash: reading and hashing every `Cargo.lock` in a monorepo
/// on every expand is real work for a question two `stat`s answer, and a lockfile rewritten with
/// identical contents genuinely has nothing to re-resolve.
pub type Stamp = Vec<(PathBuf, u64, u128)>;

/// The manifests and lockfiles under `roots`, with their current stamps.
///
/// Taken once per resolution, and compared with `==` against [`restamp`]. Correct where a watcher
/// subscription would not be: `Cargo.lock` may be gitignored, in which case `cide_fs::Filter`
/// excludes it and **no `cide://fs-changed` will ever mention it**, so a design that invalidated
/// on the watcher would go stale for exactly the projects that ignore their lockfile.
///
/// It costs a `read_dir` per directory to depth two, because it has to *find* the manifests. That
/// is why it is not the function the staleness check calls — see [`restamp`].
pub fn stamp(roots: &[PathBuf]) -> Stamp {
    let mut out = Stamp::new();
    for unit in units(roots) {
        for file in unit.stamped_files() {
            out.push(stat(file));
        }
    }
    out
}

/// The same files, re-`stat`ed. What the staleness check actually calls.
///
/// The distinction is a cost, and it matters because the caller is on the path of **every**
/// `fs_tree_count` — which the file tree issues on every watcher burst, and a `cargo build` in a
/// terminal pane produces those continuously. [`stamp`] re-walks each root to depth two to
/// rediscover the manifests; this re-`stat`s the two-to-six files the last resolution actually
/// read, which is a handful of syscalls and no directory reads at all.
///
/// What it gives up is noticing a `Cargo.toml` that appeared where there was none. That is the
/// same thing the group's own probe gives up (see `cide_app::libraries::Libraries::probe`) and for
/// the same reason: a project that gains its first manifest while open shows the group on the next
/// launch, and re-walking every root on every burst to catch it would cost more than the feature.
pub fn restamp(previous: &Stamp) -> Stamp {
    previous
        .iter()
        .map(|(path, _, _)| stat(path.clone()))
        .collect()
}

fn stat(file: PathBuf) -> (PathBuf, u64, u128) {
    let (len, mtime) = match std::fs::metadata(&file) {
        Ok(meta) => (
            meta.len(),
            meta.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ),
        // A file that is *absent* is a stamp too: `Cargo.lock` appearing is exactly the change
        // that turns a failing resolution into a working one.
        Err(_) => (0, 0),
    };
    (file, len, mtime)
}

/// Which toolchains have something to resolve under these roots.
///
/// **The only work done before the user asks for anything.** A handful of `stat`s and at most
/// one `read_dir` per directory to depth 2, which is what decides whether the *External
/// Libraries* header row exists at all. Nothing is spawned and no dependency is resolved.
pub fn project_kinds(roots: &[PathBuf]) -> Vec<Toolchain> {
    let mut kinds = Vec::new();
    for kind in [Toolchain::Cargo, Toolchain::Go] {
        if roots
            .iter()
            .any(|root| cide_core::toolchain::has_marker(kind.markers(), root))
        {
            kinds.push(kind);
        }
    }
    kinds
}

/// One thing to run a resolver against: a manifest, and the toolchain that understands it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Unit {
    kind: Toolchain,
    /// The manifest itself — `Cargo.toml`, `go.work` or `go.mod`.
    manifest: PathBuf,
}

impl Unit {
    fn dir(&self) -> &Path {
        self.manifest.parent().unwrap_or(Path::new("."))
    }

    /// The files whose contents decide this unit's answer. See [`stamp`].
    fn stamped_files(&self) -> Vec<PathBuf> {
        let dir = self.dir();
        match self.kind {
            Toolchain::Cargo => vec![self.manifest.clone(), dir.join("Cargo.lock")],
            Toolchain::Go => vec![
                self.manifest.clone(),
                dir.join("go.sum"),
                dir.join("go.work.sum"),
            ],
        }
    }
}

/// Every manifest worth resolving under `roots`, deduplicated.
///
/// Two `ProjectRoot`s can be two members of one Cargo workspace, two unrelated workspaces, or
/// one Cargo root and one Go root. The dedupe is on the **manifest path**, which is what makes
/// the first case one resolution instead of two identical ones — `cide_core::toolchain::
/// find_markers` already stops descending at the first manifest in a directory, so a workspace
/// root is found once and its members are never visited.
fn units(roots: &[PathBuf]) -> Vec<Unit> {
    let mut units: Vec<Unit> = Vec::new();
    for kind in [Toolchain::Cargo, Toolchain::Go] {
        for root in roots {
            for manifest in cide_core::toolchain::find_markers(kind.markers(), root) {
                // A directory holding both `go.work` and `go.mod` is one Go unit, not two: the
                // workspace file already covers the module. `find_markers` returns markers in
                // the order they are declared, so `go.work` is seen first.
                let same_dir = |u: &Unit| {
                    u.kind == kind && u.dir() == manifest.parent().unwrap_or(Path::new("."))
                };
                if units.iter().any(|u| u.manifest == manifest) || units.iter().any(same_dir) {
                    continue;
                }
                units.push(Unit {
                    kind,
                    manifest: manifest.clone(),
                });
            }
        }
    }
    units
}

/// Resolve every dependency of every unit under `roots`.
///
/// # This must be called from a thread that outlives the children it forks
///
/// Not from `cide_core::child_env::on_spawn_thread`, and that is the trap in this feature rather
/// than an optimisation. `on_spawn_thread` runs its closure on *one* process-global thread and
/// blocks the caller until it returns; its own documentation says a spawn there is "single-digit
/// milliseconds". A closure that spawned **and waited for** `cargo metadata` would hold that one
/// thread for the whole run — 0.22 s warm, seconds cold, and unbounded if `cargo` is queued
/// behind the package-cache lock held by a `cargo build` in a terminal pane. Every pane spawn
/// and every language-server start in the process would sit behind a file-tree expansion.
///
/// What `arm` actually requires (`PR_SET_PDEATHSIG` is delivered when the **thread** that forked
/// exits, not the process) is satisfied here by construction instead: this function forks the
/// child and waits for it on the same thread, so the forking thread's lifetime *is* the child's
/// lifetime plus epsilon. The precedent is `cide_lsp::server`, which uses `on_spawn_thread` only
/// to *create* its supervisor and forks from the supervisor, which then waits.
///
/// The degenerate case is also right: if this function is ever given a deadline and returns
/// while a child is still running, `PDEATHSIG` fires on thread exit and `SIGTERM`s the abandoned
/// child, which is exactly the desired behaviour.
///
/// **There is no timeout.** `--frozen` and `GOPROXY=off` make the network impossible, so the
/// only unbounded wait left is cargo's package-cache lock — held by the user's own build, and
/// bounded by it. Adding a deadline would mean a second thread holding the child's pid, and a
/// resolution that gives up half way is not better than one that finishes late behind a
/// *Resolving…* row the user can see.
pub fn resolve(roots: &[PathBuf]) -> Resolved {
    let units = units(roots);
    let mut out = Resolved::default();
    for unit in &units {
        let one = match unit.kind {
            Toolchain::Cargo => cargo::resolve(&unit.manifest),
            Toolchain::Go => go::resolve(unit.dir()),
        };
        out.packages.extend(one.packages);
        out.notes.extend(one.notes);
    }

    // Identity is the **directory**, not the name and not `name@version`. This repository has 41
    // crate names present at two versions at once; two rows called `base64` are two different
    // sources and both belong. A directory, on the other hand, cannot legitimately appear twice
    // — two units in one project sharing a lockfile resolve to the same unpacked crate.
    out.packages.sort_by(|a, b| {
        version::row_cmp(
            (&a.name, &a.version, dir_key(a)),
            (&b.name, &b.version, dir_key(b)),
        )
    });
    out.packages
        .dedup_by(|a, b| a.dir.is_some() && a.dir == b.dir);

    // The guarantee the group depends on: never silently empty. "Resolved fine, and this project
    // genuinely has no external dependencies" is a real answer and a surprising one, so it gets
    // a sentence like every other outcome.
    if out.packages.is_empty() && out.notes.is_empty() {
        out.notes.push(if units.is_empty() {
            "No Cargo or Go project under this project's roots.".to_string()
        } else {
            "No external dependencies.".to_string()
        });
    }
    out
}

fn dir_key(package: &Package) -> &str {
    package
        .dir
        .as_deref()
        .and_then(Path::to_str)
        .unwrap_or_default()
}

/// Why a resolver produced no packages.
#[derive(Debug, thiserror::Error)]
pub enum DepsError {
    /// The binary is not installed, or is not where a desktop launcher can see it.
    #[error("{0}")]
    NoToolchain(String),
    /// It ran and failed. Carries the first useful line of its own stderr, verbatim.
    #[error("{0}")]
    Refused(String),
    /// It ran, succeeded, and said something this crate could not parse.
    #[error("{0}")]
    Unreadable(String),
}

/// Find a toolchain binary, or say why the group is empty in words the user can act on.
///
/// The sentence follows `cide_lsp::discover::find`'s shape deliberately — same problem, same
/// phrasing, and the "but I have it installed" hint is the half that answers the question the
/// user actually has when a desktop launcher's `PATH` is missing `~/.cargo/bin`.
fn locate(kind: Toolchain) -> Result<PathBuf, DepsError> {
    match cide_core::toolchain::which(kind.binary()) {
        Some(found) => Ok(found),
        None => Err(missing(kind)),
    }
}

/// The sentence a user reads when the toolchain is not there.
///
/// A function of its own so it can be tested on a machine that *has* both toolchains installed,
/// which is every machine this suite runs on. The alternative — a test helper that rebuilds the
/// message — is the anti-pattern `cide_core::toolchain`'s own `refuses` helper was caught in: a
/// helper that restates the rule is a helper that tests itself.
fn missing(kind: Toolchain) -> DepsError {
    DepsError::NoToolchain(format!(
        "`{}` is not on PATH, so cide cannot list this project's dependencies. Install it with {}.{}",
        kind.binary(),
        kind.install_hint(),
        extra_paths_hint(kind),
    ))
}

/// Only mentioned when the toolchain's own directory exists but is not on `PATH`, because that
/// is the one case where the user's next question is "but I have it installed".
///
/// The same sentence and the same shape as `cide_lsp::discover::extra_paths_hint`, restated
/// rather than shared because the two crates must not depend on each other — and because the
/// wording is the point: a desktop launcher does not run the user's shell rc, so `~/.cargo/bin`
/// is missing from `PATH` on exactly the machines where the binary is sitting in it.
fn extra_paths_hint(kind: Toolchain) -> String {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return String::new();
    };
    let dir = match kind {
        Toolchain::Cargo => home.join(".cargo/bin"),
        Toolchain::Go => home.join("go/bin"),
    };
    if dir.join(kind.binary()).is_file() {
        format!(
            " (found in {} — cide searched there too, so this build could not execute it)",
            dir.display()
        )
    } else {
        String::new()
    }
}

/// Fork a resolver, wait for it, and hand back its stdout.
///
/// `scrub_command` then `arm`, in that order and never one without the other — the rule
/// `CLAUDE.md` states about every `Command::new` in this workspace. The order matters only in
/// that `arm` installs a `pre_exec` hook and reads no environment, so mirroring
/// `cide_lsp::server`'s sequence keeps the two spawn sites readable side by side.
///
/// The scrub is not belt-and-braces here. An AppImage's `AppRun` prepends
/// `LD_LIBRARY_PATH=$APPDIR/usr/lib` — 160 libraries including GTK 3 and WebKitGTK — and a
/// `cargo` or `go` that inherits it dies with a symbol-lookup error two processes down, which
/// reaches the user as *External Libraries is empty*.
///
/// `wait_with_output` rather than `wait` plus two reads: it drains both pipes concurrently, so a
/// resolver that writes 3 MB to stdout and a warning to stderr cannot deadlock against a full
/// pipe buffer. **It also does the waiting on this thread**, which is what satisfies `arm`'s
/// contract — see [`resolve`].
fn run(kind: Toolchain, mut command: Command, what: &str) -> Result<Vec<u8>, DepsError> {
    cide_core::child_env::scrub_command(&mut command);
    cide_core::child_env::arm(&mut command);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let started = std::time::Instant::now();
    let output = command.spawn().and_then(|child| child.wait_with_output());
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            return Err(DepsError::Refused(format!(
                "`{}` could not be started: {error}",
                kind.binary()
            )));
        }
    };
    tracing::debug!(
        binary = kind.binary(),
        what,
        ms = started.elapsed().as_millis() as u64,
        bytes = output.stdout.len(),
        status = ?output.status.code(),
        "resolved dependencies"
    );

    if output.status.success() {
        return Ok(output.stdout);
    }
    Err(DepsError::Refused(first_line(
        &String::from_utf8_lossy(&output.stderr),
        kind,
    )))
}

/// The first line of a resolver's stderr that says something.
///
/// Verbatim, because these sentences are already written for a human — cargo's own
/// `the lock file … needs to be updated but --frozen was passed to prevent this` is a better
/// explanation than anything this crate could paraphrase, and paraphrasing is how a message
/// stops matching what the user finds when they search for it.
///
/// # Which line, and why "the first non-empty one" was wrong
///
/// Cargo writes **status** lines to stderr as well as errors, and the one that matters here was
/// found by a test rather than reasoned about: with any other cargo holding the package-cache
/// lock — a `cargo build` in a terminal pane, or another window's resolution — the first line of
/// stderr is
///
/// ```text
///     Blocking waiting for file lock on package cache
/// ```
///
/// and the *actual* cause is the line after it. A user whose lockfile was stale would have been
/// told "Blocking waiting for file lock on package cache" as the reason External Libraries was
/// empty, which is a true sentence about something that had already finished.
///
/// So an `error:`-prefixed line wins outright, wherever it is. Failing that — go prefixes its own
/// failures `go:` and neither toolchain is contractually bound to either prefix — the first line
/// that is *not* a status line is taken, where a status line is one cargo indented (its verbs are
/// right-aligned to column 12) or one prefixed `warning:`. A resolver that failed with nothing at
/// all on stderr gets a sentence naming the binary, because "" in the row is the silent emptiness
/// this whole feature is built to make impossible.
fn first_line(stderr: &str, kind: Toolchain) -> String {
    let mut fallback: Option<&str> = None;
    for line in stderr.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // `error[E0433]:` as well as `error:`, so a diagnostic code does not push the cause down
        // to whatever came after it.
        if trimmed.starts_with("error:") || trimmed.starts_with("error[") {
            return trimmed.to_string();
        }
        // Go's progress lines carry go's own `go:` prefix — the same prefix its *failures* use —
        // so prefix alone cannot separate them and the verb has to. `go: downloading …` and
        // `go: finding …` are announcements of work in flight; when a build fails because the
        // module cache could not be reached, they are printed *before* the sentence that says so.
        //
        // Without this, a Go project whose go.mod pins a newer toolchain than the machine has —
        // an ordinary state for a checked-out repo — showed "go: downloading go1.24.0" as the
        // reason External Libraries was empty. That is worse than unhelpful: it is a claim that
        // cide is fetching a Go toolchain, which it is not, about a failure it is not describing.
        let go_progress = matches!(kind, Toolchain::Go)
            && (trimmed.starts_with("go: downloading ") || trimmed.starts_with("go: finding "));
        let status =
            line.starts_with(char::is_whitespace) || trimmed.starts_with("warning:") || go_progress;
        if !status && fallback.is_none() {
            fallback = Some(trimmed);
        }
    }
    fallback
        .map(str::to_string)
        .unwrap_or_else(|| format!("`{}` failed with no explanation.", kind.binary()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cide-deps-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_project_with_no_manifest_has_no_group_and_says_so() {
        let dir = temp("bare");
        std::fs::write(dir.join("notes.txt"), "").expect("write");
        assert!(project_kinds(std::slice::from_ref(&dir)).is_empty());
        let resolved = resolve(std::slice::from_ref(&dir));
        assert!(resolved.packages.is_empty());
        assert_eq!(resolved.notes.len(), 1, "{resolved:?}");
        assert!(resolved.notes[0].contains("No Cargo or Go project"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The whole reason `units` dedupes: two roots inside one Cargo workspace.
    #[test]
    fn two_roots_in_one_workspace_are_one_unit() {
        let dir = temp("workspace");
        std::fs::create_dir_all(dir.join("crates/a")).expect("mkdir");
        std::fs::create_dir_all(dir.join("crates/b")).expect("mkdir");
        std::fs::write(dir.join("Cargo.toml"), "[workspace]\n").expect("write");
        std::fs::write(dir.join("crates/a/Cargo.toml"), "[package]\n").expect("write");
        std::fs::write(dir.join("crates/b/Cargo.toml"), "[package]\n").expect("write");
        let units = units(&[dir.clone(), dir.clone()]);
        assert_eq!(units.len(), 1, "{units:?}");
        assert_eq!(units[0].manifest, dir.join("Cargo.toml"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_go_workspace_is_one_unit_and_not_two() {
        // `go list -m all` in a workspace covers every member module, so resolving `go.mod`
        // beside `go.work` would run a second process for a subset of the same answer.
        let dir = temp("gowork");
        std::fs::write(dir.join("go.work"), "go 1.21\n").expect("write");
        std::fs::write(dir.join("go.mod"), "module x\n").expect("write");
        let units = units(std::slice::from_ref(&dir));
        assert_eq!(units.len(), 1, "{units:?}");
        assert_eq!(units[0].manifest, dir.join("go.work"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_stamp_moves_when_a_lockfile_does_and_notices_one_appearing() {
        let dir = temp("stamp");
        std::fs::write(dir.join("Cargo.toml"), "[package]\n").expect("write");
        let before = stamp(std::slice::from_ref(&dir));
        assert!(!before.is_empty());
        // A lockfile that did not exist is the change that turns a failing resolution into a
        // working one, so its *absence* has to be part of the stamp.
        std::fs::write(dir.join("Cargo.lock"), "version = 4\n").expect("write");
        assert_ne!(stamp(std::slice::from_ref(&dir)), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failure_with_nothing_on_stderr_still_says_something() {
        assert!(first_line("", Toolchain::Cargo).contains("cargo"));
        assert_eq!(
            first_line(
                "warning: unused manifest key\nerror: real cause\n",
                Toolchain::Cargo
            ),
            "error: real cause",
            "a deprecation warning must not become the reported cause"
        );
    }

    /// Verbatim from a run that raced another cargo for the package-cache lock, which is what a
    /// user with a `cargo build` open in a terminal pane produces every time.
    ///
    /// Found by `a_project_that_has_never_been_built_says_so_and_writes_no_lockfile` rather than
    /// by reading: the first draft of `first_line` took the first non-empty line and reported
    /// *"Blocking waiting for file lock on package cache"* as the reason the group was empty.
    #[test]
    fn a_status_line_is_not_the_cause() {
        let stderr = concat!(
            "    Blocking waiting for file lock on package cache\n",
            "error: the lock file /p/Cargo.lock needs to be updated but --frozen was passed to ",
            "prevent this\n",
            "If you want to try to generate the lock file without accessing the network, remove ",
            "the --frozen flag and use --offline instead.\n",
        );
        assert!(
            first_line(stderr, Toolchain::Cargo).starts_with("error: the lock file"),
            "{}",
            first_line(stderr, Toolchain::Cargo)
        );
        // Go prefixes its own failures `go:` and does not use `error:`, so the fallback has to
        // carry that case rather than the `error:` fast path.
        assert_eq!(
            first_line(
                "go: cannot match \"all\": go.mod file not found in current directory\n",
                Toolchain::Go
            ),
            "go: cannot match \"all\": go.mod file not found in current directory",
        );
        // A diagnostic code still reads as the cause.
        assert_eq!(
            first_line(
                "   Compiling x\nerror[E0433]: no such module\n",
                Toolchain::Cargo
            ),
            "error[E0433]: no such module",
        );

        // Go announces work in flight with its own `go:` prefix — the same one its failures use —
        // so the verb is the only thing that separates them. A repo whose go.mod pins a newer
        // toolchain than the machine has is an ordinary checkout, and it printed
        // "go: downloading go1.24.0" as the reason External Libraries was empty: a claim that
        // cide is fetching a Go toolchain, which it never does, standing in front of the sentence
        // that actually says what went wrong.
        assert_eq!(
            first_line(
                "go: downloading go1.24.0 (linux/amd64)\n\
                 go: module lookup disabled by GOPROXY=off\n",
                Toolchain::Go,
            ),
            "go: module lookup disabled by GOPROXY=off",
        );
        assert_eq!(
            first_line(
                "go: finding module for package example.com/x\n\
                 go: example.com/x: no matching versions\n",
                Toolchain::Go,
            ),
            "go: example.com/x: no matching versions",
        );
        // ...and the skip is scoped to Go, so cargo output is read exactly as before. Nothing
        // cargo prints begins `go: `, but pinning it keeps the toolchain argument load-bearing
        // rather than decorative.
        assert_eq!(
            first_line("go: downloading go1.24.0\n", Toolchain::Cargo),
            "go: downloading go1.24.0",
        );
        // A stderr that is *only* progress must not fall through to an empty row — the silent
        // emptiness this whole function exists to prevent.
        assert!(
            !first_line("go: downloading go1.24.0\n", Toolchain::Go).is_empty(),
            "progress-only stderr still names the binary rather than saying nothing",
        );
    }

    #[test]
    /// The row a user sees when the toolchain is not installed.
    ///
    /// Asserted on the real [`missing`] rather than on a re-spelling of it, and driven for both
    /// toolchains so a machine that has one and not the other still checks both sentences. What
    /// it pins is that the sentence is *actionable*: it names the binary and it says how to get
    /// it, which is the whole difference between this row and an empty group.
    fn a_missing_toolchain_names_the_binary_and_how_to_install_it() {
        for kind in [Toolchain::Cargo, Toolchain::Go] {
            let DepsError::NoToolchain(reason) = missing(kind) else {
                panic!("`missing` must produce a NoToolchain");
            };
            assert!(reason.contains(kind.binary()), "{reason}");
            assert!(
                reason.contains("not on PATH"),
                "the row has to say what is wrong, not just that something is: {reason}"
            );
            assert!(
                reason.contains("Install it with"),
                "and what to do about it: {reason}"
            );
        }
        // The other half of `locate`, which cannot be reached on a machine that has cargo: a
        // binary nobody has is not found. `cide_core::toolchain` owns that claim.
        assert!(cide_core::toolchain::which("cide-no-such-toolchain").is_none());
    }

    /// The real thing, against this repository. `#[ignore]`d like every other test in this
    /// workspace that spawns a real binary: it needs `cargo` on PATH and a populated registry.
    ///
    /// Run it deliberately — `cargo test -p cide-deps -- --ignored` — after touching either
    /// resolver, because it is the only thing that checks the *flags*. Every other test here
    /// parses a fixture, and a fixture cannot notice that `--frozen` was dropped.
    #[test]
    #[ignore = "spawns the real cargo against this repository"]
    fn this_repository_resolves_and_writes_nothing() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("the workspace root")
            .to_path_buf();
        let lock = root.join("Cargo.lock");
        let before = std::fs::metadata(&lock).and_then(|m| m.modified()).ok();

        let resolved = resolve(std::slice::from_ref(&root));
        assert!(resolved.notes.is_empty(), "{:?}", resolved.notes);
        assert!(
            resolved.packages.len() > 100,
            "cide has hundreds of dependencies, not {}",
            resolved.packages.len()
        );
        assert!(
            resolved.packages.iter().all(|p| p.dir.is_some()),
            "cargo cannot report a package whose manifest it could not read"
        );
        assert!(
            !resolved
                .packages
                .iter()
                .any(|p| p.name.starts_with("cide-")),
            "the workspace's own crates are not dependencies of it"
        );
        // Sorted, and stably so: `base64` appears twice and the older version must be first.
        let names: Vec<&str> = resolved.packages.iter().map(|p| p.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_by(|a, b| version::name_cmp(a, b));
        assert_eq!(names, sorted);

        let after = std::fs::metadata(&lock).and_then(|m| m.modified()).ok();
        assert_eq!(
            before, after,
            "resolving must not touch the user's Cargo.lock"
        );
    }

    /// The cold-cache case, which is the one a user hits first: a project that has never been
    /// built has no `Cargo.lock`, and `--frozen` refuses rather than writing one.
    ///
    /// The assertion is that the *group is not silently empty* and that the repository is not
    /// touched. Cargo's own sentence is the row's text; paraphrasing it here would pin a string
    /// that cargo owns.
    #[test]
    #[ignore = "spawns the real cargo"]
    fn a_project_that_has_never_been_built_says_so_and_writes_no_lockfile() {
        let dir = temp("cold");
        std::fs::create_dir_all(dir.join("src")).expect("mkdir");
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname=\"cold\"\nversion=\"0.1.0\"\nedition=\"2021\"\n\
             [dependencies]\nserde=\"1\"\n",
        )
        .expect("write");
        std::fs::write(dir.join("src/main.rs"), "fn main(){}\n").expect("write");

        let resolved = resolve(std::slice::from_ref(&dir));
        assert!(resolved.packages.is_empty());
        assert_eq!(resolved.notes.len(), 1, "{resolved:?}");
        let note = &resolved.notes[0];
        assert!(
            note.contains("lock file") || note.contains("Cargo.lock"),
            "the row has to name the thing the user must fix: {note}"
        );
        assert!(
            !dir.join("Cargo.lock").exists(),
            "resolving created a lockfile in the user's project — `--frozen` is the only flag \
             combination that does not, and `--offline` was measured writing one"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
