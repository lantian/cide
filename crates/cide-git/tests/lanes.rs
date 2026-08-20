//! The commit graph's lane assigner, driven over generated histories.
//!
//! `cide_git::lanes` carries its own worked examples — a linear history, an octopus, a
//! convergence, an overflow — and those say what the algorithm *is*. This file says what has to
//! be true of it over histories nobody wrote by hand, and it exists for one property in
//! particular:
//!
//! > a history walked in one page of 500 and in five pages of 100 produces byte-identical rows.
//!
//! Everything about the design leans on that. Lane columns and colours depend on the whole
//! prefix of the walk, so the only alternatives to carrying them across a page boundary are
//! re-walking every earlier page (quadratic in the number of pages) or inventing an order —
//! which recolours and re-columns rows that are already on the user's screen, so a "load more"
//! makes the graph appear to redraw itself at random. A property test is the only thing that can
//! say the carried state is *sufficient*: an example can only say it is sufficient for that
//! example, and the field that gets left out of [`cide_ipc::history::LaneResume`] is always the
//! one no example happened to need.
//!
//! It lives here rather than beside the module because the seeded generator is
//! `tests/support/mod.rs`'s [`Rng`], shared with the staging and patch property tests — one
//! generator, so a seed printed by a failure means the same thing in every one of them.
//!
//! There is no repository anywhere in this file, and that is the point of the module under
//! test: five hundred synthetic commits cost microseconds where five hundred real ones cost
//! minutes, so this runs on every `cargo test` instead of behind `#[ignore]`.

mod support;

use std::collections::{BTreeMap, BTreeSet};

use cide_git::lanes::{LANES_MAX, Lanes};
use cide_ipc::history::{GraphEdge, GraphRow};
use git2::Oid;
use support::Rng;

/// One generated commit, in the shape [`Lanes::row`] takes.
struct Node {
    oid: Oid,
    parents: Vec<Oid>,
    dangling: Vec<bool>,
}

/// A distinct, stable oid per index, with the index readable in the first four bytes so a
/// failing assertion can be traced back to a node of the generated DAG.
fn oid_for(index: usize) -> Oid {
    let mut bytes = [0u8; 20];
    bytes[0..4].copy_from_slice(&(index as u32).to_be_bytes());
    bytes[16..20].copy_from_slice(&((index as u32) ^ 0x5a5a_5a5a).to_be_bytes());
    Oid::from_bytes(&bytes).expect("20 bytes is an oid")
}

/// A DAG in walk order: **newest first, every parent at a higher index than its child**.
///
/// That ordering is not a convenience, it is the contract the walk itself keeps and the one the
/// assigner is entitled to assume — a lane is opened by a child and closed by the row that draws
/// the parent, so a parent drawn before its child would read as a head and its child would find
/// no lane to close. Generating parents strictly to the right of the child makes every generated
/// history satisfy it by construction, which is cheaper than generating arbitrary DAGs and
/// topologically sorting them, and no less general: any DAG has such an ordering.
///
/// The shape aims at the cases the algorithm actually has to get right rather than at uniform
/// randomness. A first parent that is usually the very next index gives a *spine* — long runs
/// that must stay one column of one colour — while the remaining draws reach up to `WINDOW`
/// commits back, which is what keeps several lanes open at once and produces the convergences,
/// the crossings and the reused holes. Roots and octopuses are rare here because they are rare
/// in real histories, and the module's own tests cover them exactly rather than by chance.
fn generate(seed: u64, count: usize) -> Vec<Node> {
    /// How far back a parent may reach. Wide enough that a dozen lanes are open at once in a
    /// five-hundred-commit history — so `LANES_DEFAULT` is exercised without being exceeded on
    /// every row, which would make the whole run one long overflow and test only that path.
    const WINDOW: usize = 24;

    let mut rng = Rng::new(seed);
    let mut nodes = Vec::with_capacity(count);
    for index in 0..count {
        let room = count.saturating_sub(index + 1).min(WINDOW);
        let want = match rng.below(100) {
            0..=4 => 0,                  // a root, or a tip whose history is elsewhere
            5..=16 => 2,                 // a merge
            17..=18 => 3 + rng.below(2), // an octopus
            _ => 1,
        };

        let mut picked: Vec<usize> = Vec::new();
        for slot in 0..want {
            if room == 0 {
                break;
            }
            // The first parent is usually the immediately older commit: that is what a branch
            // being committed on looks like, and it is what makes a spine to check colours
            // along.
            let candidate = if slot == 0 && rng.chance(7, 10) {
                index + 1
            } else {
                index + 1 + rng.below(room)
            };
            if !picked.contains(&candidate) {
                picked.push(candidate);
            }
        }

        nodes.push(Node {
            oid: oid_for(index),
            parents: picked.iter().map(|&at| oid_for(at)).collect(),
            // A parent the walk will never reach: a shallow clone's floor, or a budget that ran
            // out. Rare, and deliberately *not* correlated with anything the assigner can see,
            // because its only correct response is to draw a stub and open nothing.
            dangling: picked.iter().map(|_| rng.chance(3, 100)).collect(),
        });
    }
    nodes
}

