//! Synthetic groups: rows the file tree draws that no walk produced.
//!
//! *External Libraries* is the first of these and deliberately not the last — Scratches is the
//! next — so this is a **mechanism**, not a feature. A group is a header row plus a lazily
//! materialised subtree, and everything about it that could have been a special case in the
//! index is instead a second, much smaller arena sitting beside it.
//!
//! # Why this is not a synthetic root inside `Index`
//!
//! Grafting the dependency sources into [`crate::Index`] would have been fewer lines and three
//! separate disasters, each of which is a *structural* property of that type rather than a rule
//! somebody could remember to obey:
//!
//! | coupling in `Index` | what a grafted root would cost |
//! | --- | --- |
//! | `dir_paths()` is the watcher's watch list | an inotify watch on every directory in `~/.cargo/registry` — 2,223 crate directories, 2.9 GB, on this machine alone |
//! | the walk's sink is the picker's injector | 593 crates' worth of files in Ctrl+P |
//! | `Filter::build` stats a `.gitignore` per directory | thousands of extra syscalls over a tree with no `.gitignore` in it |
//! | `Index::empty` sets `show_roots = roots.len() > 1` | a single-root project would start drawing its own root row, moving every existing row's depth |
//!
//! `Index::path_of` also reconstructs every path from `self.roots[n.root]`, so a node outside
//! every root is not merely unwise there — it is unrepresentable. Keeping the two apart leaves
//! `seek`/`advance`/`row_index`, the most delicate arithmetic in this crate, completely
//! untouched, and makes "never watched, never searched, never gitignore-walked" true because
//! there is no code that could do any of those things rather than because nobody wrote it.
//!
//! # The interface a second group needs
//!
//! Everything below is generic over "a group". To add one:
//!
//! 1. [`Groups::show`] it, with an id and a label. It appears as a collapsed header row.
//! 2. When the user expands it, [`Groups::expand`] answers [`Expanded::Resolve`] if nobody has
//!    filled it in yet. Do the work — on a thread you own — and call [`Groups::fulfil`].
//! 3. That is all. Lazy `read_dir` of any [`Entry`] that names a directory, the row window, the
//!    depth arithmetic, `reveal` into a subtree that has never been expanded, and the frontend's
//!    gesture rules (`ui/src/sidebar/groupRows.ts`) are already written and are not per-group.
//!
//! A group that is *ready before it is expanded* — a directory listing, say — simply calls
//! [`Groups::fulfil`] at [`Groups::show`] time and never sees `Resolve`.
//!
//! # What this deliberately does not do
//!
//! No watching, no indexing, no ignore rules, no `Filter`, no incremental `apply`, no deletion
//! and no free list. A group's contents are re-read by collapsing and expanding it, which is the
//! gesture a user already makes, and by [`Groups::invalidate`] when the owner knows better.

use std::path::{Path, PathBuf};

use cide_ipc::{NO_ROOT, TreeMatch, TreeRow, TreeRowKind};

/// A row the owner of a group supplies.
///
/// Not a `TreeRow`: that carries a depth and an expansion state, which are facts about where the
/// row *sits*, and only this module knows those. This is the content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// What the row draws.
    pub name: String,
    /// Drawn dim after the name.
    pub detail: Option<String>,
    /// What the row stands for on disk. `None` makes it a [`TreeRowKind::Note`] — a sentence.
    pub path: Option<PathBuf>,
    /// A directory: it gets a twisty and is `read_dir`'d the first time it is expanded.
    pub dir: bool,
}

impl Entry {
    /// A row that is a sentence rather than a thing.
    pub fn note(text: impl Into<String>) -> Self {
        Self {
            name: text.into(),
            detail: None,
            path: None,
            dir: false,
        }
    }

    /// A row that opens a directory.
    pub fn directory(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            detail: None,
            path: Some(path.into()),
            dir: true,
        }
    }

    pub fn with_detail(mut self, detail: Option<String>) -> Self {
        self.detail = detail;
        self
    }
}

/// What a group's contents are, right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupState {
    /// Nobody has asked yet. Expanding asks.
    Unresolved,
    /// Somebody is working on it. The group shows whatever [`Groups::start`] put there.
    Resolving,
    /// [`Groups::fulfil`] has been called. Expanding again shows what is there.
    Ready,
}

/// What [`Groups::expand`] wants the caller to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expanded {
    /// The rows are there. Nothing to do but redraw.
    Ready,
    /// This group has no contents yet. Run the resolver and call [`Groups::fulfil`].
    ///
    /// Answered **once** per unresolved group: the state moves to [`GroupState::Resolving`]
    /// before this is returned, so two windows expanding the same group in the same frame start
    /// one resolution and not two.
    Resolve,
}

