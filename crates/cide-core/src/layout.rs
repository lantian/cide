//! Pure operations on a tab's [`PaneTree`].
//!
//! Every pane feature in the product — split, close, detach, re-dock, promote, maximize —
//! is expressed through one of these functions, so the tree invariants have to be upheld
//! in exactly one place. Nothing here does IO, spawns a process or knows what a session
//! is: moving a pane between tabs is [`take_pane`] followed by [`insert_pane`], and the
//! PTY behind the pane is untouched by the move.
//!
//! The invariants:
//!
//! * a [`LayoutNode::Split`] always has exactly two children — the type enforces that, and
//!   collapsing the parent into the surviving sibling on close is what keeps it true once
//!   panes start disappearing, which is why a tree of *n* panes has exactly *n - 1* splits,
//! * leaf ids are unique, and the set of them equals the key set of [`PaneTree::panes`],
//!   each entry keyed by its own [`Pane::id`],
//! * `focused` and `maximized` name live leaves,
//! * split ids are unique — [`set_ratio`] addresses a divider by id, so a duplicate would
//!   silently drag two dividers at once,
//! * every ratio is finite and within `[MIN_RATIO, MAX_RATIO]`,
//! * at most one leaf is [`PaneRole::Primary`] — "the primary pane" has to name one pane for
//!   [`primary_of`] and for every refusal message that says "the console's primary pane".
//!
//! [`validate`] checks every one of those the type system does not.
//!
//! # The primary pane, and why "a Primary leaf exists" is two halves
//!
//! M1 states the invariant as *a Primary leaf exists*. That cannot be a check inside
//! [`validate`], because most trees are supposed to have no Primary at all: only the pinned
//! console tab is seeded with one, and every ordinary tab — a file, a diff, a shell — is
//! built from `Auxiliary` panes, so a blanket existence check would reject the majority of
//! legal trees. The invariant is therefore established by construction and held by
//! preservation:
//!
//! * **existence** is [`new_tree`]'s caller's business, and [`validate_console`] is the
//!   assertion for the trees that must have one (the console's). It is deliberately a
//!   separate entry point rather than a flag on [`validate`], so a caller has to know which
//!   kind of tree it is holding — which is knowledge this module does not have.
//! * **preservation** is enforced here without exception: [`take_pane`] refuses a `Primary`
//!   pane outright, so no sequence of splits, closes, detaches or promotions can remove one
//!   from a tree that has it. Refusing only the *sole*-pane case, which is what this used to
//!   do, left `split the primary, close the primary` producing a console with no
//!   conversation in it and no complaint from [`validate`].
//!
//! Maximizing is the one operation that is *not* tree surgery: it sets a flag the renderer
//! honours, which keeps restore exact and costs no terminal reflow.
//!
//! # Chains: rows of tiles on top of a binary tree
//!
//! The tree stays binary — nothing above changes — but the *gestures* now speak in rows.
//! A **chain** is a maximal same-axis subtree; its **members** are the in-order leaves and
//! cross-axis subtrees hanging off it. A chain of `n` members has exactly `n - 1` interior
//! splits, and in-order traversal yields `member, split, member, …, member`, so **divider
//! `k` is the `k`-th interior split in in-order** and separates members `k` and `k + 1`.
//! No new id space is needed for that.
//!
//! The canonical shape the gesture surface builds is *a `Col` spine of rows, each row a
//! `Row` chain of tiles*. That is a convention held by [`add_row`] and [`add_tile`], not a
//! rule in [`validate`]: an ordinary file or diff tab legitimately holds any tree, and a
//! `validate` rule would reject files already on disk.
//!
//! [`weights`] turns a chain into each member's share (the product of the ratios on its
//! path), and [`write_weights`] is its exact inverse, writing every ratio bottom-up from a
//! share vector. That pair is what makes a divider edit local: change two adjacent shares
//! while preserving their sum and *every other member keeps its share bit for bit*,
//! whatever the tree shape underneath.
//!
//! ## Why `MIN_TILE == MIN_RATIO`, and why `validate` did not have to change
//!
//! The obvious objection to deriving ratios from shares is that a flattened chain seems to
//! need a looser clamp than the stored `[MIN_RATIO, MAX_RATIO]` band. It does not. For
//! every node of a chain, `ratio = W(a) / (W(a) + W(b))`. Given `W(a), W(b) >= MIN_TILE`
//! and `W(a) + W(b) <= 1`:
//!
//! ```text
//! ratio >= MIN_TILE / (W(a) + W(b)) >= MIN_TILE
//! ratio  = 1 - W(b) / (W(a) + W(b)) <= 1 - MIN_TILE
//! ```
//!
//! so every derived ratio lands inside `[MIN_TILE, 1 - MIN_TILE]`. Setting
//! `MIN_TILE = MIN_RATIO` therefore makes the derived band *exactly* today's band, for any
//! shape and any arity — [`validate`] is untouched, no persisted guarantee weakens, and the
//! frontend keeps mirroring two literals rather than recomputing a function of the arity.
//! Relaxing `MIN_RATIO` to 0.01 was the alternative and it loses: it weakens a documented
//! guarantee on a persisted field to buy nothing.
//!
//! The one cost is a hard cap of [`MAX_MEMBERS`] members per chain, since `n * MIN_TILE`
//! must fit in 1. [`add_tile`] and [`add_row`] refuse the eleventh explicitly rather than
//! producing a silently uneven layout.
//!
//! The clamp inside [`write_weights`] is retained only as a backstop for *legacy* chains
//! whose members are already below the floor — reachable today, because three nested splits
//! at 0.1 give a 0.1% leaf while every stored ratio is legal.

use std::collections::HashSet;

use cide_ipc::{
    Axis, Direction, DockAnchor, DockSibling, LayoutNode, MAX_RATIO, MIN_RATIO, Pane, PaneId,
    PaneRole, PaneTree, Side, SplitId,
};
use indexmap::IndexMap;

use crate::error::{CoreError, Result};

/// A tab's initial tree: one leaf, focused, nothing maximized.
pub fn new_tree(pane: Pane) -> PaneTree {
    let id = pane.id;
    let mut panes = IndexMap::new();
    panes.insert(id, pane);
    PaneTree {
        root: LayoutNode::Leaf { pane: id },
        focused: id,
        maximized: None,
        panes,
    }
}

/// Split `target` in two, putting `new_pane` on `side` of the new divider.
///
/// The target leaf becomes a fresh split whose children are the old pane and the new one.
/// The new pane takes focus, which is what every editor does and what makes
/// "split and start typing" one gesture rather than two.
///
/// Returns the id of the pane that was added.
pub fn split(
    tree: &mut PaneTree,
    target: PaneId,
    axis: Axis,
    side: Side,
    new_pane: Pane,
) -> Result<PaneId> {
    attach(tree, target, axis, side, new_pane)
}

/// Put an existing pane — one taken from another tab or window — beside `target`.
///
/// This is the re-dock half of [`take_pane`]; it is the same surgery as [`split`] and
/// differs only in where the pane came from, so a pane that is dragged out and dropped
/// back lands in exactly the shape a fresh split would have produced.
pub fn insert_pane(
    tree: &mut PaneTree,
    target: PaneId,
    axis: Axis,
    side: Side,
    pane: Pane,
) -> Result<PaneId> {
    attach(tree, target, axis, side, pane)
}

/// Describe where `pane` sits, so [`insert_pane_at`] can later put it back exactly.
///
/// Must be read **before** [`take_pane`]: taking the pane collapses the very split this
/// describes, and afterwards there is nothing left to look at.
///
/// `None` when the pane is the whole tree — it has no parent split, so there is no position
/// to remember. [`take_pane`] refuses that case anyway, so a caller pairing the two never
/// stores a `None` for a pane that actually left.
pub fn anchor_of(tree: &PaneTree, pane: PaneId) -> Option<DockAnchor> {
    find_parent(&tree.root, pane)
}

/// Whether [`insert_pane_at`] would restore `anchor` exactly, rather than erroring.
///
/// Separate from the insertion so a caller can choose a fallback instead of failing: an
/// anchor going stale is the normal cost of leaving a pane out while the tab moves on, not
/// an error anybody can act on.
pub fn can_restore(tree: &PaneTree, anchor: &DockAnchor) -> bool {
    contains_node(&tree.root, anchor.sibling) && !contains_split(&tree.root, anchor.split)
}

/// Put a pane back in the exact split it was detached from.
///
/// Rebuilds the parent split around `anchor.sibling` with the recorded axis, side, divider
/// id and ratio, which is what makes detach/re-dock a no-op on the tree rather than a
/// re-layout. Errors when the sibling is gone or the divider id is somehow live again; ask
/// [`can_restore`] first and fall back rather than treating that as a failure.
///
/// The ratio is clamped and NaN-guarded here rather than trusted: an anchor survives a quit
/// in `workspace.json`, and a hand-edited or corrupt file must not be able to write a ratio
/// that [`validate`] would then reject.
pub fn insert_pane_at(tree: &mut PaneTree, anchor: &DockAnchor, pane: Pane) -> Result<PaneId> {
    if tree.panes.contains_key(&pane.id) {
        return Err(CoreError::Invariant(format!(
            "pane {} is already in this tree",
            pane.id
        )));
    }
    // Split ids address dividers, and `validate` requires them unique. Reinstating one that
    // is somehow live again would put two dividers under one id and make `set_ratio` drag
    // both, so refuse instead — `can_restore` reports the same condition in advance.
    if contains_split(&tree.root, anchor.split) {
        return Err(CoreError::Invariant(format!(
            "split {} is already in this tree",
            anchor.split
        )));
    }

    let id = pane.id;
    let ratio = if anchor.ratio.is_finite() {
        anchor.ratio.clamp(MIN_RATIO, MAX_RATIO)
    } else {
        0.5
    };
    if !graft_at(&mut tree.root, anchor, ratio, id) {
        return Err(match anchor.sibling {
            DockSibling::Pane { pane } => CoreError::NoSuchPane(pane),
            DockSibling::Split { split } => CoreError::NoSuchSplit(split),
        });
    }

    tree.panes.insert(id, pane);
    // Same coupling as `attach`: the pane the user just dropped back takes focus, and a
    // maximized flag left set would hide it behind whatever was maximized.
    tree.focused = id;
    tree.maximized = None;
    Ok(id)
}

/// Remove a leaf, and give its room back to the whole row (or column) it was part of.
///
/// Refused for a [`PaneRole::Primary`] pane ([`CoreError::PanePrimary`]) and for the only
/// pane left in the tree ([`CoreError::LastPane`]): the pinned console always shows its
/// conversation, and a tab always has at least one pane. The two errors are distinct so the
/// caller can word the message.
///
/// The re-weighting is the exact inverse of [`add_tile`]/[`add_row`], and it is the whole
/// difference between this and [`take_pane`]. Tree surgery alone collapses the parent split
/// into the sibling, so the departing pane's width lands entirely on whichever neighbour
/// the *binary* tree happened to pair it with: close one of four equal tiles and the row
/// reads 50/25/25, with the shape of the tree — invisible to the user — deciding which
/// neighbour got the lot. Spreading it in proportion instead makes four-minus-one three
/// equal tiles, matching what adding a fourth did, and leaves a hand-tuned row's
/// proportions intact.
///
/// [`take_pane`] deliberately keeps the bare collapse: its callers — detach, promote — mean
/// to put the pane back, and [`insert_pane_at`] re-grafts against the *sibling*, so a
/// spread on the way out would not be undone on the way in and a detach/re-dock round trip
/// would no longer be the no-op it is documented to be.
pub fn close(tree: &mut PaneTree, pane: PaneId) -> Result<()> {
    // Measured before the surgery, and for the same reason `detach_pane` reads its anchor
    // first: `prune` collapses the split that says how much room the pane had, so asking
    // afterwards would find the shares already merged into the sibling.
    let plan = removal_plan(&tree.root, pane);
    take_pane(tree, pane)?;
    // Only after the removal succeeded — a refusal must leave the tree untouched, ratios
    // included.
    if let Some((path, axis, shares)) = plan {
        write_weights(node_at_mut(&mut tree.root, &path), axis, &shares);
    }
    Ok(())
}

/// [`close`], but hands back the removed [`Pane`] so it can be re-homed.
///
/// Detach and promote both need the pane itself, not merely its disappearance; dropping it
/// here and reconstructing it at the destination would lose the session binding.
///
/// Refuses a `Primary` pane for *any* caller, detach included. Letting detach through was
/// the alternative and it loses: the console's primary pane is the project's conversation,
/// and while it sat in a detached window the console tab would hold no `Primary` at all —
/// the M1 invariant broken for as long as the window is open, and permanently if the
/// re-dock anchor has gone stale by the time it comes back. One rule with no exception is
/// also what lets `close` be `take_pane` rather than a second removal path that has to be
/// kept in agreement with it.
pub fn take_pane(tree: &mut PaneTree, pane: PaneId) -> Result<Pane> {
    let Some(role) = tree.panes.get(&pane).map(|p| p.role) else {
        return Err(CoreError::NoSuchPane(pane));
    };

    // Checked before the sole-leaf guard so a one-pane console reports the reason that will
    // still be true after it has company, rather than "last pane" this minute and something
    // else the next.
    if role == PaneRole::Primary {
        return Err(CoreError::PanePrimary);
    }

    // The sole leaf has no sibling to collapse into, so there is nothing this could
    // possibly leave behind.
    if let LayoutNode::Leaf { pane: only } = tree.root
        && only == pane
    {
        return Err(CoreError::LastPane);
    }

    let neighbour = match prune(&mut tree.root, pane) {
        Prune::Done { neighbour } => neighbour,
        // `Here` means the root itself was the target, which the guard above already
        // refused; either way the tree is unchanged.
        Prune::Here | Prune::Missing => return Err(CoreError::NoSuchPane(pane)),
    };

    // Ordered removal: `panes` is serialised as-is, and swap-removal would reshuffle the
    // file on every close for no reason.
    let Some(removed) = tree.panes.shift_remove(&pane) else {
        return Err(CoreError::NoSuchPane(pane));
    };

    if tree.focused == pane {
        tree.focused = neighbour;
    }
    if tree.maximized == Some(pane) {
        tree.maximized = None;
    }
    Ok(removed)
}

/// A member's minimum share of its chain.
///
/// Equal to [`MIN_RATIO`] on purpose — see the module docs for the proof that this is what
/// keeps every derived ratio inside the band [`validate`] already enforced.
pub const MIN_TILE: f32 = MIN_RATIO;

/// `1 / MIN_TILE`: a chain cannot hold more members than this and still give each one a
/// legal share.
pub const MAX_MEMBERS: usize = 10;

/// Move a divider.
///
/// `share` is the first member's share of the **pair** this divider separates within its
/// chain — not the split's `a`-share. The two coincide whenever the split is the only one
/// in its chain, which covers every two-pane tab and the shipped console, so no existing
/// caller changes.
///
/// Only the two members either side of the divider move: their shares are rewritten with
/// their sum preserved, so every other member of the chain — and every member of every
/// other chain — keeps its share bit for bit. That is a theorem about [`weights`] and
/// [`write_weights`] rather than an emergent property of tree shape, which is what lets a
/// vertical divider in one row leave the row below it alone.
///
/// Returns the value that was stored rather than `()`, so a caller dragging past the end
/// stop learns where the divider actually went instead of having to re-derive the clamp.
pub fn set_ratio(tree: &mut PaneTree, split: SplitId, share: f32) -> Result<f32> {
    // Existence first. A stale divider id is the caller's mistake and it says so plainly;
    // reporting `Invariant` because the value was also bad would send whoever is debugging
    // a dropped drag event looking at the wrong thing.
    let mut path = Vec::new();
    let Some((k, axis)) = locate_chain(&tree.root, split, &mut path) else {
        return Err(CoreError::NoSuchSplit(split));
    };

    // `f32::clamp` propagates NaN, so a non-finite ratio would sail through and poison the
    // layout for the rest of the session.
    if !share.is_finite() {
        return Err(CoreError::Invariant(format!(
            "ratio for split {split} is not finite"
        )));
    }

    let chain = node_at_mut(&mut tree.root, &path);
    let mut f = weights(chain, axis);
    let p = f[k] + f[k + 1];
    // `MIN_TILE / p` is the pair-relative floor. On a legacy chain whose pair is already
    // smaller than two tiles the two ends cross over, and `f32::clamp` panics on an
    // inverted range — halving the pair is the one answer that is legal from both ends.
    let lo = MIN_TILE / p;
    let t = if lo <= 1.0 - lo {
        share.clamp(lo, 1.0 - lo)
    } else {
        0.5
    };
    f[k] = p * t;
    f[k + 1] = p * (1.0 - t);
    write_weights(chain, axis, &f);

    // Re-read rather than reporting `t`: on a legacy chain the backstop clamp inside
    // `write_weights` can store a slightly different geometry, and a caller that was
    // clamped has to learn the truth. The final clamp is a no-op on every chain whose
    // members meet the floor, and it stops a pathological legacy chain from reporting a
    // share outside the band the wire promises.
    let g = weights(node_at(&tree.root, &path), axis);
    let stored = g[k] / (g[k] + g[k + 1]);
    Ok(if stored.is_finite() {
        stored.clamp(MIN_RATIO, MAX_RATIO)
    } else {
        t
    })
}

