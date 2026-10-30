//! The store, driven through its public face and its real files.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use cide_local_history::{
    Entry, Error, Policy, RootId, Store, TargetState, Version, VersionKind, VersionSource, exists,
};

const DAY: u64 = 86_400_000;

/// A fresh directory per call. Keyed by pid so two test binaries cannot share one.
fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "cide-lh-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn day_of(ts: u64) -> i32 {
    (ts / DAY) as i32
}

fn keep_everything() -> Policy {
    Policy {
        days_to_keep: 1000,
        max_storage: u64::MAX,
        max_rows: usize::MAX,
    }
}

fn open(dir: &Path) -> (Store, RootId) {
    let mut store = Store::open(dir).unwrap();
    let root = store.ensure_root("/project").unwrap();
    (store, root)
}

fn modified(root: RootId, rel: &str, text: &str) -> Entry {
    Entry::new(root, rel, VersionKind::Modified, VersionSource::External).content(
        text.as_bytes().to_vec(),
        0o100644,
        None,
    )
}

fn text_of(store: &Store, version: &Version) -> String {
    let bytes = store.blob(version.id).unwrap().read().unwrap();
    String::from_utf8(bytes).unwrap()
}

#[test]
fn a_version_survives_reopening_with_its_bytes_and_metadata() {
    let dir = scratch("reopen");
    {
        let (mut store, root) = open(&dir);
        let entry = Entry::new(
            root,
            "src/a.rs",
            VersionKind::Baseline,
            VersionSource::Detected,
        )
        .content(b"one\n".to_vec(), 0o100755, None);
        store.commit(1000, vec![entry]).unwrap();
        store
            .commit(2000, vec![modified(root, "src/a.rs", "two\n")])
            .unwrap();
    }
    let (store, root) = open(&dir);
    let versions = store.versions(root, "src/a.rs");
    assert_eq!(versions.len(), 2);
    assert_eq!(text_of(&store, &versions[0]), "two\n");
    assert_eq!(text_of(&store, &versions[1]), "one\n");
    assert_eq!(versions[1].mode, 0o100755);
    assert_eq!(versions[1].kind, VersionKind::Baseline);
    assert_eq!(versions[0].ts, 2000);
}

/// cide's own save arrives twice: once from the save, once from the watcher. This is the whole
/// of what keeps that from being two versions, with no timer involved.
#[test]
fn the_same_content_again_is_not_a_new_version() {
    let dir = scratch("dedupe");
    let (mut store, root) = open(&dir);
    assert_eq!(
        store
            .commit(1, vec![modified(root, "a", "x")])
            .unwrap()
            .rows,
        1
    );
    let again = store.commit(2, vec![modified(root, "a", "x")]).unwrap();
    assert_eq!(again.rows, 0);
    assert_eq!(again.group, None);
    assert_eq!(store.versions(root, "a").len(), 1);
}

/// Compared with the head and only the head: going back to an old state is a real change, and
/// it is what a `git checkout` of the previous branch looks like.
#[test]
fn returning_to_an_earlier_content_is_a_new_version() {
    let dir = scratch("aba");
    let (mut store, root) = open(&dir);
    for (ts, text) in [(1, "a"), (2, "b"), (3, "a")] {
        store.commit(ts, vec![modified(root, "f", text)]).unwrap();
    }
    assert_eq!(store.versions(root, "f").len(), 3);
    let stats = store.stats();
    assert_eq!(stats.rows, 3);
}

#[test]
fn identical_content_at_two_paths_is_stored_once() {
    let dir = scratch("shared-blob");
    let (mut store, root) = open(&dir);
    let text = "the same file\n".repeat(100);
    store.commit(1, vec![modified(root, "one", &text)]).unwrap();
    let size = store.stats().disk_bytes;
    store.commit(2, vec![modified(root, "two", &text)]).unwrap();
    assert_eq!(store.stats().disk_bytes, size);
    assert_eq!(text_of(&store, &store.versions(root, "two")[0]), text);
}

#[test]
fn a_batch_is_one_group_and_a_deletion_of_nothing_is_dropped() {
    let dir = scratch("group");
    let (mut store, root) = open(&dir);
    let done = store
        .commit(
            1,
            vec![
                modified(root, "a", "1"),
                modified(root, "dir/b", "2"),
                Entry::new(
                    root,
                    "never-seen",
                    VersionKind::Deleted,
                    VersionSource::External,
                ),
            ],
        )
        .unwrap();
    assert_eq!(done.rows, 2);
    let groups = store.groups(root, "", None, 10);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].files, 2);
    let rows = store.group_rows(root, "dir", groups[0].group);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].rel, "dir/b");
}