/// The `path` a group header carries.
///
/// A sentinel and not a real path, because a group header has no file behind it. It is
/// deliberately **not absolute**, which means [`crate::ops::check_within`] rejects it outright:
/// every file operation in `cmd::fs` therefore refuses it by the check it already performs,
/// rather than by a new special case somebody has to remember to add. An empty string was the
/// alternative and is worse — `rowFacts` in the frontend would have answered `{path: ''}` and
/// offered *Rename…* on it.
pub fn group_path(id: &str) -> PathBuf {
    PathBuf::from(format!("cide://group/{id}"))
}

/// The id inside a [`group_path`], or `None` for anything else.
pub fn group_id_of(path: &Path) -> Option<&str> {
    path.to_str()?.strip_prefix("cide://group/")
}

/// One node in a group's lazily materialised subtree.
#[derive(Debug)]
struct Node {
    name: String,
    detail: Option<String>,
    /// `None` for a note.
    path: Option<PathBuf>,
    dir: bool,
    expanded: bool,
    /// `None` until this directory has been read once. `Some(vec![])` is a directory that was
    /// read and is genuinely empty, which is why this is not just an empty `Vec` — the two need
    /// different twisties and a re-read on every draw would be a `read_dir` per frame.
    children: Option<Vec<Node>>,
}

impl Node {
    fn from_entry(entry: Entry) -> Self {
        Self {
            name: entry.name,
            detail: entry.detail,
            dir: entry.dir && entry.path.is_some(),
            path: entry.path,
            expanded: false,
            children: None,
        }
    }

    fn kind(&self) -> TreeRowKind {
        match (&self.path, self.dir) {
            (None, _) => TreeRowKind::Note,
            (Some(_), true) => TreeRowKind::Dir,
            (Some(_), false) => TreeRowKind::File,
        }
    }

    /// Rows this node contributes when its parent is expanded.
    ///
    /// Computed rather than cached, unlike `Index`'s `visible`. The trees here are tens of rows
    /// deep at most — a package's directory listing — and a cached count is a second thing to
    /// keep true across `read_dir`, `invalidate` and `reveal`.
    fn visible(&self) -> usize {
        if !self.expanded {
            return 1;
        }
        1 + self
            .children
            .iter()
            .flatten()
            .map(Node::visible)
            .sum::<usize>()
    }

    /// A directory with a twisty. An unread one is assumed to have children, because the
    /// alternative is a `read_dir` per row per frame; a directory that turns out to be empty
    /// loses its twisty the moment it has been opened once.
    fn has_children(&self) -> bool {
        match &self.children {
            Some(children) => !children.is_empty(),
            None => self.dir,
        }
    }
}

#[derive(Debug)]
struct Group {
    id: String,
    label: String,
    detail: Option<String>,
    state: GroupState,
    expanded: bool,
    children: Vec<Node>,
}

/// Every synthetic group of one project, in draw order.
#[derive(Debug, Default)]
pub struct Groups {
    groups: Vec<Group>,
}

impl Groups {
    pub fn new() -> Self {
        Self::default()
    }

    /// Draw a group with this id, or do nothing if it is already there.
    ///
    /// Idempotent because the probe that decides whether a group exists runs on the first tree
    /// read and may be raced by two windows; showing the same group twice would draw two headers
    /// over one set of rows.
    pub fn show(&mut self, id: &str, label: &str) {
        if self.groups.iter().any(|g| g.id == id) {
            return;
        }
        self.groups.push(Group {
            id: id.to_string(),
            label: label.to_string(),
            detail: None,
            state: GroupState::Unresolved,
            expanded: false,
            children: Vec::new(),
        });
    }

    pub fn state(&self, id: &str) -> Option<GroupState> {
        self.groups.iter().find(|g| g.id == id).map(|g| g.state)
    }

    /// Put a group into [`GroupState::Resolving`] with a placeholder row, without expanding it.
    ///
    /// For a resolution the owner starts on its own — a stale stamp noticed during a refresh —
    /// rather than in answer to a click. Returns false if the group is not shown.
    pub fn start(&mut self, id: &str, placeholder: Entry) -> bool {
        let Some(group) = self.groups.iter_mut().find(|g| g.id == id) else {
            return false;
        };
        group.state = GroupState::Resolving;
        group.children = vec![Node::from_entry(placeholder)];
        true
    }

    /// Install a group's rows. The group becomes [`GroupState::Ready`].
    ///
    /// Every expansion inside the group is discarded, deliberately: the rows are new, so a
    /// remembered "the user had `serde` open" would be a claim about a package that may not be
    /// in the new list at all. The group's own expansion state is *not* touched — the user
    /// opened it and it stays open, which is the whole point of resolving in the background.
    pub fn fulfil(&mut self, id: &str, rows: Vec<Entry>, detail: Option<String>) -> bool {
        let Some(group) = self.groups.iter_mut().find(|g| g.id == id) else {
            return false;
        };
        group.children = rows.into_iter().map(Node::from_entry).collect();
        group.detail = detail;
        group.state = GroupState::Ready;
        true
    }