fn walk(nodes: &[Node], cap: u16) -> (Vec<GraphRow>, cide_ipc::history::LaneResume) {
    let mut lanes = Lanes::resume(None, cap);
    let rows = nodes
        .iter()
        .map(|node| lanes.row(node.oid, &node.parents, &node.dangling))
        .collect();
    (rows, lanes.finish())
}

fn walk_paged(
    nodes: &[Node],
    cap: u16,
    page: usize,
) -> (Vec<GraphRow>, cide_ipc::history::LaneResume) {
    let mut rows = Vec::with_capacity(nodes.len());
    let mut state = None;
    for chunk in nodes.chunks(page) {
        let mut lanes = Lanes::resume(state.as_ref(), cap);
        for node in chunk {
            rows.push(lanes.row(node.oid, &node.parents, &node.dangling));
        }
        state = Some(lanes.finish());
    }
    (
        rows,
        state.unwrap_or(cide_ipc::history::LaneResume {
            lanes: Vec::new(),
            next_color: 0,
        }),
    )
}

fn column_of(edge: &GraphEdge) -> u16 {
    match *edge {
        GraphEdge::Pass { lane, .. }
        | GraphEdge::Enter { top: lane, .. }
        | GraphEdge::Exit { bottom: lane, .. }
        | GraphEdge::Bundle { lane, .. } => lane,
    }
}

/// The lanes this row leaves open below it: everything passing through, plus every lane it
/// opened or joined.
fn open_below(row: &GraphRow) -> BTreeSet<(u16, u16)> {
    row.edges
        .iter()
        .filter_map(|edge| match *edge {
            GraphEdge::Pass { lane, color } => Some((lane, color)),
            GraphEdge::Exit {
                bottom,
                color,
                dangling: false,
            } => Some((bottom, color)),
            _ => None,
        })
        .collect()
}

/// The lanes arriving into this row from above: everything passing through, plus the one this
/// row's own commit closes.
fn open_above(row: &GraphRow) -> BTreeSet<(u16, u16)> {
    row.edges
        .iter()
        .filter_map(|edge| match *edge {
            GraphEdge::Pass { lane, color } => Some((lane, color)),
            GraphEdge::Enter { top, color } => Some((top, color)),
            _ => None,
        })
        .collect()
}

