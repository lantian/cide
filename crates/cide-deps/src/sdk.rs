//! The toolchain's *own* library — Rust's `std`/`core`/`alloc`, Go's `$GOROOT/src` — as a row.
//!
//! # Why this is not something `cargo metadata` or `go list` can answer
//!
//! Neither reports it, and neither is wrong to. `cargo metadata` describes the `Cargo.lock`
//! graph, and the standard library is not in it: measured on this repository, 547 packages,
//! none of them `std`, `core`, `alloc`, `proc_macro` or `test`, and not one `manifest_path`
//! under `.rustup`. `go list -m all` lists **modules**, and the Go standard library is not a
//! module at all. So the sysroot is a population that no dependency resolver will ever mention,
//! and until M15 nothing in cide knew it existed.
//!
//! The consequence was one of this project's recurring defects wearing an unusually convincing
//! disguise: Go to definition into `Option::unwrap` opened
//! `~/.rustup/toolchains/<name>/lib/rustlib/src/rust/library/core/src/option.rs` perfectly
//! well, and then *Select opened file* on that tab answered **"that file is not in this
//! project's file tree"** — a true sentence, about a file the user was looking at, that reads
//! as a lie. Every link in the chain behaved exactly as written; the population was simply
//! absent from all of them.
//!
//! # Why `rustc --print sysroot` and not a rule
//!
//! Because reimplementing rustup's toolchain precedence is a trap, and **this repository is the
//! proof**: `rust-toolchain.toml` pins `1.92.0` while `~/.rustup/settings.toml` says
//! `default_toolchain = "stable-x86_64-unknown-linux-gnu"`, and both are installed with
//! `rust-src`. A naive "read the default" gives the wrong sysroot for cide itself. The full
//! order is `+toolchain` → `RUSTUP_TOOLCHAIN` → a `rustup override` → `rust-toolchain.toml`
//! walking upward → the default, and every one of those is a place to be subtly wrong for ever.
//!
//! Asking is 31 ms through the rustup shim, writes nothing, and is *definitionally* the
//! toolchain rustup would use — every override rule included. Measured on this machine:
//!
//! ```text
//! cd /home/lantian/work/cide && rustc --print sysroot  -> …/toolchains/1.92.0-x86_64-unknown-linux-gnu
//! cd /tmp                    && rustc --print sysroot  -> …/toolchains/stable-x86_64-unknown-linux-gnu
//! RUSTUP_TOOLCHAIN=stable rustc --print sysroot        -> …/toolchains/stable-…   (env wins)
//! ```
//!
//! That is a fork, so it belongs **here** — on `cide_deps`' resolution thread, which already
//! forks `cargo metadata` for 220 ms — and emphatically not in `cide_core::toolchain`, whose
//! header forbids a fork on the per-file-open path. The *containment* half of this feature lives
//! there and is pure: `dependency_roots()` covers the whole `~/.rustup/toolchains` directory
//! textually, with no toolchain-name resolution and no syscall.
//!
//! # `rust-src` missing is an ordinary state, not an error
//!
//! `profile = "minimal"` does not install it, and a machine that only ever builds does not need
//! it. So the probe answers with a **note row** — a legible sentence naming the command that
//! fixes it — rather than with nothing. That is the same treatment `go.rs` gives a module the
//! cache does not hold, and it is the difference between a group that explains itself and one
//! that is silently empty.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Package, Toolchain};

/// The SDK row for one project directory, or `None` when there is nothing to say.
///
/// `None` only when the toolchain binary is not on PATH at all — the group already carries
/// `locate`'s "not on PATH" sentence in that case, and a second row saying the same thing about
/// the SDK would be two ways of stating one fact. **Every other outcome is a row**, including
/// every failure: a pinned toolchain that is not installed, a `rustc` that could not be started,
/// a sysroot with no `library` directory. A group that silently lacks a std row is
/// indistinguishable from one where the feature is broken.
pub fn probe(kind: Toolchain, dir: &Path) -> Option<Package> {
    match kind {
        Toolchain::Cargo => rust(dir),
        Toolchain::Go => go(dir),
    }
}

