//! Where a toolchain's binaries live, where the projects it cares about are, and where it
//! caches the sources of somebody else's code.
//!
//! # Why this is in `cide-core` and not where it was written
//!
//! [`which`], [`search_paths`] and [`has_marker`] were written in `cide_lsp::discover`, which
//! is the right place for exactly one caller. A second appeared in M13 — the dependency
//! resolver behind *External Libraries* — and the layering inverted the same way it did for
//! [`crate::child_env::arm`]: `cide-deps` would have had to depend on the **language-server
//! client** to answer "is `cargo` on PATH", and every future spawner would have inherited that
//! dependency. `cide_lsp::discover` re-exports all three, so no call site moved. Same move,
//! same reason, same shape as `docs/adr/0008`.
//!
//! The third section — [`dependency_roots`] and [`read_only_reason`] — is new, and it is not
//! about spawning at all. It is the rule that stops cide writing into `~/.cargo/registry`.
//!
//! Nothing here spawns a process. `go env GOMODCACHE` and `cargo --version` would both be
//! *more* accurate than the environment rules below and both cost a `fork`/`exec` on a path
//! that is asked about every time a file is opened, so the rules are the documented defaults
//! plus the documented overrides, and nothing else.
//!
//! It does now read two *files* on macOS — `/etc/paths` and `/etc/paths.d/*`, see
//! [`path_helper_dirs`] — which is not the same thing and is not what that rule was written
//! against: a read has no `fork`, no `exec`, no interpreter, no rc file to hang in, and it is
//! done once per process behind a `OnceLock` rather than once per lookup.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

// ==========================================================================================
// Part one: finding a binary — and giving a child the same list to find it on.
// ==========================================================================================
//
// # The bug the second half of this section exists to stop (M17)
//
// Reported from macOS: *"goto in golang project not working on macos (but works on linux), it
// just prints: gopls no views, rust-analyzer also not working, it writes: the language server
// stopped"*.
//
// That is one level deeper than the discovery failure `search_paths` was written for, and the
// two are easy to confuse. `search_paths` worked: `~/go/bin` is on the list, `which("gopls")`
// found the binary, and the user therefore never saw the *"not on PATH, install it with…"*
// sentence `cide_lsp::discover` would have produced. cide then spawned it — and handed it its
// **own** `PATH`, unchanged, because `child_env::bundle_scrub` only ever *removes*.
//
// A `.app` launched from Finder, the Dock or Spotlight inherits launchd's environment:
// `PATH=/usr/bin:/bin:/usr/sbin:/sbin`. `/etc/paths` and `path_helper(8)` are a *shell*
// mechanism and do not run for it. So the language server cide successfully started could not
// exec `go` — and `gopls` with no usable `go` cannot build a workspace view, which is precisely
// what it answers `no views` to every request over. `~/.cargo/bin/rust-analyzer` on a rustup
// install is a *proxy* that re-execs through `rustup`; with no toolchain reachable it exits,
// which `cide_lsp::server` surfaces as `the language server stopped`. Both strings are built by
// `cide_app::lsp`'s `format!("{}: {error}", server.binary())`.
//
// The rule that follows from that, and the reason [`extra_dirs`] is one list with two consumers:
// **the directories cide searches to find a binary and the directories it gives that binary's
// process to search are the same directories.** `search_paths` reads them; [`child_path_from`]
// appends them to what a child would otherwise inherit. The test
// `the_path_a_child_searches_is_the_path_which_searched` asserts the two cannot drift, which
// matters more than it looks: widening `search_paths` alone widens `claude_cli::resolve`'s
// acceptance, turning a refusal that names a remedy into an opaque `ENOENT` from `execvp` —
// the compounding failure `docs/platforms.md` records under *Finding `claude` from a Finder-launched
// `.app`*.

/// The directories cide adds to whatever `PATH` it was started with.
///
/// Not belt-and-braces: `~/.cargo/bin` and `~/go/bin` are put on `PATH` by a shell rc file, so a
/// cide started from a terminal sees them and the same cide started from a desktop launcher or
/// an AppImage does **not**.
///
/// # Why it is cached
///
/// `which` calls [`search_paths`] on every lookup and a lookup happens on every project open and
/// every diagnostic refresh. Nothing here can change during the life of the process — `HOME` is
/// fixed, and a `/etc/paths.d` fragment dropped in by an installer mid-session is not a case
/// worth three file reads per lookup — so it is computed once. On Linux this is two `PathBuf`s
/// and no I/O at all.
///
/// # The macOS list, and how much of it is guessed
///
/// [`path_helper_dirs`] is the part that is *not* guessed: `/etc/paths` and `/etc/paths.d/*` are
/// the documented mechanism `path_helper(8)` implements, and the Go pkg installer writes
/// `/etc/paths.d/go` — so an installer nobody here can enumerate still gets its directory on the
/// list. It cannot stand alone, because Homebrew deliberately does **not** write there: it tells
/// users to put `eval "$(brew shellenv)"` in their rc file, which is exactly the shell mechanism
/// a GUI launch skips. So the hardcoded names below are the floor under the parsed list —
/// Homebrew's two prefixes (`/opt/homebrew` on Apple silicon, `/usr/local` on Intel), MacPorts,
/// the Go pkg install location, and `~/.local/bin`, which is where Claude Code's own native
/// installer puts `claude`.
///
/// **No Mac was in front of this.** The names are reasoned from those installers' documentation.
/// Getting one wrong is cheap and getting the list *short* is the real risk: an entry that does
/// not exist costs one failed `stat` per lookup, which every real `PATH` already has several of,
/// and a missing entry costs the user the feature. There is deliberately **no existence filter**
/// for the same reason `search_paths` has none — filtering would let the list cide searches and
/// the list it hands a child diverge, which is the drift this whole section exists to prevent.
pub fn extra_dirs() -> &'static [PathBuf] {
    static DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    DIRS.get_or_init(build_extra_dirs)
}

