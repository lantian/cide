//! Lane assignment for the commit graph: the pure half, and deliberately all of it. (M19)
//!
//! # Why this takes no `Repository`
//!
//! Every answer here is a function of `(oid, parents, dangling)` plus the state carried down
//! from the row above. Nothing opens an object, reads a ref or touches a file, and the only
//! thing borrowed from `git2` is [`Oid`] — kept because it is the key the walker already holds,
//! where a `String` would cost a 40-byte allocation and a hex parse per parent per row.
//!
//! The split is the one [`crate::patch::choose`] makes against [`crate::diff`], and it exists
//! for the same reason: it is what makes the algorithm drivable from a synthetic DAG. The
//! property the whole design rests on — *a history walked in one page of 500 and in five pages
//! of 100 produces byte-identical rows* — is only worth something asserted over hundreds of
//! generated histories, and generating hundreds of real repositories to get them costs minutes
//! per run. Over a synthetic DAG it costs milliseconds, so `tests/lanes.rs` asserts it on every
//! `cargo test` instead of behind `#[ignore]`.
//!
//! # Lanes are never compacted, and that is the load-bearing decision
//!
//! When a column's lane closes, the column is left empty and reused by the *next* lane that
//! opens; neighbours are never shifted left to close the hole. The alternative — compaction,
//! which is what makes a graph look tidiest — would mean a line that merely passes through a
//! row can *change column between two rows*, and a segment model able to say that needs a
//! diagonal case: "enters at column 4 above, leaves at column 2 below". That case is what
//! forces the renderer to know the row before and the row after in order to draw the row it has,
//! which a virtualised list scrolled a thousand rows down does not.
//!
//! Without compaction a crossing line enters and leaves in the same column, so every row is
//! *self-contained*: [`GraphEdge::Pass`] is a straight vertical, and the only diagonals are the
//! [`GraphEdge::Enter`]/[`GraphEdge::Exit`] pairs at this row's own dot. That is why there are
//! four edge variants and not eight, and why the renderer can stay stateless.
//!
//! The cost is a graph one or two columns wider than gitk's in a history that opens and closes
//! branches constantly. That is the trade, and it was taken knowingly.
//!
//! # Colour belongs to the lane, not to the column
//!
//! A closed column is reused immediately, and the lane that takes it mints a *fresh* colour. So
//! one line keeps one colour along its whole length — which is the only thing a colour is for —
//! while a column changes colour whenever one branch ends there and another begins. Colouring
//! by column instead would give every branch that ever passes through column 3 the same colour
//! and make two adjacent unrelated branches indistinguishable.
//!
//! **First arrival wins, always.** When a second child reaches a parent whose lane another child
//! already opened, it joins that column and keeps that column's colour, even when the second
//! child is the mainline and the first was a side branch. Re-pointing the lane to the mainline's
//! colour would be prettier and is forbidden: the rows that already drew that lane are on
//! screen, and a "load more" that recolours them is the exact symptom
//! [`cide_ipc::history::LaneResume::next_color`] exists to prevent.
//!
//! # Two children of one parent converge
//!
//! A parent already awaited by an open lane never opens a second one — both children draw an
//! `Exit` into the same column at different heights. This is what gitk and IDEA draw, and it is
//! also what keeps a repository where forty branches were cut from one base from opening forty
//! lanes that all wait for the same commit.
//!
//! # The cap: drawn width versus tracked width
//!
//! `cap` ([`cide_ipc::history::LogQuery::graph_lanes`]) is the width of the gutter, and it is a
//! width the user can change between one page and the next. It is therefore **not** the limit on
//! what the walker tracks: a lane that exists but does not fit is still followed, and is
//! summarised into one [`GraphEdge::Bundle`] in the last drawn column rather than being drawn.
//! Three consequences, each of which is the point:
//!
//! * no edge and no dot ever names a column `>= cap`, so the gutter's width is exactly `cap`
//!   and a renderer never has to clip;
//! * a `LogRefs::All` page over three hundred branches puts `cap` passes and one bundle on each
//!   row instead of three hundred passes;
//! * the *structure* survives — a parent past the cap still converges with its other children,
//!   still closes when its commit is drawn, and still keeps its colour — so shrinking the cap
//!   narrows the picture without rewiring the graph.
//!
//! What does bound tracking is [`LANES_MAX`], the largest `cap` anyone may ask for. Past it a
//! parent is refused: its `Exit` is drawn dangling into the last column, the row is marked
//! [`GraphRow::overflow`], and the parent is *not* tracked, so it later reappears as a fresh
//! head. That is a visible discontinuity, and it is the honest one — the alternative is a lane
//! set that grows with the repository, and a [`LaneResume`] token that grows with it on every
//! page of the wire.
//!
//! # This module never emits a `Bundle` for pruned commits
//!
//! [`GraphEdge::Bundle`] carries two meanings on the wire: the overflow summary above, and
//! *`count` commits were simplified away in this lane* (the visual form of
//! `CommitRow::pruned`). Only the first is produced here — this module is not told about
//! simplification and could not count it. A caller that adds the second must append it after
//! the edges [`Lanes::row`] returns, so the ordering guarantee below still holds.
//!
//! # Edge order is fixed
//!
//! `Enter`, then `Pass` ascending by lane, then the overflow `Bundle`, then `Exit` in parent
//! order. Fixed because the paging property is asserted with `==` on `Vec<GraphRow>`: an order
//! that depended on a `HashMap`'s iteration would make the assertion flap, and a renderer that
//! sorted the edges itself would be a second copy of a rule that is free to state once here.

