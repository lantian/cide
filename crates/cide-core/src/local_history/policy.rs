use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use cide_fs::{Filter, Visibility};
use cide_ipc::local_history::{HISTORY_MAX_FILE_BYTES, HistoryPolicy, HistoryRootId};
use globset::{Glob, GlobSet, GlobSetBuilder};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootIdentity {
    pub path: PathBuf,
    pub id: HistoryRootId,
}

impl RootIdentity {
    pub fn new(path: &Path) -> Result<Self, String> {
        let path = fs::canonicalize(path).map_err(|e| e.to_string())?;
        if !path.is_dir() {
            return Err("History root must be a directory".into());
        }
        // Native bytes, rather than lossy display strings, keep distinct roots distinct.
        let id = HistoryRootId(
            blake3::hash(path.as_os_str().as_encoded_bytes())
                .to_hex()
                .to_string(),
        );
        Ok(Self { path, id })
    }

    /// Resolve absent paths through their nearest existing ancestor. Restore additionally
    /// requires the immediate parent to exist before creating anything.
    pub fn resolve(&self, path: &Path) -> Result<(PathBuf, String), String> {
        let absolute = if path.is_absolute() {
            path.to_owned()
        } else {
            validate_relative(path)?;
            self.path.join(path)
        };
        if absolute
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        {
            return Err("Parent traversal is not a history path".into());
        }
        let resolved = match fs::symlink_metadata(&absolute) {
            Ok(_) => fs::canonicalize(&absolute).map_err(|e| e.to_string())?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut ancestor = absolute.as_path();
                let mut suffix = Vec::new();
                loop {
                    match fs::symlink_metadata(ancestor) {
                        Ok(_) => break,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            suffix.push(ancestor.file_name().ok_or("Missing ancestor")?.to_owned());
                            ancestor = ancestor.parent().ok_or("Missing ancestor")?;
                        }
                        Err(e) => return Err(e.to_string()),
                    }
                }
                let mut parent = fs::canonicalize(ancestor).map_err(|e| e.to_string())?;
                if !parent.is_dir() {
                    return Err("The ancestor must be a directory".into());
                }
                for name in suffix.into_iter().rev() {
                    parent.push(name);
                }
                parent
            }
            Err(e) => return Err(e.to_string()),
        };
        let relative = resolved
            .strip_prefix(&self.path)
            .map_err(|_| "Symlink target is outside the history root")?;
        validate_relative(relative)?;
        let relative = relative
            .to_str()
            .ok_or("History requires a UTF-8 relative path")?
            .to_owned();
        Ok((resolved, relative))
    }
}

pub fn validate_relative(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("History paths must be root-relative without traversal".into());
    }
    Ok(())
}

pub fn deepest_root<'a>(roots: &'a [RootIdentity], path: &Path) -> Option<&'a RootIdentity> {
    roots
        .iter()
        .filter(|r| path.starts_with(&r.path))
        .max_by_key(|r| r.path.components().count())
}

