//! Is there a server to run, and is there anything for it to look at?
//!
//! Two probes, and both produce a **sentence the user can act on** rather than a status code. The
//! prose is built here, where the fact is known — the same call `cide_app::cmd::file`'s
//! `not_connected` makes, and for the same reason: a frontend that has to turn
//! `Unavailable(NotFound)` into English is a frontend that will word it differently from the next
//! surface that needs the same sentence.
//!
//! # Where `which` went
//!
//! [`which`], `search_paths` and the marker walk are now `cide_core::toolchain` and re-exported
//! from here, so no call site moved. They left because a second caller appeared that must not
//! depend on this crate: the dependency resolver behind *External Libraries* asks "is `cargo` on
//! PATH" and "is there a `Cargo.toml` under this root", and routing that through the
//! **language-server client** would have been the same inversion `docs/adr/0008` records for
//! `child_env::arm`. This module keeps what is genuinely about servers: which binary, which
//! marker files, and the sentence to print when either is missing.

use std::path::{Path, PathBuf};

/// Re-exported so `cide_lsp::discover::which` keeps working. See the module note.
pub use cide_core::toolchain::which;

/// A language server cide knows how to drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Server {
    RustAnalyzer,
    Gopls,
}

impl Server {
    pub fn binary(self) -> &'static str {
        match self {
            Self::RustAnalyzer => "rust-analyzer",
            Self::Gopls => "gopls",
        }
    }

    pub fn source(self) -> cide_ipc::DiagnosticSourceId {
        match self {
            Self::RustAnalyzer => cide_ipc::DiagnosticSourceId::RustAnalyzer,
            Self::Gopls => cide_ipc::DiagnosticSourceId::Gopls,
        }
    }

    /// The `languageId` its documents carry.
    pub fn language_id(self) -> &'static str {
        match self {
            Self::RustAnalyzer => "rust",
            Self::Gopls => "go",
        }
    }

    /// How to install it, in the words the user would type.
    fn install_hint(self) -> &'static str {
        match self {
            Self::RustAnalyzer => "rustup component add rust-analyzer",
            Self::Gopls => "go install golang.org/x/tools/gopls@latest",
        }
    }

    /// A file whose presence means this project is worth analysing.
    fn project_markers(self) -> &'static [&'static str] {
        match self {
            Self::RustAnalyzer => &["Cargo.toml"],
            Self::Gopls => &["go.mod", "go.work"],
        }
    }

    fn project_kind(self) -> &'static str {
        match self {
            Self::RustAnalyzer => "Cargo",
            Self::Gopls => "Go module",
        }
    }
}

/// Where a server was found, or why it was not.
#[derive(Debug, Clone)]
pub enum Found {
    Ready(PathBuf),
    /// The binary is not installed, or this project has nothing for it to do. Carries the
    /// sentence the panel prints verbatim.
    Missing(String),
}

/// Both probes, in the order that gives the more useful sentence first.
pub fn find(server: Server, roots: &[PathBuf]) -> Found {
    let Some(binary) = which(server.binary()) else {
        return Found::Missing(format!(
            "{} is not on PATH. Install it with `{}`.{}",
            server.binary(),
            server.install_hint(),
            extra_paths_hint(server),
        ));
    };
    if !roots.iter().any(|root| has_marker(server, root)) {
        return Found::Missing(format!(
            "No {} project under this project's roots, so {} was not started.",
            server.project_kind(),
            server.binary(),
        ));
    }
    Found::Ready(binary)
}

/// Only mentioned when the toolchain's own directory exists but is not on `PATH`, because that is
/// the one case where the user's next question is "but I have it installed".
fn extra_paths_hint(server: Server) -> String {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return String::new();
    };
    let dir = match server {
        Server::RustAnalyzer => home.join(".cargo/bin"),
        Server::Gopls => home.join("go/bin"),
    };
    if dir.join(server.binary()).is_file() {
        format!(
            " (found in {} — cide searched there too, so this build could not execute it)",
            dir.display()
        )
    } else {
        String::new()
    }
}

fn has_marker(server: Server, root: &Path) -> bool {
    cide_core::toolchain::has_marker(server.project_markers(), root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cide-lsp-discover-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_project_with_no_roots_never_starts_a_server() {
        // Whether or not rust-analyzer is installed on the machine running this, a project with
        // no roots has nothing to analyse — so this asserts the same thing on every machine,
        // which a test keyed on "is the binary present" would not.
        for server in [Server::RustAnalyzer, Server::Gopls] {
            let Found::Missing(reason) = find(server, &[]) else {
                panic!(
                    "{} was started for a project with no roots",
                    server.binary()
                );
            };
            // Not a code: the panel prints this verbatim, and "unavailable" tells nobody
            // anything. Whichever probe failed, the sentence names the binary.
            assert!(reason.contains(server.binary()), "{reason}");
        }
    }

    #[test]
    fn a_binary_that_does_not_exist_is_not_found() {
        // The other half — `which` itself, with a name no machine has.
        assert!(which("cide-no-such-language-server").is_none());
    }

    #[test]
    fn a_project_with_no_marker_does_not_start_a_server() {
        // rust-analyzer outside a Cargo project indexes nothing and costs hundreds of megabytes.
        let dir = temp("nomarker");
        std::fs::write(dir.join("notes.txt"), "").expect("write");
        let Found::Missing(reason) = find(Server::RustAnalyzer, std::slice::from_ref(&dir)) else {
            panic!("started a server for a project with no Cargo.toml");
        };
        assert!(reason.contains("Cargo"), "{reason}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_marker_is_found_at_the_root_and_one_level_down() {
        let dir = temp("marker");
        std::fs::create_dir_all(dir.join("backend")).expect("mkdir");
        std::fs::write(dir.join("backend/go.mod"), "module x\n").expect("write");
        assert!(has_marker(Server::Gopls, &dir));

        let flat = temp("marker-flat");
        std::fs::write(flat.join("Cargo.toml"), "[package]\n").expect("write");
        assert!(has_marker(Server::RustAnalyzer, &flat));

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&flat);
    }

    #[test]
    fn a_vendored_marker_under_target_is_not_this_project() {
        // `target/` holds thousands of `Cargo.toml`s from vendored dependencies. Treating one as
        // evidence would start an indexer for a project that has none of its own.
        let dir = temp("vendored");
        std::fs::create_dir_all(dir.join("target/debug")).expect("mkdir");
        std::fs::write(dir.join("target/debug/Cargo.toml"), "[package]\n").expect("write");
        assert!(!has_marker(Server::RustAnalyzer, &dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_marker_too_deep_is_not_this_project_either() {
        let dir = temp("deep");
        std::fs::create_dir_all(dir.join("a/b/c")).expect("mkdir");
        std::fs::write(dir.join("a/b/c/Cargo.toml"), "[package]\n").expect("write");
        assert!(!has_marker(Server::RustAnalyzer, &dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // `the_toolchain_directories_are_searched_even_when_path_omits_them` moved with
    // `search_paths` into `cide_core::toolchain`, where the function now lives. It is asserted
    // there rather than restated here, because two copies of one claim is how one of them stops
    // being true.

    #[test]
    fn each_server_knows_its_own_language_id_and_source() {
        assert_eq!(Server::RustAnalyzer.language_id(), "rust");
        assert_eq!(Server::Gopls.language_id(), "go");
        assert_eq!(
            Server::RustAnalyzer.source(),
            cide_ipc::DiagnosticSourceId::RustAnalyzer
        );
    }
}