use std::collections::HashMap;

use cide_ipc::history::{GraphEdge, GraphRow, LaneResume, OpenLaneWire};
use git2::Oid;

/// Lanes drawn when the caller does not say. Sixteen columns is about a screen's worth of
/// gutter at the sizes the log renders, and wider than all but the busiest week of a real
/// history.
pub const LANES_DEFAULT: u16 = 16;

/// The largest `cap` that may be asked for, and so also the ceiling on tracked lanes.
///
/// One constant for both, deliberately: because tracking is bounded by the largest *possible*
/// cap rather than by the cap in force, lowering the cap between two pages can never lose a
/// lane the previous page opened — it only stops drawing it. If these were two numbers, the
/// one page where they disagreed would drop lanes at the boundary and the rows after it would
/// silently differ from the same rows fetched in one go, which is the one thing the paging
/// property forbids.
pub const LANES_MAX: u16 = 64;

/// How many colours the renderer's palette has.
///
/// Small on purpose: these are theme colours that must stay distinguishable against both
/// backgrounds and from each other, and past about eight the next one is always somebody's
/// existing one at a slightly different lightness. Lanes past the eighth therefore repeat a
/// colour, which is fine — colour disambiguates *neighbouring* lines, and two lines twenty
/// columns apart sharing a colour confuses nobody.
pub const PALETTE: u16 = 8;

/// One column that is open: some commit already drawn has a parent that will be drawn here.
///
/// The richer twin of [`OpenLaneWire`], as that type's own header promises. Richer in the two
/// ways that matter inside the walk and not on the wire: `awaits` is a parsed [`Oid`] rather
/// than hex, so the per-row lookup is a hash of twenty bytes and not a parse; and the column is
/// *not* a field, because it is the index this value is stored at. A column field would be a
/// second answer to "which column is this", and the first time the two disagreed the graph
/// would draw a line into a column that closes somewhere else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenLane {
    /// The commit this lane is waiting for. The row that draws it is the row that closes this
    /// lane.
    pub awaits: Oid,
    /// Palette index, always `< PALETTE`.
    pub color: u16,
}

/// The lane assigner for one walk, or for one page of one walk.
///
/// Constructed with [`Lanes::resume`], fed one [`Lanes::row`] per commit **newest first and
/// children before parents** — the order the walk already produces — and closed with
/// [`Lanes::finish`], whose [`LaneResume`] the next page resumes from.
///
/// Feeding a commit before one of its children is not checked for and does not panic: the
/// commit simply reads as a fresh head and its child, arriving later, finds no lane to close.
/// A check would cost a set of every oid ever emitted, which is precisely the unbounded state
/// the paged design exists to avoid.
#[derive(Debug, Clone)]
pub struct Lanes {
    /// Columns left to right; `None` is a hole waiting to be reused. Indices `>= cap` are
    /// tracked but never drawn — see the module header.
    columns: Vec<Option<OpenLane>>,
    /// The reverse index of `columns`, by the oid each lane awaits.
    ///
    /// A map and not a scan, because it is consulted once per row plus once per parent and the
    /// column count is not the interesting bound — the *row* count is, and a scan would make
    /// the walk quadratic in the width of the graph exactly on the histories that are widest.
    /// It is also what makes convergence a lookup rather than a search.
    by_oid: HashMap<Oid, usize>,
    /// The round-robin cursor, used only once every palette entry is taken. Carried across a
    /// page boundary so the rotation continues instead of restarting.
    next_color: u16,
    /// Drawn width. Clamped into `1..=LANES_MAX` by [`Lanes::resume`], so `cap - 1` below is
    /// always a real column.
    cap: u16,
    /// The widest column any row has actually used, plus one. See [`Lanes::width`].
    width: u16,
    /// Any row overflowed. Sticky for the page — see [`Lanes::overflowed`].
    overflow: bool,
}