#[test]
fn a_prefix_means_a_folder_and_not_a_string_prefix() {
    let dir = scratch("scope");
    let (mut store, root) = open(&dir);
    store.commit(1, vec![modified(root, "src/a", "1")]).unwrap();
    store
        .commit(2, vec![modified(root, "src-old/b", "2")])
        .unwrap();
    assert_eq!(store.groups(root, "src", None, 10).len(), 1);
    assert_eq!(store.groups(root, "", None, 10).len(), 2);
    assert_eq!(store.groups(root, "src/a", None, 10).len(), 1);
}

#[test]
fn the_timeline_pages_newest_first_without_gaps() {
    let dir = scratch("paging");
    let (mut store, root) = open(&dir);
    for i in 0..7 {
        store
            .commit(i, vec![modified(root, "f", &i.to_string())])
            .unwrap();
    }
    let first = store.groups(root, "", None, 3);
    let second = store.groups(root, "", Some(first[2].group), 3);
    let third = store.groups(root, "", Some(second[2].group), 3);
    assert_eq!(first.len() + second.len() + third.len(), 7);
    assert_eq!(third.len(), 1);
    assert!(first[0].ts > second[0].ts && second[0].ts > third[0].ts);
}

#[test]
fn a_first_scan_is_one_snapshot_line() {
    let dir = scratch("snapshot");
    let (mut store, root) = open(&dir);
    let scan: Vec<_> = (0..50)
        .map(|i| {
            Entry::new(
                root,
                format!("f{i}"),
                VersionKind::Baseline,
                VersionSource::Detected,
            )
            .content(vec![i], 0o100644, None)
        })
        .collect();
    store.commit(1, scan).unwrap();
    let groups = store.groups(root, "", None, 10);
    assert_eq!(groups.len(), 1);
    assert!(groups[0].snapshot);
    assert_eq!(groups[0].files, 50);
}

#[test]
fn the_scan_marker_is_recorded_and_stays_out_of_the_timeline() {
    let dir = scratch("marker");
    let (mut store, root) = open(&dir);
    assert!(!store.baseline_done(root));
    store
        .commit(
            1,
            vec![Entry::new(
                root,
                "",
                VersionKind::BaselineDone,
                VersionSource::Detected,
            )],
        )
        .unwrap();
    assert!(store.baseline_done(root));
    assert!(store.groups(root, "", None, 10).is_empty());
    assert!(store.heads(root).is_empty(), "the pseudo-path is no file");
    drop(store);
    let (store, root) = open(&dir);
    assert!(store.baseline_done(root));
}

#[test]
fn a_label_shows_in_every_scope_and_records_no_file() {
    let dir = scratch("label");
    let (mut store, root) = open(&dir);
    store.commit(1, vec![modified(root, "src/a", "1")]).unwrap();
    store
        .commit(
            2,
            vec![
                Entry::new(root, "", VersionKind::Label, VersionSource::Saved)
                    .label("Before refactor"),
            ],
        )
        .unwrap();
    let groups = store.groups(root, "src", None, 10);
    assert_eq!(groups[0].label.as_deref(), Some("Before refactor"));
    assert_eq!(groups[0].files, 0);
    assert_eq!(store.versions(root, "src/a").len(), 1);
}

#[test]
fn a_rename_keeps_the_files_history_and_the_old_path_shows_as_gone() {
    let dir = scratch("rename");
    let (mut store, root) = open(&dir);
    store
        .commit(1, vec![modified(root, "old.rs", "1")])
        .unwrap();
    store
        .commit(2, vec![modified(root, "old.rs", "2")])
        .unwrap();
    store
        .commit(
            3,
            vec![
                Entry::new(root, "old.rs", VersionKind::Deleted, VersionSource::FileOp)
                    .moved("new.rs"),
                Entry::new(root, "new.rs", VersionKind::Created, VersionSource::FileOp)
                    .content(b"2".to_vec(), 0o100644, None)
                    .moved("old.rs"),
            ],
        )
        .unwrap();
    store
        .commit(4, vec![modified(root, "new.rs", "3")])
        .unwrap();

    let history = store.versions(root, "new.rs");
    let timestamps: Vec<_> = history.iter().map(|v| v.ts).collect();
    assert_eq!(
        timestamps,
        [4, 3, 2, 1],
        "the move's far end is not listed twice"
    );
    assert_eq!(
        store.head(root, "old.rs").unwrap().kind,
        VersionKind::Deleted
    );
}

