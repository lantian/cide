//! `$XDG_CONFIG_HOME/cide/extensions.json` — which marketplaces are connected and what is
//! installed from them.
//!
//! # Why config and not state
//!
//! `persist.rs` states the test: *would the user want it in a dotfiles repository?* This is the
//! user's tool set — the same class of fact as `keymap.json`, and wanted on a new machine for the
//! same reason. The **clones** are the opposite: a clone is a cache of a remote, re-derivable from
//! this file alone, so it lives under `$XDG_STATE_HOME` and a `--fresh` launch may delete it
//! without losing anything the user chose.
//!
//! # Why global and not per project
//!
//! `cide-agents` argues the other way for `.cide/config.json`, and the argument is worth stating
//! here because the conclusion is different: *"a global switch would mean enabling subagents once,
//! for a repository the user trusts, and silently arming every other checkout they open
//! afterwards."* That is decisive for a switch that spawns `claude` against whatever code happens
//! to be open.
//!
//! An extension is not that. It is a *tool*, chosen once and wanted everywhere — the SQL syntax an
//! author installed is not more or less appropriate depending on which repository is open — and a
//! per-project list would mean re-installing YAML support in every checkout. What the per-project
//! version would actually buy is a place for the trust decision, and this file already carries it:
//! `granted` records exactly what the user approved at install, and a manifest that later asks for
//! more is refused rather than upgraded. See [`Installed::granted`].
//!
//! # Nothing here fails
//!
//! `load` cannot return an error, on `cide-agents/config.rs`' rule and in its direction: a
//! configuration file cide could not parse must not become a launch that has no extensions *and*
//! no explanation. A missing file is the default. An unreadable or unparseable one is the default
//! **plus a problem**, and — decisively — is never rewritten, because a rewrite would turn a
//! transient read failure into permanent data loss presented as a reset the user did not ask for.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cide_ipc::ext::{Capability, ExtProblem};
use cide_ipc::ids::{ExtensionId, MarketplaceId};
use serde::{Deserialize, Serialize};

use crate::manifest::{SCHEMA_VERSION, error, is_safe_segment, warning};

/// Where the file lives.
#[must_use]
pub fn config_path() -> PathBuf {
    cide_core::persist::config_dir().join("extensions.json")
}

/// Where a marketplace's clone lives.
#[must_use]
pub fn marketplaces_dir() -> PathBuf {
    cide_core::persist::state_dir().join("marketplaces")
}

/// Where installed extension code lives.
#[must_use]
pub fn extensions_dir() -> PathBuf {
    cide_core::persist::state_dir().join("extensions")
}

/// One connected marketplace, as the file records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connected {
    pub id: MarketplaceId,
    /// Verbatim as the user typed it. Not normalised on write: a user who wrote `~/work/x` wants
    /// to see `~/work/x` when they open this file, and an expansion baked in at connect time is
    /// wrong the moment `$HOME` differs — which is the case this file being in a dotfiles
    /// repository is entirely about.
    pub source: String,
}

/// One installed extension, as the file records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    pub marketplace: MarketplaceId,
    pub extension: ExtensionId,
    pub version: String,
    /// The marketplace commit this copy was taken at.
    #[serde(default)]
    pub commit: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// What the user approved at install, as capability strings.
    ///
    /// Recorded rather than re-read from the manifest, and that is the whole consent mechanism.
    /// An update whose manifest asks for a capability that is not in this list is **refused, not
    /// upgraded**: without that, a marketplace could add `process:spawn` in a commit and every
    /// machine that ran a refresh would silently grant it. The user is shown the difference and
    /// approves it again, or does not.
    #[serde(default)]
    pub granted: Vec<String>,
}

fn yes() -> bool {
    true
}