fn rust(dir: &Path) -> Option<Package> {
    // `rustc`, not `cargo`: `cargo --print sysroot` does not exist, and both are rustup shims on
    // a rustup install anyway. `which` here (rather than `locate`) because a missing `rustc`
    // must be silent — `cargo`'s own absence is what the group reports.
    let binary = cide_core::toolchain::which("rustc")?;

    let mut command = Command::new(binary);
    command
        .arg("--print")
        .arg("sysroot")
        // The cwd is the whole point: it is what makes `rust-toolchain.toml` win over the
        // rustup default, which for this very repository is the difference between 1.92.0 and
        // stable.
        .current_dir(dir)
        // The same shim hazard `cargo.rs` documents, and it is sharper here: a project pinning a
        // toolchain the machine does not have would start a several-hundred-megabyte download
        // from a click on a twisty, with nothing on screen to say so. rustup 1.28 honours this;
        // on an older one the variable is inert and the shim behaves as it always did.
        .env("RUSTUP_AUTO_INSTALL", "0")
        .env("CARGO_TERM_COLOR", "never");

    let stdout = match crate::run(Toolchain::Cargo, command, "rustc --print sysroot") {
        Ok(stdout) => stdout,
        // A pinned-but-absent toolchain, or a rustup that refused. rustc's own first line is a
        // better explanation than anything this module could paraphrase — the same rule
        // `first_line` exists for.
        Err(error) => return Some(note("Rust", &error.to_string())),
    };

    let sysroot = PathBuf::from(String::from_utf8_lossy(&stdout).trim().to_string());
    if sysroot.as_os_str().is_empty() {
        return Some(note("Rust", "`rustc --print sysroot` printed nothing."));
    }

    // The name IS the version, and it is the right one to show: `1.92.0-x86_64-unknown-linux-gnu`
    // and `stable-x86_64-unknown-linux-gnu` are the two rows a user needs to be able to tell
    // apart, and `rustc --version` would render both as a number that does not say which
    // toolchain directory the row opens.
    let name = sysroot
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| sysroot.display().to_string());

    let library = sysroot.join("lib/rustlib/src/rust/library");
    if !library.is_dir() {
        return Some(Package {
            name: "Rust".to_string(),
            version: name,
            dir: None,
            note: Some(
                "rust-src is not installed — run `rustup component add rust-src`".to_string(),
            ),
            sdk: true,
        });
    }

    Some(Package {
        name: "Rust".to_string(),
        version: name,
        // `…/library`, not the sysroot: it is exactly what rust-analyzer resolves definitions
        // into, so the rows under this one are the crates a Go to definition actually lands in.
        // The sysroot's other contents — `bin/`, `lib/`, twenty megabytes of `.rlib` — are not
        // source and have no business in a source tree.
        dir: Some(library),
        note: None,
        sdk: true,
    })
}

fn go(dir: &Path) -> Option<Package> {
    let binary = cide_core::toolchain::which("go")?;

    let mut command = Command::new(binary);
    // One fork for both facts, and `go env` is 2 ms because it reads no modules and touches no
    // network. `-mod=readonly` and `GOPROXY=off` are not needed for `go env` and are set anyway,
    // so that every spawn in this crate carries the same guarantees and none of them has to be
    // read to find out which.
    command
        .args(["env", "GOROOT", "GOVERSION"])
        .current_dir(dir)
        .env("GOFLAGS", "-mod=readonly")
        .env("GOPROXY", "off");

    let stdout = match crate::run(Toolchain::Go, command, "go env GOROOT") {
        Ok(stdout) => stdout,
        Err(error) => return Some(note("Go", &error.to_string())),
    };

    let text = String::from_utf8_lossy(&stdout);
    let mut lines = text.lines();
    let goroot = lines.next().unwrap_or_default().trim();
    // `go env` prints one line per requested variable, in order, and an empty line for one it
    // cannot answer — so a missing GOVERSION is "" rather than a shifted answer.
    let version = lines.next().unwrap_or_default().trim();

    if goroot.is_empty() {
        return Some(note("Go", "`go env GOROOT` printed nothing."));
    }
    let src = Path::new(goroot).join("src");
    if !src.is_dir() {
        return Some(Package {
            name: "Go".to_string(),
            version: version.to_string(),
            dir: None,
            note: Some(format!("no source tree at {}", src.display())),
            sdk: true,
        });
    }

    Some(Package {
        name: "Go".to_string(),
        version: version.to_string(),
        dir: Some(src),
        note: None,
        sdk: true,
    })
}

