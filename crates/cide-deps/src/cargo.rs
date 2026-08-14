//! `cargo metadata`, and the two lines of its output that matter.
//!
//! Split from the go half because the two have nothing in common but their shape: cargo answers
//! with one 3 MB JSON document and no partial-failure mode, go answers with a stream of small
//! ones and a per-module error field. Trying to share a parser between them would mean a type
//! that is half of each.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::{DepsError, Package, Resolved, Toolchain};

/// `cargo metadata`'s output, cut down to the fields this crate reads.
///
/// `#[serde(rename_all)]` rather than `serde_json::Value` walking: the document is 3 MB and 607
/// packages, and a typo in a key name would be a silently empty group rather than a compile
/// error. Fields cargo emits that are not here are ignored by serde, which is what keeps this
/// working across cargo releases — the schema is versioned (`--format-version 1`) and additive.
#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<MetaPackage>,
    /// Package ids that are the project itself.
    workspace_members: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct MetaPackage {
    id: String,
    name: String,
    version: String,
    manifest_path: PathBuf,
    /// `null` for a workspace member **and** for a path dependency; `registry+…` or `git+…`
    /// otherwise. It is not the test for "is this the project" — `workspace_members` is.
    source: Option<String>,
}

/// Every external dependency of the workspace that owns `manifest`.
pub fn resolve(manifest: &Path) -> Resolved {
    match run(manifest) {
        Ok(packages) => Resolved {
            packages,
            notes: Vec::new(),
        },
        Err(error) => Resolved {
            packages: Vec::new(),
            notes: vec![error.to_string()],
        },
    }
}

fn run(manifest: &Path) -> Result<Vec<Package>, DepsError> {
    let binary = crate::locate(Toolchain::Cargo)?;
    let mut command = Command::new(binary);
    command
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        // `--frozen` is `--locked --offline`, and it is the only combination that resolves the
        // full graph without writing `Cargo.lock`. See the crate header; `--offline` alone was
        // measured creating one.
        .arg("--frozen")
        .arg("--manifest-path")
        .arg(manifest)
        // `~/.cargo/bin/cargo` is a symlink to `rustup` on any rustup install, and this
        // repository pins a toolchain in `rust-toolchain.toml`. A rustup shim **auto-installs a
        // pinned toolchain that is not present**, so without this a click on a twisty could
        // start a several-hundred-megabyte download with nothing on screen to say so. rustup
        // 1.28 honours this variable; on an older one the flag is inert and the shim behaves as
        // it always did. Same shim hazard `README.md` already documents for rust-analyzer.
        .env("RUSTUP_AUTO_INSTALL", "0")
        // Colour codes in the stderr line the failure row prints verbatim would reach the user
        // as `\u{1b}[1m\u{1b}[31merror`.
        .env("CARGO_TERM_COLOR", "never")
        .current_dir(manifest.parent().unwrap_or(Path::new(".")));

    let stdout = crate::run(Toolchain::Cargo, command, "cargo metadata")?;
    parse(&stdout)
}

/// The parsing half, over bytes rather than a process.
///
/// Separate so the shape of a real `cargo metadata` document can be a unit test on a machine
/// with no cargo, no network and no registry — which is the only way the field names above stay
/// honest, since getting one wrong produces an empty group and no error anywhere.
fn parse(stdout: &[u8]) -> Result<Vec<Package>, DepsError> {
    let metadata: Metadata = serde_json::from_slice(stdout).map_err(|e| {
        DepsError::Unreadable(format!("`cargo metadata` produced unreadable JSON: {e}"))
    })?;

    Ok(metadata
        .packages
        .into_iter()
        .filter(|package| !metadata.workspace_members.contains(&package.id))
        .map(|package| Package {
            name: package.name,
            version: match git_rev(package.source.as_deref()) {
                // A git dependency's `version` is whatever its own `Cargo.toml` says, which is
                // routinely `0.1.0` for every rev a branch ever had — two rows a month apart
                // would be indistinguishable. The short rev is the only part of a git source
                // that identifies which checkout this row opens.
                Some(rev) => format!("{} (git {rev})", package.version),
                None => package.version,
            },
            // The manifest's directory *is* the unpacked source. Not `stat`ed: `--frozen` makes
            // it impossible for cargo to report a package whose manifest it could not read, so a
            // `stat` per package would be 593 syscalls to confirm something already proven.
            //
            // The `source` is not shown. A row that said `registry+https://github.com/
            // rust-lang/crates.io-index` after every one of 593 names would be 593 copies of one
            // fact; a git or path dependency is worth distinguishing, and the honest way to do
            // that is the directory the row opens, which the user can see.
            dir: package.manifest_path.parent().map(Path::to_path_buf),
            note: None,
        })
        .collect())
}