/// Add a tile beside `target`, in `target`'s row.
///
/// The new tile takes `1 / (n + 1)` of the row and every existing tile is scaled by
/// `n / (n + 1)`, so relative widths survive and four clicks from one pane give four equal
/// tiles. Equalising the row instead was the alternative and it loses: it discards a
/// hand-tuned row on every insert, and it gives the same answer in the case anyone actually
/// complained about.
///
/// Refused once the row holds [`MAX_MEMBERS`] tiles — an explicit refusal, not a silently
/// uneven row.
pub fn add_tile(tree: &mut PaneTree, target: PaneId, side: Side, pane: Pane) -> Result<PaneId> {
    if !tree.panes.contains_key(&target) {
        return Err(CoreError::NoSuchPane(target));
    }
    if tree.panes.contains_key(&pane.id) {
        return Err(CoreError::Invariant(format!(
            "pane {} is already in this tree",
            pane.id
        )));
    }

    // Read from the *old* chain: the split `graft` mints carries a placeholder 0.5 that
    // means nothing, and reading it back would bake the placeholder into the row.
    let mut path = Vec::new();
    let (f, m) = match locate_member(&tree.root, target, Axis::Row, &mut path) {
        Some(m) => (weights(node_at(&tree.root, &path), Axis::Row), m),
        // The target has no row above it — a lone pane, or one stacked inside a column. The
        // graft below mints the row.
        None => (vec![1.0], 0),
    };
    let n = f.len();
    if n >= MAX_MEMBERS {
        return Err(CoreError::Invariant(format!(
            "a row already holds {MAX_MEMBERS} panes; add a row instead"
        )));
    }

    let mut next = reweight_for_insert(
        &f,
        match side {
            Side::Before => m,
            Side::After => m + 1,
        },
    );
    legalise(&mut next);

    let id = pane.id;
    if !graft(&mut tree.root, target, Axis::Row, side, id) {
        return Err(CoreError::NoSuchPane(target));
    }
    tree.panes.insert(id, pane);
    // The same coupling `attach` enforces and `validate` checks: the pane the user just
    // made takes focus, and a maximized flag left set would hide it.
    tree.focused = id;
    tree.maximized = None;

    // Located again rather than reused: when the target had no row above it, the chain root
    // is the split that has just been minted.
    let mut after = Vec::new();
    if locate_member(&tree.root, id, Axis::Row, &mut after).is_some() {
        write_weights(node_at_mut(&mut tree.root, &after), Axis::Row, &next);
    }
    Ok(id)
}

/// Give every pane in a row — or every row of a column — the same share of it.
///
/// The user's words: *"item that will allow me to set all panels in current row to same
/// width (proportional)"*. Three tiles a drag has left at 60/25/15 become thirds; nothing
/// outside the chain moves at all, so evening out row one cannot change how tall it is or
/// touch the row beneath it — the same theorem [`set_ratio`] rests on.
///
/// **Which** row is the innermost chain of `axis` the pane sits in, not the outermost. A
/// pane stacked inside one cell of a row is a member of that inner column and of no row of
/// its own, and the chain that reads as "this pane's row" is then the one its *cell* is a
/// tile of — so the walk takes the deepest same-axis ancestor and then climbs back out
/// through the unbroken run of them, because a chain's shares are only meaningful against
/// its maximal root.
///
/// A pane with no chain of that axis above it at all — a lone pane, or one under nothing
/// but cross-axis splits — is a row of one, and this is a no-op rather than an error. The
/// menu greys its own item out from the same fact, and a refusal here would turn "there was
/// nothing to do" into an error toast.
pub fn distribute(tree: &mut PaneTree, pane: PaneId, axis: Axis) -> Result<()> {
    if !tree.panes.contains_key(&pane) {
        return Err(CoreError::NoSuchPane(pane));
    }
    let Some(path) = chain_around(&tree.root, pane, axis) else {
        return Ok(());
    };
    let n = weights(node_at(&tree.root, &path), axis).len();
    // `1 / n` is below `MIN_TILE` only past `MAX_MEMBERS` members, which both gestures
    // refuse; a legacy chain longer than that gets `write_weights`' clamp, which is uneven
    // but legal — the alternative is refusing to tidy the one layout that most needs it.
    write_weights(
        node_at_mut(&mut tree.root, &path),
        axis,
        &vec![1.0 / n as f32; n],
    );
    Ok(())
}

/// Add a full-width row holding exactly one pane.
///
/// `after: None` wraps the whole root, which is the only way to reach a *row*: [`split`]
/// only ever replaces a leaf, so splitting downwards stacks a tile inside one cell and
/// never spans the tab. `after: Some(p)` wraps the spine member whose subtree holds `p`, so
/// "split down" from row 1 of a three-row tab inserts between rows 1 and 2 rather than at
/// the bottom.
///
/// The new row takes `1 / (n + 1)` of the tab and the existing rows are scaled by
/// `n / (n + 1)`, exactly as [`add_tile`] does within a row — so one row plus one row is
/// 50/50, which is the case the user asked for by name.
pub fn add_row(
    tree: &mut PaneTree,
    after: Option<PaneId>,
    side: Side,
    pane: Pane,
) -> Result<PaneId> {
    if tree.panes.contains_key(&pane.id) {
        return Err(CoreError::Invariant(format!(
            "pane {} is already in this tree",
            pane.id
        )));
    }
    if let Some(p) = after
        && !tree.panes.contains_key(&p)
    {
        return Err(CoreError::NoSuchPane(p));
    }

    let f = weights(&tree.root, Axis::Col);
    let n = f.len();
    if n >= MAX_MEMBERS {
        return Err(CoreError::Invariant(format!(
            "a tab already holds {MAX_MEMBERS} rows"
        )));
    }

    // Which spine member gets wrapped, and where the new row lands among the members.
    let (member, at) = match after {
        None => (
            None,
            match side {
                Side::Before => 0,
                Side::After => n,
            },
        ),
        Some(p) => {
            let m = member_index_of(&tree.root, Axis::Col, p).ok_or(CoreError::NoSuchPane(p))?;
            (
                Some(m),
                match side {
                    Side::Before => m,
                    Side::After => m + 1,
                },
            )
        }
    };

    let mut next = reweight_for_insert(&f, at);
    legalise(&mut next);

    let id = pane.id;
    let mut path = Vec::new();
    if let Some(m) = member {
        member_path(&tree.root, Axis::Col, m, &mut path);
    }
    wrap_node(node_at_mut(&mut tree.root, &path), Axis::Col, side, id);

    tree.panes.insert(id, pane);
    tree.focused = id;
    tree.maximized = None;

    // The root is a `Col` split by construction now, whichever branch ran, so the whole
    // spine is one chain.
    write_weights(&mut tree.root, Axis::Col, &next);
    Ok(id)
}

/// Focus a pane. Fails rather than focusing something that is not in the tree.
///
/// Focusing anything other than the maximized pane un-maximizes, since the renderer hides
/// every other pane and there is no way to focus one you cannot see. That makes
/// "maximize, then navigate away" behave the way a user expects rather than typing into a
/// hidden pane.
pub fn focus(tree: &mut PaneTree, pane: PaneId) -> Result<()> {
    if !contains_leaf(&tree.root, pane) {
        return Err(CoreError::NoSuchPane(pane));
    }
    if tree.maximized.is_some_and(|m| m != pane) {
        tree.maximized = None;
    }
    tree.focused = pane;
    Ok(())
}

/// The pane the keyboard should move to, or `None` at the edge of the tree.
///
/// Walks up to the nearest ancestor split on `dir`'s axis where `from` sits on the side
/// being moved away from, then descends the other subtree taking the pane that lies under
/// the source's own position on the perpendicular axis.
///
/// That last part used to be a fudge — "with no pane rectangles to consult, take the first
/// child" — and it is visibly wrong in the layout this module now builds: `Down` from the
/// fourth tile of a row landed on the *first* tile of the row below. [`weights`] is the
/// measured layout the core was said not to have, up to a constant factor per axis, so the
/// source's interval on the perpendicular axis is accumulated on the way up and its midpoint
/// picks the child on the way down. Ties go to the first child, which is what keeps every
/// answer this rule shares with the old one identical.
pub fn navigate(tree: &PaneTree, from: PaneId, dir: Direction) -> Option<PaneId> {
    let mut path = Vec::new();
    if !path_to(&tree.root, from, &mut path) {
        return None;
    }

    let axis = dir.axis();
    let forward = dir.is_forward();
    for (i, (node, took_b)) in path.iter().enumerate().rev() {
        let LayoutNode::Split {
            axis: split_axis,
            a,
            b,
            ..
        } = node
        else {
            continue;
        };
        // Crossing this divider is only possible from the side we are leaving: moving
        // forward requires `from` to be in `a`, backward requires it to be in `b`.
        if *split_axis != axis || *took_b == forward {
            continue;
        }
        // The ancestor splits along the axis of travel, so the subtree across the divider
        // has exactly the ancestor's extent on the perpendicular axis — which is what makes
        // an interval measured against the ancestor directly usable over there.
        let (lo, hi) = perpendicular_span(&path[i + 1..], axis);
        let across = if forward { b } else { a };
        return Some(boundary_leaf(across, axis, forward, (lo + hi) / 2.0));
    }
    None
}

/// Exchange two panes' positions in the tree, leaving the panes themselves untouched.
pub fn swap(tree: &mut PaneTree, a: PaneId, b: PaneId) -> Result<()> {
    if !contains_leaf(&tree.root, a) {
        return Err(CoreError::NoSuchPane(a));
    }
    if !contains_leaf(&tree.root, b) {
        return Err(CoreError::NoSuchPane(b));
    }
    if a != b {
        relabel(&mut tree.root, a, b);
    }
    Ok(())
}

/// Give one pane the whole tab, or clear the flag with `None`.
///
/// Deliberately not a tree mutation — see the module docs.
///
/// Maximizing also focuses, because the two must never disagree: the renderer hides every
/// pane but the maximized one, so a focus left behind on a hidden pane sends the user's
/// keystrokes somewhere they cannot see. [`focus`] and [`split`] enforce the same coupling
/// from the other side.
pub fn maximize(tree: &mut PaneTree, pane: Option<PaneId>) -> Result<()> {
    if let Some(id) = pane {
        if !contains_leaf(&tree.root, id) {
            return Err(CoreError::NoSuchPane(id));
        }
        tree.focused = id;
    }
    tree.maximized = pane;
    Ok(())
}

/// Every leaf under `node`, depth first, `a` before `b`.
///
/// This order is the tab's reading order, so it is also the order of the `Alt+1..9` jump
/// bindings — see [`pane_index`].
pub fn leaves(node: &LayoutNode) -> Vec<PaneId> {
    let mut out = Vec::new();
    collect_leaves(node, &mut out);
    out
}

/// One-based position of a pane in depth-first order, or `None` if it is not in the tree.
pub fn pane_index(tree: &PaneTree, pane: PaneId) -> Option<usize> {
    leaves(&tree.root)
        .iter()
        .position(|p| *p == pane)
        .map(|i| i + 1)
}

/// The tree's [`PaneRole::Primary`] leaf, if it has one.
///
/// At most one can exist — [`validate`] rejects a second — so this answers with a single id
/// rather than an iterator. `None` is the normal answer for every tab that is not the pinned
/// console.
pub fn primary_of(tree: &PaneTree) -> Option<PaneId> {
    // Leaf order rather than `panes` order, so the answer is the pane the user can actually
    // see; `validate` guarantees the two sets agree anyway.
    leaves(&tree.root).into_iter().find(|id| {
        tree.panes
            .get(id)
            .is_some_and(|p| p.role == PaneRole::Primary)
    })
}

/// [`validate`], plus the half it cannot check on its own: **a Primary leaf exists**.
///
/// For the trees that are required to have one — today that is the pinned console tab, and
/// the caller is the only party that knows which those are. See the module docs for why this
/// is not folded into [`validate`]: an ordinary file or shell tab legitimately has no
/// `Primary`, so a blanket check there would reject most of the workspace.
pub fn validate_console(tree: &PaneTree) -> Result<()> {
    validate(tree)?;
    if primary_of(tree).is_none() {
        return Err(CoreError::Invariant(
            "the console's tree has no Primary leaf".into(),
        ));
    }
    Ok(())
}

