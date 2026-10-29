//! Conservative allocation accounting for a project's indexes, including transient copies.
//!
//! Reservations belong to a generation, not its registry slot. A query holding an old
//! generation must keep its allocations charged after that project is closed. Charges are
//! deliberately not refunded on a deletion: arenas and append-only matchers still own that
//! capacity. A replacement generation releases it when its final reader goes away.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use cide_ipc::{IndexAllocation, IndexBudgetUsage, IndexLimit, IndexRefusal};

pub const MIB: u64 = 1024 * 1024;

#[derive(Debug)]
pub struct SharedBudget {
    used: AtomicU64,
    limit: AtomicU64,
}

impl SharedBudget {
    pub fn new(bytes: u64) -> Arc<Self> {
        Arc::new(Self {
            used: AtomicU64::new(0),
            limit: AtomicU64::new(bytes),
        })
    }

    pub fn set_limit(&self, bytes: u64) {
        self.limit.store(bytes, Ordering::Release);
    }

    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
pub struct Budget {
    shared: Arc<SharedBudget>,
    limit: u64,
    used: AtomicU64,
    cancelled: AtomicBool,
    refusal: Mutex<Option<IndexRefusal>>,
}

impl Budget {
    pub fn new(shared: Arc<SharedBudget>, limit: u64) -> Arc<Self> {
        Arc::new(Self {
            shared,
            limit,
            used: AtomicU64::new(0),
            cancelled: AtomicBool::new(false),
            refusal: Mutex::new(None),
        })
    }

    pub fn standalone() -> Arc<Self> {
        Self::new(SharedBudget::new(2048 * MIB), 512 * MIB)
    }

    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Acquire)
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.cancelled
    }
    pub fn reason(&self) -> Option<IndexLimit> {
        self.refusal.lock().unwrap().as_ref().map(|r| r.reason)
    }
    pub fn refusal(&self) -> Option<IndexRefusal> {
        self.refusal.lock().unwrap().clone()
    }
    pub fn usage(&self) -> IndexBudgetUsage {
        IndexBudgetUsage {
            project_bytes: self.used(),
            project_limit_bytes: self.limit,
            total_bytes: self.shared.used(),
            total_limit_bytes: self.shared.limit.load(Ordering::Acquire),
        }
    }
    pub fn stopped(&self) -> bool {
        self.cancelled() || self.reason().is_some()
    }

    pub fn reserve(&self, bytes: u64) -> bool {
        self.reserve_for(bytes, IndexAllocation::Overhead, None)
    }

    pub fn reserve_for(
        &self,
        bytes: u64,
        allocation: IndexAllocation,
        path: Option<&Path>,
    ) -> bool {
        if self.stopped() {
            return false;
        }
        if !take(&self.used, self.limit, bytes) {
            self.refuse(IndexLimit::ProjectBudget, bytes, allocation, path);
            return false;
        }
        if !take(
            &self.shared.used,
            self.shared.limit.load(Ordering::Acquire),
            bytes,
        ) {
            self.used.fetch_sub(bytes, Ordering::AcqRel);
            self.refuse(IndexLimit::TotalBudget, bytes, allocation, path);
            return false;
        }
        true
    }

    fn refuse(
        &self,
        reason: IndexLimit,
        bytes: u64,
        allocation: IndexAllocation,
        path: Option<&Path>,
    ) {
        // Competing workers may already have passed stopped(). Preserve the first refusal,
        // and allocate only one diagnostic path even when many subsequent attempts fail.
        self.refusal
            .lock()
            .unwrap()
            .get_or_insert_with(|| IndexRefusal {
                reason,
                allocation,
                path: path.map(|p| p.to_string_lossy().into_owned()),
                folder: path
                    .and_then(Path::parent)
                    .map(|p| p.to_string_lossy().into_owned()),
                requested_bytes: bytes,
                usage: self.usage(),
            });
    }

    /// Covers the node, hash table capacity, path copies, matcher UTF-32 column and watcher
    /// metadata. Eight times UTF-8 length bounds UTF-32 plus the retained UTF-8 copies.
    pub fn reserve_path(&self, path: &std::path::Path, relative: &str) -> bool {
        self.reserve_path_for(path, relative, IndexAllocation::Files)
    }

    pub fn reserve_path_for(
        &self,
        path: &Path,
        relative: &str,
        allocation: IndexAllocation,
    ) -> bool {
        self.reserve_for(Self::path_bytes(path, relative), allocation, Some(path))
    }

    pub fn path_bytes(path: &Path, relative: &str) -> u64 {
        1024 + 8 * (path.as_os_str().len() + relative.len()) as u64
    }
}

fn take(counter: &AtomicU64, limit: u64, bytes: u64) -> bool {
    counter
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
            used.checked_add(bytes).filter(|next| *next <= limit)
        })
        .is_ok()
}

impl Drop for Budget {
    fn drop(&mut self) {
        self.shared.used.fetch_sub(self.used(), Ordering::AcqRel);
    }
}

// Serial directory iterators have bounded working sets. At most four may run at once,
// including walks in different projects. Waiting is cancellable and holds no project lock.
fn workers() -> &'static (Mutex<usize>, Condvar) {
    static WORKERS: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new();
    WORKERS.get_or_init(|| (Mutex::new(0), Condvar::new()))
}

pub struct Worker;
impl Worker {
    pub fn acquire(budget: &Budget) -> Option<Self> {
        Self::acquire_until(&|| budget.stopped())
    }
    pub fn acquire_until(stopped: &dyn Fn() -> bool) -> Option<Self> {
        let (lock, wake) = workers();
        let mut count = lock.lock().unwrap();
        while *count >= 4 {
            if stopped() {
                return None;
            }
            count = wake
                .wait_timeout(count, Duration::from_millis(25))
                .unwrap()
                .0;
        }
        if stopped() {
            return None;
        }
        *count += 1;
        Some(Self)
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let (lock, wake) = workers();
        *lock.lock().unwrap() -= 1;
        wake.notify_one();
    }
}