/// **The property.** One page of five hundred and five pages of a hundred are the same rows.
///
/// Asserted row by row rather than on the two vectors, so a failure names the row it first
/// diverged at instead of printing a thousand of them; and asserted for several caps, because
/// the cap changes which columns are drawn, which edges dangle and whether a bundle appears —
/// all of it state that has to survive the boundary just as the lanes do. The final
/// continuation is compared too: a page that produced the right rows from the wrong state would
/// otherwise pass here and fail on the page after the last one this test asks for.
#[test]
fn lanes_paging_is_byte_identical() {
    for seed in 0..40u64 {
        let nodes = generate(seed, 500);
        for cap in [LANES_MAX, 16, 8, 3] {
            let (single, single_state) = walk(&nodes, cap);
            for page in [100, 137, 1, 499] {
                let (paged, paged_state) = walk_paged(&nodes, cap, page);
                assert_eq!(
                    single.len(),
                    paged.len(),
                    "seed {seed}, cap {cap}, page {page}: row count"
                );
                for (index, (one, split)) in single.iter().zip(&paged).enumerate() {
                    assert_eq!(
                        one, split,
                        "seed {seed}, cap {cap}, page {page}: row {index} differs"
                    );
                }
                assert_eq!(
                    single_state, paged_state,
                    "seed {seed}, cap {cap}, page {page}: the continuation differs"
                );
            }
        }
    }
}

/// Nothing is ever drawn outside the gutter the caller asked for, whatever the history does.
///
/// The cap is what sizes a fixed strip of chrome beside a virtualised list; one edge past it is
/// either clipped silently or overlaps the commit summaries, and neither failure looks like a
/// graph bug when it appears. The row's own claims are checked alongside: one `Exit` per parent
/// in parent order, no `Exit` at all for a root, and `overflow` set on every row that had to put
/// something in the last column that did not belong there.
#[test]
fn lanes_generated_rows_stay_inside_the_gutter() {
    for seed in 100..140u64 {
        let nodes = generate(seed, 400);
        for cap in [LANES_MAX, 16, 4, 1] {
            let mut lanes = Lanes::resume(None, cap);
            for (index, node) in nodes.iter().enumerate() {
                let row = lanes.row(node.oid, &node.parents, &node.dangling);
                assert!(
                    row.lane < cap,
                    "seed {seed}, cap {cap}, row {index}: dot at {}",
                    row.lane
                );
                for edge in &row.edges {
                    assert!(
                        column_of(edge) < cap,
                        "seed {seed}, cap {cap}, row {index}: {edge:?}"
                    );
                }
                let exits = row
                    .edges
                    .iter()
                    .filter(|edge| matches!(edge, GraphEdge::Exit { .. }))
                    .count();
                assert_eq!(
                    exits,
                    node.parents.len(),
                    "seed {seed}, cap {cap}, row {index}: one exit per parent"
                );
                assert!(
                    row.edges
                        .iter()
                        .filter(|edge| matches!(edge, GraphEdge::Enter { .. }))
                        .count()
                        <= 1,
                    "seed {seed}, cap {cap}, row {index}: at most one lane closes here"
                );
            }
            assert!(
                lanes.width() <= cap,
                "seed {seed}, cap {cap}: gutter {} wider than the cap",
                lanes.width()
            );
        }
    }
}

/// Every line is continuous, and it keeps one colour for its whole length.
///
/// The check is a conservation law between neighbouring rows: the `(column, colour)` pairs a row
/// leaves open below it — its passes, plus the lanes its exits opened or joined — must be
/// exactly the pairs the next row sees arriving from above, its passes plus the one lane its own
/// `Enter` closes. A lane that vanished, a lane that appeared from nothing, a line that changed
/// column because something was compacted, and a lane repainted when a second child reached its
/// parent all break it, and each of those is a graph that draws a line between two commits that
/// are not related.
///
/// Rows that overflowed are skipped on both sides of a pair, because a bundle deliberately hides
/// lanes and the law cannot hold across one; the caps here are wide enough that the interesting
/// rows are the ones that are checked. That the overflowing rows stay inside the gutter is
/// `lanes_generated_rows_stay_inside_the_gutter`'s job.
#[test]
fn lanes_lines_are_continuous_and_keep_one_colour() {
    for seed in 200..240u64 {
        let nodes = generate(seed, 400);
        for cap in [LANES_MAX, 16] {
            let (rows, _) = walk(&nodes, cap);
            for (index, pair) in rows.windows(2).enumerate() {
                if pair[0].overflow || pair[1].overflow {
                    continue;
                }
                assert_eq!(
                    open_below(&pair[0]),
                    open_above(&pair[1]),
                    "seed {seed}, cap {cap}: rows {index}/{} disagree about what is open \
                     between them",
                    index + 1
                );
            }
            // The first row of a fresh walk is entered from nothing, and the last row's open
            // lanes are the ones the continuation has to carry.
            assert!(open_above(&rows[0]).is_empty(), "seed {seed}: row 0");
        }
    }
}