#[test]
fn a_rename_cycle_ends() {
    let dir = scratch("rename-cycle");
    let (mut store, root) = open(&dir);
    store.commit(1, vec![modified(root, "a", "1")]).unwrap();
    for (ts, from, to) in [(2, "a", "b"), (3, "b", "a")] {
        store
            .commit(
                ts,
                vec![
                    Entry::new(root, from, VersionKind::Deleted, VersionSource::FileOp).moved(to),
                    Entry::new(root, to, VersionKind::Created, VersionSource::FileOp)
                        .content(b"1".to_vec(), 0o100644, None)
                        .moved(from),
                ],
            )
            .unwrap();
    }
    assert!(!store.versions(root, "a").is_empty());
}

/// The failure this guards: a log cut at any point must lose at most the rows after the cut, and
/// the store must keep working. Every single length is tried, because the bug this prevents is a
/// new row glued onto half of an old one, which only some offsets produce.
#[test]
fn a_log_truncated_at_any_byte_opens_keeps_its_prefix_and_accepts_rows() {
    let dir = scratch("torn");
    {
        let (mut store, root) = open(&dir);
        for (ts, text) in [(1, "one"), (2, "two"), (3, "three")] {
            store.commit(ts, vec![modified(root, "f", text)]).unwrap();
        }
    }
    let log = fs::read(dir.join("log.jsonl")).unwrap();
    let packs = dir.join("packs");
    let pack_names: Vec<_> = fs::read_dir(&packs)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();

    for len in 0..=log.len() {
        let copy = scratch("torn-copy");
        fs::create_dir_all(copy.join("packs")).unwrap();
        fs::write(copy.join("log.jsonl"), &log[..len]).unwrap();
        for pack in &pack_names {
            fs::copy(pack, copy.join("packs").join(pack.file_name().unwrap())).unwrap();
        }

        let (mut store, root) = open(&copy);
        let survived: Vec<_> = store
            .versions(root, "f")
            .iter()
            .map(|v| text_of(&store, v))
            .collect();
        let expected = ["three", "two", "one"];
        assert_eq!(
            survived,
            expected[3 - survived.len()..],
            "cut at byte {len}"
        );

        store.commit(9, vec![modified(root, "g", "after")]).unwrap();
        drop(store);
        let (store, root) = open(&copy);
        assert_eq!(
            text_of(&store, &store.versions(root, "g")[0]),
            "after",
            "a row appended after a cut at byte {len} must be readable by the next open"
        );
        fs::remove_dir_all(&copy).unwrap();
    }
}

/// A crash between the two fsyncs: the bytes reached the pack and no row names them.
#[test]
fn bytes_no_row_names_are_harmless() {
    let dir = scratch("orphan-bytes");
    {
        let (mut store, root) = open(&dir);
        store.commit(1, vec![modified(root, "f", "kept")]).unwrap();
    }
    let log = fs::read(dir.join("log.jsonl")).unwrap();
    {
        let (mut store, root) = open(&dir);
        store
            .commit(2, vec![modified(root, "f", "lost in the crash")])
            .unwrap();
    }
    fs::write(dir.join("log.jsonl"), &log).unwrap();

    let (mut store, root) = open(&dir);
    assert_eq!(text_of(&store, &store.versions(root, "f")[0]), "kept");
    store
        .commit(3, vec![modified(root, "f", "lost in the crash")])
        .unwrap();
    assert_eq!(
        text_of(&store, &store.versions(root, "f")[0]),
        "lost in the crash"
    );
}

#[test]
fn a_bad_line_in_the_middle_costs_that_line_and_not_the_rest() {
    let dir = scratch("bad-line");
    {
        let (mut store, root) = open(&dir);
        store.commit(1, vec![modified(root, "a", "1")]).unwrap();
        store.commit(2, vec![modified(root, "b", "2")]).unwrap();
        store.commit(3, vec![modified(root, "c", "3")]).unwrap();
    }
    let log = fs::read_to_string(dir.join("log.jsonl")).unwrap();
    let damaged: Vec<_> = log
        .lines()
        .map(|line| {
            if line.contains(r#""p":"b""#) {
                "{\"id\":not json"
            } else {
                line
            }
        })
        .collect();
    fs::write(dir.join("log.jsonl"), damaged.join("\n") + "\n").unwrap();

    let (store, root) = open(&dir);
    assert_eq!(store.stats().skipped_lines, 1);
    assert!(store.head(root, "a").is_some());
    assert!(store.head(root, "b").is_none());
    assert!(store.head(root, "c").is_some());
}

