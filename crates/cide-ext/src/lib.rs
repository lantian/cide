//! Extensions, and the git repositories they are distributed in.
//!
//! # What this crate is responsible for
//!
//! Reading `extensions.json`, cloning and refreshing marketplaces, installing and removing
//! extension code, validating manifests, and merging what survives into the one contribution set
//! the rest of the app uses. It produces a [`cide_ipc::ext::ExtensionSnapshot`] and nothing else.
//!
//! # What it deliberately is not responsible for
//!
//! **Running extension code.** No worker starts here, no JavaScript is evaluated here, and nothing
//! in this crate can be reached by an extension. The host that runs a worker lives in the webview,
//! on the far side of a structured clone, and the API it exposes is a fixed table of requests —
//! see `ui/src/ext/`. That separation is what makes `capabilities` enforceable at all: a
//! capability check in a process the extension cannot enter is a check the extension cannot
//! subvert.
//!
//! It also owns no thread and no `AppHandle`. Ticking, broadcasting and running the blocking parts
//! off the UI thread are `cide-app`'s job, on the pattern `cide-tasks` established — see
//! `cide-app/src/ext_state.rs`.
//!
//! # The store
//!
//! [`ExtStore`] mirrors `cide_tasks::TaskStore`, which mirrors `cide_app::WorkspaceState`, line for
//! line and on purpose: snapshot, run the closure, validate, roll back on `Err`, bump `rev`,
//! persist. A reviewer who knows one knows all three.
//!
//! What it adds is one refusal `TaskStore` also has and which matters more here: **while the config
//! file is unreadable, every write is refused.** A save that overwrote a file cide had failed to
//! parse would turn a transient read failure into permanent data loss, presented to the user as a
//! reset they did not ask for — and the thing lost would be the record of what they had approved.

pub mod assets;
pub mod config;
pub mod contribute;
pub mod install;
pub mod manifest;
pub mod market;

use std::collections::BTreeSet;
use std::path::PathBuf;

use cide_core::proxy::ProxyEnv;
use cide_ipc::ext::{
    Capability, ContributionSource, ExtProblem, ExtensionRef, ExtensionSnapshot,
    InstalledExtension, Marketplace, MarketplaceEntry, MarketplaceState, ResolvedContributions,
};
use cide_ipc::ids::{ExtensionId, MarketplaceId};
use parking_lot::Mutex;

use crate::config::{Connected, ExtConfig, Installed};
use crate::contribute::Contributor;
use crate::manifest::{Manifest, error, warning};

#[derive(Debug, thiserror::Error)]
pub enum ExtError {
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    Io(String),
    #[error("no marketplace called `{0}` is connected")]
    NoMarketplace(MarketplaceId),
    #[error("`{0}` is not in this marketplace")]
    NoExtension(ExtensionId),
}

pub type Result<T> = std::result::Result<T, ExtError>;

/// Everything read off disk for one marketplace, before the merge.
#[derive(Debug, Clone, Default)]
struct Scan {
    marketplaces: Vec<Marketplace>,
    extensions: Vec<InstalledExtension>,
    problems: Vec<ExtProblem>,
}

/// The owning store.
///
/// One `Mutex` and not three, unlike `TaskStore`: everything here is behind a single read of a
/// single small file, and the reason that crate has a lock ladder — a `.cide/tasks.json` with
/// several writers, one of which is a `git pull` — has an answer here that is simpler. The clone
/// directories can move under us, but they are re-read wholesale on every refresh rather than
/// merged, because unlike a task file there is nothing in them the user authored.
pub struct ExtStore {
    inner: Mutex<Inner>,
}

struct Inner {
    config: ExtConfig,
    /// True when `extensions.json` exists and could not be understood. Every write is refused
    /// while it is set — see the crate header.
    unreadable: bool,
    rev: u64,
    snapshot: ExtensionSnapshot,
}

impl Default for ExtStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtStore {
    /// Read everything and build the first snapshot.
    #[must_use]
    pub fn new() -> Self {
        let outcome = config::load();
        let store = Self {
            inner: Mutex::new(Inner {
                config: outcome.config,
                unreadable: outcome.unreadable,
                rev: 0,
                snapshot: ExtensionSnapshot::default(),
            }),
        };
        {
            let mut inner = store.inner.lock();
            let problems = outcome.problems;
            rebuild(&mut inner, problems);
        }
        store
    }

