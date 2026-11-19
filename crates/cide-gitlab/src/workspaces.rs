//! Disposable review checkouts. A held OS lock is the authority for ownership, not a PID.
use crate::{Result, validate_relative};
use git2::{Oid, Repository};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

struct Checkout {
    parent: PathBuf,
    root: PathBuf,
    sha: String,
}
pub struct Workspaces {
    parent: PathBuf,
    _owner: File,
    entries: Mutex<HashMap<String, Checkout>>,
    stopped: Arc<AtomicBool>,
    cancelled: Mutex<HashMap<String, Arc<AtomicBool>>>,
}
fn io<T>(r: std::io::Result<T>) -> Result<T> {
    r.map_err(|e| format!("Review workspace: {e}"))
}
fn git<T>(r: std::result::Result<T, git2::Error>) -> Result<T> {
    r.map_err(|e| {
        format!(
            "Review workspace failed ({:?}/{:?}): {}",
            e.class(),
            e.code(),
            e.message()
        )
    })
}
/// Delete a checkout's tree even where something inside it made directories read-only.
///
/// # Why a plain `remove_dir_all` is not enough
///
/// A review checkout is a directory an agent stood in, and the reviewer brief lets it run
/// "builds or tests that do not write outside the checkout" — a codex reviewer's sandbox lets it
/// write nowhere else at all. On a Go MR the obedient way to run `go test` is therefore
/// `GOMODCACHE`/`GOPATH` pointed **inside** the checkout, and Go makes its module cache
/// read-only on purpose: 0555 directories, 0444 files (it is why `go clean -modcache` exists).
/// Unlinking an entry needs write permission on its *directory*, so `remove_dir_all` stopped at
/// the first one with EACCES — `Review workspace: Permission denied (os error 13)` on Close,
/// on some MRs and not others (2026-11, macOS: only the MRs whose reviewer had built). Bazel
/// output trees and Nix-copied files are the same shape.
///
/// So on a failure the tree is walked and every directory is given `u+rwx` — **before** it is
/// listed, since a 0000 directory cannot be read — and the delete is tried once more. Files are
/// left as they are: their own mode never decides whether they can be unlinked.
///
/// # Never through a symlink
///
/// The checkout is the MR author's content plus whatever the agent made, and a symlink in it
/// may point anywhere — at `~`, at the user's real module cache. `symlink_metadata` decides
/// what an entry *is*, so a link is never followed, never chmodded and never descended into;
/// `remove_dir_all` itself already unlinks a link rather than its target.
fn remove_tree(path: &Path) -> std::io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {}
    }
    #[cfg(unix)]
    {
        fn make_writable(dir: &Path) {
            use std::os::unix::fs::PermissionsExt;
            let Ok(meta) = fs::symlink_metadata(dir) else {
                return;
            };
            if !meta.is_dir() {
                return;
            }
            let mode = meta.permissions().mode();
            if mode & 0o700 != 0o700 {
                // Best effort: a directory owned by somebody else refuses, and the retry below
                // reports the error that is actually in the way.
                let _ = fs::set_permissions(dir, fs::Permissions::from_mode(mode | 0o700));
            }
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    make_writable(&entry.path());
                }
            }
        }
        make_writable(path);
    }
    match fs::remove_dir_all(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}