#[test]
fn a_second_opener_reads_and_cannot_write() {
    let dir = scratch("lock");
    let (mut first, root) = open(&dir);
    first.commit(1, vec![modified(root, "f", "x")]).unwrap();

    let mut second = Store::open(&dir).unwrap();
    assert!(first.is_writable());
    assert!(!second.is_writable());
    assert_eq!(
        second.head(root, "f").map(|h| h.kind),
        Some(VersionKind::Modified)
    );
    assert!(matches!(
        second.commit(2, vec![modified(root, "f", "y")]),
        Err(Error::ReadOnly)
    ));
    assert!(matches!(
        second.compact(&keep_everything(), &day_of),
        Err(Error::ReadOnly)
    ));

    drop(first);
    assert!(
        Store::open(&dir).unwrap().is_writable(),
        "the lock dies with its holder"
    );
}

/// A read-only opener must leave a writer's files alone, torn tail included: that tail may be
/// the writer's append in progress.
#[test]
fn a_read_only_opener_never_truncates() {
    let dir = scratch("readonly-torn");
    let (mut first, root) = open(&dir);
    first.commit(1, vec![modified(root, "f", "x")]).unwrap();
    let log = dir.join("log.jsonl");
    let mut bytes = fs::read(&log).unwrap();
    bytes.extend_from_slice(b"{\"id\":99,");
    fs::write(&log, &bytes).unwrap();

    let second = Store::open(&dir).unwrap();
    assert!(!second.is_writable());
    assert_eq!(fs::read(&log).unwrap(), bytes);
}

#[test]
fn a_store_from_a_newer_format_is_not_rewritten() {
    let dir = scratch("newer");
    let log = dir.join("log.jsonl");
    let text = "{\"format\":99,\"next_id\":5}\n{\"id\":1,\"future\":true}\n";
    fs::write(&log, text).unwrap();
    let store = Store::open(&dir).unwrap();
    assert!(!store.is_writable());
    assert_eq!(store.stats().rows, 0);
    assert_eq!(fs::read_to_string(&log).unwrap(), text);
}

#[test]
fn ids_are_not_reused_after_the_newest_rows_are_compacted_away() {
    let dir = scratch("next-id");
    let last = {
        let (mut store, root) = open(&dir);
        store.commit(DAY, vec![modified(root, "a", "1")]).unwrap();
        store
            .commit(2 * DAY, vec![modified(root, "b", "2")])
            .unwrap();
        let last = store.head(root, "b").unwrap().id;
        store
            .commit(
                3 * DAY,
                vec![Entry::new(
                    root,
                    "b",
                    VersionKind::Deleted,
                    VersionSource::External,
                )],
            )
            .unwrap();
        store
            .commit(4 * DAY, vec![modified(root, "a", "3")])
            .unwrap();
        store
            .compact(
                &Policy {
                    days_to_keep: 1,
                    ..keep_everything()
                },
                &day_of,
            )
            .unwrap();
        assert!(store.head(root, "b").is_none());
        last
    };
    let (mut store, root) = open(&dir);
    store
        .commit(5 * DAY, vec![modified(root, "c", "4")])
        .unwrap();
    assert!(store.head(root, "c").unwrap().id > last + 2);
}

/// The bug this exists for: a writer holding the old inode after the rename would report success
/// for every later row, and all of them would vanish at the next open.
#[test]
fn rows_appended_after_compaction_reach_the_new_log() {
    let dir = scratch("after-compaction");
    {
        let (mut store, root) = open(&dir);
        store.commit(DAY, vec![modified(root, "a", "1")]).unwrap();
        store
            .commit(2 * DAY, vec![modified(root, "a", "2")])
            .unwrap();
        store
            .commit(3 * DAY, vec![modified(root, "a", "3")])
            .unwrap();
        let done = store
            .compact(
                &Policy {
                    days_to_keep: 1,
                    ..keep_everything()
                },
                &day_of,
            )
            .unwrap();
        assert_eq!(done.dropped_rows, 2);
        store
            .commit(4 * DAY, vec![modified(root, "a", "4")])
            .unwrap();
    }
    let (store, root) = open(&dir);
    let texts: Vec<_> = store
        .versions(root, "a")
        .iter()
        .map(|v| text_of(&store, v))
        .collect();
    assert_eq!(texts, ["4", "3"]);
}