impl Installed {
    /// The capabilities the user actually granted, dropping any this build no longer knows.
    ///
    /// Dropping rather than refusing, because the direction is safe: a capability cide cannot name
    /// is one it cannot enforce, and treating it as *not granted* is the conservative reading of a
    /// file written by a newer build.
    #[must_use]
    pub fn granted_caps(&self) -> Vec<Capability> {
        let mut out: Vec<Capability> = self
            .granted
            .iter()
            .filter_map(|name| Capability::parse(name))
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// The whole file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtConfig {
    pub schema: u32,
    #[serde(default)]
    pub marketplaces: Vec<Connected>,
    #[serde(default)]
    pub installed: Vec<Installed>,
}

impl Default for ExtConfig {
    fn default() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            marketplaces: Vec::new(),
            installed: Vec::new(),
        }
    }
}

impl ExtConfig {
    #[must_use]
    pub fn marketplace(&self, id: &MarketplaceId) -> Option<&Connected> {
        self.marketplaces.iter().find(|m| &m.id == id)
    }

    #[must_use]
    pub fn installed(&self, market: &MarketplaceId, ext: &ExtensionId) -> Option<&Installed> {
        self.installed
            .iter()
            .find(|i| &i.marketplace == market && &i.extension == ext)
    }

    /// A marketplace id derived from a source, unique against what is already connected.
    ///
    /// The repository's own name, which is what a person would call it, with a numeric suffix only
    /// when that is taken. Asking the user to name every marketplace would be a decision with an
    /// obvious answer in every case but the second `cide-marketplace`.
    #[must_use]
    pub fn mint_id(&self, source: &str) -> MarketplaceId {
        let base = source
            .trim_end_matches('/')
            .trim_end_matches(".git")
            .rsplit(['/', ':'])
            .find(|s| !s.is_empty())
            .unwrap_or("marketplace")
            .to_ascii_lowercase();
        let base: String = base
            .chars()
            .map(|c| {
                if c.is_ascii_lowercase() || c.is_ascii_digit() {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let base = base.trim_matches('-').to_string();
        let base = if is_safe_segment(&base) {
            base
        } else {
            "marketplace".to_string()
        };
        let taken: BTreeSet<&str> = self.marketplaces.iter().map(|m| m.id.as_str()).collect();
        if !taken.contains(base.as_str()) {
            return MarketplaceId(base);
        }
        for n in 2u32..1000 {
            let candidate = format!("{base}-{n}");
            if !taken.contains(candidate.as_str()) {
                return MarketplaceId(candidate);
            }
        }
        MarketplaceId(format!("{base}-{}", cide_core::persist::now_ms()))
    }
}

/// What reading the file produced.
///
/// The problems ride alongside rather than replacing the config, because the two are independent:
/// a file with one bad marketplace row still has three good ones, and refusing all four for the
/// sake of one would be the loudest possible way to be unhelpful.
#[derive(Debug, Clone, Default)]
pub struct LoadOutcome {
    pub config: ExtConfig,
    pub problems: Vec<ExtProblem>,
    /// True when the file exists but could not be understood.
    ///
    /// The flag [`crate::ExtStore`] refuses writes on, so a save can never overwrite a file cide
    /// failed to read — `cide-tasks`' `DiskState::Unreadable` rule, and enforced in the domain
    /// rather than in the panel because a write can arrive from a command the panel never saw.
    pub unreadable: bool,
}

/// Read the file. Never fails; see the module header.
#[must_use]
pub fn load() -> LoadOutcome {
    load_from(&config_path())
}

pub(crate) fn load_from(path: &Path) -> LoadOutcome {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            // Not even a debug line: no extensions configured is the ordinary state of a fresh
            // install, not an event.
            return LoadOutcome::default();
        }
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "extensions.json could not be read");
            return LoadOutcome {
                problems: vec![error(path, None, format!("could not be read: {err}"))],
                unreadable: true,
                ..LoadOutcome::default()
            };
        }
    };

    let raw: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "extensions.json could not be parsed");
            let line = u32::try_from(err.line()).ok().filter(|n| *n > 0);
            return LoadOutcome {
                problems: vec![error(
                    path,
                    line,
                    format!(
                        "could not be parsed: {err}. Nothing has been changed, and cide will not \
                         write over it until it reads."
                    ),
                )],
                unreadable: true,
                ..LoadOutcome::default()
            };
        }
    };

    if let Some(schema) = raw.get("schema").and_then(serde_json::Value::as_u64)
        && schema > u64::from(SCHEMA_VERSION)
    {
        return LoadOutcome {
            problems: vec![error(
                path,
                None,
                format!(
                    "is schema {schema}; this cide understands up to {SCHEMA_VERSION}. It has \
                     been left alone."
                ),
            )],
            unreadable: true,
            ..LoadOutcome::default()
        };
    }

    let mut config: ExtConfig = match serde_json::from_value(raw) {
        Ok(config) => config,
        Err(err) => {
            return LoadOutcome {
                problems: vec![error(path, None, format!("could not be understood: {err}"))],
                unreadable: true,
                ..LoadOutcome::default()
            };
        }
    };

    // Repair rather than refuse, and name every repair. A row with an unusable id cannot be
    // acted on at all — it names a directory that cannot exist — so it is dropped; everything
    // else is kept.
    let mut problems = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    config.marketplaces.retain(|m| {
        if !is_safe_segment(m.id.as_str()) {
            problems.push(error(
                path,
                None,
                format!("marketplace `{}` has an unusable id and is ignored.", m.id),
            ));
            return false;
        }
        if m.source.trim().is_empty() {
            problems.push(error(
                path,
                None,
                format!("marketplace `{}` has no source and is ignored.", m.id),
            ));
            return false;
        }
        if !seen.insert(m.id.to_string()) {
            problems.push(warning(
                path,
                None,
                format!(
                    "marketplace `{}` is listed twice; the second is ignored.",
                    m.id
                ),
            ));
            return false;
        }
        true
    });

    let known: BTreeSet<String> = config
        .marketplaces
        .iter()
        .map(|m| m.id.to_string())
        .collect();
    let mut seen_installed: BTreeSet<(String, String)> = BTreeSet::new();
    config.installed.retain(|i| {
        if !known.contains(i.marketplace.as_str()) {
            problems.push(warning(
                path,
                None,
                format!(
                    "`{}` is installed from `{}`, which is not connected. Reconnect that \
                     marketplace or remove the row.",
                    i.extension, i.marketplace
                ),
            ));
            return false;
        }
        if !is_safe_segment(i.extension.as_str()) {
            problems.push(error(
                path,
                None,
                format!(
                    "`{}` has an unusable extension id and is ignored.",
                    i.extension
                ),
            ));
            return false;
        }
        if !seen_installed.insert((i.marketplace.to_string(), i.extension.to_string())) {
            problems.push(warning(
                path,
                None,
                format!("`{}` is listed twice; the second is ignored.", i.extension),
            ));
            return false;
        }
        true
    });

    LoadOutcome {
        config,
        problems,
        unreadable: false,
    }
}

/// Write the file.
///
/// Through `persist::write_atomic`, so it gets 0600 and the whole sibling-temp/fsync/rename dance
/// — the same treatment `keymap.json` gets, for the same reason: both are files the user may hand
/// edit and neither may ever be observed half-written.
pub fn save(config: &ExtConfig) -> Result<(), cide_core::CoreError> {
    save_to(&config_path(), config)
}

pub(crate) fn save_to(path: &Path, config: &ExtConfig) -> Result<(), cide_core::CoreError> {
    let mut json = serde_json::to_vec_pretty(config)?;
    // A trailing newline: without one the last line shows as `\ No newline at end of file` in
    // every diff the user's dotfiles repository ever produces.
    json.push(b'\n');
    cide_core::persist::write_atomic(path, &json)
}