    /// The current snapshot.
    #[must_use]
    pub fn snapshot(&self) -> ExtensionSnapshot {
        self.inner.lock().snapshot.clone()
    }

    /// Just the resolved set, for `Bootstrap`.
    #[must_use]
    pub fn resolved(&self) -> ResolvedContributions {
        self.inner.lock().snapshot.resolved.clone()
    }

    /// Where an installed extension's code is, if it is installed and enabled.
    ///
    /// The only way `cide-ext://` learns a root, and it answers `None` for a disabled extension on
    /// purpose: a disabled extension must not be able to serve a file, or "disabled" would mean
    /// "its panels are hidden" rather than "it is not running".
    #[must_use]
    pub fn asset_root(&self, id: &ExtensionRef) -> Option<PathBuf> {
        let inner = self.inner.lock();
        inner
            .snapshot
            .extensions
            .iter()
            .find(|e| e.id == *id && e.enabled)
            .map(|e| e.path.clone())
    }

    /// Re-read everything from disk.
    ///
    /// Cheap and called freely: `cide-agents` makes the same choice and states the reason —
    /// `.cide/*` is committed and a teammate's commit can change it under the running app, so a
    /// cached roster is a roster that is wrong for as long as nobody happens to invalidate it. The
    /// same is true here for a clone directory the user may `git pull` by hand.
    pub fn reload(&self) -> ExtensionSnapshot {
        let outcome = config::load();
        let mut inner = self.inner.lock();
        inner.config = outcome.config;
        inner.unreadable = outcome.unreadable;
        rebuild(&mut inner, outcome.problems);
        inner.snapshot.clone()
    }

    /// Run a mutation over the config, persist it, and rebuild.
    ///
    /// The whole write path goes through here so the roll-back is in one place: a closure that
    /// returns `Err`, or a save that fails, leaves the store exactly as it was rather than with a
    /// config in memory that disagrees with the one on disk.
    fn update<T>(
        &self,
        work: impl FnOnce(&mut ExtConfig) -> Result<T>,
    ) -> Result<(T, ExtensionSnapshot)> {
        let mut inner = self.inner.lock();
        if inner.unreadable {
            return Err(ExtError::Refused(format!(
                "{} could not be read, so cide will not write over it. Fix or remove the file \
                 and try again.",
                config::config_path().display()
            )));
        }
        let previous = inner.config.clone();
        let answer = match work(&mut inner.config) {
            Ok(answer) => answer,
            Err(error) => {
                inner.config = previous;
                return Err(error);
            }
        };
        if let Err(error) = config::save(&inner.config) {
            inner.config = previous;
            return Err(ExtError::Io(error.to_string()));
        }
        rebuild(&mut inner, Vec::new());
        Ok((answer, inner.snapshot.clone()))
    }

    /// Connect a marketplace: record it, clone it, read its index.
    ///
    /// The clone happens **after** the config write, so a clone that fails leaves a connected
    /// marketplace in a `Failed` state with a Retry beside it, rather than nothing at all and a
    /// toast the user has already dismissed.
    pub fn connect(&self, proxy: &ProxyEnv, source: &str) -> Result<ExtensionSnapshot> {
        let source = source.trim().to_string();
        if source.is_empty() {
            return Err(ExtError::Refused(
                "a marketplace needs a URL or a path.".into(),
            ));
        }
        let (id, _) = self.update(|config| {
            if let Some(existing) = config.marketplaces.iter().find(|m| m.source == source) {
                return Err(ExtError::Refused(format!(
                    "`{source}` is already connected as `{}`.",
                    existing.id
                )));
            }
            let id = config.mint_id(&source);
            config.marketplaces.push(Connected {
                id: id.clone(),
                source: source.clone(),
            });
            Ok(id)
        })?;
        Ok(self.refresh(proxy, &id))
    }