#[test]
fn stamps_survive_compaction_and_reopening() {
    use cide_local_history::Stamp;
    let dir = scratch("stamps");
    let stamp = Stamp {
        mtime_ns: 1,
        ctime_ns: 2,
        inode: 3,
        len: 1,
    };
    let touched = Stamp {
        mtime_ns: 9,
        ..stamp
    };
    {
        let (mut store, root) = open(&dir);
        let entry = modified(root, "a", "1");
        store
            .commit(
                DAY,
                vec![Entry {
                    stamp: Some(stamp),
                    ..entry
                }],
            )
            .unwrap();
        // Same bytes, moved stamp: no row, and the stamp still has to land.
        let entry = modified(root, "a", "1");
        let done = store
            .commit(
                DAY,
                vec![Entry {
                    stamp: Some(touched),
                    ..entry
                }],
            )
            .unwrap();
        assert_eq!(done.rows, 0);
        assert_eq!(store.head(root, "a").unwrap().stamp, Some(touched));
    }
    {
        let (mut store, root) = open(&dir);
        assert_eq!(
            store.head(root, "a").unwrap().stamp,
            Some(touched),
            "stamp-only line replayed"
        );
        store
            .commit(2 * DAY, vec![modified(root, "b", "2")])
            .unwrap();
        store
            .compact(
                &Policy {
                    days_to_keep: 1,
                    ..keep_everything()
                },
                &|_| 0,
            )
            .unwrap();
    }
    let (store, root) = open(&dir);
    assert_eq!(store.head(root, "a").unwrap().stamp, Some(touched));
}

