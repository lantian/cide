//! What an image tab is showing. (M18)
//!
//! Deliberately tiny, and deliberately **not** the bytes. Every other document type in this
//! crate carries its contents — `FileDoc` has `text`, `FileDiff` has hunks — and an image
//! does not, for a reason that is worth stating once here rather than rediscovering at the
//! first 40 MB PNG.
//!
//! Tauri's IPC is JSON in both directions. A `Vec<u8>` crossing it becomes a JSON array of
//! decimal numbers — three to four characters per byte — which `cide-pty`'s coalescer already
//! exists to keep off the GTK main loop for terminal output measured in kilobytes. A
//! base64 `data:` URI is better and still wrong: it inflates by a third, it is a `String`
//! that has to be built in Rust, parsed by the JSON reader, held by the JS engine and decoded
//! again by the image decoder, so a 40 MB PNG is well over 100 MB of peak footprint spread
//! across three heaps, all of it on the thread that also draws the window.
//!
//! So this DTO carries **identity**, and the pixels travel down Tauri's asset protocol
//! instead — a real streaming response, with range support, served off the webview's own
//! network thread and never marshalled at all. `crates/cide-app/src/cmd/file.rs::image_read`
//! is the whole of that decision and states how the scope is granted; `ui/src/panes/ImagePane.tsx`
//! is the consumer.
//!
//! The same argument as [`crate::DiffSpec`], one layer down: a key re-reads, a copy lies.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::FileStamp;

/// The image formats cide will put on screen, as **sniffed from the file's own bytes**.
///
/// Not derived from the extension. The extension decides which *viewer* a tab opens with —
/// that rule is `ui/src/panes/imageKinds.ts`, on the frontend, because it has to be answered
/// before any IPC happens — and this is the answer to the different question of what the file
/// actually contains. Keeping them apart is what lets a `.png` that is really a JPEG render
/// correctly *and* describe itself honestly in the status bar, and it is what makes
/// "this claims to be a PNG and is not" a sentence rather than a blank pane.
///
/// The list is what a WebKitGTK `<img>` decodes without help. TIFF, AVIF, JPEG XL and `.svgz`
/// are deliberately absent: the first two are engine-version-dependent, and a gzipped SVG
/// needs a `Content-Encoding` the asset protocol does not set, so it would fail *silently* in
/// the image decoder — the one failure mode this whole feature is written to avoid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    Webp,
    Bmp,
    /// Windows icon. Several sizes in one file; the decoder picks one and `naturalWidth`
    /// reports what it picked, which is why dimensions are measured on the frontend.
    Ico,
    /// Scalable Vector Graphics — the one text format in the list, and the one with a
    /// security decision attached. See `ui/src/panes/ImagePane.tsx`: it is rendered through
    /// `<img>`, never inlined, because an `<img>` puts SVG in the spec's secure static mode
    /// (no script, no external references, no interactivity) while inlining it would run any
    /// `<script>` inside it *with this webview's origin*, which can `invoke` every Tauri
    /// command in the app.
    Svg,
}

/// One image file, as far as the pane needs to know before it can draw it.
///
/// Answered by `image_read`, which has already proved three things by the time this exists:
/// the file is within the cap, its bytes really are one of [`ImageFormat`], and the asset
/// protocol scope has been widened to admit exactly this path. A pane holding one of these
/// can build a URL and render it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImageDoc {
    /// **The canonical path**, which is what the asset URL is built from — not the path that
    /// was asked for.
    ///
    /// Tauri's asset scope canonicalises before it matches (`scope::fs::Scope::is_allowed`
    /// calls `try_resolve_symlink_and_canonicalize`), while the protocol handler itself does
    /// a plain `File::open` on whatever string the URL carried. Handing the frontend the
    /// canonical form makes those two agree by construction. Building the URL from the
    /// symlink instead would work today and break the day the scope's matcher changes, and
    /// the failure would be a 403 the `<img>` reports as a generic load error.
    ///
    /// The tab still *opens* on the path as given — see `tab_open_file` — so the caller keeps
    /// the name the user pointed at for display. This field is a fetch address, not a label.
    #[ts(type = "string")]
    pub path: PathBuf,
    /// What the bytes say the file is, which the caller may not assume matches its extension.
    pub format: ImageFormat,
    /// The file's size on disk, for the readout.
    ///
    /// A `number` on the wire rather than [`FileStamp::mtime_nanos`]'s decimal string: the cap
    /// is 32 MiB, so this cannot approach `Number.MAX_SAFE_INTEGER` and there is nothing here
    /// for a double to round away.
    #[ts(type = "number")]
    pub bytes: u64,
    /// What the file was when this was read, used as a **cache-buster** on the asset URL.
    ///
    /// The asset protocol serves a real HTTP-shaped response and WebKit caches it by URL, so
    /// an image an agent has just rewritten would keep showing the old pixels for as long as
    /// the tab lives. Appending the stamp to the query string makes a changed file a changed
    /// URL. `None` — a filesystem with no usable mtime — simply means no cache-buster, which
    /// is the same behaviour cide had before and not a failure worth reporting.
    pub stamp: Option<FileStamp>,
}
