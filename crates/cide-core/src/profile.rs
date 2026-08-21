//! Named instances: one machine running two cides that share nothing on disk.
//!
//! cide is developed inside cide. The daily driver is an installed build; `./run.sh` launches
//! the working tree to try a change. Before this module they were the same application as far
//! as the filesystem was concerned — one `$XDG_STATE_HOME/cide/workspace.json`, one
//! `$XDG_CONFIG_HOME/cide`, one Tauri per-app directory. Every write is atomic, so nothing
//! ever corrupted; there is simply no merge. The last instance to flush its layout won, and
//! closing the one under test stamped its tree over the one the user was working in.
//!
//! A *profile* is the answer: a name that changes where the app looks, and nothing else.
//! [`dir_leaf`] moves everything hanging off `persist::state_dir`/`config_dir`, `cide-app`
//! suffixes the Tauri bundle identifier with [`identifier`] to move the four directories
//! Tauri owns rather than we do, and [`title_prefix`] puts a marker in the OS window title so
//! two otherwise identical windows can be told apart in a task switcher.
//!
//! # Why an environment variable, and why it cannot be anything else
//!
//! A `cfg!(debug_assertions)` split would be free but wrong: `./run.sh --release` builds an
//! optimised binary from the same tree and wants the same isolation. A cargo feature would
//! fork the build, so the artefact under test would stop being the artefact that ships.
//!
//! The tempting third option is to read it back off the Tauri config, since `cide-app` is
//! already writing the profile into the bundle identifier — and that one is a trap worth
//! naming, because it would break the thing cide cannot start without. `main` calls
//! `cmd::settings::apply_graphics_overrides` *before* the Tauri application exists, and that
//! reads `workspace.json` straight off disk to find the user's pinned graphics rungs. So
//! `persist::state_dir()` has to answer correctly with no `AppHandle` in scope (ADR 0006).
//! An environment variable is also the only mechanism that reaches a `cide-headless`
//! inspecting the same state, a `cide --wait` client, and a hook.
//!
//! # The shape of this module
//!
//! Every public function is a thin wrapper over a pure one — [`parse`], [`leaf_for`],
//! [`prefix_for`], [`identifier_for`]. The wrappers latch, because the process resolves a
//! path on many threads and must not get two answers, and because a warning about a bad name
//! should be printed once rather than on every call to `state_dir()`. The pure cores are
//! where the behaviour actually lives, so both branches stay testable in one test binary
//! despite the latch.

use std::sync::OnceLock;

/// Selects the profile. Unset, empty, or `default` all mean the production instance.
pub const PROFILE_VAR: &str = "CIDE_PROFILE";

/// The name reserved for "no profile", so a launcher can turn one off by passing a value
/// rather than by unsetting a variable it has already put in an array.
pub const DEFAULT_PROFILE: &str = "default";

/// The longest name accepted, chosen only to keep a bundle identifier and a directory name
/// within anything either could reasonably object to.
const MAX_LEN: usize = 32;

/// The active profile, or `None` for the production instance.
///
/// **Latched on first call.** `persist::state_dir` is reached from the IPC thread, the PTY
/// reaper, the file watcher and every command worker; a profile that could change under them
/// would mean two threads resolving one path two ways, which is the split-brain this module
/// exists to end. The XDG bases stay dynamic — `cmd::app`'s test helper swaps
/// `XDG_CONFIG_HOME` and puts it back — the profile is the one part of the answer fixed for
/// the life of the process.
///
/// Children inherit it, and should: `child_env::prepare_command` scrubs and overrides a named
/// list over the inherited environment and never clears it, so a `claude`, a language server,
/// a hook and a dispatched agent run all stay in the profile that launched them. That is what
/// makes `cide-headless tree` in a profiled pane inspect the profile's own workspace.
pub fn active() -> Option<&'static str> {
    static LATCHED: OnceLock<Option<String>> = OnceLock::new();
    LATCHED
        .get_or_init(|| {
            let raw = std::env::var(PROFILE_VAR).ok();
            let parsed = parse(raw.as_deref());
            if parsed.is_none()
                && let Some(name) = raw.as_deref()
                && !is_absent(name)
            {
                // A warning rather than a refusal, and once rather than per call. A typo in an
                // environment variable must not be a way to make the IDE fail to start, and
                // falling back to production is the safe direction: the worst case is a shared
                // workspace rather than a directory written somewhere nobody asked for.
                tracing::warn!(
                    profile = %name,
                    "{PROFILE_VAR} is not a valid profile name (lowercase letters, digits and \
                     hyphens, starting with a letter or digit, at most {MAX_LEN} characters); \
                     running as the default instance"
                );
            }
            parsed.map(str::to_string)
        })
        .as_deref()
}

