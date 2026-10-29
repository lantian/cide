//! Root-relative folder exclusions, independent of gitignore and visibility.
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct Exclusions {
    roots: Vec<PathBuf>,
    rules: Vec<String>,
    globs: GlobSet,
}

impl Exclusions {
    pub fn compile(roots: Vec<PathBuf>, rules: &[String]) -> Result<Self, String> {
        let mut builder = GlobSetBuilder::new();
        if rules.len() > 256 {
            return Err("Use at most 256 indexing exclusions".into());
        }
        for rule in rules {
            let pattern = rule
                .strip_prefix("./")
                .unwrap_or(rule)
                .trim_end_matches('/');
            if pattern.is_empty()
                || rule.len() > 1024
                || pattern.starts_with(['/', '!', '~'])
                || pattern.contains(['\\', ':'])
                || pattern
                    .split('/')
                    .any(|c| c == ".." || c == "." || c.is_empty())
            {
                return Err(format!(
                    "Invalid folder exclusion {rule:?}: use a root-relative path or glob with / separators"
                ));
            }
            builder.add(
                GlobBuilder::new(pattern)
                    .literal_separator(true)
                    .backslash_escape(false)
                    .build()
                    .map_err(|e| format!("Invalid folder exclusion {rule:?}: {e}"))?,
            );
        }
        Ok(Self {
            roots,
            rules: rules.to_vec(),
            globs: builder.build().map_err(|e| e.to_string())?,
        })
    }

    pub fn rules(&self) -> &[String] {
        &self.rules
    }

    /// Test directories and every directory ancestor, never a similarly named file.
    /// The deepest project root owns the path; an opened worktree is its own project.
    pub fn matching_rule(&self, path: &Path, is_dir: bool) -> Option<&str> {
        if self.rules.is_empty() {
            return None;
        }
        let root = self
            .roots
            .iter()
            .filter(|r| path.starts_with(r))
            .max_by_key(|r| r.components().count())?;
        let mut dir = if is_dir { path } else { path.parent()? };
        while dir != root {
            let relative = dir
                .strip_prefix(root)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            if let Some(i) = self.globs.matches(relative).first() {
                return Some(&self.rules[*i]);
            }
            dir = dir.parent()?;
        }
        None
    }
}
