//! The one extension store, and what it hands the rest of the app. (M22)
//!
//! # One, not one per project
//!
//! Every other registry in this crate is a `DashMap<ProjectId, _>`, because the thing it holds
//! belongs to a project: a task tracker is a file in the repository, a diagnostics store is a view
//! of one workspace, an IDE server advertises one root. An extension is none of those. It is a
//! *tool* the user installed, chosen once and wanted everywhere, and `extensions.json` is global
//! for the reason `cide_ext::config`'s header sets out at length.
//!
//! So this is a single `ExtStore` behind managed state, and the shape below is the small half of
//! `tasks_state.rs`: `ensure`-free, because there is nothing to key on.
//!
//! # The language table, and why it is a global
//!
//! [`languages`] answers *"which language is this path"* for `cide_app::lsp::server_for`, which
//! runs on the LSP pump thread with no `AppHandle` and no `State` in reach. A `OnceLock` mirror of
//! the resolved list, refreshed whenever the store changes, is what lets that question be asked
//! from anywhere without threading a handle through four call frames that have no other use for
//! one.
//!
//! It is a mirror and not the truth: the truth is the store, and this is a snapshot of one field
//! of it. Nothing writes to it except [`install`].

use std::sync::{Arc, RwLock};

use cide_ext::ExtStore;
use cide_ipc::ext::ExtensionSnapshot;
use cide_ipc::lang::LanguageDef;
use tauri::AppHandle;

/// The resolved language table, for callers with no `State` in reach.
///
/// Seeded with the builtins so a question asked before the store has been read — during startup,
/// or in a test — gets the honest answer rather than "cide knows no languages".
static LANGUAGES: RwLock<Vec<LanguageDef>> = RwLock::new(Vec::new());

/// Every language in the resolved registry, builtins included.
#[must_use]
pub fn languages() -> Vec<LanguageDef> {
    let mirrored = LANGUAGES.read().ok().map(|l| l.clone()).unwrap_or_default();
    if mirrored.is_empty() {
        cide_ipc::lang::builtins()
    } else {
        mirrored
    }
}

/// Publish a snapshot's resolved set to the parts of the app that read a global.
///
/// Two tables move together and must: the language table this module mirrors, and `cide-lsp`'s
/// server registry. A window in which one had been updated and the other had not is a window in
/// which a `.yaml` resolves to a language whose server does not exist yet, which shows as a file
/// that is highlighted and never analysed.
///
/// **Safe to call with servers running.** `cide_lsp::discover`'s table is append-only: a row keeps
/// its index for the life of the process and a server whose extension was disabled is marked
/// inactive rather than removed, so a `Server` a live session is holding still resolves to the row
/// it was minted for. That property is what makes this a mirror update rather than a restart —
/// the alternative was stopping everything first, which is correct and costs a full
/// rust-analyzer re-index every time somebody toggles a YAML extension.
///
/// What it does **not** do is start or stop anything. A newly contributed server reaches a project
/// when its `ProjectDiagnostics` is next created; an existing project keeps the servers it has
/// until it is reopened. That is the honest limit of this call and `README.md` records it.
pub fn install(snapshot: &ExtensionSnapshot) {
    let languages: Vec<LanguageDef> = snapshot
        .resolved
        .languages
        .iter()
        .map(|binding| binding.def.clone())
        .collect();
    if let Ok(mut mirror) = LANGUAGES.write() {
        *mirror = languages;
    }
    let servers: Vec<cide_ipc::lang::LanguageServerDef> = snapshot
        .resolved
        .servers
        .iter()
        .map(|binding| binding.def.clone())
        .collect();
    cide_lsp::discover::install(servers);
}

/// The process's extension store.
///
/// `Arc` rather than a bare value in managed state, for the reason every other registry here is
/// one: `State` hands out a reference, and a `thread::spawn` — which every install does, because
/// a clone can take seconds — cannot keep it.
pub struct ExtState {
    store: Arc<ExtStore>,
}

impl Default for ExtState {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtState {
    /// Read `extensions.json`, every clone and every installed tree, and publish the result.
    #[must_use]
    pub fn new() -> Self {
        let store = Arc::new(ExtStore::new());
        install(&store.snapshot());
        Self { store }
    }

    #[must_use]
    pub fn store(&self) -> Arc<ExtStore> {
        Arc::clone(&self.store)
    }

    #[must_use]
    pub fn snapshot(&self) -> ExtensionSnapshot {
        self.store.snapshot()
    }
}

/// Publish a new snapshot: mirror it, and tell every window.
///
/// The one place both happen, so a command cannot do half of it. `cide://ext-changed` carries the
/// whole snapshot **and** its `rev` — see `emit.rs`, where the choice between carrying one and not
/// is argued for this event specifically.
pub fn publish(app: &AppHandle, snapshot: &ExtensionSnapshot) {
    install(snapshot);
    crate::emit::ext_changed(app, snapshot);
}
