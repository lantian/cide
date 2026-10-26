//! Poll only registered paths. Unlike notify's PollWatcher, registering a directory does
//! not recursively populate a second, unbudgeted file index. Directory timestamps detect
//! new children; the index's bounded reconciliation decides which ones to admit.
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime};

#[derive(PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
    dir: bool,
}
fn stamp(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    Some(Stamp {
        modified: m.modified().ok(),
        len: m.len(),
        dir: m.is_dir(),
    })
}

pub(crate) struct CatalogPoller {
    paths: Arc<Mutex<HashMap<PathBuf, Option<Stamp>>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Watcher for CatalogPoller {
    fn new<F: notify::EventHandler>(
        mut handler: F,
        config: notify::Config,
    ) -> notify::Result<Self> {
        let paths = Arc::new(Mutex::new(HashMap::<PathBuf, Option<Stamp>>::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::clone(&paths);
        let cancelled = Arc::clone(&stop);
        let interval = config.poll_interval().unwrap_or(Duration::from_secs(30));
        let thread = std::thread::spawn(move || {
            let mut next = std::time::Instant::now() + interval;
            while !cancelled.load(Ordering::Acquire) {
                std::thread::park_timeout(Duration::from_millis(25));
                if std::time::Instant::now() < next {
                    continue;
                }
                next = std::time::Instant::now() + interval;
                let mut paths = shared.lock().unwrap();
                for (path, old) in paths.iter_mut() {
                    if cancelled.load(Ordering::Acquire) {
                        break;
                    }
                    let new = stamp(path);
                    if *old != new {
                        let kind = match (&old, &new) {
                            (None, Some(_)) => EventKind::Create(notify::event::CreateKind::Any),
                            (Some(_), None) => EventKind::Remove(notify::event::RemoveKind::Any),
                            _ => EventKind::Modify(notify::event::ModifyKind::Any),
                        };
                        *old = new;
                        handler.handle_event(Ok(Event::new(kind).add_path(path.clone())));
                    }
                }
            }
        });
        Ok(Self {
            paths,
            stop,
            thread: Some(thread),
        })
    }
    fn watch(&mut self, path: &Path, _mode: RecursiveMode) -> notify::Result<()> {
        let mut paths = self.paths.lock().unwrap();
        paths
            .entry(path.to_path_buf())
            .or_insert_with(|| stamp(path));
        Ok(())
    }
    fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
        self.paths.lock().unwrap().remove(path);
        Ok(())
    }
    fn kind() -> notify::WatcherKind {
        notify::WatcherKind::PollWatcher
    }
}
impl Drop for CatalogPoller {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}
