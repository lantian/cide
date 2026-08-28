//! A scratch directory for tests, without a `tempfile` dependency.
//!
//! Public because the integration tests in `tests/` need it too, and `#[cfg(test)]` items
//! are not visible to them. It is a dozen lines and pulling a crate into the dependency
//! graph for that seemed the worse trade — the rest of this workspace builds its temporary
//! paths out of `std::env::temp_dir` for the same reason.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A directory that is removed when it goes out of scope.
#[derive(Debug)]
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // A failure here is a leaked temp directory, which is not worth failing a passing
        // test over — and panicking in a `Drop` during unwinding would abort the process.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Create an empty directory unique to this process, test and call.
///
/// **Canonicalised**, and that is not tidiness. `std::env::temp_dir()` is `$TMPDIR`, which on
/// macOS is under `/var` — a symlink to `/private/var` — so a fixture root spelled `/var/…`
/// names the same directory as the `/private/var/…` that anything resolving symlinks reports.
/// `git rev-parse --path-format=absolute` is one such thing, and `cide_fs::filter` compares
/// paths **textually on purpose** (see `ops.rs`: canonicalising would resolve a symlinked
/// project root into a path the user never typed). Those two decisions are individually right
/// and meet here: a test that builds a filter from git's answer and then asks it about a path
/// built from this root is comparing two spellings of one directory, and every containment
/// question answers `false`.
///
/// The failure that found this was a *positive control* — `nothing_under_git_objects…` asserts
/// that `.git/HEAD` is still admitted, precisely so "nothing under objects is admitted" cannot
/// pass by admitting nothing at all. Every assertion in that test about paths that must be
/// refused was passing vacuously. It reproduces on any host with
/// `TMPDIR=<a symlink to a real dir> cargo test -p cide-fs`.
pub fn scratch(tag: &str) -> Scratch {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("cide-fs-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("could not create a scratch directory");
    // After creation, because `canonicalize` needs the path to exist. Falling back to the
    // uncanonicalised path rather than panicking: on a host where this fails, the tests behave
    // as they did before it was added.
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    Scratch(path)
}
