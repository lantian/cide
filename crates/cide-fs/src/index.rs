//! The multi-root file index: a parallel gitignore-aware walk and the flattened row model
//! the file tree renders from.
//!
//! # The row model
//!
//! The tree is stored as an arena of nodes, and every node caches `visible`: the number of
//! rows its subtree contributes when its parent is expanded. Expanding a directory updates
//! that number on the directory and on its ancestors — `O(depth)` — and nothing else. Row
//! `n` is then found by descending the tree, skipping any subtree whose `visible` count is
//! smaller than what is left of `n`.
//!
//! The obvious alternative is to keep a `Vec<NodeId>` of the visible rows and rebuild it on
//! every expand. That is `O(total nodes)` per click, which on a 100k-file project is a
//! visible stall on a keypress, and it makes `tree_rows` no cheaper than it is here.
//!
//! # Streaming
//!
//! [`build`] hands every batch of walked entries to a sink *while the walk is running*, so
//! the picker has candidates to match against long before the tree is ready. That is the
//! whole reason the walk is not simply `WalkBuilder::build().collect()`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use cide_ipc::{FsChange, TreeRow, TreeRowKind};
use ignore::{DirEntry, WalkBuilder, WalkState};

use crate::filter::Filter;

pub type NodeId = u32;

/// One directory tree cide has been asked to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub path: PathBuf,
    /// Shown as the top row when a project has more than one root.
    pub label: String,
}

impl Root {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let label = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        Self { path, label }
    }
}

/// One walked entry, as handed to the streaming sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkItem {
    pub path: PathBuf,
    /// Path relative to its root, prefixed with the root label in a multi-root project.
    /// This is what the picker matches and displays.
    pub rel: String,
    pub root: u16,
    pub is_dir: bool,
    pub symlink: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct BuildOptions {
    /// 0 lets `ignore` pick one thread per core.
    pub threads: usize,
    /// How many entries accumulate before a worker hands them to the sink. Small enough
    /// that the picker fills visibly during a long walk, large enough that the channel is
    /// not the bottleneck.
    pub batch: usize,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            threads: 0,
            batch: 512,
        }
    }
}

#[derive(Debug)]
struct Node {
    name: String,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    kind: TreeRowKind,
    symlink: bool,
    expanded: bool,
    /// Rows this node contributes when its parent is expanded: itself, plus its children's
    /// contribution when it is expanded. Maintained on every mutation.
    visible: u32,
    depth: u16,
    root: u16,
}

/// A project's file tree.
#[derive(Debug)]
pub struct Index {
    nodes: Vec<Node>,
    /// Slots freed by deletions, reused before the arena grows.
    free: Vec<NodeId>,
    by_path: HashMap<PathBuf, NodeId>,
    roots: Vec<Root>,
    /// The node for each root, in the same order as `roots`.
    root_nodes: Vec<NodeId>,
    /// With one root the root itself is not drawn — its children are the top-level rows,
    /// which is what every editor does and what the mock shows. With several, each root is
    /// a row so the user can tell them apart.
    show_roots: bool,
    files: u32,
    dirs: u32,
}

impl Index {
    /// An index with the roots recorded and nothing walked yet.
    ///
    /// The app registers this before starting the walk so that a picker query arriving in
    /// the first millisecond has something to answer from.
    pub fn empty(roots: Vec<Root>) -> Self {
        let mut index = Self {
            nodes: Vec::new(),
            free: Vec::new(),
            by_path: HashMap::new(),
            show_roots: roots.len() > 1,
            roots,
            root_nodes: Vec::new(),
            files: 0,
            dirs: 0,
        };
        let specs: Vec<(PathBuf, u16)> = index
            .roots
            .iter()
            .enumerate()
            .map(|(i, r)| (r.path.clone(), i as u16))
            .collect();
        for (path, i) in specs {
            let name = index.roots[i as usize].label.clone();
            let node = index.push_node(Node {
                name,
                parent: None,
                children: Vec::new(),
                kind: TreeRowKind::Dir,
                symlink: false,
                // A root is expanded from the start: with one root it is not even drawn, and
                // with several a collapsed-by-default project looks empty on open.
                expanded: true,
                visible: 1,
                depth: 0,
                root: i,
            });
            index.by_path.insert(path, node);
            index.root_nodes.push(node);
        }
        index
    }

    pub fn roots(&self) -> &[Root] {
        &self.roots
    }

    pub fn files(&self) -> u32 {
        self.files
    }