/// The directory name to use under each XDG base: `cide`, or `cide-<profile>`.
pub fn dir_leaf() -> &'static str {
    static LEAF: OnceLock<String> = OnceLock::new();
    LEAF.get_or_init(|| leaf_for(active()))
}

/// What every OS window title is prefixed with: `""`, or `"[DEV] "`.
pub fn title_prefix() -> &'static str {
    static PREFIX: OnceLock<String> = OnceLock::new();
    PREFIX.get_or_init(|| prefix_for(active()))
}

/// The bundle identifier this instance should run under, given the one compiled in.
pub fn identifier(base: &str) -> String {
    identifier_for(base, active())
}

// --- the pure cores -------------------------------------------------------------------------

/// Whether a raw value means "no profile" without being an error.
///
/// Empty is how a launcher unsets a variable it has already placed in an argument array, so it
/// is silence rather than a complaint; `default` is the same thing said out loud, which is what
/// `run.sh --profile default` passes.
fn is_absent(raw: &str) -> bool {
    let name = raw.trim();
    name.is_empty() || name == DEFAULT_PROFILE
}

/// Turn the raw environment value into a profile name, or `None`.
fn parse(raw: Option<&str>) -> Option<&str> {
    let name = raw?.trim();
    if is_absent(name) || !is_valid(name) {
        return None;
    }
    Some(name)
}

/// Whether a name may be joined onto a state root and appended to a bundle identifier.
///
/// Both halves of that sentence are a hard constraint, and this function is load-bearing
/// security rather than hygiene. The name becomes a **path component** twice over: once as the
/// leaf under `$XDG_STATE_HOME`, and once inside the bundle identifier, which Tauri joins as a
/// single segment when it resolves the webview data directory. Without this check
/// `CIDE_PROFILE=../../..` would relocate an instance's entire state, and its browser storage,
/// somewhere the user never named. It is also the last label of a reverse-DNS identifier, and
/// lowercase alphanumerics and hyphens are what every consumer of one accepts.
fn is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_LEN
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The XDG leaf for a profile.
///
/// Sibling of the production directory rather than a child of it, so that removing a profile is
/// `rm -r` on one directory and cannot take the real one with it, and so that a stray
/// `ls ~/.local/state` shows what exists.
fn leaf_for(profile: Option<&str>) -> String {
    match profile {
        Some(profile) => format!("cide-{profile}"),
        None => "cide".to_string(),
    }
}

/// The OS window title prefix for a profile.
///
/// Uppercased and bracketed rather than special-cased for `dev`, so a second profile is not a
/// second code path. Deliberately a *prefix*: a task switcher truncates a long title from the
/// right, and a marker has to survive that to be worth having — the same argument
/// `windows::title_with` already makes for `Awaiting: N`.
fn prefix_for(profile: Option<&str>) -> String {
    match profile {
        Some(profile) => format!("[{}] ", profile.to_uppercase()),
        None => String::new(),
    }
}