    /// Forget a marketplace, and everything installed from it.
    ///
    /// Uninstalling with it, rather than leaving orphans: an installed extension whose marketplace
    /// is gone cannot be updated, cannot be reinstalled, and — because `extensions.json`'s loader
    /// drops rows naming an unknown marketplace — would come back as a warning on every launch.
    pub fn disconnect(&self, id: &MarketplaceId) -> Result<ExtensionSnapshot> {
        let (removed, snapshot) = self.update(|config| {
            if config.marketplace(id).is_none() {
                return Err(ExtError::NoMarketplace(id.clone()));
            }
            let removed: Vec<ExtensionId> = config
                .installed
                .iter()
                .filter(|i| &i.marketplace == id)
                .map(|i| i.extension.clone())
                .collect();
            config.marketplaces.retain(|m| &m.id != id);
            config.installed.retain(|i| &i.marketplace != id);
            Ok(removed)
        })?;
        for ext in removed {
            let _ = install::uninstall(&install_path(id, &ext));
        }
        let _ = std::fs::remove_dir_all(clone_path(id));
        Ok(snapshot)
    }

    /// Clone or fetch a marketplace and re-read its index.
    ///
    /// Never returns an error: a marketplace that will not clone is a row in a `Failed` state, and
    /// the panel draws that. Making it an `Err` would mean a refresh of four marketplaces stopping
    /// at the first one behind a VPN.
    pub fn refresh(&self, proxy: &ProxyEnv, id: &MarketplaceId) -> ExtensionSnapshot {
        let source = {
            let inner = self.inner.lock();
            inner.config.marketplace(id).map(|m| m.source.clone())
        };
        let Some(source) = source else {
            return self.snapshot();
        };
        let dest = clone_path(id);
        let outcome = if market::is_clone(&dest) {
            market::refresh(proxy, &dest)
        } else {
            market::clone(proxy, &source, &dest)
        };
        if let Ok(output) = &outcome
            && !output.ok
        {
            tracing::warn!(marketplace = %id, "marketplace refresh failed: {}", output.error());
        }
        self.reload()
    }

    /// Install, or update, one extension.
    pub fn install(&self, id: &ExtensionRef, granted: &[Capability]) -> Result<ExtensionSnapshot> {
        let dir = self.entry_dir(id)?;
        let manifest =
            manifest::read_manifest(&dir).map_err(|problem| ExtError::Refused(problem.message))?;

        // The consent check, and the whole reason `granted` is echoed back rather than read from
        // the manifest cide is about to install: without it a marketplace could add
        // `process:spawn` between the sheet being drawn and the button being pressed, and the user
        // would have approved a different extension from the one that installed.
        let asked: BTreeSet<Capability> = manifest.capabilities.iter().copied().collect();
        let given: BTreeSet<Capability> = granted.iter().copied().collect();
        if asked != given {
            let missing: Vec<&str> = asked.difference(&given).map(|c| c.as_str()).collect();
            let extra: Vec<&str> = given.difference(&asked).map(|c| c.as_str()).collect();
            return Err(ExtError::Refused(format!(
                "what this extension asks for has changed since you were shown it{}{}. Look \
                 again before installing.",
                if missing.is_empty() {
                    String::new()
                } else {
                    format!(" — it now also wants: {}", missing.join(", "))
                },
                if extra.is_empty() {
                    String::new()
                } else {
                    format!(" — it no longer wants: {}", extra.join(", "))
                },
            )));
        }

        let dest = install_path(&id.marketplace, &id.extension);
        install::install(&dir, &dest).map_err(|error| ExtError::Refused(error.to_string()))?;

        let proxy = ProxyEnv::default();
        let commit = market::head(&proxy, &clone_path(&id.marketplace)).unwrap_or_default();
        let version = manifest.version.clone();
        let granted: Vec<String> = granted.iter().map(|c| c.as_str().to_string()).collect();
        let (_, snapshot) = self.update(|config| {
            if let Some(row) = config
                .installed
                .iter_mut()
                .find(|i| i.marketplace == id.marketplace && i.extension == id.extension)
            {
                row.version = version;
                row.commit = commit;
                row.granted = granted;
                // An update does not silently re-enable something the user switched off.
            } else {
                config.installed.push(Installed {
                    marketplace: id.marketplace.clone(),
                    extension: id.extension.clone(),
                    version,
                    commit,
                    enabled: true,
                    // A newly installed extension shows its button. Anything else would make a
                    // panel the user just chose to install invisible with no indication why.
                    rail_icon: true,
                    // Empty: every setting reads as its default until the user changes one. An
                    // install that wrote today's defaults would pin them for ever — see
                    // `config::Installed::settings`.
                    settings: Default::default(),
                    granted,
                });
            }
            Ok(())
        })?;
        Ok(snapshot)
    }