impl Lanes {
    /// Continue a walk, or start one.
    ///
    /// `state` is the previous page's [`LaneResume`]; `None` starts fresh, which is also what a
    /// re-rooted page does (`GraphOff::Rerooted` covers the case where starting fresh would be
    /// misleading rather than merely narrow).
    ///
    /// A token is client-held and therefore forgeable, so every field of it is treated as a
    /// suggestion: an unparseable oid, a column past [`LANES_MAX`], a column already claimed and
    /// a second lane awaiting an oid another lane already awaits are each dropped rather than
    /// rejected. Dropping loses at worst one line's continuity — the awaited commit reappears as
    /// a head — where refusing the whole token loses the user's scroll position, and neither is
    /// reachable from a token this crate minted.
    pub fn resume(state: Option<&LaneResume>, cap: u16) -> Self {
        let mut lanes = Self {
            columns: Vec::new(),
            by_oid: HashMap::new(),
            next_color: 0,
            cap: cap.clamp(1, LANES_MAX),
            width: 0,
            overflow: false,
        };
        let Some(state) = state else {
            return lanes;
        };
        lanes.next_color = state.next_color % PALETTE;
        for wire in &state.lanes {
            if wire.column >= LANES_MAX {
                continue;
            }
            let Ok(awaits) = Oid::from_str(&wire.awaits) else {
                continue;
            };
            if lanes.by_oid.contains_key(&awaits) {
                continue;
            }
            let column = usize::from(wire.column);
            if column >= lanes.columns.len() {
                lanes.columns.resize(column + 1, None);
            }
            if lanes.columns[column].is_some() {
                continue;
            }
            // `% PALETTE` rather than a rejection: a colour out of range only mis-colours a
            // line, and the modulo keeps `mint`'s `[bool; PALETTE]` bookkeeping sound.
            lanes.columns[column] = Some(OpenLane {
                awaits,
                color: wire.color % PALETTE,
            });
            lanes.by_oid.insert(awaits, column);
        }
        lanes
    }

