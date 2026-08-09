//! A real repository-sized tree on disk, walked by the real walker, feeding the real picker.
//!
//! `tests/streaming.rs` shows the seam works at 20 000 files. This file is about the two
//! claims that only a *known* corpus can settle:
//!
//! * **The counter is exact.** `matched` / `total` are the numbers behind the overlay's
//!   `50 of 500` readout. A corpus whose file count, directory count and matching count are
//!   decided before anything is written is the only way to assert them rather than assert
//!   they are self-consistent.
//! * **100 000 files really stream.** The synthetic 100k test in `cide-search` pushes
//!   candidates straight into the injector and never walks anything, so it cannot show that
//!   `Index::build`'s sink runs early enough on a tree that size. This one walks the inodes.
//!
//! Two sizes on purpose. The 6 000-file streaming test and the 500-file counter test run in
//! the default suite in about 30 ms between them on an idle machine; the 100 000-file one is
//! `#[ignore]`d for the inodes it writes rather than for the clock, and is run with
//! `cargo test -p cide-fs --test large_repo -- --ignored --nocapture`. A test nobody runs
//! would not be worth much on its own, which is why the small pair carries both properties
//! and the big one exists to show they survive the scale.
//!
//! Nothing here touches a path outside [`cide_fs::testing::scratch`], and the tree is
//! removed when the `Scratch` drops — including on a panic, since `Drop` runs while
//! unwinding.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use cide_fs::testing::{Scratch, scratch};
use cide_fs::{BuildOptions, Index, Root, WalkItem};
use cide_search::{Candidate, Matcher, NucleoMatcher};

/// The fuzzy needle. `q` and `z` appear nowhere else in the corpus — not in `filler`, not
/// in `pkg`, not in `src`, not in `.rs` — so a query for it matches the needle files and
/// nothing else. Fuzzy matching is subsequence matching, so an exact `matched` assertion is
/// only defensible if the filler cannot accidentally spell the needle out.
const NEEDLE: &str = "zqneedle";

/// A tree whose counts were decided before it was written.
struct Corpus {
    dir: Scratch,
    /// Files the walk should report — excludes everything under the ignored and hidden
    /// directories, and the `.gitignore` itself.
    files: u32,
    /// Directories likewise.
    dirs: u32,
    /// Files whose name contains [`NEEDLE`].
    needles: u32,
}

impl Corpus {
    fn root(&self) -> Root {
        Root::new(self.dir.path())
    }
}

/// Write `packages` × `per_package` files under a fresh scratch directory.
///
/// Every `needle_every`-th file is named for [`NEEDLE`]. Two subtrees are planted that the
/// walk must *not* report — a gitignored one and a dotted one — because a counter that is
/// exact only on a corpus with nothing to exclude is not exact on a real repository.
///
/// Creation is threaded because at 100 000 files the `create` syscalls are the test's whole
/// cost — the walk they exist for is a fifth of it.
fn corpus(tag: &str, packages: usize, per_package: usize, needle_every: usize) -> Corpus {
    let dir = scratch(tag);

    std::fs::write(dir.join(".gitignore"), "ignored/\n").expect("write .gitignore");
    for excluded in ["ignored", ".hidden"] {
        let sub = dir.join(excluded).join("nested");
        std::fs::create_dir_all(&sub).expect("create an excluded directory");
        for f in 0..10 {
            std::fs::write(sub.join(format!("{NEEDLE}_excluded{f}.rs")), []).expect("write");
        }
    }

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(packages.max(1));
    let per_thread = packages.div_ceil(threads);
    std::thread::scope(|scope| {
        for chunk in 0..threads {
            let base: &Path = dir.path();
            scope.spawn(move || {
                let start = chunk * per_thread;
                for p in start..(start + per_thread).min(packages) {
                    let sub = base.join(format!("pkg{p:05}/src"));
                    std::fs::create_dir_all(&sub).expect("create a package directory");
                    for f in 0..per_package {
                        let n = p * per_package + f;
                        let name = if n.is_multiple_of(needle_every) {
                            format!("{NEEDLE}{n}.rs")
                        } else {
                            format!("filler{n}.rs")
                        };
                        std::fs::write(sub.join(name), []).expect("write a corpus file");
                    }
                }
            });
        }
    });

    let files = packages * per_package;
    Corpus {
        dir,
        files: files as u32,
        // `pkgN` and `pkgN/src`. The excluded subtrees contribute nothing: `ignore` never
        // descends into them, so neither the directories nor their contents are reported.
        dirs: (packages * 2) as u32,
        needles: files.div_ceil(needle_every) as u32,
    }
}