    pub fn dirs(&self) -> u32 {
        self.dirs
    }

    /// Every directory in the tree — the watcher's watch list.
    pub fn dir_paths(&self) -> Vec<PathBuf> {
        self.by_path
            .iter()
            .filter(|&(_, &id)| matches!(self.nodes[id as usize].kind, TreeRowKind::Dir))
            .map(|(p, _)| p.clone())
            .collect()
    }

    /// Total rows currently visible. The scrollbar's range.
    pub fn count(&self) -> usize {
        self.top()
            .iter()
            .map(|&n| self.nodes[n as usize].visible as usize)
            .sum()
    }

    /// The rows in `[offset, offset + len)`, clamped to what exists.
    ///
    /// Out-of-range offsets return an empty slice rather than an error: a virtualized list
    /// that shrinks under a scrolled viewport asks for rows past the end as a matter of
    /// course, and that is not a failure worth a dialog.
    pub fn rows(&self, offset: usize, len: usize) -> Vec<TreeRow> {
        let Some(mut stack) = self.seek(offset) else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(len.min(1024));
        while out.len() < len {
            let (container, i) = *stack.last().expect("seek leaves a non-empty stack");
            let node = self.list(container)[i];
            out.push(self.row(node));
            if !self.advance(&mut stack, node) {
                break;
            }
        }
        out
    }

    /// Expand a directory. Returns the new total row count.
    pub fn expand(&mut self, path: &Path) -> Option<usize> {
        self.set_expanded(path, true)
    }

    /// Collapse a directory. Returns the new total row count.
    pub fn collapse(&mut self, path: &Path) -> Option<usize> {
        self.set_expanded(path, false)
    }

    /// Expand everything above `path` and return the row it now occupies.
    ///
    /// `None` when the path is not in the index — ignored, deleted, or outside every root.
    /// The caller shows nothing rather than scrolling somewhere arbitrary.
    pub fn reveal(&mut self, path: &Path) -> Option<usize> {
        let node = *self.by_path.get(path)?;
        let mut chain = Vec::new();
        let mut cur = self.nodes[node as usize].parent;
        while let Some(p) = cur {
            chain.push(p);
            cur = self.nodes[p as usize].parent;
        }
        for id in chain.into_iter().rev() {
            if !self.nodes[id as usize].expanded {
                self.nodes[id as usize].expanded = true;
                self.recompute_up(id);
            }
        }
        self.row_index(node)
    }

    /// Whether a path is in the index at all.
    pub fn contains(&self, path: &Path) -> bool {
        self.by_path.contains_key(path)
    }

    // --- construction ---------------------------------------------------------------

    /// Walk the roots and build the tree, streaming batches of entries to `sink` as they
    /// are found.
    ///
    /// The sink runs on the walker's threads, so it must be cheap and it must not take a
    /// lock the caller holds. Injecting into a lock-free `nucleo` queue is exactly the shape
    /// it is meant for.
    pub fn build(
        roots: Vec<Root>,
        opts: BuildOptions,
        sink: &(dyn Fn(&[WalkItem]) + Sync),
    ) -> Self {
        let mut index = Self::empty(roots);
        let multi = index.roots.len() > 1;
        let mut entries: Vec<WalkItem> = Vec::new();

        for (i, root) in index.roots.clone().iter().enumerate() {
            let start = entries.len();
            walk_root(root, i as u16, multi, opts, |batch| {
                sink(batch);
                entries.extend_from_slice(batch);
            });
            tracing::debug!(
                root = %root.path.display(),
                entries = entries.len() - start,
                "walked a project root"
            );
        }

        index.graft_entries(entries);
        index
    }

    /// Insert walked entries into the arena.
    ///
    /// Sorting by path is what makes a second pass unnecessary: a parent path is a
    /// component-wise prefix of its children, so it always sorts before them and its node
    /// id is in `by_path` by the time they are reached.
    fn graft_entries(&mut self, mut entries: Vec<WalkItem>) -> Vec<WalkItem> {
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        entries.dedup_by(|a, b| a.path == b.path);

        let mut inserted = Vec::with_capacity(entries.len());
        let mut touched: HashSet<NodeId> = HashSet::new();
        for item in entries {
            if self.by_path.contains_key(&item.path) {
                continue;
            }
            let Some(parent_path) = item.path.parent() else {
                continue;
            };
            let Some(&parent) = self.by_path.get(parent_path) else {
                // The root entries themselves land here (their parent is outside the tree),
                // and so would an entry whose parent was ignored. Both are already handled:
                // roots exist from `empty`, and `ignore` never descends into an ignored
                // directory.
                continue;
            };
            let name = match item.path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name.to_string(),
                None => continue,
            };
            let node = self.new_child(parent, name, &item);
            self.by_path.insert(item.path.clone(), node);
            touched.insert(parent);
            inserted.push(item);
        }