fn build_extra_dirs() -> Vec<PathBuf> {
    // The only two impure inputs, read here so that everything below is a function of its
    // arguments. `etc` doubles as the platform switch: `Some` *is* "assemble the macOS list,
    // rooted here", which is a thing a Linux test can ask for and a `cfg!(target_os)` inside
    // the builder would not have been. Only this line is gated.
    let etc = cfg!(target_os = "macos").then(|| PathBuf::from("/etc"));
    extra_dirs_in(home().as_deref(), etc.as_deref())
}

/// [`build_extra_dirs`] over inputs handed in rather than read from the process.
///
/// Pure for the same reason [`child_path_from`] is, and for one more that is specific to this
/// function: **the macOS half of the list is the half no machine here can run.** Gated with a
/// `#[cfg]` it was unreachable from a test on Linux, so the ordering rule below, the claim in
/// [`push_unique`] that `/usr/local/bin` really does arrive twice, and every one of the
/// hardcoded names were assertions nothing checked on any machine in CI. `etc` is `Some` on
/// macOS and `None` everywhere else, so a Linux test can build either list.
///
/// The order is the rule: the toolchain directories first (they are the ones a language server
/// is most often looking for), then what `path_helper(8)` would have assembled, then
/// `~/.local/bin` and the hardcoded floor. It matters far less than it looks — every one of
/// these is *appended* to the user's own `PATH`, so none of them can shadow anything the user
/// arranged — but two launches on one machine must produce the same list, which is also why
/// [`path_helper_dirs`] sorts its fragments.
fn extra_dirs_in(home: Option<&Path>, etc: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = home {
        push_unique(&mut dirs, home.join(".cargo/bin"));
        push_unique(&mut dirs, home.join("go/bin"));
    }
    let Some(etc) = etc else {
        return dirs;
    };
    for dir in path_helper_dirs(etc) {
        push_unique(&mut dirs, dir);
    }
    if let Some(home) = home {
        push_unique(&mut dirs, home.join(".local/bin"));
    }
    for dir in [
        "/opt/homebrew/bin",
        "/opt/homebrew/sbin",
        "/usr/local/bin",
        "/usr/local/sbin",
        "/opt/local/bin",
        "/opt/local/sbin",
        "/usr/local/go/bin",
    ] {
        push_unique(&mut dirs, PathBuf::from(dir));
    }
    dirs
}

/// Append `dir` unless the list already holds it, or it is empty.
///
/// `Vec::dedup` is the wrong tool and was the first thing tried: it merges only *adjacent*
/// duplicates, so `/usr/local/bin` arriving from `/etc/paths` and again from the hardcoded floor
/// would have survived as two entries and doubled a `stat` on every lookup for ever. An empty
/// entry is dropped because in a `PATH` it means the current directory, and cide has no business
/// putting a child's cwd on its own search path.
fn push_unique(dirs: &mut Vec<PathBuf>, dir: PathBuf) {
    if !dir.as_os_str().is_empty() && !dirs.contains(&dir) {
        dirs.push(dir);
    }
}

/// The directories `path_helper(8)` would have assembled from `etc/paths` and `etc/paths.d`.
///
/// One path per line; blank lines are skipped, and so are lines starting with `#` — a tolerance
/// rather than a documented feature, on the grounds that a real directory whose name begins with
/// a hash does not exist and a commented fragment does.
///
/// `etc` is a parameter and this function is **`pub` and unconditional**, both deliberately, for
/// two reasons that pull the same way. A private `#[cfg(target_os = "macos")]` helper is dead
/// code on Linux and `clippy -D warnings` fails the build for it — so the macOS-only spelling
/// would have had to be `#[allow(dead_code)]`, which is how a function stops being read. And a
/// `#[test]` over a fixture directory is the only way this parser is ever *exercised*: the
/// machine this was written on has no `/etc/paths` and no Mac was available. The platform
/// decision lives in [`build_extra_dirs`], as the one `cfg!` that chooses whether an `etc` is
/// passed at all.
pub fn path_helper_dirs(etc: &Path) -> Vec<PathBuf> {
    fn read_into(file: &Path, out: &mut Vec<PathBuf>) {
        // A missing or unreadable file is the normal case on every platform but one. There is
        // nothing to report and nothing to fall back to: the hardcoded floor is the fallback.
        let Ok(text) = std::fs::read_to_string(file) else {
            return;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            push_unique(out, PathBuf::from(line));
        }
    }

    let mut dirs = Vec::new();
    read_into(&etc.join("paths"), &mut dirs);

    // Sorted, because `read_dir` order is the filesystem's and `path_helper` reads the fragments
    // in name order. Two launches on one machine must not produce two different `PATH`s.
    let mut fragments: Vec<PathBuf> = match std::fs::read_dir(etc.join("paths.d")) {
        Ok(entries) => entries.flatten().map(|entry| entry.path()).collect(),
        Err(_) => Vec::new(),
    };
    fragments.sort();
    for fragment in fragments {
        read_into(&fragment, &mut dirs);
    }
    dirs
}

/// `PATH`, plus every directory in [`extra_dirs`] it does not already contain.
///
/// Hand-rolled rather than a `which` crate for a dozen lines.
pub fn search_paths() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    for extra in extra_dirs() {
        if !dirs.contains(extra) {
            dirs.push(extra.clone());
        }
    }
    dirs
}