    /// Remove an installed extension. Its marketplace stays connected.
    pub fn uninstall(&self, id: &ExtensionRef) -> Result<ExtensionSnapshot> {
        let (_, snapshot) = self.update(|config| {
            let before = config.installed.len();
            config
                .installed
                .retain(|i| !(i.marketplace == id.marketplace && i.extension == id.extension));
            if config.installed.len() == before {
                return Err(ExtError::NoExtension(id.extension.clone()));
            }
            Ok(())
        })?;
        let _ = install::uninstall(&install_path(&id.marketplace, &id.extension));
        Ok(snapshot)
    }

    /// Change one of an extension's settings.
    ///
    /// The value is **coerced against the declared kind before it is stored** — clamped to a
    /// number's band, refused if it names a choice that does not exist, defaulted if it is the
    /// wrong type outright. That is the same call `scan` makes when reading, so the spin control,
    /// a hand-edited `extensions.json` and an update that narrowed a range all get one answer.
    /// A clamp that lived only in the Settings page would be one `invoke` away from being
    /// bypassed, which is the argument `cide_ipc::settings`' own clamps already make.
    ///
    /// A value equal to the default **removes** the key rather than storing it. Storing it would
    /// pin the user to today's default for ever, having never chosen it — see
    /// [`config::Installed::settings`].
    pub fn set_setting(
        &self,
        id: &ExtensionRef,
        key: &str,
        value: serde_json::Value,
    ) -> Result<ExtensionSnapshot> {
        // The definition, read before the lock the write takes: `scan`'s snapshot is what knows
        // what an extension declares, and a caller may name a setting that no longer exists.
        let def = {
            let inner = self.inner.lock();
            inner
                .snapshot
                .extensions
                .iter()
                .find(|e| e.id == *id)
                .and_then(|e| {
                    e.contributes
                        .settings
                        .iter()
                        .find(|def| def.id == key)
                        .cloned()
                })
        };
        let Some(def) = def else {
            return Err(ExtError::Refused(format!(
                "`{id}` has no setting called `{key}`."
            )));
        };
        let coerced = def.coerce(Some(&value));
        let is_default = coerced == def.coerce(None);

        let (_, snapshot) = self.update(|config| {
            let row = config
                .installed
                .iter_mut()
                .find(|i| i.marketplace == id.marketplace && i.extension == id.extension)
                .ok_or_else(|| ExtError::NoExtension(id.extension.clone()))?;
            if is_default {
                row.settings.remove(key);
            } else {
                row.settings.insert(key.to_string(), coerced);
            }
            Ok(())
        })?;
        Ok(snapshot)
    }

    /// Switch an installed extension on or off.
    pub fn set_enabled(&self, id: &ExtensionRef, enabled: bool) -> Result<ExtensionSnapshot> {
        let (_, snapshot) = self.update(|config| {
            let row = config
                .installed
                .iter_mut()
                .find(|i| i.marketplace == id.marketplace && i.extension == id.extension)
                .ok_or_else(|| ExtError::NoExtension(id.extension.clone()))?;
            row.enabled = enabled;
            Ok(())
        })?;
        Ok(snapshot)
    }

    /// Show or hide an installed extension's button on the activity rail.
    ///
    /// Deliberately *not* folded into [`Self::set_enabled`]: hiding a rail icon must not stop a
    /// worker, drop a language or silence a diagnostic. See
    /// [`crate::config::Installed::rail_icon`].
    pub fn set_rail_icon(&self, id: &ExtensionRef, rail_icon: bool) -> Result<ExtensionSnapshot> {
        let (_, snapshot) = self.update(|config| {
            let row = config
                .installed
                .iter_mut()
                .find(|i| i.marketplace == id.marketplace && i.extension == id.extension)
                .ok_or_else(|| ExtError::NoExtension(id.extension.clone()))?;
            row.rail_icon = rail_icon;
            Ok(())
        })?;
        Ok(snapshot)
    }

