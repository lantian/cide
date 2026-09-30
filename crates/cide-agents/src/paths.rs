//! Which tasks may run side by side: the overlap of what they declare they will change. (M132)
//!
//! # Why this exists
//!
//! A worktree per task keeps two runs from writing one *file on disk*, but not from writing one
//! *file in history*: each branch looks clean in its own checkout, and the second merge conflicts.
//! In this design a conflict is not a local `git` chore — the merge refuses, the task goes back to
//! its role, the role rebases, verify runs again and a reviewer reads it again, so one conflict
//! costs about a whole second task. On selfcraft eight sprite tasks started together, all on
//! `tools/spritegen/*`, and nearly every one of them came back.
//!
//! So a task declares what it will change (`Task::touches`), and admission does not start a run
//! whose declaration overlaps what another live or unmerged task holds — its declaration and the
//! files its branch has actually changed. This module is the overlap rule, pure.
//!
//! # The rule, and why it is conservative
//!
//! A declaration is a list of paths relative to the project root: a file (`game/view/map.gd`), a
//! directory (`tools/spritegen/`, or the same without the slash), or a glob
//! (`art/**/*.png`). An empty declaration is *not declared*, which reads as the whole repository:
//! a task that says nothing about what it touches runs alone. That is the safe default for a
//! board written before `touches` existed, and the reason to write one.
//!
//! Two globs are compared by their literal prefixes — everything before the first `*`, `?`, `[`
//! or `{` — and overlap when one prefix is a path-prefix of the other. That says `art/**/*.png`
//! and `art/**/*.json` overlap, which a precise intersection would not: wrongly waiting costs a
//! few minutes of one run, wrongly starting costs a conflict. A concrete changed file is matched
//! against the other side's globs exactly.

use globset::Glob;

/// Whether `path` is a glob, rather than a literal file or directory.
fn is_glob(path: &str) -> bool {
    path.contains(['*', '?', '[', '{'])
}

/// A declaration entry as it is compared: no `./`, no trailing `/`, and `*`/`**` as "everything".
fn normal(entry: &str) -> &str {
    entry.trim().trim_start_matches("./").trim_end_matches('/')
}

/// The part of `entry` before its first glob character.
fn literal_prefix(entry: &str) -> &str {
    let entry = normal(entry);
    match entry.find(['*', '?', '[', '{']) {
        Some(at) => &entry[..at],
        None => entry,
    }
}

/// Whether `prefix` is `path` or a directory above it. The empty prefix is above everything.
fn is_path_prefix(prefix: &str, path: &str) -> bool {
    let prefix = prefix.trim_end_matches('/');
    prefix.is_empty()
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/') || prefix.ends_with('/'))
}

/// Two declaration entries overlap. See the module header for why this is by prefix.
fn entries_overlap(a: &str, b: &str) -> bool {
    let (pa, pb) = (literal_prefix(a), literal_prefix(b));
    // A literal prefix that stops mid-name (`art/ic*`) is compared as a string prefix, which is
    // what it is; a full component compares at a path boundary.
    let loose = |prefix: &str, other: &str, glob: bool| {
        if glob && !prefix.is_empty() && !prefix.ends_with('/') {
            other.starts_with(prefix)
        } else {
            is_path_prefix(prefix, other)
        }
    };
    loose(pa, pb, is_glob(a)) || loose(pb, pa, is_glob(b))
}

/// Whether a changed `file` falls under one declaration entry.
fn entry_matches(entry: &str, file: &str) -> bool {
    let entry_n = normal(entry);
    if entry_n.is_empty() || entry_n == "*" || entry_n == "**" {
        return true;
    }
    if !is_glob(entry_n) {
        return is_path_prefix(entry_n, file);
    }
    match Glob::new(entry_n) {
        Ok(glob) => glob.compile_matcher().is_match(file),
        // A glob that will not compile is compared by its prefix — conservative again.
        Err(_) => is_path_prefix(literal_prefix(entry_n), file),
    }
}