        for parent in touched {
            self.sort_children(parent);
        }
        self.recompute_all();
        inserted
    }

    fn new_child(&mut self, parent: NodeId, name: String, item: &WalkItem) -> NodeId {
        let depth = self.nodes[parent as usize].depth + 1;
        let kind = if item.is_dir {
            self.dirs += 1;
            TreeRowKind::Dir
        } else {
            self.files += 1;
            TreeRowKind::File
        };
        let node = self.push_node(Node {
            name,
            parent: Some(parent),
            children: Vec::new(),
            kind,
            symlink: item.symlink,
            expanded: false,
            visible: 1,
            depth,
            root: item.root,
        });
        self.nodes[parent as usize].children.push(node);
        node
    }

    fn push_node(&mut self, node: Node) -> NodeId {
        match self.free.pop() {
            Some(id) => {
                self.nodes[id as usize] = node;
                id
            }
            None => {
                self.nodes.push(node);
                (self.nodes.len() - 1) as NodeId
            }
        }
    }

    /// Directories first, then case-insensitively by name. Matches every file tree the user
    /// has ever used, and the parallel walk hands entries over in no order at all.
    fn sort_children(&mut self, parent: NodeId) {
        let mut children = std::mem::take(&mut self.nodes[parent as usize].children);
        children.sort_by(|&a, &b| {
            let (a, b) = (&self.nodes[a as usize], &self.nodes[b as usize]);
            let dir_first =
                matches!(b.kind, TreeRowKind::Dir).cmp(&matches!(a.kind, TreeRowKind::Dir));
            dir_first
                .then_with(|| name_cmp(&a.name, &b.name))
                .then_with(|| a.name.cmp(&b.name))
        });
        self.nodes[parent as usize].children = children;
    }

    /// Recompute every `visible` count bottom-up.
    ///
    /// Used after a bulk build, where walking up from each inserted node would be
    /// quadratic on a deep tree.
    fn recompute_all(&mut self) {
        let roots = self.root_nodes.clone();
        for root in roots {
            self.recompute_subtree(root);
        }
    }

    fn recompute_subtree(&mut self, node: NodeId) -> u32 {
        let children = self.nodes[node as usize].children.clone();
        let mut sum = 0;
        for child in children {
            sum += self.recompute_subtree(child);
        }
        let n = &mut self.nodes[node as usize];
        n.visible = if n.expanded { 1 + sum } else { 1 };
        n.visible
    }

    fn recompute_up(&mut self, from: NodeId) {
        let mut cur = Some(from);
        while let Some(id) = cur {
            let sum: u32 = self.nodes[id as usize]
                .children
                .iter()
                .map(|&c| self.nodes[c as usize].visible)
                .sum();
            let n = &mut self.nodes[id as usize];
            n.visible = if n.expanded { 1 + sum } else { 1 };
            cur = n.parent;
        }
    }

    // --- incremental update ---------------------------------------------------------

    /// Bring the tree back in line with the disk after a watcher notification.
    ///
    /// Re-reads only the directories the change names — the parent of every changed path,
    /// plus any changed path that is itself a known directory. A new subdirectory is walked
    /// in full, because a `git clone` into the tree arrives as one create event and a
    /// thousand new files.
    ///
    /// Returns the entries that appeared, so the caller can hand them to the picker, which
    /// would otherwise know only the one path the event named. An empty result also means
    /// "nothing to repaint", which is the common case for a write to an existing file.
    ///
    /// Deletions are deliberately not reported for the picker: `nucleo` has no way to
    /// withdraw an injected item, so a stale row survives until the next full index.
    /// Offering a path that has gone costs one failed open; re-injecting 100k items on every
    /// `rm` costs the property this crate exists for.
    pub fn apply(&mut self, change: &FsChange, filter: &Filter) -> Vec<WalkItem> {
        // A `Vec` for the order, a set for the membership test: a burst can carry thousands
        // of paths that share a handful of directories, and a linear `contains` over that is
        // quadratic in the size of the burst.
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        for path in &change.paths {
            // A `.git` write is reported to the UI but changes no row: the walk never put
            // anything under `.git` in the tree.
            if filter.is_git_path(path) {
                continue;
            }
            let dir = match self.by_path.get(path) {
                Some(&id) if matches!(self.nodes[id as usize].kind, TreeRowKind::Dir) => {
                    path.clone()
                }
                _ => match path.parent() {
                    Some(p) => p.to_path_buf(),
                    None => continue,
                },
            };
            if seen.insert(dir.clone()) {
                dirs.push(dir);
            }
        }
        let mut added = Vec::new();
        for dir in dirs {
            added.extend(self.rescan_dir(&dir, filter));
        }
        added
    }

    /// Re-read one directory and reconcile its children.
    fn rescan_dir(&mut self, dir: &Path, filter: &Filter) -> Vec<WalkItem> {
        let Some(&node) = self.by_path.get(dir) else {
            return Vec::new();
        };
        if !matches!(self.nodes[node as usize].kind, TreeRowKind::Dir) {
            return Vec::new();
        }

        let read = match std::fs::read_dir(dir) {
            Ok(read) => read,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // The directory itself is gone. Detaching it here rather than waiting for
                // the parent's rescan keeps the tree right even when the parent was not part
                // of this change.
                self.remove_node(node);
                return Vec::new();
            }
            Err(err) => {
                tracing::debug!(%err, path = %dir.display(), "could not re-read a directory");
                return Vec::new();
            }
        };

        let mut on_disk: HashMap<String, (PathBuf, bool, bool)> = HashMap::new();
        for entry in read.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let meta = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            let is_dir = meta.is_dir();
            if !filter.admits(&path, is_dir) {
                continue;
            }
            on_disk.insert(name.to_string(), (path, is_dir, meta.is_symlink()));
        }

        let existing: Vec<(NodeId, String)> = self.nodes[node as usize]
            .children
            .iter()
            .map(|&c| (c, self.nodes[c as usize].name.clone()))
            .collect();

        let mut changed = false;
        let mut added = Vec::new();
        for (child, name) in &existing {
            if !on_disk.contains_key(name) {
                changed |= self.remove_node(*child);
            }
        }
        let known: HashSet<String> = self.nodes[node as usize]
            .children
            .iter()
            .map(|&c| self.nodes[c as usize].name.clone())
            .collect();
        let root = self.nodes[node as usize].root;
        for (name, (path, is_dir, symlink)) in on_disk {
            if known.contains(&name) {
                continue;
            }
            let item = WalkItem {
                rel: self.rel_of(&path, root),
                path: path.clone(),
                root,
                is_dir,
                symlink,
            };
            let child = self.new_child(node, name, &item);
            self.by_path.insert(path.clone(), child);
            added.push(item);
            changed = true;
            if is_dir && !symlink {
                added.extend(self.graft_subtree(&path, root, filter));
            }
        }
        if changed {
            self.sort_children(node);
            self.recompute_up(node);
        }
        added
    }

    /// A path as the picker shows it: relative to its root, prefixed with the root label
    /// when the project has more than one.
    fn rel_of(&self, path: &Path, root_index: u16) -> String {
        let root = &self.roots[root_index as usize];
        let rel = path
            .strip_prefix(&root.path)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        if self.show_roots {
            format!("{}/{rel}", root.label)
        } else {
            rel
        }
    }

    /// Walk a directory that has just appeared and insert everything under it.
    ///
    /// The walk is rooted at the new directory, so `WalkBuilder` sees only the `.gitignore`
    /// files at or below it — the project's own `.gitignore` is above the walk root and
    /// `parents(false)` keeps it out. A `git clone` of a Rust project into the tree would
    /// therefore index `target/` if the rule that ignores it lives in the project root. So
    /// every entry is put through `Filter`, which does consult the ancestor matchers, before
    /// it is grafted. Without this the index and the watcher disagree: `watch_tree` already
    /// asks `Filter`, so those rows would appear in the tree and never be watched.
    fn graft_subtree(&mut self, path: &Path, root_index: u16, filter: &Filter) -> Vec<WalkItem> {
        let root = Root::new(path);
        let mut entries = Vec::new();
        walk_root(&root, root_index, false, BuildOptions::default(), |batch| {
            entries.extend_from_slice(batch);
        });
        entries.retain(|item| filter.admits(&item.path, item.is_dir));
        // The walk made `rel` relative to the new directory; the picker wants it relative to
        // the project root.
        for item in &mut entries {
            item.rel = self.rel_of(&item.path, root_index);
        }
        // The directory node itself already exists; `graft_entries` skips known paths.
        self.graft_entries(entries)
    }

    /// Drop a node and everything under it.
    fn remove_node(&mut self, node: NodeId) -> bool {
        let Some(parent) = self.nodes[node as usize].parent else {
            // A root: it can vanish from disk, but the row stays so the user can see that
            // the project still points there. Removing it would silently drop a root from a
            // multi-root project on a transient unmount.
            return false;
        };
        self.nodes[parent as usize].children.retain(|&c| c != node);

        // Paths are computed before any slot is freed: `path_of` walks parent links, and a
        // freed slot is overwritten by the next insert.
        let mut stack = vec![node];
        let mut doomed = Vec::new();
        while let Some(id) = stack.pop() {
            let n = &self.nodes[id as usize];
            match n.kind {
                TreeRowKind::Dir => self.dirs = self.dirs.saturating_sub(1),
                TreeRowKind::File => self.files = self.files.saturating_sub(1),
            }
            stack.extend_from_slice(&n.children);
            doomed.push(id);
        }
        for id in doomed {
            let path = self.path_of(id);
            self.by_path.remove(&path);
            self.free.push(id);
        }
        self.recompute_up(parent);
        true
    }

    // --- row addressing -------------------------------------------------------------

    /// The rows at the top of the list: the roots, or the single root's children.
    fn top(&self) -> &[NodeId] {
        if self.show_roots {
            &self.root_nodes
        } else {
            match self.root_nodes.first() {
                Some(&root) => &self.nodes[root as usize].children,
                None => &[],
            }
        }
    }

    fn list(&self, container: Option<NodeId>) -> &[NodeId] {
        match container {
            None => self.top(),
            Some(id) => &self.nodes[id as usize].children,
        }
    }

    /// The DFS stack pointing at row `index`, or `None` when the list is shorter.
    fn seek(&self, index: usize) -> Option<Vec<(Option<NodeId>, usize)>> {
        let mut remaining = index;
        let mut container: Option<NodeId> = None;
        let mut stack = Vec::new();
        loop {
            let list = self.list(container);
            let mut found = None;
            for (i, &node) in list.iter().enumerate() {
                let visible = self.nodes[node as usize].visible as usize;
                if remaining < visible {
                    stack.push((container, i));
                    found = Some(node);
                    break;
                }
                remaining -= visible;
            }
            let node = found?;
            if remaining == 0 {
                return Some(stack);
            }
            // The row is inside this node's subtree; descend past the node's own row.
            remaining -= 1;
            container = Some(node);
        }
    }

    /// Move the cursor to the next visible row. False when there is none.
    fn advance(&self, stack: &mut Vec<(Option<NodeId>, usize)>, node: NodeId) -> bool {
        let n = &self.nodes[node as usize];
        if n.expanded && !n.children.is_empty() {
            stack.push((Some(node), 0));
            return true;
        }
        loop {
            let Some((container, i)) = stack.last_mut() else {
                return false;
            };
            *i += 1;
            let container = *container;
            let i = *i;
            if i < self.list(container).len() {
                return true;
            }
            stack.pop();
        }
    }

    /// Which list a node appears in: its parent, or the top list.
    fn container_of(&self, node: NodeId) -> Option<Option<NodeId>> {
        let parent = self.nodes[node as usize].parent;
        match parent {
            None => (self.show_roots).then_some(None),
            Some(p) if !self.show_roots && self.root_nodes.first() == Some(&p) => Some(None),
            Some(p) => Some(Some(p)),
        }
    }

    fn row_index(&self, node: NodeId) -> Option<usize> {
        let mut index = 0usize;
        let mut cur = node;
        loop {
            let container = self.container_of(cur)?;
            let list = self.list(container);
            let pos = list.iter().position(|&n| n == cur)?;
            for &sibling in &list[..pos] {
                index += self.nodes[sibling as usize].visible as usize;
            }
            match container {
                None => return Some(index),
                Some(parent) => {
                    // The parent's own row sits above everything in its subtree.
                    index += 1;
                    cur = parent;
                }
            }
        }
    }

    fn set_expanded(&mut self, path: &Path, expanded: bool) -> Option<usize> {
        let node = *self.by_path.get(path)?;
        if !matches!(self.nodes[node as usize].kind, TreeRowKind::Dir) {
            return None;
        }
        if self.nodes[node as usize].expanded != expanded {
            self.nodes[node as usize].expanded = expanded;
            self.recompute_up(node);
        }
        Some(self.count())
    }

    fn row(&self, node: NodeId) -> TreeRow {
        let n = &self.nodes[node as usize];
        TreeRow {
            path: self.path_of(node),
            name: n.name.clone(),
            // With one root its children are the top rows, so their depth-1 becomes 0.
            depth: if self.show_roots {
                n.depth
            } else {
                n.depth.saturating_sub(1)
            },
            kind: n.kind,
            expanded: n.expanded,
            has_children: !n.children.is_empty(),
            symlink: n.symlink,
            root: n.root,
        }
    }

    /// Rebuild a node's path from its ancestors.
    ///
    /// Names rather than a stored `PathBuf` per node: a 100k-entry index would spend several
    /// megabytes on paths that are only ever needed for the ~200 rows on screen, and this is
    /// `O(depth)` on a path nobody keeps.
    fn path_of(&self, node: NodeId) -> PathBuf {
        let mut names = Vec::new();
        let mut cur = Some(node);
        while let Some(id) = cur {
            let n = &self.nodes[id as usize];
            match n.parent {
                Some(p) => {
                    names.push(n.name.as_str());
                    cur = Some(p);
                }
                None => {
                    let mut path = self.roots[n.root as usize].path.clone();
                    for name in names.iter().rev() {
                        path.push(name);
                    }
                    return path;
                }
            }
        }
        PathBuf::new()
    }
}

