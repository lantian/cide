use super::HistoryStore;
use cide_ipc::local_history::*;
use std::path::Path;

impl HistoryStore {
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
