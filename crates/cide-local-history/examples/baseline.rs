//! What a first scan costs: time, peak memory, disk, reopening and compaction.
//!
//! The store's defaults rest on these numbers, so they are measured rather than assumed, on a
//! synthetic tree in a scratch directory. No application state and no live project is touched.
//!
//! ```sh
//! cargo run --release --locked -p cide-local-history --example baseline -- <scratch> [files]
//! ```
//!
//! Content is this repository's own source, cycled with a unique line per copy: real text, so the
//! compression ratio means something, and unique, so deduplication does not flatter it.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use cide_local_history::{Entry, MAX_ROWS, Policy, Store, VersionKind, VersionSource};

/// `worker.rs`' batch limits, so this exercises the shape the app does.
const BATCH_FILES: usize = 256;
const BATCH_BYTES: usize = 8 << 20;

fn sources(dir: &Path, out: &mut Vec<Vec<u8>>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name != "target" && name != "node_modules" && name != "dist" {
                sources(&path, out);
            }
        } else if path
            .extension()
            .is_some_and(|ext| ext == "rs" || ext == "ts" || ext == "tsx" || ext == "css")
            && let Ok(bytes) = fs::read(&path)
        {
            out.push(bytes);
        }
    }
}

fn peak_rss_mib() -> f64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            let line = status.lines().find(|line| line.starts_with("VmHWM:"))?;
            line.split_whitespace().nth(1)?.parse::<f64>().ok()
        })
        .map_or(0.0, |kib| kib / 1024.0)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let scratch = PathBuf::from(args.next().expect("a scratch directory"));
    let files: usize = args.next().map_or(100_000, |n| n.parse().unwrap());
    let dir = scratch.join("store");
    let _ = fs::remove_dir_all(&dir);

    let mut corpus = Vec::new();
    sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
        &mut corpus,
    );
    assert!(!corpus.is_empty(), "no source files found to cycle");
    let corpus_bytes: usize = corpus.iter().map(Vec::len).sum();
    println!(
        "corpus: {} files, {:.1} MiB; simulating {files} files",
        corpus.len(),
        corpus_bytes as f64 / 1_048_576.0
    );

    let started = Instant::now();
    let mut store = Store::open(&dir).unwrap();
    let root = store.ensure_root("/synthetic").unwrap();
    let mut batch = Vec::new();
    let mut pending = 0;
    let mut original = 0_u64;
    for i in 0..files {
        let mut bytes = format!("// {i}\n").into_bytes();
        bytes.extend_from_slice(&corpus[i % corpus.len()]);
        original += bytes.len() as u64;
        pending += bytes.len();
        batch.push(
            Entry::new(
                root,
                format!("src/m{}/f{i}.rs", i % 512),
                VersionKind::Baseline,
                VersionSource::Detected,
            )
            .content(bytes, 0o100644, None),
        );
        if batch.len() >= BATCH_FILES || pending >= BATCH_BYTES {
            store.commit(1, std::mem::take(&mut batch)).unwrap();
            pending = 0;
        }
    }
    store.commit(1, batch).unwrap();
    let scan = started.elapsed();
    let stats = store.stats();
    println!(
        "baseline: {:.1} s, {:.0} files/s, {:.1} MiB/s of source",
        scan.as_secs_f64(),
        files as f64 / scan.as_secs_f64(),
        original as f64 / 1_048_576.0 / scan.as_secs_f64()
    );
    println!(
        "disk: {:.1} MiB source -> {:.1} MiB packs ({:.0}%) + {:.1} MiB log",
        original as f64 / 1_048_576.0,
        stats.disk_bytes as f64 / 1_048_576.0,
        stats.disk_bytes as f64 / original as f64 * 100.0,
        stats.log_bytes as f64 / 1_048_576.0,
    );
    println!("peak RSS after baseline: {:.1} MiB", peak_rss_mib());
    drop(store);

    let started = Instant::now();
    let mut store = Store::open(&dir).unwrap();
    println!(
        "reopen: {:.2} s for {} rows, peak RSS {:.1} MiB",
        started.elapsed().as_secs_f64(),
        store.stats().rows,
        peak_rss_mib()
    );

    let started = Instant::now();
    let root = store.root_id("/synthetic").unwrap();
    let heads = store.heads(root);
    println!(
        "heads: {:.2} s for {}",
        started.elapsed().as_secs_f64(),
        heads.len()
    );

    let started = Instant::now();
    store
        .compact(
            &Policy {
                days_to_keep: 5,
                max_storage: u64::MAX,
                max_rows: MAX_ROWS,
            },
            &|_| 0,
        )
        .unwrap();
    println!(
        "compaction with nothing to drop: {:.2} s",
        started.elapsed().as_secs_f64()
    );

    let started = Instant::now();
    for i in 0..100 {
        store
            .commit(
                2,
                vec![
                    Entry::new(
                        root,
                        "src/edited.rs",
                        VersionKind::Modified,
                        VersionSource::External,
                    )
                    .content(
                        format!("edit {i}\n").into_bytes(),
                        0o100644,
                        None,
                    ),
                ],
            )
            .unwrap();
    }
    println!(
        "one-file commit: {:.2} ms each",
        started.elapsed().as_secs_f64() * 10.0
    );
    drop(store);
    fs::remove_dir_all(&dir).unwrap();
}