/// Compare names the way a file tree does: case-insensitively, so `Cargo.toml` and `build`
/// sort next to each other rather than in two ASCII blocks.
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

/// The `ignore` walk of one root, batching entries to `on_batch` as they are found.
fn walk_root(
    root: &Root,
    root_index: u16,
    multi: bool,
    opts: BuildOptions,
    mut on_batch: impl FnMut(&[WalkItem]),
) {
    let mut builder = WalkBuilder::new(&root.path);
    builder
        // `hidden` and the git rules are `WalkBuilder`'s defaults; they are spelled out
        // because `Filter` reimplements exactly this set for the watcher and the two have to
        // be read together.
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        // See `Filter`: ignore files above the project root are not consulted, so that the
        // watcher can make the same decision without an unbounded ancestor walk.
        .parents(false)
        // Honour `.gitignore` even when the root is not a git repository. The default is to
        // ignore it outside a repo, which surprises anyone who keeps a project in a plain
        // directory.
        .require_git(false)
        // A symlinked directory is shown as a link and not descended into: following links
        // is how a walk finds a cycle, and how one project's tree ends up containing another.
        .follow_links(false)
        .threads(opts.threads);

    let (tx, rx) = mpsc::channel::<Vec<WalkItem>>();
    let root_path = root.path.clone();
    let label = root.label.clone();
    let batch = opts.batch;

    std::thread::scope(|scope| {
        let walker = builder.build_parallel();
        scope.spawn(move || {
            let mut visitors = Collector {
                tx,
                root: root_path,
                label,
                root_index,
                multi,
                batch,
            };
            walker.visit(&mut visitors);
        });
        for batch in rx {
            on_batch(&batch);
        }
    });
}

