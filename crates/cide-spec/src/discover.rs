//! Finding the `openspec` binary, and saying why when it cannot be found. (M28)
//!
//! # Why this crate does its own searching, when `cide_core::toolchain::which` exists
//!
//! Because `which` cannot find it on the machine it is installed on. `toolchain::extra_dirs` is
//! `~/.cargo/bin` and `~/go/bin` on Linux, and its header states the rule that keeps it that
//! short: **the directories cide searches to find a binary and the directories it gives that
//! binary's process are one list**, so widening `search_paths` would widen `claude_cli::resolve`'s
//! acceptance and turn a refusal that names a remedy into an opaque `ENOENT`.
//!
//! `openspec` is installed by `npm -g`, which on any Node version manager means a directory under
//! `~/.nvm`, `~/.volta`, `~/.local/share/pnpm` or the like — a directory a *shell* rc file adds to
//! `PATH` and a desktop launcher never does. So the extra rungs live here, contained: this crate
//! widens what cide can *find `openspec` at* and touches nothing `claude_cli` reads, and the
//! directory it chooses is handed back to `child_env::prepare_command_with` so the one-list rule
//! still holds for the child. The directory enumeration itself lives in `cide_core::node_dirs`
//! since `cide-lsp` became its second consumer (npm-installed language servers, same launch
//! problem); this crate's ladder, refusals and caching asymmetry are unchanged by the move.
//!
//! # The ladder is pure; the probing is not
//!
//! [`ladder`] is a function of a [`Probes`], exactly as `cide_lsp::discover`'s is: every rung's
//! precedence, and every refusal sentence, is decided by a value a test can construct on a
//! machine that has none of these directories.

use std::path::PathBuf;

// Re-exported so no call site moved: the enumeration left for `cide_core::node_dirs` when
// `cide-lsp` became its second consumer, the same move (and the same reason) as `which`'s.
pub use cide_core::node_dirs::{node_dirs_in, nvm_dirs};

/// The environment variable that overrides everything below it.
pub const OVERRIDE_ENV: &str = "CIDE_OPENSPEC_PATH";

/// What the user is told to run when nothing was found.
pub const INSTALL_COMMAND: &str = "npm install -g @fission-ai/openspec";

/// The binary's name, everywhere it is looked for.
pub const BINARY: &str = "openspec";

/// What was on disk when the question was asked.
///
/// Impure inputs, gathered once at the edge, so that [`ladder`] below is a function. The order of
/// [`Self::node_dirs`] is the search order — the caller sorts, because "newest nvm version first"
/// is a filesystem walk and not a rule.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Probes {
    /// `CIDE_OPENSPEC_PATH`, if set: `Ok` when it points at something executable, `Err` with the
    /// value when it does not. `None` when the variable is unset.
    ///
    /// Three states and not two, because a *broken* override must refuse rather than fall
    /// through — see [`ladder`].
    pub override_env: Option<Result<PathBuf, PathBuf>>,
    /// What `cide_core::toolchain::which` answered.
    pub on_path: Option<PathBuf>,
    /// Node installation directories that hold an executable `openspec`, best first.
    pub node_dirs: Vec<PathBuf>,
    /// A directory that holds an `openspec` cide could *not* execute — a permissions or
    /// architecture problem. Named in the refusal, because "but I have it installed" is the
    /// reply a bare "not found" earns.
    pub present_but_unexecutable: Option<PathBuf>,
}

/// Why no binary could be chosen. Tagged rather than prose, so the sentence is built once, in
/// [`Refusal::sentence`], and every caller shows the same words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// `CIDE_OPENSPEC_PATH` is set and does not point at an executable.
    BrokenOverride(PathBuf),
    /// Nothing anywhere.
    NothingFound {
        /// A directory that had the file and could not run it, if there was one.
        present_but_unexecutable: Option<PathBuf>,
    },
}

impl Refusal {
    /// The sentence a panel prints, and a run's log line.
    ///
    /// Modelled word for word on `cide_agents::defs::installed` and `cide_deps`' `missing`: name
    /// the remedy, and name the *reason a user who has it installed is being told they have not*.
    /// A bare "not found" sends somebody to install a thing that is already there.
    pub fn sentence(&self) -> String {
        match self {
            Self::BrokenOverride(path) => format!(
                "{OVERRIDE_ENV} is set to `{}`, which is not an executable file. It is an \
                 override, so cide has not fallen back to any other `openspec` — unset it or \
                 point it at a real one.",
                path.display()
            ),
            Self::NothingFound {
                present_but_unexecutable,
            } => {
                let mut sentence = format!(
                    "`{BINARY}` is not on this app's PATH, nor in any Node installation \
                     directory cide searched, so cide cannot read this project's specs. Install \
                     it with `{INSTALL_COMMAND}`. A cide started from a desktop launcher has a \
                     different PATH from one started in a terminal."
                );
                if let Some(dir) = present_but_unexecutable {
                    sentence.push_str(&format!(
                        " (There is an `{BINARY}` in `{}` that cide could not execute — check \
                         its permissions.)",
                        dir.display()
                    ));
                }
                sentence
            }
        }
    }
}

