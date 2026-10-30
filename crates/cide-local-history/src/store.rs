//! One project's store: the tables in memory, and the two files they are replayed from.
//!
//! Owns no thread and no timer, as `cide-tasks` does not: `cide-app` runs one worker per store,
//! and every write here happens on it. That is what lets the methods be plain `&mut self` with no
//! story about a reconcile racing a compaction: they are chunks on one thread.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use cide_ipc::local_history::{VersionKind, VersionSource};
use serde::{Deserialize, Serialize};

use crate::log::{self, BlobLine, Header, Line, RootLine, RowLine, StampLine, Writer};
use crate::pack::{self, BlobHandle, BlobRef, FILE_MODE, Packs};
use crate::retention::{self, Fact, Policy};
use crate::{Error, Hash, Result};

const LOCK: &str = "lock";
const PACKS: &str = "packs";

/// Stamp-only lines worth folding away even when nothing else needs compacting.
const STALE_LINES: u64 = 10_000;

/// A project root, as a small number in every row. See [`Store::ensure_root`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RootId(pub u16);

/// Whether a file might have changed, without reading it.
///
/// Private to this feature and stricter than `cide_ipc::FileStamp`, which is mtime and length.
/// That is the right trade for an editor's conflict bar and the wrong one for a store whose job
/// is to not miss a change: `tar` and `cp -p` restore old mtimes, and a same-length edit within
/// one timestamp tick moves neither field. `ctime` and the inode catch the first two, and git's
/// racy rule catches the third in the worker. Compared with `!=`, never `<`: a stamp going
/// backwards is a change as well.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Stamp {
    #[serde(rename = "m")]
    pub mtime_ns: i64,
    #[serde(rename = "c")]
    pub ctime_ns: i64,
    #[serde(rename = "i")]
    pub inode: u64,
    #[serde(rename = "l")]
    pub len: u64,
}

impl Stamp {
    pub fn of(meta: &fs::Metadata) -> Self {
        Self {
            mtime_ns: meta
                .mtime()
                .saturating_mul(1_000_000_000)
                .saturating_add(meta.mtime_nsec()),
            ctime_ns: meta
                .ctime()
                .saturating_mul(1_000_000_000)
                .saturating_add(meta.ctime_nsec()),
            inode: meta.ino(),
            len: meta.len(),
        }
    }
}

/// One thing to record. See [`Store::commit`].
#[derive(Debug, Clone)]
pub struct Entry {
    pub root: RootId,
    pub rel: String,
    pub kind: VersionKind,
    pub source: VersionSource,
    /// The file's bytes. `None` for a deletion, a marker, or an [`Self::omit`]ted file.
    pub content: Option<Vec<u8>>,
    pub size: u64,
    pub mode: u32,
    pub stamp: Option<Stamp>,
    /// The other end of a move, on either of the two rows a move writes.
    pub moved: Option<String>,
    pub label: Option<String>,
    pub omitted: bool,
}

impl Entry {
    pub fn new(
        root: RootId,
        rel: impl Into<String>,
        kind: VersionKind,
        source: VersionSource,
    ) -> Self {
        Self {
            root,
            rel: rel.into(),
            kind,
            source,
            content: None,
            size: 0,
            mode: 0,
            stamp: None,
            moved: None,
            label: None,
            omitted: false,
        }
    }

    pub fn content(mut self, bytes: Vec<u8>, mode: u32, stamp: Option<Stamp>) -> Self {
        self.size = bytes.len() as u64;
        self.content = Some(bytes);
        self.mode = mode;
        self.stamp = stamp;
        self
    }

    /// The file changed and is too large to keep. A row anyway, because a timeline that goes
    /// silent over a file is a lie, and because without one the file would look brand new to a
    /// folder restore. See [`VersionKind::Baseline`].
    pub fn omit(mut self, size: u64, mode: u32, stamp: Option<Stamp>) -> Self {
        self.size = size;
        self.mode = mode;
        self.stamp = stamp;
        self.omitted = true;
        self
    }

    pub fn moved(mut self, other: impl Into<String>) -> Self {
        self.moved = Some(other.into());
        self
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// What a path looks like right now, as far as the store knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Head {
    pub id: u64,
    pub ts: u64,
    pub kind: VersionKind,
    pub hash: Option<Hash>,
    pub stamp: Option<Stamp>,
    pub omitted: bool,
}

/// One row, for a reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub id: u64,
    pub ts: u64,
    pub group: u64,
    pub root: RootId,
    pub rel: String,
    pub kind: VersionKind,
    pub source: VersionSource,
    pub hash: Option<Hash>,
    pub size: u64,
    pub mode: u32,
    pub moved: Option<String>,
    pub label: Option<String>,
    pub omitted: bool,
}

/// What a path should become, in a folder restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetState {
    /// This version's bytes.
    Content(u64),
    /// It did not exist at that moment.
    Absent,
    /// The store cannot say, so the file is left alone.
    Unknown,
}