struct Collector {
    tx: mpsc::Sender<Vec<WalkItem>>,
    root: PathBuf,
    label: String,
    root_index: u16,
    multi: bool,
    batch: usize,
}

impl<'s> ignore::ParallelVisitorBuilder<'s> for Collector {
    fn build(&mut self) -> Box<dyn ignore::ParallelVisitor + 's> {
        Box::new(Visitor {
            tx: self.tx.clone(),
            root: self.root.clone(),
            label: self.label.clone(),
            root_index: self.root_index,
            multi: self.multi,
            batch: self.batch,
            buf: Vec::with_capacity(self.batch),
        })
    }
}

struct Visitor {
    tx: mpsc::Sender<Vec<WalkItem>>,
    root: PathBuf,
    label: String,
    root_index: u16,
    multi: bool,
    batch: usize,
    buf: Vec<WalkItem>,
}

impl Visitor {
    fn flush(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let batch = std::mem::replace(&mut self.buf, Vec::with_capacity(self.batch));
        // A closed receiver means the walk is being abandoned; there is nothing useful to
        // do about it here and the walker will wind down on its own.
        let _ = self.tx.send(batch);
    }

    fn item(&self, entry: &DirEntry) -> Option<WalkItem> {
        let path = entry.path();
        // Non-UTF-8 names are dropped rather than lossily converted. A row's path is the
        // identifier the frontend hands back to `fs.expand`, and `to_string_lossy` produces
        // a path that no longer names the file.
        let rel = path.strip_prefix(&self.root).ok()?.to_str()?;
        path.to_str()?;
        if rel.is_empty() {
            // The root entry itself; its node exists before the walk starts.
            return None;
        }
        let rel = if self.multi {
            format!("{}/{rel}", self.label)
        } else {
            rel.to_string()
        };
        let file_type = entry.file_type()?;
        Some(WalkItem {
            path: path.to_path_buf(),
            rel,
            root: self.root_index,
            is_dir: file_type.is_dir(),
            symlink: file_type.is_symlink(),
        })
    }
}

