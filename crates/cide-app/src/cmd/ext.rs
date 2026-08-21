//! Extensions and marketplaces: the command surface. (M22)
//!
//! Thin by policy like every other module here: reach the one [`ExtStore`](cide_ext::ExtStore),
//! call one method on it, publish what comes back. Nothing in this file knows what a git route is,
//! what a capability means, or how two extensions claiming `.sql` are resolved.
//!
//! Four shapes, each a decision rather than a habit, and three of them are `cmd::tasks`' shapes
//! reused deliberately:
//!
//! **Every mutation answers with the whole snapshot**, on the argument `cmd::git` states one panel
//! over: a call that returned `{ok: true}` would be followed at once by a second asking what
//! happened, and the frame in between shows a list that is visibly wrong. It is stronger here than
//! for tasks — installing an extension changes the *language registry*, so a panel that re-asked
//! would draw a row saying "not installed" beside a `.sql` file that has already changed colour.
//!
//! **Every mutation also publishes.** `ext_state::publish` mirrors the resolved set into the two
//! globals that read it — the language table and `cide-lsp`'s server registry — and emits
//! `cide://ext-changed` for the *other* windows. Both halves in one call, so a command cannot do
//! one of them.
//!
//! **Nothing here runs on the main thread.** Tauri 2 polls a synchronous command on the GTK loop,
//! and a clone can take thirty seconds on hotel wifi. Every handler is `async` and hands its work
//! to [`blocking`].
//!
//! **Consent is never something the caller composes.** [`ext_install`] takes the capability list
//! the user was *shown*, and `ExtStore::install` refuses if the manifest has since changed. Taking
//! them from the manifest instead would mean a marketplace could add `process:spawn` between the
//! sheet being drawn and the button being pressed. This is the same rule `cmd::tasks` states for
//! `TaskAuthor` — identity and permission must not arrive in the payload unchecked — arrived at
//! from the other direction: there the caller may not name it, here the caller must, and must
//! match.

use std::sync::Arc;

use cide_core::CoreError;
use cide_ext::ExtStore;
use cide_ipc::ext::{Capability, ConnectRequest, ExtensionRef, ExtensionSnapshot, InstallRequest};
use cide_ipc::ids::MarketplaceId;
use tauri::State;

use crate::ext_state::{self, ExtState};
use crate::workspace_state::WorkspaceState;

type Result<T> = std::result::Result<T, CoreError>;

/// Run store work on the blocking pool.
///
/// `cmd::tasks`' helper verbatim, and its note applies unchanged: `#[tauri::command(async)]` and an
/// `async fn` whose body never awaits are both *not* the fix, because either one only moves the
/// call onto the async runtime where a blocking file read still occupies a runtime worker for its
/// whole duration.
async fn blocking<T>(work: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T>
where
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| CoreError::Io(format!("the extension worker did not finish: {error}")))?
}

fn refused(error: cide_ext::ExtError) -> CoreError {
    CoreError::Io(error.to_string())
}

/// The proxy settings a `git` child should inherit.
///
/// Resolved on the caller's thread, because `ProxyEnv::for_target` reads `std::env::var` and the
/// blocking pool is not where the process environment should be sampled from. `cmd::git` resolves
/// it the same way and for the same reason.
fn proxy(state: &State<'_, WorkspaceState>) -> cide_core::proxy::ProxyEnv {
    let settings = state.with(|ws| ws.settings.proxy.clone());
    cide_core::proxy::ProxyEnv::for_target(&settings, settings.scope.git)
}

/// Everything the Extensions panel draws.
#[tauri::command(rename_all = "camelCase")]
pub async fn ext_snapshot(state: State<'_, ExtState>) -> Result<ExtensionSnapshot> {
    let store = state.store();
    blocking(move || Ok(store.snapshot())).await
}

/// Re-read `extensions.json`, every clone and every installed tree.
///
/// Called freely, and cheap for the reason `cide-agents` gives for reading the disk on every ask:
/// a clone directory is one the user can `git pull` by hand, so a cached answer is one that is
/// wrong for as long as nobody happens to invalidate it.
#[tauri::command(rename_all = "camelCase")]
pub async fn ext_reload(
    app: tauri::AppHandle,
    state: State<'_, ExtState>,
) -> Result<ExtensionSnapshot> {
    let store = state.store();
    let snapshot = blocking(move || Ok(store.reload())).await?;
    ext_state::publish(&app, &snapshot);
    Ok(snapshot)
}

/// Connect a marketplace by URL or local path, and clone it.
#[tauri::command(rename_all = "camelCase")]
pub async fn ext_connect(
    app: tauri::AppHandle,
    state: State<'_, ExtState>,
    workspace: State<'_, WorkspaceState>,
    req: ConnectRequest,
) -> Result<ExtensionSnapshot> {
    let store = state.store();
    let proxy = proxy(&workspace);
    let snapshot = blocking(move || store.connect(&proxy, &req.source).map_err(refused)).await?;
    ext_state::publish(&app, &snapshot);
    Ok(snapshot)
}