    /// Forget a group's rows because the answer is stale — a `Cargo.lock` that was rewritten.
    ///
    /// The header stays; only the contents go. Whether the *stamp* moved is a question the owner
    /// answers, not this type.
    ///
    /// The return value is the reason this is not a plain `bool`. A group that is **currently
    /// expanded** has just been emptied on screen, which is precisely the silently-empty state
    /// this whole design exists to make unrepresentable — so it answers [`Expanded::Resolve`] and
    /// the caller must start the work and [`Groups::start`] a placeholder immediately, exactly as
    /// it would for a click. A collapsed group answers [`Expanded::Ready`]: nothing is on screen,
    /// and the next expand will ask.
    pub fn invalidate(&mut self, id: &str) -> Option<Expanded> {
        let group = self.groups.iter_mut().find(|g| g.id == id)?;
        group.children = Vec::new();
        if group.expanded {
            group.state = GroupState::Resolving;
            return Some(Expanded::Resolve);
        }
        group.state = GroupState::Unresolved;
        Some(Expanded::Ready)
    }

    /// Total rows: every header, plus everything expanded under one.
    pub fn count(&self) -> usize {
        self.groups
            .iter()
            .map(|group| {
                if group.expanded {
                    1 + group.children.iter().map(Node::visible).sum::<usize>()
                } else {
                    1
                }
            })
            .sum()
    }

    /// The rows in `[offset, offset + len)`, clamped to what exists.
    ///
    /// Depth 0 for a header, so a group sits at the same indentation as a top-level file. Out of
    /// range answers `[]` rather than an error, exactly as [`crate::Index::rows`] does and for
    /// the same reason — a virtualized list asks past the end as a matter of course.
    pub fn rows(&self, offset: usize, len: usize) -> Vec<TreeRow> {
        let mut out = Vec::new();
        if len == 0 {
            return out;
        }
        let mut at = 0usize;
        for group in &self.groups {
            if at >= offset.saturating_add(len) {
                break;
            }
            if at >= offset {
                out.push(TreeRow {
                    path: group_path(&group.id),
                    name: group.label.clone(),
                    depth: 0,
                    kind: TreeRowKind::Group,
                    expanded: group.expanded,
                    // Always, even before it has been resolved: a header with no twisty is a
                    // header nobody can open, and opening it is what starts the resolution.
                    has_children: true,
                    symlink: false,
                    root: NO_ROOT,
                    detail: group.detail.clone(),
                });
            }
            at += 1;
            if group.expanded {
                for node in &group.children {
                    emit(node, 1, offset, len, &mut at, &mut out);
                    if at >= offset.saturating_add(len) {
                        break;
                    }
                }
            }
        }
        out
    }

    /// Which visible rows' names match a speed-search query, in walk order.
    ///
    /// The second half of `cmd::fs`'s composed tree, and it exists for the same reason the
    /// composed `rows` does: a query that found `serde` inside *External Libraries* but only
    /// searched the walked index would answer nothing, and a query whose group matches were
    /// numbered from zero rather than from `Index::count()` would scroll the user to a file in
    /// their own project instead. The caller offsets these; see `compose_matches`.
    ///
    /// Row indices are counted with exactly the same walk [`Groups::rows`] emits with — header,
    /// then children when expanded, depth-first — because the two have to agree about what row
    /// 12 is or the highlight lands on a different line from the scroll.
    pub fn match_rows(
        &self,
        needle: &crate::speed::Needle,
        limit: usize,
    ) -> (Vec<TreeMatch>, bool) {
        let mut out = Vec::new();
        let mut at = 0u32;
        for group in &self.groups {
            if crate::speed::push_match(needle, &group.label, at, limit, &mut out) {
                return (out, true);
            }
            at += 1;
            if !group.expanded {
                continue;
            }
            for node in &group.children {
                if match_node(node, needle, limit, &mut at, &mut out) {
                    return (out, true);
                }
            }
        }
        (out, false)
    }

    /// Expand a group header or a directory inside one.
    ///
    /// `None` when this tree holds no such path, which is how the caller tells a group row from
    /// an index row without asking twice.
    pub fn expand(&mut self, path: &Path) -> Option<Expanded> {
        if let Some(id) = group_id_of(path) {
            let group = self.groups.iter_mut().find(|g| g.id == id)?;
            group.expanded = true;
            if group.state == GroupState::Unresolved {
                // Moved *before* the answer is returned, so two windows expanding the same
                // header in one frame start one resolution between them.
                group.state = GroupState::Resolving;
                return Some(Expanded::Resolve);
            }
            return Some(Expanded::Ready);
        }
        let node = self.node_mut(path)?;
        if !node.dir {
            return None;
        }
        node.expanded = true;
        if node.children.is_none() {
            let listing = read_dir(node.path.as_deref()?);
            let node = self.node_mut(path)?;
            node.children = Some(listing);
        }
        Some(Expanded::Ready)
    }