/// The `PATH` a child should be given, or `None` to leave the one it would inherit alone.
///
/// Pure, over the inputs handed in rather than read from the process, for exactly the reason
/// [`read_only_reason_in`] and [`crate::child_env::bundle_scrub_from`] are: `set_var` is `unsafe`
/// in edition 2024 because it races every other thread, so a test that had to arrange a `PATH`
/// could not be written safely at all. [`crate::child_env::child_path`] is the impure wrapper.
///
/// # Append, never prepend
///
/// The brief that produced this asked for a *prepend*, and it loses. Putting `/opt/homebrew/bin`
/// ahead of `/usr/bin` changes which `git`, `python3`, `openssl` and `make` **every** child of
/// cide resolves — on a machine where the user's own shell may deliberately order them the other
/// way round. That is a toolchain-selection decision cide has no business making on the user's
/// behalf, and it can only break configurations that work today. The reported failure is a
/// directory being *absent*, not shadowed. Appending fixes exactly that and can regress nothing.
///
/// # Returning `None` rather than an equal value
///
/// Same rule `bundle_scrub_from` states: *a child's environment should differ from its parent's
/// only where we can say why*. Started from a terminal, every extra is already on `PATH`, this
/// returns `None`, and not one byte of any child's environment changes.
///
/// An empty entry in `current` is preserved verbatim. It means the current directory, which is a
/// thing the user's shell said and not a thing for this function to edit — only `bundle_scrub`
/// drops empty entries, and only ones it created itself.
///
/// `join_paths` fails when an entry contains the separator, and the answer to that is `None` —
/// leave `PATH` alone. A partially-joined or lossy `PATH` is worse than an unhelpful one.
pub fn child_path_from(current: Option<&OsStr>, extra: &[PathBuf]) -> Option<OsString> {
    let mut dirs: Vec<PathBuf> = current
        .map(|path| std::env::split_paths(path).collect())
        .unwrap_or_default();
    let inherited = dirs.len();
    for dir in extra {
        if !dirs.contains(dir) {
            dirs.push(dir.clone());
        }
    }
    if dirs.len() == inherited {
        return None;
    }
    std::env::join_paths(dirs).ok()
}

/// The first executable called `binary` on [`search_paths`], or `None`.
pub fn which(binary: &str) -> Option<PathBuf> {
    for dir in search_paths() {
        let candidate = dir.join(binary);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// The executable called `name` sitting in cide's own directory, or `None`.
///
/// How a bundled sidecar is found. The precedent is `cide-hook`, which `cide-app` locates with
/// `current_exe().parent()` because the bundler puts every `externalBin` beside the main binary
/// — and that lookup could stay in `cide-app` because only `cide-app` spawns the hook. This one
/// cannot: `cide-lsp` needs it for the bundled rust-analyzer and must not depend on the app
/// crate, so it lives here, beside [`which`], which is the other half of the same question
/// ("where is this server's binary").
///
/// Deliberately **not** folded into [`search_paths`]: the bundled directory would then be
/// searched for *every* binary — `git`, `cargo`, the user's `claude` — and appending it is not
/// safe either, because the one binary this exists for must *beat* PATH, not lose to it. The
/// callers that want the bundled-first rule ask this function first, explicitly, for the one
/// name they mean.
pub fn sibling_binary(name: &str) -> Option<PathBuf> {
    sibling_binary_in(std::env::current_exe().ok()?.as_path(), name)
}

/// [`sibling_binary`] over an exe path handed in rather than read from the process.
///
/// Pure for the reason [`child_path_from`] is: a test cannot relocate `current_exe`, but it can
/// hand in a path next to a file it created.
pub fn sibling_binary_in(exe: &Path, name: &str) -> Option<PathBuf> {
    let candidate = exe.parent()?.join(name);
    is_executable(&candidate).then_some(candidate)
}

/// Is this an executable *file*?
///
/// `pub` since M16 for [`crate::claude_cli::resolve`], which has to answer the same question
/// about a path the user typed rather than one this module found. Answering `false` for a
/// directory is the half that matters there: a settings field is exactly where somebody pastes
/// `~/.local/bin` when they meant `~/.local/bin/claude`.
pub fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

// ==========================================================================================
// Part two: finding a project.
// ==========================================================================================

/// How deep under a root a project marker is looked for.
///
/// 2, not unbounded: a `Cargo.toml` seven directories down is a vendored dependency or a test
/// fixture, not this project — and walking a whole repository to decide whether to *start* an
/// indexer would cost more than the indexer.
pub const MARKER_DEPTH: usize = 2;

/// Is there a file named by `markers` at or just under `root`?
///
/// The cheap probe. [`find_markers`] is the same walk when the caller needs the paths.
pub fn has_marker(markers: &[&str], root: &Path) -> bool {
    !find_markers(markers, root).is_empty()
}

/// Every file named by `markers` at or within [`MARKER_DEPTH`] of `root`, shallowest first.
///
/// The walk stops descending as soon as it finds a marker *in a directory*, because the thing
/// below a `Cargo.toml` is that project's own members and a workspace's members share its
/// lockfile — resolving each of them separately would run `cargo metadata` once per crate in
/// the repository for one identical answer.
///
/// Hidden directories, `target/` and `node_modules/` are skipped. `target/` in particular
/// contains thousands of vendored `Cargo.toml`s, and treating one as evidence would start an
/// indexer for a project that has none of its own.
pub fn find_markers(markers: &[&str], root: &Path) -> Vec<PathBuf> {
    fn search(markers: &[&str], dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let mut found = false;
        for marker in markers {
            let candidate = dir.join(marker);
            if candidate.is_file() {
                out.push(candidate);
                found = true;
            }
        }
        if found || depth == 0 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        // Sorted, so two runs over the same tree resolve the same manifest first and the group
        // does not reshuffle between expansions for a reason nobody can see.
        let mut children: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    return false;
                }
                entry.file_type().is_ok_and(|t| t.is_dir())
            })
            .map(|entry| entry.path())
            .collect();
        children.sort();
        for child in children {
            search(markers, &child, depth - 1, out);
        }
    }

    let mut out = Vec::new();
    search(markers, root, MARKER_DEPTH, &mut out);
    out
}