// Keep the ownership marker until every checkout is gone. A failed recursive
// deletion must remain discoverable by the next process's recovery sweep.
fn remove_owned(parent: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        if entry.file_name() == "owner.lock" {
            continue;
        }
        if entry.file_type()?.is_dir() {
            remove_tree(&entry.path())?;
        } else {
            fs::remove_file(entry.path())?;
        }
    }
    fs::remove_file(parent.join("owner.lock"))?;
    fs::remove_dir(parent)
}
impl Workspaces {
    pub fn new(base: PathBuf) -> Result<Self> {
        io(fs::create_dir_all(&base))?;
        // Only our UUID directories carrying an owner lock are candidates for recovery.
        for entry in io(fs::read_dir(&base))?.flatten() {
            if !entry.file_type().is_ok_and(|t| t.is_dir())
                || uuid::Uuid::parse_str(&entry.file_name().to_string_lossy()).is_err()
            {
                continue;
            }
            let path = entry.path();
            if let Ok(owner) = OpenOptions::new()
                .read(true)
                .write(true)
                .open(path.join("owner.lock"))
                && owner.try_lock().is_ok()
                && let Err(error) = remove_owned(&path)
            {
                tracing::warn!(%error,"Abandoned review workspace cleanup will be retried");
            }
        }
        let parent = base.join(uuid::Uuid::new_v4().to_string());
        io(fs::create_dir(&parent))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            io(fs::set_permissions(
                &parent,
                fs::Permissions::from_mode(0o700),
            ))?;
        }
        let owner = io(OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(parent.join("owner.lock")))?;
        io(owner.lock())?;
        Ok(Self {
            parent,
            _owner: owner,
            entries: Mutex::new(HashMap::new()),
            stopped: Arc::new(AtomicBool::new(false)),
            cancelled: Mutex::new(HashMap::new()),
        })
    }
    pub fn checkout(
        &self,
        id: &str,
        sha: &str,
        url: &str,
        token: &str,
        proxy: &cide_ipc::ProxySettings,
    ) -> Result<String> {
        let cancelled = self
            .cancelled
            .lock()
            .entry(id.into())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .clone();
        if cancelled.load(Ordering::Acquire) {
            return Err("This review has been closed".into());
        }
        let oid = git(Oid::from_str(sha))?;
        if sha.len() != 40 {
            return Err("Review checkout requires an exact commit SHA".into());
        }
        let key = format!("{id}:{sha}");
        let mut entries = self.entries.lock();
        if self.stopped.load(Ordering::Acquire) {
            return Err("Cide is closing".into());
        }
        if let Some(old) = entries.get(&key)
            && old.sha == sha
        {
            return Ok(old.root.to_string_lossy().into());
        }
        let parent = self.parent.join(uuid::Uuid::new_v4().to_string());
        io(fs::create_dir(&parent))?;
        let result = (|| {
            let repo = git(Repository::init_bare(parent.join("repository")))?;
            crate::fetch::fetch(
                repo.path(),
                sha,
                url,
                token,
                proxy,
                &self.stopped,
                &cancelled,
            )?;
            if self.stopped.load(Ordering::Acquire) || cancelled.load(Ordering::Acquire) {
                return Err("Review checkout cancelled".into());
            }
            let commit = git(repo.find_commit(oid))?;
            let branch = git(repo.branch("review", &commit, true))?;
            let mut options = git2::WorktreeAddOptions::new();
            options.reference(Some(branch.get()));
            let root = parent.join("source");
            git(repo.worktree("review", &root, Some(&options)))?;
            let tree = git(Repository::open(&root))?;
            git(tree.set_head_detached(oid))?;
            if self.stopped.load(Ordering::Acquire) || cancelled.load(Ordering::Acquire) {
                return Err("Review checkout cancelled".into());
            }
            Ok(Checkout {
                parent: parent.clone(),
                root,
                sha: sha.to_string(),
            })
        })();
        match result {
            Ok(new) => {
                let root = new.root.to_string_lossy().into_owned();
                entries.insert(key, new);
                Ok(root)
            }
            Err(e) => {
                let _ = remove_tree(&parent);
                Err(e)
            }
        }
    }
    /// Fetch one more commit into the repository behind an existing checkout of `head` — the
    /// MR base, so an agent standing in the checkout can `git diff <base> <head>`. A worktree
    /// shares its repository's objects, so nothing else has to move.
    ///
    /// The entries lock is **not** held across the fetch, unlike `checkout`'s: that one guards
    /// a directory being created, and this only adds objects to one that already exists, so a
    /// slow network must not stall every other review's source view behind it.
    pub fn fetch_into(
        &self,
        id: &str,
        head: &str,
        extra: &str,
        url: &str,
        token: &str,
        proxy: &cide_ipc::ProxySettings,
    ) -> Result<()> {
        let oid = git(Oid::from_str(extra))?;
        if extra.len() != 40 {
            return Err("Review fetch requires an exact commit SHA".into());
        }
        let repository = self
            .entries
            .lock()
            .get(&format!("{id}:{head}"))
            .map(|e| e.parent.join("repository"))
            .ok_or("Prepare the review source workspace first")?;
        if git(Repository::open_bare(&repository))?
            .find_commit(oid)
            .is_ok()
        {
            return Ok(());
        }
        let cancelled = self
            .cancelled
            .lock()
            .entry(id.into())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .clone();
        crate::fetch::fetch(
            &repository,
            extra,
            url,
            token,
            proxy,
            &self.stopped,
            &cancelled,
        )
    }
    pub fn versions(&self, id: &str) -> Vec<String> {
        let prefix = format!("{id}:");
        self.entries
            .lock()
            .keys()
            .filter_map(|key| key.strip_prefix(&prefix).map(str::to_string))
            .collect()
    }
    pub fn root(&self, id: &str, sha: &str) -> Option<PathBuf> {
        self.entries
            .lock()
            .get(&format!("{id}:{sha}"))
            .map(|e| e.root.clone())
    }
    pub fn reopen(&self, id: &str) {
        self.cancelled.lock().remove(id);
    }
    pub fn cancel(&self, id: &str) {
        self.cancelled
            .lock()
            .entry(id.into())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .store(true, Ordering::Release);
    }
    /// Forget a review's checkouts and delete them.
    ///
    /// **Infallible, and the entry goes whether or not its directory did.** This used to return
    /// the first deletion error, and `GitLabRequest::Close` propagated it before forgetting the
    /// review — by which point `cmd::gitlab` had already stopped the review's runs and closed
    /// its tabs. The review then could not be closed at all, by any number of retries, over a
    /// scratch directory in cide's cache. A directory [`remove_tree`] still cannot remove stays
    /// under `self.parent`, whose owner lock keeps it ours: `shutdown` retries it at quit and the
    /// next start's sweep after that, and a reopened MR gets a fresh UUID directory, so the
    /// leftover never collides with anything.
    pub fn close(&self, id: &str) {
        self.cancel(id);
        let mut entries = self.entries.lock();
        let keys: Vec<_> = entries
            .keys()
            .filter(|key| key.starts_with(&format!("{id}:")))
            .cloned()
            .collect();
        for key in keys {
            if let Some(entry) = entries.remove(&key)
                && let Err(error) = remove_tree(&entry.parent)
            {
                tracing::warn!(
                    %error,
                    path = %entry.parent.display(),
                    "a closed review's checkout was left for the cleanup at quit"
                );
            }
        }
    }
    pub fn shutdown(&self) {
        self.stopped.store(true, Ordering::Release);
        let mut entries = self.entries.lock();
        match remove_owned(&self.parent) {
            Ok(()) => entries.clear(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => entries.clear(),
            Err(error) => {
                tracing::warn!(%error,"Review workspace cleanup will be retried at startup")
            }
        }
    }
    pub fn files(&self, id: &str, sha: &str) -> Result<Vec<String>> {
        let entries = self.entries.lock();
        let entry = entries
            .get(&format!("{id}:{sha}"))
            .ok_or("Prepare the review source workspace first")?;
        let repo = git(Repository::open(&entry.root))?;
        let index = git(repo.index())?;
        Ok(index
            .iter()
            .filter_map(|entry| String::from_utf8(entry.path).ok())
            .collect())
    }
    pub fn file(&self, id: &str, sha: &str, path: &str) -> Result<String> {
        validate_relative(path)?;
        let entries = self.entries.lock();
        let entry = entries
            .get(&format!("{id}:{sha}"))
            .ok_or("Prepare the review source workspace first")?;
        // Read the immutable tree blob, never a symlink in the checkout.
        let repo = git(Repository::open(&entry.root))?;
        let tree = git(git(repo.head())?.peel_to_tree())?;
        let object = git(tree.get_path(Path::new(path)))?;
        if object.filemode() == 0o120000 {
            return Err("This source entry is a symbolic link".into());
        }
        let blob = git(repo.find_blob(object.id()))?;
        if blob.is_binary() {
            return Err("This file is binary; open it in GitLab".into());
        }
        if blob.size() > 8 * 1024 * 1024 {
            return Err("Source file exceeds the 8 MiB viewer limit".into());
        }
        String::from_utf8(blob.content().to_vec()).map_err(|_| "This file is binary".into())
    }
}
impl Drop for Workspaces {
    fn drop(&mut self) {
        self.shutdown();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    /// The shape `go test` leaves when an agent points `GOMODCACHE` inside the checkout: 0555
    /// directories holding 0444 files, plus a 0000 directory, which cannot even be listed until
    /// its mode is changed. Answers the deepest directory.
    #[cfg(unix)]
    fn go_module_cache(at: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let module = at.join("pkg/mod/example.com/m@v1.0.0");
        fs::create_dir_all(&module).unwrap();
        fs::write(module.join("go.mod"), "module example.com/m\n").unwrap();
        let sealed = at.join("pkg/mod/cache/sealed");
        fs::create_dir_all(&sealed).unwrap();
        fs::write(sealed.join("lock"), "").unwrap();
        fs::set_permissions(module.join("go.mod"), fs::Permissions::from_mode(0o444)).unwrap();
        fs::set_permissions(&sealed, fs::Permissions::from_mode(0o000)).unwrap();
        for dir in [
            module.clone(),
            at.join("pkg/mod/example.com"),
            at.join("pkg/mod/cache"),
            at.join("pkg/mod"),
        ] {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o555)).unwrap();
        }
        module
    }
    /// Whether permission bits bind this process at all. Under root (a container CI) they do
    /// not, and the "a plain delete fails here" precondition would be false for that reason only.
    #[cfg(unix)]
    fn permissions_bind() -> bool {
        unsafe { libc::geteuid() != 0 }
    }
    #[test]
    fn cleanup_respects_live_owners_and_recovers_crashes() {
        let base = std::env::temp_dir().join(format!("cide-review-test-{}", uuid::Uuid::new_v4()));
        let first = Workspaces::new(base.clone()).unwrap();
        let live = first.parent.clone();
        let abandoned = base.join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&abandoned).unwrap();
        File::create(abandoned.join("owner.lock")).unwrap();
        // A crashed process's checkout holding a read-only module cache: the sweep used to
        // stop at it with EACCES and leave it — and every later start did the same, for good.
        #[cfg(unix)]
        go_module_cache(&abandoned.join("checkout/source/.gopath"));
        let unrelated = base.join("user-worktree");
        fs::create_dir(&unrelated).unwrap();
        let second = Workspaces::new(base.clone()).unwrap();
        assert!(live.exists());
        assert!(!abandoned.exists());
        assert!(unrelated.exists());
        // And the same at quit, for this process's own checkouts.
        #[cfg(unix)]
        go_module_cache(&live.join("checkout/source/.gopath"));
        first.shutdown();
        assert!(!live.exists());
        first.shutdown();
        drop(second);
        fs::remove_dir_all(base).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn a_read_only_tree_in_a_checkout_does_not_stop_its_review_closing() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("cide-review-ro-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        let origin = Repository::init(base.join("origin")).unwrap();
        fs::write(base.join("origin/main.go"), "package main\n").unwrap();
        let mut index = origin.index().unwrap();
        index.add_path(Path::new("main.go")).unwrap();
        index.write().unwrap();
        let tree = origin.find_tree(index.write_tree().unwrap()).unwrap();
        let signature = git2::Signature::now("Test", "test@example.invalid").unwrap();
        let sha = origin
            .commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])
            .unwrap()
            .to_string();
        let url = format!("file://{}", base.join("origin").display());
        let workspaces = Workspaces::new(base.join("owned")).unwrap();
        let root = PathBuf::from(
            workspaces
                .checkout(
                    "review",
                    &sha,
                    &url,
                    "",
                    &cide_ipc::ProxySettings::default(),
                )
                .unwrap(),
        );
        // A directory outside the checkout that the agent linked to — the user's own module
        // cache, say. Read-only too, so a deleter that followed the link would chmod it.
        let outside = base.join("users-own-cache");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), "theirs\n").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("linked-cache")).unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o555)).unwrap();
        let module = go_module_cache(&root.join(".gopath"));
        if permissions_bind() {
            assert_eq!(
                fs::remove_file(module.join("go.mod")).unwrap_err().kind(),
                std::io::ErrorKind::PermissionDenied,
                "the fixture must reproduce the failure: a plain delete is refused in it"
            );
        }
        workspaces.close("review");
        assert!(!root.exists(), "the checkout must be gone");
        assert_eq!(
            fs::read_dir(&workspaces.parent).unwrap().count(),
            1,
            "only the owner lock is left"
        );
        assert!(workspaces.versions("review").is_empty());
        assert_eq!(
            fs::read_to_string(outside.join("keep")).unwrap(),
            "theirs\n",
            "a symlink's target is never deleted"
        );
        assert_eq!(
            fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
            0o555,
            "a symlink's target is never chmodded"
        );
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).unwrap();
        workspaces.shutdown();
        drop(workspaces);
        drop(tree);
        drop(origin);
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn immutable_checkout_closes_and_can_be_reopened() {
        let base =
            std::env::temp_dir().join(format!("cide-review-checkout-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        let origin = Repository::init(base.join("origin")).unwrap();
        fs::write(base.join("origin/main.go"), "package main\n").unwrap();
        let mut index = origin.index().unwrap();
        index.add_path(Path::new("main.go")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = origin.find_tree(tree_id).unwrap();
        let signature = git2::Signature::now("Test", "test@example.invalid").unwrap();
        let sha = origin
            .commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])
            .unwrap()
            .to_string();
        let workspaces = Workspaces::new(base.join("owned")).unwrap();
        let url = format!("file://{}", base.join("origin").display());
        let failure = workspaces
            .checkout(
                "review",
                &"0".repeat(40),
                &url,
                "",
                &cide_ipc::ProxySettings::default(),
            )
            .unwrap_err();
        assert!(failure.contains("Review fetch failed"), "{failure}");
        assert_eq!(
            fs::read_dir(&workspaces.parent).unwrap().count(),
            1,
            "a failed fetch must leave only the owner lock, with no partial repository"
        );
        let root = workspaces
            .checkout(
                "review",
                &sha,
                &url,
                "",
                &cide_ipc::ProxySettings::default(),
            )
            .unwrap();
        assert_eq!(workspaces.files("review", &sha).unwrap(), vec!["main.go"]);
        assert_eq!(
            workspaces.file("review", &sha, "main.go").unwrap(),
            "package main\n"
        );
        assert!(Repository::open(&root).unwrap().head_detached().unwrap());
        assert!(workspaces.file("review", &sha, "../main.go").is_err());
        fs::write(base.join("origin/main.go"), "package revised\n").unwrap();
        index.add_path(Path::new("main.go")).unwrap();
        index.write().unwrap();
        let revised_tree = origin.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = origin.head().unwrap().peel_to_commit().unwrap();
        let revised_sha = origin
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                "revised",
                &revised_tree,
                &[&parent],
            )
            .unwrap()
            .to_string();
        let revised_root = workspaces
            .checkout(
                "review",
                &revised_sha,
                &url,
                "",
                &cide_ipc::ProxySettings::default(),
            )
            .unwrap();
        assert_ne!(root, revised_root);
        assert_eq!(
            workspaces.file("review", &sha, "main.go").unwrap(),
            "package main\n"
        );
        assert_eq!(
            workspaces.file("review", &revised_sha, "main.go").unwrap(),
            "package revised\n"
        );
        assert_eq!(workspaces.versions("review").len(), 2);
        workspaces.close("review");
        assert!(!Path::new(&root).exists());
        assert!(!Path::new(&revised_root).exists());
        assert!(base.join("origin/main.go").exists());
        assert!(
            workspaces
                .checkout(
                    "review",
                    &sha,
                    &url,
                    "",
                    &cide_ipc::ProxySettings::default()
                )
                .is_err()
        );
        workspaces.reopen("review");
        assert!(
            workspaces
                .checkout(
                    "review",
                    &sha,
                    &url,
                    "",
                    &cide_ipc::ProxySettings::default()
                )
                .is_ok()
        );
        workspaces.shutdown();
        drop(workspaces);
        drop(tree);
        drop(revised_tree);
        drop(parent);
        drop(origin);
        fs::remove_dir_all(base).unwrap();
    }
}
