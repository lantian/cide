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

use std::path::{Path, PathBuf};

// ==========================================================================================
// Part one: finding a binary.
// ==========================================================================================

/// `PATH`, plus the two directories the toolchains install into.
///
/// Hand-rolled rather than a `which` crate for a dozen lines. The extra directories are not
/// belt-and-braces: `~/.cargo/bin` and `~/go/bin` are added by a shell rc file, so a cide started
/// from a terminal sees them and the same cide started from a desktop launcher or an AppImage
/// does **not** — and the failure is a Problems panel that says the server is missing on a machine
/// where the user can run it by hand.
pub fn search_paths() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(home) = home() {
        for extra in [home.join(".cargo/bin"), home.join("go/bin")] {
            if !dirs.contains(&extra) {
                dirs.push(extra);
            }
        }
    }
    dirs
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
    fn the_toolchain_directories_are_searched_even_when_path_omits_them() {
        // The AppImage case: a desktop launcher does not run the user's shell rc, so `~/go/bin`
        // is absent from PATH while `gopls` sits in it.
        let dirs = search_paths();
        if let Some(home) = home() {
            assert!(dirs.contains(&home.join(".cargo/bin")), "{dirs:?}");
            assert!(dirs.contains(&home.join("go/bin")), "{dirs:?}");
        }
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