/// The short revision out of a `git+…#<sha>` source, or `None` for anything else.
///
/// Textual on purpose. The full form is
/// `git+https://github.com/o/r?branch=main#0123456789abcdef0123456789abcdef01234567`, and the
/// only part worth showing in a 252px panel is the seven characters git itself abbreviates to.
/// A source with no `#` is a registry or a path and answers `None`.
fn git_rev(source: Option<&str>) -> Option<&str> {
    let source = source?;
    let rev = source.strip_prefix("git+")?.split('#').nth(1)?;
    Some(&rev[..rev.len().min(7)])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `cargo metadata --format-version 1` document, trimmed to the shape of a real one.
    ///
    /// Verbatim field names and verbatim id syntax from `cargo 1.92.0` over this repository:
    /// one workspace member, one registry dependency, one path dependency outside the workspace.
    const DOC: &str = r#"{
      "packages": [
        {
          "id": "path+file:///home/u/work/cide/crates/cide-app#0.1.0",
          "name": "cide-app",
          "version": "0.1.0",
          "manifest_path": "/home/u/work/cide/crates/cide-app/Cargo.toml",
          "source": null,
          "dependencies": []
        },
        {
          "id": "registry+https://github.com/rust-lang/crates.io-index#serde@1.0.229",
          "name": "serde",
          "version": "1.0.229",
          "manifest_path": "/home/u/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/serde-1.0.229/Cargo.toml",
          "source": "registry+https://github.com/rust-lang/crates.io-index"
        },
        {
          "id": "path+file:///home/u/work/sibling#0.3.0",
          "name": "sibling",
          "version": "0.3.0",
          "manifest_path": "/home/u/work/sibling/Cargo.toml",
          "source": null
        },
        {
          "id": "git+https://github.com/o/r?branch=main#0123456789abcdef0123456789abcdef01234567#pinned@0.1.0",
          "name": "pinned",
          "version": "0.1.0",
          "manifest_path": "/home/u/.cargo/git/checkouts/r-9f1/0123456/Cargo.toml",
          "source": "git+https://github.com/o/r?branch=main#0123456789abcdef0123456789abcdef01234567"
        }
      ],
      "workspace_members": ["path+file:///home/u/work/cide/crates/cide-app#0.1.0"],
      "workspace_root": "/home/u/work/cide",
      "target_directory": "/home/u/work/cide/target",
      "version": 1
    }"#;

    #[test]
    fn a_workspace_member_is_the_project_and_is_not_a_dependency() {
        let packages = parse(DOC.as_bytes()).expect("parse");
        assert!(
            !packages.iter().any(|p| p.name == "cide-app"),
            "the project's own crates are already drawn above this group: {packages:?}"
        );
    }

    #[test]
    fn a_registry_dependency_carries_its_version_and_its_unpacked_directory() {
        let packages = parse(DOC.as_bytes()).expect("parse");
        let serde = packages
            .iter()
            .find(|p| p.name == "serde")
            .expect("serde is a dependency");
        assert_eq!(serde.version, "1.0.229");
        assert_eq!(
            serde.dir.as_deref(),
            Some(Path::new(
                "/home/u/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/serde-1.0.229"
            )),
            "the row opens the directory the manifest sits in"
        );
        assert_eq!(serde.note, None);
    }

    /// `source: null` means "not from a registry", which covers both the project and a path
    /// dependency. Using it as the membership test would drop every sibling crate the user is
    /// developing against — the most useful rows the group can have.
    #[test]
    fn a_path_dependency_outside_the_workspace_is_a_dependency() {
        let packages = parse(DOC.as_bytes()).expect("parse");
        let sibling = packages
            .iter()
            .find(|p| p.name == "sibling")
            .expect("a path dependency outside the workspace belongs in the group");
        assert_eq!(
            sibling.dir.as_deref(),
            Some(Path::new("/home/u/work/sibling"))
        );
    }

    /// A git dependency's `version` is its own manifest's, which is the same string for every
    /// rev the branch ever had. Two rows a month apart would otherwise be identical.
    #[test]
    fn a_git_dependency_shows_the_rev_that_actually_identifies_it() {
        let packages = parse(DOC.as_bytes()).expect("parse");
        let pinned = packages
            .iter()
            .find(|p| p.name == "pinned")
            .expect("a git dependency belongs in the group");
        assert_eq!(pinned.version, "0.1.0 (git 0123456)");
        assert_eq!(git_rev(Some("registry+https://x")), None);
        assert_eq!(git_rev(None), None);
    }

    #[test]
    fn unreadable_output_is_reported_rather_than_becoming_an_empty_group() {
        let Err(DepsError::Unreadable(reason)) = parse(b"not json at all") else {
            panic!("garbage on stdout must not resolve to zero dependencies");
        };
        assert!(reason.contains("cargo metadata"), "{reason}");
    }

    /// The one failure a user is most likely to hit, and the sentence they get for it.
    ///
    /// `--frozen` refuses rather than writing, so a project that has never been built reports
    /// this instead of quietly creating a `Cargo.lock` the user did not ask for.
    #[test]
    fn a_stale_lockfile_reports_cargos_own_words() {
        let stderr = "error: the lock file /home/u/p/Cargo.lock needs to be updated but \
                      --frozen was passed to prevent this\n";
        assert_eq!(
            crate::first_line(stderr, Toolchain::Cargo),
            stderr.trim(),
            "cargo's own sentence is better than any paraphrase, and it is what the user finds \
             when they search for it"
        );
    }
}