    /// The directory in a clone that an extension's manifest lives in.
    fn entry_dir(&self, id: &ExtensionRef) -> Result<PathBuf> {
        let clone = clone_path(&id.marketplace);
        if !market::is_clone(&clone) {
            return Err(ExtError::NoMarketplace(id.marketplace.clone()));
        }
        manifest::read_index(&clone)
            .rows
            .into_iter()
            .find(|row| row.id == id.extension)
            .map(|row| row.dir)
            .ok_or_else(|| ExtError::NoExtension(id.extension.clone()))
    }
}

/// Where a marketplace's clone lives.
#[must_use]
pub fn clone_path(id: &MarketplaceId) -> PathBuf {
    config::marketplaces_dir().join(id.as_str())
}

/// Where an installed extension's code lives.
#[must_use]
pub fn install_path(market: &MarketplaceId, ext: &ExtensionId) -> PathBuf {
    config::extensions_dir()
        .join(market.as_str())
        .join(ext.as_str())
}

/// Read every clone and every installed tree, and merge.
fn rebuild(inner: &mut Inner, mut problems: Vec<ExtProblem>) {
    let scan = scan(&inner.config);
    problems.extend(scan.problems);

    // Only enabled, non-greyed extensions contribute. A greyed one keeps its row — that is where
    // the reason is printed — but nothing of it reaches the registry, because a manifest cide
    // only half understood is a manifest whose contributions it cannot vouch for.
    let contributing: Vec<&InstalledExtension> = scan
        .extensions
        .iter()
        .filter(|e| e.enabled && e.unavailable.is_none())
        .collect();
    let contributors: Vec<Contributor<'_>> = contributing
        .iter()
        .map(|e| Contributor {
            id: e.id.clone(),
            manifest: e.path.join(manifest::EXT_FILE),
            contributes: &e.contributes,
        })
        .collect();

    let resolved = contribute::resolve(
        cide_ipc::lang::builtins(),
        cide_ipc::lang::builtin_servers(),
        &contributors,
    );

    inner.rev = inner.rev.saturating_add(1);
    inner.snapshot = ExtensionSnapshot {
        rev: inner.rev,
        marketplaces: scan.marketplaces,
        extensions: scan.extensions,
        resolved,
        problems,
    };
}