    /// The row for one commit, and the lane bookkeeping it implies.
    ///
    /// `dangling[i]` says the caller knows parent `i` will never be drawn — it is past a shallow
    /// clone's floor, past a `scan_limit`, or simply below the last row of the last page. Such a
    /// parent gets a stub that fades out instead of a lane, because a lane opened for it would
    /// be passed on every remaining row of the page and then carried into the next page's token,
    /// waiting for a commit that is never coming. A short `dangling` reads as `false`, so a
    /// caller that has nothing to say can pass `&[]`.
    ///
    /// A merge is `>= 2` exits and a root is no exits; neither is signalled, because both are
    /// derivable and a second copy of a fact is a chance for the two to disagree.
    pub fn row(&mut self, oid: Oid, parents: &[Oid], dangling: &[bool]) -> GraphRow {
        let cap = usize::from(self.cap);
        // `cap >= 1` after the clamp in `resume`, so the last drawn column always exists. It is
        // where everything that does not fit is drawn: the bundle, an overflowing dot, and every
        // edge whose true column is off the right-hand side.
        let last = cap - 1;
        let mut edges = Vec::with_capacity(parents.len() + 4);
        let mut overflow = false;

        // 1. The home column: where this commit's dot sits.
        //
        // A lane already awaiting this oid gives both the column and the colour; the line
        // arriving from above is that lane, so this is the row's one `Enter`. No lane means the
        // commit is a head — nothing drawn so far has it as a parent — and a head is entered
        // from nowhere, so it allocates a column, mints a colour and emits no `Enter`.
        let (home, color) = match self.by_oid.remove(&oid) {
            Some(column) => {
                let color = self.columns[column]
                    .as_ref()
                    .expect("by_oid and columns are written together")
                    .color;
                if column < cap {
                    edges.push(GraphEdge::Enter {
                        top: column as u16,
                        color,
                    });
                } else {
                    // The line came out of the bundle. An `Enter` from `last` would claim it was
                    // the lane drawn there, which is a different branch; the dot lands in the
                    // bundle column with `overflow` set and no incoming edge, which is the same
                    // "several lines are here" the bundle already says.
                    overflow = true;
                }
                (Some(column), color)
            }
            None => {
                let color = self.mint();
                match self.free_column(&[]) {
                    Some(column) => (Some(column), color),
                    // Every column up to `LANES_MAX` is taken. The commit still has to be drawn
                    // somewhere, so its dot goes in the last column and its parents dangle.
                    None => {
                        overflow = true;
                        (None, color)
                    }
                }
            }
        };

        // 2. Every *other* open column passes straight through this row. Ascending, which is
        //    both the natural order of a `Vec` and the order the equality assertion needs.
        let mut bundled: u16 = 0;
        for (column, lane) in self.columns.iter().enumerate() {
            let Some(lane) = lane else { continue };
            if Some(column) == home {
                continue;
            }
            if column < cap {
                edges.push(GraphEdge::Pass {
                    lane: column as u16,
                    color: lane.color,
                });
            } else {
                bundled = bundled.saturating_add(1);
            }
        }
        if bundled > 0 {
            edges.push(GraphEdge::Bundle {
                lane: last as u16,
                count: bundled,
            });
            overflow = true;
        }

        // 3. Free the home column *before* the parents are placed. It is either retaken by the
        //    first parent in step 4 — which is what draws a mainline as one unbroken column —
        //    or it genuinely closes here and becomes the hole the next new lane fills.
        if let Some(column) = home
            && let Some(slot) = self.columns.get_mut(column)
        {
            *slot = None;
        }

        // 4. The parents, in git's order, each producing exactly one `Exit`.
        //
        // `reserved` holds columns claimed by *stubs* this row: a dangling parent occupies no
        // lane, but its fading edge is drawn into a column, and a later parent taking that same
        // column would put two arrows on one line. It is a `Vec` and searched linearly because
        // it holds at most `parents.len()` entries and an octopus merge with more than a handful
        // of parents does not exist outside a test.
        let mut reserved: Vec<usize> = Vec::new();
        for (index, &parent) in parents.iter().enumerate() {
            let cut = dangling.get(index).copied().unwrap_or(false);

            // Convergence, and it wins over `cut`: whatever the caller believes about
            // reachability, a lane for this parent is already open and has to be joined, or it
            // is passed on every remaining row waiting for a commit nobody will draw.
            if let Some(&column) = self.by_oid.get(&parent) {
                let color = self.columns[column]
                    .as_ref()
                    .expect("by_oid and columns are written together")
                    .color;
                // `faded`, not `dangling`, only so the slice parameter of that name stays
                // readable through the rest of the loop body.
                let (bottom, faded) = self.draw(column, last, &mut overflow);
                edges.push(GraphEdge::Exit {
                    bottom,
                    color,
                    dangling: faded,
                });
                continue;
            }

            // The first parent takes the home column and this commit's own colour. That single
            // rule is what makes a mainline one straight line of one colour: every commit on it
            // hands its column and colour down to its first parent, for as long as the branch
            // lasts. Later parents are side branches — leftmost free column, fresh colour.
            let (column, color) = if index == 0 {
                (home.or_else(|| self.free_column(&reserved)), color)
            } else {
                (self.free_column(&reserved), self.mint())
            };

            match column {
                // A parent the caller says will never be drawn: draw the stub, open nothing.
                Some(column) if cut => {
                    reserved.push(column);
                    let (bottom, _) = self.draw(column, last, &mut overflow);
                    edges.push(GraphEdge::Exit {
                        bottom,
                        color,
                        dangling: true,
                    });
                }
                Some(column) => {
                    if column >= self.columns.len() {
                        self.columns.resize(column + 1, None);
                    }
                    self.columns[column] = Some(OpenLane {
                        awaits: parent,
                        color,
                    });
                    self.by_oid.insert(parent, column);
                    let (bottom, faded) = self.draw(column, last, &mut overflow);
                    edges.push(GraphEdge::Exit {
                        bottom,
                        color,
                        dangling: faded,
                    });
                }
                // Nothing free below `LANES_MAX`. The parent is dropped rather than tracked, so
                // it will be drawn later as a head with no line into it. A discontinuity the
                // reader can see beats a lane set that grows without bound — and `overflow` on
                // the row is what lets the panel say so.
                None => {
                    overflow = true;
                    edges.push(GraphEdge::Exit {
                        bottom: last as u16,
                        color,
                        dangling: true,
                    });
                }
            }
        }

        // The dot itself. A home column past the cap — a head that had to be opened out there,
        // or a lane the previous page opened while the gutter was wider — is drawn in the
        // bundle column with `overflow` set, which is the same answer the bundle gives for the
        // lines it summarises: *several things are here and one of them is this*.
        let lane = match home {
            Some(column) if column < cap => column as u16,
            _ => {
                overflow = true;
                last as u16
            }
        };
        let mut used = lane + 1;
        for edge in &edges {
            used = used.max(
                match *edge {
                    GraphEdge::Pass { lane, .. }
                    | GraphEdge::Enter { top: lane, .. }
                    | GraphEdge::Exit { bottom: lane, .. }
                    | GraphEdge::Bundle { lane, .. } => lane,
                } + 1,
            );
        }
        self.width = self.width.max(used);
        self.overflow |= overflow;

        GraphRow {
            lane,
            color,
            edges,
            overflow,
        }
    }

    /// The widest column any row used, plus one: the gutter's width in columns.
    ///
    /// A running maximum over the page rather than the current number of open lanes, because
    /// the gutter is sized once for the whole list — one that resized as the list scrolled would
    /// move every commit summary sideways. Never greater than `cap`, since no edge and no dot
    /// ever names a column past it.
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Any row so far needed more lanes than `cap` allowed.
    ///
    /// Sticky, and the page-level mirror of [`GraphRow::overflow`]: the panel shows one notice
    /// for the page instead of scanning every row to discover whether to.
    pub fn overflowed(&self) -> bool {
        self.overflow
    }