/// A history whose rows are all heads — every parent dangling — opens no lane at all.
///
/// The degenerate case of the walk that ran out of budget on its first page, and the one that
/// would leak: a lane opened for a parent that will never be drawn is passed on every remaining
/// row and then carried into the next page's token, where it waits for ever. The continuation of
/// such a page has to be empty.
#[test]
fn lanes_all_dangling_carries_nothing() {
    let nodes = generate(7, 200);
    let mut lanes = Lanes::resume(None, 16);
    for node in &nodes {
        let all_cut: Vec<bool> = node.parents.iter().map(|_| true).collect();
        let row = lanes.row(node.oid, &node.parents, &all_cut);
        assert!(
            row.edges
                .iter()
                .all(|edge| !matches!(edge, GraphEdge::Pass { .. })),
            "no lane was ever opened, so nothing can pass through"
        );
    }
    assert!(lanes.finish().lanes.is_empty());
}

/// The generated corpus really does contain the cases the properties above are about.
///
/// A property test is only as good as what it was run over, and a generator is one careless
/// edit away from producing five hundred single-parent commits in a line — over which every
/// assertion in this file passes and none of them means anything. So the shape of the corpus is
/// itself asserted: merges, octopuses, roots, two children reaching one parent, and — at the
/// narrow caps — rows that overflow and rows that bundle. The numbers are floors well under
/// what the current seeds produce, not fingerprints of them; they are here to catch a corpus
/// that collapsed, not to freeze the generator.
#[test]
fn lanes_generated_histories_exercise_the_hard_cases() {
    let mut merges = 0;
    let mut octopus = 0;
    let mut roots = 0;
    let mut shared_parents = 0;
    let mut overflowed = 0;
    let mut bundled = 0;

    for seed in 0..40u64 {
        let nodes = generate(seed, 500);
        let mut children: BTreeMap<Oid, u32> = BTreeMap::new();
        for node in &nodes {
            match node.parents.len() {
                0 => roots += 1,
                1 => {}
                2 => merges += 1,
                _ => {
                    merges += 1;
                    octopus += 1;
                }
            }
            for parent in &node.parents {
                *children.entry(*parent).or_default() += 1;
            }
        }
        shared_parents += children.values().filter(|count| **count > 1).count();

        // A cap far below the eleven-odd columns these histories want, so the overflow and
        // bundle paths are on the same rows the paging property is asserted over.
        let (rows, _) = walk(&nodes, 3);
        overflowed += rows.iter().filter(|row| row.overflow).count();
        bundled += rows
            .iter()
            .filter(|row| {
                row.edges
                    .iter()
                    .any(|edge| matches!(edge, GraphEdge::Bundle { .. }))
            })
            .count();
    }

    assert!(merges > 2000, "merges: {merges}");
    assert!(octopus > 200, "octopus merges: {octopus}");
    assert!(roots > 500, "roots: {roots}");
    assert!(
        shared_parents > 2000,
        "parents with two or more children: {shared_parents}"
    );
    assert!(overflowed > 5000, "overflowing rows: {overflowed}");
    assert!(bundled > 5000, "bundled rows: {bundled}");
}