#[test]
fn a_damaged_blob_is_an_error_and_not_a_wrong_answer() {
    let dir = scratch("corrupt");
    let (mut store, root) = open(&dir);
    store
        .commit(1, vec![modified(root, "f", &"some content\n".repeat(50))])
        .unwrap();
    let pack = fs::read_dir(dir.join("packs"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut bytes = fs::read(&pack).unwrap();
    let at = bytes.len() - 5;
    bytes[at] ^= 0xff;
    fs::write(&pack, bytes).unwrap();

    let version = &store.versions(root, "f")[0];
    assert!(matches!(
        store.blob(version.id).unwrap().read(),
        Err(Error::Corrupt(_))
    ));
}

/// Enough data to seal a pack, then retention drops most of it. The sealed pack must be retired
/// and a surviving blob must still read from wherever it moved to.
#[test]
fn compaction_reclaims_a_sealed_pack_and_moves_what_is_still_live() {
    fn incompressible(seed: u64, len: usize) -> Vec<u8> {
        let mut state = seed | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect()
    }

    let dir = scratch("sealed");
    let (mut store, root) = open(&dir);
    let keeper = incompressible(7, 1024);
    store
        .commit(
            DAY,
            vec![
                Entry::new(root, "keep", VersionKind::Modified, VersionSource::External).content(
                    keeper.clone(),
                    0o100644,
                    None,
                ),
            ],
        )
        .unwrap();
    for i in 0..8 {
        // Doubled, because `incompressible` sets the low bit: consecutive seeds would be the same
        // bytes, and the store would rightly keep one copy.
        store
            .commit(
                DAY,
                vec![
                    Entry::new(
                        root,
                        "churn",
                        VersionKind::Modified,
                        VersionSource::External,
                    )
                    .content(
                        incompressible((100 + i) * 2, 10 << 20),
                        0o100644,
                        None,
                    ),
                ],
            )
            .unwrap();
    }
    let before = store.stats();
    assert!(before.disk_bytes > 64 << 20, "the first pack has rolled");
    let handle_before = store.blob(store.head(root, "churn").unwrap().id).unwrap();
    let old_blob = store.blob(store.versions(root, "churn")[7].id).unwrap();

    let done = store
        .compact(
            &Policy {
                days_to_keep: 1,
                max_storage: 1,
                max_rows: usize::MAX,
            },
            &day_of,
        )
        .unwrap();
    assert!(done.dropped_rows > 0);
    assert!(done.removed_packs > 0, "{done:?}");
    let after = store.stats();
    assert!(after.disk_bytes < before.disk_bytes);

    let version = &store.versions(root, "keep")[0];
    assert_eq!(store.blob(version.id).unwrap().read().unwrap(), keeper);
    assert!(handle_before.read().is_ok());
    assert!(
        old_blob.read().is_ok(),
        "a read in flight when its pack is unlinked finishes on the open descriptor"
    );

    drop(store);
    let (store, root) = open(&dir);
    assert_eq!(store.stats().rows, after.rows);
    let version = &store.versions(root, "keep")[0];
    assert_eq!(store.blob(version.id).unwrap().read().unwrap(), keeper);
}

#[test]
fn nothing_to_do_does_not_rewrite_the_log() {
    let dir = scratch("idle-compaction");
    let (mut store, root) = open(&dir);
    store.commit(DAY, vec![modified(root, "a", "1")]).unwrap();
    let log = dir.join("log.jsonl");
    let before = fs::metadata(&log).unwrap().modified().unwrap();
    let done = store.compact(&keep_everything(), &day_of).unwrap();
    assert_eq!(done, Default::default());
    assert_eq!(fs::metadata(&log).unwrap().modified().unwrap(), before);
}

#[test]
fn a_store_is_private_to_its_user() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("modes").join("store");
    let (mut store, root) = open(&dir);
    store.commit(1, vec![modified(root, "a", "1")]).unwrap();
    let mode = |path: PathBuf| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(dir.clone()), 0o700);
    assert_eq!(mode(dir.join("packs")), 0o700);
    assert_eq!(mode(dir.join("log.jsonl")), 0o600);
    assert_eq!(mode(dir.join("lock")), 0o600);
    let pack = fs::read_dir(dir.join("packs"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(mode(pack), 0o600);
}

#[test]
fn checking_for_a_store_creates_nothing() {
    let dir = scratch("exists").join("never");
    assert!(!exists(&dir));
    assert!(!dir.exists());
}

fn created(root: RootId, rel: &str, text: &str) -> Entry {
    Entry::new(root, rel, VersionKind::Created, VersionSource::External).content(
        text.as_bytes().to_vec(),
        0o100644,
        None,
    )
}

fn baseline(root: RootId, rel: &str, text: &str) -> Entry {
    Entry::new(root, rel, VersionKind::Baseline, VersionSource::Detected).content(
        text.as_bytes().to_vec(),
        0o100644,
        None,
    )
}

fn deleted(root: RootId, rel: &str) -> Entry {
    Entry::new(root, rel, VersionKind::Deleted, VersionSource::External)
}

fn target_of(plan: &[cide_local_history::PlannedFile], rel: &str) -> TargetState {
    plan.iter().find(|file| file.rel == rel).unwrap().target
}

/// The whole safety argument of a folder restore, as one table.
#[test]
fn a_plan_only_removes_what_it_can_prove_did_not_exist() {
    let dir = scratch("plan");
    let (mut store, root) = open(&dir);
    store
        .commit(
            1,
            vec![baseline(root, "kept", "1"), baseline(root, "gone", "1")],
        )
        .unwrap();
    let point = store.head(root, "gone").unwrap().id;
    store.commit(2, vec![modified(root, "kept", "2")]).unwrap();
    store
        .commit(3, vec![deleted(root, "gone"), created(root, "new", "x")])
        .unwrap();
    store
        .commit(4, vec![baseline(root, "found-later", "y")])
        .unwrap();

    let plan = store.plan_at(root, "", point);
    let kept = store.versions(root, "kept");
    assert_eq!(
        target_of(&plan, "kept"),
        TargetState::Content(kept[1].id),
        "the older content comes back"
    );
    let gone = store.versions(root, "gone");
    assert_eq!(
        target_of(&plan, "gone"),
        TargetState::Content(gone[1].id),
        "a deleted file comes back"
    );
    assert_eq!(
        target_of(&plan, "new"),
        TargetState::Absent,
        "the watcher saw this appear after that moment, so it goes"
    );
    assert_eq!(
        target_of(&plan, "found-later"),
        TargetState::Unknown,
        "a first sighting says nothing about when it appeared, so it is left alone"
    );
}

/// The reason `Created` is a claim and not a default: retention can remove the row that proved
/// it, and a file whose history starts part way through must not then read as new.
#[test]
fn retention_turns_a_proof_into_an_unknown_and_never_the_reverse() {
    let dir = scratch("plan-retention");
    let (mut store, root) = open(&dir);
    store.commit(DAY, vec![created(root, "a", "1")]).unwrap();
    let early = store.head(root, "a").unwrap().id;
    store
        .commit(5 * DAY, vec![modified(root, "a", "2")])
        .unwrap();
    store
        .commit(6 * DAY, vec![modified(root, "a", "3")])
        .unwrap();
    store
        .compact(
            &Policy {
                days_to_keep: 2,
                ..keep_everything()
            },
            &day_of,
        )
        .unwrap();
    let plan = store.plan_at(root, "", early);
    assert_eq!(target_of(&plan, "a"), TargetState::Unknown);
}

#[test]
fn a_version_with_no_copy_is_unknown_and_not_empty() {
    let dir = scratch("plan-omitted");
    let (mut store, root) = open(&dir);
    let big = Entry::new(root, "big", VersionKind::Baseline, VersionSource::Detected).omit(
        1 << 30,
        0o100644,
        None,
    );
    store.commit(1, vec![big]).unwrap();
    let plan = store.plan_at(root, "", u64::MAX);
    assert_eq!(target_of(&plan, "big"), TargetState::Unknown);
}

#[test]
fn a_plan_stays_inside_its_folder_and_knows_what_needs_no_work() {
    let dir = scratch("plan-scope");
    let (mut store, root) = open(&dir);
    store
        .commit(
            1,
            vec![
                baseline(root, "src/a", "1"),
                baseline(root, "src-old/b", "1"),
            ],
        )
        .unwrap();
    let plan = store.plan_at(root, "src", u64::MAX);
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].rel, "src/a");
    assert!(plan[0].unchanged);
}

