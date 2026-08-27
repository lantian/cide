//! Editing one requirement block in a delta file, safely. (M28)
//!
//! The CLI has no content-mutation command, so a panel that lets somebody fix a requirement has
//! to write the markdown. Everything here exists to make that as close to *not* writing markdown
//! as it can be:
//!
//! 1. The file is **located by asking the CLI**, never by joining a filename. `openspec status
//!    --change <c> --json` reports absolute `existingOutputPaths` per artifact, and the artifact
//!    set is schema-driven — anything that assumed `specs/<id>/spec.md` would break on the first
//!    project with a custom schema, and break by editing the wrong file rather than by failing.
//! 2. The block is addressed as a **byte range** ([`crate::block`]) and the replacement is
//!    spliced into it. Every other byte is copied. Nothing is re-serialised.
//! 3. The write is **compare-and-swapped** against a [`FileStamp`] taken at the read. An agent
//!    working in a worktree edits these same files, and the answer to losing a race must be a
//!    refusal, not a clobber.
//! 4. It is **validated before and after**, and rolled back if it got worse.
//!
//! # Why the after-check is a subset and not "green"
//!
//! Requiring `validate --strict` to pass after the write would make an already-broken file
//! uneditable — which is exactly the state somebody opens the editor to fix. So the rule is that
//! the set of blocking issues afterwards must be a **subset** of the set before: an edit may fix
//! things, may change nothing, and may not introduce anything. `TaskBoard::Unreadable`'s argument
//! in miniature — a tool that repairs a file it cannot parse by overwriting it has destroyed the
//! user's data to fix its own display.

use std::path::{Path, PathBuf};

use cide_core::persist;
use cide_ipc::{ChangeName, DeltaOperation, SpecId, SpecIssue, SpecWriteOutcome};

use crate::{Openspec, SpecError, block};

/// The artifact id that holds a change's delta specs, in OpenSpec's default schema.
///
/// A *starting point* and not an assumption: [`delta_path`] falls back to searching every
/// artifact's existing files for one whose path ends `specs/<id>/spec.md`, so a schema that calls
/// this artifact something else still works.
const SPECS_ARTIFACT: &str = "specs";