impl ignore::ParallelVisitor for Visitor {
    fn visit(&mut self, entry: Result<DirEntry, ignore::Error>) -> WalkState {
        match entry {
            Ok(entry) => {
                if let Some(item) = self.item(&entry) {
                    self.buf.push(item);
                    if self.buf.len() >= self.batch {
                        self.flush();
                    }
                }
                WalkState::Continue
            }
            Err(err) => {
                // A permission error on one directory is not a reason to abandon the walk;
                // an unreadable `node_modules` should cost that subtree and nothing else.
                tracing::debug!(%err, "skipping an unreadable entry");
                WalkState::Continue
            }
        }
    }
}

impl Drop for Visitor {
    fn drop(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::scratch;

    fn tree(dir: &Path) {
        std::fs::create_dir_all(dir.join("src/deep")).unwrap();
        std::fs::create_dir_all(dir.join("target/debug")).unwrap();
        std::fs::write(dir.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();
        std::fs::write(dir.join("src/main.rs"), "").unwrap();
        std::fs::write(dir.join("src/lib.rs"), "").unwrap();
        std::fs::write(dir.join("src/deep/mod.rs"), "").unwrap();
        std::fs::write(dir.join("target/debug/binary"), "").unwrap();
    }

    fn build(dir: &Path) -> Index {
        Index::build(
            vec![Root::new(dir)],
            BuildOptions::default(),
            &|_: &[WalkItem]| {},
        )
    }

    #[test]
    fn a_gitignored_path_never_appears() {
        let dir = scratch("index-ignore");
        tree(&dir);
        let index = build(&dir);
        assert!(!index.contains(&dir.join("target")));
        assert!(!index.contains(&dir.join("target/debug/binary")));
        assert!(index.contains(&dir.join("src/main.rs")));
    }

    #[test]
    fn a_single_root_shows_its_children_at_depth_zero() {
        let dir = scratch("index-rows");
        tree(&dir);
        let index = build(&dir);
        // src (dir, collapsed) then Cargo.toml — directories first.
        let rows = index.rows(0, 10);
        assert_eq!(rows.len(), 2, "{rows:#?}");
        assert_eq!(rows[0].name, "src");
        assert_eq!(rows[0].depth, 0);
        assert!(rows[0].has_children);
        assert_eq!(rows[1].name, "Cargo.toml");
        assert_eq!(index.count(), 2);
    }

    #[test]
    fn expanding_inserts_children_and_collapsing_removes_them() {
        let dir = scratch("index-expand");
        tree(&dir);
        let mut index = build(&dir);
        assert_eq!(index.expand(&dir.join("src")), Some(5));
        let rows = index.rows(0, 10);
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["src", "deep", "lib.rs", "main.rs", "Cargo.toml"]
        );
        assert_eq!(rows[1].depth, 1);
        assert_eq!(index.collapse(&dir.join("src")), Some(2));
        assert_eq!(index.count(), 2);
    }

    #[test]
    fn windows_are_correct_at_the_boundaries_and_past_the_end() {
        let dir = scratch("index-window");
        tree(&dir);
        let mut index = build(&dir);
        index.expand(&dir.join("src")).unwrap();
        index.expand(&dir.join("src/deep")).unwrap();
        let all: Vec<String> = index.rows(0, 100).into_iter().map(|r| r.name).collect();
        assert_eq!(
            all,
            ["src", "deep", "mod.rs", "lib.rs", "main.rs", "Cargo.toml"]
        );

        for offset in 0..all.len() {
            for len in 0..=all.len() {
                let window: Vec<String> = index
                    .rows(offset, len)
                    .into_iter()
                    .map(|r| r.name)
                    .collect();
                let end = (offset + len).min(all.len());
                assert_eq!(window, all[offset..end], "offset {offset} len {len}");
            }
        }
        assert!(index.rows(all.len(), 10).is_empty());
        assert!(index.rows(9_999, 10).is_empty());
        assert!(index.rows(0, 0).is_empty());
    }

    #[test]
    fn reveal_expands_the_ancestors_and_returns_the_row() {
        let dir = scratch("index-reveal");
        tree(&dir);
        let mut index = build(&dir);
        let row = index.reveal(&dir.join("src/deep/mod.rs")).unwrap();
        assert_eq!(row, 2);
        assert_eq!(index.rows(row, 1)[0].name, "mod.rs");
        assert_eq!(index.reveal(&dir.join("nope.rs")), None);
    }

    #[test]
    fn multi_root_puts_each_root_on_a_row_of_its_own() {
        let dir = scratch("index-multi");
        std::fs::create_dir_all(dir.join("alpha/src")).unwrap();
        std::fs::create_dir_all(dir.join("beta")).unwrap();
        std::fs::write(dir.join("alpha/src/a.rs"), "").unwrap();
        std::fs::write(dir.join("beta/b.rs"), "").unwrap();

        let mut index = Index::build(
            vec![Root::new(dir.join("alpha")), Root::new(dir.join("beta"))],
            BuildOptions::default(),
            &|_: &[WalkItem]| {},
        );
        let rows = index.rows(0, 10);
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["alpha", "src", "beta", "b.rs"]
        );
        assert_eq!(rows[0].depth, 0);
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[2].root, 1);
        assert_eq!(index.reveal(&dir.join("beta/b.rs")), Some(3));
    }

    #[test]
    fn the_walk_streams_before_it_finishes() {
        let dir = scratch("index-stream");
        for i in 0..2_000 {
            std::fs::write(dir.join(format!("f{i}.txt")), "").unwrap();
        }
        let batches = std::sync::Mutex::new(Vec::new());
        let index = Index::build(vec![Root::new(dir.path())], BuildOptions::default(), &|b| {
            batches.lock().unwrap().push(b.len());
        });
        let batches = batches.into_inner().unwrap();
        assert!(batches.len() > 1, "one batch is not streaming: {batches:?}");
        assert_eq!(batches.iter().sum::<usize>(), 2_000);
        assert_eq!(index.files(), 2_000);
    }

    /// A `git clone` into the tree arrives as one create event and a whole subtree, and that
    /// subtree is walked from its own directory — where the project's `.gitignore` is an
    /// ancestor the walk is told not to read. `Filter` is what puts it back.
    #[test]
    fn a_directory_that_appears_later_still_obeys_the_project_gitignore() {
        let dir = scratch("index-graft-ignore");
        tree(&dir);
        let mut index = build(&dir);
        let filter = Filter::build(
            &[dir.to_path_buf()],
            index.dir_paths().iter().map(|p| p.as_path()),
        );

        // `target/` is ignored by the root `.gitignore`, which is above this walk's root.
        std::fs::create_dir_all(dir.join("vendored/target/debug")).unwrap();
        std::fs::create_dir_all(dir.join("vendored/src")).unwrap();
        std::fs::write(dir.join("vendored/target/debug/a.o"), "").unwrap();
        std::fs::write(dir.join("vendored/src/lib.rs"), "").unwrap();

        let added = index.apply(
            &FsChange {
                paths: vec![dir.join("vendored")],
                truncated: false,
                git: false,
            },
            &filter,
        );

        assert!(index.contains(&dir.join("vendored/src/lib.rs")));
        assert!(
            !index.contains(&dir.join("vendored/target")),
            "the project's own .gitignore has to reach a subtree grafted after the walk"
        );
        assert!(!index.contains(&dir.join("vendored/target/debug/a.o")));
        let rels: Vec<&str> = added.iter().map(|i| i.rel.as_str()).collect();
        assert!(
            !rels.iter().any(|r| r.contains("target")),
            "an ignored path was offered to the picker: {rels:?}"
        );
    }

    #[test]
    fn names_sort_case_insensitively() {
        assert_eq!(name_cmp("apple", "Banana"), std::cmp::Ordering::Less);
        assert_eq!(name_cmp("Zebra", "apple"), std::cmp::Ordering::Greater);
        assert_eq!(name_cmp("same", "same"), std::cmp::Ordering::Equal);
        assert_eq!(name_cmp("ab", "abc"), std::cmp::Ordering::Less);
    }
}