#[test]
fn a_group_ends_at_its_last_row() {
    let dir = scratch("group-end");
    let (mut store, root) = open(&dir);
    let done = store
        .commit(1, vec![modified(root, "a", "1"), modified(root, "b", "2")])
        .unwrap();
    store.commit(2, vec![modified(root, "c", "3")]).unwrap();
    let group = done.group.unwrap();
    assert_eq!(store.group_end(group), Some(group + 1));
    assert_eq!(store.group_end(999), None);
}

#[test]
fn clearing_forgets_everything_and_the_store_keeps_working() {
    let dir = scratch("clear");
    let last = {
        let (mut store, root) = open(&dir);
        store.commit(1, vec![modified(root, "a", "1")]).unwrap();
        let last = store.head(root, "a").unwrap().id;
        store
            .commit(
                2,
                vec![Entry::new(
                    root,
                    "",
                    VersionKind::BaselineDone,
                    VersionSource::Detected,
                )],
            )
            .unwrap();
        store.clear().unwrap();
        assert_eq!(store.stats().rows, 0);
        assert_eq!(store.disk_bytes(), 0);
        assert!(
            !store.baseline_done(root),
            "the next pass is a fresh first scan"
        );
        store.commit(3, vec![modified(root, "b", "2")]).unwrap();
        last
    };
    let (store, root) = open(&dir);
    assert!(store.head(root, "a").is_none());
    assert_eq!(text_of(&store, &store.versions(root, "b")[0]), "2");
    assert!(
        store.head(root, "b").unwrap().id > last,
        "ids are not reused"
    );
}

/// A file that was there, went, and came back. Once retention has trimmed the early rows, the
/// `Created` left at the front is only true since the deletion, and read as "did not exist before
/// this" it would have a restore to an older moment trash a file that existed at that moment.
#[test]
fn a_creation_stops_being_proof_once_the_rows_before_it_are_gone() {
    let dir = scratch("plan-recreated");
    let (mut store, root) = open(&dir);
    store
        .commit(
            DAY,
            vec![baseline(root, "a", "1"), baseline(root, "cold", "c")],
        )
        .unwrap();
    let early = store.head(root, "a").unwrap().id;
    store.commit(2 * DAY, vec![deleted(root, "a")]).unwrap();
    store
        .commit(9 * DAY, vec![created(root, "a", "2")])
        .unwrap();

    assert_eq!(
        target_of(&store.plan_at(root, "", early), "a"),
        TargetState::Content(early),
        "with its whole history the store simply knows"
    );
    store
        .compact(
            &Policy {
                days_to_keep: 1,
                ..keep_everything()
            },
            &day_of,
        )
        .unwrap();
    assert_eq!(
        store.versions(root, "a").len(),
        1,
        "the early rows aged out"
    );
    assert_eq!(
        target_of(&store.plan_at(root, "", early), "a"),
        TargetState::Unknown,
        "it existed at that moment, and nothing left in the store can say so"
    );
    drop(store);
    let (store, root) = open(&dir);
    assert_eq!(
        target_of(&store.plan_at(root, "", early), "a"),
        TargetState::Unknown,
        "and that survives the rewrite of the log"
    );
}