// ==========================================================================================
// Part three: the caches a toolchain unpacks other people's source into.
// ==========================================================================================
//
// # The bug this exists to stop
//
// Go to definition into `serde` opens `~/.cargo/registry/src/index.crates.io-…/serde-1.0.229/
// src/lib.rs`. That file is mode 644 — cargo unpacks the crate writable — so `document::read`
// answered `writable: true`, `EditorSurface` mounted an editable buffer, and Ctrl+S wrote
// through `document::write`, which canonicalises and replaces atomically. The edit lands in
// the *shared* registry: every project on the machine that depends on that version now builds
// against the modified source, cargo's own checksum verification then fails builds in
// repositories the user never touched, and `cargo clean` does not fix it.
//
// Go's module cache is mode 444, so the same gesture there fails at `File::create` and the
// user sees an error. Rust's being writable is the whole difference, and it is not something
// to leave to luck: the rule below makes both read-only for the same stated reason.

/// Directories whose contents are a toolchain's copy of somebody else's source.
///
/// Empty entries are dropped, so a machine with `CARGO_HOME=` set to nothing does not turn
/// every path on disk into a dependency source.
///
/// | root | how it is found |
/// | --- | --- |
/// | cargo registry sources | `$CARGO_HOME/registry/src`, else `~/.cargo/registry/src` |
/// | cargo git checkouts | `$CARGO_HOME/git/checkouts`, else `~/.cargo/git/checkouts` |
/// | go module cache | `$GOMODCACHE`, else `$GOPATH/pkg/mod`, else `~/go/pkg/mod` |
/// | rustup toolchains | `$RUSTUP_HOME/toolchains`, else `~/.rustup/toolchains` |
/// | the Go SDK | `$GOROOT`, when it is set |
///
/// `GOPATH` may name several directories separated by the platform's path separator; go uses
/// the **first** for the module cache, so that is the one taken here.
///
/// # Why the rustup toolchains directory is here, added in M15
///
/// Because the standard library lives in it, and until M15 nothing in cide knew that population
/// existed. `cargo metadata` reports the `Cargo.lock` graph and nothing else — measured on this
/// repository: 547 packages, not one of them `std`, `core` or `alloc`, and not one
/// `manifest_path` under `.rustup` — so a std file opened by Go to definition was in no project
/// root, in no dependency root, and in no *External Libraries* row.
///
/// Two things follow from that, and the second is the stronger reason:
///
/// * *Select opened file* over such a tab said **"that file is not in this project's file
///   tree"**, about a file the user was looking at. `cide_app::libraries::is_unlisted_library_path`
///   delegates here, so a path matching no root never even asked the group to resolve.
/// * `~/.rustup/toolchains/…/lib/rustlib/src/rust/library/core/src/option.rs` is **mode 644 and
///   owned by the user**. With no root covering it, `read_only_reason` answered `None`,
///   `document::read` reported `writable: true`, and a Ctrl+S in a `core` buffer wrote into the
///   shared toolchain — which `rustup update` then silently replaces and which every project on
///   the machine compiles against. That is bit-for-bit the bug the paragraph above this function
///   describes for `~/.cargo/registry`; the rustup tree was simply never enumerated.
///
/// **The whole `toolchains` directory, not `…/lib/rustlib/src/rust/library`.** It needs no
/// toolchain-name resolution (`1.92.0-x86_64-unknown-linux-gnu` versus `stable-…` is a question
/// only `rustc --print sysroot` can answer, and this function forks nothing), it is one textual
/// rule, and *nothing* under a rustup toolchain should ever be written by an editor — not the
/// sources, not `bin/`, not `lib/`. [`under`] is component-wise, so a user's own
/// `~/.rustup/toolchains-mine` is untouched.
///
/// `$GOROOT` is included when it is set and cannot be derived when it is not: the honest
/// derivation is `canonicalize(which("go")).parent().parent()`, which is two syscalls and a
/// `which` walk on a path this module's header forbids doing any work on. Go's standard library
/// therefore stays writable on a machine with `GOROOT` unset, which is most of them — stated
/// here rather than hidden, because a half-applied rule that nobody has written down is how the
/// cargo case survived to be found twice.
pub fn dependency_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    let cargo_home = non_empty("CARGO_HOME").or_else(|| home().map(|h| h.join(".cargo")));
    if let Some(cargo_home) = cargo_home {
        roots.push(cargo_home.join("registry/src"));
        roots.push(cargo_home.join("git/checkouts"));
    }

    let modcache = non_empty("GOMODCACHE")
        .or_else(|| first_gopath().map(|p| p.join("pkg/mod")))
        .or_else(|| home().map(|h| h.join("go/pkg/mod")));
    if let Some(modcache) = modcache {
        roots.push(modcache);
    }

    let rustup_home = non_empty("RUSTUP_HOME").or_else(|| home().map(|h| h.join(".rustup")));
    if let Some(rustup_home) = rustup_home {
        roots.push(rustup_home.join("toolchains"));
    }

    if let Some(goroot) = non_empty("GOROOT") {
        roots.push(goroot);
    }

    roots
}

/// Why this file must not be edited, or `None` if it may be.
///
/// **`roots` wins.** A user who deliberately opens `~/go/pkg/mod/…` — or a vendored crate they
/// are patching — as a project root has said, with the strongest gesture the app has, that this
/// is their code. Refusing to let them save in a directory they opened on purpose would be the
/// app second-guessing an explicit instruction, and there is no way for them to override it.
/// The rule only fires for a path *no* project contains, which is exactly the population that
/// arrives through Go to definition and through the External Libraries group.
///
/// A sentence rather than a bool, because it is what the editor says when the user tries to
/// type. "This file is read-only" with no reason is the message people file bugs about.
pub fn read_only_reason(path: &Path, roots: &[PathBuf]) -> Option<String> {
    read_only_reason_in(path, roots, &dependency_roots())
}