/// What the picker looked like at one moment during the walk.
#[derive(Debug, Clone, Copy)]
struct Sample {
    at: Duration,
    matched: u32,
    total: u32,
}

/// One walk, and everything observed while it ran.
struct Walk {
    index: Index,
    matcher: Arc<NucleoMatcher>,
    injected: u32,
    samples: Vec<Sample>,
    elapsed: Duration,
}

impl Walk {
    /// Frames answered from an index the walk had not finished filling.
    ///
    /// `matched > 0 && total < files` is the whole proof: `total` is how many candidates the
    /// matcher has ever seen, so a frame reporting fewer than the corpus was taken while the
    /// walk was still injecting — and `matched > 0` says the picker was already useful. This
    /// is checked against the counts rather than against a flag the walking thread flips,
    /// because the counts cannot lie about the ordering and a flag can.
    fn partial_frames(&self, files: u32) -> Vec<Sample> {
        self.samples
            .iter()
            .copied()
            .filter(|s| s.matched > 0 && s.total < files)
            .collect()
    }
}

/// Walk `corpus` repeatedly until one attempt catches the picker answering from a partial
/// index, and return that attempt.
///
/// The retry is for the machine, not for the code. `nucleo` scores on its own thread pool,
/// and on a box whose cores are all busy — a CI runner with `-j32`, or another test binary
/// beside this one — that pool can go unscheduled for the entire walk, so every frame reads
/// `(0, 0)` and the run observes nothing rather than observing a failure. Measured on a
/// 32-core box under 2x oversubscription, about a quarter of attempts see nothing; five
/// independent attempts put that at roughly one run in a thousand. A regression that made
/// the picker wait for the walk fails all five, every time, because there is no window for
/// it to slip through.
fn walk_until_partial(corpus: &Corpus, opts: BuildOptions, attempts: usize) -> Walk {
    let mut last = None;
    for _ in 0..attempts {
        let walk = walk_while_polling(corpus, opts);
        if !walk.partial_frames(corpus.files).is_empty() {
            return walk;
        }
        last = Some(walk);
    }
    let walk = last.expect("at least one attempt");
    let seen: Vec<(u32, u32)> = walk.samples.iter().map(|s| (s.matched, s.total)).collect();
    panic!(
        "in {attempts} walks the picker never matched against a partial index — every frame \
         was empty or complete, which proves nothing about streaming. last run's samples \
         (matched, total): {seen:?}"
    );
}