/// Read every connected marketplace's clone and every installed extension's tree.
fn scan(config: &ExtConfig) -> Scan {
    let mut out = Scan::default();

    for connected in &config.marketplaces {
        let clone = clone_path(&connected.id);
        let authenticated = market::route(&connected.source) == market::Route::Remote;
        if !market::is_clone(&clone) {
            out.marketplaces.push(Marketplace {
                id: connected.id.clone(),
                name: connected.id.to_string(),
                source: connected.source.clone(),
                authenticated,
                state: MarketplaceState::Missing,
                path: clone,
                entries: Vec::new(),
                problems: Vec::new(),
            });
            continue;
        }
        let index = manifest::read_index(&clone);
        let head = market::head(&ProxyEnv::default(), &clone);
        let mut entries = Vec::new();
        for row in &index.rows {
            let installed = config.installed(&connected.id, &row.id);
            match manifest::read_manifest(&row.dir) {
                Ok(read) => entries.push(MarketplaceEntry {
                    id: row.id.clone(),
                    name: read.name,
                    version: read.version.clone(),
                    description: read.description,
                    capabilities: read.capabilities,
                    installed: installed.map(|i| i.version.clone()),
                    // The *declared* version, and nothing else. This used to also compare the
                    // installed extension's pinned commit against the clone's HEAD, as a net
                    // under authors who ship code without bumping `version` — but HEAD moves for
                    // the whole repository, so the first time a marketplace gained an extension,
                    // every other extension installed from it offered a byte-identical "update".
                    // A button that cries wolf teaches people to stop reading it. The version is
                    // the author's own claim that something shipped, it is what the marketplace
                    // README already demands be bumped with every code change, and inequality
                    // (not ordering) keeps a rollback offerable too. The per-directory tree hash
                    // was considered and lost: one `git` fork per row per refresh to catch only
                    // authors ignoring their own versioning discipline.
                    update_available: installed.is_some_and(|i| i.version != read.version),
                    unavailable: read.unavailable,
                }),
                Err(problem) => {
                    let message = problem.message.clone();
                    entries.push(MarketplaceEntry {
                        id: row.id.clone(),
                        name: row.id.to_string(),
                        version: String::new(),
                        description: String::new(),
                        capabilities: Vec::new(),
                        installed: installed.map(|i| i.version.clone()),
                        update_available: false,
                        unavailable: Some(message.clone()),
                    });
                    out.problems.push(problem);
                }
            }
        }
        out.marketplaces.push(Marketplace {
            id: connected.id.clone(),
            name: index.name,
            source: connected.source.clone(),
            authenticated,
            state: match head {
                Some(head) => MarketplaceState::Ready {
                    head,
                    fetched_at: fetched_at(&clone),
                },
                None => MarketplaceState::Failed {
                    error: "this directory is not a git repository any more.".into(),
                },
            },
            path: clone,
            entries,
            problems: index.problems,
        });
    }

    for row in &config.installed {
        let path = install_path(&row.marketplace, &row.extension);
        let id = ExtensionRef {
            marketplace: row.marketplace.clone(),
            extension: row.extension.clone(),
        };
        match manifest::read_manifest(&path) {
            Ok(read) => {
                let mut unavailable = read.unavailable.clone();
                let mut problems = read.problems.clone();
                let granted = row.granted_caps();
                // The consent check again, on every load and not only at install: the installed
                // tree is copied, so its manifest cannot change under us — but `extensions.json`
                // is a file the user may hand-edit, and a `granted` list that no longer covers
                // what the manifest asks for must grey the extension rather than be topped up.
                let asked: BTreeSet<Capability> = read.capabilities.iter().copied().collect();
                let given: BTreeSet<Capability> = granted.iter().copied().collect();
                if !asked.is_subset(&given) {
                    let missing: Vec<&str> = asked.difference(&given).map(|c| c.as_str()).collect();
                    let message = format!(
                        "asks for {} which you have not granted. Reinstall it to review what it \
                         wants.",
                        missing.join(", ")
                    );
                    problems.push(error(&path.join(manifest::EXT_FILE), None, message.clone()));
                    unavailable.get_or_insert(message);
                }
                // Resolved here and nowhere else: the defaults with the user's changes layered
                // on, every one through `SettingDef::coerce`. A reader — the Settings page, a
                // worker — never has to know what a default is or what a stored value of the
                // wrong type means.
                let settings = read
                    .contributes
                    .settings
                    .iter()
                    .map(|def| (def.id.clone(), def.coerce(row.settings.get(&def.id))))
                    .collect();
                out.extensions.push(InstalledExtension {
                    id,
                    name: read.name,
                    version: read.version,
                    description: read.description,
                    enabled: row.enabled,
                    rail_icon: row.rail_icon,
                    commit: row.commit.clone(),
                    capabilities: granted,
                    contributes: read.contributes,
                    settings,
                    // Resolved through the same jail `cide-ext://` uses, so a `main` that leaves
                    // the installed directory is `None` — a declarative extension — rather than a
                    // URL the protocol handler would refuse at load time with nothing on screen
                    // to say why.
                    main: read
                        .main
                        .as_ref()
                        .and_then(|main| assets::resolve(&path, main).ok().map(|_| main.clone())),
                    path,
                    unavailable,
                    problems,
                });
            }
            Err(problem) => {
                // Installed according to the config and not on disk: a `--fresh` launch, a
                // cleaned `$XDG_STATE_HOME`, a half-finished install. The row stays so the user
                // can see what to reinstall.
                out.extensions.push(InstalledExtension {
                    id,
                    name: row.extension.to_string(),
                    version: row.version.clone(),
                    description: String::new(),
                    enabled: false,
                    // Whatever the config says: the row is here so the user can see what to
                    // reinstall, and their rail choice should survive the reinstall.
                    rail_icon: row.rail_icon,
                    commit: row.commit.clone(),
                    capabilities: row.granted_caps(),
                    contributes: Default::default(),
                    // Nothing declares them, so nothing resolves. The stored values survive in
                    // `extensions.json` and come back if the extension is installed again.
                    settings: Default::default(),
                    main: None,
                    path,
                    unavailable: Some(format!(
                        "its files are not on disk any more ({}). Install it again.",
                        problem.message
                    )),
                    problems: vec![problem],
                });
            }
        }
    }

    out
}