/// One path of a folder restore. See [`Store::plan_at`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    pub rel: String,
    pub target: TargetState,
    /// The store's newest row already says this, so there is probably nothing to do.
    pub unchanged: bool,
}

/// What [`Store::commit`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Committed {
    /// The group the rows landed in, or `None` when every entry was a duplicate.
    pub group: Option<u64>,
    pub rows: usize,
}

/// One capture burst in a folder's or project's timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupSummary {
    pub group: u64,
    pub ts: u64,
    pub source: VersionSource,
    /// Rows in this group under the scope that was asked about.
    pub files: u32,
    pub label: Option<String>,
    /// Every row is a [`VersionKind::Baseline`]: one "First snapshot" line rather than 100,000.
    pub snapshot: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    pub rows: usize,
    pub paths: usize,
    /// Stored bytes rows still refer to.
    pub live_bytes: u64,
    /// Bytes on disk in packs, which retention reclaims a pack at a time.
    pub disk_bytes: u64,
    pub log_bytes: u64,
    /// Log lines that were complete and unreadable. Nonzero means something damaged the log.
    pub skipped_lines: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Compacted {
    pub dropped_rows: usize,
    pub rewritten_packs: usize,
    pub removed_packs: usize,
}

/// A row as held in memory. An `Option<u32>` for the blob rather than the [`Version`]'s strings:
/// the store may hold a million of these, and everything variable-length is interned.
#[derive(Debug, Clone)]
struct Row {
    id: u64,
    ts: u64,
    group: u64,
    path: u32,
    kind: VersionKind,
    source: VersionSource,
    omitted: bool,
    blob: Option<u32>,
    size: u64,
    mode: u32,
    moved: Option<u32>,
    label: Option<u32>,
}

#[derive(Debug, Clone)]
struct PathEntry {
    root: u16,
    rel: Box<str>,
    /// Indices into `Tables::rows`, oldest first.
    rows: Vec<u32>,
    stamp: Option<Stamp>,
}

/// Everything derived from the log. A struct of its own so compaction can build a replacement
/// and swap it in only after the new log is durable.
#[derive(Debug, Default)]
struct Tables {
    /// Canonical root paths, indexed by [`RootId`].
    roots: Vec<String>,
    paths: Vec<PathEntry>,
    path_ids: HashMap<(u16, Box<str>), u32>,
    /// In id order, always: appends are monotonic and compaction preserves order.
    rows: Vec<Row>,
    blobs: Vec<BlobRef>,
    by_hash: HashMap<Hash, u32>,
    notes: Vec<Box<str>>,
}

impl Tables {
    fn path(&mut self, root: u16, rel: &str) -> u32 {
        if let Some(&id) = self.path_ids.get(&(root, Box::from(rel))) {
            return id;
        }
        let id = self.paths.len() as u32;
        let rel: Box<str> = Box::from(rel);
        self.paths.push(PathEntry {
            root,
            rel: rel.clone(),
            rows: Vec::new(),
            stamp: None,
        });
        self.path_ids.insert((root, rel), id);
        id
    }

    fn path_of(&self, root: u16, rel: &str) -> Option<u32> {
        self.path_ids.get(&(root, Box::from(rel))).copied()
    }

    fn blob(&mut self, blob: BlobRef) -> u32 {
        *self.by_hash.entry(blob.hash).or_insert_with(|| {
            self.blobs.push(blob);
            (self.blobs.len() - 1) as u32
        })
    }

    fn note(&mut self, text: Option<&str>) -> Option<u32> {
        text.map(|text| {
            self.notes.push(Box::from(text));
            (self.notes.len() - 1) as u32
        })
    }

    fn push_row(&mut self, line: RowLine) {
        let path = self.path(line.root, &line.rel);
        let blob = line
            .blob
            .as_ref()
            .and_then(BlobLine::blob)
            .map(|b| self.blob(b));
        let moved = self.note(line.moved.as_deref());
        let label = self.note(line.label.as_deref());
        let index = self.rows.len() as u32;
        self.rows.push(Row {
            id: line.id,
            ts: line.ts,
            group: line.group,
            path,
            kind: line.kind,
            source: line.source,
            omitted: line.omitted,
            blob,
            size: line.size,
            mode: line.mode,
            moved,
            label,
        });
        let entry = &mut self.paths[path as usize];
        entry.rows.push(index);
        // The marker and labels sit on the pseudo-path, which has no file behind it.
        if !matches!(line.kind, VersionKind::Label | VersionKind::BaselineDone) {
            entry.stamp = line.stamp;
        }
    }

    fn version(&self, row: &Row) -> Version {
        let path = &self.paths[row.path as usize];
        Version {
            id: row.id,
            ts: row.ts,
            group: row.group,
            root: RootId(path.root),
            rel: path.rel.to_string(),
            kind: row.kind,
            source: row.source,
            hash: row.blob.map(|b| self.blobs[b as usize].hash),
            size: row.size,
            mode: row.mode,
            moved: row.moved.map(|n| self.notes[n as usize].to_string()),
            label: row.label.map(|n| self.notes[n as usize].to_string()),
            omitted: row.omitted,
        }
    }