/// What another task holds: its declaration and the files its branch has actually changed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Held {
    /// Its `touches`. Empty is the whole repository.
    pub touches: Vec<String>,
    /// Files its branch or checkout has changed, relative to the project root.
    pub files: Vec<String>,
}

/// Where a declaration `touches` meets what another task holds, as a short phrase for a queue
/// note, or `None` when they are apart.
///
/// Empty `touches` — undeclared — meets anything; an undeclared hold meets anything too, since
/// nobody can say what it will change.
#[must_use]
pub fn conflict(touches: &[String], held: &Held) -> Option<String> {
    if touches.is_empty() {
        return Some("this task declares no `touches`, so it runs alone".to_string());
    }
    if held.touches.is_empty() {
        return Some("it declares no `touches`, so nothing runs beside it".to_string());
    }
    for mine in touches {
        if let Some(theirs) = held
            .touches
            .iter()
            .find(|theirs| entries_overlap(mine, theirs))
        {
            return Some(normal(theirs).to_string())
                .map(|t| if t.is_empty() { "*".into() } else { t });
        }
        if let Some(file) = held.files.iter().find(|file| entry_matches(mine, file)) {
            return Some(file.clone());
        }
    }
    None
}

/// The files of `changed` that fall outside `touches` — what a run edited beyond its declaration.
#[must_use]
pub fn outside(touches: &[String], changed: &[String]) -> Vec<String> {
    if touches.is_empty() {
        return Vec::new();
    }
    changed
        .iter()
        .filter(|file| !touches.iter().any(|entry| entry_matches(entry, file)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(touches: &[&str], files: &[&str]) -> Held {
        Held {
            touches: touches.iter().map(|s| s.to_string()).collect(),
            files: files.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_directory_and_a_file_inside_it_overlap_and_siblings_do_not() {
        let hold = held(&["tools/spritegen/"], &[]);
        assert!(conflict(&v(&["tools/spritegen/render.py"]), &hold).is_some());
        assert!(conflict(&v(&["tools/spritegen"]), &hold).is_some());
        assert!(conflict(&v(&["tools/spritegen-old/x.py"]), &hold).is_none());
        assert!(conflict(&v(&["tools/other/"]), &hold).is_none());
    }

    #[test]
    fn globs_compare_by_their_literal_prefix_and_err_towards_waiting() {
        assert!(conflict(&v(&["art/**/*.png"]), &held(&["art/**/*.json"], &[])).is_some());
        assert!(conflict(&v(&["art/mobs/*.png"]), &held(&["art/tiles/"], &[])).is_none());
        assert!(conflict(&v(&["art/ic*"]), &held(&["art/icons/sword.png"], &[])).is_some());
    }

    #[test]
    fn a_changed_file_is_matched_exactly_against_the_declaration() {
        let hold = held(&["game/view/terrain.gd"], &["game/view/map_view.gd"]);
        assert_eq!(
            conflict(&v(&["game/view/map_view.gd"]), &hold).as_deref(),
            Some("game/view/map_view.gd")
        );
        assert!(conflict(&v(&["game/ui/*.gd"]), &hold).is_none());
        assert!(conflict(&v(&["game/**/*.gd"]), &hold).is_some());
    }

    #[test]
    fn undeclared_on_either_side_is_the_whole_repository() {
        assert!(conflict(&[], &held(&["a/"], &[])).is_some());
        assert!(conflict(&v(&["a/"]), &held(&[], &[])).is_some());
        assert!(conflict(&v(&["*"]), &held(&["b/c.txt"], &[])).is_some());
    }

    #[test]
    fn outside_names_what_a_run_changed_beyond_its_declaration() {
        assert_eq!(
            outside(
                &v(&["tools/spritegen/subjects/"]),
                &v(&[
                    "tools/spritegen/subjects/boar.py",
                    "tools/spritegen/render.py"
                ])
            ),
            v(&["tools/spritegen/render.py"])
        );
        assert!(outside(&[], &v(&["anything"])).is_empty());
    }
}