/// Pick the binary, or say why not.
///
/// # The override is alone, not first
///
/// `cide_lsp::discover::ladder`'s rule, and it is worth restating because the difference only
/// shows up when something is wrong: a *set* `CIDE_OPENSPEC_PATH` that points at nothing is a
/// refusal, not a rung that failed. Everything below exists to be fallen back to, and the one
/// thing an override must never do is quietly become something else — a developer pointing cide
/// at a build of their own and silently getting the system copy would debug the wrong binary.
pub fn ladder(probes: Probes) -> Result<PathBuf, Refusal> {
    if let Some(verdict) = probes.override_env {
        return verdict.map_err(Refusal::BrokenOverride);
    }
    if let Some(path) = probes.on_path {
        return Ok(path);
    }
    if let Some(dir) = probes.node_dirs.first() {
        return Ok(dir.join(BINARY));
    }
    Err(Refusal::NothingFound {
        present_but_unexecutable: probes.present_but_unexecutable,
    })
}

/// Gather [`Probes`] from this machine, then run the [`ladder`].
///
/// # Only the positive answer is cached
///
/// A found binary cannot move under a running cide in any way that matters, so the caller holds
/// it in a `OnceLock`. A *miss* is re-probed on every call, deliberately and asymmetrically:
/// `npm install -g @fission-ai/openspec` in a terminal pane is exactly what a user does after
/// reading the refusal, and the panel's Retry has to work without a relaunch.
/// `toolchain::extra_dirs` caches both directions; this does not, and the difference is the
/// point.
pub fn find() -> Result<PathBuf, Refusal> {
    let override_env = std::env::var_os(OVERRIDE_ENV)
        .map(PathBuf::from)
        .filter(|value| !value.as_os_str().is_empty())
        .map(|value| {
            if cide_core::toolchain::is_executable(&value) {
                Ok(value)
            } else {
                Err(value)
            }
        });

    // `enumerate` re-reads the environment on every call, which is `find`'s own miss-path
    // discipline (see the doc above) kept by the shared module.
    let candidates = cide_core::node_dirs::enumerate();

    let mut node_dirs = Vec::new();
    let mut present_but_unexecutable = None;
    for dir in candidates {
        let file = dir.join(BINARY);
        if cide_core::toolchain::is_executable(&file) {
            node_dirs.push(dir);
        } else if file.exists() && present_but_unexecutable.is_none() {
            present_but_unexecutable = Some(dir);
        }
    }

    ladder(Probes {
        override_env,
        on_path: cide_core::toolchain::which(BINARY),
        node_dirs,
        present_but_unexecutable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probes() -> Probes {
        Probes::default()
    }

    #[test]
    fn an_override_wins_alone_and_a_broken_one_refuses_rather_than_falling_through() {
        let mut p = probes();
        p.override_env = Some(Ok(PathBuf::from("/home/u/src/openspec/bin/openspec")));
        p.on_path = Some(PathBuf::from("/usr/bin/openspec"));
        assert_eq!(
            ladder(p).expect("the override is taken"),
            PathBuf::from("/home/u/src/openspec/bin/openspec")
        );

        // The half that matters: a developer pointing cide at their own build and silently
        // getting the system copy would spend the afternoon debugging the wrong binary.
        let mut p = probes();
        p.override_env = Some(Err(PathBuf::from("/home/u/typo")));
        p.on_path = Some(PathBuf::from("/usr/bin/openspec"));
        let refusal = ladder(p).expect_err("a broken override refuses");
        assert_eq!(
            refusal,
            Refusal::BrokenOverride(PathBuf::from("/home/u/typo"))
        );
        assert!(refusal.sentence().contains("has not fallen back"));
    }

    #[test]
    fn path_beats_a_node_directory_and_a_node_directory_beats_nothing() {
        let mut p = probes();
        p.on_path = Some(PathBuf::from("/usr/bin/openspec"));
        p.node_dirs = vec![PathBuf::from("/home/u/.nvm/versions/node/v22.21.0/bin")];
        assert_eq!(ladder(p).unwrap(), PathBuf::from("/usr/bin/openspec"));

        let mut p = probes();
        p.node_dirs = vec![PathBuf::from("/home/u/.nvm/versions/node/v22.21.0/bin")];
        assert_eq!(
            ladder(p).unwrap(),
            PathBuf::from("/home/u/.nvm/versions/node/v22.21.0/bin/openspec"),
            "the rung that exists for a desktop launch, which sources no rc file"
        );
    }

    #[test]
    fn nothing_found_names_the_install_command_and_the_launcher_difference() {
        let sentence = ladder(probes()).expect_err("nothing was found").sentence();
        assert!(sentence.contains(INSTALL_COMMAND), "{sentence}");
        assert!(
            sentence.contains("desktop launcher"),
            "a user who installed it in a terminal must be told why this app cannot see it: \
             {sentence}"
        );
    }

    #[test]
    fn a_binary_that_is_there_and_will_not_run_says_so() {
        // "But I have it installed" is the reply a bare not-found earns.
        let mut p = probes();
        p.present_but_unexecutable = Some(PathBuf::from("/home/u/.npm-global/bin"));
        let sentence = ladder(p).expect_err("still nothing runnable").sentence();
        assert!(sentence.contains("could not execute"), "{sentence}");
        assert!(sentence.contains("/home/u/.npm-global/bin"), "{sentence}");
    }

    // `node_dirs_in`'s ordering and `version_key`'s numeric sort are tested where they now
    // live, in `cide_core::node_dirs`.
}