    fn row_by_id(&self, id: u64) -> Option<&Row> {
        self.rows
            .binary_search_by_key(&id, |row| row.id)
            .ok()
            .map(|i| &self.rows[i])
    }

    fn head(&self, path: &PathEntry) -> Option<Head> {
        let row = &self.rows[*path.rows.last()? as usize];
        Some(Head {
            id: row.id,
            ts: row.ts,
            kind: row.kind,
            hash: row.blob.map(|b| self.blobs[b as usize].hash),
            stamp: path.stamp,
            omitted: row.omitted,
        })
    }

    fn line_of(&self, row: &Row, stamp: Option<Stamp>) -> RowLine {
        let path = &self.paths[row.path as usize];
        RowLine {
            id: row.id,
            ts: row.ts,
            group: row.group,
            root: path.root,
            rel: path.rel.to_string(),
            kind: row.kind,
            source: row.source,
            blob: row.blob.map(|b| BlobLine::of(&self.blobs[b as usize])),
            size: row.size,
            mode: row.mode,
            moved: row.moved.map(|n| self.notes[n as usize].to_string()),
            label: row.label.map(|n| self.notes[n as usize].to_string()),
            omitted: row.omitted,
            stamp,
        }
    }

    /// The tables holding only the rows `keep` says to, with every id renumbered.
    fn retain(&self, keep: &[bool], moved: &HashMap<u32, BlobRef>) -> Self {
        let mut next = Self {
            roots: self.roots.clone(),
            ..Self::default()
        };
        for (index, row) in self.rows.iter().enumerate() {
            if !keep[index] {
                continue;
            }
            let source = &self.paths[row.path as usize];
            let path = next.path(source.root, &source.rel);
            let blob = row.blob.map(|b| {
                let blob = moved.get(&b).copied().unwrap_or(self.blobs[b as usize]);
                next.blob(blob)
            });
            let moved_note = next.note(row.moved.map(|n| &*self.notes[n as usize]));
            let label = next.note(row.label.map(|n| &*self.notes[n as usize]));
            let index = next.rows.len() as u32;
            next.rows.push(Row {
                path,
                blob,
                moved: moved_note,
                label,
                ..row.clone()
            });
            next.paths[path as usize].rows.push(index);
            next.paths[path as usize].stamp = source.stamp;
        }
        next
    }
}

/// The directory a store lives in need not exist. Check with this first, because
/// [`Store::open`] creates it, and a project with capture off must not get one.
pub fn exists(dir: &Path) -> bool {
    dir.join(log::LOG).exists()
}

#[derive(Debug)]
pub struct Store {
    dir: PathBuf,
    /// Held for the store's life. `None` means this process only reads.
    ///
    /// The lock exists because two writers on one store are reachable and not hypothetical: a
    /// symlinked path and its target are two projects with one key, `remove_root` rekeys a
    /// project onto another's store, and `CIDE_ALLOW_SECOND_INSTANCE` is a documented escape
    /// hatch. Two append handles would interleave rows with offsets computed from a stale length.
    ///
    /// `flock` semantics: released by the kernel when the process dies, so a crash leaves
    /// nothing stale, which is why this is a lock and not a pidfile.
    _lock: Option<File>,
    log: Option<Writer>,
    packs: Packs,
    tables: Tables,
    next_id: u64,
    /// The newest row retention has ever dropped. See [`Self::plan_at`].
    trimmed_through: u64,
    skipped_lines: u32,
    /// Stamp-only lines since the log was last rewritten.
    stale_lines: u64,
}