/// Walk `corpus` with [`NEEDLE`] standing as the query, sampling the picker throughout.
///
/// The query is set *before* the walk starts, which is the real sequence: the overlay opens
/// on a project that has just been registered and starts asking immediately.
fn walk_while_polling(corpus: &Corpus, opts: BuildOptions) -> Walk {
    let matcher = Arc::new(NucleoMatcher::new());
    let injected = Arc::new(AtomicU32::new(0));
    matcher.query(NEEDLE);

    let walking = Arc::new(AtomicBool::new(true));
    let observer = {
        let matcher = Arc::clone(&matcher);
        let walking = Arc::clone(&walking);
        let started = Instant::now();
        std::thread::spawn(move || {
            let mut samples = Vec::new();
            // Sampling continues for one extra pass after the flag drops so the finished
            // state is on the record too; `frame` is the only way to advance the worker.
            loop {
                let running = walking.load(Ordering::Acquire);
                let frame = matcher.frame(20);
                samples.push(Sample {
                    at: started.elapsed(),
                    matched: frame.matched,
                    total: frame.total,
                });
                if !running {
                    return samples;
                }
                // The sleep is not politeness. `nucleo` matches on its own thread pool, and
                // a poll loop with no yield on a box whose cores are already taken by the
                // walk starves it: every frame then reports `(0, 0)` until the walk ends,
                // and the test fails claiming there was no streaming. It is also what the
                // overlay does — it polls at frame rate, not in a spin.
                std::thread::sleep(Duration::from_millis(1));
            }
        })
    };

    let started = Instant::now();
    let index = Index::build(vec![corpus.root()], opts, &|batch: &[WalkItem]| {
        for item in batch.iter().filter(|i| !i.is_dir) {
            matcher.push(Candidate::new(
                item.rel.clone(),
                item.path.to_string_lossy().into_owned(),
            ));
            injected.fetch_add(1, Ordering::Relaxed);
        }
    });
    let elapsed = started.elapsed();
    walking.store(false, Ordering::Release);

    Walk {
        index,
        injected: injected.load(Ordering::Relaxed),
        samples: observer.join().expect("observer thread"),
        elapsed,
        matcher,
    }
}

/// Poll until the worker settles, so a count assertion is about matching and not timing.
fn settle(matcher: &NucleoMatcher) -> cide_ipc::PickerFrame {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let frame = matcher.frame(20);
        if !frame.running || Instant::now() > deadline {
            return frame;
        }
    }
}

/// The counts the overlay prints, against a corpus whose answer was known before the walk.
///
/// Deliberately small: exactness has nothing to do with scale, and a second big walk in this
/// binary would only contend with the streaming test the harness runs beside it.
#[test]
fn the_counter_is_exact_after_a_real_walk() {
    let corpus = corpus("counts", 20, 25, 10);
    let walk = walk_while_polling(&corpus, BuildOptions::default());
    let matcher = &walk.matcher;

    assert_eq!(walk.index.files(), corpus.files, "files the walk found");
    assert_eq!(walk.index.dirs(), corpus.dirs, "directories the walk found");
    assert_eq!(
        walk.injected, corpus.files,
        "every walked file reached the injector exactly once"
    );

    let frame = settle(matcher);
    assert_eq!(
        frame.total, corpus.files,
        "`total` is the corpus, with the gitignored and dotted subtrees excluded"
    );
    assert_eq!(
        frame.matched, corpus.needles,
        "`matched` is exactly the needle files — no filler matched, and none of the \
         {NEEDLE} files planted under `ignored/` or `.hidden/` leaked in"
    );
    assert!(frame.items.iter().all(|row| row.text.contains(NEEDLE)));

    // An empty query matches everything, which is the state the overlay opens in.
    matcher.query("");
    let all = settle(matcher);
    assert_eq!(all.matched, corpus.files);
    assert_eq!(all.total, corpus.files);

    // And a query that matches nothing still reports the whole corpus as `total`.
    matcher.query("qqqqqqzzzzzz");
    let none = settle(matcher);
    assert_eq!(none.matched, 0);
    assert_eq!(none.total, corpus.files);
}