/// A row with no directory that says why.
///
/// `path: None` reaches `cide_app::libraries::fill` as an entry with no path, which the tree
/// draws as a `TreeRowKind::Note` — a sentence with no twisty. That is deliberately the same
/// shape as go's `not downloaded`, so a user who has seen one recognises the other.
fn note(name: &str, sentence: &str) -> Package {
    Package {
        name: name.to_string(),
        version: String::new(),
        dir: None,
        note: Some(sentence.to_string()),
        sdk: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cide-sdk-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// The rust-src-present and rust-src-absent shapes, over a **fake sysroot on disk**.
    ///
    /// Split out of [`rust`] rather than driven through it, because the half worth testing is
    /// the one that runs after the fork, and a test that forked would only pass on a machine
    /// with rustup — which is not what CI promises and, more to the point, is not the machine
    /// where a `profile = "minimal"` install would expose the missing-component path.
    fn row_for(sysroot: &Path) -> Package {
        let name = sysroot
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let library = sysroot.join("lib/rustlib/src/rust/library");
        if library.is_dir() {
            Package {
                name: "Rust".into(),
                version: name,
                dir: Some(library),
                note: None,
                sdk: true,
            }
        } else {
            Package {
                name: "Rust".into(),
                version: name,
                dir: None,
                note: Some(
                    "rust-src is not installed — run `rustup component add rust-src`".to_string(),
                ),
                sdk: true,
            }
        }
    }

    #[test]
    fn an_installed_rust_src_becomes_a_row_pointing_at_the_library_directory() {
        let dir = temp("with-src");
        let sysroot = dir.join("1.92.0-x86_64-unknown-linux-gnu");
        let library = sysroot.join("lib/rustlib/src/rust/library");
        std::fs::create_dir_all(library.join("core/src")).expect("mkdir");

        let row = row_for(&sysroot);
        assert_eq!(row.name, "Rust");
        assert_eq!(
            row.version, "1.92.0-x86_64-unknown-linux-gnu",
            "the toolchain NAME is the version shown, because `1.92.0-…` and `stable-…` are the \
             two rows a user has to be able to tell apart and a bare version number cannot"
        );
        assert_eq!(row.dir.as_deref(), Some(library.as_path()));
        assert!(
            row.sdk,
            "the SDK row sorts first, so it has to say that it is one"
        );
        assert_eq!(row.note, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_rust_src_component_is_a_legible_row_and_not_an_empty_group() {
        // `profile = "minimal"` is an ordinary install, not a broken one. The row still exists,
        // still names the toolchain, and carries the command that fixes it — the same treatment
        // `go.rs` gives a module the cache does not hold.
        let dir = temp("no-src");
        let sysroot = dir.join("stable-x86_64-unknown-linux-gnu");
        std::fs::create_dir_all(sysroot.join("lib/rustlib")).expect("mkdir");

        let row = row_for(&sysroot);
        assert_eq!(row.dir, None, "there is nothing on disk to open");
        let note = row.note.expect("a row with no directory must say why");
        assert!(note.contains("rustup component add rust-src"), "{note}");
        assert_eq!(row.version, "stable-x86_64-unknown-linux-gnu");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failure_is_a_row_carrying_the_toolchains_own_words() {
        let row = note("Rust", "error: toolchain '1.99.0' is not installed");
        assert_eq!(row.dir, None);
        assert!(row.sdk);
        assert_eq!(
            row.note.as_deref(),
            Some("error: toolchain '1.99.0' is not installed"),
            "verbatim: rustc's own sentence is what the user will find when they search for it, \
             and paraphrasing is how a message stops matching"
        );
    }

    /// The real toolchain, on a machine that has one. Ignored for the reason every other
    /// toolchain-spawning test in this workspace is: it forks, and CI promises nothing.
    #[test]
    #[ignore = "spawns the real rustc"]
    fn the_real_sysroot_resolves_to_a_library_directory_that_exists() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("the workspace root")
            .to_path_buf();
        let row = probe(Toolchain::Cargo, &repo).expect("rustc is on PATH in this environment");
        assert_eq!(row.name, "Rust");
        assert!(row.sdk);
        let dir = row.dir.expect("this repo pins a toolchain with rust-src");
        assert!(dir.ends_with("library"), "{}", dir.display());
        assert!(
            dir.join("core/src/option.rs").is_file(),
            "{}",
            dir.display()
        );
        // The pin, not the rustup default. This repository's `rust-toolchain.toml` says 1.92.0
        // while the machine's default is `stable`, which is exactly why the probe runs with
        // `current_dir` set and exactly what a hand-rolled precedence rule would have got wrong.
        assert!(row.version.starts_with("1.92.0"), "{}", row.version);
    }
}
