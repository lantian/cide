//! What cide tells the build of a server it ships.
//!
//! One producer, deliberately. The keys below are a **two-repo contract**: the forked
//! rust-analyzer (built from `../rust-analyzer`, branch `cide`) reads them, and a rename on
//! either side does not error — it silently degrades the fork to its defaults, which look
//! exactly like the feature working. So the whole vocabulary lives in this file, the fork's
//! side lives in its `config.rs`, and a change to either is reviewed against the other.
//!
//! Everything here is gated on [`Provenance`]: cide configures only binaries it shipped (or a
//! developer explicitly pointed it at). A stock server the user installed gets no
//! `initializationOptions` at all — not empty ones — because cide cannot know what version it
//! is or which keys it would misread, and the pre-M25 handshake is the one behaviour every
//! installed server is known to survive.

use serde_json::{Value, json};

use crate::discover::{Provenance, Server};

/// The user-tunable knobs that shape the shipped server's configuration, resolved from
/// cide's settings by `cide-app` and carried to the one place options are built. `Copy` and
/// all-zeros by [`Default`], which means "the fork's own compiled defaults" throughout —
/// tests and throwaway callers say `Tuning::default()` and get yesterday's handshake.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tuning {
    /// `InspectionSettings::server_index_working_set_pct`, already clamped where the
    /// settings patch landed. `0` = say nothing, the fork's `lru = N` literals stand.
    pub index_working_set_pct: u32,
    /// `InspectionSettings::server_memory_limit_mb` — the watchdog threshold, `0` = off.
    /// Carried here because the shipped gopls turns it into a *soft* limit too
    /// (`GOMEMLIMIT`, see [`extra_env`]): the GC should be fighting for memory well before
    /// the watchdog concludes the fight was lost.
    pub memory_limit_mb: u32,
}

/// cide's configuration for this server as it was resolved, or `None` for "say nothing".
///
/// `None` is the common answer and the safe one — see the module docs. `Some` currently means
/// "the shipped rust-analyzer", and the object it carries is what `Session` sends as
/// `initializationOptions` and repeats as every `workspace/configuration` answer.
pub fn init_options(server: Server, provenance: Provenance, tuning: Tuning) -> Option<Value> {
    // **Lane one**, and it is checked first because it is unconditional: options a *definition*
    // declares are sent whatever the provenance. `yaml-language-server` is always a stock
    // installation and has no idea a Compose file is a Compose file until it is handed a schema
    // map — see `cide_ipc::lang::LanguageServerDef::init_options` for the whole argument.
    //
    // One producer each, and the separation is the point: nothing may put a fork option in a
    // definition, and nothing may put a definition's options behind the gate below.
    if let Some(declared) = server.def().init_options {
        return Some(declared);
    }
    if !wants_options(server, provenance) {
        return None;
    }
    let disk_index = cide_core::persist::cache_dir().join("rust-analyzer");
    // Created here, not in the fork: the directory's *location* is cide's decision (it moves
    // with the profile — see `cide_core::persist::cache_dir`), so cide proves the decision is
    // realisable. What goes inside it, and when any of it is evicted, is entirely the fork's.
    if let Err(error) = std::fs::create_dir_all(&disk_index) {
        tracing::warn!(%error, dir = %disk_index.display(), "could not create the disk-index dir");
    }
    Some(rust_analyzer_options(&disk_index, tuning))
}

/// Does this resolution get cide's configuration at all?
///
/// The provenance is the whole test: [`Provenance::Bundled`] and [`Provenance::Override`] can
/// only be produced by a row in `discover`'s bundled table, so "cide shipped it or the
/// developer pointed at their own build of it" is already established. The binary-name match
/// picks *which* configuration: rust-analyzer's arrives as `initializationOptions`; the
/// shipped gopls has no init-options vocabulary yet (its Phase-0 configuration is an
/// environment variable — see [`extra_env`]) and deliberately stays out of this gate.
fn wants_options(server: Server, provenance: Provenance) -> bool {
    matches!(provenance, Provenance::Bundled | Provenance::Override)
        && server.binary() == "rust-analyzer"
}

