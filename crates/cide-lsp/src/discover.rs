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
    tracing::info!(
        binaries = ?defs.iter().map(|d| d.binary.as_str()).collect::<Vec<_>>(),
        "server registry install"
    );
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
            init_options: None,
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
    /// A caller that knows which document it is labelling must use [`Self::language_id_for`]
    /// instead — for a server driving several languages, the first id is the wrong label for
    /// every document but one.
    #[must_use]
    pub fn language_id(self) -> String {
        self.def().language_ids.first().cloned().unwrap_or_default()
    }

    /// The `languageId` to put on one document: the document's own resolved language when this
    /// server owns it, the first declared id otherwise — the pre-M34 behaviour, and the honest
    /// fallback for a caller that could not resolve the document at all.
    ///
    /// The didOpen label is what picks the dialect parser inside a multi-language server: one
    /// declaring `["css", "scss", "less"]` must see an SCSS document labelled `scss`, or
    /// `vscode-css-language-server` parses it as plain CSS and reports every nested rule as an
    /// error.
    #[must_use]
    pub fn language_id_for(self, document_language: Option<&str>) -> String {
        match document_language {
            Some(id) if self.owns_language(id) => id.to_string(),
            _ => self.language_id(),
        }
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
    /// dependency graph. Neither builtin uses it; both name a manifest. A `*.ext` entry matches
    /// any file with that extension (`toolchain::find_markers` is the matcher and carries the
    /// argument): the form for a server whose projects may have no manifest at all, like
    /// `typescript-language-server` over a plain HTML+JS folder.
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

/// The servers cide ships its own build of, and what the shipped file is called.
///
/// Registry name → sidecar file name, and the two are different **on purpose**. The registry
/// keeps saying `rust-analyzer` — it is what the Problems panel prints, what
/// `DiagnosticSourceId::for_server` keys on, and what `cide_ext::contribute`'s conflict rule
/// guards — while the file beside cide's binary is `cide-rust-analyzer`, because the tarball's
/// contract is "put `bin/` on your PATH" and a file there named `rust-analyzer` would shadow
/// the user's own for every shell on the machine. That is the toolchain-selection decision
/// `cide_core::toolchain`'s append-never-prepend note says cide must never make.
///
/// A const table rather than a `LanguageServerDef` field, also on purpose: a manifest field
/// would let an extension name an arbitrary sibling binary — `cide` itself, say — as its
/// server, and nothing needs the generality. cide ships what cide ships.
///
/// The third column is the developer override: an environment variable naming a build to use
/// **instead of everything**, so the fork can be run from its own `target/release` without
/// packaging a sidecar first. Set and broken is a refusal, not a fall-through — an override
/// that silently degraded to PATH would have the developer debugging the wrong binary.
const BUNDLED: &[(&str, &str, &str)] = &[
    ("rust-analyzer", "cide-rust-analyzer", "CIDE_RA_PATH"),
    ("gopls", "cide-gopls", "CIDE_GOPLS_PATH"),
];

/// Where a resolved binary came from. Decides whether cide's own configuration is sent — see
/// `session::Session::with_init_options`: the shipped build is configured, a stock one gets
/// byte-for-byte the handshake it always got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// The env override in [`BUNDLED`]'s third column — a developer's own build of the fork.
    Override,
    /// The sidecar shipped beside cide's own binary.
    Bundled,
    /// The user's own installation, from `PATH` — how every server resolved before M25.
    SystemPath,
    /// A directory an extension's `extraPathHints` named, or a Node version-manager directory
    /// (`cide_core::node_dirs`) — where `npm -g` puts a server on a machine whose shell rc,
    /// not its desktop launcher, put that directory on `PATH`. The user's own installation
    /// still, never cide's build, so `config.rs`'s `Bundled | Override` gate leaves it
    /// unconfigured exactly like [`Self::SystemPath`].
    HintDir,
}

