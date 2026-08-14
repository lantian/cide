//! Structure: one file's outline, and the project-wide symbol picker.
//!
//! Two very different shapes behind one word, the same split `cide-search` makes:
//!
//! * [`symbols_outline`] parses **one file, on demand**, from the buffer the user is looking at.
//! * [`symbol_query`] fuzzy-matches a **repository**, streamed by a background walk.
//!
//! # Why the outline takes text
//!
//! Not a convenience. The editor holds unsaved edits, and an outline of on-disk content while the
//! user types is wrong in the one state where it matters most. Worse, the breadcrumb and the
//! member walk are *positional*: three inserted lines above the caret and an on-disk outline names
//! the wrong function, silently, for as long as the buffer stays dirty.
//!
//! `text: None` serves the callers with no live buffer, and goes through `cide_fs::ops` so the
//! containment check and the size cap are the ones the rest of the app already uses.
//!
//! # Why there is no cache here
//!
//! Ctrl+F12 is one parse per keypress — single-digit milliseconds for a container-only walk on the
//! blocking pool. The expensive surface is the *breadcrumb*, which updates on every caret move:
//! thirty parses and thirty IPC round trips a second under a held arrow key. So the cache lives in
//! the webview, once, and serves all three surfaces — `ui/src/editor/memberNav.ts` derives the
//! trail and the member step locally from one fetched outline.
//!
//! A `DashMap<PathBuf, (blake3::Hash, …)>` here was the obvious alternative. It would save a parse
//! only when the *same text* is asked for twice, which the frontend's own cache already prevents,
//! and it would add a second invalidation rule to get wrong. The genuinely right optimisation is
//! incremental reparse (`Parser::parse(text, Some(&old_tree))`), and it is out of scope because it
//! needs the edit deltas — which means shipping CodeMirror `ChangeSet`s over IPC. That is a
//! design, not a tweak.

use std::path::PathBuf;

use cide_ipc::{FileOutline, ProjectId, SymbolFrame, SymbolIndexStatus, SymbolRow};
use cide_lang::SymbolError;
use cide_search::Matcher as _;
use tauri::State;

use crate::files::FsRegistry;

type Result<T> = std::result::Result<T, SymbolError>;

/// At most this many rows per frame, matching the picker's own cap.
const DEFAULT_LIMIT: u32 = 50;

/// One file's structure, for the popup, the breadcrumb and the member walk.
#[tauri::command(rename_all = "camelCase")]
pub async fn symbols_outline(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    path: PathBuf,
    text: Option<String>,
) -> Result<FileOutline> {
    let roots = registry
        .get(project)
        .map(|fs| fs.root_paths())
        .unwrap_or_default();

    blocking(move || outline_of(&roots, &path, text)).await
}

/// The body of [`symbols_outline`], as a free function over the roots.
///
/// Split out for the same reason `cmd::file::openable` is: the containment rule below is the
/// half that must not rot, and a `#[tauri::command]` taking a `State` cannot be called from a
/// test.
///
/// # Containment guards the read, and only the read
///
/// It used to be the first statement in this function, before the `match` — and that was quietly
/// wrong. The check exists so that *a window cannot read any file by asking for its outline*;
/// when the caller supplies `text` no file is read, so there is nothing to guard, and refusing
/// anyway answered every out-of-project buffer with a rejection.
///
/// That rejection is *swallowed on purpose* by `ui/src/editor/outlineStore.ts` — a toast per
/// keystroke of the breadcrumb would be unusable — so the whole visible symptom was: Ctrl+F12
/// opens an empty popup, the status bar's member trail stops after the filename, and Alt+Down
/// does nothing. No error, anywhere. It was unreachable while nothing outside the roots could be
/// opened; Ctrl+B into `~/.cargo/registry/…` has reached it for as long as go-to-definition has
/// existed, and the approved out-of-project open in `cmd::file::terminal_open_path` makes it
/// ordinary.
///
/// Moving it into the `None` arm keeps the security property exactly as it was — the only thing
/// that touches the disk is inside the guard — and costs the outline nothing.
fn outline_of(
    roots: &[PathBuf],
    path: &std::path::Path,
    text: Option<String>,
) -> Result<FileOutline> {
    let text = match text {
        Some(text) => text,
        None => {
            if !roots.is_empty() && cide_fs::ops::check_within(roots, path).is_err() {
                return Err(SymbolError::Outside(path.display().to_string()));
            }
            std::fs::read_to_string(path).map_err(|error| SymbolError::Io {
                path: path.display().to_string(),
                message: error.to_string(),
            })?
        }
    };
    Ok(cide_lang::outline(
        path,
        &text,
        cide_lang::Limits::default(),
    ))
}

/// Build this project's symbol index, or report the one already built.
///
/// Idempotent and lazy: the overlay calls it on open, and a user who never presses
/// Ctrl+Alt+Shift+N pays nothing. Chaining it onto the file walk would make every project launch
/// parse every source file for a surface most sessions never use.
#[tauri::command(rename_all = "camelCase")]
pub async fn symbols_index(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
) -> Result<SymbolIndexStatus> {
    let fs = registry.get(project).ok_or(SymbolError::NotIndexed)?;
    blocking(move || Ok(fs.index_symbols())).await
}