    /// Collapse a group header or a directory inside one. `None` when there is no such row.
    ///
    /// A collapsed directory keeps what it read. A collapsed *group* keeps its packages too —
    /// re-running `cargo metadata` because somebody folded a twisty would be a click that costs
    /// a quarter of a second for no new information; [`Groups::invalidate`] is how the owner
    /// says the answer is actually stale.
    pub fn collapse(&mut self, path: &Path) -> Option<()> {
        if let Some(id) = group_id_of(path) {
            let group = self.groups.iter_mut().find(|g| g.id == id)?;
            group.expanded = false;
            return Some(());
        }
        let node = self.node_mut(path)?;
        node.expanded = false;
        Some(())
    }

    /// Expand everything above `path` and answer the row it now sits on.
    ///
    /// **This materialises a chain that has never been expanded**, one `read_dir` per level,
    /// which is the property that makes *Select Opened File* work for a dependency source: the
    /// user has a tab open on `…/serde-1.0.229/src/de/mod.rs` and nothing under the group has
    /// ever been read. It does *not* resolve an unresolved group — there is nothing to descend
    /// from until the packages are known, and forcing a `cargo metadata` out of a reveal would
    /// put a quarter-second stall on a gesture that has to feel instant. An unresolved group
    /// answers `None`, and the caller says so.
    pub fn reveal(&mut self, path: &Path) -> Option<usize> {
        if let Some(id) = group_id_of(path) {
            return self.row_of_group(id);
        }
        // Which group owns it: the one holding a row whose directory is a prefix of the target.
        // Longest match, so a package unpacked inside another's directory resolves to the inner
        // one — the same rule `rowPaths::rootOf` applies to project roots, for the same reason.
        let (group_index, node_index) = self.owner_of(path)?;
        self.groups[group_index].expanded = true;
        let base = self.groups[group_index].children[node_index].path.clone()?;
        let rest = path.strip_prefix(&base).ok()?;

        // Every ancestor of the target, as its own absolute path. Addressing by path rather
        // than by a trail of child indices is what keeps this a plain loop: `node_mut` already
        // descends by prefix, and an index trail would go stale the instant a `read_dir`
        // inserted rows above it.
        let mut chain = vec![base.clone()];
        let mut cur = base;
        for component in rest.components() {
            cur = cur.join(component);
            chain.push(cur.clone());
        }

        // Top-down, reading each level as it is reached — one `read_dir` per level, and only
        // for the levels on the way to this file.
        for ancestor in &chain[..chain.len() - 1] {
            let node = self.node_mut(ancestor)?;
            if !node.dir {
                return None;
            }
            node.expanded = true;
            if node.children.is_none() {
                let dir = node.path.clone()?;
                let listing = read_dir(&dir);
                self.node_mut(ancestor)?.children = Some(listing);
            }
        }

        // The target itself is deliberately left folded: revealing a directory is a request to
        // *show* it, and unfolding it as well would move every row below it under a user who
        // only asked to be shown where something is.
        self.node(path)?;
        self.position_of(path)
    }

    /// What this tree holds at a path: a file, a directory, a group header, a note, or nothing.
    ///
    /// Only rows that have been *materialised* answer — an unexpanded package's contents are not
    /// here, because reading them to answer a hover would be the `read_dir`-per-candidate cost
    /// `Index::kind_of` exists to avoid.
    pub fn kind_of(&self, path: &Path) -> Option<TreeRowKind> {
        if let Some(id) = group_id_of(path) {
            return self
                .groups
                .iter()
                .any(|g| g.id == id)
                .then_some(TreeRowKind::Group);
        }
        self.node(path).map(Node::kind)
    }

    /// Whether any group is shown at all. `count() == 0` says the same thing and reads worse.
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    // --- addressing -------------------------------------------------------------------

    fn row_of_group(&self, id: &str) -> Option<usize> {
        let mut at = 0usize;
        for group in &self.groups {
            if group.id == id {
                return Some(at);
            }
            at += if group.expanded {
                1 + group.children.iter().map(Node::visible).sum::<usize>()
            } else {
                1
            };
        }
        None
    }