/// Append a profile to a bundle identifier as a further label.
///
/// `dev.cide.ide` becomes `dev.cide.ide.dev`. Tauri resolves the webview data directory, the
/// log directory and the plugin store directory from this string at runtime, and those are the
/// locations `cide-app` does not compute for itself and so cannot separate any other way.
fn identifier_for(base: &str, profile: Option<&str>) -> String {
    match profile {
        Some(profile) => format!("{base}.{profile}"),
        None => base.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `active()` latches a process-global and the test binary shares one environment, so the
    // behaviour is tested through the pure cores it delegates to. That split is the reason
    // they exist.

    #[test]
    fn a_plain_name_is_accepted() {
        assert_eq!(parse(Some("dev")), Some("dev"));
        assert_eq!(parse(Some("dev-2")), Some("dev-2"));
        assert_eq!(parse(Some("d")), Some("d"));
        assert_eq!(parse(Some("0")), Some("0"));
        assert_eq!(parse(Some("  dev  ")), Some("dev"), "trimmed");
    }

    #[test]
    fn absence_is_silent_rather_than_an_error() {
        assert_eq!(parse(None), None, "unset");
        assert_eq!(parse(Some("")), None, "set but empty");
        assert_eq!(parse(Some("   ")), None, "whitespace only");
        assert_eq!(parse(Some("default")), None, "the reserved name");
        for raw in ["", "   ", "default"] {
            assert!(is_absent(raw), "{raw:?} must not warn");
        }
    }

    #[test]
    fn a_name_that_could_escape_the_state_root_is_refused() {
        assert_eq!(parse(Some("..")), None, "traversal");
        assert_eq!(parse(Some("../tmp")), None, "traversal with a separator");
        assert_eq!(parse(Some("a/b")), None, "a separator anywhere");
        assert_eq!(
            parse(Some("-lead")),
            None,
            "a leading hyphen is not a label"
        );
        // These are wrong rather than absent, so they are the ones that warn.
        for raw in ["..", "a/b", "-lead"] {
            assert!(!is_absent(raw), "{raw:?} must warn rather than be ignored");
        }
    }

    #[test]
    fn a_name_that_is_not_a_bundle_label_is_refused() {
        assert_eq!(parse(Some("Dev")), None, "uppercase");
        assert_eq!(parse(Some("dev_2")), None, "underscore");
        assert_eq!(
            parse(Some("dev.2")),
            None,
            "a dot would add a label of its own"
        );
        assert_eq!(parse(Some("dev 2")), None, "a space");
        let long = "a".repeat(MAX_LEN + 1);
        assert_eq!(parse(Some(&long)), None, "too long");
        let limit = "a".repeat(MAX_LEN);
        assert_eq!(
            parse(Some(&limit)).map(str::len),
            Some(MAX_LEN),
            "the limit itself is fine"
        );
    }

    #[test]
    fn the_leaf_is_a_sibling_of_the_production_directory() {
        assert_eq!(leaf_for(None), "cide");
        assert_eq!(leaf_for(Some("dev")), "cide-dev");
        // One path component, which is what makes it safe to `join`.
        assert_eq!(leaf_for(Some("dev")).split('/').count(), 1);
    }

    #[test]
    fn the_title_prefix_leads_and_is_empty_by_default() {
        assert_eq!(
            prefix_for(None),
            "",
            "an unprofiled title must be byte-identical"
        );
        assert_eq!(prefix_for(Some("dev")), "[DEV] ");
        assert_eq!(prefix_for(Some("try-2")), "[TRY-2] ");
        assert!(
            prefix_for(Some("dev")).ends_with(' '),
            "it is glued to a title"
        );
    }

    #[test]
    fn the_identifier_gains_exactly_one_label() {
        assert_eq!(identifier_for("dev.cide.ide", None), "dev.cide.ide");
        assert_eq!(
            identifier_for("dev.cide.ide", Some("dev")),
            "dev.cide.ide.dev"
        );
        // Tauri joins the whole identifier as ONE path segment under `~/.local/share`, so a
        // separator reaching this point would be a directory traversal. `is_valid` is what
        // stops it; this asserts the two agree.
        let id = identifier_for("dev.cide.ide", Some("dev"));
        assert!(!id.contains('/'), "must stay a single path segment");
    }
}
