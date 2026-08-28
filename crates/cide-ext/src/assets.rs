//! The jail behind `cide-ext://`.
//!
//! The webview asks for `cide-ext://localhost/<marketplace>/<extension>/<path>` and gets bytes
//! back. That
//! is the one place in cide where a URL from the renderer chooses a file on disk, so it is the one
//! place a path-traversal bug would be a real one.
//!
//! # Three checks, and the last one is the one that works
//!
//! 1. The first two path segments are a marketplace and an extension id, and both must be
//!    [`crate::manifest::is_safe_segment`] — which forbids `.` and `/` outright, so no request can
//!    name a directory that is not an installed extension's.
//! 2. The path is resolved with [`crate::manifest::safe_relative`], which refuses `..`, an
//!    absolute path and a backslash **on the string**, before any filesystem call. That matters
//!    more than it looks: `Path::join` with an absolute right-hand side silently *discards* the
//!    left one, so `root.join("/etc/passwd")` is `/etc/passwd` and does not error.
//! 3. And then the resolved path is canonicalised and required to still be under the canonical
//!    root. This is the check that does not depend on having thought of every spelling: a symlink
//!    inside the installed tree pointing outward passes the first two and fails this one.
//!
//! Check 3 alone would very nearly do. All three are here because check 3 needs the file to
//! *exist* to canonicalise it, so a refusal from checks 1 and 2 is the difference between "no"
//! and "no, and here is which rule you broke" — and because `install.rs` already refuses to copy
//! a symlink, which makes check 3 a second lock on a door that should have no key cut for it.

use std::path::{Path, PathBuf};

use cide_ipc::ids::{ExtensionId, MarketplaceId};

use crate::manifest::{is_safe_segment, safe_relative};

/// The URI scheme extension assets are served under.
pub const SCHEME: &str = "cide-ext";

/// Why a request was refused. One sentence each, and each names the rule rather than the file:
/// a message that echoed the requested path back would be the one thing an attacker controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AssetError {
    #[error("the path does not start with `<marketplace>/<extension>/`")]
    BadHost,
    #[error("the path leaves the extension's directory")]
    Escapes,
    #[error("no such file in this extension")]
    NotFound,
}

/// Split a `cide-ext://` request path into the pair that names an extension, and the rest.
///
/// # Why the identity is in the path and not the host
///
/// `cide-ext://<marketplace>.<extension>/main.js` is the obvious spelling and it is not portable.
/// Tauri serves a custom scheme as `<scheme>://localhost/…` on macOS, iOS and Linux, and as
/// `http://<scheme>.localhost/…` on Windows and Android — so on the second pair the *host* is the
/// scheme's own name and an identity put there is simply gone, on the platforms this would not
/// have been tested on. In the path it is the same string everywhere.
///
/// `ui/src/ext/assetUrl.ts` is the other end of that split and states it in the same words;
/// `check:ext` drives it with one user agent per platform, because getting the *boundary* wrong —
/// macOS put on the Windows side — is a bug no Linux machine can see and every Mac hits at once.
///
/// Both segments must be [`is_safe_segment`], which forbids `.` and `/` outright, so no request
/// can name a directory that is not an installed extension's — the first of this module's three
/// checks.
pub fn split_path(path: &str) -> Result<(MarketplaceId, ExtensionId, String), AssetError> {
    let trimmed = path.trim_start_matches('/');
    let trimmed = trimmed.split(['?', '#']).next().unwrap_or(trimmed);
    let mut parts = trimmed.splitn(3, '/');
    let market = parts.next().unwrap_or_default();
    let ext = parts.next().unwrap_or_default();
    let rest = parts.next().unwrap_or_default();
    if !is_safe_segment(market) || !is_safe_segment(ext) {
        return Err(AssetError::BadHost);
    }
    if rest.is_empty() {
        return Err(AssetError::NotFound);
    }
    Ok((
        MarketplaceId(market.to_string()),
        ExtensionId(ext.to_string()),
        rest.to_string(),
    ))
}