impl Store {
    /// Open, creating the directory if needed. Never fails over the log's *contents*: a torn
    /// tail is cut and a bad line is skipped, because a loader that refuses leaves the user with
    /// no history at all over damage to one row of it.
    pub fn open(dir: &Path) -> Result<Self> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir.join(PACKS))?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(FILE_MODE)
            .open(dir.join(LOCK))?;
        let writable = match lock.try_lock() {
            Ok(()) => true,
            Err(fs::TryLockError::WouldBlock) => false,
            Err(fs::TryLockError::Error(e)) => return Err(e.into()),
        };

        let log_path = dir.join(log::LOG);
        let mut tables = Tables::default();
        let mut header = None;
        let mut max_id = 0;
        let mut newer_format = false;
        let replayed = log::replay(&log_path, |line| match line {
            Line::Header(h) => {
                newer_format |= h.format != log::FORMAT;
                header = Some(h);
            }
            _ if newer_format => {}
            Line::Root(root) => {
                let at = root.root as usize;
                if tables.roots.len() <= at {
                    tables.roots.resize(at + 1, String::new());
                }
                tables.roots[at] = root.path;
            }
            Line::Row(row) => {
                max_id = max_id.max(row.id);
                tables.push_row(*row);
            }
            Line::Stamp(line) => {
                if let Some(path) = tables.path_of(line.root, &line.rel) {
                    tables.paths[path as usize].stamp = Some(line.stamp);
                }
            }
        })?;
        let writable = writable && !newer_format;
        let next_id = header.map_or(1, |h| h.next_id).max(max_id + 1);

        let packs = Packs::open(&dir.join(PACKS), writable)?;
        let log = if writable {
            log::remove_temp(dir);
            let mut writer = Writer::open(&log_path, replayed.good_len)?;
            if header.is_none() {
                // The first open, or a log torn inside its first line. Replay works from a header
                // anywhere in the file, and `next_id` above already accounts for any rows, so one
                // written now is correct wherever it lands.
                let mut out = Vec::new();
                log::push(
                    &mut out,
                    &Header {
                        format: log::FORMAT,
                        next_id,
                        trimmed_through: 0,
                    },
                )?;
                writer.append(&out)?;
            }
            Some(writer)
        } else {
            None
        };

        let mut store = Self {
            dir: dir.to_path_buf(),
            _lock: writable.then_some(lock),
            log,
            packs,
            tables,
            next_id,
            trimmed_through: header.map_or(0, |h| h.trimmed_through),
            skipped_lines: replayed.skipped,
            stale_lines: 0,
        };
        if writable {
            store.remove_unreferenced_packs()?;
        }
        Ok(store)
    }

    pub fn is_writable(&self) -> bool {
        self.log.is_some()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Packs no row points at: left by a compaction that crashed after its log was durable.
    fn remove_unreferenced_packs(&mut self) -> Result<usize> {
        let live: HashSet<u32> = self.tables.blobs.iter().map(|b| b.pack).collect();
        let tail = self.packs.tail();
        let dead: Vec<u32> = self
            .packs
            .sizes()
            .map(|(id, _)| id)
            .filter(|id| !live.contains(id) && Some(*id) != tail)
            .collect();
        for &id in &dead {
            self.packs.remove(id)?;
        }
        Ok(dead.len())
    }

    /// The id for a canonical root path, assigning one the first time it is seen.
    ///
    /// A number rather than the path in every row for size, and because of what the key is:
    /// `Root.path` is not canonical and the store key is, so a row holding a path as spelled
    /// would split one file's history in two.
    pub fn ensure_root(&mut self, canonical: &str) -> Result<RootId> {
        if let Some(id) = self.root_id(canonical) {
            return Ok(id);
        }
        let log = self.log.as_mut().ok_or(Error::ReadOnly)?;
        let root = self.tables.roots.len() as u16;
        let mut out = Vec::new();
        log::push(
            &mut out,
            &RootLine {
                root,
                path: canonical.to_owned(),
            },
        )?;
        log.append(&out)?;
        self.tables.roots.push(canonical.to_owned());
        Ok(RootId(root))
    }

    pub fn root_id(&self, canonical: &str) -> Option<RootId> {
        let at = self
            .tables
            .roots
            .iter()
            .position(|root| root == canonical)?;
        Some(RootId(at as u16))
    }

    pub fn roots(&self) -> &[String] {
        &self.tables.roots
    }

    pub fn head(&self, root: RootId, rel: &str) -> Option<Head> {
        let path = self.tables.path_of(root.0, rel)?;
        self.tables.head(&self.tables.paths[path as usize])
    }

    /// Whether this root's first scan has reached the disk.
    pub fn baseline_done(&self, root: RootId) -> bool {
        self.tables.path_of(root.0, "").is_some_and(|path| {
            self.tables.paths[path as usize]
                .rows
                .iter()
                .any(|&i| self.tables.rows[i as usize].kind == VersionKind::BaselineDone)
        })
    }

    /// Every file a root has a head for, for the reconcile that compares them with the disk.
    pub fn heads(&self, root: RootId) -> Vec<(String, Head)> {
        self.tables
            .paths
            .iter()
            .filter(|path| path.root == root.0 && !path.rel.is_empty())
            .filter_map(|path| Some((path.rel.to_string(), self.tables.head(path)?)))
            .collect()
    }

    /// Record a batch as one group.
    ///
    /// **Blobs, then their fsync, then rows, then theirs.** A row is the only thing that makes a
    /// blob reachable, so if the order were ever reversed a crash could leave a durable row
    /// naming bytes that never arrived, which is a version that looks restorable and is not.
    /// This way a crash leaves bytes nothing refers to, and the next reconcile recaptures.
    ///
    /// **Duplicates are dropped here**, compared with the path's head and only its head. That
    /// is what handles the double report for cide's own saves, which arrive once from the save
    /// and again from the watcher, with no suppression timer to get wrong. Comparing with the
    /// head rather than with every version matters: going A → B → A is three real versions,
    /// and `git checkout` back to a branch must show as a change.
    ///
    /// Rows are appended whole or not at all, because [`Writer::append`] is. Any blobs written
    /// before a failed log append stay in the pack as unreferenced bytes.
    pub fn commit(&mut self, ts: u64, entries: Vec<Entry>) -> Result<Committed> {
        self.commit_into(ts, None, entries)
    }

    /// [`Self::commit`], continuing a group started by an earlier batch.
    ///
    /// For a burst too large for one batch: a 400-file `cargo fmt` is one line in the timeline
    /// however many fsyncs it takes. The caller must not commit anything else in between, because
    /// [`Self::group_rows`] relies on a group's rows being adjacent.
    pub fn commit_into(
        &mut self,
        ts: u64,
        group: Option<u64>,
        entries: Vec<Entry>,
    ) -> Result<Committed> {
        if self.log.is_none() {
            return Err(Error::ReadOnly);
        }

        // What each path's head is, including the entries before this one in the batch.
        let mut pending: HashMap<u32, (VersionKind, Option<Hash>, bool)> = HashMap::new();
        let mut hashes: HashMap<usize, Hash> = HashMap::new();
        let mut kept = Vec::with_capacity(entries.len());
        let mut stamp_only = Vec::new();

        for (index, entry) in entries.iter().enumerate() {
            let path = self.tables.path(entry.root.0, &entry.rel);
            let hash = entry
                .content
                .as_ref()
                .map(|bytes| *blake3::hash(bytes).as_bytes());
            if let Some(hash) = hash {
                hashes.insert(index, hash);
            }
            let head = pending.get(&path).copied().or_else(|| {
                let head = self.tables.head(&self.tables.paths[path as usize])?;
                Some((head.kind, head.hash, head.omitted))
            });
            let is_file_state =
                !matches!(entry.kind, VersionKind::Label | VersionKind::BaselineDone);
            // A move's row is never a duplicate. The watcher can report the destination first, so
            // the path may already hold these exact bytes, and dropping this row would lose the
            // only link from the file's new name back to its old history.
            let duplicate = is_file_state
                && entry.moved.is_none()
                && match (head, entry.kind) {
                    (None, VersionKind::Deleted) => true,
                    (Some((VersionKind::Deleted, ..)), VersionKind::Deleted) => true,
                    (Some((VersionKind::Deleted, ..)), _) | (None, _) => false,
                    (Some((_, _, was_omitted)), _) if entry.omitted => was_omitted,
                    (Some((_, head_hash, _)), _) => hash.is_some() && head_hash == hash,
                };
            if duplicate {
                if let Some(stamp) = entry.stamp {
                    stamp_only.push((entry.root.0, entry.rel.clone(), stamp, path));
                }
                continue;
            }
            if is_file_state {
                pending.insert(path, (entry.kind, hash, entry.omitted));
            }
            kept.push(index);
        }

        if kept.is_empty() && stamp_only.is_empty() {
            return Ok(Committed::default());
        }

        // Blobs first. `fresh` holds what this batch added, and joins the tables only once the
        // rows naming it are durable.
        let mut fresh: HashMap<Hash, BlobRef> = HashMap::new();
        for &index in &kept {
            let (Some(bytes), Some(&hash)) = (&entries[index].content, hashes.get(&index)) else {
                continue;
            };
            if self.tables.by_hash.contains_key(&hash) || fresh.contains_key(&hash) {
                continue;
            }
            let stored = pack::encode(bytes);
            let (pack, offset) = self.packs.append(&stored)?;
            fresh.insert(
                hash,
                BlobRef {
                    hash,
                    pack,
                    offset,
                    stored: stored.len() as u64,
                },
            );
        }
        if !fresh.is_empty() {
            self.packs.sync()?;
        }

        // Then rows, as one append.
        let group = group.unwrap_or(self.next_id);
        let mut next_id = self.next_id;
        let mut out = Vec::with_capacity(kept.len() * 200);
        let mut lines = Vec::with_capacity(kept.len());
        for &index in &kept {
            let entry = &entries[index];
            let blob = hashes.get(&index).and_then(|hash| {
                let known = self.tables.by_hash.get(hash);
                fresh
                    .get(hash)
                    .copied()
                    .or_else(|| known.map(|&b| self.tables.blobs[b as usize]))
            });
            let line = RowLine {
                id: next_id,
                ts,
                group,
                root: entry.root.0,
                rel: entry.rel.clone(),
                kind: entry.kind,
                source: entry.source,
                blob: blob.as_ref().map(BlobLine::of),
                size: entry.size,
                mode: entry.mode,
                moved: entry.moved.clone(),
                label: entry.label.clone(),
                omitted: entry.omitted,
                stamp: entry.stamp,
            };
            log::push(&mut out, &line)?;
            lines.push(line);
            next_id += 1;
        }
        for (root, rel, stamp, _) in &stamp_only {
            log::push(
                &mut out,
                &StampLine {
                    stamp: *stamp,
                    root: *root,
                    rel: rel.clone(),
                },
            )?;
        }
        self.log.as_mut().ok_or(Error::ReadOnly)?.append(&out)?;

        // Durable. Only now does memory move.
        let rows = lines.len();
        for line in lines {
            self.tables.push_row(line);
        }
        for (_, _, stamp, path) in stamp_only.iter().map(|(a, b, c, d)| (a, b, *c, *d)) {
            self.tables.paths[path as usize].stamp = Some(stamp);
        }
        self.stale_lines += stamp_only.len() as u64;
        self.next_id = next_id;
        Ok(Committed {
            group: (rows > 0).then_some(group),
            rows,
        })
    }

    /// A row by id.
    pub fn version(&self, id: u64) -> Option<Version> {
        self.tables
            .row_by_id(id)
            .map(|row| self.tables.version(row))
    }

    /// A version's bytes, readable after the store's lock is released.
    pub fn blob(&self, id: u64) -> Option<BlobHandle> {
        let row = self.tables.row_by_id(id)?;
        self.packs.handle(self.tables.blobs[row.blob? as usize])
    }

    /// A file's versions, newest first, following cide's own renames back.
    pub fn versions(&self, root: RootId, rel: &str) -> Vec<Version> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut cursor = Some((rel.to_owned(), u64::MAX));
        while let Some((rel, before)) = cursor.take() {
            if !seen.insert(rel.clone()) {
                break;
            }
            let Some(path) = self.tables.path_of(root.0, &rel) else {
                break;
            };
            for &index in self.tables.paths[path as usize].rows.iter().rev() {
                let row = &self.tables.rows[index as usize];
                if row.id >= before {
                    continue;
                }
                // The far end of a move is the same event as the row that sent us here.
                if row.kind == VersionKind::Deleted && row.moved.is_some() && before != u64::MAX {
                    continue;
                }
                out.push(self.tables.version(row));
                if row.kind == VersionKind::Created
                    && let Some(from) = row.moved
                {
                    cursor = Some((self.tables.notes[from as usize].to_string(), row.id));
                    break;
                }
            }
        }
        out
    }

    /// Whether `rel` is `prefix` or inside it. The empty prefix is the whole root.
    fn in_scope(rel: &str, prefix: &str) -> bool {
        prefix.is_empty()
            || rel == prefix
            || (rel.starts_with(prefix) && rel.as_bytes().get(prefix.len()) == Some(&b'/'))
    }

    /// A folder's or project's timeline, newest first, a page at a time.
    ///
    /// `before` is the `group` of the last entry of the previous page.
    pub fn groups(
        &self,
        root: RootId,
        prefix: &str,
        before: Option<u64>,
        limit: usize,
    ) -> Vec<GroupSummary> {
        let mut out: Vec<GroupSummary> = Vec::new();
        for row in self.tables.rows.iter().rev() {
            if before.is_some_and(|before| row.group >= before) {
                continue;
            }
            if row.kind == VersionKind::BaselineDone {
                continue;
            }
            let path = &self.tables.paths[row.path as usize];
            let is_label = row.kind == VersionKind::Label;
            if !is_label && (path.root != root.0 || !Self::in_scope(&path.rel, prefix)) {
                continue;
            }
            // A baseline is committed a batch at a time with live saves between the batches, so
            // it spans many groups and is still one event. Every batch folds into one line.
            if row.kind == VersionKind::Baseline
                && let Some(snapshot) = out.iter_mut().find(|summary| summary.snapshot)
            {
                snapshot.files += 1;
                continue;
            }
            match out.last_mut() {
                Some(last) if last.group == row.group => {
                    last.files += u32::from(!is_label);
                    last.snapshot &= row.kind == VersionKind::Baseline;
                }
                _ => {
                    if out.len() == limit {
                        break;
                    }
                    out.push(GroupSummary {
                        group: row.group,
                        ts: row.ts,
                        source: row.source,
                        files: u32::from(!is_label),
                        label: row.label.map(|n| self.tables.notes[n as usize].to_string()),
                        snapshot: row.kind == VersionKind::Baseline,
                    });
                }
            }
        }
        out
    }

    /// The rows of one group under a scope, in path order.
    pub fn group_rows(&self, root: RootId, prefix: &str, group: u64) -> Vec<Version> {
        let first = self.tables.rows.partition_point(|row| row.id < group);
        let mut out: Vec<Version> = self.tables.rows[first..]
            .iter()
            .take_while(|row| row.group == group)
            .filter(|row| {
                let path = &self.tables.paths[row.path as usize];
                path.root == root.0 && !path.rel.is_empty() && Self::in_scope(&path.rel, prefix)
            })
            .map(|row| self.tables.version(row))
            .collect();
        out.sort_by(|a, b| a.rel.cmp(&b.rel));
        out
    }

    /// The newest row of a group, which is the moment "the state after this change" means.
    pub fn group_end(&self, group: u64) -> Option<u64> {
        let first = self.tables.rows.partition_point(|row| row.id < group);
        self.tables.rows[first..]
            .iter()
            .take_while(|row| row.group == group)
            .last()
            .map(|row| row.id)
    }

    /// What every file under a scope was at a moment, for a folder restore. Paths in order.
    ///
    /// **Refuses to guess, and the refusal is the feature.** The newest row at or before `at`
    /// decides, and with no such row only one thing can be concluded: if the path's first row is
    /// a [`VersionKind::Created`], the watcher saw it appear after that moment, so it did not
    /// exist then. Anything else (a first row that is a baseline) says nothing about that moment,
    /// and the answer is [`TargetState::Unknown`], which leaves the file alone. The difference
    /// between those two is the difference between a restore and deleting a file that had been
    /// there for years.
    ///
    /// **And that conclusion needs the store to remember the moment.** A `Created` proves absence
    /// only back to the file's previous life, and retention forgets those: a file that existed,
    /// was deleted and came back has a `Created` for a first row once the older rows age out,
    /// and when *all* of them have expired there is no row in the store to say otherwise. So any
    /// moment at or before `trimmed_through` gets no proof at all. Per-row bookkeeping was the
    /// first fix for this and it cannot work: the path that lost every row has no row left to
    /// carry the note.
    pub fn plan_at(&self, root: RootId, prefix: &str, at: u64) -> Vec<PlannedFile> {
        let mut out = Vec::new();
        for path in &self.tables.paths {
            if path.root != root.0 || path.rel.is_empty() || !Self::in_scope(&path.rel, prefix) {
                continue;
            }
            let rows = || path.rows.iter().map(|&i| &self.tables.rows[i as usize]);
            let Some(last) = rows().next_back() else {
                continue;
            };
            let then = rows().rfind(|row| row.id <= at);
            let target = match then {
                Some(row) if row.kind == VersionKind::Deleted => TargetState::Absent,
                Some(row) if row.blob.is_some() => TargetState::Content(row.id),
                Some(_) => TargetState::Unknown,
                None if at > self.trimmed_through
                    && rows()
                        .next()
                        .is_some_and(|r| r.kind == VersionKind::Created) =>
                {
                    TargetState::Absent
                }
                None => TargetState::Unknown,
            };
            let unchanged = match target {
                TargetState::Absent => last.kind == VersionKind::Deleted,
                TargetState::Content(id) => {
                    let hash = |row: &Row| row.blob.map(|b| self.tables.blobs[b as usize].hash);
                    last.kind != VersionKind::Deleted
                        && hash(last) == self.tables.row_by_id(id).and_then(hash)
                }
                TargetState::Unknown => false,
            };
            out.push(PlannedFile {
                rel: path.rel.to_string(),
                target,
                unchanged,
            });
        }
        out.sort_by(|a, b| a.rel.cmp(&b.rel));
        out
    }

    /// Forget every version, keeping the roots.
    ///
    /// The log first, in the order [`Self::compact`] uses: a crash before the rename leaves
    /// everything as it was, and a crash after it leaves packs no row names, which the next open
    /// removes. `next_id` survives, so nothing still holding an old id can match a new row.
    pub fn clear(&mut self) -> Result<()> {
        let trimmed_through = self.next_id.saturating_sub(1);
        let header = Header {
            format: log::FORMAT,
            next_id: self.next_id,
            trimmed_through,
        };
        let roots = self.tables.roots.clone();
        self.log.as_mut().ok_or(Error::ReadOnly)?.replace(|out| {
            let mut buffer = Vec::new();
            log::push(&mut buffer, &header)?;
            for (root, path) in roots.iter().enumerate() {
                let line = RootLine {
                    root: root as u16,
                    path: path.clone(),
                };
                log::push(&mut buffer, &line)?;
            }
            out.write_all(&buffer)
        })?;
        self.tables = Tables {
            roots,
            ..Tables::default()
        };
        self.trimmed_through = trimmed_through;
        self.stale_lines = 0;
        let packs: Vec<u32> = self.packs.sizes().map(|(id, _)| id).collect();
        for id in packs {
            self.packs.remove(id)?;
        }
        Ok(())
    }

    /// Bytes in packs. Cheap, unlike [`Self::stats`]: a baseline asks this once per batch.
    pub fn disk_bytes(&self) -> u64 {
        self.packs.disk_bytes()
    }

    pub fn stats(&self) -> Stats {
        let mut live = vec![false; self.tables.blobs.len()];
        for row in &self.tables.rows {
            if let Some(blob) = row.blob {
                live[blob as usize] = true;
            }
        }
        Stats {
            rows: self.tables.rows.len(),
            // Files, so not the pseudo-path the scan marker and labels live on.
            paths: self
                .tables
                .paths
                .iter()
                .filter(|p| !p.rows.is_empty() && !p.rel.is_empty())
                .count(),
            live_bytes: self
                .tables
                .blobs
                .iter()
                .zip(&live)
                .filter(|(_, live)| **live)
                .map(|(blob, _)| blob.stored)
                .sum(),
            disk_bytes: self.packs.disk_bytes(),
            log_bytes: self.log.as_ref().map_or(0, Writer::len),
            skipped_lines: self.skipped_lines,
        }
    }

    /// Apply retention and reclaim space.
    ///
    /// The order is the crash story, and it mirrors [`Self::commit`]: copy live blobs out of the
    /// packs being retired and fsync, then replace the log, and only then unlink. A crash before
    /// the rename leaves the old log and old packs, all intact. A crash after it leaves packs no
    /// row names, which [`Self::open`] removes.
    pub fn compact(&mut self, policy: &Policy, day_of: &dyn Fn(u64) -> i32) -> Result<Compacted> {
        if self.log.is_none() {
            return Err(Error::ReadOnly);
        }
        let facts: Vec<Fact> = self
            .tables
            .rows
            .iter()
            .map(|row| Fact {
                ts: row.ts,
                kind: row.kind,
                blob: row.blob,
            })
            .collect();
        let by_path: Vec<&[u32]> = self
            .tables
            .paths
            .iter()
            .map(|path| path.rows.as_slice())
            .collect();
        let stored: Vec<u64> = self.tables.blobs.iter().map(|blob| blob.stored).collect();
        let keep = retention::plan(&facts, &by_path, &stored, policy, day_of);
        let dropped_rows = keep.iter().filter(|keep| !**keep).count();

        let mut live = vec![false; self.tables.blobs.len()];
        for (row, _) in self
            .tables
            .rows
            .iter()
            .zip(&keep)
            .filter(|(_, keep)| **keep)
        {
            if let Some(blob) = row.blob {
                live[blob as usize] = true;
            }
        }
        let mut live_bytes: HashMap<u32, u64> = HashMap::new();
        for (blob, _) in self
            .tables
            .blobs
            .iter()
            .zip(&live)
            .filter(|(_, live)| **live)
        {
            *live_bytes.entry(blob.pack).or_default() += blob.stored;
        }
        let tail = self.packs.tail();
        let retiring: Vec<u32> = self
            .packs
            .sizes()
            .filter(|(id, len)| {
                let holding = live_bytes.get(id).copied().unwrap_or(0);
                Some(*id) != tail && holding.saturating_mul(2) < *len
            })
            .map(|(id, _)| id)
            .collect();

        if dropped_rows == 0 && retiring.is_empty() && self.stale_lines < STALE_LINES {
            return Ok(Compacted::default());
        }

        // A blob that cannot be read stays where it is, and so does its pack. Losing the ability
        // to compact over one bad sector would let the store outgrow its cap for ever, and
        // leaving the pack in place loses nothing that was not already lost.
        let mut relocated: HashMap<u32, BlobRef> = HashMap::new();
        let mut rewritten = HashSet::new();
        for (index, blob) in self.tables.blobs.iter().enumerate() {
            if !live[index] || !retiring.contains(&blob.pack) {
                continue;
            }
            match self.packs.stored(blob) {
                Ok(stored) => {
                    let (pack, offset) = self.packs.append(&stored)?;
                    rewritten.insert(blob.pack);
                    relocated.insert(
                        index as u32,
                        BlobRef {
                            pack,
                            offset,
                            ..*blob
                        },
                    );
                }
                Err(error) => {
                    tracing::warn!(%error, pack = blob.pack, "could not move a Local History blob");
                }
            }
        }
        self.packs.sync()?;

        let next = self.tables.retain(&keep, &relocated);
        let dropped_through = self
            .tables
            .rows
            .iter()
            .zip(&keep)
            .filter(|(_, keep)| !**keep)
            .map(|(row, _)| row.id)
            .max();
        let trimmed_through =
            dropped_through.map_or(self.trimmed_through, |id| id.max(self.trimmed_through));
        let header = Header {
            format: log::FORMAT,
            next_id: self.next_id,
            trimmed_through,
        };
        self.log.as_mut().ok_or(Error::ReadOnly)?.replace(|out| {
            let mut buffer = Vec::with_capacity(1 << 16);
            log::push(&mut buffer, &header)?;
            for (root, path) in next.roots.iter().enumerate() {
                log::push(
                    &mut buffer,
                    &RootLine {
                        root: root as u16,
                        path: path.clone(),
                    },
                )?;
            }
            for (index, row) in next.rows.iter().enumerate() {
                // Only a path's newest row carries its stamp: it is what `Tables::push_row`
                // keeps, and a stamp on every old row is bytes nobody reads.
                let path = &next.paths[row.path as usize];
                let is_head = path.rows.last() == Some(&(index as u32));
                log::push(
                    &mut buffer,
                    &next.line_of(row, path.stamp.filter(|_| is_head)),
                )?;
                if buffer.len() >= 1 << 16 {
                    out.write_all(&buffer)?;
                    buffer.clear();
                }
            }
            out.write_all(&buffer)
        })?;

        self.tables = next;
        self.trimmed_through = trimmed_through;
        self.stale_lines = 0;
        let removed_packs = self.remove_unreferenced_packs()?;
        Ok(Compacted {
            dropped_rows,
            rewritten_packs: rewritten.len(),
            removed_packs,
        })
    }
}
