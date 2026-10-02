//! Shared, generation-aware reads. Pending entries are never evicted or duplicated.
use parking_lot::{Condvar, Mutex};
use std::collections::HashMap;
use std::hash::Hash;
use std::time::Instant;

struct Entry<V> {
    generation: u64,
    running: bool,
    value: Option<Result<V, String>>,
    touched: Instant,
}

pub struct ReadCache<K, V> {
    capacity: usize,
    entries: Mutex<HashMap<K, Entry<V>>>,
    changed: Condvar,
}

impl<K, V> Default for ReadCache<K, V> {
    fn default() -> Self {
        Self {
            capacity: 64,
            entries: Mutex::new(HashMap::new()),
            changed: Condvar::new(),
        }
    }
}

impl<K: Eq + Hash + Clone, V: Clone> ReadCache<K, V> {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity,
            ..Self::default()
        }
    }

    pub fn retain(&self, keep: impl Fn(&K) -> bool) {
        self.entries
            .lock()
            .retain(|key, entry| entry.running || keep(key));
    }

    pub fn invalidate(&self, matches: impl Fn(&K) -> bool) {
        for (key, entry) in self.entries.lock().iter_mut() {
            if matches(key) {
                entry.generation += 1;
                entry.value = None;
            }
        }
    }

    pub fn get(&self, key: K, read: impl Fn() -> Result<V, String>) -> Result<V, String> {
        let mut entries = self.entries.lock();
        loop {
            let entry = entries.entry(key.clone()).or_insert_with(|| Entry {
                generation: 0,
                running: false,
                value: None,
                touched: Instant::now(),
            });
            entry.touched = Instant::now();
            if let Some(value) = &entry.value {
                tracing::debug!("openspec cache hit");
                return value.clone();
            }
            if entry.running {
                tracing::debug!("openspec shared pending read");
                self.changed.wait(&mut entries);
                continue;
            }
            entry.running = true;
            break;
        }
        let mut discarded = 0;
        loop {
            let generation = entries[&key].generation;
            drop(entries);
            let started = Instant::now();
            // Restore the pending flag even if a reader panics, so waiters cannot hang forever.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&read));
            entries = self.entries.lock();
            let entry = entries.get_mut(&key).expect("running entries are retained");
            let value = match result {
                Ok(value) => value,
                Err(payload) => {
                    entry.running = false;
                    self.changed.notify_all();
                    drop(entries);
                    std::panic::resume_unwind(payload);
                }
            };
            if entry.generation != generation {
                tracing::debug!("openspec discarded invalidated read");
                discarded += 1;
                if discarded >= 3 {
                    entry.running = false;
                    self.changed.notify_all();
                    return Err("OpenSpec files kept changing during the read. Retry once the current edits finish.".into());
                }
                continue;
            }
            entry.running = false;
            entry.value = Some(value.clone());
            while entries.len() > self.capacity {
                let oldest = entries
                    .iter()
                    .filter(|(k, e)| !e.running && *k != &key)
                    .min_by_key(|(_, e)| e.touched)
                    .map(|(k, _)| k.clone());
                let Some(oldest) = oldest else {
                    break;
                };
                entries.remove(&oldest);
            }
            self.changed.notify_all();
            tracing::debug!(
                ms = started.elapsed().as_millis() as u64,
                "openspec read finished"
            );
            return value;
        }
    }
}

/// Foreground requests go ahead of queued background reads. Each project owns one pool.
#[derive(Default)]
pub struct ReadPool {
    state: Mutex<(usize, usize)>,
    changed: Condvar,
}

impl ReadPool {
    pub fn run<T>(&self, foreground: bool, read: impl FnOnce() -> T) -> T {
        let mut state = self.state.lock();
        if foreground {
            state.1 += 1;
        }
        while state.0 >= 2 || (!foreground && state.1 > 0) {
            self.changed.wait(&mut state);
        }
        if foreground {
            state.1 -= 1;
        }
        state.0 += 1;
        drop(state);
        struct Release<'a>(&'a ReadPool);
        impl Drop for Release<'_> {
            fn drop(&mut self) {
                self.0.state.lock().0 -= 1;
                self.0.changed.notify_all();
            }
        }
        let _release = Release(self);
        read()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn continuous_edits_release_waiters_and_allow_a_later_retry() {
        let cache = ReadCache::default();
        assert!(
            cache
                .get("change", || {
                    cache.invalidate(|_| true);
                    Ok(0)
                })
                .is_err()
        );
        assert_eq!(cache.get("change", || Ok(1)), Ok(1));
    }

    #[test]
    fn foreground_jobs_start_before_queued_background_jobs() {
        let pool = ReadPool::default();
        // Reserve both slots, then release exactly one after both jobs have queued.
        pool.state.lock().0 = 2;
        let order = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            scope.spawn(|| pool.run(false, || order.lock().push("background")));
            scope.spawn(|| pool.run(true, || order.lock().push("foreground")));
            while pool.state.lock().1 == 0 {
                std::thread::yield_now();
            }
            pool.state.lock().0 = 1;
            pool.changed.notify_all();
        });
        assert_eq!(*order.lock(), ["foreground", "background"]);
        assert_eq!(pool.state.lock().0, 1);
    }

    #[test]
    fn concurrent_consumers_share_one_read() {
        let cache = Arc::new(ReadCache::default());
        let calls = AtomicUsize::new(0);
        let start = Barrier::new(8);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    start.wait();
                    assert_eq!(
                        cache.get("change", || {
                            calls.fetch_add(1, Ordering::SeqCst);
                            std::thread::sleep(std::time::Duration::from_millis(20));
                            Ok(42)
                        }),
                        Ok(42)
                    );
                });
            }
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn invalidation_during_read_retries_without_publishing_old_data() {
        let cache = ReadCache::default();
        let calls = AtomicUsize::new(0);
        assert_eq!(
            cache.get("change", || {
                let call = calls.fetch_add(1, Ordering::SeqCst);
                if call == 0 {
                    cache.invalidate(|_| true);
                }
                Ok(call)
            }),
            Ok(1)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(cache.get("change", || panic!("already cached")), Ok(1));
    }

    #[test]
    fn invalidation_is_targeted_and_cache_is_bounded() {
        let cache = ReadCache::default();
        for key in 0..100 {
            cache.get(key, || Ok(key)).unwrap();
        }
        assert_eq!(cache.entries.lock().len(), 64);
        cache.invalidate(|key| *key == 99);
        assert_eq!(cache.get(98, || panic!("unrelated")), Ok(98));
        assert_eq!(cache.get(99, || Ok(100)), Ok(100));
    }
}