    /// Which row a materialised path currently sits on.
    ///
    /// A DFS over the *visible* rows rather than arithmetic over cached counts. The visible set
    /// here is a handful of directory listings — nothing like the 100k rows `Index::row_index`
    /// is written to avoid walking — and counting what is on screen cannot disagree with what
    /// [`Groups::rows`] draws, which a second cached number could.
    fn position_of(&self, path: &Path) -> Option<usize> {
        fn walk(list: &[Node], path: &Path, at: &mut usize) -> bool {
            for node in list {
                if node.path.as_deref() == Some(path) {
                    return true;
                }
                *at += 1;
                if node.expanded && walk(node.children.as_deref().unwrap_or(&[]), path, at) {
                    return true;
                }
            }
            false
        }
        let mut at = 0usize;
        for group in &self.groups {
            // The header's own row, whether or not anything is under it.
            at += 1;
            if !group.expanded {
                continue;
            }
            if walk(&group.children, path, &mut at) {
                return Some(at);
            }
        }
        None
    }

    fn owner_of(&self, path: &Path) -> Option<(usize, usize)> {
        let mut best: Option<(usize, usize, usize)> = None;
        for (g, group) in self.groups.iter().enumerate() {
            for (n, node) in group.children.iter().enumerate() {
                let Some(dir) = node.path.as_deref() else {
                    continue;
                };
                if !path.starts_with(dir) {
                    continue;
                }
                let len = dir.as_os_str().len();
                if best.is_none_or(|(_, _, best_len)| len > best_len) {
                    best = Some((g, n, len));
                }
            }
        }
        best.map(|(g, n, _)| (g, n))
    }

    fn node(&self, path: &Path) -> Option<&Node> {
        fn find<'a>(list: &'a [Node], path: &Path) -> Option<&'a Node> {
            for node in list {
                if node.path.as_deref() == Some(path) {
                    return Some(node);
                }
                if let Some(children) = &node.children
                    && node.path.as_deref().is_some_and(|p| path.starts_with(p))
                    && let Some(found) = find(children, path)
                {
                    return Some(found);
                }
            }
            None
        }
        self.groups.iter().find_map(|g| find(&g.children, path))
    }

    fn node_mut(&mut self, path: &Path) -> Option<&mut Node> {
        fn find<'a>(list: &'a mut [Node], path: &Path) -> Option<&'a mut Node> {
            for node in list {
                if node.path.as_deref() == Some(path) {
                    return Some(node);
                }
                let descend = node.path.as_deref().is_some_and(|p| path.starts_with(p));
                if descend
                    && let Some(children) = node.children.as_mut()
                    && let Some(found) = find(children, path)
                {
                    return Some(found);
                }
            }
            None
        }
        self.groups
            .iter_mut()
            .find_map(|g| find(&mut g.children, path))
    }
}

/// Append the rows of one node's subtree that fall inside the window.
/// One node of a group's subtree, counted and tested. `true` means stop.
///
/// Recursive in the same shape as [`emit`], and deliberately so: the two walks have to number
/// the rows identically, and the cheapest way to keep them in step is for them to look the same.
fn match_node(
    node: &Node,
    needle: &crate::speed::Needle,
    limit: usize,
    at: &mut u32,
    out: &mut Vec<TreeMatch>,
) -> bool {
    if crate::speed::push_match(needle, &node.name, *at, limit, out) {
        return true;
    }
    *at += 1;
    if !node.expanded {
        return false;
    }
    for child in node.children.iter().flatten() {
        if match_node(child, needle, limit, at, out) {
            return true;
        }
    }
    false
}

fn emit(
    node: &Node,
    depth: u16,
    offset: usize,
    len: usize,
    at: &mut usize,
    out: &mut Vec<TreeRow>,
) {
    if *at >= offset.saturating_add(len) {
        return;
    }
    if *at >= offset {
        out.push(TreeRow {
            path: node
                .path
                .clone()
                // A note has no path. It gets a unique, non-absolute one rather than an empty
                // string so React can key it and so a stray file operation is refused by
                // `check_within` rather than acting on `""`.
                .unwrap_or_else(|| PathBuf::from(format!("cide://note/{}", node.name))),
            name: node.name.clone(),
            depth,
            kind: node.kind(),
            expanded: node.expanded,
            has_children: node.has_children(),
            symlink: false,
            root: NO_ROOT,
            detail: node.detail.clone(),
        });
    }
    *at += 1;
    if !node.expanded {
        return;
    }
    for child in node.children.iter().flatten() {
        emit(child, depth + 1, offset, len, at, out);
        if *at >= offset.saturating_add(len) {
            return;
        }
    }
}