/// Replace one requirement block, or explain why not.
///
/// `block` is the whole replacement including its `### Requirement:` header — what the editor's
/// textarea holds. The name on that header must match `requirement`, which is checked here rather
/// than after the write, so a rename typed into the header is a refusal naming the field and not
/// a validator complaint arriving after a rollback.
pub fn set_requirement(
    os: &Openspec,
    change: &ChangeName,
    spec: &SpecId,
    operation: DeltaOperation,
    requirement: &str,
    replacement: &str,
) -> Result<SpecWriteOutcome, SpecError> {
    let path = delta_path(os, change, spec)?;

    // The verdict *before*, so the after-check can compare rather than demand. Taken before the
    // read so a file that is failing for an unrelated reason stays editable.
    let before = blocking_issues(&os.validate(Some(change))?);

    let raw = std::fs::read(&path).map_err(|error| {
        SpecError::Write(format!("{} could not be read: {error}", path.display()))
    })?;
    let text = String::from_utf8(raw).map_err(|_| {
        SpecError::Write(format!(
            "{} is not UTF-8, so cide will not rewrite it",
            path.display()
        ))
    })?;
    // `cide_core::document::stamp_at`, so this compares the same token an editor buffer does —
    // and through `canonicalize`, so a symlinked delta file is stamped on the inode the write
    // lands on rather than on the link.
    let stamp = cide_core::document::stamp_at(&path);

    let shape = Shape::of(&text);
    let normalised = shape.normalise(&text);
    let found = block::find(&normalised, operation.header(), requirement).map_err(|missing| {
        SpecError::Write(match missing {
            block::Missing::NoSection => format!(
                "{} has no `## {} Requirements` section, so there is no `{requirement}` to \
                 replace there",
                path.display(),
                operation.header()
            ),
            block::Missing::NoRequirement => format!(
                "{} has no requirement called `{requirement}` under `## {} Requirements`",
                path.display(),
                operation.header()
            ),
            block::Missing::Ambiguous(n) => format!(
                "{} has {n} requirements called `{requirement}` under `## {} Requirements`. \
                 cide will not guess which one you meant — give them distinct names first; \
                 `openspec archive` refuses this file too",
                path.display(),
                operation.header()
            ),
        })
    })?;

    let current = &normalised[found.start..found.end];
    check_replacement(operation, requirement, replacement, current)?;

    // The separator depends on whether anything follows.
    //
    // A block is followed by a blank line and then the next requirement — except the last block
    // in a file, which is followed by nothing. Appending `\n\n` unconditionally put a trailing
    // blank line into the file on *every* edit of the last requirement, and the file grew a line
    // each time; nothing failed, nothing was reported, and the diff was one invisible character.
    // Caught by the round-trip test, not by review.
    let rest = normalised[found.end..].trim_start_matches('\n');
    let spliced = if rest.is_empty() {
        format!("{}{}\n", &normalised[..found.start], replacement.trim_end())
    } else {
        format!(
            "{}{}\n\n{rest}",
            &normalised[..found.start],
            replacement.trim_end()
        )
    };
    let restored = shape.restore(&spliced);

    // Compare-and-swap. An agent in a worktree writes these same files; losing that race must be
    // a refusal, not a clobber. One `stat`, the primitive `TaskStore::refresh_from_disk` uses.
    //
    // `None` on either side means the filesystem would not answer, which `document::stamp_of`
    // documents as "no precondition to check" — and it is right to write in that case rather
    // than refuse for ever on a filesystem with no mtime, because the alternative is an editor
    // that never saves and cannot say why.
    let now = cide_core::document::stamp_at(&path);
    if let (Some(then), Some(now)) = (stamp, now)
        && then != now
    {
        return Ok(SpecWriteOutcome::Conflicted { path });
    }

    // 0o644: `openspec/` is committed and read in pull requests, exactly like `.cide/tasks.json`.
    persist::write_atomic_with_mode(&path, restored.as_bytes(), persist::SHARED_MODE).map_err(
        |error| SpecError::Write(format!("{} could not be written: {error}", path.display())),
    )?;

    let after = os.validate(Some(change))?;
    let introduced: Vec<SpecIssue> = blocking_issues(&after)
        .into_iter()
        .filter(|issue| !before.iter().any(|was| same_issue(was, issue)))
        .collect();
    if !introduced.is_empty() {
        // Put it back exactly as it was. The rollback is a second atomic write rather than a
        // temp-file dance because the bytes to restore are already in hand.
        persist::write_atomic_with_mode(&path, text.as_bytes(), persist::SHARED_MODE).map_err(
            |error| {
                SpecError::Write(format!(
                    "{} validated worse after the edit and could not be put back: {error}",
                    path.display()
                ))
            },
        )?;
        return Ok(SpecWriteOutcome::Regressed { issues: introduced });
    }

    Ok(SpecWriteOutcome::Written {
        change: os.change(change)?,
        path,
    })
}

/// Which file holds this capability's delta for this change.
///
/// Asks the CLI, then matches on the path's *tail components* rather than on a string: a spec id
/// may itself be a path (`identity/user-auth`), and a substring match would let one capability's
/// file answer for another whose id is a suffix of it.
fn delta_path(os: &Openspec, change: &ChangeName, spec: &SpecId) -> Result<PathBuf, SpecError> {
    let detail = os.change(change)?;
    let wanted: Vec<String> = std::iter::once("specs".to_string())
        .chain(spec.as_str().split('/').map(str::to_string))
        .chain(std::iter::once("spec.md".to_string()))
        .collect();

    let mut looked: Vec<String> = Vec::new();
    for artifact in &detail.artifacts {
        for path in &artifact.existing {
            if ends_with_components(path, &wanted) {
                return Ok(path.clone());
            }
            if artifact.id == SPECS_ARTIFACT {
                looked.push(path.display().to_string());
            }
        }
    }
    Err(SpecError::Write(format!(
        "the change `{change}` has no delta file for `{spec}`. Its spec files are: {}",
        if looked.is_empty() {
            "none at all".to_string()
        } else {
            looked.join(", ")
        }
    )))
}

/// Does `path` end with exactly these components?
fn ends_with_components(path: &Path, wanted: &[String]) -> bool {
    let have: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    have.len() >= wanted.len() && have[have.len() - wanted.len()..] == *wanted
}