/// The picker answers from a partially-walked index — the property the streaming injector
/// exists for — in the default suite.
///
/// Two threads, not the default one-per-core. On a 32-core box a 6 000-file tree in tmpfs is
/// walked in about the time one `frame` spends in `Nucleo::tick`, so the test becomes a race
/// with the thing it is measuring rather than a proof of it. The alternative was to keep the
/// default thread count and write ten times the files, which is what the `#[ignore]`d test
/// below does; capping the walk's parallelism buys the same window for a tenth of the I/O.
/// Nothing about the ordering being asserted depends on how many cores the walk gets.
#[test]
fn the_picker_matches_before_a_six_thousand_file_walk_finishes() {
    let corpus = corpus("stream-6k", 60, 100, 10);
    let opts = BuildOptions {
        threads: 2,
        ..BuildOptions::default()
    };
    let walk = walk_until_partial(&corpus, opts, 5);

    let partial = walk.partial_frames(corpus.files);
    let first = partial[0];
    assert!(first.matched > 0 && first.total < corpus.files);

    // The walk that produced the partial frame also produced the right answer, so a picker
    // that streamed early by dropping entries would not pass on the strength of the frame.
    assert_eq!(walk.index.files(), corpus.files);
    assert_eq!(walk.injected, corpus.files);
    assert_eq!(settle(&walk.matcher).matched, corpus.needles);
}

/// The milestone's headline: 100 000 real files on disk, and Ctrl+P works during the walk.
///
/// `#[ignore]`d for the inodes rather than for the clock. Where `TMPDIR` is tmpfs the whole
/// test is about half a second; where it is a real filesystem, 100 000 `create` calls and
/// the `unlink` storm behind them are tens of seconds and a lot of journal traffic, which is
/// not a cost every `cargo test` should pay. Run it with
/// `cargo test -p cide-fs --test large_repo -- --ignored --nocapture`; the timings it prints
/// are the point of running it by hand.
#[test]
#[ignore = "writes and removes 100k inodes: cheap on tmpfs, tens of seconds on a real disk"]
fn a_hundred_thousand_real_files_stream_into_the_picker() {
    const PACKAGES: usize = 1_000;
    const PER_PACKAGE: usize = 100;

    let started = Instant::now();
    let corpus = corpus("stream-100k", PACKAGES, PER_PACKAGE, 10);
    let generated = started.elapsed();
    assert_eq!(corpus.files, 100_000);

    let walk = walk_until_partial(&corpus, BuildOptions::default(), 5);
    let partial = walk.partial_frames(corpus.files);
    let first = partial[0];
    println!(
        "100k: generated in {generated:?}, built in {:?}, first non-empty frame at {:?} \
         ({} of {} files injected), {} partial frames of {} samples",
        walk.elapsed,
        first.at,
        first.total,
        corpus.files,
        partial.len(),
        walk.samples.len(),
    );

    // "Usable *before* indexing finishes" is only interesting if "before" means near the
    // start. Both bounds are ~20x looser than what this machine does idle (first match at
    // 3–7 ms with 20–400 of the 100 000 files injected, against a 220–290 ms build), which
    // leaves room for a busy machine while still catching a picker that had to wait for a
    // substantial part of the walk.
    assert!(
        first.total < corpus.files / 20,
        "the first useful frame needed {} of {} files — the picker is waiting on the walk",
        first.total,
        corpus.files
    );
    assert!(
        first.at * 10 < walk.elapsed,
        "the first useful frame took {:?} of a {:?} build",
        first.at,
        walk.elapsed
    );
    // The index filled progressively rather than arriving in one lump at the end. Only
    // asserted at this size: 100 000 files give the observer a couple of hundred samples to
    // catch it in, where 6 000 on tmpfs give it a handful.
    assert!(
        partial.len() >= 2,
        "one partial frame in {} samples",
        walk.samples.len()
    );

    assert_eq!(walk.index.files(), corpus.files);
    assert_eq!(walk.index.dirs(), corpus.dirs);
    assert_eq!(walk.injected, corpus.files);

    let frame = settle(&walk.matcher);
    assert_eq!(frame.total, corpus.files);
    assert_eq!(frame.matched, corpus.needles);
    // The frame stays small however large the repository gets: what crosses IPC — and so
    // what the frontend allocates per keystroke — is capped by the caller's limit, not by
    // the 10 000 rows that matched.
    assert_eq!(frame.items.len(), 20);
}
