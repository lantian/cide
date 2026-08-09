//! The two halves composed: a real walk feeding a real picker.
//!
//! `cide-search` has its own test that 100k injected candidates are matchable before
//! injection finishes. This one is about the seam — that [`Index::build`]'s sink really does
//! run during the walk and really does reach the matcher — which is the part a unit test on
//! either side cannot show.
//!
//! 20 000 files rather than 100 000: creating them is the slow part of the test, and the
//! property being demonstrated does not get truer with five times as many inodes.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use cide_fs::testing::scratch;
use cide_fs::{BuildOptions, Index, Root, WalkItem};
use cide_search::{Candidate, Matcher, NucleoMatcher};

const DIRS: usize = 200;
const PER_DIR: usize = 100;

#[test]
fn the_picker_matches_while_the_walk_is_still_running() {
    let dir = scratch("stream-e2e");
    for d in 0..DIRS {
        let sub = dir.join(format!("crate{d}/src"));
        std::fs::create_dir_all(&sub).unwrap();
        for f in 0..PER_DIR {
            std::fs::write(sub.join(format!("needle{d}_{f}.rs")), "").unwrap();
        }
    }

    let matcher = Arc::new(NucleoMatcher::new());
    matcher.query("needle");

    let injected = Arc::new(AtomicU32::new(0));
    let walking = Arc::new(AtomicBool::new(true));

    let observer = {
        let matcher = Arc::clone(&matcher);
        let walking = Arc::clone(&walking);
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(60);
            while Instant::now() < deadline {
                // Matches are checked before the walk flag, so a frame taken in the instant
                // the walk finishes still counts. The proof is `total`, asserted below: a
                // frame scored against a partial index is one the walk had not finished
                // filling.
                let frame = matcher.frame(20);
                if frame.matched > 0 {
                    return Some(frame);
                }
                if !walking.load(Ordering::Acquire) {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            None
        })
    };

    let sink_matcher = Arc::clone(&matcher);
    let sink_count = Arc::clone(&injected);
    let index = Index::build(
        vec![Root::new(dir.path())],
        BuildOptions::default(),
        &|batch: &[WalkItem]| {
            for item in batch.iter().filter(|i| !i.is_dir) {
                sink_matcher.push(Candidate::new(
                    item.rel.clone(),
                    item.path.to_string_lossy().into_owned(),
                ));
                sink_count.fetch_add(1, Ordering::Relaxed);
            }
        },
    );
    walking.store(false, Ordering::Release);

    let early = observer
        .join()
        .unwrap()
        .expect("the picker should have matched before the walk finished");
    assert!(early.matched > 0);
    assert!(
        early.total < (DIRS * PER_DIR) as u32,
        "matched against a complete index ({}), which proves nothing about streaming",
        early.total
    );
    assert!(early.items.len() <= 20, "frames stay capped");
    assert!(early.items[0].text.contains("needle"));

    assert_eq!(index.files() as usize, DIRS * PER_DIR);
    assert_eq!(injected.load(Ordering::Relaxed) as usize, DIRS * PER_DIR);
}