/// One directory listing, directories first and then case-insensitively by name — the same order
/// [`crate::Index`] sorts its own children in, so a dependency's `src/` looks like the project's.
///
/// No ignore rules. A `.gitignore` inside an unpacked crate describes *that crate's* build
/// outputs, and hiding rows by it would mean the file tree showing less of a dependency than
/// `ls` does, for a repository the user is not building.
///
/// An unreadable directory answers an empty listing rather than an error: the row is already on
/// screen, and the honest degradation is a twisty that opens onto nothing found.
fn read_dir(dir: &Path) -> Vec<Node> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        tracing::debug!(path = %dir.display(), "a library directory could not be read");
        return Vec::new();
    };
    let mut nodes: Vec<Node> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            // Non-UTF-8 names are dropped, exactly as the walk drops them: a row's path is the
            // identifier the frontend hands back, and `to_string_lossy` produces one that no
            // longer names the file.
            let name = path.file_name()?.to_str()?.to_string();
            path.to_str()?;
            let is_dir = entry.file_type().ok()?.is_dir();
            Some(Node {
                name,
                detail: None,
                dir: is_dir,
                path: Some(path),
                expanded: false,
                children: if is_dir { None } else { Some(Vec::new()) },
            })
        })
        .collect();
    nodes.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then_with(|| name_cmp(&a.name, &b.name))
            .then_with(|| a.name.cmp(&b.name))
    });
    nodes
}