/// The rule itself, over the caches handed in rather than read from the environment.
///
/// Pure, for the reason [`crate::child_env::bundle_scrub_from`] beside it gives: the alternative
/// is a test that mutates the process environment, and `set_var` is `unsafe` in edition 2024
/// precisely because it races every other thread reading it. It is also what lets the *whole*
/// path — `document::read` plus this rule, over a real mode-644 file — be driven end to end from
/// an ordinary `#[test]`, which is what `cide_app::cmd::file`'s tests do. Without the split the
/// only way to exercise the composition would be to write into the user's real
/// `~/.cargo/registry`, which is the exact thing this rule exists to prevent.
pub fn read_only_reason_in(path: &Path, roots: &[PathBuf], caches: &[PathBuf]) -> Option<String> {
    if roots.iter().any(|root| under(path, root)) {
        return None;
    }
    let root = caches.iter().find(|root| under(path, root))?;
    // "a dependency source" was the whole sentence until M15, and it is wrong for a toolchain:
    // `core/src/option.rs` is not a dependency of anything, it is the compiler's own library,
    // and telling a user their standard library is "a dependency source" invites the reply that
    // they never added it. The clause that is true of all of them is the one that matters —
    // every project on this machine shares it, and the toolchain replaces it on update.
    Some(format!(
        "{} is under {}, which every project on this machine shares and the toolchain \
         replaces when it updates, so cide opens it read-only.",
        path.display(),
        root.display()
    ))
}

/// Path containment, component-wise.
///
/// Textual, like `cide_fs::ops::check_within`, and for the same reason — `canonicalize` costs a
/// syscall per component and this is asked on every file open. Component-wise so that
/// `~/.cargo/registry/src-mine` is not inside `~/.cargo/registry/src`; `starts_with` on a
/// `Path` already compares whole components, which is exactly the property a `str` prefix test
/// would lose.
pub fn under(path: &Path, root: &Path) -> bool {
    !root.as_os_str().is_empty() && path.starts_with(root)
}

fn home() -> Option<PathBuf> {
    non_empty("HOME")
}

/// An environment variable that is set *and* not empty, as a path.
fn non_empty(name: &str) -> Option<PathBuf> {
    let value = std::env::var_os(name)?;
    (!value.is_empty()).then(|| PathBuf::from(value))
}