/// Rank this project's symbols against a query.
#[tauri::command(rename_all = "camelCase")]
pub async fn symbol_query(
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    query: String,
    limit: Option<u32>,
) -> Result<SymbolFrame> {
    let fs = registry.get(project).ok_or(SymbolError::NotIndexed)?;
    blocking(move || {
        if !fs.symbols_started() {
            return Err(SymbolError::NotIndexed);
        }
        let limit = limit
            .unwrap_or(DEFAULT_LIMIT)
            .min(cide_search::MAX_FRAME as u32);

        // Read **before** the frame, for the reason `cmd::picker::query_project` spells out at
        // length — and it is the same bug one surface over, not an analogous one.
        // `Matcher::frame` fills `running` from `nucleo::tick`, which answers "does the scorer
        // have queued work". The symbol walk registers its project before it parses a byte, so an
        // overlay opened in the gap finds an empty, idle matcher, is told `running: false`, stops
        // polling, and shows an empty picker over a repository that is mid-walk.
        let indexing = fs.is_symbol_indexing();

        let matcher = fs.symbol_matcher();
        matcher.query(&query);
        // Over-fetch, because `resolve` drops rows the store no longer backs — see
        // `crate::symbols` for why the matcher is allowed to be stale. Without the headroom a
        // frame could come back short of `limit` while live rows were still available.
        let raw = matcher.frame((limit * 2).min(cide_search::MAX_FRAME as u32) as usize);

        let store = fs.symbol_store();
        let store = store.read();
        let items: Vec<SymbolRow> = raw
            .items
            .into_iter()
            .filter_map(|row| store.resolve(&row.value, &row.text, row.indices))
            .take(limit as usize)
            .collect();

        Ok(SymbolFrame {
            items,
            matched: raw.matched,
            total: raw.total,
            running: raw.running || indexing,
            query: raw.query,
            truncated: store.truncated,
        })
    })
    .await
}

/// Run on the blocking pool, never on the main thread.
///
/// Tauri 2 polls a plain `#[tauri::command]` on the main thread, and both of these read files —
/// see the note at `cmd::git`'s `blocking` for the full argument, including why an `async fn`
/// whose body never awaits is not the fix.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| SymbolError::Io {
            path: String::new(),
            message: format!("the symbol worker did not finish: {error}"),
        })?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch project root, removed and recreated so a rerun is not a failure.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cide-outline-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch root");
        dir
    }

    const RUST: &str = "pub fn alpha() {}\npub fn beta() {}\n";

    /// The defect this arrangement exists to prevent, stated as a test.
    ///
    /// Every editor surface that shows structure — Ctrl+F12, the status bar's member trail,
    /// Alt+Up/Down — asks with the live buffer. While the containment check sat above the
    /// `match`, all three answered nothing for a file the project does not contain, and
    /// answered it *silently*, because `outlineStore.ts` swallows the rejection on purpose.
    #[test]
    fn a_buffer_outside_the_project_still_has_an_outline_when_its_text_is_supplied() {
        let root = scratch("outside-with-text");
        let elsewhere =
            std::env::temp_dir().join(format!("cide-outline-elsewhere-{}.rs", std::process::id()));
        let outline = outline_of(&[root], &elsewhere, Some(RUST.to_owned()))
            .expect("supplied text needs no disk and therefore no containment check");
        let FileOutline::Ready { symbols, .. } = outline else {
            panic!("a .rs buffer parses whether or not the project contains it: {outline:?}");
        };
        assert_eq!(
            symbols.len(),
            2,
            "the popup, the breadcrumb and the member walk all read this list"
        );
    }

    /// And the property the check was there for is unchanged.
    #[test]
    fn a_file_outside_the_project_is_still_never_read_from_disk() {
        let root = scratch("outside-no-text");
        let elsewhere = root
            .parent()
            .expect("temp dir")
            .join(format!("cide-outline-secret-{}.rs", std::process::id()));
        std::fs::write(&elsewhere, RUST).expect("a perfectly readable file");
        assert!(
            matches!(
                outline_of(&[root], &elsewhere, None),
                Err(SymbolError::Outside(_))
            ),
            "with no text supplied this would `read_to_string` a path the project does not \
             contain, which is the one thing the guard is for"
        );
        let _ = std::fs::remove_file(&elsewhere);
    }

    /// A project with no index yet answers rather than refusing — the pre-existing rule.
    #[test]
    fn no_roots_means_no_containment_to_check() {
        let dir = scratch("no-roots");
        let file = dir.join("lib.rs");
        std::fs::write(&file, RUST).expect("write");
        let outline = outline_of(&[], &file, None).expect("no roots, nothing to be outside of");
        assert!(matches!(outline, FileOutline::Ready { ref symbols, .. } if symbols.len() == 2));
    }
}