/// Case-insensitive name order. The same rule `index::name_cmp` applies, restated because it is
/// private there and this module must sort identically or two halves of one list disagree.
fn name_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let mut lhs = a.chars().flat_map(char::to_lowercase);
    let mut rhs = b.chars().flat_map(char::to_lowercase);
    loop {
        match (lhs.next(), rhs.next()) {
            (Some(x), Some(y)) if x == y => continue,
            (Some(x), Some(y)) => return x.cmp(&y),
            (rest_a, rest_b) => return rest_a.is_some().cmp(&rest_b.is_some()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::scratch;

    const ID: &str = "externalLibraries";

    fn shown() -> Groups {
        let mut groups = Groups::new();
        groups.show(ID, "External Libraries");
        groups
    }

    fn names(groups: &Groups) -> Vec<String> {
        groups
            .rows(0, 100)
            .into_iter()
            .map(|row| row.name)
            .collect()
    }

    #[test]
    fn a_shown_group_is_one_collapsed_row_and_nothing_else() {
        let groups = shown();
        assert_eq!(groups.count(), 1);
        let rows = groups.rows(0, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, TreeRowKind::Group);
        assert_eq!(rows[0].depth, 0);
        assert!(!rows[0].expanded);
        assert!(
            rows[0].has_children,
            "a header with no twisty cannot be opened, and opening it is what resolves it"
        );
        assert_eq!(rows[0].root, NO_ROOT);
    }

    #[test]
    fn showing_the_same_group_twice_draws_one_header() {
        let mut groups = shown();
        groups.show(ID, "External Libraries");
        assert_eq!(groups.count(), 1);
    }

    /// The handover contract: the first expand asks for a resolution, and only the first.
    #[test]
    fn expanding_an_unresolved_group_asks_once() {
        let mut groups = shown();
        let path = group_path(ID);
        assert_eq!(groups.expand(&path), Some(Expanded::Resolve));
        assert_eq!(groups.state(ID), Some(GroupState::Resolving));
        groups.collapse(&path);
        assert_eq!(
            groups.expand(&path),
            Some(Expanded::Ready),
            "a second window expanding the same header must not start a second cargo"
        );
    }

    #[test]
    fn a_group_that_is_resolving_shows_the_placeholder_rather_than_nothing() {
        let mut groups = shown();
        groups.expand(&group_path(ID));
        groups.start(ID, Entry::note("Resolving dependencies…"));
        let rows = groups.rows(0, 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].kind, TreeRowKind::Note);
        assert_eq!(rows[1].name, "Resolving dependencies…");
        assert_eq!(rows[1].depth, 1);
        assert!(!rows[1].has_children, "a sentence has nothing to open");
    }

    #[test]
    fn fulfilling_installs_rows_and_a_count_and_leaves_the_group_open() {
        let dir = scratch("groups-fulfil");
        std::fs::create_dir_all(dir.join("serde-1.0.229/src")).unwrap();
        let mut groups = shown();
        groups.expand(&group_path(ID));
        groups.fulfil(
            ID,
            vec![
                Entry::directory("serde", dir.join("serde-1.0.229"))
                    .with_detail(Some("1.0.229".into())),
                Entry::note("not downloaded"),
            ],
            Some("2".into()),
        );
        assert_eq!(groups.state(ID), Some(GroupState::Ready));
        let rows = groups.rows(0, 10);
        assert!(
            rows[0].expanded,
            "the user opened it; resolving must not shut it"
        );
        assert_eq!(rows[0].detail.as_deref(), Some("2"));
        assert_eq!(rows[1].name, "serde");
        assert_eq!(rows[1].detail.as_deref(), Some("1.0.229"));
        assert_eq!(rows[1].kind, TreeRowKind::Dir);
        assert_eq!(rows[2].kind, TreeRowKind::Note);
    }

    #[test]
    fn expanding_a_package_reads_its_directory_once_and_sorts_it_like_the_tree() {
        let dir = scratch("groups-readdir");
        let pkg = dir.join("serde-1.0.229");
        std::fs::create_dir_all(pkg.join("src")).unwrap();
        std::fs::create_dir_all(pkg.join("Alpha")).unwrap();
        std::fs::write(pkg.join("Cargo.toml"), "").unwrap();
        std::fs::write(pkg.join("build.rs"), "").unwrap();

        let mut groups = shown();
        groups.expand(&group_path(ID));
        groups.fulfil(ID, vec![Entry::directory("serde", &pkg)], None);
        assert_eq!(groups.expand(&pkg), Some(Expanded::Ready));
        assert_eq!(
            names(&groups),
            [
                "External Libraries",
                "serde",
                // Directories first, then case-insensitively — `Alpha` before `src`, and
                // `build.rs` after `Cargo.toml`.
                "Alpha",
                "src",
                "build.rs",
                "Cargo.toml",
            ]
        );
        assert_eq!(groups.count(), 6);
    }

    #[test]
    fn a_file_row_inside_a_package_has_no_twisty() {
        let dir = scratch("groups-file");
        let pkg = dir.join("pkg");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("lib.rs"), "").unwrap();
        let mut groups = shown();
        groups.expand(&group_path(ID));
        groups.fulfil(ID, vec![Entry::directory("pkg", &pkg)], None);
        groups.expand(&pkg);
        let row = groups
            .rows(0, 10)
            .into_iter()
            .find(|r| r.name == "lib.rs")
            .expect("the file is a row");
        assert_eq!(row.kind, TreeRowKind::File);
        assert!(!row.has_children);
        assert_eq!(row.depth, 2);
    }

    /// The property *Select Opened File* needs: a chain nobody has ever expanded.
    #[test]
    fn reveal_materialises_a_chain_that_was_never_opened() {
        let dir = scratch("groups-reveal");
        let pkg = dir.join("serde-1.0.229");
        std::fs::create_dir_all(pkg.join("src/de")).unwrap();
        std::fs::write(pkg.join("src/de/mod.rs"), "").unwrap();
        std::fs::write(pkg.join("src/lib.rs"), "").unwrap();

        let mut groups = shown();
        groups.fulfil(ID, vec![Entry::directory("serde", &pkg)], None);
        let target = pkg.join("src/de/mod.rs");
        let at = groups.reveal(&target).expect("the file has a row");
        assert_eq!(
            names(&groups),
            [
                "External Libraries",
                "serde",
                "src",
                "de",
                "mod.rs",
                "lib.rs"
            ]
        );
        assert_eq!(at, 4);
        assert_eq!(groups.rows(at, 1)[0].path, target);
        assert!(
            groups.rows(0, 1)[0].expanded,
            "revealing into a collapsed group has to open it"
        );
    }

    #[test]
    fn reveal_does_not_unfold_the_directory_it_lands_on() {
        let dir = scratch("groups-reveal-dir");
        let pkg = dir.join("pkg");
        std::fs::create_dir_all(pkg.join("src/deep")).unwrap();
        std::fs::write(pkg.join("src/deep/a.rs"), "").unwrap();
        let mut groups = shown();
        groups.fulfil(ID, vec![Entry::directory("pkg", &pkg)], None);
        let at = groups.reveal(&pkg.join("src/deep")).expect("row");
        assert_eq!(names(&groups), ["External Libraries", "pkg", "src", "deep"]);
        assert!(!groups.rows(at, 1)[0].expanded);
    }

    #[test]
    fn reveal_answers_nothing_for_a_path_no_group_owns() {
        let mut groups = shown();
        groups.fulfil(ID, vec![Entry::note("No external dependencies.")], None);
        assert_eq!(groups.reveal(Path::new("/etc/shadow")), None);
        assert_eq!(
            groups.reveal(Path::new("/home/u/work/cide/src/lib.rs")),
            None
        );
    }

    #[test]
    fn reveal_finds_a_group_header_by_its_sentinel() {
        let mut groups = Groups::new();
        groups.show("a", "A");
        groups.show(ID, "External Libraries");
        assert_eq!(groups.reveal(&group_path(ID)), Some(1));
        assert_eq!(groups.reveal(&group_path("nope")), None);
    }

    /// Every window of every size, against the flat list — the same sweep `Index` gets, because
    /// the arithmetic here is a second implementation of the same idea and off-by-ones in it are
    /// exactly as invisible.
    #[test]
    fn windows_are_correct_at_the_boundaries_and_past_the_end() {
        let dir = scratch("groups-window");
        let pkg = dir.join("pkg");
        std::fs::create_dir_all(pkg.join("src")).unwrap();
        std::fs::write(pkg.join("src/lib.rs"), "").unwrap();
        std::fs::write(pkg.join("Cargo.toml"), "").unwrap();

        let mut groups = Groups::new();
        groups.show("first", "First");
        groups.show(ID, "External Libraries");
        groups.expand(&group_path(ID));
        groups.fulfil(
            ID,
            vec![Entry::directory("pkg", &pkg), Entry::note("and a note")],
            None,
        );
        groups.expand(&pkg);
        groups.expand(&pkg.join("src"));

        let all = names(&groups);
        assert_eq!(
            all,
            [
                "First",
                "External Libraries",
                "pkg",
                "src",
                "lib.rs",
                "Cargo.toml",
                "and a note",
            ]
        );
        assert_eq!(groups.count(), all.len());
        for offset in 0..all.len() {
            for len in 0..=all.len() {
                let window: Vec<String> = groups
                    .rows(offset, len)
                    .into_iter()
                    .map(|r| r.name)
                    .collect();
                let end = (offset + len).min(all.len());
                assert_eq!(window, all[offset..end], "offset {offset} len {len}");
            }
        }
        assert!(groups.rows(all.len(), 10).is_empty());
        assert!(groups.rows(9_999, 10).is_empty());
        assert!(groups.rows(0, 0).is_empty());
    }

    #[test]
    fn collapsing_a_group_keeps_its_packages_and_invalidating_drops_them() {
        let mut groups = shown();
        let path = group_path(ID);
        groups.expand(&path);
        groups.fulfil(ID, vec![Entry::note("one")], None);
        groups.collapse(&path);
        assert_eq!(groups.count(), 1);
        assert_eq!(
            groups.expand(&path),
            Some(Expanded::Ready),
            "folding a twisty must not cost a quarter of a second of cargo"
        );
        assert_eq!(groups.count(), 2);

        // Invalidating an *open* group empties it on screen, which is the silently-empty state
        // this design exists to make unrepresentable — so it demands a resolution then and there
        // rather than waiting for an expand that will never come.
        assert_eq!(groups.invalidate(ID), Some(Expanded::Resolve));
        assert_eq!(groups.state(ID), Some(GroupState::Resolving));
        assert_eq!(groups.count(), 1, "the header stays; its rows are gone");

        // A *collapsed* group has nothing on screen to be empty, so the next expand asks.
        groups.fulfil(ID, vec![Entry::note("one")], None);
        groups.collapse(&path);
        assert_eq!(groups.invalidate(ID), Some(Expanded::Ready));
        assert_eq!(groups.state(ID), Some(GroupState::Unresolved));
        assert_eq!(groups.expand(&path), Some(Expanded::Resolve));
        assert_eq!(groups.invalidate("nope"), None);
    }

    #[test]
    fn a_path_this_tree_does_not_hold_is_not_expandable_and_has_no_kind() {
        let mut groups = shown();
        groups.fulfil(ID, vec![Entry::note("nothing here")], None);
        assert_eq!(groups.expand(Path::new("/etc")), None);
        assert_eq!(groups.collapse(Path::new("/etc")), None);
        assert_eq!(groups.kind_of(Path::new("/etc")), None);
        assert_eq!(groups.kind_of(&group_path(ID)), Some(TreeRowKind::Group));
        assert_eq!(groups.kind_of(&group_path("other")), None);
    }

    /// The sentinel must be the shape `check_within` already refuses, or every file operation
    /// needs a new special case for it.
    #[test]
    fn a_group_header_path_is_refused_by_the_containment_check() {
        let path = group_path(ID);
        assert!(!path.is_absolute());
        assert!(
            crate::ops::check_within(&[PathBuf::from("/home/u/work/cide")], &path).is_err(),
            "a group sentinel must not pass containment"
        );
        assert_eq!(group_id_of(&path), Some(ID));
        assert_eq!(group_id_of(Path::new("/home/u/work/cide")), None);
    }

    #[test]
    fn a_directory_that_turns_out_to_be_empty_loses_its_twisty() {
        let dir = scratch("groups-empty");
        let pkg = dir.join("pkg");
        std::fs::create_dir_all(&pkg).unwrap();
        let mut groups = shown();
        groups.fulfil(ID, vec![Entry::directory("pkg", &pkg)], None);
        groups.expand(&group_path(ID));
        assert!(
            groups.rows(1, 1)[0].has_children,
            "an unread directory is assumed to have children — a read_dir per row per frame is \
             not affordable"
        );
        groups.expand(&pkg);
        assert!(!groups.rows(1, 1)[0].has_children);
    }
}