/// Check every invariant listed in the module docs, naming the first breach found.
///
/// Cheap enough for tests and debug assertions after each mutation; a tab holds a handful
/// of panes, not thousands.
///
/// Note what is *not* here: "a Primary leaf exists". That is [`validate_console`]; see the
/// module docs.
pub fn validate(tree: &PaneTree) -> Result<()> {
    let leaf_ids = leaves(&tree.root);

    let mut seen = HashSet::with_capacity(leaf_ids.len());
    for id in &leaf_ids {
        if !seen.insert(*id) {
            return Err(CoreError::Invariant(format!(
                "pane {id} appears at more than one leaf"
            )));
        }
    }

    for id in &leaf_ids {
        if !tree.panes.contains_key(id) {
            return Err(CoreError::Invariant(format!(
                "leaf {id} has no entry in panes"
            )));
        }
    }
    for (id, pane) in &tree.panes {
        if !seen.contains(id) {
            return Err(CoreError::Invariant(format!(
                "pane {id} has an entry in panes but no leaf"
            )));
        }
        // The leaf addresses the pane by map key while every command the frontend sends
        // carries `Pane::id`; if a hand-edited or migrated file lets the two drift, the
        // pane renders but every click on it answers `NoSuchPane`.
        if pane.id != *id {
            return Err(CoreError::Invariant(format!(
                "pane keyed as {id} carries id {}",
                pane.id
            )));
        }
    }

    // Two primaries would make "the console's primary pane" ambiguous: `primary_of` would
    // answer with whichever came first in leaf order, and `take_pane`'s refusal would then
    // protect a pane the rest of the app does not think of as the console's. Nothing in this
    // module can produce the state — a tree gets its Primary at construction and `take_pane`
    // never removes one — so the ways in are `split`/`insert_pane` handed a caller-built
    // `Primary`, and a hand-edited or migrated `workspace.json`. Both arrive here.
    let primaries: Vec<PaneId> = leaf_ids
        .iter()
        .copied()
        .filter(|id| {
            tree.panes
                .get(id)
                .is_some_and(|p| p.role == PaneRole::Primary)
        })
        .collect();
    if primaries.len() > 1 {
        return Err(CoreError::Invariant(format!(
            "{} leaves are Primary ({}); a tree may hold at most one",
            primaries.len(),
            primaries
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    if !seen.contains(&tree.focused) {
        return Err(CoreError::Invariant(format!(
            "focused pane {} is not a live leaf",
            tree.focused
        )));
    }
    if let Some(max) = tree.maximized {
        if !seen.contains(&max) {
            return Err(CoreError::Invariant(format!(
                "maximized pane {max} is not a live leaf"
            )));
        }
        // The renderer hides every pane but the maximized one, so a focus elsewhere means
        // keystrokes go to a pane the user cannot see. `maximize`, `focus` and `attach`
        // each maintain this; checking it here is what stops a future operation from
        // quietly breaking the coupling.
        if tree.focused != max {
            return Err(CoreError::Invariant(format!(
                "pane {max} is maximized but {} has focus",
                tree.focused
            )));
        }
    }

    // A one-child split is unrepresentable — `LayoutNode::Split` holds two boxes — so what
    // is left to check is that no divider is addressable twice and that no ratio has drifted
    // outside the band the renderer can draw.
    let mut splits = Vec::new();
    collect_splits(&tree.root, &mut splits);
    let mut seen_splits = HashSet::with_capacity(splits.len());
    for (id, ratio) in &splits {
        if !seen_splits.insert(*id) {
            return Err(CoreError::Invariant(format!(
                "split {id} appears more than once"
            )));
        }
        if !ratio.is_finite() {
            return Err(CoreError::Invariant(format!(
                "split {id} has a non-finite ratio"
            )));
        }
        if *ratio < MIN_RATIO || *ratio > MAX_RATIO {
            return Err(CoreError::Invariant(format!(
                "split {id} has ratio {ratio} outside [{MIN_RATIO}, {MAX_RATIO}]"
            )));
        }
    }

    Ok(())
}

// --- internals ---------------------------------------------------------------------

/// Shared body of [`split`] and [`insert_pane`].
fn attach(
    tree: &mut PaneTree,
    target: PaneId,
    axis: Axis,
    side: Side,
    pane: Pane,
) -> Result<PaneId> {
    if !tree.panes.contains_key(&target) {
        return Err(CoreError::NoSuchPane(target));
    }
    // Two leaves with one id would make the pane unaddressable: close, focus and navigate
    // all key on it. Refuse before touching the tree.
    if tree.panes.contains_key(&pane.id) {
        return Err(CoreError::Invariant(format!(
            "pane {} is already in this tree",
            pane.id
        )));
    }

    let id = pane.id;
    if !graft(&mut tree.root, target, axis, side, id) {
        return Err(CoreError::NoSuchPane(target));
    }
    tree.panes.insert(id, pane);
    tree.focused = id;
    // A new pane is by definition not the maximized one, and it takes focus — leaving the
    // flag set would hide the pane the user just created.
    tree.maximized = None;
    Ok(id)
}

/// Replace the leaf `target` with a split of `target` and `new_pane`.
fn graft(node: &mut LayoutNode, target: PaneId, axis: Axis, side: Side, new_pane: PaneId) -> bool {
    match node {
        LayoutNode::Leaf { pane } if *pane == target => {
            let old = LayoutNode::Leaf { pane: target };
            let new = LayoutNode::Leaf { pane: new_pane };
            let (a, b) = match side {
                Side::Before => (new, old),
                Side::After => (old, new),
            };
            *node = LayoutNode::Split {
                id: SplitId::new(),
                axis,
                a: Box::new(a),
                b: Box::new(b),
                ratio: 0.5,
            };
            true
        }
        LayoutNode::Leaf { .. } => false,
        LayoutNode::Split { a, b, .. } => {
            graft(a, target, axis, side, new_pane) || graft(b, target, axis, side, new_pane)
        }
    }
}

/// The split that has `pane` as a direct child, described as a [`DockAnchor`].
fn find_parent(node: &LayoutNode, pane: PaneId) -> Option<DockAnchor> {
    let LayoutNode::Split {
        id,
        axis,
        a,
        b,
        ratio,
    } = node
    else {
        return None;
    };

    // `a` and `b` are the only two places the pane can be a *direct* child; anywhere else it
    // belongs to a deeper split, which the recursion finds instead.
    let side = match (a.as_ref(), b.as_ref()) {
        (LayoutNode::Leaf { pane: p }, _) if *p == pane => Some(Side::Before),
        (_, LayoutNode::Leaf { pane: p }) if *p == pane => Some(Side::After),
        _ => None,
    };
    if let Some(side) = side {
        let sibling = node_ref(match side {
            Side::Before => b,
            Side::After => a,
        });
        return Some(DockAnchor {
            sibling,
            split: *id,
            axis: *axis,
            side,
            ratio: *ratio,
        });
    }

    find_parent(a, pane).or_else(|| find_parent(b, pane))
}

/// Name a node by whichever id it carries.
fn node_ref(node: &LayoutNode) -> DockSibling {
    match node {
        LayoutNode::Leaf { pane } => DockSibling::Pane { pane: *pane },
        LayoutNode::Split { id, .. } => DockSibling::Split { split: *id },
    }
}

fn contains_node(node: &LayoutNode, wanted: DockSibling) -> bool {
    if node_ref(node) == wanted {
        return true;
    }
    match node {
        LayoutNode::Leaf { .. } => false,
        LayoutNode::Split { a, b, .. } => contains_node(a, wanted) || contains_node(b, wanted),
    }
}

fn contains_split(node: &LayoutNode, wanted: SplitId) -> bool {
    contains_node(node, DockSibling::Split { split: wanted })
}

/// Wrap the anchor's sibling node in the split it used to share with `new_pane`.
///
/// The mirror image of [`prune`]: where that collapses a split into the survivor, this
/// re-expands the survivor into a split. Reports whether the sibling was found.
fn graft_at(node: &mut LayoutNode, anchor: &DockAnchor, ratio: f32, new_pane: PaneId) -> bool {
    if node_ref(node) == anchor.sibling {
        // The placeholder is dropped on the next line; it exists only so the sibling — which
        // may be a whole subtree — can be moved into the new split rather than cloned.
        let old = std::mem::replace(node, LayoutNode::Leaf { pane: new_pane });
        let new = LayoutNode::Leaf { pane: new_pane };
        let (a, b) = match anchor.side {
            Side::Before => (new, old),
            Side::After => (old, new),
        };
        *node = LayoutNode::Split {
            id: anchor.split,
            axis: anchor.axis,
            a: Box::new(a),
            b: Box::new(b),
            ratio,
        };
        return true;
    }
    match node {
        LayoutNode::Leaf { .. } => false,
        LayoutNode::Split { a, b, .. } => {
            graft_at(a, anchor, ratio, new_pane) || graft_at(b, anchor, ratio, new_pane)
        }
    }
}

/// What a subtree did with a removal request.
enum Prune {
    /// The pane is not in this subtree.
    Missing,
    /// This node *is* the pane; the parent must collapse into the other child.
    Here,
    /// Already removed below. `neighbour` is the first leaf of the subtree that survived
    /// the collapse, which is where focus goes if the removed pane held it.
    Done { neighbour: PaneId },
}

/// Remove a leaf, collapsing its parent split into the sibling.
fn prune(node: &mut LayoutNode, target: PaneId) -> Prune {
    match node {
        LayoutNode::Leaf { pane } => {
            if *pane == target {
                Prune::Here
            } else {
                Prune::Missing
            }
        }
        LayoutNode::Split { a, b, .. } => match prune(a, target) {
            Prune::Done { neighbour } => Prune::Done { neighbour },
            Prune::Here => {
                let neighbour = first_leaf(b);
                // The placeholder is dropped on the next line; it exists only so the
                // survivor can be moved out of the split that is going away.
                let survivor = std::mem::replace(b.as_mut(), LayoutNode::Leaf { pane: target });
                *node = survivor;
                Prune::Done { neighbour }
            }
            Prune::Missing => match prune(b, target) {
                Prune::Done { neighbour } => Prune::Done { neighbour },
                Prune::Here => {
                    let neighbour = first_leaf(a);
                    let survivor = std::mem::replace(a.as_mut(), LayoutNode::Leaf { pane: target });
                    *node = survivor;
                    Prune::Done { neighbour }
                }
                Prune::Missing => Prune::Missing,
            },
        },
    }
}

// --- chain algebra -----------------------------------------------------------------
//
// A chain is addressed by the path from the root to its root node — a `Vec<bool>` of "took
// `b`" steps. A path rather than a `&mut` because every one of these operations reads the
// chain, decides, and then writes it: holding a mutable borrow across the decision would
// mean cloning the subtree to look at it.

/// Each member's share of the chain `node` roots, treated as a chain of `axis`.
///
/// The share is the product of the ratios on the member's path (`ratio` descending into
/// `a`, `1 - ratio` into `b`), so the vector sums to 1 by construction. A node that is not
/// an `axis` split is a chain of one.
fn weights(node: &LayoutNode, axis: Axis) -> Vec<f32> {
    let mut out = Vec::new();
    collect_weights(node, axis, 1.0, &mut out);
    out
}

fn collect_weights(node: &LayoutNode, axis: Axis, factor: f32, out: &mut Vec<f32>) {
    match node {
        LayoutNode::Split {
            axis: ax,
            a,
            b,
            ratio,
            ..
        } if *ax == axis => {
            collect_weights(a, axis, factor * *ratio, out);
            collect_weights(b, axis, factor * (1.0 - *ratio), out);
        }
        // A leaf, or a split on the other axis: one member, whatever it holds inside.
        _ => out.push(factor),
    }
}

/// The exact inverse of [`weights`]: write every ratio of the chain from a share vector.
///
/// Bottom-up, because a node's ratio is the ratio of its two subtrees' *totals* and those
/// are only known once the children have been visited. Returns the subtree's total so the
/// parent can use it.
///
/// The clamp is a backstop, not the design: on any chain whose members meet `MIN_TILE` it
/// provably never fires (see the module docs). It is here for a legacy chain that already
/// holds a sub-floor member, where letting an out-of-band ratio through would make the very
/// next [`validate`] reject the whole workspace as corrupt.
fn write_weights(node: &mut LayoutNode, axis: Axis, f: &[f32]) -> f32 {
    let mut i = 0;
    write_axis(node, axis, f, &mut i)
}

fn write_axis(node: &mut LayoutNode, axis: Axis, f: &[f32], i: &mut usize) -> f32 {
    match node {
        LayoutNode::Split {
            axis: ax,
            a,
            b,
            ratio,
            ..
        } if *ax == axis => {
            let wa = write_axis(a, axis, f, i);
            let wb = write_axis(b, axis, f, i);
            let total = wa + wb;
            // A zero or non-finite total can only come from a caller's vector, never from
            // `weights`; halving keeps the tree renderable rather than writing a NaN that
            // `validate` would then reject.
            let want = if total.is_finite() && total > 0.0 {
                wa / total
            } else {
                0.5
            };
            *ratio = want.clamp(MIN_RATIO, MAX_RATIO);
            total
        }
        _ => {
            let w = f.get(*i).copied().unwrap_or(0.0);
            *i += 1;
            w
        }
    }
}

/// Raise every sub-floor share to [`MIN_TILE`] and scale the surplus of the rest down to
/// pay for it.
///
/// One pass is exact: the surplus above the floor sums to `1 - n * MIN_TILE`, which is
/// exactly what is left after every member is given its floor, so scaling the surplus by
/// `(1 - n * MIN_TILE) / free` lands the vector back on 1 and can never push a member back
/// below the floor. A no-op — deliberately, down to the last bit — on a healthy vector, so
/// "a divider edit moves only its two members" survives it.
fn legalise(f: &mut [f32]) {
    let n = f.len();
    if n == 0 || f.iter().all(|w| w.is_finite() && *w >= MIN_TILE) {
        return;
    }
    // More members than the floor can pay for. Only reachable on a legacy chain, since both
    // gestures refuse past `MAX_MEMBERS`; equal shares is the least surprising answer.
    let floor_total = n as f32 * MIN_TILE;
    let total: f32 = f.iter().sum();
    if floor_total > 1.0 || !total.is_finite() || total <= 0.0 {
        f.fill(1.0 / n as f32);
        return;
    }
    for w in f.iter_mut() {
        *w /= total;
    }
    let free: f32 = f
        .iter()
        .filter(|w| **w > MIN_TILE)
        .map(|w| *w - MIN_TILE)
        .sum();
    if free <= 0.0 {
        f.fill(1.0 / n as f32);
        return;
    }
    let scale = (1.0 - floor_total) / free;
    for w in f.iter_mut() {
        *w = MIN_TILE + (*w - MIN_TILE).max(0.0) * scale;
    }
}

/// The share vector for a chain that is about to gain a member at `at`.
///
/// The newcomer takes `1 / (n + 1)` and everyone else is scaled by `n / (n + 1)`, so the
/// sum stays 1 and the existing members keep their *relative* sizes.
fn reweight_for_insert(f: &[f32], at: usize) -> Vec<f32> {
    let n = f.len();
    let share = 1.0 / (n + 1) as f32;
    let keep = n as f32 * share;
    let mut next: Vec<f32> = f.iter().map(|w| w * keep).collect();
    next.insert(at.min(n), share);
    next
}

/// The share vector for a chain that is about to lose the member at `at`.
///
/// The exact inverse of [`reweight_for_insert`]: the departing member's share is handed to
/// the survivors in proportion to what they already hold, so the sum stays 1 and everyone
/// keeps their *relative* size — `n` equal tiles minus one is `n - 1` equal tiles.
fn reweight_for_remove(f: &[f32], at: usize) -> Vec<f32> {
    let rest: Vec<f32> = f
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != at)
        .map(|(_, w)| *w)
        .collect();
    let total: f32 = rest.iter().sum();
    // A chain whose survivors sum to nothing can only come from a corrupt file, never from
    // `weights`; equal shares keep it renderable, where dividing by it would write NaNs that
    // `validate` then rejects.
    if !total.is_finite() || total <= 0.0 {
        return vec![1.0 / rest.len().max(1) as f32; rest.len()];
    }
    rest.iter().map(|w| w / total).collect()
}

/// How the chain around `pane` must be re-weighted once `pane` has gone: the path to the
/// chain root, its axis, and the survivors' shares.
///
/// `None` when there is nothing to write. Either the pane is a chain of one — its parent is
/// a cross-axis split, so the cell it shared is the only thing that grows and there is no
/// row to spread across — or the chain holds two members, and the survivor takes all of it
/// whatever we would have written.
///
/// A pane is a member in its own right of at most one axis, since that axis is its parent
/// split's, so the first match is the answer and the loop is a two-case lookup, not a
/// search.
///
/// The path survives the [`prune`] that follows. Either the chain root is a strict ancestor
/// of the collapsing split — then nothing on the way down to it moves — or it *is* that
/// split, and since a chain root with three or more members splits them across both sides,
/// the survivor is a same-axis split of the remaining `n - 1` that takes its place at the
/// very same path.
fn removal_plan(root: &LayoutNode, pane: PaneId) -> Option<(Vec<bool>, Axis, Vec<f32>)> {
    for axis in [Axis::Row, Axis::Col] {
        let mut path = Vec::new();
        let Some(m) = locate_member(root, pane, axis, &mut path) else {
            continue;
        };
        let f = weights(node_at(root, &path), axis);
        if f.len() < 3 {
            return None;
        }
        let mut next = reweight_for_remove(&f, m);
        // The survivors only ever grow, so this cannot fire on a healthy chain; it is here
        // for the legacy chain that already held a sub-floor member.
        legalise(&mut next);
        return Some((path, axis, next));
    }
    None
}

/// The path to the root of the innermost chain of `axis` that holds `pane`, or `None` when
/// no split of that axis is above it.
///
/// Different from [`locate_member`], and deliberately: that one answers "is the pane a
/// member of this chain **in its own right**", which is what a removal needs, and says
/// `None` for a pane sitting inside a cross-axis subtree that is *itself* a member. Here
/// such a pane belongs to the row its cell is a tile of, so the search takes the deepest
/// same-axis ancestor and then climbs out through the run of same-axis splits above it to
/// reach the chain's maximal root — the node [`weights`] and [`write_weights`] are defined
/// against.
fn chain_around(root: &LayoutNode, pane: PaneId, axis: Axis) -> Option<Vec<bool>> {
    let mut trail = Vec::new();
    if !path_to(root, pane, &mut trail) {
        return None;
    }
    let same_axis =
        |node: &LayoutNode| matches!(node, LayoutNode::Split { axis: ax, .. } if *ax == axis);
    let deepest = trail.iter().rposition(|(node, _)| same_axis(node))?;
    let mut root_at = deepest;
    while root_at > 0 && same_axis(trail[root_at - 1].0) {
        root_at -= 1;
    }
    // `trail[k]` is the ancestor reached by the first `k` steps, so the path to it is the
    // steps before it — not including its own.
    Some(trail[..root_at].iter().map(|(_, took_b)| *took_b).collect())
}

/// The interior splits of the chain `node` roots, in in-order — so index `k` is the divider
/// between members `k` and `k + 1`.
fn chain_splits(node: &LayoutNode, axis: Axis, out: &mut Vec<SplitId>) {
    if let LayoutNode::Split {
        id, axis: ax, a, b, ..
    } = node
        && *ax == axis
    {
        chain_splits(a, axis, out);
        out.push(*id);
        chain_splits(b, axis, out);
    }
}

/// The maximal same-axis chain containing `split`: the path to its root node, the divider
/// index of `split` within it, and the chain's axis.
///
/// Descending from the tree root finds the *outermost* node whose chain holds the split,
/// which is the chain root by definition: had an ancestor's chain held it, that ancestor
/// would have answered first.
fn locate_chain(node: &LayoutNode, split: SplitId, path: &mut Vec<bool>) -> Option<(usize, Axis)> {
    let LayoutNode::Split { axis, a, b, .. } = node else {
        return None;
    };
    let mut dividers = Vec::new();
    chain_splits(node, *axis, &mut dividers);
    if let Some(k) = dividers.iter().position(|id| *id == split) {
        return Some((k, *axis));
    }
    path.push(false);
    if let Some(found) = locate_chain(a, split, path) {
        return Some(found);
    }
    path.pop();
    path.push(true);
    if let Some(found) = locate_chain(b, split, path) {
        return Some(found);
    }
    path.pop();
    None
}

/// The chain of `axis` in which `pane` is a member in its own right: the path to the chain
/// root and the pane's member index.
///
/// `None` when the pane's parent is not an `axis` split — it is then a chain of one, and the
/// caller mints the chain.
fn locate_member(
    node: &LayoutNode,
    pane: PaneId,
    axis: Axis,
    path: &mut Vec<bool>,
) -> Option<usize> {
    let LayoutNode::Split { axis: ax, a, b, .. } = node else {
        return None;
    };
    if *ax == axis
        && let Some(i) = member_index_of(node, axis, pane)
        && matches!(chain_member(node, axis, i), Some(LayoutNode::Leaf { .. }))
    {
        return Some(i);
    }
    path.push(false);
    if let Some(i) = locate_member(a, pane, axis, path) {
        return Some(i);
    }
    path.pop();
    path.push(true);
    if let Some(i) = locate_member(b, pane, axis, path) {
        return Some(i);
    }
    path.pop();
    None
}

/// Every member of the chain `node` roots, in order.
fn chain_members<'a>(node: &'a LayoutNode, axis: Axis, out: &mut Vec<&'a LayoutNode>) {
    match node {
        LayoutNode::Split { axis: ax, a, b, .. } if *ax == axis => {
            chain_members(a, axis, out);
            chain_members(b, axis, out);
        }
        _ => out.push(node),
    }
}

fn chain_member(node: &LayoutNode, axis: Axis, index: usize) -> Option<&LayoutNode> {
    let mut members = Vec::new();
    chain_members(node, axis, &mut members);
    members.get(index).copied()
}

/// Which member of the chain `node` roots holds `pane`, anywhere inside it.
fn member_index_of(node: &LayoutNode, axis: Axis, pane: PaneId) -> Option<usize> {
    let mut members = Vec::new();
    chain_members(node, axis, &mut members);
    members.iter().position(|m| contains_leaf(m, pane))
}

/// The path from the chain root down to its `index`-th member.
fn member_path(node: &LayoutNode, axis: Axis, index: usize, path: &mut Vec<bool>) {
    let LayoutNode::Split { axis: ax, a, b, .. } = node else {
        return;
    };
    if *ax != axis {
        return;
    }
    let mut left = Vec::new();
    chain_members(a, axis, &mut left);
    if index < left.len() {
        path.push(false);
        member_path(a, axis, index, path);
    } else {
        path.push(true);
        member_path(b, axis, index - left.len(), path);
    }
}

/// Wrap a whole node — a leaf or a subtree — in a fresh split holding it and a new pane.
///
/// The generalisation of [`graft_at`], which can only wrap an anchor's sibling. Wrapping a
/// *subtree* is what makes a new row span the tab rather than land inside one cell.
fn wrap_node(node: &mut LayoutNode, axis: Axis, side: Side, new_pane: PaneId) {
    // The placeholder is dropped on the next line; it exists only so the old node — which
    // may be the whole tree — can be moved into the new split rather than cloned.
    let old = std::mem::replace(node, LayoutNode::Leaf { pane: new_pane });
    let new = LayoutNode::Leaf { pane: new_pane };
    let (a, b) = match side {
        Side::Before => (new, old),
        Side::After => (old, new),
    };
    *node = LayoutNode::Split {
        id: SplitId::new(),
        axis,
        a: Box::new(a),
        b: Box::new(b),
        // Overwritten by the caller's `write_weights`; a fresh split has no ratio of its own
        // to contribute, and reading this back would bake the placeholder into the layout.
        ratio: 0.5,
    };
}

fn node_at<'a>(root: &'a LayoutNode, path: &[bool]) -> &'a LayoutNode {
    let mut cursor = root;
    for &took_b in path {
        let LayoutNode::Split { a, b, .. } = cursor else {
            return cursor;
        };
        cursor = if took_b { b } else { a };
    }
    cursor
}