/// The three things `validate --strict` rejects, checked before the file is touched.
///
/// Checked here as well as by the validator so the *editor* can say what is wrong with the field
/// somebody is typing in, at the moment they press Save. Learning it from a validator issue list
/// after a write and a rollback is the same information arriving too late to be about anything.
fn check_replacement(
    operation: DeltaOperation,
    requirement: &str,
    replacement: &str,
    current: &str,
) -> Result<(), SpecError> {
    let names = block::names(replacement, operation.header());
    let header_name = first_requirement_name(replacement);
    match header_name.as_deref() {
        None => {
            return Err(SpecError::Write(format!(
                "a requirement block has to begin with `### Requirement: {requirement}`"
            )));
        }
        Some(name) if name != requirement.trim() => {
            return Err(SpecError::Write(format!(
                "this block's header says `{name}` and it is replacing `{requirement}`. Renaming \
                 a requirement is a `## RENAMED Requirements` delta, not an edit — the archive \
                 matches requirements by name."
            )));
        }
        Some(_) => {}
    }
    let _ = names;

    if operation != DeltaOperation::Removed {
        if block::scenario_names(replacement).is_empty() {
            return Err(SpecError::Write(
                "every requirement needs at least one `#### Scenario:` — a requirement with no \
                 scenario is a statement nothing can check, and `openspec validate` refuses it"
                    .to_string(),
            ));
        }
        if !replacement.contains("SHALL") && !replacement.contains("MUST") {
            return Err(SpecError::Write(
                "a requirement has to say SHALL or MUST — that is what makes it a requirement \
                 rather than a note, and `openspec validate --strict` refuses it without one"
                    .to_string(),
            ));
        }
    }

    // A MODIFIED block replaces the whole requirement, so a scenario the current block has and
    // this one does not is a deletion. `archive` refuses that at the far end; refusing it here is
    // the same refusal, at the point where the scenario is still on screen to be put back.
    if operation == DeltaOperation::Modified {
        let had = block::scenario_names(current);
        let has = block::scenario_names(replacement);
        let dropped: Vec<String> = had.into_iter().filter(|name| !has.contains(name)).collect();
        if !dropped.is_empty() {
            return Err(SpecError::Write(format!(
                "this edit drops {} that the requirement has now: {}. A MODIFIED requirement \
                 replaces its whole block, so a scenario left out is a scenario deleted — add it \
                 back, or remove it deliberately in its own change.",
                if dropped.len() == 1 {
                    "a scenario"
                } else {
                    "scenarios"
                },
                dropped.join(", ")
            )));
        }
    }
    Ok(())
}

/// The name on the first `### Requirement:` header in a block.
fn first_requirement_name(block: &str) -> Option<String> {
    let line = block.lines().find(|line| !line.trim().is_empty())?;
    let rest = line.trim_start().strip_prefix("###")?;
    let rest = rest.trim_start();
    let lower = rest.to_ascii_lowercase();
    lower
        .strip_prefix("requirement:")
        .map(|_| rest["requirement:".len()..].trim().to_string())
}

/// A file's line endings and byte-order mark, so they can be put back.
///
/// The only two normalisations the write path performs, and both are undone. A cide that
/// converted a CRLF file to LF on an edit would produce a diff touching every line of a file
/// somebody else maintains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shape {
    bom: bool,
    crlf: bool,
}

impl Shape {
    fn of(text: &str) -> Self {
        Self {
            bom: text.starts_with('\u{feff}'),
            crlf: text.contains("\r\n"),
        }
    }

    fn normalise(self, text: &str) -> String {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        if self.crlf {
            text.replace("\r\n", "\n")
        } else {
            text.to_string()
        }
    }

    fn restore(self, text: &str) -> String {
        let mut out = if self.crlf {
            text.replace('\n', "\r\n")
        } else {
            text.to_string()
        };
        if self.bom {
            out.insert(0, '\u{feff}');
        }
        out
    }
}

/// The issues that make a change invalid.
fn blocking_issues(validation: &cide_ipc::SpecValidation) -> Vec<SpecIssue> {
    validation
        .issues
        .iter()
        .filter(|issue| issue.blocking())
        .cloned()
        .collect()
}

