//! Locate CLI-reported checklist lines and change only a checkbox marker.
//! The CLI remains authoritative for task identity and progress. Refuse ambiguous artifacts.
use crate::{Openspec, SpecError};
use cide_ipc::{ChangeName, SpecProgress, SpecTask, SpecWriteOutcome};
use std::{ops::Range, sync::Mutex};

static WRITER: Mutex<()> = Mutex::new(());

/// Mirrors upstream parseTaskLines, including nested bullets and fenced examples.
fn scan(text: &str) -> Vec<(SpecTask, Range<usize>)> {
    let mut rows = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
        let Some(rest) = trimmed
            .strip_prefix('-')
            .or_else(|| trimmed.strip_prefix('*'))
        else {
            offset += line.len();
            continue;
        };
        let rest = rest.trim_start();
        if let Some(inner) = rest.strip_prefix('[')
            && let Some(mark) = inner.chars().next()
        {
            let tail = &inner[mark.len_utf8()..];
            if (mark.is_whitespace() || mark == 'x' || mark == 'X') && tail.starts_with(']') {
                let start = offset + line.len() - inner.len();
                rows.push((
                    SpecTask {
                        done: mark == 'x' || mark == 'X',
                        description: tail[1..].trim().to_string(),
                    },
                    start..start + mark.len_utf8(),
                ));
            }
        }
        offset += line.len();
    }
    rows
}

fn progress(rows: &[(SpecTask, Range<usize>)]) -> SpecProgress {
    SpecProgress {
        total: rows.len() as u32,
        completed: rows.iter().filter(|(task, _)| task.done).count() as u32,
        tasks: rows
            .iter()
            .filter(|(task, _)| !task.description.is_empty())
            .map(|(task, _)| task.clone())
            .collect(),
    }
}

pub fn set_task(
    os: &Openspec,
    change: &ChangeName,
    index: usize,
    expected: &SpecProgress,
    done: bool,
) -> Result<SpecWriteOutcome, SpecError> {
    let _guard = WRITER
        .lock()
        .map_err(|_| SpecError::Write("Checklist writer unavailable. Retry.".into()))?;
    let detail = os.change(change)?;
    let directory = os.cwd.join("openspec/changes").join(change.as_str());
    if !matches!(detail.origin, cide_ipc::SpecOrigin::Active) {
        return Err(SpecError::Write(
            "Archived checklists are read-only.".into(),
        ));
    }
    if detail.progress != *expected {
        return Ok(SpecWriteOutcome::Conflicted { path: directory });
    }
    if index >= expected.tasks.len() {
        return Err(SpecError::Write(
            "This checklist step no longer exists. Reload the change.".into(),
        ));
    }
    let jail = directory
        .canonicalize()
        .map_err(|e| SpecError::Write(e.to_string()))?;
    let mut candidates = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for artifact in &detail.artifacts {
        for path in &artifact.existing {
            let Ok(path) = path.canonicalize() else {
                continue;
            };
            if !path.starts_with(&jail) || !seen.insert(path.clone()) {
                continue;
            }
            let stamp = cide_core::document::stamp_at(&path);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let rows = scan(&text);
            if progress(&rows) == *expected {
                candidates.push((path, stamp, text, rows));
            }
        }
    }
    if candidates.len() != 1 {
        return Err(SpecError::Write("Could not uniquely locate this checklist in OpenSpec's artifacts. Open the checklist document to edit it.".into()));
    }
    let (path, stamp, text, rows) = candidates.pop().unwrap();
    let range = rows
        .iter()
        .filter(|(task, _)| !task.description.is_empty())
        .nth(index)
        .unwrap()
        .1
        .clone();
    if stamp.is_none()
        || cide_core::document::stamp_at(&path) != stamp
        || std::fs::read_to_string(&path).ok().as_ref() != Some(&text)
    {
        return Ok(SpecWriteOutcome::Conflicted { path });
    }
    if expected.tasks[index].done != done {
        let mut replacement = text.clone();
        replacement.replace_range(range, if done { "x" } else { " " });
        cide_core::persist::write_atomic_with_mode(
            &path,
            replacement.as_bytes(),
            cide_core::persist::SHARED_MODE,
        )
        .map_err(|e| SpecError::Write(format!("{} could not be saved: {e}", path.display())))?;
    }
    Ok(SpecWriteOutcome::Written {
        change: os.change(change)?,
        path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markers_preserve_all_other_bytes_and_distinguish_duplicate_descriptions() {
        let text = "\u{feff}# Tasks\r\n  * [X] same\r\n- [ ]\r\n```\r\n- [ ] same\r\n```";
        let rows = scan(text);
        assert_eq!(
            progress(&rows),
            SpecProgress {
                total: 3,
                completed: 1,
                tasks: vec![
                    SpecTask {
                        done: true,
                        description: "same".into()
                    },
                    SpecTask {
                        done: false,
                        description: "same".into()
                    }
                ]
            }
        );
        let mut edited = text.to_string();
        edited.replace_range(rows[2].1.clone(), "x");
        assert_eq!(edited, text.replacen("- [ ] same", "- [x] same", 1));
    }
}