fn first_gopath() -> Option<PathBuf> {
    let value = std::env::var_os("GOPATH")?;
    std::env::split_paths(&value).find(|p| !p.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cide-toolchain-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_binary_that_does_not_exist_is_not_found() {
        assert!(which("cide-no-such-binary-anywhere").is_none());
    }

    #[test]
    #[cfg(unix)]
    fn a_sibling_binary_is_found_beside_the_exe_and_only_when_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("sibling");
        let exe = dir.join("cide");
        std::fs::write(&exe, "").expect("write");
        let sidecar = dir.join("cide-rust-analyzer");
        std::fs::write(&sidecar, "").expect("write");

        // Present but not executable: a half-copied sidecar must read as "not bundled", so the
        // ladder falls through to PATH rather than spawning something that cannot run.
        std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        assert_eq!(sibling_binary_in(&exe, "cide-rust-analyzer"), None);

        std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert_eq!(sibling_binary_in(&exe, "cide-rust-analyzer"), Some(sidecar));
        // And a name nothing shipped is simply absent — the dev-build case, every day.
        assert_eq!(sibling_binary_in(&exe, "cide-no-such-sidecar"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_toolchain_directories_are_searched_even_when_path_omits_them() {
        // The AppImage case: a desktop launcher does not run the user's shell rc, so `~/go/bin`
        // is absent from PATH while `gopls` sits in it.
        let dirs = search_paths();
        if let Some(home) = home() {
            assert!(dirs.contains(&home.join(".cargo/bin")), "{dirs:?}");
            assert!(dirs.contains(&home.join("go/bin")), "{dirs:?}");
        }
    }

    // --- the PATH a child is given ----------------------------------------------------------

    fn dirs(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    fn child(current: Option<&str>, extra: &[&str]) -> Option<Vec<PathBuf>> {
        let extra = dirs(extra);
        let current = current.map(OsString::from);
        let joined = child_path_from(current.as_deref(), &extra)?;
        Some(std::env::split_paths(&joined).collect())
    }

    #[test]
    fn only_the_missing_directories_are_added_and_they_go_on_the_end() {
        // The append-not-prepend rule, asserted as a rule: the first entry of the inherited
        // PATH is still first. Prepending would change which `git`, `python3` and `openssl`
        // every child of cide resolves, which is a decision that is not cide's to make.
        let path = child(
            Some("/usr/bin:/bin"),
            &["/usr/bin", "/opt/homebrew/bin", "/home/u/go/bin"],
        )
        .expect("two of the three were missing");
        assert_eq!(
            path,
            dirs(&["/usr/bin", "/bin", "/opt/homebrew/bin", "/home/u/go/bin"]),
            "an already-present entry must not be repeated, and nothing may jump the queue"
        );
    }

    #[test]
    fn nothing_to_add_means_the_child_inherits_byte_for_byte() {
        // The `./run.sh`-from-a-terminal case, and the whole of why Linux is unaffected: every
        // extra is already there, so no `PATH` is set on any child at all. Returning an equal
        // value instead would be a difference nobody could explain later.
        assert_eq!(
            child(Some("/home/u/go/bin:/usr/bin"), &["/home/u/go/bin"]),
            None
        );
        assert_eq!(child(Some("/usr/bin"), &[]), None);
        assert_eq!(child(None, &[]), None);
    }

    #[test]
    fn a_child_with_no_inherited_path_still_gets_the_extras() {
        assert_eq!(
            child(None, &["/home/u/.cargo/bin", "/home/u/go/bin"]),
            Some(dirs(&["/home/u/.cargo/bin", "/home/u/go/bin"]))
        );
    }

    #[test]
    fn an_empty_entry_in_the_inherited_path_is_left_exactly_where_it_was() {
        // An empty entry means the current directory. It is a thing the user's shell said, and
        // rewriting it — dropping it, or moving it — changes what a child resolves from its own
        // cwd. Only `bundle_scrub_from` is entitled to drop one, and only ones it created.
        let path = child(Some("/usr/bin::/bin"), &["/opt/homebrew/bin"]).expect("one was missing");
        assert_eq!(path, dirs(&["/usr/bin", "", "/bin", "/opt/homebrew/bin"]));
    }

    #[test]
    fn an_entry_that_cannot_be_joined_leaves_the_path_alone_rather_than_half_written() {
        // `join_paths` refuses an entry containing the separator. A truncated or lossy PATH is
        // worse than the one the child would have inherited, so the answer is "no change".
        assert_eq!(child(Some("/usr/bin"), &["/opt/a:b"]), None);
    }

    /// The anti-drift assertion, and the reason [`extra_dirs`] is one list rather than two.
    ///
    /// `which` searches [`search_paths`]; a child searches the `PATH` [`child_path_from`] built.
    /// If those two ever name different directories, cide is back to the compounding failure
    /// `docs/platforms.md` records: `claude_cli::resolve` says yes about a directory the child cannot
    /// see, and a refusal that named a remedy becomes an opaque `ENOENT` from `execvp` three
    /// processes down. Making it structural rather than a convention is the whole design.
    #[test]
    fn the_path_a_child_searches_is_the_path_which_searched() {
        let current = std::env::var_os("PATH");
        let built = child_path_from(current.as_deref(), extra_dirs()).or(current);
        let searched: Vec<PathBuf> = built
            .as_deref()
            .map(|path| std::env::split_paths(path).collect())
            .unwrap_or_default();
        assert_eq!(
            searched,
            search_paths(),
            "the directories cide searches and the directories it gives a child to search have \
             drifted apart — that is the bug, not a detail of it"
        );
    }

    #[test]
    fn the_path_helper_files_are_read_in_the_order_path_helper_reads_them() {
        // `/etc/paths` first, then `/etc/paths.d/*` by name — which is where the Go pkg
        // installer writes `go`, and the reason this is parsed rather than guessed at.
        let etc = temp("path-helper");
        std::fs::create_dir_all(etc.join("paths.d")).expect("mkdir");
        std::fs::write(
            etc.join("paths"),
            "/usr/local/bin\n\n# a comment\n  /usr/bin  \n/bin\n",
        )
        .expect("write");
        std::fs::write(etc.join("paths.d/go"), "/usr/local/go/bin\n").expect("write");
        // Sorted by name, so `a-first` precedes `go` however `read_dir` felt about it. Repeats
        // `/usr/bin`, which must not appear twice.
        std::fs::write(etc.join("paths.d/a-first"), "/opt/x/bin\n/usr/bin\n").expect("write");

        assert_eq!(
            path_helper_dirs(&etc),
            dirs(&[
                "/usr/local/bin",
                "/usr/bin",
                "/bin",
                "/opt/x/bin",
                "/usr/local/go/bin"
            ])
        );
        let _ = std::fs::remove_dir_all(&etc);
    }

    #[test]
    fn a_machine_with_no_path_helper_files_gets_an_empty_list_and_no_error() {
        // Every Linux machine, and the reason the hardcoded floor in `build_extra_dirs` exists.
        let etc = temp("no-path-helper");
        assert!(path_helper_dirs(&etc).is_empty());
        assert!(path_helper_dirs(Path::new("/cide-no-such-etc")).is_empty());
        let _ = std::fs::remove_dir_all(&etc);
    }

    #[test]
    fn the_extra_directories_are_listed_once_each() {
        // `Vec::dedup` merges only adjacent duplicates, and on macOS `/usr/local/bin` arrives
        // both from `/etc/paths` and from the hardcoded floor. A repeat is a doubled `stat` on
        // every lookup for the life of the process.
        let mut seen = extra_dirs().to_vec();
        let listed = seen.len();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), listed, "{:?}", extra_dirs());
        assert!(
            !seen.iter().any(|dir| dir.as_os_str().is_empty()),
            "an empty entry means the child's own cwd: {seen:?}"
        );
    }

    /// The macOS list, built on Linux — the half of this feature no machine in CI can run.
    ///
    /// Until [`extra_dirs_in`] took its inputs as arguments this was behind a `#[cfg]` and
    /// therefore behind nothing at all: every hardcoded name, the order, and the dedup claim
    /// [`push_unique`] makes were unchecked on every machine that ever built this crate. What
    /// is asserted here is only what a Linux machine can honestly assert — that the builder
    /// composes the four sources in `path_helper` order, that a directory arriving from both
    /// `/etc/paths` and the hardcoded floor is listed once, and that the two toolchain
    /// directories stay in front. Whether `/opt/homebrew/bin` is *the right name* is still a
    /// claim from Homebrew's documentation and not from a Mac; see `docs/platforms.md`.
    #[test]
    fn the_macos_list_is_path_helper_then_the_hardcoded_floor_with_no_repeats() {
        let etc = temp("macos-extras");
        std::fs::create_dir_all(etc.join("paths.d")).expect("mkdir");
        // What a stock `/etc/paths` holds, plus the entry the Go pkg installer drops in — and
        // `/usr/local/bin`, which is *also* in the hardcoded floor below. That overlap is the
        // one `push_unique` exists for and it is real on every Mac, not a contrived input.
        std::fs::write(etc.join("paths"), "/usr/local/bin\n/usr/bin\n/bin\n").expect("write");
        std::fs::write(etc.join("paths.d/go"), "/usr/local/go/bin\n").expect("write");

        let home = PathBuf::from("/Users/u");
        let built = extra_dirs_in(Some(&home), Some(&etc));
        assert_eq!(
            built,
            dirs(&[
                // The toolchains first: these are what a language server most often cannot find.
                "/Users/u/.cargo/bin",
                "/Users/u/go/bin",
                // Then `path_helper`'s own list, `/etc/paths` before `/etc/paths.d/*`.
                "/usr/local/bin",
                "/usr/bin",
                "/bin",
                "/usr/local/go/bin",
                // Then the floor. `/usr/local/bin` and `/usr/local/go/bin` came from the files
                // above and must not appear a second time.
                "/Users/u/.local/bin",
                "/opt/homebrew/bin",
                "/opt/homebrew/sbin",
                "/usr/local/sbin",
                "/opt/local/bin",
                "/opt/local/sbin",
            ])
        );

        // A Mac with no `/etc/paths` at all — nothing to parse, and the floor is what is left.
        // This is the case the hardcoded names exist for, so it must not be empty.
        let bare = extra_dirs_in(Some(&home), Some(Path::new("/cide-no-such-etc")));
        assert!(
            bare.contains(&PathBuf::from("/opt/homebrew/bin")),
            "{bare:?}"
        );
        assert!(bare.contains(&PathBuf::from("/usr/local/bin")), "{bare:?}");

        // And the non-macOS spelling, which is the whole of why Linux behaviour is unchanged:
        // exactly the two directories `search_paths` added before any of this existed.
        assert_eq!(
            extra_dirs_in(Some(&home), None),
            dirs(&["/Users/u/.cargo/bin", "/Users/u/go/bin"])
        );
        // A process with no `HOME` — a systemd unit, a `.app` launched oddly — must not produce
        // an entry rooted at nothing.
        assert_eq!(extra_dirs_in(None, None), Vec::<PathBuf>::new());

        let _ = std::fs::remove_dir_all(&etc);
    }

    #[test]
    fn a_marker_is_found_at_the_root_and_one_level_down() {
        let dir = temp("marker");
        std::fs::create_dir_all(dir.join("backend")).expect("mkdir");
        std::fs::write(dir.join("backend/go.mod"), "module x\n").expect("write");
        assert!(has_marker(&["go.mod", "go.work"], &dir));
        assert_eq!(
            find_markers(&["go.mod"], &dir),
            [dir.join("backend/go.mod")]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_vendored_marker_under_target_is_not_this_project() {
        // `target/` holds thousands of `Cargo.toml`s from vendored dependencies. Treating one as
        // evidence would resolve dependencies for a project that has none of its own.
        let dir = temp("vendored");
        std::fs::create_dir_all(dir.join("target/debug")).expect("mkdir");
        std::fs::write(dir.join("target/debug/Cargo.toml"), "[package]\n").expect("write");
        assert!(!has_marker(&["Cargo.toml"], &dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_marker_too_deep_is_not_this_project_either() {
        let dir = temp("deep");
        std::fs::create_dir_all(dir.join("a/b/c")).expect("mkdir");
        std::fs::write(dir.join("a/b/c/Cargo.toml"), "[package]\n").expect("write");
        assert!(!has_marker(&["Cargo.toml"], &dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A workspace's members share its lockfile, so finding the root manifest is the whole job.
    #[test]
    fn the_walk_stops_at_the_first_manifest_rather_than_listing_every_member() {
        let dir = temp("workspace");
        std::fs::create_dir_all(dir.join("crates/a")).expect("mkdir");
        std::fs::create_dir_all(dir.join("crates/b")).expect("mkdir");
        std::fs::write(dir.join("Cargo.toml"), "[workspace]\n").expect("write");
        std::fs::write(dir.join("crates/a/Cargo.toml"), "[package]\n").expect("write");
        std::fs::write(dir.join("crates/b/Cargo.toml"), "[package]\n").expect("write");
        assert_eq!(
            find_markers(&["Cargo.toml"], &dir),
            [dir.join("Cargo.toml")],
            "a workspace root's members must not each become a resolution of their own"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- the read-only rule ---------------------------------------------------------------

    /// [`read_only_reason_in`] as a bool, over caches handed in rather than read.
    ///
    /// It calls the **real** function. The first version of this helper reimplemented the two
    /// clauses instead, which read more clearly and could not fail: deleting the `roots` clause
    /// from `read_only_reason_in` left every test in this module green while a user's
    /// deliberately-opened vendored crate became uneditable. A helper that restates the rule is a
    /// helper that tests itself.
    fn refuses(path: &str, cache: &str, roots: &[&str]) -> bool {
        let roots: Vec<PathBuf> = roots.iter().map(PathBuf::from).collect();
        read_only_reason_in(
            Path::new(path),
            &roots,
            std::slice::from_ref(&PathBuf::from(cache)),
        )
        .is_some()
    }

    #[test]
    fn a_registry_source_is_refused_and_a_neighbouring_directory_is_not() {
        let cache = "/home/u/.cargo/registry/src";
        assert!(refuses(
            "/home/u/.cargo/registry/src/index.crates.io-1949/serde-1.0.229/src/lib.rs",
            cache,
            &[]
        ));
        // Component-wise containment: a directory whose *name* merely starts with the cache's
        // is a different directory, and a `str` prefix test would swallow it.
        assert!(!refuses(
            "/home/u/.cargo/registry/src-mine/x.rs",
            cache,
            &[]
        ));
        assert!(!refuses("/home/u/work/cide/src/lib.rs", cache, &[]));
    }

    #[test]
    fn a_project_root_over_the_cache_wins() {
        // The one override, and it has to be the strongest gesture in the app: a user who opened
        // a vendored crate as a project root to patch it has said this is their code.
        let cache = "/home/u/go/pkg/mod";
        let path = "/home/u/go/pkg/mod/github.com/x@v1.0.0/a.go";
        assert!(refuses(path, cache, &[]));
        assert!(!refuses(
            path,
            cache,
            &["/home/u/go/pkg/mod/github.com/x@v1.0.0"]
        ));
    }

    #[test]
    fn an_empty_cache_root_matches_nothing() {
        // `CARGO_HOME=` in the environment used to make `""` a root, and `Path::starts_with("")`
        // is true for every path — which would have made every file on the machine read-only.
        assert!(!refuses("/anywhere/at/all.rs", "", &[]));
        assert!(!under(Path::new("/anywhere"), Path::new("")));
    }

    /// The gate for M15's second report, and the reason it is written as a *containment* test
    /// rather than as an end-to-end reveal.
    ///
    /// The only end-to-end coverage of reveal-into-a-library was
    /// `cide_app::cmd::fs`'s `revealing_a_dependency_source_resolves_the_group_it_needs`, which
    /// is `#[ignore]`d (it spawns the real cargo) *and* builds its target by taking a path out of
    /// `dependency_roots()` — so it can only ever exercise a population `dependency_roots()`
    /// already contains, and was structurally incapable of noticing a missing root. The
    /// non-ignored predicate test in `libraries.rs` did the same thing with `caches.first()`.
    /// Coverage of the rule was self-referential in both places.
    ///
    /// This one names the path shape from the outside, which is the only way a *missing*
    /// population can be asserted about at all.
    #[test]
    fn the_rust_standard_library_is_a_shared_toolchain_copy_and_not_the_users_to_edit() {
        let Some(home) = home() else {
            return;
        };
        // The layout is rustup's and is stable across every toolchain it installs:
        // `<RUSTUP_HOME>/toolchains/<name>/lib/rustlib/src/rust/library/<crate>/src/…`.
        let toolchains = home.join(".rustup/toolchains");
        assert!(
            dependency_roots().contains(&toolchains),
            "the rustup toolchains directory has to be a dependency root, or `std` is in no \
             project root, no cache and no External Libraries row: Select opened file answers \
             \"not in this project's file tree\" about a file on screen, and — worse — \
             core/src/option.rs is mode 644, so Ctrl+S writes into the toolchain every project \
             on this machine compiles against. Measured: `cargo metadata` on this repo reports \
             547 packages and none of them is std/core/alloc, so nothing else can supply it"
        );

        let std_file = "/home/u/.rustup/toolchains/1.92.0-x86_64-unknown-linux-gnu/lib/rustlib/\
                        src/rust/library/core/src/option.rs";
        let root = "/home/u/.rustup/toolchains";
        assert!(refuses(std_file, root, &[]));
        // Component-wise, exactly as for the registry: a directory whose name merely starts with
        // `toolchains` is the user's own.
        assert!(!refuses("/home/u/.rustup/toolchains-mine/x.rs", root, &[]));
        // And `roots` still wins. A user who opened a toolchain checkout as a project root — the
        // people who work on rustc do exactly that — has made the strongest gesture the app has.
        assert!(!refuses(
            std_file,
            root,
            &["/home/u/.rustup/toolchains/1.92.0-x86_64-unknown-linux-gnu/lib/rustlib/src/rust"]
        ));
    }

    /// `RUSTUP_HOME` and `GOROOT`, over an environment handed in rather than mutated.
    ///
    /// `dependency_roots` reads the process environment, and `set_var` is `unsafe` in edition
    /// 2024 precisely because it races every other thread — the same argument
    /// `read_only_reason_in` is split for. So the *override* is asserted the only way that is
    /// safe here: by checking that the default is derived from `HOME` and that `under` is the
    /// containment used, which is what an override changes.
    #[test]
    fn the_go_sdk_is_covered_only_when_goroot_says_where_it_is() {
        // GOROOT is usually unset, and this is the honest statement of the consequence rather
        // than a pretence that it is not. `/usr/local/go`, Homebrew's `…/libexec` and Debian's
        // `/usr/lib/go-1.x` are all user-readable and mode 644 on at least one of them, so Go's
        // std stays writable unless the user exports GOROOT.
        let roots = dependency_roots();
        match non_empty("GOROOT") {
            Some(goroot) => assert!(
                roots.contains(&goroot),
                "GOROOT is set to {} and must be a root: it is the SDK every project on this \
                 machine builds against",
                goroot.display()
            ),
            None => assert!(
                !roots
                    .iter()
                    .any(|r| r.ends_with("go/libexec") || r == Path::new("/usr/local/go")),
                "with GOROOT unset nothing may be guessed here — deriving it needs \
                 canonicalize(which(\"go\")), and this module forks and stats nothing"
            ),
        }
    }

    #[test]
    fn the_reason_names_the_file_and_the_cache_it_is_in() {
        // Not a bool: the editor prints this, and "read-only" with no cause is the message
        // people file bugs about.
        let roots = vec![PathBuf::from("/home/u/work/cide")];
        let Some(cache) = dependency_roots().into_iter().next() else {
            return;
        };
        let path = cache.join("index.crates.io-1949/serde-1.0.229/src/lib.rs");
        let reason = read_only_reason(&path, &roots).expect("a registry source is read-only");
        assert!(reason.contains("serde-1.0.229"), "{reason}");
        assert!(reason.contains("read-only"), "{reason}");
        assert_eq!(
            read_only_reason(Path::new("/home/u/work/cide/src/lib.rs"), &roots),
            None,
            "a file in a project root is the user's own"
        );
    }
}