/// Are these the same complaint?
///
/// Compared on message and path and **not on line**, deliberately: a write moves every line below
/// it, so a pre-existing issue would otherwise look introduced and every edit to a file with one
/// unrelated problem would roll itself back.
fn same_issue(a: &SpecIssue, b: &SpecIssue) -> bool {
    a.message == b.message && a.path == b.path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(n: usize) -> String {
        "#".repeat(n)
    }

    fn requirement(name: &str, body: &str, scenario: &str) -> String {
        format!(
            "{} Requirement: {name}\n{body}\n\n{} Scenario: {scenario}\n- **WHEN** a\n- **THEN** b\n",
            h(3),
            h(4)
        )
    }

    #[test]
    fn a_block_whose_header_renames_the_requirement_is_refused() {
        // The archive matches requirements by name, so a rename typed into a header is a silent
        // ADD plus a silent DELETE at archive time.
        let current = requirement("Theme switching", "The app SHALL switch.", "toggled");
        let renamed = requirement("Theme switch", "The app SHALL switch.", "toggled");
        let error = check_replacement(
            DeltaOperation::Modified,
            "Theme switching",
            &renamed,
            &current,
        )
        .expect_err("refused");
        assert!(error.to_string().contains("RENAMED"), "{error}");
    }

    #[test]
    fn a_requirement_with_no_scenario_or_no_shall_is_refused_before_the_write() {
        let current = requirement("R", "The app SHALL do it.", "s");
        let no_scenario = format!("{} Requirement: R\nThe app SHALL do it.\n", h(3));
        let error = check_replacement(DeltaOperation::Added, "R", &no_scenario, &current)
            .expect_err("refused");
        assert!(error.to_string().contains("Scenario"), "{error}");

        let no_shall = format!(
            "{} Requirement: R\nThe app does it.\n\n{} Scenario: s\n- **WHEN** a\n",
            h(3),
            h(4)
        );
        let error = check_replacement(DeltaOperation::Added, "R", &no_shall, &current)
            .expect_err("refused");
        assert!(error.to_string().contains("SHALL"), "{error}");

        // A REMOVED delta states a reason and a migration, not a scenario, so it is exempt.
        let removed = format!("{} Requirement: R\n**Reason**: gone\n", h(3));
        assert!(check_replacement(DeltaOperation::Removed, "R", &removed, &current).is_ok());
    }

    #[test]
    fn a_modified_block_that_drops_a_scenario_is_refused_here_rather_than_at_archive_time() {
        let current = format!(
            "{} Requirement: R\nThe app SHALL do it.\n\n{} Scenario: one\n- **WHEN** a\n\n\
             {} Scenario: two\n- **WHEN** b\n",
            h(3),
            h(4),
            h(4)
        );
        let shorter = requirement("R", "The app SHALL do it.", "one");
        let error = check_replacement(DeltaOperation::Modified, "R", &shorter, &current)
            .expect_err("refused");
        assert!(error.to_string().contains("two"), "{error}");
        assert!(error.to_string().contains("deleted"), "{error}");

        // …and the same edit under ADDED is not a deletion of anything.
        assert!(check_replacement(DeltaOperation::Added, "R", &shorter, &current).is_ok());
    }

    #[test]
    fn crlf_and_a_bom_survive_the_round_trip() {
        // A cide that silently converted line endings would produce a diff touching every line of
        // a file somebody else maintains.
        let text = "\u{feff}## ADDED Requirements\r\n\r\n### Requirement: R\r\nBody.\r\n";
        let shape = Shape::of(text);
        assert!(shape.bom && shape.crlf);
        let normalised = shape.normalise(text);
        assert!(!normalised.contains('\r'));
        assert!(!normalised.starts_with('\u{feff}'));
        assert_eq!(shape.restore(&normalised), text);

        // And a plain file stays plain.
        let plain = "## ADDED Requirements\n\n### Requirement: R\nBody.\n";
        let shape = Shape::of(plain);
        assert_eq!(shape.restore(&shape.normalise(plain)), plain);
    }

    #[test]
    fn an_issue_is_matched_without_its_line_number() {
        // A write moves every line below it. Comparing on line would make a pre-existing issue
        // look introduced, and every edit to a file with one unrelated problem would roll back.
        let a = SpecIssue {
            level: "ERROR".into(),
            path: "requirement".into(),
            message: "needs a scenario".into(),
            line: Some(4),
            column: None,
        };
        let moved = SpecIssue {
            line: Some(19),
            ..a.clone()
        };
        assert!(same_issue(&a, &moved));
        let other = SpecIssue {
            message: "needs a SHALL".into(),
            ..a.clone()
        };
        assert!(!same_issue(&a, &other));
    }

    #[test]
    fn a_delta_file_is_matched_on_whole_components_and_not_on_a_substring() {
        // A spec id can itself be a path, so `user-auth` must not be answered by a file belonging
        // to `identity/user-auth`.
        let wanted = vec![
            "specs".to_string(),
            "user-auth".to_string(),
            "spec.md".to_string(),
        ];
        assert!(ends_with_components(
            Path::new("/repo/openspec/changes/c/specs/user-auth/spec.md"),
            &wanted
        ));
        assert!(!ends_with_components(
            Path::new("/repo/openspec/changes/c/specs/identity/user-auth/spec.md"),
            &[
                "specs".to_string(),
                "user-auth".to_string(),
                "spec.md".to_string()
            ]
        ));
    }
}
