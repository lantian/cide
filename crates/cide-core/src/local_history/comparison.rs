use super::HistoryStore;
use cide_ipc::local_history::*;
use std::path::Path;

impl HistoryStore {
    pub fn validate_restore(
        &self,
        token: &HistoryCompareToken,
    ) -> Result<std::path::PathBuf, String> {
        self.check_generation(token.generation)?;
        self.revision_belongs(&token.path, &token.target)?;
        if !self.eligibility.policy.enabled {
            return Err(
                "Enable Local History before restoring so it can retain a safety revision".into(),
            );
        }
        if self.latest(&token.path).map(|r| &r.lifecycle) != token.lifecycle.as_ref() {
            return Err("The file lifecycle changed; refresh the comparison".into());
        }
        let (physical, relative) = self.eligibility.path(Path::new(&token.path), false)?;
        if physical.to_string_lossy() != token.canonical_target || relative != token.path {
            return Err("The symlink target changed; refresh the comparison".into());
        }
        let (_, current) = self.eligibility.read(Path::new(&token.path))?;
        if Self::content(current.as_deref()) != token.current {
            return Err(
                "The file changed on disk; refresh the comparison and confirm again".into(),
            );
        }
        let parent = physical.parent().ok_or("Missing parent directory")?;
        if !parent.is_dir() {
            return Err("The parent directory must exist before restoring".into());
        }
        for path in [
            Some(parent),
            physical.exists().then_some(physical.as_path()),
        ]
        .into_iter()
        .flatten()
        {
            if std::fs::metadata(path)
                .map_err(|e| e.to_string())?
                .permissions()
                .readonly()
            {
                return Err("The restore target is read-only; fix its permissions first".into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStrExt;
                let name = std::ffi::CString::new(path.as_os_str().as_bytes())
                    .map_err(|_| "Invalid target path")?;
                if unsafe { libc::access(name.as_ptr(), libc::W_OK) } != 0 {
                    return Err(
                        "The restore target is not writable; fix its permissions first".into(),
                    );
                }
            }
        }
        self.bytes(&self.get(&token.target)?.content)?;
        Ok(physical)
    }
    fn revision_side(&self, path: &str, id: &LocalRevisionId) -> Result<HistorySide, String> {
        self.revision_belongs(path, id)?;
        let r = self.get(id)?;
        let (text, unavailable) = match self.bytes(&r.content) {
            Ok(bytes) => (
                bytes.map(|b| String::from_utf8(b).expect("store validates UTF-8")),
                None,
            ),
            Err(reason) => (None, Some(reason)),
        };
        Ok(HistorySide {
            revision: Some(id.clone()),
            observed_ms: Some(r.observed_ms),
            content: r.content.clone(),
            text,
            unavailable,
        })
    }

    pub fn compare(
        &self,
        path: &str,
        target: &LocalRevisionId,
        other: Option<&LocalRevisionId>,
    ) -> Result<HistoryComparison, String> {
        let (physical, relative) = self.eligibility.root.resolve(Path::new(path))?;
        let left = self.revision_side(&relative, target)?;
        let current = self.eligibility.read(Path::new(&relative));
        let (current_content, current_text, unavailable) = match current {
            Ok((_, bytes)) => (
                Self::content(bytes.as_deref()),
                bytes.map(|b| String::from_utf8(b).expect("eligibility validates UTF-8")),
                None,
            ),
            Err(reason) => (HistoryContent::Absent, None, Some(reason)),
        };
        let right = match other {
            Some(id) => self.revision_side(&relative, id)?,
            None => HistorySide {
                revision: None,
                observed_ms: Some(super::store::now_ms()),
                content: current_content.clone(),
                text: current_text,
                unavailable,
            },
        };
        Ok(HistoryComparison {
            left,
            right,
            token: HistoryCompareToken {
                generation: self.generation(),
                path: relative.clone(),
                canonical_target: physical.to_string_lossy().into_owned(),
                current: current_content,
                target: target.clone(),
                lifecycle: self.latest(&relative).map(|r| r.lifecycle.clone()),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restore_validation_detects_same_length_changes_and_missing_parents() {
        let project = scratch("history-stale-restore");
        let state = scratch("history-stale-state");
        std::fs::write(project.join("a"), "A").unwrap();
        let mut s =
            HistoryStore::open(RootIdentity::new(&project).unwrap(), state.to_path_buf()).unwrap();
        let id = s
            .capture(1, "a", Some(b"retained"), HistorySource::Baseline)
            .unwrap();
        let token = s.compare("a", &id, None).unwrap().token;
        s.validate_restore(&token).unwrap();
        std::fs::write(project.join("a"), "B").unwrap();
        assert!(
            s.validate_restore(&token)
                .unwrap_err()
                .contains("changed on disk")
        );
        let id = s
            .capture(
                1,
                "gone/child",
                Some(b"recoverable"),
                HistorySource::Baseline,
            )
            .unwrap();
        let token = s.compare("gone/child", &id, None).unwrap().token;
        assert!(
            s.validate_restore(&token)
                .unwrap_err()
                .contains("parent directory")
        );
    }
    #[cfg(unix)]
    #[test]
    fn restore_validation_rejects_changed_symlink_targets_and_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let project = scratch("history-symlink-restore");
        let state = scratch("history-symlink-state");
        std::fs::write(project.join("a"), "A").unwrap();
        std::fs::write(project.join("other"), "A").unwrap();
        let mut s =
            HistoryStore::open(RootIdentity::new(&project).unwrap(), state.to_path_buf()).unwrap();
        let id = s
            .capture(1, "a", Some(b"old"), HistorySource::Baseline)
            .unwrap();
        let token = s.compare("a", &id, None).unwrap().token;
        std::fs::set_permissions(project.join("a"), std::fs::Permissions::from_mode(0o444))
            .unwrap();
        assert!(s.validate_restore(&token).is_err());
        std::fs::set_permissions(project.join("a"), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        std::fs::remove_file(project.join("a")).unwrap();
        symlink(project.join("other"), project.join("a")).unwrap();
        assert!(
            s.validate_restore(&token)
                .unwrap_err()
                .contains("symlink target")
        );
    }
    use crate::local_history::RootIdentity;
    use cide_fs::testing::scratch;
    #[test]
    fn comparisons_are_scoped_byte_exact_and_represent_absence() {
        let project = scratch("history-compare");
        let state = scratch("history-compare-state");
        let mut s =
            HistoryStore::open(RootIdentity::new(&project).unwrap(), state.to_path_buf()).unwrap();
        let first = s
            .capture(1, "a", Some(b"A\r\n"), HistorySource::Baseline)
            .unwrap();
        let second = s
            .capture(1, "a", Some(b"B\r\n"), HistorySource::External)
            .unwrap();
        let comparison = s.compare("a", &first, None).unwrap();
        assert_eq!(comparison.left.text.as_deref(), Some("A\r\n"));
        assert_eq!(comparison.right.content, HistoryContent::Absent);
        assert!(!project.join("a").exists());
        assert_eq!(
            s.compare("a", &first, Some(&second))
                .unwrap()
                .right
                .text
                .as_deref(),
            Some("B\r\n")
        );
        assert!(s.compare("other", &first, None).is_err());
        assert!(s.compare("../escape", &first, None).is_err());
    }
}