fn node_at_mut<'a>(root: &'a mut LayoutNode, path: &[bool]) -> &'a mut LayoutNode {
    let mut cursor = root;
    for &took_b in path {
        let LayoutNode::Split { a, b, .. } = cursor else {
            return cursor;
        };
        cursor = if took_b { b } else { a };
    }
    cursor
}

/// The ancestor chain of `target`, outermost first, each paired with whether the walk
/// descended into `b`.
fn path_to<'a>(
    node: &'a LayoutNode,
    target: PaneId,
    out: &mut Vec<(&'a LayoutNode, bool)>,
) -> bool {
    match node {
        LayoutNode::Leaf { pane } => *pane == target,
        LayoutNode::Split { a, b, .. } => {
            let depth = out.len();
            out.push((node, false));
            if path_to(a, target, out) {
                return true;
            }
            out.truncate(depth);
            out.push((node, true));
            if path_to(b, target, out) {
                return true;
            }
            out.truncate(depth);
            false
        }
    }
}

/// The source's interval on the axis perpendicular to `axis`, as a fraction of the crossing
/// ancestor's extent.
///
/// `descent` is the ancestor's own sub-path down to the source leaf; splits along the axis
/// of travel narrow nothing, because the perpendicular extent is the same on both sides.
fn perpendicular_span(descent: &[(&LayoutNode, bool)], axis: Axis) -> (f32, f32) {
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for (node, took_b) in descent {
        if let LayoutNode::Split {
            axis: split_axis,
            ratio,
            ..
        } = node
            && *split_axis != axis
        {
            let mid = lo + (hi - lo) * *ratio;
            if *took_b {
                lo = mid;
            } else {
                hi = mid;
            }
        }
    }
    (lo, hi)
}

/// Descend to the leaf lying against the divider we have just crossed, at position `at` on
/// the perpendicular axis.
///
/// `at` is rescaled into whichever child it lands in, so the descent tracks one point
/// through however many levels the subtree has.
fn boundary_leaf(node: &LayoutNode, axis: Axis, forward: bool, at: f32) -> PaneId {
    let mut cursor = node;
    let mut at = at;
    loop {
        match cursor {
            LayoutNode::Leaf { pane } => return *pane,
            LayoutNode::Split {
                axis: split_axis,
                a,
                b,
                ratio,
                ..
            } => {
                if *split_axis == axis {
                    // Along the axis of travel exactly one child touches the divider we
                    // crossed; the position carries through it unchanged.
                    cursor = if forward { a } else { b };
                } else if at <= *ratio {
                    // `<=`, so a position exactly on a divider takes the first child. That
                    // is the old rule's answer, which is why every case the two agree on
                    // still reads the same.
                    cursor = a;
                    at = if *ratio > 0.0 { at / *ratio } else { 0.5 };
                } else {
                    cursor = b;
                    let rest = 1.0 - *ratio;
                    at = if rest > 0.0 {
                        (at - *ratio) / rest
                    } else {
                        0.5
                    };
                }
            }
        }
    }
}

/// The first leaf of a subtree in depth-first order.
fn first_leaf(node: &LayoutNode) -> PaneId {
    let mut cursor = node;
    loop {
        match cursor {
            LayoutNode::Leaf { pane } => return *pane,
            LayoutNode::Split { a, .. } => cursor = a,
        }
    }
}

fn contains_leaf(node: &LayoutNode, pane: PaneId) -> bool {
    match node {
        LayoutNode::Leaf { pane: id } => *id == pane,
        LayoutNode::Split { a, b, .. } => contains_leaf(a, pane) || contains_leaf(b, pane),
    }
}

/// Exchange two pane ids wherever they appear as leaves.
fn relabel(node: &mut LayoutNode, x: PaneId, y: PaneId) {
    match node {
        LayoutNode::Leaf { pane } => {
            if *pane == x {
                *pane = y;
            } else if *pane == y {
                *pane = x;
            }
        }
        LayoutNode::Split { a, b, .. } => {
            relabel(a, x, y);
            relabel(b, x, y);
        }
    }
}

fn collect_leaves(node: &LayoutNode, out: &mut Vec<PaneId>) {
    match node {
        LayoutNode::Leaf { pane } => out.push(*pane),
        LayoutNode::Split { a, b, .. } => {
            collect_leaves(a, out);
            collect_leaves(b, out);
        }
    }
}

