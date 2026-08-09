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
pub fn scratch(tag: &str) -> Scratch {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("cide-fs-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("could not create a scratch directory");
    Scratch(path)
}
