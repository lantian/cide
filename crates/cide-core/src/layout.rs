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

/// Remove a leaf and collapse its parent split into the surviving sibling.
///
/// Refused for a [`PaneRole::Primary`] pane ([`CoreError::PanePrimary`]) and for the only
/// pane left in the tree ([`CoreError::LastPane`]): the pinned console always shows its
/// conversation, and a tab always has at least one pane. The two errors are distinct so the
/// caller can word the message.
pub fn close(tree: &mut PaneTree, pane: PaneId) -> Result<()> {
    take_pane(tree, pane).map(|_| ())
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

/// Move a divider, clamped to `[MIN_RATIO, MAX_RATIO]`.
///
/// Returns the value that was stored rather than `()`, so a caller dragging past the end
/// stop learns where the divider actually went instead of having to re-derive the clamp.
pub fn set_ratio(tree: &mut PaneTree, split: SplitId, ratio: f32) -> Result<f32> {
    // Existence first. A stale divider id is the caller's mistake and it says so plainly;
    // reporting `Invariant` because the value was also bad would send whoever is debugging
    // a dropped drag event looking at the wrong thing.
    let mut splits = Vec::new();
    collect_splits(&tree.root, &mut splits);
    if !splits.iter().any(|(id, _)| *id == split) {
        return Err(CoreError::NoSuchSplit(split));
    }

    // `f32::clamp` propagates NaN, so a non-finite ratio would sail through and poison the
    // layout for the rest of the session.
    if !ratio.is_finite() {
        return Err(CoreError::Invariant(format!(
            "ratio for split {split} is not finite"
        )));
    }
    let clamped = ratio.clamp(MIN_RATIO, MAX_RATIO);
    if !write_ratio(&mut tree.root, split, clamped) {
        return Err(CoreError::NoSuchSplit(split));
    }
    Ok(clamped)
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
/// being moved away from, then descends the other subtree taking the child nearest the
/// shared boundary.
///
/// Where that descent crosses a split on the *other* axis both children touch the boundary
/// equally, and with no pane rectangles to consult the first child is chosen. Making that
/// choice geometric — or last-focus based, as tmux does — needs measured layout the core
/// does not have; the rule here is at least stable, so repeated moves do not wander.
pub fn navigate(tree: &PaneTree, from: PaneId, dir: Direction) -> Option<PaneId> {
    let mut path = Vec::new();
    if !path_to(&tree.root, from, &mut path) {
        return None;
    }

    let axis = dir.axis();
    let forward = dir.is_forward();
    for (node, took_b) in path.iter().rev() {
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
        let across = if forward { b } else { a };
        return Some(boundary_leaf(across, axis, forward));
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

/// Store a ratio on the split with this id, reporting whether it was found.
///
/// Returns a bool rather than the `&mut f32` it found so that the borrow of `node` ends
/// with the call: [`set_ratio`] has nothing further to do with the slot, and handing one
/// out would let a future caller hold a live pointer into the tree across a mutation.
fn write_ratio(node: &mut LayoutNode, target: SplitId, value: f32) -> bool {
    let LayoutNode::Split {
        id, a, b, ratio, ..
    } = node
    else {
        return false;
    };
    if *id == target {
        *ratio = value;
        return true;
    }
    write_ratio(a, target, value) || write_ratio(b, target, value)
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

/// Descend to the leaf lying against the divider we have just crossed.
fn boundary_leaf(node: &LayoutNode, axis: Axis, forward: bool) -> PaneId {
    let mut cursor = node;
    loop {
        match cursor {
            LayoutNode::Leaf { pane } => return *pane,
            LayoutNode::Split {
                axis: split_axis,
                a,
                b,
                ..
            } => {
                // Along the axis of travel one child touches the divider we crossed;
                // across it both do, so take the first and stay predictable.
                cursor = if *split_axis == axis && !forward {
                    b
                } else {
                    a
                };
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
            title: "cide : claude".into(),
        }
    }

    fn aux() -> Pane {
        Pane {
            id: PaneId::new(),
            kind: PaneKind::Shell,
            role: PaneRole::Auxiliary,
            session: Some(SessionId::new()),
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
    fn navigate_across_a_perpendicular_split_takes_the_first_child() {
        // p5 is bottom left; the subtree to its right is split top/bottom, and with no pane
        // geometry the top child wins. Pinned here so a later geometry-aware rule is a
        // deliberate change rather than an accident.
        let (tree, [_p1, p2, _p3, _p4, p5]) = asymmetric();
        assert_eq!(navigate(&tree, p5, Direction::Right), Some(p2));
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

                match rng.next(9) {
                    0..=2 => {
                        split(&mut tree, pick, axis, side, aux())
                            .unwrap_or_else(|e| panic!("{where_}: split of a live leaf: {e}"));
                    }
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