fn collect_splits(node: &LayoutNode, out: &mut Vec<(SplitId, f32)>) {
    if let LayoutNode::Split {
        id, a, b, ratio, ..
    } = node
    {
        out.push((*id, *ratio));
        collect_splits(a, out);
        collect_splits(b, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::{PaneKind, SessionId};

    fn primary() -> Pane {
        Pane {
            id: PaneId::new(),
            kind: PaneKind::Claude,
            role: PaneRole::Primary,
            session: Some(SessionId::new()),
            conversation: None,
            title: "cide : claude".into(),
        }
    }

    fn aux() -> Pane {
        Pane {
            id: PaneId::new(),
            kind: PaneKind::Shell,
            role: PaneRole::Auxiliary,
            session: Some(SessionId::new()),
            conversation: None,
            title: "cide : bash".into(),
        }
    }

    /// Split `at` and return the id of the pane that appeared.
    fn split_at(tree: &mut PaneTree, at: PaneId, axis: Axis, side: Side) -> PaneId {
        split(tree, at, axis, side, aux()).expect("split of a live leaf succeeds")
    }

    /// The five-pane, depth-three tree used by the navigation tests:
    ///
    /// ```text
    /// +-------+-----------+
    /// |  p1   |    p2     |
    /// +-------+-----+-----+
    /// |  p5   | p3  | p4  |
    /// +-------+-----+-----+
    /// ```
    ///
    /// Deliberately asymmetric: the right column is split one level deeper than the left,
    /// so crossing the middle divider has to descend rather than land on a leaf.
    ///
    /// Every pane is `Auxiliary`, including `p1` — this is an ordinary tab, not the console.
    /// The tests built on it are about *shape* (navigation, anchors, ratios) and several of
    /// them remove `p1`, which a `Primary` root would now refuse. The primary's own rules get
    /// purpose-built fixtures instead of riding along on this one.
    fn asymmetric() -> (PaneTree, [PaneId; 5]) {
        let root = aux();
        let p1 = root.id;
        let mut tree = new_tree(root);
        let p2 = split_at(&mut tree, p1, Axis::Row, Side::After);
        let p3 = split_at(&mut tree, p2, Axis::Col, Side::After);
        let p4 = split_at(&mut tree, p3, Axis::Row, Side::After);
        let p5 = split_at(&mut tree, p1, Axis::Col, Side::After);
        validate(&tree).expect("fixture is well formed");
        (tree, [p1, p2, p3, p4, p5])
    }

    #[test]
    fn a_new_tree_is_one_focused_leaf() {
        let pane = primary();
        let id = pane.id;
        let tree = new_tree(pane);
        assert_eq!(tree.root, LayoutNode::Leaf { pane: id });
        assert_eq!(tree.focused, id);
        assert_eq!(tree.maximized, None);
        assert_eq!(leaves(&tree.root), vec![id]);
        validate(&tree).unwrap();
    }

    #[test]
    fn splitting_after_puts_the_new_pane_second_and_focuses_it() {
        let first = primary();
        let a = first.id;
        let mut tree = new_tree(first);
        let b = split_at(&mut tree, a, Axis::Row, Side::After);

        assert_eq!(leaves(&tree.root), vec![a, b]);
        assert_eq!(tree.focused, b);
        let LayoutNode::Split { axis, ratio, .. } = tree.root else {
            panic!("the target leaf should have become a split");
        };
        assert_eq!(axis, Axis::Row);
        assert_eq!(ratio, 0.5);
        validate(&tree).unwrap();
    }

    #[test]
    fn splitting_before_puts_the_new_pane_first() {
        let first = primary();
        let a = first.id;
        let mut tree = new_tree(first);
        let b = split_at(&mut tree, a, Axis::Col, Side::Before);
        assert_eq!(leaves(&tree.root), vec![b, a]);
        validate(&tree).unwrap();
    }

    #[test]
    fn splitting_an_unknown_pane_is_refused() {
        let mut tree = new_tree(primary());
        let ghost = PaneId::new();
        assert_eq!(
            split(&mut tree, ghost, Axis::Row, Side::After, aux()),
            Err(CoreError::NoSuchPane(ghost))
        );
        validate(&tree).unwrap();
    }

    #[test]
    fn splitting_in_a_pane_that_is_already_present_is_refused() {
        let first = primary();
        let a = first.id;
        let mut tree = new_tree(first);
        let duplicate = tree.panes[&a].clone();

        assert!(matches!(
            split(&mut tree, a, Axis::Row, Side::After, duplicate),
            Err(CoreError::Invariant(_))
        ));
        // The refusal must leave the tree untouched, not half-grafted.
        assert_eq!(leaves(&tree.root), vec![a]);
        validate(&tree).unwrap();
    }

    #[test]
    fn split_then_close_restores_the_tree_exactly() {
        let first = primary();
        let a = first.id;
        let mut tree = new_tree(first);
        let before = tree.clone();

        let b = split_at(&mut tree, a, Axis::Row, Side::After);
        close(&mut tree, b).unwrap();

        assert_eq!(tree, before);
        validate(&tree).unwrap();
    }

    #[test]
    fn closing_the_last_child_collapses_the_split() {
        let (mut tree, [p1, p2, p3, p4, p5]) = asymmetric();

        close(&mut tree, p4).unwrap();

        // The innermost split had two children; losing one must dissolve it rather than
        // leave a split wrapping p3 alone.
        assert_eq!(leaves(&tree.root), vec![p1, p5, p2, p3]);
        let mut splits = Vec::new();
        collect_splits(&tree.root, &mut splits);
        assert_eq!(splits.len(), 3, "n panes must leave n-1 splits");
        validate(&tree).unwrap();
    }

    #[test]
    fn closing_the_only_primary_pane_is_refused() {
        let first = primary();
        let a = first.id;
        let mut tree = new_tree(first);
        assert_eq!(close(&mut tree, a), Err(CoreError::PanePrimary));
        validate(&tree).unwrap();
    }

    #[test]
    fn closing_the_only_auxiliary_pane_is_refused_as_the_last_pane() {
        let first = aux();
        let a = first.id;
        let mut tree = new_tree(first);
        assert_eq!(close(&mut tree, a), Err(CoreError::LastPane));
        validate(&tree).unwrap();
    }

    /// The regression this whole rule exists for.
    ///
    /// This test used to assert the opposite — that company makes the primary closable — and
    /// the shape it blessed is a console tab with no conversation in it, which `validate`
    /// was happy to accept because it never looked for a `Primary` at all.
    #[test]
    fn the_primary_pane_stays_closed_off_even_once_it_has_company() {
        let first = primary();
        let a = first.id;
        let mut tree = new_tree(first);
        let b = split_at(&mut tree, a, Axis::Row, Side::After);

        assert_eq!(close(&mut tree, a), Err(CoreError::PanePrimary));
        assert_eq!(leaves(&tree.root), vec![a, b], "the tree is untouched");
        assert_eq!(primary_of(&tree), Some(a));
        validate_console(&tree).unwrap();

        // Its company, on the other hand, closes normally — the refusal is about the role,
        // not about the tree having become small.
        close(&mut tree, b).unwrap();
        assert_eq!(leaves(&tree.root), vec![a]);
        validate_console(&tree).unwrap();
    }

    #[test]
    fn a_tree_with_no_primary_fails_only_the_console_check() {
        let first = aux();
        let a = first.id;
        let tree = new_tree(first);

        // An ordinary file or shell tab has no Primary and is entirely valid.
        validate(&tree).unwrap();
        assert_eq!(primary_of(&tree), None);
        let Err(CoreError::Invariant(msg)) = validate_console(&tree) else {
            panic!("a console tree with no Primary must be reported");
        };
        assert!(msg.contains("Primary"), "{msg}");
        assert_eq!(a, tree.focused);
    }

    #[test]
    fn validate_rejects_a_second_primary_leaf() {
        let first = primary();
        let a = first.id;
        let mut tree = new_tree(first);
        let b = split_at(&mut tree, a, Axis::Row, Side::After);

        // Only reachable by re-docking a Primary into a tree that already has one, which is
        // exactly the case the check is for; forge it directly rather than build the pair of
        // trees it would take to get here honestly.
        tree.panes[&b].role = PaneRole::Primary;

        let Err(CoreError::Invariant(msg)) = validate(&tree) else {
            panic!("two Primary leaves must be reported");
        };
        assert!(
            msg.contains(&a.to_string()) && msg.contains(&b.to_string()),
            "{msg}"
        );
    }

    #[test]
    fn closing_an_unknown_pane_is_refused() {
        let (mut tree, _) = asymmetric();
        let ghost = PaneId::new();
        assert_eq!(close(&mut tree, ghost), Err(CoreError::NoSuchPane(ghost)));
        validate(&tree).unwrap();
    }

    #[test]
    fn closing_the_focused_pane_moves_focus_into_the_surviving_sibling() {
        let (mut tree, [_p1, p2, p3, p4, _p5]) = asymmetric();

        focus(&mut tree, p2).unwrap();
        close(&mut tree, p2).unwrap();

        // p2's sibling is the split holding p3 and p4; focus lands on its first leaf.
        assert_eq!(tree.focused, p3);
        assert!(leaves(&tree.root).contains(&p4));
        validate(&tree).unwrap();
    }

    #[test]
    fn closing_a_pane_that_is_not_focused_leaves_focus_alone() {
        let (mut tree, [p1, _p2, _p3, p4, _p5]) = asymmetric();
        focus(&mut tree, p1).unwrap();
        close(&mut tree, p4).unwrap();
        assert_eq!(tree.focused, p1);
        validate(&tree).unwrap();
    }

    #[test]
    fn closing_the_maximized_pane_clears_the_flag() {
        let (mut tree, [p1, p2, _p3, p4, _p5]) = asymmetric();
        maximize(&mut tree, Some(p4)).unwrap();
        close(&mut tree, p4).unwrap();
        assert_eq!(tree.maximized, None);

        maximize(&mut tree, Some(p1)).unwrap();
        close(&mut tree, p2).unwrap();
        assert_eq!(
            tree.maximized,
            Some(p1),
            "closing another pane must leave the maximized one alone"
        );
        validate(&tree).unwrap();
    }

    #[test]
    fn take_pane_hands_back_the_pane_it_removed() {
        let (mut tree, [_p1, _p2, p3, _p4, _p5]) = asymmetric();
        let title = tree.panes[&p3].title.clone();

        let taken = take_pane(&mut tree, p3).unwrap();

        assert_eq!(taken.id, p3);
        assert_eq!(taken.title, title);
        assert!(!tree.panes.contains_key(&p3));
        assert!(!leaves(&tree.root).contains(&p3));
        validate(&tree).unwrap();
    }

    #[test]
    fn take_pane_refuses_the_last_pane_like_close_does() {
        let first = aux();
        let a = first.id;
        let mut tree = new_tree(first);
        assert_eq!(take_pane(&mut tree, a).unwrap_err(), CoreError::LastPane);
        validate(&tree).unwrap();
    }

    /// Detach goes through `take_pane`, so this is the assertion that the console's
    /// conversation cannot be moved out into a window of its own either.
    #[test]
    fn take_pane_refuses_the_primary_so_it_can_never_be_detached() {
        let first = primary();
        let a = first.id;
        let mut tree = new_tree(first);
        let b = split_at(&mut tree, a, Axis::Row, Side::After);

        assert_eq!(take_pane(&mut tree, a).unwrap_err(), CoreError::PanePrimary);
        assert_eq!(leaves(&tree.root), vec![a, b]);
        validate_console(&tree).unwrap();
    }

    #[test]
    fn a_taken_pane_redocks_into_another_tree_unchanged() {
        let (mut source, [_p1, _p2, p3, _p4, _p5]) = asymmetric();
        let taken = take_pane(&mut source, p3).unwrap();
        let session = taken.session;

        let host = primary();
        let anchor = host.id;
        let mut target = new_tree(host);
        let landed = insert_pane(&mut target, anchor, Axis::Col, Side::Before, taken).unwrap();

        assert_eq!(landed, p3);
        assert_eq!(leaves(&target.root), vec![p3, anchor]);
        assert_eq!(target.panes[&p3].session, session);
        assert_eq!(target.focused, p3);
        validate(&source).unwrap();
        validate(&target).unwrap();
    }

    #[test]
    fn set_ratio_clamps_at_both_ends_and_reports_what_it_stored() {
        let first = primary();
        let a = first.id;
        let mut tree = new_tree(first);
        split_at(&mut tree, a, Axis::Row, Side::After);
        let mut splits = Vec::new();
        collect_splits(&tree.root, &mut splits);
        let id = splits[0].0;

        assert_eq!(set_ratio(&mut tree, id, -3.0).unwrap(), MIN_RATIO);
        assert_eq!(set_ratio(&mut tree, id, 42.0).unwrap(), MAX_RATIO);
        assert_eq!(set_ratio(&mut tree, id, 0.25).unwrap(), 0.25);

        let mut after = Vec::new();
        collect_splits(&tree.root, &mut after);
        assert_eq!(after[0].1, 0.25);
        validate(&tree).unwrap();
    }

    #[test]
    fn set_ratio_refuses_a_non_finite_value() {
        let first = primary();
        let a = first.id;
        let mut tree = new_tree(first);
        split_at(&mut tree, a, Axis::Row, Side::After);
        let mut splits = Vec::new();
        collect_splits(&tree.root, &mut splits);
        let id = splits[0].0;

        assert!(matches!(
            set_ratio(&mut tree, id, f32::NAN),
            Err(CoreError::Invariant(_))
        ));
        // The stored ratio must survive the rejection, or the layout would go blank.
        validate(&tree).unwrap();
    }

    #[test]
    fn set_ratio_on_an_unknown_split_is_refused() {
        let (mut tree, _) = asymmetric();
        let ghost = SplitId::new();
        assert_eq!(
            set_ratio(&mut tree, ghost, 0.5),
            Err(CoreError::NoSuchSplit(ghost))
        );
    }

    #[test]
    fn set_ratio_moves_only_the_named_divider() {
        let (mut tree, _) = asymmetric();
        let mut splits = Vec::new();
        collect_splits(&tree.root, &mut splits);
        let target = splits[1].0;
        set_ratio(&mut tree, target, 0.8).unwrap();

        let mut after = Vec::new();
        collect_splits(&tree.root, &mut after);
        for (id, ratio) in after {
            let expected = if id == target { 0.8 } else { 0.5 };
            assert_eq!(ratio, expected);
        }
    }

    // --- chain algebra -----------------------------------------------------------------

    /// A chain of `axis` with `n` members and the shape the seed picks, so the properties
    /// below are asserted over combs, balanced trees and everything between.
    fn chain_of(rng: &mut Lcg, axis: Axis, n: usize) -> PaneTree {
        let first = aux();
        let mut tree = new_tree(first);
        for _ in 1..n {
            let ids = leaves(&tree.root);
            let at = ids[rng.next(ids.len())];
            let side = if rng.next(2) == 0 {
                Side::Before
            } else {
                Side::After
            };
            split(&mut tree, at, axis, side, aux()).expect("split of a live leaf");
        }
        tree
    }

    /// Overwrite every ratio in a tree, so a test can build the sub-floor shapes that only a
    /// file written by an older build can reach.
    fn force_ratios(node: &mut LayoutNode, value: f32) {
        if let LayoutNode::Split { a, b, ratio, .. } = node {
            *ratio = value;
            force_ratios(a, value);
            force_ratios(b, value);
        }
    }

    /// Give a chain shares that are legal by construction: every member at or above the
    /// floor, summing to one.
    fn healthy_shares(rng: &mut Lcg, n: usize) -> Vec<f32> {
        let mut f: Vec<f32> = (0..n).map(|_| 1.0 + rng.next(100) as f32).collect();
        let total: f32 = f.iter().sum();
        for w in &mut f {
            *w /= total;
        }
        legalise(&mut f);
        f
    }

    #[test]
    fn write_weights_is_the_exact_inverse_of_weights() {
        for seed in 0..200u64 {
            let mut rng = Lcg(seed.wrapping_mul(0x2545_F491_4F6C_DD1D) | 1);
            let n = 2 + rng.next(7);
            let mut tree = chain_of(&mut rng, Axis::Row, n);
            let want = healthy_shares(&mut rng, n);

            write_weights(&mut tree.root, Axis::Row, &want);
            let got = weights(&tree.root, Axis::Row);

            assert_eq!(got.len(), want.len(), "seed {seed}");
            for (i, (g, w)) in got.iter().zip(&want).enumerate() {
                assert!(
                    (g - w).abs() < 1e-6,
                    "seed {seed} member {i}: {g} != {w} (shape held {n} members)"
                );
            }
            validate(&tree).expect("a written chain is still a legal tree");
        }
    }

    #[test]
    fn every_ratio_write_weights_derives_is_inside_the_clamp_band() {
        // The theorem the whole design rests on: shares at or above `MIN_TILE` can only
        // produce ratios inside the band `validate` already enforced. This is what lets
        // `MIN_RATIO` stay at 0.1 and `validate` stay untouched.
        for seed in 0..300u64 {
            let mut rng = Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 3);
            let n = 2 + rng.next(MAX_MEMBERS - 1);
            let axis = if rng.next(2) == 0 {
                Axis::Row
            } else {
                Axis::Col
            };
            let mut tree = chain_of(&mut rng, axis, n);
            let want = healthy_shares(&mut rng, n);
            assert!(want.iter().all(|w| *w >= MIN_TILE), "seed {seed}: fixture");

            write_weights(&mut tree.root, axis, &want);

            let mut splits = Vec::new();
            collect_splits(&tree.root, &mut splits);
            for (id, ratio) in splits {
                assert!(
                    (MIN_RATIO..=MAX_RATIO).contains(&ratio),
                    "seed {seed}: split {id} landed on {ratio}, outside the band"
                );
            }
        }
    }

    #[test]
    fn a_divider_edit_moves_only_its_two_members() {
        for seed in 0..300u64 {
            let mut rng = Lcg(seed.wrapping_mul(0xD1B5_4A32_D192_ED03) | 1);
            let n = 3 + rng.next(MAX_MEMBERS - 2);
            let mut tree = chain_of(&mut rng, Axis::Row, n);
            write_weights(&mut tree.root, Axis::Row, &healthy_shares(&mut rng, n));

            let before = weights(&tree.root, Axis::Row);
            let mut dividers = Vec::new();
            chain_splits(&tree.root, Axis::Row, &mut dividers);
            let k = rng.next(dividers.len());
            let asked = (rng.next(300) as f32) / 100.0 - 1.0;

            set_ratio(&mut tree, dividers[k], asked).expect("a live divider moves");

            let after = weights(&tree.root, Axis::Row);
            for i in 0..n {
                if i == k || i == k + 1 {
                    continue;
                }
                // Exact in ℝ — the pair's sum is preserved, so no ancestor's *total* moves —
                // and within a couple of ulp in f32, because the ancestors' ratios really are
                // recomputed and a share is a product of them. A pixel is 1e-3 of a tab, so
                // 1e-6 is three orders of magnitude tighter than anything visible.
                assert!(
                    (after[i] - before[i]).abs() < 1e-6,
                    "seed {seed}: member {i} moved when divider {k} did: {} -> {}",
                    before[i],
                    after[i]
                );
            }
            // And the pair kept its own budget, so nothing outside it had to give.
            assert!(
                ((after[k] + after[k + 1]) - (before[k] + before[k + 1])).abs() < 1e-6,
                "seed {seed}: the pair's own share changed"
            );
            validate(&tree).expect("valid");
        }
    }

    #[test]
    fn divider_k_separates_members_k_and_k_plus_one_in_a_left_leaning_chain() {
        // The one contract this module shares with the renderer: divider `k` is the `k`-th
        // interior split *in in-order*. Every other test here reaches a divider through
        // `chain_splits` and so agrees with whatever order that function happens to use —
        // swap it for pre-order and the entire suite still passes while a drag moves the
        // wrong pair. This test reads the two ids off the tree by what they physically
        // separate instead, which is the only way to pin it.
        //
        // Left-leaning on purpose: `add_row(None, After)` wraps the whole root, so a tab
        // built by appending rows is `Col(Col(r1, r2), r3)` — the one shape where in-order
        // and pre-order disagree. `pane.addRow` defaults `after` to null, so that is the
        // shape the default gesture builds.
        let first = aux();
        let r1 = first.id;
        let mut tree = new_tree(first);
        let r2 = add_row(&mut tree, None, Side::After, aux()).expect("row 2");
        add_row(&mut tree, None, Side::After, aux()).expect("row 3");

        let LayoutNode::Split { id: outer, a, .. } = &tree.root else {
            panic!("appending rows leaves a Col spine at the root");
        };
        let LayoutNode::Split { id: inner, .. } = a.as_ref() else {
            panic!("with the older two rows nested inside it");
        };
        // `inner` divides r1 from r2; `outer` divides {r1, r2} from r3.
        assert_eq!(leaves(a), vec![r1, r2]);
        let (outer, inner) = (*outer, *inner);

        let mut dividers = Vec::new();
        chain_splits(&tree.root, Axis::Col, &mut dividers);
        assert_eq!(
            dividers,
            vec![inner, outer],
            "divider 0 is the one between rows 1 and 2, whichever way the tree leans"
        );

        // And the behaviour that indexing buys: the *lower* divider leaves row 1 alone.
        let before = weights(&tree.root, Axis::Col);
        set_ratio(&mut tree, outer, 0.8).expect("the lower divider moves");
        let after = weights(&tree.root, Axis::Col);
        assert!(
            (after[0] - before[0]).abs() < 1e-6,
            "row 1 sits above that divider: {before:?} -> {after:?}"
        );
        assert!(after[1] > before[1] && after[2] < before[2], "{after:?}");

        // ...and the upper one leaves row 3 alone.
        let before = weights(&tree.root, Axis::Col);
        set_ratio(&mut tree, inner, 0.25).expect("the upper divider moves");
        let after = weights(&tree.root, Axis::Col);
        assert!(
            (after[2] - before[2]).abs() < 1e-6,
            "row 3 sits below it: {before:?} -> {after:?}"
        );
        assert!(after[0] < before[0] && after[1] > before[1], "{after:?}");
        validate(&tree).expect("valid");
    }

    #[test]
    fn set_ratio_on_a_two_leaf_split_behaves_exactly_as_it_did() {
        // The shipped console's shape. A pair that is the whole chain has `pair share` and
        // `a`'s share meaning the same number, which is why no existing caller changed.
        let first = primary();
        let a = first.id;
        let mut tree = new_tree(first);
        split_at(&mut tree, a, Axis::Row, Side::After);
        let mut splits = Vec::new();
        collect_splits(&tree.root, &mut splits);
        let id = splits[0].0;

        for asked in [0.2_f32, 0.35, 0.5, 0.75, 0.9] {
            assert_eq!(set_ratio(&mut tree, id, asked).unwrap(), asked);
            let mut after = Vec::new();
            collect_splits(&tree.root, &mut after);
            assert_eq!(after[0].1, asked, "the stored ratio is the share asked for");
        }
    }

    #[test]
    fn set_ratio_on_a_legacy_chain_with_sub_floor_members_still_leaves_a_valid_tree() {
        // Three nested splits at the floor give a 1% leaf while every stored ratio is legal
        // — reachable on a file written by the current build, which is why the backstop
        // clamp inside `write_weights` exists at all.
        let first = aux();
        let a = first.id;
        let mut tree = new_tree(first);
        let b = split_at(&mut tree, a, Axis::Row, Side::After);
        let c = split_at(&mut tree, b, Axis::Row, Side::After);
        split_at(&mut tree, c, Axis::Row, Side::After);
        // Written straight onto the nodes: every ratio here is legal, and no gesture can
        // produce the shares they multiply out to, which is the whole point.
        force_ratios(&mut tree.root, MIN_RATIO);
        validate(&tree).expect("the fixture is a tree the current build could have written");
        let shares = weights(&tree.root, Axis::Row);
        assert!(
            shares.iter().any(|w| *w < MIN_TILE),
            "the fixture must really hold a sub-floor member, got {shares:?}"
        );

        let mut dividers = Vec::new();
        chain_splits(&tree.root, Axis::Row, &mut dividers);
        let stored = set_ratio(&mut tree, dividers[1], 0.5).expect("moves");

        let after = weights(&tree.root, Axis::Row);
        let actual = after[1] / (after[1] + after[2]);
        assert!(
            (stored - actual).abs() < 1e-6,
            "the reported share {stored} is not the one stored, {actual}"
        );
        assert!(
            (MIN_RATIO..=MAX_RATIO).contains(&stored),
            "and it is still a number the wire promises: {stored}"
        );
        validate(&tree).expect("the backstop kept every ratio inside the band");
    }

    // --- rows and tiles ----------------------------------------------------------------

    /// The shares of the tab's rows, and of the tiles within each row.
    fn shape(tree: &PaneTree) -> (Vec<f32>, Vec<Vec<f32>>) {
        let spine = weights(&tree.root, Axis::Col);
        let mut rows = Vec::new();
        chain_members(&tree.root, Axis::Col, &mut rows);
        let tiles = rows.iter().map(|r| weights(r, Axis::Row)).collect();
        (spine, tiles)
    }

    fn close_to(got: &[f32], want: &[f32]) -> bool {
        got.len() == want.len() && got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-5)
    }

    #[test]
    fn add_tile_four_times_gives_four_equal_tiles() {
        let first = aux();
        let a = first.id;
        let mut tree = new_tree(first);
        let mut last = a;
        for _ in 0..3 {
            last = add_tile(&mut tree, last, Side::After, aux()).expect("a tile joins the row");
        }

        let (spine, tiles) = shape(&tree);
        assert!(close_to(&spine, &[1.0]), "one row: {spine:?}");
        assert!(
            close_to(&tiles[0], &[0.25; 4]),
            "four equal tiles: {tiles:?}"
        );
        assert_eq!(tree.focused, last);
        validate(&tree).expect("valid");
    }

    #[test]
    fn add_tile_preserves_the_relative_widths_of_a_hand_tuned_row() {
        let first = aux();
        let a = first.id;
        let mut tree = new_tree(first);
        let b = add_tile(&mut tree, a, Side::After, aux()).expect("joins");
        let mut dividers = Vec::new();
        chain_splits(&tree.root, Axis::Row, &mut dividers);
        set_ratio(&mut tree, dividers[0], 0.75).expect("moves");

        add_tile(&mut tree, b, Side::After, aux()).expect("joins");

        let (_, tiles) = shape(&tree);
        // 75/25 scaled by 2/3, plus a third of the row for the newcomer. Equalising instead
        // would have thrown the tuning away.
        assert!(
            close_to(&tiles[0], &[0.5, 1.0 / 6.0, 1.0 / 3.0]),
            "{tiles:?}"
        );
        validate(&tree).expect("valid");
    }

    // --- evening a row out --------------------------------------------------------------

    #[test]
    fn distribute_evens_out_a_hand_tuned_row() {
        let first = aux();
        let a = first.id;
        let mut tree = new_tree(first);
        let b = add_tile(&mut tree, a, Side::After, aux()).expect("joins");
        add_tile(&mut tree, b, Side::After, aux()).expect("joins");
        let mut dividers = Vec::new();
        chain_splits(&tree.root, Axis::Row, &mut dividers);
        set_ratio(&mut tree, dividers[0], 0.8).expect("moves");
        set_ratio(&mut tree, dividers[1], 0.75).expect("moves");

        distribute(&mut tree, b, Axis::Row).expect("evens out");

        let (_, tiles) = shape(&tree);
        assert!(close_to(&tiles[0], &[1.0 / 3.0; 3]), "thirds: {tiles:?}");
        validate(&tree).expect("valid");
    }

    /// The claim that makes this safe to reach for: it is the row, not the tab.
    #[test]
    fn distribute_leaves_every_other_chain_alone() {
        let first = aux();
        let a = first.id;
        let mut tree = new_tree(first);
        let b = add_tile(&mut tree, a, Side::After, aux()).expect("joins");
        let second = add_row(&mut tree, Some(b), Side::After, aux()).expect("a row is added");
        let sibling = add_tile(&mut tree, second, Side::After, aux()).expect("joins row two");
        let mut spine = Vec::new();
        chain_splits(&tree.root, Axis::Col, &mut spine);
        set_ratio(&mut tree, spine[0], 0.7).expect("moves");
        let mut dividers = Vec::new();
        chain_splits(&tree.root, Axis::Row, &mut dividers);
        // Row one only. `chain_splits` on the whole tree walks the `Col` spine, so row one's
        // divider is not in `dividers` — take it from the row itself.
        let mut path = Vec::new();
        let m = locate_member(&tree.root, a, Axis::Row, &mut path).expect("a is a tile of row one");
        assert_eq!(m, 0);
        let mut row_one = Vec::new();
        chain_splits(node_at(&tree.root, &path), Axis::Row, &mut row_one);
        set_ratio(&mut tree, row_one[0], 0.85).expect("moves");

        distribute(&mut tree, a, Axis::Row).expect("evens out");

        let (spine_shares, tiles) = shape(&tree);
        assert!(
            close_to(&tiles[0], &[0.5, 0.5]),
            "row one is even: {tiles:?}"
        );
        assert!(
            close_to(&spine_shares, &[0.7, 0.3]),
            "the rows keep the heights the user dragged them to: {spine_shares:?}",
        );
        assert!(
            close_to(&tiles[1], &[0.5, 0.5]),
            "and row two is untouched: {tiles:?}",
        );
        assert!(tree.panes.contains_key(&sibling));
        validate(&tree).expect("valid");
    }

    #[test]
    fn distribute_evens_the_rows_of_a_tab_when_asked_for_the_column() {
        let first = aux();
        let a = first.id;
        let mut tree = new_tree(first);
        add_row(&mut tree, None, Side::After, aux()).expect("a row is added");
        add_row(&mut tree, None, Side::After, aux()).expect("a row is added");
        let mut spine = Vec::new();
        chain_splits(&tree.root, Axis::Col, &mut spine);
        set_ratio(&mut tree, spine[0], 0.8).expect("moves");

        distribute(&mut tree, a, Axis::Col).expect("evens out");

        let (spine_shares, _) = shape(&tree);
        assert!(close_to(&spine_shares, &[1.0 / 3.0; 3]), "{spine_shares:?}");
        validate(&tree).expect("valid");
    }

    /// A pane stacked inside one cell is a member of no row of its own, so the row it means
    /// is the one its *cell* is a tile of.
    #[test]
    fn distribute_reaches_the_row_a_stacked_pane_sits_in() {
        let first = aux();
        let a = first.id;
        let mut tree = new_tree(first);
        let b = add_tile(&mut tree, a, Side::After, aux()).expect("joins");
        let mut dividers = Vec::new();
        chain_splits(&tree.root, Axis::Row, &mut dividers);
        set_ratio(&mut tree, dividers[0], 0.8).expect("moves");
        let stacked = split_at(&mut tree, b, Axis::Col, Side::After);

        distribute(&mut tree, stacked, Axis::Row).expect("evens out");

        let (_, tiles) = shape(&tree);
        assert!(
            close_to(&tiles[0], &[0.5, 0.5]),
            "the two cells of the row are even: {tiles:?}",
        );
        validate(&tree).expect("valid");
    }

    /// And when there *is* an inner row, that one is the one it means — the maximal chain
    /// around the pane, not the outermost row on its path.
    #[test]
    fn distribute_evens_the_innermost_row_around_the_pane() {
        let first = aux();
        let outer = first.id;
        let mut tree = new_tree(first);
        let cell = add_tile(&mut tree, outer, Side::After, aux()).expect("joins");
        let mut dividers = Vec::new();
        chain_splits(&tree.root, Axis::Row, &mut dividers);
        set_ratio(&mut tree, dividers[0], 0.8).expect("moves");
        // A column inside the second cell, then a row inside *that* — the inner row is the
        // one the pane is a tile of.
        let stacked = split_at(&mut tree, cell, Axis::Col, Side::After);
        let inner = split_at(&mut tree, stacked, Axis::Row, Side::After);
        let mut path = Vec::new();
        locate_member(&tree.root, inner, Axis::Row, &mut path).expect("a tile of the inner row");
        let mut inner_dividers = Vec::new();
        chain_splits(node_at(&tree.root, &path), Axis::Row, &mut inner_dividers);
        set_ratio(&mut tree, inner_dividers[0], 0.9).expect("moves");

        distribute(&mut tree, inner, Axis::Row).expect("evens out");

        let (_, tiles) = shape(&tree);
        assert!(
            close_to(&tiles[0], &[0.8, 0.2]),
            "the outer row keeps the widths the user dragged: {tiles:?}",
        );
        let mut after = Vec::new();
        locate_member(&tree.root, inner, Axis::Row, &mut after).expect("still a tile of it");
        assert!(
            close_to(
                &weights(node_at(&tree.root, &after), Axis::Row),
                &[0.5, 0.5]
            ),
            "and the inner row is even",
        );
        validate(&tree).expect("valid");
    }

    #[test]
    fn distribute_on_a_pane_with_no_row_of_its_own_changes_nothing() {
        let (mut tree, [p1, ..]) = asymmetric();
        let before = tree.clone();
        // `asymmetric`'s root is a `Col`, and p1 hangs off it with no `Row` above it.
        distribute(&mut tree, p1, Axis::Row).expect("a row of one is a no-op, not an error");
        assert_eq!(tree, before);
    }

    #[test]
    fn distribute_refuses_a_pane_that_is_not_in_the_tree() {
        let (mut tree, _) = asymmetric();
        let ghost = PaneId::new();
        assert_eq!(
            distribute(&mut tree, ghost, Axis::Row),
            Err(CoreError::NoSuchPane(ghost)),
        );
        validate(&tree).unwrap();
    }

    /// The report this answers: four equal panes, close one, and the row went 50/25/25
    /// because the collapse handed the whole gap to one neighbour.
    #[test]
    fn closing_a_tile_spreads_its_width_over_the_rest_of_the_row() {
        let first = aux();
        let mut ids = vec![first.id];
        let mut tree = new_tree(first);
        for _ in 0..3 {
            let last = *ids.last().expect("seeded");
            ids.push(add_tile(&mut tree, last, Side::After, aux()).expect("a tile joins"));
        }

        // The second tile, whose sibling in the binary tree is the subtree holding the other
        // two — the shape that used to make one neighbour twice the size of the rest.
        close(&mut tree, ids[1]).expect("closes");

        let (spine, tiles) = shape(&tree);
        assert!(close_to(&spine, &[1.0]), "still one row: {spine:?}");
        assert!(
            close_to(&tiles[0], &[1.0 / 3.0; 3]),
            "three equal tiles, exactly as adding a third would have built: {tiles:?}"
        );
        validate(&tree).expect("valid");
    }

    #[test]
    fn closing_a_tile_preserves_the_relative_widths_of_the_survivors() {
        let first = aux();
        let a = first.id;
        let mut tree = new_tree(first);
        let b = add_tile(&mut tree, a, Side::After, aux()).expect("joins");
        add_tile(&mut tree, b, Side::After, aux()).expect("joins");
        let mut dividers = Vec::new();
        chain_splits(&tree.root, Axis::Row, &mut dividers);
        // Thirds, then the second divider moved: 1/3, 1/2, 1/6.
        set_ratio(&mut tree, dividers[1], 0.75).expect("moves");

        close(&mut tree, a).expect("closes");

        let (_, tiles) = shape(&tree);
        // 1/2 and 1/6 renormalised. Equalising instead would have thrown the tuning away,
        // just as it would on the way in.
        assert!(close_to(&tiles[0], &[0.75, 0.25]), "{tiles:?}");
        validate(&tree).expect("valid");
    }

    #[test]
    fn closing_a_row_gives_its_height_to_the_other_rows() {
        let first = aux();
        let mut tree = new_tree(first);
        let second = add_row(&mut tree, None, Side::After, aux()).expect("a row is added");
        add_row(&mut tree, None, Side::After, aux()).expect("a row is added");

        close(&mut tree, second).expect("closes");

        let (spine, _) = shape(&tree);
        assert!(close_to(&spine, &[0.5, 0.5]), "two equal rows: {spine:?}");
        validate(&tree).expect("valid");
    }

    /// A pane whose parent is a cross-axis split has no row to spread across: the cell it
    /// shared is the only thing that may grow, and the rest of the tab must not move.
    #[test]
    fn closing_a_stacked_tile_leaves_the_other_rows_alone() {
        let first = aux();
        let a = first.id;
        let mut tree = new_tree(first);
        let b = add_tile(&mut tree, a, Side::After, aux()).expect("joins");
        // Stacked *inside* b's cell rather than as a full-width row.
        let stacked = split_at(&mut tree, b, Axis::Col, Side::After);

        close(&mut tree, stacked).expect("closes");

        let (spine, tiles) = shape(&tree);
        assert!(close_to(&spine, &[1.0]), "one row: {spine:?}");
        assert!(close_to(&tiles[0], &[0.5, 0.5]), "untouched: {tiles:?}");
        validate(&tree).expect("valid");
    }

    /// [`take_pane`] keeps the bare collapse, so detach and re-dock stays the round trip it
    /// is documented to be.
    #[test]
    fn detaching_a_tile_does_not_spread_the_row() {
        let first = aux();
        let mut ids = vec![first.id];
        let mut tree = new_tree(first);
        for _ in 0..3 {
            let last = *ids.last().expect("seeded");
            ids.push(add_tile(&mut tree, last, Side::After, aux()).expect("a tile joins"));
        }
        let anchor = anchor_of(&tree, ids[1]).expect("has a parent");

        let taken = take_pane(&mut tree, ids[1]).expect("leaves");
        insert_pane_at(&mut tree, &anchor, taken).expect("comes back");

        let (_, tiles) = shape(&tree);
        assert!(
            close_to(&tiles[0], &[0.25; 4]),
            "the row is exactly as it was: {tiles:?}"
        );
        validate(&tree).expect("valid");
    }

    #[test]
    fn add_row_on_a_one_row_tab_gives_fifty_fifty() {
        let first = aux();
        let a = first.id;
        let mut tree = new_tree(first);
        add_tile(&mut tree, a, Side::After, aux()).expect("joins");

        let new = add_row(&mut tree, Some(a), Side::After, aux()).expect("a row is added");

        let (spine, tiles) = shape(&tree);
        assert!(close_to(&spine, &[0.5, 0.5]), "50/50: {spine:?}");
        assert!(close_to(&tiles[1], &[1.0]), "the new row holds one tile");
        assert_eq!(tree.focused, new);
        validate(&tree).expect("valid");
    }

    #[test]
    fn the_users_layout_is_four_tiles_then_two_at_fifty_fifty() {
        let first = aux();
        let a = first.id;
        let mut tree = new_tree(first);
        let mut last = a;
        for _ in 0..3 {
            last = add_tile(&mut tree, last, Side::After, aux()).expect("joins");
        }
        let second = add_row(&mut tree, Some(last), Side::After, aux()).expect("a row is added");
        add_tile(&mut tree, second, Side::After, aux()).expect("joins the new row");

        let (spine, tiles) = shape(&tree);
        assert!(close_to(&spine, &[0.5, 0.5]), "two rows, 50/50: {spine:?}");
        assert!(close_to(&tiles[0], &[0.25; 4]), "row 1 holds four tiles");
        assert!(close_to(&tiles[1], &[0.5, 0.5]), "row 2 holds two");
        assert_eq!(leaves(&tree.root).len(), 6);
        validate(&tree).expect("valid");

        // The claim the user actually made: a divider in row 1 must not move row 2.
        let mut rows = Vec::new();
        chain_members(&tree.root, Axis::Col, &mut rows);
        let mut row_one = Vec::new();
        chain_splits(rows[0], Axis::Row, &mut row_one);
        let before = shape(&tree);
        set_ratio(&mut tree, row_one[0], 0.8).expect("moves");
        let after = shape(&tree);

        assert_eq!(after.0, before.0, "the rows' own heights must not move");
        assert_eq!(after.1[1], before.1[1], "row 2's tiles must not move");
        assert!(
            close_to(&after.1[0], &[0.4, 0.1, 0.25, 0.25]),
            "{:?}",
            after.1[0]
        );
    }

    #[test]
    fn add_row_inserts_below_the_named_panes_row_not_at_the_bottom() {
        let first = aux();
        let a = first.id;
        let mut tree = new_tree(first);
        let second = add_row(&mut tree, Some(a), Side::After, aux()).expect("row 2");
        let third = add_row(&mut tree, Some(second), Side::After, aux()).expect("row 3");

        let inserted = add_row(&mut tree, Some(a), Side::After, aux()).expect("between 1 and 2");

        let mut rows = Vec::new();
        chain_members(&tree.root, Axis::Col, &mut rows);
        let order: Vec<PaneId> = rows.iter().map(|r| leaves(r)[0]).collect();
        assert_eq!(order, vec![a, inserted, second, third]);
        validate(&tree).expect("valid");
    }

    #[test]
    fn add_tile_refuses_an_eleventh_pane_in_a_row() {
        let first = aux();
        let a = first.id;
        let mut tree = new_tree(first);
        let mut last = a;
        for _ in 1..MAX_MEMBERS {
            last = add_tile(&mut tree, last, Side::After, aux()).expect("joins");
        }
        assert_eq!(weights(&tree.root, Axis::Row).len(), MAX_MEMBERS);

        let Err(CoreError::Invariant(msg)) = add_tile(&mut tree, last, Side::After, aux()) else {
            panic!("an eleventh tile must be refused");
        };
        assert_eq!(msg, "a row already holds 10 panes; add a row instead");
        validate(&tree).expect("the refusal left the tree alone");
    }

    #[test]
    fn add_row_refuses_an_eleventh_row() {
        let first = aux();
        let a = first.id;
        let mut tree = new_tree(first);
        let mut last = a;
        for _ in 1..MAX_MEMBERS {
            last = add_row(&mut tree, Some(last), Side::After, aux()).expect("a row is added");
        }
        assert_eq!(weights(&tree.root, Axis::Col).len(), MAX_MEMBERS);

        let Err(CoreError::Invariant(msg)) = add_row(&mut tree, Some(last), Side::After, aux())
        else {
            panic!("an eleventh row must be refused");
        };
        assert_eq!(msg, "a tab already holds 10 rows");
        validate(&tree).expect("the refusal left the tree alone");
    }

    #[test]
    fn add_row_mints_one_split_and_leaves_every_existing_split_id_live() {
        let (mut tree, [p1, ..]) = asymmetric();
        let before: Vec<SplitId> = {
            let mut out = Vec::new();
            collect_splits(&tree.root, &mut out);
            out.into_iter().map(|(id, _)| id).collect()
        };

        add_row(&mut tree, Some(p1), Side::After, aux()).expect("a row is added");

        let after: Vec<SplitId> = {
            let mut out = Vec::new();
            collect_splits(&tree.root, &mut out);
            out.into_iter().map(|(id, _)| id).collect()
        };
        assert_eq!(after.len(), before.len() + 1, "exactly one new divider");
        for id in &before {
            assert!(after.contains(id), "divider {id} went away");
        }
    }

    #[test]
    fn a_dock_anchor_taken_before_add_row_still_restores_exactly() {
        // The reason `add_row` mints ids rather than re-minting them: a persisted anchor
        // must stay exactly as valid as it was, or a detached pane comes home to a guess.
        let (mut tree, [_p1, _p2, p3, p4, _p5]) = asymmetric();
        let anchor = anchor_of(&tree, p4).expect("has a parent");
        let taken = take_pane(&mut tree, p4).expect("leaves");

        add_row(&mut tree, Some(p3), Side::After, aux()).expect("a row is added while it is out");

        assert!(
            can_restore(&tree, &anchor),
            "the anchor survived the new row"
        );
        insert_pane_at(&mut tree, &anchor, taken).expect("restores");
        let mut splits = Vec::new();
        collect_splits(&tree.root, &mut splits);
        let (_, ratio) = splits
            .iter()
            .find(|(id, _)| *id == anchor.split)
            .expect("the recorded divider is back, under its own id");
        assert_eq!(*ratio, anchor.ratio);
        validate(&tree).expect("valid");
    }

    #[test]
    fn add_row_clears_maximized_and_takes_focus() {
        let (mut tree, [p1, p2, ..]) = asymmetric();
        maximize(&mut tree, Some(p2)).expect("maximizes");

        let new = add_row(&mut tree, Some(p1), Side::After, aux()).expect("a row is added");

        assert_eq!(tree.focused, new);
        assert_eq!(tree.maximized, None, "a hidden new row is no use to anyone");
        validate(&tree).expect("valid");
    }

    #[test]
    fn add_row_on_the_console_keeps_validate_console_passing() {
        let first = primary();
        let a = first.id;
        let mut tree = new_tree(first);

        add_row(&mut tree, Some(a), Side::After, aux()).expect("a row is added");
        add_tile(&mut tree, a, Side::After, aux()).expect("a tile joins the primary's row");

        validate_console(&tree).expect("the console still holds its conversation");
        assert_eq!(primary_of(&tree), Some(a));
    }

    #[test]
    fn add_row_and_add_tile_on_a_file_tab_produce_a_valid_tree() {
        // A file tab's editor pane is `Auxiliary` and holds no session; neither gesture may
        // move it or change what it is.
        let editor = Pane {
            id: PaneId::new(),
            kind: PaneKind::Editor,
            role: PaneRole::Auxiliary,
            session: None,
            conversation: None,
            title: "workspace.rs".into(),
        };
        let e = editor.id;
        let mut tree = new_tree(editor);

        add_tile(&mut tree, e, Side::After, aux()).expect("a tile joins");
        add_row(&mut tree, Some(e), Side::After, aux()).expect("a row is added");

        assert_eq!(leaves(&tree.root)[0], e, "the editor keeps its position");
        assert_eq!(tree.panes[&e].kind, PaneKind::Editor);
        assert_eq!(tree.panes[&e].role, PaneRole::Auxiliary);
        assert!(tree.panes[&e].session.is_none());
        validate(&tree).expect("valid");
    }

    #[test]
    fn add_tile_and_add_row_refuse_a_pane_that_is_not_there() {
        let (mut tree, _) = asymmetric();
        let ghost = PaneId::new();
        assert_eq!(
            add_tile(&mut tree, ghost, Side::After, aux()),
            Err(CoreError::NoSuchPane(ghost))
        );
        assert_eq!(
            add_row(&mut tree, Some(ghost), Side::After, aux()),
            Err(CoreError::NoSuchPane(ghost))
        );
    }

    #[test]
    fn add_row_with_no_anchor_wraps_the_whole_tab() {
        let (mut tree, _) = asymmetric();
        let top = add_row(&mut tree, None, Side::Before, aux()).expect("a row is added");
        let bottom = add_row(&mut tree, None, Side::After, aux()).expect("another");

        let mut rows = Vec::new();
        chain_members(&tree.root, Axis::Col, &mut rows);
        assert_eq!(rows.len(), 3, "the old tree became the middle row");
        assert_eq!(leaves(rows[0]), vec![top]);
        assert_eq!(leaves(rows[2]), vec![bottom]);
        validate(&tree).expect("valid");
    }

    #[test]
    fn focus_refuses_a_pane_that_is_not_in_the_tree() {
        let (mut tree, [p1, ..]) = asymmetric();
        let held = tree.focused;
        let ghost = PaneId::new();

        assert_eq!(focus(&mut tree, ghost), Err(CoreError::NoSuchPane(ghost)));
        assert_eq!(tree.focused, held, "a refused focus must not move it");

        focus(&mut tree, p1).unwrap();
        assert_eq!(tree.focused, p1);
    }

    #[test]
    fn navigate_crosses_the_nearest_ancestor_split_on_the_axis() {
        let (tree, [p1, p2, p3, p4, p5]) = asymmetric();

        assert_eq!(navigate(&tree, p1, Direction::Right), Some(p2));
        assert_eq!(navigate(&tree, p2, Direction::Left), Some(p1));
        assert_eq!(navigate(&tree, p3, Direction::Right), Some(p4));
        assert_eq!(navigate(&tree, p4, Direction::Left), Some(p3));
        assert_eq!(navigate(&tree, p1, Direction::Down), Some(p5));
        assert_eq!(navigate(&tree, p5, Direction::Up), Some(p1));
        assert_eq!(navigate(&tree, p2, Direction::Down), Some(p3));
        assert_eq!(navigate(&tree, p3, Direction::Up), Some(p2));
        assert_eq!(navigate(&tree, p4, Direction::Up), Some(p2));
    }

    #[test]
    fn navigate_returns_none_at_every_edge() {
        let (tree, [p1, p2, p3, p4, p5]) = asymmetric();

        assert_eq!(navigate(&tree, p1, Direction::Left), None);
        assert_eq!(navigate(&tree, p1, Direction::Up), None);
        assert_eq!(navigate(&tree, p5, Direction::Left), None);
        assert_eq!(navigate(&tree, p5, Direction::Down), None);
        assert_eq!(navigate(&tree, p2, Direction::Up), None);
        assert_eq!(navigate(&tree, p2, Direction::Right), None);
        assert_eq!(navigate(&tree, p3, Direction::Down), None);
        assert_eq!(navigate(&tree, p4, Direction::Down), None);
        assert_eq!(navigate(&tree, p4, Direction::Right), None);
    }

    #[test]
    fn navigate_across_a_perpendicular_split_takes_the_tile_under_the_cursor() {
        // p5 is bottom left; the subtree to its right is split top/bottom, and the old rule
        // took the top child because the core had no geometry to consult. It does now —
        // `weights` is that geometry — so `Right` from the bottom-left pane lands on the
        // bottom-middle one. The old expectation was pinned so that this change would be a
        // deliberate one rather than an accident; this is that change.
        let (tree, [_p1, _p2, p3, _p4, p5]) = asymmetric();
        assert_eq!(navigate(&tree, p5, Direction::Right), Some(p3));
    }

    #[test]
    fn navigate_down_from_the_third_tile_of_a_row_lands_under_it() {
        // The layout the user asked for: four tiles over two. `Down` from tile 3 has to
        // reach the tile it is sitting on, which under the old first-child rule was always
        // the leftmost one whatever the source's position.
        let first = aux();
        let a = first.id;
        let mut tree = new_tree(first);
        let b = add_tile(&mut tree, a, Side::After, aux()).unwrap();
        let c = add_tile(&mut tree, b, Side::After, aux()).unwrap();
        let d = add_tile(&mut tree, c, Side::After, aux()).unwrap();
        let e = add_row(&mut tree, Some(a), Side::After, aux()).unwrap();
        let f = add_tile(&mut tree, e, Side::After, aux()).unwrap();

        // Row two is 25/75, so no tile's midpoint lands on a divider in the row above and
        // every answer below is the one a ruler gives. (Exact ties do happen — two tiles
        // over four is nothing but ties — and go to the first child by the rule in
        // `boundary_leaf`; pinning one here would be pinning f32 rounding.)
        let mut row_two = Vec::new();
        chain_splits(node_at(&tree.root, &[true]), Axis::Row, &mut row_two);
        set_ratio(&mut tree, row_two[0], 0.25).expect("moves");

        // Tile 1 spans [0, .25) and sits over `e`; tiles 2, 3 and 4 span the rest, over `f`.
        assert_eq!(navigate(&tree, a, Direction::Down), Some(e));
        assert_eq!(navigate(&tree, b, Direction::Down), Some(f));
        assert_eq!(navigate(&tree, c, Direction::Down), Some(f));
        assert_eq!(navigate(&tree, d, Direction::Down), Some(f));
        // And back up, into the tile whose span holds the source's midpoint. `f`'s midpoint
        // is 0.625, which is inside tile 3 — the old first-child rule always answered tile 1.
        assert_eq!(navigate(&tree, e, Direction::Up), Some(a));
        assert_eq!(navigate(&tree, f, Direction::Up), Some(c));
    }

    #[test]
    fn navigate_from_an_unknown_pane_is_none_rather_than_a_guess() {
        let (tree, _) = asymmetric();
        assert_eq!(navigate(&tree, PaneId::new(), Direction::Left), None);
    }

    #[test]
    fn navigate_in_a_single_pane_tree_is_none_in_all_directions() {
        let first = primary();
        let a = first.id;
        let tree = new_tree(first);
        for dir in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            assert_eq!(navigate(&tree, a, dir), None);
        }
    }

    #[test]
    fn swap_exchanges_two_positions_and_nothing_else() {
        let (mut tree, [p1, p2, p3, p4, p5]) = asymmetric();
        let before = tree.panes.clone();

        swap(&mut tree, p1, p4).unwrap();

        assert_eq!(leaves(&tree.root), vec![p4, p5, p2, p3, p1]);
        assert_eq!(
            tree.panes, before,
            "swapping positions must not touch pane data"
        );
        validate(&tree).unwrap();
    }

    #[test]
    fn swapping_a_pane_with_itself_changes_nothing() {
        let (mut tree, [p1, ..]) = asymmetric();
        let before = tree.clone();
        swap(&mut tree, p1, p1).unwrap();
        assert_eq!(tree, before);
    }

    #[test]
    fn swapping_with_an_unknown_pane_is_refused() {
        let (mut tree, [p1, ..]) = asymmetric();
        let before = tree.clone();
        let ghost = PaneId::new();

        assert_eq!(
            swap(&mut tree, p1, ghost),
            Err(CoreError::NoSuchPane(ghost))
        );
        assert_eq!(
            swap(&mut tree, ghost, p1),
            Err(CoreError::NoSuchPane(ghost))
        );
        assert_eq!(tree, before);
    }

    #[test]
    fn maximize_sets_and_clears_a_flag_without_reshaping_the_tree() {
        let (mut tree, [p1, _p2, p3, ..]) = asymmetric();
        let shape = tree.root.clone();

        maximize(&mut tree, Some(p3)).unwrap();
        assert_eq!(tree.maximized, Some(p3));
        assert_eq!(tree.root, shape);

        maximize(&mut tree, None).unwrap();
        assert_eq!(tree.maximized, None);
        assert_eq!(tree.root, shape);

        maximize(&mut tree, Some(p1)).unwrap();
        validate(&tree).unwrap();
    }

    #[test]
    fn maximizing_takes_focus() {
        let (mut tree, [p1, _p2, p3, ..]) = asymmetric();
        focus(&mut tree, p1).unwrap();

        maximize(&mut tree, Some(p3)).unwrap();

        // Without this the renderer would show p3 alone while p1 kept the keystrokes.
        assert_eq!(tree.focused, p3);
        validate(&tree).unwrap();
    }

    #[test]
    fn focusing_elsewhere_un_maximizes() {
        let (mut tree, [p1, _p2, p3, ..]) = asymmetric();
        maximize(&mut tree, Some(p3)).unwrap();

        focus(&mut tree, p1).unwrap();

        assert_eq!(tree.maximized, None, "p1 is hidden while p3 is maximized");
        assert_eq!(tree.focused, p1);
        validate(&tree).unwrap();
    }

    #[test]
    fn focusing_the_maximized_pane_keeps_it_maximized() {
        let (mut tree, [_p1, _p2, p3, ..]) = asymmetric();
        maximize(&mut tree, Some(p3)).unwrap();

        focus(&mut tree, p3).unwrap();

        assert_eq!(tree.maximized, Some(p3));
        validate(&tree).unwrap();
    }

    #[test]
    fn splitting_while_maximized_reveals_the_new_pane() {
        let (mut tree, [p1, ..]) = asymmetric();
        maximize(&mut tree, Some(p1)).unwrap();

        let fresh = split(&mut tree, p1, Axis::Row, Side::After, aux()).unwrap();

        // The new pane takes focus, so leaving the flag set would hide the thing the user
        // just asked for.
        assert_eq!(tree.maximized, None);
        assert_eq!(tree.focused, fresh);
        validate(&tree).unwrap();
    }

    #[test]
    fn validate_rejects_focus_and_maximize_disagreeing() {
        let (mut tree, [p1, _p2, p3, ..]) = asymmetric();
        maximize(&mut tree, Some(p3)).unwrap();
        // Reach past the operations to the field, as a corrupt file or a future op could.
        tree.focused = p1;

        let Err(CoreError::Invariant(msg)) = validate(&tree) else {
            panic!("expected an invariant breach");
        };
        assert!(msg.contains("maximized"), "unhelpful message: {msg}");
    }

    #[test]
    fn a_stale_split_id_is_reported_as_such_even_with_a_bad_ratio() {
        let (mut tree, _) = asymmetric();
        let ghost = SplitId::new();

        // Existence outranks validity: someone debugging a dropped drag needs to be told
        // the divider is gone, not that their float was odd.
        assert_eq!(
            set_ratio(&mut tree, ghost, f32::NAN),
            Err(CoreError::NoSuchSplit(ghost))
        );
    }

    #[test]
    fn maximizing_an_unknown_pane_is_refused() {
        let (mut tree, _) = asymmetric();
        let ghost = PaneId::new();
        assert_eq!(
            maximize(&mut tree, Some(ghost)),
            Err(CoreError::NoSuchPane(ghost))
        );
        assert_eq!(tree.maximized, None);
    }

    #[test]
    fn pane_index_tracks_depth_first_order_through_a_run_of_mutations() {
        let (mut tree, [p1, p2, p3, p4, p5]) = asymmetric();

        assert_eq!(pane_index(&tree, p1), Some(1));
        assert_eq!(pane_index(&tree, p5), Some(2));
        assert_eq!(pane_index(&tree, p2), Some(3));
        assert_eq!(pane_index(&tree, p3), Some(4));
        assert_eq!(pane_index(&tree, p4), Some(5));

        close(&mut tree, p5).unwrap();
        assert_eq!(pane_index(&tree, p1), Some(1));
        assert_eq!(pane_index(&tree, p2), Some(2));
        assert_eq!(pane_index(&tree, p5), None);

        let p6 = split_at(&mut tree, p1, Axis::Row, Side::Before);
        assert_eq!(pane_index(&tree, p6), Some(1));
        assert_eq!(pane_index(&tree, p1), Some(2));
        assert_eq!(pane_index(&tree, p4), Some(5));

        swap(&mut tree, p6, p4).unwrap();
        assert_eq!(pane_index(&tree, p4), Some(1));
        assert_eq!(pane_index(&tree, p6), Some(5));
        assert_eq!(pane_index(&tree, PaneId::new()), None);
        validate(&tree).unwrap();
    }

    #[test]
    fn validate_names_a_leaf_with_no_pane_entry() {
        let (mut tree, [p1, ..]) = asymmetric();
        tree.panes.shift_remove(&p1);
        let Err(CoreError::Invariant(msg)) = validate(&tree) else {
            panic!("a leaf without a pane entry must be reported");
        };
        assert!(msg.contains(&p1.to_string()));
    }

    #[test]
    fn validate_names_a_pane_entry_with_no_leaf() {
        let (mut tree, _) = asymmetric();
        let orphan = aux();
        let id = orphan.id;
        tree.panes.insert(id, orphan);
        let Err(CoreError::Invariant(msg)) = validate(&tree) else {
            panic!("an orphaned pane entry must be reported");
        };
        assert!(msg.contains(&id.to_string()));
    }

    #[test]
    fn validate_catches_the_same_pane_at_two_leaves() {
        let (mut tree, [_p1, _p2, _p3, p4, _p5]) = asymmetric();
        overwrite_first_leaf(&mut tree.root, p4);

        let Err(CoreError::Invariant(msg)) = validate(&tree) else {
            panic!("one pane at two leaves must be reported");
        };
        assert!(msg.contains(&p4.to_string()));
    }

    /// Point the first leaf at another pane, which is only ever a way to build a broken
    /// tree for the validator's benefit.
    fn overwrite_first_leaf(node: &mut LayoutNode, to: PaneId) {
        match node {
            LayoutNode::Leaf { pane } => *pane = to,
            LayoutNode::Split { a, .. } => overwrite_first_leaf(a, to),
        }
    }

    #[test]
    fn validate_catches_focus_on_a_dead_pane() {
        let (mut tree, _) = asymmetric();
        tree.focused = PaneId::new();
        let Err(CoreError::Invariant(msg)) = validate(&tree) else {
            panic!("focus must be a live leaf");
        };
        assert!(msg.contains("focused"));
    }

    #[test]
    fn validate_catches_maximize_on_a_dead_pane() {
        let (mut tree, _) = asymmetric();
        tree.maximized = Some(PaneId::new());
        let Err(CoreError::Invariant(msg)) = validate(&tree) else {
            panic!("a maximized pane must be a live leaf");
        };
        assert!(msg.contains("maximized"));
    }

    #[test]
    fn validate_catches_a_ratio_outside_the_band() {
        let (mut tree, _) = asymmetric();
        let LayoutNode::Split { ratio, .. } = &mut tree.root else {
            panic!("the fixture root is a split");
        };
        *ratio = 0.99;
        assert!(matches!(validate(&tree), Err(CoreError::Invariant(_))));

        let LayoutNode::Split { ratio, .. } = &mut tree.root else {
            panic!("the fixture root is a split");
        };
        *ratio = f32::INFINITY;
        assert!(matches!(validate(&tree), Err(CoreError::Invariant(_))));
    }

    #[test]
    fn validate_catches_a_duplicated_split_id() {
        let (mut tree, _) = asymmetric();
        let mut splits = Vec::new();
        collect_splits(&tree.root, &mut splits);
        let stolen = splits[1].0;
        let LayoutNode::Split { id, .. } = &mut tree.root else {
            panic!("the fixture root is a split");
        };
        *id = stolen;

        let Err(CoreError::Invariant(msg)) = validate(&tree) else {
            panic!("two dividers with one id must be reported");
        };
        assert!(msg.contains(&stolen.to_string()));
    }

    /// One step of the scripted sequence below.
    ///
    /// Panes and splits are named by one-based depth-first position, so the script reads
    /// as a table rather than as bookkeeping over generated ids.
    enum Step {
        /// Split the pane at this position; the new pane lands on the given side.
        Split(usize, Axis, Side),
        Close(usize),
        /// Ask for a ratio on the split at this position, before clamping.
        Ratio(usize, f32),
        Focus(usize),
        /// Move focus, or leave it alone at the edge of the tree.
        Nav(Direction),
        Maximize(Option<usize>),
        Swap(usize, usize),
        /// Add a tile beside the pane at this position, in its row.
        AddTile(usize, Side),
        /// Add a full-width row below or above the row holding this pane.
        AddRow(usize, Side),
    }

    #[test]
    fn a_long_run_of_operations_never_breaks_an_invariant() {
        use Axis::{Col, Row};
        use Direction::{Down, Left, Right, Up};
        use Side::{After, Before};
        use Step::*;

        let script = vec![
            Split(1, Row, After),
            Split(2, Col, After),
            Split(1, Col, Before),
            Ratio(1, 0.75),
            Ratio(2, 0.0),
            Focus(3),
            Nav(Right),
            Split(4, Row, After),
            Maximize(Some(2)),
            Nav(Up),
            Close(2),
            Split(1, Col, After),
            Swap(1, 4),
            Nav(Down),
            Ratio(3, 1.5),
            Focus(4),
            Close(4),
            Split(3, Row, Before),
            Maximize(None),
            Swap(2, 3),
            Nav(Left),
            Close(1),
            Close(1),
            Split(1, Col, After),
            Nav(Down),
            Close(2),
            Close(1),
            // The rows model, interleaved with the old gestures on purpose: the two build
            // the same kind of tree and must not be able to leave each other a broken one.
            AddTile(1, After),
            AddTile(2, After),
            AddRow(1, After),
            AddTile(3, Before),
            Ratio(1, 0.8),
            AddRow(2, Before),
            Nav(Down),
            Close(2),
            AddTile(1, After),
            Maximize(Some(1)),
            AddRow(1, After),
        ];

        let first = primary();
        let mut tree = new_tree(first);

        for (n, step) in script.into_iter().enumerate() {
            let ids = leaves(&tree.root);
            let at = |i: usize| ids[(i - 1) % ids.len()];
            match step {
                Split(i, axis, side) => {
                    split(&mut tree, at(i), axis, side, aux())
                        .unwrap_or_else(|e| panic!("step {n}: split failed: {e}"));
                }
                Close(i) => {
                    let target = at(i);
                    // Read before the call: a successful close takes the entry with it.
                    let was_primary = tree.panes[&target].role == PaneRole::Primary;
                    let outcome = close(&mut tree, target);
                    // The script deliberately closes down onto the primary pane, which must
                    // be refused however much company it has — that refusal is the whole
                    // reason the tab still has a conversation at the end.
                    if was_primary {
                        assert_eq!(outcome, Err(CoreError::PanePrimary), "step {n}");
                    } else {
                        outcome.unwrap_or_else(|e| panic!("step {n}: close failed: {e}"));
                    }
                }
                Ratio(s, value) => {
                    let mut splits = Vec::new();
                    collect_splits(&tree.root, &mut splits);
                    if !splits.is_empty() {
                        let id = splits[(s - 1) % splits.len()].0;
                        let stored = set_ratio(&mut tree, id, value)
                            .unwrap_or_else(|e| panic!("step {n}: set_ratio failed: {e}"));
                        assert!((MIN_RATIO..=MAX_RATIO).contains(&stored), "step {n}");
                    }
                }
                Focus(i) => focus(&mut tree, at(i))
                    .unwrap_or_else(|e| panic!("step {n}: focus failed: {e}")),
                Nav(dir) => {
                    if let Some(next) = navigate(&tree, tree.focused, dir) {
                        focus(&mut tree, next)
                            .unwrap_or_else(|e| panic!("step {n}: navigate led nowhere: {e}"));
                    }
                }
                Maximize(target) => maximize(&mut tree, target.map(at))
                    .unwrap_or_else(|e| panic!("step {n}: maximize failed: {e}")),
                Swap(x, y) => swap(&mut tree, at(x), at(y))
                    .unwrap_or_else(|e| panic!("step {n}: swap failed: {e}")),
                AddTile(i, side) => {
                    add_tile(&mut tree, at(i), side, aux())
                        .unwrap_or_else(|e| panic!("step {n}: add_tile failed: {e}"));
                }
                AddRow(i, side) => {
                    add_row(&mut tree, Some(at(i)), side, aux())
                        .unwrap_or_else(|e| panic!("step {n}: add_row failed: {e}"));
                }
            }
            validate(&tree).unwrap_or_else(|e| panic!("step {n} left the tree broken: {e}"));
        }

        // Whatever the script did, the tab still has its primary pane — which is now a
        // claim about the pane rather than merely about the tree being non-empty.
        assert!(!leaves(&tree.root).is_empty());
        validate_console(&tree).expect("the primary survived the whole script");
    }

    /// A tiny deterministic generator: the fuzz below needs a reproducible shuffle, not
    /// statistical quality, and the crate has no dev-dependency worth adding for it.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self, n: usize) -> usize {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
            ((self.0 >> 33) as usize) % n.max(1)
        }
    }

    #[test]
    fn no_sequence_of_legal_calls_breaks_an_invariant() {
        let dirs = [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ];
        for seed in 0..400u64 {
            let mut rng = Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let mut tree = new_tree(primary());
            // Panes taken out of the tree and waiting to be re-docked, which is the one way
            // a caller can hold a live pane that no tree owns.
            let mut detached: Vec<Pane> = Vec::new();

            for step in 0..80 {
                let ids = leaves(&tree.root);
                let pick = ids[rng.next(ids.len())];
                let axis = if rng.next(2) == 0 {
                    Axis::Row
                } else {
                    Axis::Col
                };
                let side = if rng.next(2) == 0 {
                    Side::Before
                } else {
                    Side::After
                };
                let where_ = format!("seed {seed} step {step}");

                match rng.next(11) {
                    0..=2 => {
                        split(&mut tree, pick, axis, side, aux())
                            .unwrap_or_else(|e| panic!("{where_}: split of a live leaf: {e}"));
                    }
                    // The two rows gestures. Both refuse an over-full chain, which is a legal
                    // answer rather than a failure — the same shape as `close` on a sole pane.
                    9 => match add_tile(&mut tree, pick, side, aux()) {
                        Ok(_) => {}
                        Err(CoreError::Invariant(msg)) if msg.contains("already holds") => {}
                        Err(e) => panic!("{where_}: add_tile: {e}"),
                    },
                    10 => match add_row(&mut tree, Some(pick), side, aux()) {
                        Ok(_) => {}
                        Err(CoreError::Invariant(msg)) if msg.contains("already holds") => {}
                        Err(e) => panic!("{where_}: add_row: {e}"),
                    },
                    // Refused on the sole pane, which is a legal answer rather than a failure.
                    3 => drop(close(&mut tree, pick)),
                    4 => {
                        if let Ok(taken) = take_pane(&mut tree, pick) {
                            detached.push(taken);
                        }
                    }
                    5 => {
                        if let Some(taken) = detached.pop() {
                            insert_pane(&mut tree, pick, axis, side, taken)
                                .unwrap_or_else(|e| panic!("{where_}: re-dock: {e}"));
                        }
                    }
                    6 => {
                        let mut splits = Vec::new();
                        collect_splits(&tree.root, &mut splits);
                        if !splits.is_empty() {
                            let id = splits[rng.next(splits.len())].0;
                            // Deliberately ranges well outside the band, so the clamp is
                            // exercised from both ends.
                            let asked = (rng.next(300) as f32) / 100.0 - 1.0;
                            let stored = set_ratio(&mut tree, id, asked)
                                .unwrap_or_else(|e| panic!("{where_}: set_ratio: {e}"));
                            assert!((MIN_RATIO..=MAX_RATIO).contains(&stored), "{where_}");
                        }
                    }
                    7 => {
                        let dir = dirs[rng.next(4)];
                        if let Some(next) = navigate(&tree, tree.focused, dir) {
                            assert_ne!(next, tree.focused, "{where_}: navigate stood still");
                            focus(&mut tree, next)
                                .unwrap_or_else(|e| panic!("{where_}: navigate went nowhere: {e}"));
                        }
                    }
                    _ => {
                        let other = ids[rng.next(ids.len())];
                        swap(&mut tree, pick, other)
                            .unwrap_or_else(|e| panic!("{where_}: swap of two live leaves: {e}"));
                        match rng.next(3) {
                            0 => maximize(&mut tree, Some(pick))
                                .unwrap_or_else(|e| panic!("{where_}: maximize: {e}")),
                            1 => maximize(&mut tree, None)
                                .unwrap_or_else(|e| panic!("{where_}: unmaximize: {e}")),
                            _ => {}
                        }
                    }
                }

                // `validate_console`, not `validate`: this tree was seeded with a primary, so
                // "a Primary leaf exists" is a live claim about it at every single step —
                // including after the close and take_pane arms, which is where it used to
                // stop being true.
                validate_console(&tree).unwrap_or_else(|e| panic!("seed {seed} step {step}: {e}"));
                let mut splits = Vec::new();
                collect_splits(&tree.root, &mut splits);
                assert_eq!(
                    splits.len() + 1,
                    leaves(&tree.root).len(),
                    "seed {seed} step {step}: n panes must leave n-1 splits"
                );
            }
        }
    }

    #[test]
    fn navigate_never_leaves_the_tree_or_returns_its_own_argument() {
        let dirs = [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ];
        for seed in 0..300u64 {
            let mut rng = Lcg(seed.wrapping_mul(0xD1B5_4A32_D192_ED03) | 1);
            let mut tree = new_tree(primary());
            for _ in 0..=rng.next(9) {
                let ids = leaves(&tree.root);
                let pick = ids[rng.next(ids.len())];
                let axis = if rng.next(2) == 0 {
                    Axis::Row
                } else {
                    Axis::Col
                };
                let side = if rng.next(2) == 0 {
                    Side::Before
                } else {
                    Side::After
                };
                split(&mut tree, pick, axis, side, aux()).expect("split of a live leaf");
            }

            let ids = leaves(&tree.root);
            for &from in &ids {
                for dir in dirs {
                    let Some(to) = navigate(&tree, from, dir) else {
                        continue;
                    };
                    assert!(ids.contains(&to), "seed {seed}: navigate left the tree");
                    assert_ne!(to, from, "seed {seed}: navigate stood still");
                }
            }
        }
    }

    #[test]
    fn validate_catches_a_pane_keyed_under_someone_elses_id() {
        // Reachable only from a hand-edited or migrated `workspace.json`, which is exactly
        // what `validate` is the gate for: the leaf addresses the pane by key while the
        // frontend addresses it by `Pane::id`, so a mismatch renders a pane that answers
        // `NoSuchPane` to every command aimed at it.
        let (mut tree, [p1, ..]) = asymmetric();
        let stray = PaneId::new();
        if let Some(pane) = tree.panes.get_mut(&p1) {
            pane.id = stray;
        }

        let Err(CoreError::Invariant(msg)) = validate(&tree) else {
            panic!("a pane whose key and id disagree must be reported");
        };
        assert!(msg.contains(&p1.to_string()) && msg.contains(&stray.to_string()));
    }

    /// Take a pane out and put it back by its anchor: the tree must be bit-for-bit what it
    /// was, divider ids and ratios included.
    ///
    /// Every pane of the five-pane fixture is tried rather than one hand-picked case,
    /// because the interesting variation is *what the sibling is*: `p1` leaves a whole
    /// subtree behind, `p4` leaves a leaf, and a restore that re-entered beside some leaf
    /// inside a subtree would pass the leaf cases and quietly nest the others one level deep.
    #[test]
    fn an_anchor_puts_a_pane_back_in_the_split_it_left() {
        let (fixture, ids) = asymmetric();
        // A ratio nobody would land on by accident: 0.5 is what a fresh split writes, so a
        // restore that forgot the ratio entirely would still match if the fixture kept it.
        let mut fixture = fixture;
        let splits = {
            let mut out = Vec::new();
            collect_splits(&fixture.root, &mut out);
            out
        };
        for (i, (id, _)) in splits.iter().enumerate() {
            set_ratio(&mut fixture, *id, 0.18 + 0.13 * i as f32).expect("a live divider moves");
        }

        for pane in ids {
            let mut tree = fixture.clone();
            let anchor = anchor_of(&tree, pane).expect("a pane below the root has a parent split");
            let taken = take_pane(&mut tree, pane).expect("a pane with a sibling can leave");

            assert!(
                can_restore(&tree, &anchor),
                "nothing changed while the pane was out, so its anchor is still good"
            );
            insert_pane_at(&mut tree, &anchor, taken).expect("restores");

            assert_eq!(
                tree.root, fixture.root,
                "pane {pane} came back to a different shape"
            );
            assert_eq!(
                tree.panes, fixture.panes,
                "pane {pane}: the side map lost or altered an entry"
            );
            // `IndexMap`'s `PartialEq` compares by key, not by position, so the assertion
            // above says nothing about order — and the order really does change: `take_pane`
            // shift-removes and `insert_pane_at` appends, so a restored pane comes back last
            // in `panes` whatever it was before. Pinned rather than left implicit, because
            // `panes` is serialised in this order — so every detach/re-dock rewrites that
            // part of `workspace.json` — and because `Object.values(tree.panes)` is how the
            // frontend picks "the first Claude pane of the console" for an @-mention.
            //
            // Not repaired here: repairing it means the anchor carrying the pane's index as
            // well, a wire field with its own stale-index case, for a property nothing yet
            // documents a dependency on. This assertion is what makes a later decision to
            // depend on it visible instead of silent.
            let restored_order: Vec<_> = tree.panes.keys().copied().collect();
            let mut expected: Vec<_> = fixture
                .panes
                .keys()
                .copied()
                .filter(|p| *p != pane)
                .collect();
            expected.push(pane);
            assert_eq!(
                restored_order, expected,
                "pane {pane}: `panes` order is the old order with the restored pane appended"
            );
            assert_eq!(tree.focused, pane, "the pane just dropped back takes focus");
            validate(&tree).expect("valid");
        }
    }

    /// The sibling closing while the pane is out is the case the old code assumed was the
    /// only one. It must be *detected*, not restored wrongly.
    #[test]
    fn an_anchor_whose_sibling_has_closed_cannot_be_restored() {
        let (mut tree, [p1, p2, p3, p4, _p5]) = asymmetric();
        let anchor = anchor_of(&tree, p4).expect("has a parent");
        let taken = take_pane(&mut tree, p4).expect("leaves");
        let DockSibling::Pane { pane: sibling } = anchor.sibling else {
            panic!("p4 split against a leaf");
        };
        assert_eq!(sibling, p3, "the fixture's shape is what this test assumes");

        close(&mut tree, sibling).expect("the sibling closes while the pane is out");

        assert!(!can_restore(&tree, &anchor));
        assert_eq!(
            insert_pane_at(&mut tree, &anchor, taken),
            Err(CoreError::NoSuchPane(sibling)),
            "the refusal names the node that went away, so a caller can say why it fell back"
        );
        // And the tree is untouched by the refusal — the caller still has a pane to place.
        assert!(!tree.panes.contains_key(&p4));
        assert!(tree.panes.contains_key(&p1) && tree.panes.contains_key(&p2));
        validate(&tree).expect("valid");
    }

    /// A subtree sibling can go away without any single pane doing so: close enough of it and
    /// the split that named it collapses.
    #[test]
    fn an_anchor_naming_a_split_goes_stale_when_that_split_collapses() {
        let (mut tree, [_p1, p2, p3, p4, _p5]) = asymmetric();
        let anchor = anchor_of(&tree, p2).expect("has a parent");
        let DockSibling::Split { .. } = anchor.sibling else {
            panic!("p2 split against a subtree, which is the case this test is about");
        };
        let taken = take_pane(&mut tree, p2).expect("leaves");
        assert!(can_restore(&tree, &anchor));

        // Collapsing the sibling subtree into a single leaf destroys the split the anchor
        // names, even though the pane that survived it is still there — which is exactly the
        // stale case a pane-id-only anchor could not see.
        close(&mut tree, p3).expect("closes");
        assert!(tree.panes.contains_key(&p4), "a pane of it survives");

        assert!(
            !can_restore(&tree, &anchor),
            "the subtree is gone even though its panes are not"
        );
        assert!(matches!(
            insert_pane_at(&mut tree, &anchor, taken),
            Err(CoreError::NoSuchSplit(_))
        ));
    }

    /// The root leaf has no parent split, so there is no position to record. `take_pane`
    /// refuses it anyway; the two must agree rather than one of them panicking.
    #[test]
    fn the_only_pane_in_a_tree_has_no_anchor() {
        let pane = primary();
        let id = pane.id;
        let tree = new_tree(pane);

        assert!(anchor_of(&tree, id).is_none());
        assert!(
            anchor_of(&tree, PaneId::new()).is_none(),
            "nor does a stranger"
        );
    }

    /// An anchor is persisted, so it can come back from a file that was hand-edited, written
    /// by a different build, or corrupted. A ratio outside the band would fail `validate` on
    /// the very next check — silently rejecting the whole workspace as corrupt over a
    /// cosmetic number — so it is clamped where it is read.
    #[test]
    fn a_restored_ratio_is_clamped_and_nan_guarded() {
        let (fixture, [p1, ..]) = asymmetric();

        for (stored, expected) in [(5.0_f32, MAX_RATIO), (-2.0, MIN_RATIO), (f32::NAN, 0.5)] {
            let mut tree = fixture.clone();
            let mut anchor = anchor_of(&tree, p1).expect("has a parent");
            anchor.ratio = stored;
            let taken = take_pane(&mut tree, p1).expect("leaves");

            insert_pane_at(&mut tree, &anchor, taken).expect("restores");

            let mut splits = Vec::new();
            collect_splits(&tree.root, &mut splits);
            let (_, ratio) = splits
                .iter()
                .find(|(id, _)| *id == anchor.split)
                .expect("the recorded divider is back");
            assert_eq!(*ratio, expected, "stored ratio {stored}");
            validate(&tree).expect("valid");
        }
    }

    /// Reinstating the recorded divider id is what keeps a drag in flight pointing at the
    /// same divider. It is also the one way this can break an invariant, so the impossible
    /// case is refused rather than trusted.
    #[test]
    fn restoring_a_divider_id_that_is_somehow_live_is_refused() {
        let (mut tree, [p1, ..]) = asymmetric();
        let mut anchor = anchor_of(&tree, p1).expect("has a parent");
        let taken = take_pane(&mut tree, p1).expect("leaves");

        let mut splits = Vec::new();
        collect_splits(&tree.root, &mut splits);
        anchor.split = splits.first().expect("the tree still has dividers").0;

        assert!(!can_restore(&tree, &anchor));
        assert!(matches!(
            insert_pane_at(&mut tree, &anchor, taken),
            Err(CoreError::Invariant(_))
        ));
        validate(&tree).expect("the refusal left the tree alone");
    }
}
