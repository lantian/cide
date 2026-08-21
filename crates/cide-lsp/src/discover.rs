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

/// A language server cide can drive.
///
/// # Why this is an index and not an enum
///
/// It was an enum with two variants until M22 — `RustAnalyzer` and `Gopls` — with six `match` arms
/// here, one in `session.rs` and four in `cide-app`. That shape had a consequence nobody set out
/// to choose: `cide_app::lsp::server_for` derived a server from `cide_lang::Lang`, so *"a language
/// cide can outline"* and *"a language cide can run a server for"* were forced to be the same set —
/// and `Lang` is closed because every variant costs a statically linked C parser table. A YAML
/// server would therefore have needed a tree-sitter grammar for YAML, which is an absurd price for
/// a `--stdio` flag.
///
/// So the arms became rows. [`Server`] is a `Copy` handle into a process-global table installed
/// once at startup: the two builtins from `cide_ipc::lang::builtin_servers`, plus whatever the
/// enabled extensions contribute. Every property that used to be a `match` — the binary, the
/// arguments, the language ids, the project markers — is a field on [`cide_ipc::lang::LanguageServerDef`].
///
/// Staying `Copy` and `Ord` is what kept the change to a rename at the ninety-odd call sites that
/// hold one in a `BTreeMap` key, compare two, or put one in a `Vec`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Server(u16);

/// One row of the table.
struct Row {
    def: cide_ipc::lang::LanguageServerDef,
    /// Whether this server is currently contributed. A row for a disabled extension stays — see
    /// [`install`] — and is simply not started.
    active: bool,
}

/// The installed table. Read from every thread.
///
/// **Append-only.** A row is never removed and never moves, so a [`Server`] handed to a session
/// three minutes ago still resolves to the row it was minted for. That is the invariant, and it
/// is not a micro-optimisation: `Server` is a `Copy` index precisely so it can live in a
/// `BTreeMap` key and be compared in ninety places, and an index that shifted when the user
/// disabled some *other* extension would make a live rust-analyzer session start reporting its
/// diagnostics under a different server's name.
///
/// A `RwLock` and not a `OnceLock` because enabling an extension changes the *active* set while
/// the app is running. The alternative was to stop every server before reinstalling, which is
/// correct and costs a full rust-analyzer re-index — 2–8 GB and thirty seconds — every time
/// somebody toggles a YAML extension.
static REGISTRY: std::sync::RwLock<Vec<Row>> = std::sync::RwLock::new(Vec::new());

/// Install the contributed server set, keeping every index that already exists.
///
/// Merged by binary name: a server already in the table keeps its row and is marked active, and
/// one that is not in `defs` is marked inactive rather than removed. The definition is *updated*
/// in place, because an extension may have been updated with different `args` and the next start
/// should use them.
///
/// Returns the handles for `defs`, in the order given.
pub fn install(defs: Vec<cide_ipc::lang::LanguageServerDef>) -> Vec<Server> {
    let Ok(mut registry) = REGISTRY.write() else {
        return Vec::new();
    };
    for row in registry.iter_mut() {
        row.active = false;
    }
    let mut handles = Vec::with_capacity(defs.len());
    for def in defs {
        let at = match registry.iter().position(|row| row.def.binary == def.binary) {
            Some(at) => {
                registry[at].def = def;
                registry[at].active = true;
                at
            }
            None => {
                registry.push(Row { def, active: true });
                registry.len() - 1
            }
        };
        handles.push(Server(u16::try_from(at).unwrap_or(u16::MAX)));
    }
    handles
}

/// Every server currently contributed, in registry order.
///
/// The *active* rows, not every row the table has ever held: a server whose extension has been
/// disabled keeps its index — see [`install`] — and must not be started.
#[must_use]
pub fn servers() -> Vec<Server> {
    if let Ok(registry) = REGISTRY.read()
        && !registry.is_empty()
    {
        return registry
            .iter()
            .enumerate()
            .filter(|(_, row)| row.active)
            .filter_map(|(at, _)| u16::try_from(at).ok())
            .map(Server)
            .collect();
    }
    (0..cide_ipc::lang::builtin_servers().len())
        .filter_map(|at| u16::try_from(at).ok())
        .map(Server)
        .collect()
}