pub fn validate_policy(policy: &HistoryPolicy) -> Result<GlobSet, String> {
    if policy.retention_days == 0 || policy.budget_bytes == 0 {
        return Err("Retention days and history budget must be positive".into());
    }
    if policy.exclusions.len() > 256 {
        return Err("At most 256 exclusions are allowed".into());
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in &policy.exclusions {
        if pattern.len() > 1024 || pattern.contains('\\') || pattern.contains('\0') {
            return Err("Invalid history exclusion".into());
        }
        validate_relative(Path::new(pattern))?;
        builder.add(Glob::new(pattern).map_err(|e| e.to_string())?);
    }
    builder.build().map_err(|e| e.to_string())
}

#[derive(Debug)]
pub struct HistoryEligibility {
    pub root: RootIdentity,
    pub policy: HistoryPolicy,
    filter: Filter,
    exclusions: GlobSet,
}

impl HistoryEligibility {
    pub fn new(root: RootIdentity, policy: HistoryPolicy) -> Result<Self, String> {
        let exclusions = validate_policy(&policy)?;
        let filter = Filter::build(
            std::slice::from_ref(&root.path),
            vec![root.path.as_path()],
            Visibility {
                hidden: true,
                ignored: false,
            },
        );
        Ok(Self {
            root,
            policy,
            filter,
            exclusions,
        })
    }

    /// This answer never consults tree visibility or the tree's explicit watch exceptions.
    pub fn path(&self, path: &Path, is_dir: bool) -> Result<(PathBuf, String), String> {
        let (physical, relative) = self.root.resolve(path)?;
        // Check both spellings: an excluded alias cannot bypass policy through its target.
        let lexical = if path.is_absolute() {
            path.to_owned()
        } else {
            self.root.path.join(path)
        };
        self.check_components(&lexical, is_dir)?;
        self.check_components(&physical, is_dir)?;
        Ok((physical, relative))
    }

    fn check_components(&self, path: &Path, is_dir: bool) -> Result<(), String> {
        let storage = crate::persist::state_dir().join("local-history");
        let storage = fs::canonicalize(&storage).unwrap_or(storage);
        if path.starts_with(storage) {
            return Err("Local History storage is excluded".into());
        }
        let relative = path
            .strip_prefix(&self.root.path)
            .map_err(|_| "Outside history root")?;
        validate_relative(relative)?;
        let mut prefix = self.root.path.clone();
        let components: Vec<_> = relative.components().collect();
        for (i, c) in components.iter().enumerate() {
            let name = c.as_os_str().to_string_lossy();
            if matches!(
                name.as_ref(),
                ".git"
                    | "node_modules"
                    | "vendor"
                    | ".venv"
                    | "venv"
                    | "__pycache__"
                    | ".pnpm-store"
            ) {
                return Err("Git metadata or dependency directory is excluded".into());
            }
            if name == ".env" || name.starts_with(".env.") {
                return Err("Environment files are excluded by default".into());
            }
            self.filter.refresh_rules(&prefix);
            prefix.push(c.as_os_str());
            let directory = i + 1 < components.len() || is_dir;
            if self.filter.is_ignored(&prefix, directory) {
                return Err("Excluded by Git ignore rules".into());
            }
            if self
                .exclusions
                .is_match(prefix.strip_prefix(&self.root.path).unwrap())
            {
                return Err("Excluded by root history policy".into());
            }
        }
        Ok(())
    }

    /// Bounded even if an external writer grows the file after stat.
    pub fn read(&self, path: &Path) -> Result<(String, Option<Vec<u8>>), String> {
        let (physical, relative) = self.path(path, false)?;
        match fs::metadata(&physical) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((relative, None)),
            Err(e) => Err(e.to_string()),
            Ok(meta) => {
                if !meta.is_file() {
                    return Err("History supports regular files only".into());
                }
                if meta.len() > HISTORY_MAX_FILE_BYTES {
                    return Err("History supports text up to 2 MiB".into());
                }
                let mut bytes = Vec::new();
                fs::File::open(physical)
                    .map_err(|e| e.to_string())?
                    .take(HISTORY_MAX_FILE_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                Self::validate_bytes(&bytes)?;
                Ok((relative, Some(bytes)))
            }
        }
    }

    pub fn validate_bytes(bytes: &[u8]) -> Result<(), String> {
        if bytes.len() as u64 > HISTORY_MAX_FILE_BYTES {
            return Err("History supports text up to 2 MiB".into());
        }
        if bytes.contains(&0) || std::str::from_utf8(bytes).is_err() {
            return Err("History supports UTF-8 text without NUL bytes".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_fs::testing::scratch;

    #[test]
    fn non_git_hidden_files_and_ignores_are_independent_of_tree_visibility() {
        let dir = scratch("history-policy");
        fs::create_dir_all(dir.join(".hidden")).unwrap();
        fs::write(dir.join(".gitignore"), "build/\n*.log\n!keep.log\n").unwrap();
        fs::create_dir_all(dir.join("build")).unwrap();
        let policy =
            HistoryEligibility::new(RootIdentity::new(&dir).unwrap(), HistoryPolicy::default())
                .unwrap();
        for name in [".hidden/source", "keep.log", "source"] {
            fs::write(dir.join(name), b"a\r\nb\r\n").unwrap();
            assert_eq!(
                policy.read(Path::new(name)).unwrap().1.unwrap(),
                b"a\r\nb\r\n"
            );
        }
        for name in [
            ".env",
            ".env.local",
            "build/a",
            "node_modules/a",
            ".git/HEAD",
            "noise.log",
        ] {
            assert!(policy.path(Path::new(name), false).is_err(), "{name}");
        }
        assert!(policy.path(Path::new("../escape"), false).is_err());
        assert!(HistoryEligibility::validate_bytes(b"\xff").is_err());
        assert!(HistoryEligibility::validate_bytes(b"a\0b").is_err());
        assert!(
            HistoryEligibility::validate_bytes(&vec![b'a'; HISTORY_MAX_FILE_BYTES as usize + 1])
                .is_err()
        );
    }

    #[test]
    fn roots_are_canonical_deepest_and_worktrees_are_separate() {
        let dir = scratch("history-roots");
        for p in ["nested", "other"] {
            fs::create_dir(dir.join(p)).unwrap();
        }
        let roots = vec![
            RootIdentity::new(&dir).unwrap(),
            RootIdentity::new(&dir.join("nested")).unwrap(),
            RootIdentity::new(&dir.join("other")).unwrap(),
        ];
        assert_eq!(deepest_root(&roots, &dir.join("nested/a")), Some(&roots[1]));
        assert_ne!(roots[1].id, roots[2].id);
        assert_eq!(RootIdentity::new(&dir.join("nested/..")).unwrap(), roots[0]);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_aliases_share_identity_and_escapes_are_refused() {
        use std::os::unix::fs::symlink;
        let dir = scratch("history-links");
        let outside = scratch("history-outside");
        fs::write(dir.join("source"), "text").unwrap();
        symlink(dir.join("source"), dir.join("alias")).unwrap();
        symlink(&*outside, dir.join("escape")).unwrap();
        let p = HistoryEligibility::new(RootIdentity::new(&dir).unwrap(), HistoryPolicy::default())
            .unwrap();
        assert_eq!(p.path(Path::new("alias"), false).unwrap().1, "source");
        assert!(p.path(Path::new("escape/new"), false).is_err());
        assert_eq!(
            p.path(Path::new("missing/child"), false).unwrap().1,
            "missing/child"
        );
    }

    #[test]
    fn user_exclusions_and_limits_are_validated() {
        let dir = scratch("history-exclusions");
        let mut policy = HistoryPolicy {
            exclusions: vec!["secret/**".into(), "*.key".into()],
            ..Default::default()
        };
        fs::create_dir(dir.join("secret")).unwrap();
        let p = HistoryEligibility::new(RootIdentity::new(&dir).unwrap(), policy.clone()).unwrap();
        assert!(p.path(Path::new("secret/a"), false).is_err());
        assert!(p.path(Path::new("test.key"), false).is_err());
        for pattern in ["../secret", "/absolute", "[broken"] {
            policy.exclusions = vec![pattern.into()];
            assert!(validate_policy(&policy).is_err());
        }
        policy.exclusions.clear();
        policy.budget_bytes = 0;
        assert!(validate_policy(&policy).is_err());
    }
}
