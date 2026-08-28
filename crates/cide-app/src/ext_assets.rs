//! Serving an extension's own files to the webview, over `cide-ext://`. (M22)
//!
//! An extension's worker is an ES module, and a module has to be *fetched* by a URL — there is no
//! way to hand `new Worker()` a string of source and keep module semantics. So there has to be a
//! URL scheme that maps to an installed extension's directory, and that makes this the one place
//! in cide where a request from the renderer chooses a file on disk.
//!
//! # The shape of the URL
//!
//! `cide-ext://localhost/<marketplace>/<extension>/<path>` — and `http://cide-ext.localhost/…`
//! with the same path on Windows and Android, which is why the identity pair is in the path and
//! not the host (`cide_ext::assets::split_path` argues it, `ui/src/ext/assetUrl.ts` builds it).
//! The pair and not the extension id alone, because an id is not unique — two marketplaces may
//! each ship a `sql`. Both halves are `cide_ext::manifest::is_safe_segment`, which forbids `.`
//! and `/` outright, so the split is unambiguous and no request can name a directory that is not
//! an installed extension's.
//!
//! # Why not the asset protocol
//!
//! `tauri-plugin-asset`'s `convertFileSrc` already serves files by absolute path, and cide already
//! uses it for images. It is the wrong tool here twice over. It has a *scope* — a list of allowed
//! paths — that would have to be rewritten on every install and every uninstall, and a scope that
//! is edited at runtime is a scope that is wrong for the window between the two writes. And it
//! serves by absolute path, so an extension's worker would receive a URL naming the user's home
//! directory, which is a fact about the machine that nothing running an extension should be told.
//!
//! # What is refused
//!
//! A disabled extension serves nothing. That is worth stating because it is the difference between
//! *disabled* meaning "its panels are hidden" and meaning "it is not running": a disabled
//! extension whose worker could still be fetched by a stale URL would be a disabled extension that
//! is running.
//!
//! Everything else is `cide_ext::assets`' jail, which refuses on the string before touching the
//! filesystem and then canonicalises and re-checks. Its own header sets out why both halves are
//! there.

use cide_ext::assets::{self, AssetError};
// `tauri::http` and not a direct `http` dependency: tauri re-exports the exact version its
// runtime is built against, and a second one in the graph would be two incompatible `Response`
// types with the same name — the 0.x hazard `[workspace.dependencies]`'s header warns about for
// `wry` and `tao`, arrived at from a different direction.
use tauri::http;
use tauri::{Manager, UriSchemeContext};

use crate::ext_state::ExtState;

/// Answer one `cide-ext://` request.
///
/// Never panics and never blocks for long: a read of a file that is at most a few hundred
/// kilobytes, on the thread Tauri hands it. Every refusal is a 403 or a 404 with an empty body —
/// the reason goes to the log and not to the response, because the requester is the one thing that
/// controls the path and echoing it back is how a refusal message becomes a probe.
pub fn respond(
    context: UriSchemeContext<'_, tauri::Wry>,
    request: http::Request<Vec<u8>>,
) -> http::Response<Vec<u8>> {
    let uri = request.uri();
    let (id, rest) = match assets::split_path(uri.path()) {
        Ok((marketplace, extension, rest)) => (
            cide_ipc::ext::ExtensionRef {
                marketplace,
                extension,
            },
            rest,
        ),
        Err(error) => return refuse(http::StatusCode::BAD_REQUEST, &error.to_string()),
    };

    let app = context.app_handle();
    let Some(state) = app.try_state::<ExtState>() else {
        return refuse(http::StatusCode::SERVICE_UNAVAILABLE, "no extension store");
    };
    // `None` for an extension that is not installed *or* not enabled — see the module header.
    let Some(root) = state.store().asset_root(&id) else {
        return refuse(http::StatusCode::FORBIDDEN, "not an enabled extension");
    };

    match assets::resolve(&root, &rest) {
        Ok(path) => match std::fs::read(&path) {
            Ok(bytes) => http::Response::builder()
                .status(http::StatusCode::OK)
                .header(http::header::CONTENT_TYPE, assets::content_type(&path))
                // The renderer's origin is `tauri://localhost` (or `http://tauri.localhost`), and
                // a module worker is fetched with CORS. Without this the worker fails to load with
                // a message about the origin rather than about the extension, which is the kind of
                // failure that sends somebody looking in the wrong place for an hour.
                .header(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                // Installed code is immutable: the tree is copied at a pinned commit and an update
                // is a new install into a directory that is replaced wholesale. A long cache is
                // therefore safe *and* load-bearing — a panel that re-imports its worker on every
                // open should not re-read the disk each time.
                .header(http::header::CACHE_CONTROL, "public, max-age=3600")
                .body(bytes)
                .unwrap_or_else(|_| refuse(http::StatusCode::INTERNAL_SERVER_ERROR, "build")),
            Err(error) => {
                tracing::debug!(%error, extension = %id, "cide-ext read failed");
                refuse(http::StatusCode::NOT_FOUND, "unreadable")
            }
        },
        Err(AssetError::Escapes) => {
            // Logged at `warn`, unlike the others: a path that tried to leave the jail is either a
            // bug in an extension or an attempt, and both are worth a line somebody might read.
            tracing::warn!(extension = %id, "a cide-ext request tried to leave its directory");
            refuse(http::StatusCode::FORBIDDEN, "escapes")
        }
        Err(error) => refuse(http::StatusCode::NOT_FOUND, &error.to_string()),
    }
}

fn refuse(status: http::StatusCode, why: &str) -> http::Response<Vec<u8>> {
    tracing::debug!(%status, why, "cide-ext refused a request");
    http::Response::builder()
        .status(status)
        .body(Vec::new())
        .unwrap_or_default()
}
