//! Explicit directory reads. At most 512 entries per page, 16 MiB per project.
use super::*;
use std::collections::BTreeMap;

const PAGE_SIZE: usize = 512;
const CACHE_LIMIT: u64 = 16 * 1024 * 1024;
type Key = (bool, String, String);

fn key(name: &str, dir: bool) -> Key {
    (!dir, name.to_lowercase(), name.to_owned())
}

impl Index {
    pub fn browse_cache_bytes(&self) -> u64 {
        self.browse_bytes
    }

    /// Previous (-1), first (0), or next (1) page. Never returns search candidates.
    pub fn browse_page(&mut self, path: &Path, direction: i8) -> Option<usize> {
        let id = *self.by_path.get(path)?;
        if !self.nodes[id as usize].browse || self.nodes[id as usize].kind != TreeRowKind::Dir {
            return None;
        }
        self.load_browse_page(id, direction, None);
        self.set_expanded(path, true)
    }

    fn clear_browse_children(&mut self, id: NodeId) {
        let children = self.nodes[id as usize].children.clone();
        for child in children {
            self.remove_node(child);
        }
        self.nodes[id as usize].page = None;
    }

    pub(super) fn load_browse_page(&mut self, id: NodeId, direction: i8, reveal: Option<&Path>) {
        let path = self.path_of(id);
        // Collapse normally releases these immediately. Also cover listings opened by a
        // second window/API while an ancestor is collapsed, before refusing visible rows.
        let inactive: Vec<_> = self
            .by_path
            .iter()
            .filter_map(|(p, &node)| {
                if path.starts_with(p) || self.nodes[node as usize].page.is_none() {
                    return None;
                }
                let mut parent = self.nodes[node as usize].parent;
                while let Some(ancestor) = parent {
                    if !self.nodes[ancestor as usize].expanded {
                        return Some(p.clone());
                    }
                    parent = self.nodes[ancestor as usize].parent;
                }
                None
            })
            .collect();
        for p in inactive {
            if let Some(&node) = self.by_path.get(&p) {
                self.clear_browse_children(node);
            }
        }
        let root = self.nodes[id as usize].root;
        let boundary = &self.roots[root as usize].path;
        if directory_cycle(&path, boundary) {
            self.clear_browse_children(id);
            self.nodes[id as usize].page = Some(cide_ipc::TreeBrowsePage {
                previous: false,
                next: false,
                error: Some("Symbolic link cycle; this folder cannot be expanded".into()),
            });
            return;
        }
        let ancestors = path.ancestors().take_while(|p| p.starts_with(boundary));
        let filter = Filter::build(std::slice::from_ref(boundary), ancestors, self.visibility);
        let anchor = if let Some(target) = reveal {
            target
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| key(n, target.is_dir()))
        } else {
            let children = &self.nodes[id as usize].children;
            let edge = if direction < 0 {
                children.first()
            } else if direction > 0 {
                children.last()
            } else {
                None
            };
            edge.map(|&child| {
                let n = &self.nodes[child as usize];
                key(&n.name, n.kind == TreeRowKind::Dir)
            })
        };
        // Selection scans a directory without ever collecting it. Previous pages retain the
        // largest 512 keys below the cursor; next pages retain the smallest 512 above it.
        let read = std::fs::read_dir(&path);
        let mut selected = BTreeMap::new();
        let mut previous = false;
        let mut next = false;
        let mut error = None;
        match read {
            Err(e) => error = Some(format!("Cannot read folder: {e}")),
            Ok(read) => {
                for entry in read {
                    let entry = match entry {
                        Ok(e) => e,
                        Err(e) => {
                            error = Some(format!("Cannot read entry: {e}"));
                            continue;
                        }
                    };
                    let child = entry.path();
                    let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                        continue;
                    };
                    let ty = match entry.file_type() {
                        Ok(t) => t,
                        Err(e) => {
                            error = Some(format!("Cannot read {name}: {e}"));
                            continue;
                        }
                    };
                    let is_dir = ty.is_dir() || (ty.is_symlink() && child.is_dir());
                    if !filter.browsable(&child, is_dir) {
                        continue;
                    }
                    let k = key(&name, is_dir);
                    if let Some(bound) = &anchor {
                        if direction < 0 && k >= *bound {
                            next = true;
                            continue;
                        }
                        if direction >= 0 && (k < *bound || (k == *bound && reveal.is_none())) {
                            previous = true;
                            continue;
                        }
                    }
                    selected.insert(
                        k,
                        WalkItem {
                            rel: self.rel_of(&child, root),
                            path: child,
                            root,
                            is_dir,
                            symlink: ty.is_symlink(),
                        },
                    );
                    if selected.len() > PAGE_SIZE {
                        if direction < 0 {
                            selected.pop_first();
                            previous = true;
                        } else {
                            selected.pop_last();
                            next = true;
                        }
                    }
                }
            }
        }
        self.clear_browse_children(id);
        for ((_, _, name), item) in selected {
            // Includes arena storage, hash entries, logical paths, names and page metadata.
            let bytes = 1024 + 4 * (item.path.as_os_str().len() + name.len()) as u64;
            if self.browse_bytes + bytes > CACHE_LIMIT {
                error = Some("Explorer browsing cache reached 16 MiB. Collapse another excluded folder, then refresh this folder.".into());
                next = true;
                break;
            }
            let child = self.new_child(id, name, &item);
            self.nodes[child as usize].cache_bytes = bytes;
            self.browse_bytes += bytes;
            self.by_path.insert(item.path, child);
        }
        self.nodes[id as usize].page = Some(cide_ipc::TreeBrowsePage {
            previous,
            next,
            error,
        });
        self.recompute_up(id);
    }

    pub(super) fn release_browse_below(&mut self, path: &Path) {
        // Drop outermost listings first. Descendant ids may have been freed by that drop.
        let mut dirs: Vec<_> = self
            .by_path
            .iter()
            .filter(|(p, id)| p.starts_with(path) && self.nodes[**id as usize].browse)
            .map(|(p, _)| p.clone())
            .collect();
        dirs.sort_by_key(|p| p.components().count());
        for p in dirs {
            if let Some(&id) = self.by_path.get(&p) {
                self.clear_browse_children(id);
                self.nodes[id as usize].expanded = false;
                self.recompute_up(id);
            }
        }
    }

    pub(super) fn reveal_browse(&mut self, path: &Path) {
        let Some(root) = self
            .roots
            .iter()
            .filter(|r| path.starts_with(&r.path))
            .max_by_key(|r| r.path.components().count())
        else {
            return;
        };
        let mut chain: Vec<_> = path
            .ancestors()
            .take_while(|p| p.starts_with(&root.path))
            .map(Path::to_path_buf)
            .collect();
        chain.reverse();
        for pair in chain.windows(2) {
            let Some(&id) = self.by_path.get(&pair[0]) else {
                return;
            };
            if self.nodes[id as usize].browse && !self.by_path.contains_key(&pair[1]) {
                self.load_browse_page(id, 0, Some(&pair[1]));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_browsing_cache_reports_its_limit_without_spending_index_memory() {
        let dir = crate::testing::scratch("browse-cache-limit");
        std::fs::create_dir(dir.join("excluded")).unwrap();
        std::fs::write(dir.join("excluded/file"), "").unwrap();
        let policy = Arc::new(
            crate::Exclusions::compile(vec![dir.path().into()], &["excluded".into()]).unwrap(),
        );
        let mut index = Index::build(
            vec![Root::new(dir.path())],
            BuildOptions {
                exclusions: policy,
                ..Default::default()
            },
            &|_| {},
        );
        let charged = index.budget.used();
        index.browse_bytes = CACHE_LIMIT;
        index.expand(&dir.join("excluded"));
        assert_eq!(index.browse_bytes, CACHE_LIMIT);
        assert_eq!(index.budget.used(), charged);
        assert!(
            index.rows(0, 10)[0]
                .browse_page
                .as_ref()
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .contains("16 MiB")
        );
        assert!(!index.contains(&dir.join("excluded/file")));
        index.browse_bytes = 0;
        index.browse_page(&dir.join("excluded"), 0);
        assert!(index.contains(&dir.join("excluded/file")));
    }
}