/// One row, falling back to the builtins when nothing has been installed.
///
/// The fallback is not a convenience. `install` is called by `cide-app` once the extension store
/// has been read, and everything that runs before that — this crate's own tests, `cide-headless`,
/// a `cargo test -p cide-lsp` on a machine with no `$XDG_CONFIG_HOME` — would otherwise see an
/// empty table and conclude cide drives no language servers at all. The builtins are the honest
/// answer to *"what would be installed if nobody had said otherwise"*, so they are what an
/// uninitialised registry reports.
///
/// Resolves an **inactive** row too, deliberately: a session started before its extension was
/// disabled still holds a handle, and it needs a binary name for its shutdown log and its last
/// diagnostics rather than `<unknown>`.
fn row(at: usize) -> Option<cide_ipc::lang::LanguageServerDef> {
    if let Ok(registry) = REGISTRY.read()
        && !registry.is_empty()
    {
        return registry.get(at).map(|row| row.def.clone());
    }
    cide_ipc::lang::builtin_servers().into_iter().nth(at)
}

/// The server whose `binary` is this, if one is installed.
#[must_use]
pub fn by_binary(binary: &str) -> Option<Server> {
    servers().into_iter().find(|s| s.binary() == binary)
}

/// The server that owns a `languageId`, if one is installed.
///
/// First match wins, and `cide_ext::contribute` has already made that deterministic: it refuses a
/// second server with the same binary and reports a language nothing contributes.
#[must_use]
pub fn by_language(language_id: &str) -> Option<Server> {
    servers().into_iter().find(|s| s.owns_language(language_id))
}

impl Server {
    /// The handle a lookup answers with when the registry has no such row.
    ///
    /// Not a panic and not an `Option`: this is reachable only if a `Server` outlived an
    /// `install`, which `cide_app::lsp` prevents, and the useful behaviour for a bug that should
    /// not happen is a server called `<unknown>` that fails to start with a legible sentence —
    /// not a window that closes.
    pub const UNKNOWN: Server = Server(u16::MAX);

    /// The two builtins, at the indices `cide_ipc::lang::builtin_servers` puts them.
    ///
    /// Named constants rather than lookups because ninety call sites used to name a variant and
    /// most of them still mean exactly one server: `run_flycheck` is a rust-analyzer request,
    /// `didChangeWatchedFiles` batching is a gopls one, and the go-to-implementation path asks
    /// rust-analyzer a question only it answers. `cide-app` installs the builtins first and in
    /// this order, and `builtins_are_where_the_constants_say` pins it.
    pub const RUST_ANALYZER: Server = Server(0);
    pub const GOPLS: Server = Server(1);

    /// This server's row.
    #[must_use]
    pub fn def(self) -> cide_ipc::lang::LanguageServerDef {
        row(self.0 as usize).unwrap_or_else(|| cide_ipc::lang::LanguageServerDef {
            binary: "<unknown>".into(),
            args: Vec::new(),
            language_ids: Vec::new(),
            project_markers: Vec::new(),
            project_kind: "unknown".into(),
            install_hint: "reinstall the extension that provided it".into(),
            declares_watched_files: false,
            extra_path_hints: Vec::new(),
        })
    }

    #[must_use]
    pub fn binary(self) -> String {
        self.def().binary
    }

    /// The arguments it is spawned with.
    ///
    /// New in M22, and the field that turned out to matter most: `server.rs` built
    /// `Command::new(binary)` with nothing after it, and both builtins speak LSP on stdio with no
    /// flags, so nothing noticed. `yaml-language-server` needs `--stdio` and says so in its own
    /// README.
    #[must_use]
    pub fn args(self) -> Vec<String> {
        self.def().args
    }

    #[must_use]
    pub fn source(self) -> cide_ipc::DiagnosticSourceId {
        cide_ipc::DiagnosticSourceId::for_server(&self.binary())
    }