/// Forget a marketplace, and uninstall everything that came from it.
#[tauri::command(rename_all = "camelCase")]
pub async fn ext_disconnect(
    app: tauri::AppHandle,
    state: State<'_, ExtState>,
    marketplace: MarketplaceId,
) -> Result<ExtensionSnapshot> {
    let store = state.store();
    let snapshot = blocking(move || store.disconnect(&marketplace).map_err(refused)).await?;
    ext_state::publish(&app, &snapshot);
    Ok(snapshot)
}

/// Fetch a marketplace and re-read its index.
#[tauri::command(rename_all = "camelCase")]
pub async fn ext_refresh(
    app: tauri::AppHandle,
    state: State<'_, ExtState>,
    workspace: State<'_, WorkspaceState>,
    marketplace: MarketplaceId,
) -> Result<ExtensionSnapshot> {
    let store = state.store();
    let proxy = proxy(&workspace);
    // Never an `Err`: a marketplace that will not fetch is a row in a `Failed` state with its
    // reason in it, and the panel draws that. An error here would mean refreshing four
    // marketplaces stopped at the first one behind a VPN.
    let snapshot = blocking(move || Ok(store.refresh(&proxy, &marketplace))).await?;
    ext_state::publish(&app, &snapshot);
    Ok(snapshot)
}

/// Install, or update, one extension — with the capabilities the user approved.
#[tauri::command(rename_all = "camelCase")]
pub async fn ext_install(
    app: tauri::AppHandle,
    state: State<'_, ExtState>,
    req: InstallRequest,
) -> Result<ExtensionSnapshot> {
    let store: Arc<ExtStore> = state.store();
    let granted: Vec<Capability> = req.granted.clone();
    let snapshot = blocking(move || store.install(&req.id, &granted).map_err(refused)).await?;
    ext_state::publish(&app, &snapshot);
    Ok(snapshot)
}

/// Remove an installed extension. Its marketplace stays connected.
#[tauri::command(rename_all = "camelCase")]
pub async fn ext_uninstall(
    app: tauri::AppHandle,
    state: State<'_, ExtState>,
    extension: ExtensionRef,
) -> Result<ExtensionSnapshot> {
    let store = state.store();
    let snapshot = blocking(move || store.uninstall(&extension).map_err(refused)).await?;
    ext_state::publish(&app, &snapshot);
    Ok(snapshot)
}

/// Switch an installed extension on or off.
#[tauri::command(rename_all = "camelCase")]
pub async fn ext_set_enabled(
    app: tauri::AppHandle,
    state: State<'_, ExtState>,
    extension: ExtensionRef,
    enabled: bool,
) -> Result<ExtensionSnapshot> {
    let store = state.store();
    let snapshot =
        blocking(move || store.set_enabled(&extension, enabled).map_err(refused)).await?;
    ext_state::publish(&app, &snapshot);
    Ok(snapshot)
}

// ---------------------------------------------------------------------------------------
// What a worker publishes back.
//
// Two commands, and both take a project because the thing they write to is per project while the
// extension that wrote them is not. The frontend supplies it — it is the project whose editor the
// worker was told about — and neither command trusts anything else from the payload: the source id
// is derived from the extension reference, never taken from it, so an extension cannot publish
// under `rustAnalyzer` and have its findings restart somebody else's server.
// ---------------------------------------------------------------------------------------

/// Publish an extension's findings for one file.
#[tauri::command(rename_all = "camelCase")]
pub async fn ext_publish_diagnostics(
    app: tauri::AppHandle,
    diagnostics: State<'_, crate::lsp::DiagnosticsRegistry>,
    project: cide_ipc::ProjectId,
    extension: ExtensionRef,
    abs_path: String,
    items: Vec<cide_ipc::Diagnostic>,
) -> Result<()> {
    let Some(entry) = diagnostics.get(project) else {
        // A project with no diagnostics registry is one that was closed while a worker was
        // thinking. Not an error: the extension has nothing to do about it, and a rejected promise
        // in a worker's request path is an exception in somebody else's code.
        return Ok(());
    };
    let source = source_of(&extension);
    let mut items = items;
    for item in &mut items {
        // The producer's own name is the extension's, always. `Diagnostic::source` is a free
        // string precisely so an unrecognised one survives to the panel — `clippy` arrives inside
        // rust-analyzer's stream — and that freedom is exactly why it must be stamped here rather
        // than trusted: a row claiming to be from `rust-analyzer` would be filtered, grouped and
        // restarted as if it were.
        item.source = extension.to_string();
    }
    entry.publish_external(&app, source, abs_path, items);
    Ok(())
}

/// The id an extension's findings are attributed to.
///
/// `ext:<marketplace>.<extension>`, which is a value `DiagnosticSourceId::for_server` can never
/// produce for a language server: server ids are bare binary names, and a binary name containing a
/// colon is refused by `cide_ext::manifest`. So the two namespaces cannot collide, and the panel's
/// Restart button — which asks the LSP registry for a server with this id — finds nothing and does
/// not offer one, which is correct: there is no process to restart.
fn source_of(extension: &ExtensionRef) -> cide_ipc::DiagnosticSourceId {
    cide_ipc::DiagnosticSourceId(format!("ext:{extension}"))
}