    /// The continuation the next page resumes from.
    ///
    /// Consumes `self` because a page's lanes are finished when its rows are: reusing the
    /// assigner after taking its state is how a second page would be produced from a first
    /// page's columns and quietly draw the same commits twice.
    ///
    /// Includes the lanes past `cap` — they are tracked, so they have to survive, and
    /// [`Lanes::resume`] accepts columns up to [`LANES_MAX`] for exactly that reason. Ascending
    /// by column, so the token is stable byte for byte between two runs that saw the same
    /// history.
    pub fn finish(self) -> LaneResume {
        LaneResume {
            lanes: self
                .columns
                .iter()
                .enumerate()
                .filter_map(|(column, lane)| {
                    lane.as_ref().map(|lane| OpenLaneWire {
                        column: column as u16,
                        awaits: lane.awaits.to_string(),
                        color: lane.color,
                    })
                })
                .collect(),
            next_color: self.next_color,
        }
    }

    /// Where an edge into `column` is actually drawn, and whether it fades out.
    ///
    /// A column past the cap is not on screen, so an edge into it is drawn into the bundle and
    /// marked `dangling`: from the renderer's side it does not reach a dot it can see, which is
    /// what `dangling` means and what a fading segment says. Marking it also sets the row's
    /// `overflow`, so the fade reads as *there is more here* rather than as *this history stops*.
    fn draw(&self, column: usize, last: usize, overflow: &mut bool) -> (u16, bool) {
        if column < usize::from(self.cap) {
            (column as u16, false)
        } else {
            *overflow = true;
            (last as u16, true)
        }
    }

    /// The leftmost column that is neither open nor claimed by a stub this row, up to
    /// [`LANES_MAX`].
    ///
    /// Leftmost, not next-to-the-right: reusing holes is what keeps the graph narrow given that
    /// nothing is ever compacted. Scanning from zero every time is `O(width)` per lane opened,
    /// with width at most 64 — a free-list would be faster and would also have to be carried in
    /// the resume token to keep two runs identical, which is a lot of wire for sixty-four
    /// comparisons.
    fn free_column(&self, reserved: &[usize]) -> Option<usize> {
        (0..usize::from(LANES_MAX)).find(|column| {
            !matches!(self.columns.get(*column), Some(Some(_))) && !reserved.contains(column)
        })
    }