    /// The `languageId` its documents carry.
    ///
    /// The first of its `language_ids`, for the callers that send one document and need one word.
    /// A server driving several languages — which nothing does yet and a manifest may — gets the
    /// right answer from [`Self::owns_language`] instead.
    #[must_use]
    pub fn language_id(self) -> String {
        self.def().language_ids.first().cloned().unwrap_or_default()
    }

    #[must_use]
    pub fn owns_language(self, language_id: &str) -> bool {
        self.def().language_ids.iter().any(|id| id == language_id)
    }

    /// Whether the server declares `workspace/didChangeWatchedFiles` and should be sent them.
    #[must_use]
    pub fn declares_watched_files(self) -> bool {
        self.def().declares_watched_files
    }

    /// How to install it, in the words the user would type.
    fn install_hint(self) -> String {
        self.def().install_hint
    }

    /// Files whose presence mean a directory is worth analysing.
    ///
    /// An **empty** list means *any root*, which is right for a server that analyses single files
    /// — `sqls` has no manifest to look for — and would be wrong for anything that resolves a
    /// dependency graph. Neither builtin uses it; both name a manifest.
    fn project_markers(self) -> Vec<String> {
        self.def().project_markers
    }

    fn project_kind(self) -> String {
        self.def().project_kind
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
    let Some(binary) = which(&server.binary()) else {
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
    let binary = server.binary();
    for hint in server.def().extra_path_hints {
        // `~` and nothing else, matching `cide_ext::market::expand`: a hint out of a manifest that
        // could name `$ANYTHING` would mean a different directory on the next machine, and this
        // one is used to tell the user where cide looked.
        let dir = match hint.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(&hint),
        };
        if dir.join(&binary).is_file() {
            return format!(
                " (found in {} — cide searched there too, so this build could not execute it)",
                dir.display()
            );
        }
    }
    String::new()
}

fn has_marker(server: Server, root: &Path) -> bool {
    let markers = server.project_markers();
    // No markers means any root will do — see `Server::project_markers`.
    if markers.is_empty() {
        return true;
    }
    let refs: Vec<&str> = markers.iter().map(String::as_str).collect();
    cide_core::toolchain::has_marker(&refs, root)
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
        for server in [Server::RUST_ANALYZER, Server::GOPLS] {
            let Found::Missing(reason) = find(server, &[]) else {
                panic!(
                    "{} was started for a project with no roots",
                    server.binary()
                );
            };
            // Not a code: the panel prints this verbatim, and "unavailable" tells nobody
            // anything. Whichever probe failed, the sentence names the binary.
            assert!(reason.contains(&server.binary()), "{reason}");
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
        let Found::Missing(reason) = find(Server::RUST_ANALYZER, std::slice::from_ref(&dir)) else {
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
        assert!(has_marker(Server::GOPLS, &dir));

        let flat = temp("marker-flat");
        std::fs::write(flat.join("Cargo.toml"), "[package]\n").expect("write");
        assert!(has_marker(Server::RUST_ANALYZER, &flat));

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
        assert!(!has_marker(Server::RUST_ANALYZER, &dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_marker_too_deep_is_not_this_project_either() {
        let dir = temp("deep");
        std::fs::create_dir_all(dir.join("a/b/c")).expect("mkdir");
        std::fs::write(dir.join("a/b/c/Cargo.toml"), "[package]\n").expect("write");
        assert!(!has_marker(Server::RUST_ANALYZER, &dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // `the_toolchain_directories_are_searched_even_when_path_omits_them` moved with
    // `search_paths` into `cide_core::toolchain`, where the function now lives. It is asserted
    // there rather than restated here, because two copies of one claim is how one of them stops
    // being true.

    #[test]
    fn each_server_knows_its_own_language_id_and_source() {
        assert_eq!(Server::RUST_ANALYZER.language_id(), "rust");
        assert_eq!(Server::GOPLS.language_id(), "go");
        assert_eq!(
            Server::RUST_ANALYZER.source(),
            cide_ipc::DiagnosticSourceId::rust_analyzer()
        );
    }
}