/// Environment variables for the shipped build of this server — the second lane of the same
/// contract, for configuration a server reads from its environment rather than the handshake.
///
/// Today that is one variable: the shipped gopls gets `GOPLSCACHE`, the per-profile cache
/// directory, so its file cache moves with the profile like rust-analyzer's disk index does.
/// The ownership split is the same as `cide.diskIndex.dir`'s, only stronger: cide names and
/// creates the directory, gopls owns everything inside it *including the eviction budget* —
/// gopls garbage-collects its own cache, so cide never manages the contents at all.
///
/// Provenance-gated like everything here, and the System direction matters more than usual: a
/// stock PATH gopls shares one machine-global cache with the user's other editors, and moving
/// *that* would be cide reaching into an installation it does not own. Empty is the answer
/// for every stock server and for the shipped rust-analyzer (whose configuration all rides
/// the handshake).
///
/// The second variable is `GOMEMLIMIT`, sent only when the user set the memory limit: Go's
/// runtime soft limit, at 75% of the watchdog threshold, so the GC compresses the heap —
/// trading CPU — *before* the watchdog's restart is the only move left. Measured need: a Go
/// heap idles at roughly double its live bytes (GC headroom), so a gopls that could live in
/// 400 MB sits at 850 unless something asks it not to. 75% and not 100% because the two
/// mechanisms must not meet: a soft limit *at* the kill line invites sawtoothing against it.
pub fn extra_env(server: Server, provenance: Provenance, tuning: Tuning) -> Vec<(String, String)> {
    if !matches!(provenance, Provenance::Bundled | Provenance::Override)
        || server.binary() != "gopls"
    {
        return Vec::new();
    }
    let cache = cide_core::persist::cache_dir().join("gopls");
    // Created here for the same reason `init_options` creates the disk-index dir: the
    // location is cide's decision, so cide proves the decision is realisable.
    if let Err(error) = std::fs::create_dir_all(&cache) {
        tracing::warn!(%error, dir = %cache.display(), "could not create the gopls cache dir");
    }
    let mut env = vec![(
        "GOPLSCACHE".to_owned(),
        cache.to_string_lossy().into_owned(),
    )];
    if tuning.memory_limit_mb > 0 {
        let soft = u64::from(tuning.memory_limit_mb) * 3 / 4;
        env.push(("GOMEMLIMIT".to_owned(), format!("{soft}MiB")));
    }
    env
}

/// The rust-analyzer section — the contract's vocabulary, in one place.
///
/// `cide.diskIndex.dir` is where the fork keeps its persisted index. `cachePriming` is a
/// *stock* rust-analyzer key, and its value here has flipped once, deliberately, in each
/// direction. Off in Phase 0, because priming then only warmed caches a restart would drop.
/// **On since the disk index landed**, for two reasons that arrived together: primed memos
/// now flow *into* the snapshot and the memo tier — priming is index construction, not
/// heat — and priming is the one phase whose `$/progress` is honest per-module progress
/// ("Indexing 45/514 (serde + 2 more)", a true fraction), which is what the Problems
/// panel's determinate bar and the rail's busy dot render. Its `End` triggers the GC/LRU
/// sweep, so the priming RSS peak settles into the caps and the evicted values land in the
/// tier rather than being recomputed. One interplay worth knowing: the peak happens
/// *before* that sweep, so a memory-watchdog limit set below the workspace's priming peak
/// would restart the server mid-prime, warm-restore, and prime again — the watchdog is off
/// by default, and a user who sets it very low on a huge workspace has asked for exactly
/// that loop.
///
/// `cide.diskIndex.lru.*` are the four persisted-query cap overrides — sent only when the
/// user moved [`Tuning::index_working_set_pct`] off `0`, because absent keys are how the
/// fork knows to keep its own compiled tuning. (Upstream's *stock* `lru.*` keys remain
/// deliberately unstated: that runtime plumbing is a commented-out no-op in `ide-db`, and
/// stating them would be configuration theatre. These four go through the fork's own
/// out-of-band road instead.)
fn rust_analyzer_options(disk_index: &std::path::Path, tuning: Tuning) -> Value {
    let mut options = json!({
        "cachePriming": { "enable": true },
        // The symbol half of priming stays off: `cide.primeCaches.symbols` is the fork's
        // A *stock* key, `true` meaning "check under `target/rust-analyzer`, not `target`".
        // cide is an IDE whose panes run the user's own `cargo build` in the same workspace,
        // and cargo serialises everything under one target dir behind one lock — so a
        // flycheck sharing `target/` sits silently behind whatever build a pane (or an
        // agent) has going, and the panel shows "cargo check" frozen for exactly that long,
        // indistinguishable from a hang. The price is a second set of check artefacts on
        // disk; the alternative was a progress bar whose stillness is somebody else's build.
        "cargo": { "targetDir": true },
        // One `cide` object — `json!` silently keeps only the LAST duplicate key, so a
        // second `"cide"` block would erase this one without a warning anywhere.
        "cide": {
            "diskIndex": { "dir": disk_index.to_string_lossy() },
            // The symbol half of priming stays off: the fork's gate for the per-module
            // symbol index, which serves `workspace/symbol` — a request cide never sends
            // (symbol navigation is cide-lang's tree-sitter outline). The index is
            // unbounded residency rebuilt every start, for a consumer that does not exist
            // here.
            "primeCaches": { "symbols": false },
        },
    });
    if tuning.index_working_set_pct != 0 {
        let lru: serde_json::Map<String, Value> = LRU_DEFAULTS
            .iter()
            .map(|&(key, default)| {
                // Integer math floors; `.max(1)` so no rounding can ever produce `0`, which
                // is salsa's "cap off" and the opposite of what a small percentage asks for.
                let cap = (default * tuning.index_working_set_pct / 100).max(1);
                (key.to_string(), Value::from(cap))
            })
            .collect();
        options["cide"]["diskIndex"]["lru"] = Value::Object(lru);
    }
    options
}