/// One way to run a server. [`locate`] returns them best-first.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: PathBuf,
    pub provenance: Provenance,
    /// Directories this binary's process must have on its `PATH` beyond what every child
    /// already gets: empty for Override/Bundled/SystemPath — byte-identical behaviour to
    /// before the hint rung existed — and, for a [`Provenance::HintDir`] candidate, the
    /// directory the binary was found in plus the directory `node` lives in when `PATH`
    /// reaches none. The latter because an npm server is a `#!/usr/bin/env node` script:
    /// `execve` succeeds and the shebang dies with `env: node: No such file or directory`
    /// (`child_env::prepare_command_with`'s documented failure). This field is what makes the
    /// marketplace README's "appended to the child's PATH" sentence about `extraPathHints`
    /// true.
    pub child_path_dirs: Vec<PathBuf>,
}

/// Every way this server's binary can be run, best first — or the sentence for why none can.
///
/// The ladder: env override (wins alone), the bundled sidecar, `PATH`, then the hint
/// directories — the manifest's `extraPathHints` and the Node installation directories, for
/// the npm-installed servers a desktop launch's `PATH` never reaches. `choice` is the user's
/// say over the bundled rung — [`ServerBinaryChoice::System`] skips it, nothing else changes —
/// and the override outranks the setting because it exists precisely for testing a build the
/// setting does not know about.
///
/// More than one candidate is returned when more than one exists, and the caller is expected
/// to try them **in order**: a bundled build that dies before its handshake falls back to the
/// system one (see `server::supervise_lives`), so a broken sidecar degrades to exactly the
/// behaviour cide had before it shipped one.
///
/// The project-marker probe still gates everything: with no `Cargo.toml` there is nothing to
/// analyse, whichever binary exists.
pub fn locate(
    server: Server,
    roots: &[PathBuf],
    choice: cide_ipc::settings::ServerBinaryChoice,
) -> Result<Vec<Candidate>, String> {
    let binary = server.binary();
    let bundled_row = BUNDLED.iter().find(|(name, _, _)| *name == binary);
    let dirs = hint_dirs(server);
    let probes = Probes {
        override_env: bundled_row.and_then(|(_, _, env)| {
            let value = PathBuf::from(std::env::var_os(env)?);
            Some(if cide_core::toolchain::is_executable(&value) {
                Ok(value)
            } else {
                Err(value)
            })
        }),
        bundled: bundled_row
            .and_then(|(_, sidecar, _)| cide_core::toolchain::sibling_binary(sidecar)),
        system: which(&binary),
        hints: dirs
            .iter()
            .map(|dir| dir.join(&binary))
            .filter(|file| cide_core::toolchain::is_executable(file))
            .collect(),
        node_dir: match which("node") {
            Some(_) => None,
            None => dirs
                .iter()
                .find(|dir| cide_core::toolchain::is_executable(&dir.join("node")))
                .cloned(),
        },
    };
    let candidates = match ladder(probes, choice) {
        Ok(candidates) => candidates,
        Err(Refusal::BrokenOverride(value)) => {
            let env = bundled_row.map(|(_, _, env)| *env).unwrap_or_default();
            return Err(format!(
                "{env} is set to {}, which is not an executable file, so {binary} was not \
                 started. Unset it to fall back to the bundled or installed build.",
                value.display(),
            ));
        }
        Err(Refusal::NothingFound) => {
            // `cide-spec`'s launcher clause, for the same reason: a user who installed the
            // server in a terminal must hear why this app cannot see it.
            return Err(format!(
                "{binary} is not on PATH, nor in any Node installation directory cide \
                 searched. Install it with `{}`. A cide started from a desktop launcher has \
                 a different PATH from one started in a terminal.{}",
                server.install_hint(),
                extra_paths_hint(server),
            ));
        }
    };
    if !roots.iter().any(|root| has_marker(server, root)) {
        return Err(format!(
            "No {} project under this project's roots, so {binary} was not started.",
            server.project_kind(),
        ));
    }
    Ok(candidates)
}