/// Still a proof when nothing was trimmed, or a folder restore could never remove anything.
#[test]
fn an_untrimmed_creation_is_still_proof() {
    let dir = scratch("plan-untrimmed");
    let (mut store, root) = open(&dir);
    store.commit(DAY, vec![baseline(root, "a", "1")]).unwrap();
    let early = store.head(root, "a").unwrap().id;
    store
        .commit(2 * DAY, vec![created(root, "new", "x")])
        .unwrap();
    store
        .commit(3 * DAY, vec![modified(root, "a", "2")])
        .unwrap();
    store
        .compact(
            &Policy {
                days_to_keep: 100,
                ..keep_everything()
            },
            &day_of,
        )
        .unwrap();
    assert_eq!(
        target_of(&store.plan_at(root, "", early), "new"),
        TargetState::Absent
    );
}

/// A rename's chain survives its row losing the claim, or trimming would cut the history in two.
#[test]
fn a_trimmed_rename_still_leads_back_to_the_old_name() {
    let dir = scratch("plan-rename-trimmed");
    let (mut store, root) = open(&dir);
    store.commit(DAY, vec![baseline(root, "old", "1")]).unwrap();
    store
        .commit(2 * DAY, vec![baseline(root, "new", "0")])
        .unwrap();
    store
        .commit(
            3 * DAY,
            vec![
                Entry::new(root, "old", VersionKind::Deleted, VersionSource::FileOp).moved("new"),
                created(root, "new", "1").moved("old"),
            ],
        )
        .unwrap();
    store
        .compact(
            &Policy {
                days_to_keep: 1,
                ..keep_everything()
            },
            &day_of,
        )
        .unwrap();
    let chain: Vec<_> = store.versions(root, "new").iter().map(|v| v.ts).collect();
    assert_eq!(chain, [3 * DAY]);
    assert_eq!(
        store.versions(root, "new")[0].moved.as_deref(),
        Some("old"),
        "the row keeps saying where it came from"
    );
}

/// The same false proof by a route no per-row rule can see: retention removes every row of a
/// deleted file, so when it comes back its `Created` really is its first row.
#[test]
fn a_file_whose_whole_history_expired_is_not_proven_new_when_it_returns() {
    let dir = scratch("plan-expired");
    let (mut store, root) = open(&dir);
    store
        .commit(
            DAY,
            vec![baseline(root, "a", "1"), baseline(root, "cold", "c")],
        )
        .unwrap();
    let early = store.head(root, "a").unwrap().id;
    store.commit(2 * DAY, vec![deleted(root, "a")]).unwrap();
    store
        .commit(3 * DAY, vec![modified(root, "cold", "d")])
        .unwrap();
    store
        .commit(4 * DAY, vec![modified(root, "cold", "e")])
        .unwrap();
    store
        .compact(
            &Policy {
                days_to_keep: 2,
                ..keep_everything()
            },
            &day_of,
        )
        .unwrap();
    assert!(store.head(root, "a").is_none(), "its history has expired");
    store
        .commit(9 * DAY, vec![created(root, "a", "2")])
        .unwrap();
    assert_eq!(
        target_of(&store.plan_at(root, "", early), "a"),
        TargetState::Unknown
    );
}

/// The watermark only covers what retention has actually forgotten, so a recent restore still
/// removes what it should.
#[test]
fn a_moment_newer_than_anything_trimmed_keeps_its_proof() {
    let dir = scratch("plan-recent");
    let (mut store, root) = open(&dir);
    store.commit(DAY, vec![baseline(root, "a", "1")]).unwrap();
    store
        .commit(2 * DAY, vec![modified(root, "a", "2")])
        .unwrap();
    store
        .commit(9 * DAY, vec![modified(root, "a", "3")])
        .unwrap();
    let recent = store.head(root, "a").unwrap().id;
    store
        .commit(10 * DAY, vec![created(root, "new", "x")])
        .unwrap();
    store
        .compact(
            &Policy {
                days_to_keep: 2,
                ..keep_everything()
            },
            &day_of,
        )
        .unwrap();
    assert_eq!(
        target_of(&store.plan_at(root, "", recent), "new"),
        TargetState::Absent
    );
}