/// The fork's compiled `lru = N` literals, duplicated because this producer scales them.
///
/// The attribute sites in `../rust-analyzer`'s `hir-def` (`item_tree.rs`, `nameres.rs`) are
/// the source of truth; attribute arguments cannot name constants, so the duplication is
/// unavoidable somewhere, and here it sits inside the one two-repo-contract file where a
/// fork retune is already a paired review. The contract test below names all four keys.
const LRU_DEFAULTS: [(&str, u32); 4] = [
    ("fileItemTree", 512),
    ("blockItemTree", 256),
    ("crateDefMap", 128),
    ("blockDefMap", 256),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stock_server_from_path_is_told_nothing() {
        // The load-bearing direction: the user's own rust-analyzer must get byte-for-byte the
        // handshake it always got. `init_options` (not just the pure gate) so the test breaks
        // if a future edit builds options before checking provenance.
        assert_eq!(
            init_options(
                Server::RUST_ANALYZER,
                Provenance::SystemPath,
                Tuning::default()
            ),
            None
        );
        // Nor does a tuned setting change that: the gate is provenance, never the knobs.
        assert_eq!(
            init_options(
                Server::RUST_ANALYZER,
                Provenance::SystemPath,
                Tuning {
                    index_working_set_pct: 50,
                    ..Tuning::default()
                },
            ),
            None
        );
    }

    #[test]
    fn only_the_shipped_rust_analyzer_gets_the_rust_analyzer_section() {
        assert!(wants_options(Server::RUST_ANALYZER, Provenance::Bundled));
        assert!(wants_options(Server::RUST_ANALYZER, Provenance::Override));
        assert!(!wants_options(
            Server::RUST_ANALYZER,
            Provenance::SystemPath
        ));
        // The shipped gopls is configured through its environment (`extra_env`), not the
        // handshake — it has no init-options vocabulary yet, and it must never receive
        // rust-analyzer's keys.
        assert!(!wants_options(Server::GOPLS, Provenance::Bundled));
        assert!(!wants_options(Server::GOPLS, Provenance::SystemPath));
        // A hint-dir binary is the user's own installation, exactly like SystemPath: stock
        // handshake, no env lane. Pinned so the `matches!` gates cannot quietly widen when
        // somebody adds the variant to the wrong side.
        assert!(!wants_options(Server::RUST_ANALYZER, Provenance::HintDir));
        assert!(
            extra_env(Server::GOPLS, Provenance::HintDir, Tuning::default()).is_empty(),
            "an npm-dir binary must get byte-for-byte the environment a PATH one gets"
        );
    }

    #[test]
    fn the_contract_names_the_gopls_cache_variable() {
        // The env lane's one meeting point: the shipped gopls reads `GOPLSCACHE` (an upstream
        // gopls variable, not a cide invention) and keeps its file cache there. Name and
        // per-profile location pinned; the value's leaf is the server's binary name.
        let env = extra_env(Server::GOPLS, Provenance::Bundled, Tuning::default());
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].0, "GOPLSCACHE");
        assert!(
            std::path::Path::new(&env[0].1).ends_with("gopls"),
            "the cache dir's leaf names the server: {}",
            env[0].1
        );
        assert_eq!(
            extra_env(Server::GOPLS, Provenance::Override, Tuning::default()).len(),
            1
        );
    }

    #[test]
    fn the_memory_limit_becomes_a_soft_go_limit_at_three_quarters() {
        // The watchdog threshold doubles as GOMEMLIMIT for the shipped gopls — at 75%, so
        // the GC's fight and the watchdog's verdict never share a line. Off means absent,
        // not "0MiB", which Go would read as a real (and absurd) limit.
        let tuned = Tuning {
            memory_limit_mb: 1024,
            ..Tuning::default()
        };
        let env = extra_env(Server::GOPLS, Provenance::Bundled, tuned);
        assert!(
            env.contains(&("GOMEMLIMIT".to_owned(), "768MiB".to_owned())),
            "{env:?}"
        );
        let off = extra_env(Server::GOPLS, Provenance::Bundled, Tuning::default());
        assert!(!off.iter().any(|(k, _)| k == "GOMEMLIMIT"), "{off:?}");
        // And never for a stock gopls, whatever the setting says.
        assert!(extra_env(Server::GOPLS, Provenance::SystemPath, tuned).is_empty());
    }

    #[test]
    fn a_stock_server_gets_no_environment_either() {
        // Same load-bearing direction as `a_stock_server_from_path_is_told_nothing`, for the
        // env lane: a PATH gopls keeps its machine-global cache, shared with the user's other
        // editors — not cide's to move. And rust-analyzer's configuration all rides the
        // handshake, so its environment stays untouched at every provenance.
        assert!(extra_env(Server::GOPLS, Provenance::SystemPath, Tuning::default()).is_empty());
        assert!(
            extra_env(
                Server::RUST_ANALYZER,
                Provenance::Bundled,
                Tuning::default()
            )
            .is_empty()
        );
        assert!(
            extra_env(
                Server::RUST_ANALYZER,
                Provenance::SystemPath,
                Tuning::default()
            )
            .is_empty()
        );
    }

    #[test]
    fn the_contract_names_the_disk_index_dir() {
        // The one meeting point with the fork. If this shape moves, the fork's `config.rs`
        // moves in the same review — that is the rule this test exists to make somebody read.
        let options = rust_analyzer_options(
            std::path::Path::new("/cache/cide/rust-analyzer"),
            Tuning::default(),
        );
        assert_eq!(
            options["cide"]["diskIndex"]["dir"],
            "/cache/cide/rust-analyzer"
        );
        // Stock key, so it works on the fork's stock config parsing from day one. See the
        // producer for why priming is off under cide.
        // Explicitly `true` rather than unstated-and-defaulted: an upstream default flip
        // must not silently change what cide's bundled build does.
        assert_eq!(options["cachePriming"]["enable"], true);
        assert_eq!(options["cide"]["primeCaches"]["symbols"], false);
        // The duplicate-key trap above, pinned: both halves of the one `cide` object.
        assert!(options["cide"]["diskIndex"]["dir"].is_string());
        // The default handshake carries no cap overrides at all — absent, not zeros: absent
        // is how the fork knows to keep its compiled tuning.
        assert_eq!(options["cide"]["diskIndex"].get("lru"), None);
    }

    #[test]
    fn the_contract_names_the_four_lru_caps() {
        // The other half of the meeting point — the fork's `config.rs` reads exactly these
        // four names under `cide.diskIndex.lru`, each standing alone. 50% of the compiled
        // defaults (512/256/128/256), floored so no value can round to salsa's "cap off" 0.
        let options = rust_analyzer_options(
            std::path::Path::new("/cache/cide/rust-analyzer"),
            Tuning {
                index_working_set_pct: 50,
                ..Tuning::default()
            },
        );
        let lru = &options["cide"]["diskIndex"]["lru"];
        assert_eq!(lru["fileItemTree"], 256);
        assert_eq!(lru["blockItemTree"], 128);
        assert_eq!(lru["crateDefMap"], 64);
        assert_eq!(lru["blockDefMap"], 128);
        assert_eq!(
            lru.as_object().map(|o| o.len()),
            Some(4),
            "no fifth key unnamed here"
        );
    }
}