/// When a clone was last written, as epoch milliseconds.
///
/// The mtime of `.git/FETCH_HEAD`, falling back to the directory's own — a fetch that changed
/// nothing still touches it, which is what the panel's *"checked N minutes ago"* means.
fn fetched_at(clone: &std::path::Path) -> i64 {
    let candidate = clone.join(".git").join("FETCH_HEAD");
    let meta = std::fs::metadata(&candidate).or_else(|_| std::fs::metadata(clone));
    meta.ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// A source name for a contribution, for messages outside `contribute`.
#[must_use]
pub fn describe_source(source: &ContributionSource) -> String {
    match source {
        ContributionSource::Builtin => "cide".to_string(),
        ContributionSource::Extension { extension } => extension.to_string(),
    }
}

/// A problem about `extensions.json` itself.
#[must_use]
pub fn config_warning(message: impl Into<String>) -> ExtProblem {
    warning(&config::config_path(), None, message)
}

/// The manifests an extension's worker is allowed to be started from.
///
/// `Manifest::main` resolved against the installed tree, refused if it leaves it. The refusal is
/// [`assets::resolve`]'s, reused rather than restated: the worker URL goes through the same
/// scheme as every other asset, so it must go through the same jail.
pub fn worker_entry(root: &std::path::Path, manifest: &Manifest) -> Option<PathBuf> {
    let main = manifest.main.as_ref()?;
    assets::resolve(root, main).ok()
}

/// One extension's page, as the README tab draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// The README's text, or `None` when the extension ships none.
    pub readme: Option<String>,
    /// The file it came from, for the tab's tooltip and for resolving relative links.
    pub path: Option<PathBuf>,
}

impl ExtStore {
    /// Read one extension's README.
    ///
    /// **From the installed tree when it is installed, and from the marketplace clone when it is
    /// not.** That is the whole reason this is a method rather than a path the frontend builds: an
    /// uninstalled extension has a README worth reading — it is what a person decides *whether to
    /// install* by — and the only copy of it is in a clone directory the frontend has no business
    /// knowing about.
    ///
    /// Never fails. An extension with no README is a page with a sentence saying so, which is a
    /// state the tab draws; an `Err` here would be a tab that will not open because a file the
    /// author chose not to write is missing.
    #[must_use]
    pub fn page(&self, id: &ExtensionRef) -> Page {
        // Every name a README is plausibly spelled with, in the order a person would expect one to
        // win. Not a glob: this is a directory a marketplace controls, and matching `*.md` would
        // let the first file alphabetically become the page.
        const NAMES: &[&str] = &["README.md", "readme.md", "README", "Readme.md"];

        let installed = self
            .inner
            .lock()
            .snapshot
            .extensions
            .iter()
            .find(|e| e.id == *id)
            .map(|e| e.path.clone());
        let from_clone = || self.entry_dir(id).ok();

        for root in [installed, from_clone()].into_iter().flatten() {
            for name in NAMES {
                // Through the jail, not `root.join(name)` — the names above are constants, so this
                // cannot currently fail, and routing them through the same check every other path
                // takes is what keeps that true if somebody makes the list configurable.
                let Ok(path) = assets::resolve(&root, name) else {
                    continue;
                };
                if let Ok(text) = std::fs::read_to_string(&path) {
                    return Page {
                        readme: Some(text),
                        path: Some(path),
                    };
                }
            }
        }
        Page {
            readme: None,
            path: None,
        }
    }
}

impl ExtStore {
    /// Everything one extension's page draws: its catalog row, its installed row, and its README.
    ///
    /// One answer rather than three calls, on `Bootstrap`'s argument: a tab that had to make
    /// several requests before it could paint would show several intermediate states, and here the
    /// first of them is a header over an empty document.
    #[must_use]
    pub fn page_for(&self, id: &ExtensionRef) -> cide_ipc::ext::ExtensionPage {
        let page = self.page(id);
        let inner = self.inner.lock();
        cide_ipc::ext::ExtensionPage {
            id: id.clone(),
            entry: inner
                .snapshot
                .marketplaces
                .iter()
                .find(|m| m.id == id.marketplace)
                .and_then(|m| m.entries.iter().find(|e| e.id == id.extension))
                .cloned(),
            installed: inner
                .snapshot
                .extensions
                .iter()
                .find(|e| e.id == *id)
                .cloned(),
            readme: page.readme,
            readme_path: page.path,
        }
    }
}