/// One extension's page: its catalog row, its installed row, and its README.
///
/// A read, so it takes no project and mutates nothing. It is not on the snapshot because a README
/// is potentially tens of kilobytes and the snapshot rides every `cide://ext-changed` to every
/// window — a page is read once, by the tab that is about to draw it.
#[tauri::command(rename_all = "camelCase")]
pub async fn ext_page(
    state: State<'_, ExtState>,
    extension: ExtensionRef,
) -> Result<cide_ipc::ext::ExtensionPage> {
    let store = state.store();
    blocking(move || Ok(store.page_for(&extension))).await
}

/// Open this extension's page as a workspace tab, or activate the one it already has.
///
/// A singleton **per extension** and not per project: two pages are two different READMEs, and a
/// user comparing SQL against YAML wants both open. `cmd::project::same_tab` holds the rule; this
/// only has to name the kind.
#[tauri::command(rename_all = "camelCase")]
pub async fn tab_open_extension(
    state: State<'_, WorkspaceState>,
    project: cide_ipc::ProjectId,
    extension: ExtensionRef,
    name: String,
) -> Result<cide_ipc::TabId> {
    let kind = cide_ipc::TabKind::Extension {
        marketplace: extension.marketplace.clone(),
        extension: extension.extension.clone(),
        // Falls back to the id, so a tab opened from a row whose manifest would not parse still
        // has a caption. An empty tab title is a strip with a gap in it.
        name: if name.trim().is_empty() {
            extension.extension.to_string()
        } else {
            name
        },
    };
    state.update(|ws| {
        let p = cide_core::workspace::project(ws, project)?;
        let existing = p
            .tabs
            .iter()
            .find(|t| {
                matches!(
                    &t.kind,
                    cide_ipc::TabKind::Extension { marketplace, extension: ext, .. }
                        if *marketplace == extension.marketplace && *ext == extension.extension
                )
            })
            .map(|t| t.id);
        if let Some(id) = existing {
            cide_core::workspace::activate_tab(ws, project, id)?;
            return Ok(id);
        }
        cide_core::workspace::open_tab(
            ws,
            project,
            kind,
            cide_ipc::Pane {
                id: cide_ipc::PaneId::new(),
                kind: cide_ipc::PaneKind::Editor,
                role: cide_ipc::PaneRole::Auxiliary,
                // No process. The tree exists because every tab has one — that invariant is what
                // makes "promote pane to tab" and "split editor" one code path — and the page
                // simply renders over it, exactly as the Settings tab does.
                session: None,
                conversation: None,
                title: "extension".into(),
            },
        )
    })
}

/// Open a link from an extension's README in the user's browser.
///
/// # Why this is here and not a general `app_open_url`
///
/// Because it exists for exactly one caller and the refusal below is written for that caller's
/// threat. A README is **markdown somebody else wrote**, rendered in cide's own window, and its
/// links are the one part of it that can reach out of the page. So the scheme is checked here, in
/// Rust, and not in the component that renders the link: a check that lived only in the frontend
/// would be one `invoke` away from being bypassed — the same argument `cmd::settings`' clamps make
/// about a number input.
///
/// `http` and `https` only. Not `file:` (a README could name any path on the machine), not
/// `mailto:` (a desktop mail client is a program, and the user did not grant `process:spawn` to a
/// README), and not a custom scheme, because the set of handlers a desktop has registered is
/// unknown and some of them take arguments.
///
/// Opened from Rust rather than through the `opener` plugin's JS command, on
/// `app_open_log_dir`'s reasoning: the JS path is capability-gated per window and a detached-pane
/// window deliberately has none, so going through it would make a link work in some windows and
/// not others for reasons no user could guess.
#[tauri::command(rename_all = "camelCase")]
pub fn ext_open_link(app: tauri::AppHandle, url: String) -> std::result::Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let trimmed = url.trim();
    // Parsed rather than prefix-matched. `https:/\/evil` and `hTtPs://…` and a leading control
    // character all defeat `starts_with`, and none of them defeats asking for the scheme.
    let scheme = trimmed
        .split_once("://")
        .map(|(scheme, _)| scheme.to_ascii_lowercase())
        .unwrap_or_default();
    if scheme != "http" && scheme != "https" {
        return Err(format!(
            "cide opens http and https links only, and this one is `{scheme}`."
        ));
    }
    // A second look at the whole string, because the scheme test above says nothing about the
    // characters after it: a newline or a NUL in a URL handed to a desktop opener is an argument
    // injection on some platforms.
    if trimmed.contains(['\n', '\r', '\0', ' ']) {
        return Err("that link contains whitespace or a control character.".to_string());
    }
    app.opener()
        .open_url(trimmed, None::<&str>)
        .map_err(|error| format!("could not open the link: {error}"))
}