/// What the three probes said, before any rule is applied. See [`ladder`].
struct Probes {
    /// The override variable's verdict, when it is set at all: `Ok` the executable it names,
    /// `Err` the broken value — kept for the sentence, because "CIDE_RA_PATH is set to <what>"
    /// is the difference between a ten-second fix and an hour in the log.
    override_env: Option<Result<PathBuf, PathBuf>>,
    /// The sidecar beside cide's own binary, when it exists and can run.
    bundled: Option<PathBuf>,
    /// `PATH`, via [`which`].
    system: Option<PathBuf>,
    /// Executable `binary` files found in hint directories, best first: the extension's own
    /// `extraPathHints` (`~`-expanded), then the Node installation directories from
    /// `cide_core::node_dirs`. Full paths to the files, not the directories, so the ladder
    /// stays a function of facts already probed.
    hints: Vec<PathBuf>,
    /// Where `node` lives when `which("node")` answered nothing — the interpreter an npm
    /// script's shebang needs. `None` when the child's `PATH` already reaches one.
    node_dir: Option<PathBuf>,
}

#[derive(Debug)]
enum Refusal {
    /// The override is set and names something that cannot run. Deliberately **not** a
    /// fall-through: an explicit override that silently degraded would have its author
    /// debugging a binary they were not running.
    BrokenOverride(PathBuf),
    NothingFound,
}

/// The resolution rule, pure over facts already probed — for the reason
/// `cide_core::toolchain::child_path_from` is: `set_var` is unsafe in edition 2024, so a test
/// that had to arrange the environment could not be written safely at all.
fn ladder(
    probes: Probes,
    choice: cide_ipc::settings::ServerBinaryChoice,
) -> Result<Vec<Candidate>, Refusal> {
    if let Some(verdict) = probes.override_env {
        return match verdict {
            // Alone, not first: everything below exists to be fallen back to, and the one
            // thing an override must never do is quietly become something else.
            Ok(path) => Ok(vec![Candidate {
                path,
                provenance: Provenance::Override,
                child_path_dirs: Vec::new(),
            }]),
            Err(value) => Err(Refusal::BrokenOverride(value)),
        };
    }
    let mut candidates = Vec::new();
    if choice == cide_ipc::settings::ServerBinaryChoice::Builtin
        && let Some(path) = probes.bundled
    {
        candidates.push(Candidate {
            path,
            provenance: Provenance::Bundled,
            child_path_dirs: Vec::new(),
        });
    }
    if let Some(path) = probes.system {
        candidates.push(Candidate {
            path,
            provenance: Provenance::SystemPath,
            child_path_dirs: Vec::new(),
        });
    }
    // The hint rung, after PATH: a user-managed PATH install wins over anything a manifest
    // guessed at, and a hint that yields nothing simply contributes no candidate. Deliberately
    // *not* the override's refuse-when-broken rule — a hint is a manifest author's guess about
    // somebody else's machine, the override is a developer's instruction about their own.
    // `ServerBinaryChoice::System` does not skip this rung either: its documented meaning is
    // "skip the bundled build, nothing else changes", and a hint-dir binary is the user's own
    // installation.
    for path in probes.hints {
        let mut child_path_dirs = Vec::new();
        if let Some(parent) = path.parent() {
            child_path_dirs.push(parent.to_path_buf());
        }
        if let Some(node_dir) = probes.node_dir.as_ref()
            && !child_path_dirs.contains(node_dir)
        {
            child_path_dirs.push(node_dir.clone());
        }
        candidates.push(Candidate {
            path,
            provenance: Provenance::HintDir,
            child_path_dirs,
        });
    }
    if candidates.is_empty() {
        return Err(Refusal::NothingFound);
    }
    Ok(candidates)
}

/// Both probes, in the order that gives the more useful sentence first.
///
/// [`locate`] with the default binary choice, folded to its first candidate — kept because
/// "which one binary would run" is the question the tests and the throwaway callers ask, and
/// the ladder behind it answers identically for every server without a bundled build.
pub fn find(server: Server, roots: &[PathBuf]) -> Found {
    match locate(
        server,
        roots,
        cide_ipc::settings::ServerBinaryChoice::default(),
    ) {
        Ok(candidates) => Found::Ready(
            candidates
                .into_iter()
                .next()
                .map(|candidate| candidate.path)
                .unwrap_or_default(),
        ),
        Err(reason) => Found::Missing(reason),
    }
}