/// Resolve a request against an installed extension's root.
pub fn resolve(root: &Path, path: &str) -> Result<PathBuf, AssetError> {
    let trimmed = path.trim_start_matches('/');
    let trimmed = trimmed.split(['?', '#']).next().unwrap_or(trimmed);
    if trimmed.is_empty() {
        return Err(AssetError::NotFound);
    }
    let candidate = safe_relative(root, trimmed).ok_or(AssetError::Escapes)?;
    let real = candidate.canonicalize().map_err(|_| AssetError::NotFound)?;
    let real_root = root.canonicalize().map_err(|_| AssetError::NotFound)?;
    if !real.starts_with(&real_root) {
        return Err(AssetError::Escapes);
    }
    if !real.is_file() {
        return Err(AssetError::NotFound);
    }
    Ok(real)
}

/// The `Content-Type` for a file this scheme serves.
///
/// A short, closed table. Anything not in it is `application/octet-stream` and **not** guessed
/// from the bytes: a worker script is fetched with an explicit type by the browser's module
/// loader, and everything else here is an asset a panel references. Sniffing would let a file
/// with the wrong extension be interpreted as something it is not.
#[must_use]
pub fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "webp" => "image/webp",
        "md" | "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicU64, Ordering};

    /// A scratch directory, from `std::env::temp_dir()` keyed by pid and a counter — the
    /// workspace's convention, and the reason this crate has no dev-dependencies.
    fn scratch(tag: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "cide-ext-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp dir");
        path
    }

    #[test]
    fn a_request_path_starts_with_a_pair_of_safe_segments() {
        let (market, ext, rest) = split_path("/cide-marketplace/sql/main.js").expect("split");
        assert_eq!(market.as_str(), "cide-marketplace");
        assert_eq!(ext.as_str(), "sql");
        assert_eq!(rest, "main.js");
        assert_eq!(
            split_path("/cide-marketplace/sql/lib/parse.js")
                .expect("nested")
                .2,
            "lib/parse.js",
            "everything after the pair is the file, slashes included"
        );
        assert_eq!(split_path("/sql/main.js").unwrap_err(), AssetError::BadHost);
        assert_eq!(
            split_path("/../etc/sql/x").unwrap_err(),
            AssetError::BadHost
        );
        assert_eq!(split_path("/M/sql/x").unwrap_err(), AssetError::BadHost);
        assert_eq!(
            split_path("/cide-marketplace/sql").unwrap_err(),
            AssetError::NotFound,
            "a pair with no file after it names a directory, and this scheme serves files"
        );
    }

    #[test]
    fn traversal_is_refused_before_the_filesystem_is_touched() {
        let root = scratch("jail");
        std::fs::write(root.join("main.js"), b"export {}").unwrap();
        assert!(resolve(&root, "/main.js").is_ok());
        for attempt in ["../../etc/passwd", "/etc/passwd", "a/../../../etc/passwd"] {
            let refused = resolve(&root, attempt).unwrap_err();
            assert!(
                matches!(refused, AssetError::Escapes | AssetError::NotFound),
                "{attempt} was not refused: {refused:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The check that does not depend on having thought of every spelling.
    ///
    /// `install.rs` refuses to copy a symlink, so this should be unreachable in production. It is
    /// asserted anyway, because "unreachable because another module is careful" is exactly the
    /// kind of guarantee that stops being true during a refactor of that other module.
    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_tree_is_refused() {
        let root = scratch("jail-link");
        let outside = scratch("jail-outside");
        std::fs::write(outside.join("secret"), b"no").unwrap();
        std::os::unix::fs::symlink(outside.join("secret"), root.join("link")).unwrap();
        assert_eq!(resolve(&root, "link").unwrap_err(), AssetError::Escapes);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