    /// A colour for a lane that is about to open.
    ///
    /// The smallest palette index no open lane is using, so neighbouring branches differ; when
    /// every index is taken, the round-robin cursor, so the repeat at least moves around instead
    /// of always landing on colour zero.
    ///
    /// Both halves read only carried state — the open columns and `next_color`, which are
    /// exactly what [`LaneResume`] holds. That is what makes the answer identical whether or not
    /// a page boundary falls immediately before this row, and it is the reason the cursor is on
    /// the wire at all.
    fn mint(&mut self) -> u16 {
        let mut used = [false; PALETTE as usize];
        for lane in self.columns.iter().flatten() {
            used[usize::from(lane.color % PALETTE)] = true;
        }
        match used.iter().position(|taken| !taken) {
            Some(color) => color as u16,
            None => {
                let color = self.next_color % PALETTE;
                self.next_color = (color + 1) % PALETTE;
                color
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A distinct, stable oid per index. The bytes are arbitrary; only distinctness and
    /// determinism matter, and encoding the index means a failing assertion names the commit.
    fn oid(index: u32) -> Oid {
        let mut bytes = [0u8; 20];
        bytes[0..4].copy_from_slice(&index.to_be_bytes());
        // The tail keeps the low bytes from being all-zero, so a bug that truncates an oid to
        // eight bytes cannot pass by accident.
        bytes[16..20].copy_from_slice(&(index ^ 0x5a5a_5a5a).to_be_bytes());
        Oid::from_bytes(&bytes).expect("20 bytes is an oid")
    }

    fn exits(row: &GraphRow) -> Vec<(u16, u16, bool)> {
        row.edges
            .iter()
            .filter_map(|edge| match *edge {
                GraphEdge::Exit {
                    bottom,
                    color,
                    dangling,
                } => Some((bottom, color, dangling)),
                _ => None,
            })
            .collect()
    }

    fn passes(row: &GraphRow) -> Vec<(u16, u16)> {
        row.edges
            .iter()
            .filter_map(|edge| match *edge {
                GraphEdge::Pass { lane, color } => Some((lane, color)),
                _ => None,
            })
            .collect()
    }

    fn enters(row: &GraphRow) -> Vec<(u16, u16)> {
        row.edges
            .iter()
            .filter_map(|edge| match *edge {
                GraphEdge::Enter { top, color } => Some((top, color)),
                _ => None,
            })
            .collect()
    }

    fn bundles(row: &GraphRow) -> Vec<(u16, u16)> {
        row.edges
            .iter()
            .filter_map(|edge| match *edge {
                GraphEdge::Bundle { lane, count } => Some((lane, count)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn lanes_linear_history_is_one_column_of_one_colour() {
        let mut lanes = Lanes::resume(None, LANES_DEFAULT);
        let rows: Vec<GraphRow> = (0..6)
            .map(|index| lanes.row(oid(index), &[oid(index + 1)], &[]))
            .collect();

        for (index, row) in rows.iter().enumerate() {
            assert_eq!(row.lane, 0, "row {index}");
            assert_eq!(row.color, 0, "row {index}");
            assert!(!row.overflow);
            assert!(passes(row).is_empty(), "row {index} has no other lane");
            assert_eq!(exits(row), vec![(0, 0, false)], "row {index}");
            // Only the head is entered from nowhere.
            assert_eq!(enters(row).len(), usize::from(index > 0), "row {index}");
        }
        assert_eq!(lanes.width(), 1);
        assert!(!lanes.overflowed());
    }

    #[test]
    fn lanes_root_commit_emits_no_exit() {
        let mut lanes = Lanes::resume(None, LANES_DEFAULT);
        let child = lanes.row(oid(0), &[oid(1)], &[]);
        let root = lanes.row(oid(1), &[], &[]);

        assert_eq!(exits(&child).len(), 1);
        assert!(exits(&root).is_empty(), "a root has no parent to leave to");
        assert_eq!(enters(&root), vec![(0, 0)]);
        // The column closed with the root, so the token carries nothing.
        assert!(lanes.finish().lanes.is_empty());
    }

    #[test]
    fn lanes_octopus_merge_widens_by_three() {
        let mut lanes = Lanes::resume(None, LANES_DEFAULT);
        let parents = [oid(1), oid(2), oid(3), oid(4)];
        let row = lanes.row(oid(0), &parents, &[]);

        assert!(enters(&row).is_empty(), "the merge is this walk's head");
        let exits = exits(&row);
        assert_eq!(exits.len(), 4, "one exit per parent, in parent order");
        assert_eq!(exits[0], (0, row.color, false), "first parent keeps home");
        assert_eq!(exits[1].0, 1);
        assert_eq!(exits[2].0, 2);
        assert_eq!(exits[3].0, 3);
        // Every side branch gets a colour of its own.
        let colors: Vec<u16> = exits.iter().map(|exit| exit.1).collect();
        assert_eq!(colors, vec![0, 1, 2, 3]);
        assert_eq!(lanes.width(), 4, "one column plus three");
    }

    #[test]
    fn lanes_two_children_of_one_parent_converge() {
        // Two heads, one shared parent. The second child must join the first child's column
        // rather than open a lane of its own, and must not repaint it.
        let mut lanes = Lanes::resume(None, LANES_DEFAULT);
        let first = lanes.row(oid(0), &[oid(9)], &[]);
        let second = lanes.row(oid(1), &[oid(9)], &[]);
        let parent = lanes.row(oid(9), &[], &[]);

        assert_eq!(exits(&first), vec![(0, first.color, false)]);
        assert_eq!(
            exits(&second),
            vec![(0, first.color, false)],
            "first arrival wins: the side branch's column and its colour"
        );
        assert_ne!(second.lane, 0, "the second head still needs its own dot");
        assert_eq!(lanes.by_oid.len(), 0, "one lane opened, and it closed");
        assert_eq!(enters(&parent), vec![(0, first.color)]);
    }

    #[test]
    fn lanes_dangling_parent_opens_no_lane() {
        let mut lanes = Lanes::resume(None, LANES_DEFAULT);
        let row = lanes.row(oid(0), &[oid(1), oid(2)], &[true, true]);

        let exits = exits(&row);
        assert_eq!(exits.len(), 2, "both parents are still drawn");
        assert!(exits.iter().all(|exit| exit.2), "both fade out");
        assert_ne!(exits[0].0, exits[1].0, "two stubs do not share a column");
        assert!(!row.overflow, "dangling is not overflow");
        assert!(
            lanes.finish().lanes.is_empty(),
            "nothing is carried for a commit that will never be drawn"
        );
    }

    #[test]
    fn lanes_freed_column_is_reused_not_compacted() {
        // Three heads side by side; the middle one ends. The next new lane must take the hole
        // at column 1 rather than the neighbours shuffling left.
        let mut lanes = Lanes::resume(None, LANES_DEFAULT);
        lanes.row(oid(0), &[oid(10)], &[]);
        lanes.row(oid(1), &[oid(11)], &[]);
        lanes.row(oid(2), &[oid(12)], &[]);
        let ended = lanes.row(oid(11), &[], &[]);
        let fresh = lanes.row(oid(3), &[oid(13)], &[]);

        assert_eq!(ended.lane, 1);
        assert_eq!(fresh.lane, 1, "the hole is refilled, nothing shifted");
        // The neighbours are exactly where they were: compaction would have moved the lane at
        // column 2 to column 1 and made its `Pass` a diagonal no row can describe on its own.
        assert_eq!(
            passes(&fresh),
            vec![(0, 0), (2, 2)],
            "columns 0 and 2 pass straight through, unmoved and unrecoloured"
        );
        // It really is a new lane in that column and not the old one revived.
        let state = lanes.finish();
        assert_eq!(state.lanes[1].column, 1);
        assert_eq!(state.lanes[1].awaits, oid(13).to_string());
    }

    #[test]
    fn lanes_overflow_bundles_the_rest_and_never_names_a_column_past_the_cap() {
        const CAP: u16 = 8;
        const BRANCHES: u32 = 40;

        let mut lanes = Lanes::resume(None, CAP);
        let mut rows = Vec::new();
        // Forty heads, each with one parent, so forty lanes are open at once.
        for index in 0..BRANCHES {
            rows.push(lanes.row(oid(index), &[oid(1000 + index)], &[]));
        }

        for (index, row) in rows.iter().enumerate() {
            assert!(row.lane < CAP, "row {index} dot is inside the gutter");
            for edge in &row.edges {
                let column = match *edge {
                    GraphEdge::Pass { lane, .. }
                    | GraphEdge::Enter { top: lane, .. }
                    | GraphEdge::Exit { bottom: lane, .. }
                    | GraphEdge::Bundle { lane, .. } => lane,
                };
                assert!(
                    column < CAP,
                    "row {index} edge {edge:?} is inside the gutter"
                );
            }
        }
        assert!(lanes.overflowed());
        assert_eq!(lanes.width(), CAP);

        // The last row sees seven drawn neighbours and everything else in one bundle, and the
        // two together account for every lane that is open.
        let last = rows.last().expect("forty rows");
        assert!(last.overflow);
        let open = BRANCHES - 1; // every earlier head's lane, the row's own excluded
        let drawn = passes(last).len() as u32;
        assert_eq!(bundles(last), vec![(CAP - 1, (open - drawn) as u16)]);
        assert_eq!(drawn + u32::from(bundles(last)[0].1), open);

        // Tracking survives the cap, which is the whole point of bundling rather than dropping:
        // every one of the forty parents is still awaited and comes back in the token.
        assert_eq!(lanes.finish().lanes.len(), BRANCHES as usize);
    }

    #[test]
    fn lanes_past_the_hard_ceiling_are_dropped_not_tracked() {
        // `LANES_MAX` concurrent branches fill every trackable column; the next parent has
        // nowhere to go and is refused rather than growing the state.
        let mut lanes = Lanes::resume(None, LANES_MAX);
        for index in 0..u32::from(LANES_MAX) {
            lanes.row(oid(index), &[oid(1000 + index)], &[]);
        }
        let refused = lanes.row(oid(500), &[oid(501)], &[]);

        assert!(refused.overflow);
        assert_eq!(refused.lane, LANES_MAX - 1);
        assert_eq!(exits(&refused), vec![(LANES_MAX - 1, refused.color, true)]);
        let state = lanes.finish();
        assert_eq!(state.lanes.len(), usize::from(LANES_MAX));
        assert!(
            state
                .lanes
                .iter()
                .all(|lane| lane.awaits != oid(501).to_string()),
            "the refused parent is not tracked; it reappears later as a head"
        );
    }

    #[test]
    fn lanes_resume_round_trips() {
        let mut lanes = Lanes::resume(None, LANES_DEFAULT);
        lanes.row(oid(0), &[oid(10)], &[]);
        lanes.row(oid(1), &[oid(11), oid(12)], &[]);
        let state = lanes.finish();

        let resumed = Lanes::resume(Some(&state), LANES_DEFAULT).finish();
        assert_eq!(
            resumed, state,
            "the token is a fixed point of resume/finish"
        );
        assert_eq!(
            state
                .lanes
                .iter()
                .map(|lane| lane.column)
                .collect::<Vec<_>>(),
            vec![0, 1, 2],
            "ascending by column"
        );
    }

    #[test]
    fn lanes_resume_ignores_a_forged_token() {
        let state = LaneResume {
            lanes: vec![
                OpenLaneWire {
                    column: 0,
                    awaits: "not an oid".into(),
                    color: 0,
                },
                OpenLaneWire {
                    column: LANES_MAX + 5,
                    awaits: oid(1).to_string(),
                    color: 1,
                },
                OpenLaneWire {
                    column: 2,
                    awaits: oid(2).to_string(),
                    color: 99,
                },
                OpenLaneWire {
                    column: 2,
                    awaits: oid(3).to_string(),
                    color: 1,
                },
                OpenLaneWire {
                    column: 4,
                    awaits: oid(2).to_string(),
                    color: 1,
                },
            ],
            next_color: 4000,
        };
        let kept = Lanes::resume(Some(&state), LANES_DEFAULT).finish();

        assert_eq!(kept.lanes.len(), 1, "only the one usable lane survives");
        assert_eq!(kept.lanes[0].column, 2);
        assert_eq!(kept.lanes[0].color, 99 % PALETTE);
        assert!(kept.next_color < PALETTE);
    }

    #[test]
    fn lanes_cap_may_shrink_between_pages_without_losing_structure() {
        // Page one is drawn wide: four branches, four columns.
        let mut wide = Lanes::resume(None, LANES_DEFAULT);
        for index in 0..4 {
            wide.row(oid(index), &[oid(100 + index)], &[]);
        }
        let state = wide.finish();

        // Page two is drawn narrow. The two lanes that no longer fit are bundled, not dropped —
        // and when one of their commits arrives it still closes the right lane in the right
        // colour, which is the thing dropping them would break.
        let mut narrow = Lanes::resume(Some(&state), 2);
        let row = narrow.row(oid(103), &[], &[]);

        assert_eq!(
            bundles(&row),
            vec![(1, 1)],
            "column 2 is the other hidden lane"
        );
        assert_eq!(row.lane, 1, "the dot lands in the bundle column");
        assert!(row.overflow);
        assert_eq!(row.color, state.lanes[3].color, "its colour is unchanged");
        assert!(
            enters(&row).is_empty(),
            "no edge is drawn out of the bundle"
        );
    }

    #[test]
    fn lanes_colour_is_stable_along_a_line_across_a_page_boundary() {
        // Two branches, so the second one's colour is not simply zero. The page boundary falls
        // in the middle of both lines.
        let history: Vec<(Oid, Vec<Oid>)> = vec![
            (oid(0), vec![oid(2)]),
            (oid(1), vec![oid(3)]),
            (oid(2), vec![oid(4)]),
            (oid(3), vec![oid(5)]),
            (oid(4), vec![]),
            (oid(5), vec![]),
        ];

        let mut whole = Lanes::resume(None, LANES_DEFAULT);
        let one: Vec<GraphRow> = history
            .iter()
            .map(|(oid, parents)| whole.row(*oid, parents, &[]))
            .collect();

        let mut first = Lanes::resume(None, LANES_DEFAULT);
        let mut split: Vec<GraphRow> = history[..3]
            .iter()
            .map(|(oid, parents)| first.row(*oid, parents, &[]))
            .collect();
        let mut second = Lanes::resume(Some(&first.finish()), LANES_DEFAULT);
        split.extend(
            history[3..]
                .iter()
                .map(|(oid, parents)| second.row(*oid, parents, &[])),
        );

        assert_eq!(one, split);
        assert_eq!(one[0].color, one[2].color, "one line, one colour");
        assert_eq!(one[1].color, one[3].color);
        assert_ne!(one[0].color, one[1].color, "two lines, two colours");
    }

    #[test]
    fn lanes_edges_are_ordered_enter_pass_bundle_exit() {
        let mut lanes = Lanes::resume(None, 2);
        // Three open lanes at a cap of two: column 0 and 1 are drawn, column 2 is bundled.
        lanes.row(oid(0), &[oid(10)], &[]);
        lanes.row(oid(1), &[oid(11)], &[]);
        lanes.row(oid(2), &[oid(12)], &[]);
        let row = lanes.row(oid(10), &[oid(20), oid(21)], &[]);

        let kinds: Vec<&'static str> = row
            .edges
            .iter()
            .map(|edge| match edge {
                GraphEdge::Enter { .. } => "enter",
                GraphEdge::Pass { .. } => "pass",
                GraphEdge::Bundle { .. } => "bundle",
                GraphEdge::Exit { .. } => "exit",
            })
            .collect();
        assert_eq!(kinds, vec!["enter", "pass", "bundle", "exit", "exit"]);
    }

    #[test]
    fn lanes_cap_is_clamped() {
        assert_eq!(
            Lanes::resume(None, 0).cap,
            1,
            "a zero-wide gutter has no last column"
        );
        assert_eq!(Lanes::resume(None, u16::MAX).cap, LANES_MAX);
        // A cap of one still draws, and says so: the second head has nowhere of its own to sit,
        // so its dot lands on top of the only column there is and the row is marked.
        let mut lanes = Lanes::resume(None, 1);
        lanes.row(oid(0), &[oid(10)], &[]);
        let crowded = lanes.row(oid(1), &[oid(11)], &[]);
        assert_eq!(crowded.lane, 0);
        assert!(crowded.overflow);
        assert_eq!(passes(&crowded), vec![(0, 0)]);
        assert_eq!(
            exits(&crowded),
            vec![(0, crowded.color, true)],
            "its parent's lane is off-screen, so the edge fades"
        );
        // A third head is bundled rather than drawn, because by then two lanes are open.
        let third = lanes.row(oid(2), &[oid(12)], &[]);
        assert_eq!(bundles(&third), vec![(0, 1)]);
    }
}