/// Every directory the hint rung searches, best first: the manifest's own `extraPathHints`,
/// then the Node installation directories.
///
/// The Node directories are probed for **every** server, not just ones whose manifest wrote
/// hints — an extension cannot spell nvm's versioned `~/.nvm/versions/node/v22.x/bin` path,
/// and the whole rung exists for the desktop launch whose `PATH` no shell rc widened. The
/// override and bundled rungs still outrank everything here, so the two shipped servers
/// cannot be shadowed by a Node directory.
fn hint_dirs(server: Server) -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut dirs: Vec<PathBuf> = server
        .def()
        .extra_path_hints
        .iter()
        .filter_map(|hint| expand_hint(hint, home.as_deref()))
        .collect();
    for dir in cide_core::node_dirs::enumerate() {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// `~` and nothing else, matching `cide_ext::market::expand`: a hint out of a manifest that
/// could name `$ANYTHING` would mean a different directory on the next machine, and the same
/// expansion decides both where cide searches and where it tells the user it searched.
fn expand_hint(hint: &str, home: Option<&Path>) -> Option<PathBuf> {
    match hint.strip_prefix("~/") {
        Some(rest) => Some(home?.join(rest)),
        None => Some(PathBuf::from(hint)),
    }
}

/// Only mentioned when a hint directory holds the file and the ladder still found nothing —
/// which, now that executable hits are candidates in their own right, means a binary cide
/// could not execute. That is the one case where the user's next question is "but I have it
/// installed", so the sentence names the directory.
fn extra_paths_hint(server: Server) -> String {
    let binary = server.binary();
    for dir in hint_dirs(server) {
        if dir.join(&binary).is_file() {
            return format!(
                " (found in {} — cide searched there too, so this build could not execute it)",
                dir.display()
            );
        }
    }
    String::new()
}

/// Whether this server has anything to look at under `root` — its project marker
/// (`Cargo.toml`, `go.mod`) exists somewhere the walk reaches.
///
/// `pub` since M25 for one caller beyond [`locate`]: `cide-app`'s `sync_servers` asks it
/// *before* attempting a start, because the answer decides whether the server appears in
/// the Problems panel at all. A server with no project here is not "unavailable" — that
/// word is for something the user might fix — it is simply not applicable, and a Rust
/// workspace listing gopls as a greyed row was reporting a fact about cide's server table,
/// not about the workspace.
pub fn applicable(server: Server, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| has_marker(server, root))
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

    #[test]
    fn applicability_is_the_marker_and_nothing_else() {
        let dir = temp("applicable");
        // A Rust project: gopls has no business here, rust-analyzer does.
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"t\"\n").unwrap();
        let roots = vec![dir.clone()];
        assert!(applicable(Server::RUST_ANALYZER, &roots));
        assert!(!applicable(Server::GOPLS, &roots));
        // The marker appearing is the whole test for a mixed repo.
        std::fs::write(dir.join("go.mod"), "module t\n").unwrap();
        assert!(applicable(Server::GOPLS, &roots));
        let _ = std::fs::remove_dir_all(&dir);
    }

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

    // --- the ladder ---------------------------------------------------------------------
    //
    // Driven through the pure rule over pre-probed facts, because the real probes read the
    // environment and `set_var` is unsafe in edition 2024 — the same reason
    // `cide_core::toolchain::child_path_from` is pure.

    fn probed(
        override_env: Option<Result<&str, &str>>,
        bundled: Option<&str>,
        system: Option<&str>,
    ) -> Probes {
        Probes {
            override_env: override_env
                .map(|verdict| verdict.map(PathBuf::from).map_err(PathBuf::from)),
            bundled: bundled.map(PathBuf::from),
            system: system.map(PathBuf::from),
            hints: Vec::new(),
            node_dir: None,
        }
    }

    fn shape(candidates: &[Candidate]) -> Vec<(Provenance, &str)> {
        candidates
            .iter()
            .map(|c| (c.provenance, c.path.to_str().unwrap_or_default()))
            .collect()
    }

    #[test]
    fn an_override_wins_alone_rather_than_first() {
        // Everything below the override exists to be fallen back to; the override itself must
        // never quietly become something else, so it is the *only* candidate.
        let got = ladder(
            probed(
                Some(Ok("/fork/ra")),
                Some("/app/cide-rust-analyzer"),
                Some("/usr/bin/ra"),
            ),
            cide_ipc::settings::ServerBinaryChoice::Builtin,
        )
        .expect("resolved");
        assert_eq!(shape(&got), vec![(Provenance::Override, "/fork/ra")]);
    }

    #[test]
    fn a_broken_override_refuses_rather_than_falling_through() {
        // A developer whose CIDE_RA_PATH points at a typo must hear that, not silently debug
        // the PATH build for an hour.
        let refusal = ladder(
            probed(
                Some(Err("/fork/typo")),
                Some("/app/cide-rust-analyzer"),
                Some("/usr/bin/ra"),
            ),
            cide_ipc::settings::ServerBinaryChoice::Builtin,
        );
        assert!(matches!(refusal, Err(Refusal::BrokenOverride(_))));
    }

    #[test]
    fn the_bundled_build_beats_path_and_path_remains_the_fallback() {
        let got = ladder(
            probed(None, Some("/app/cide-rust-analyzer"), Some("/usr/bin/ra")),
            cide_ipc::settings::ServerBinaryChoice::Builtin,
        )
        .expect("resolved");
        assert_eq!(
            shape(&got),
            vec![
                (Provenance::Bundled, "/app/cide-rust-analyzer"),
                (Provenance::SystemPath, "/usr/bin/ra"),
            ],
            "the system build must stay on the list — a bundled build that fails to start \
             falls back to it"
        );
    }

    #[test]
    fn with_nothing_bundled_the_ladder_is_exactly_the_old_behaviour() {
        // Every dev build (no sidecar beside a debug binary), and any future server that has
        // no bundled row: PATH, alone.
        let got = ladder(
            probed(None, None, Some("/usr/bin/some-ls")),
            cide_ipc::settings::ServerBinaryChoice::Builtin,
        )
        .expect("resolved");
        assert_eq!(
            shape(&got),
            vec![(Provenance::SystemPath, "/usr/bin/some-ls")]
        );
    }

    #[test]
    fn the_bundled_table_names_both_shipped_servers() {
        // The strings are contracts, not configuration: the sidecar file name meets
        // packaging's install step, and the env var is what a developer types. A rename on
        // either side of those meetings is silent breakage, so pin them here.
        assert_eq!(
            BUNDLED,
            [
                ("rust-analyzer", "cide-rust-analyzer", "CIDE_RA_PATH"),
                ("gopls", "cide-gopls", "CIDE_GOPLS_PATH"),
            ]
        );
    }

    #[test]
    fn choosing_system_skips_a_bundled_build_that_exists() {
        let got = ladder(
            probed(None, Some("/app/cide-rust-analyzer"), Some("/usr/bin/ra")),
            cide_ipc::settings::ServerBinaryChoice::System,
        )
        .expect("resolved");
        assert_eq!(shape(&got), vec![(Provenance::SystemPath, "/usr/bin/ra")]);
    }

    #[test]
    fn choosing_system_with_nothing_on_path_is_missing_not_a_quiet_fallback() {
        // The user said System; running the bundled build anyway would be the settings row
        // lying. The Missing sentence names the install command, which is the remedy they
        // asked for.
        let refusal = ladder(
            probed(None, Some("/app/cide-rust-analyzer"), None),
            cide_ipc::settings::ServerBinaryChoice::System,
        );
        assert!(matches!(refusal, Err(Refusal::NothingFound)));
    }

    #[test]
    fn nothing_anywhere_is_nothing_found() {
        let refusal = ladder(
            probed(None, None, None),
            cide_ipc::settings::ServerBinaryChoice::Builtin,
        );
        assert!(matches!(refusal, Err(Refusal::NothingFound)));
    }

    // --- the hint rung ------------------------------------------------------------------
    //
    // The rung that exists for npm-installed servers: a desktop launch's PATH reaches no
    // Node directory, so the manifest's `extraPathHints` and `cide_core::node_dirs` are
    // searched after everything else. A hint never refuses — with no hits anywhere the
    // refusal is `NothingFound`, exactly as before the rung existed. Deliberate asymmetry
    // with `a_broken_override_refuses_rather_than_falling_through`: a hint is a manifest
    // author's guess about somebody else's machine, the override is a developer's
    // instruction about their own.

    #[test]
    fn path_beats_a_hint_and_the_hint_stays_on_the_ladder() {
        let mut p = probed(None, None, Some("/usr/bin/typescript-language-server"));
        p.hints = vec![PathBuf::from(
            "/home/u/.nvm/versions/node/v22.0.0/bin/typescript-language-server",
        )];
        let got = ladder(p, cide_ipc::settings::ServerBinaryChoice::Builtin).expect("resolved");
        assert_eq!(
            shape(&got),
            vec![
                (
                    Provenance::SystemPath,
                    "/usr/bin/typescript-language-server"
                ),
                (
                    Provenance::HintDir,
                    "/home/u/.nvm/versions/node/v22.0.0/bin/typescript-language-server",
                ),
            ],
            "a user-managed PATH install wins over a manifest's guess, and the hint stays a \
             fallback for a PATH build that dies before its handshake"
        );
        assert!(
            got[0].child_path_dirs.is_empty(),
            "a PATH binary gets the PATH every child gets — byte-identical to before the rung"
        );
    }

    #[test]
    fn a_hint_candidate_carries_the_directory_it_was_found_in() {
        // The half the whole rung exists for: the binary is a `#!/usr/bin/env node` script,
        // and without its own directory (and node's) on the child's PATH, execve succeeds
        // and the shebang dies with `env: node: No such file or directory`.
        let mut p = probed(None, None, None);
        p.hints = vec![PathBuf::from("/home/u/.npm-global/bin/svelteserver")];
        p.node_dir = Some(PathBuf::from("/home/u/.nvm/versions/node/v22.0.0/bin"));
        let got = ladder(p, cide_ipc::settings::ServerBinaryChoice::Builtin).expect("resolved");
        assert_eq!(
            got[0].child_path_dirs,
            vec![
                PathBuf::from("/home/u/.npm-global/bin"),
                PathBuf::from("/home/u/.nvm/versions/node/v22.0.0/bin"),
            ]
        );

        // When the hint directory *is* node's directory, it is not listed twice.
        let mut p = probed(None, None, None);
        p.hints = vec![PathBuf::from(
            "/home/u/.nvm/versions/node/v22.0.0/bin/svelteserver",
        )];
        p.node_dir = Some(PathBuf::from("/home/u/.nvm/versions/node/v22.0.0/bin"));
        let got = ladder(p, cide_ipc::settings::ServerBinaryChoice::Builtin).expect("resolved");
        assert_eq!(
            got[0].child_path_dirs,
            vec![PathBuf::from("/home/u/.nvm/versions/node/v22.0.0/bin")]
        );
    }

    #[test]
    fn a_hint_resolves_alone_and_the_system_choice_does_not_skip_it() {
        // `ServerBinaryChoice::System`'s documented meaning is "skip the bundled build,
        // nothing else changes" — and a hint-dir binary is the user's own installation, so
        // it stays reachable under that choice.
        let mut p = probed(None, Some("/app/cide-x"), None);
        p.hints = vec![PathBuf::from("/home/u/.npm-global/bin/x")];
        let got = ladder(p, cide_ipc::settings::ServerBinaryChoice::System).expect("resolved");
        assert_eq!(
            shape(&got),
            vec![(Provenance::HintDir, "/home/u/.npm-global/bin/x")]
        );
    }

    #[test]
    fn the_override_still_wins_alone_over_hints() {
        let mut p = probed(Some(Ok("/fork/ra")), None, None);
        p.hints = vec![PathBuf::from("/home/u/.npm-global/bin/ra")];
        let got = ladder(p, cide_ipc::settings::ServerBinaryChoice::Builtin).expect("resolved");
        assert_eq!(shape(&got), vec![(Provenance::Override, "/fork/ra")]);
        assert!(got[0].child_path_dirs.is_empty());
    }

    #[test]
    fn a_documents_own_language_labels_it_and_an_unowned_one_falls_back() {
        // The didOpen label picks the dialect parser inside a multi-language server; a
        // language the server never declared must not be sent as a label, and a caller that
        // could not resolve the document gets the first-declared id — the old behaviour.
        assert_eq!(Server::RUST_ANALYZER.language_id_for(Some("rust")), "rust");
        assert_eq!(Server::RUST_ANALYZER.language_id_for(Some("go")), "rust");
        assert_eq!(Server::RUST_ANALYZER.language_id_for(None), "rust");
    }

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
