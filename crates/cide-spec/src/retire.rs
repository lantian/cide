//! Retiring a capability — the one archive outcome that deletes a file from `openspec/specs/`.
//!
//! When a change's REMOVED deltas take a capability's *last* requirement, `openspec archive`
//! cannot write the spec (a spec with no requirements fails validation). It aborts, unless the
//! change's `.openspec.yaml` carries `retire_capabilities: true`, in which case it deletes the
//! emptied `spec.md`. The marker is deliberate on the CLI's side — the deletion is recoverable
//! only from git, so it is the author's call — and cide keeps that: it never writes the marker
//! unless the person ticked the box that says so (`spec_change_archive`'s `retire`).
//!
//! Until this module the preview said "10 requirements removed" and the failure arrived after
//! the click, as a refusal nobody could read. These are cide's own mirrors of the CLI's two
//! facts — which capabilities would be emptied, and whether the marker is set — and, like
//! [`crate::block`], are scanners rather than parsers: the CLI stays the authority on archive,
//! and a disagreement here costs a missing warning or a needless one, never a wrong write.

use std::io;
use std::path::{Path, PathBuf};

use crate::block;

/// The marker's key in `<change>/.openspec.yaml`.
pub const MARKER: &str = "retire_capabilities";
const METADATA: &str = ".openspec.yaml";

/// The requirement names a capability has today; empty when it has no spec file.
pub fn spec_requirements(root: &Path, spec: &str) -> Vec<String> {
    let path = crate::spec_path(root)
        .join("specs")
        .join(spec)
        .join("spec.md");
    std::fs::read_to_string(path)
        .map(|text| block::all_names(&text))
        .unwrap_or_default()
}

/// Does `.openspec.yaml` in this change directory say `retire_capabilities: true`?
///
/// Unreadable, missing or malformed all answer `false`: the preview then offers the checkbox,
/// which is the safe direction.
pub fn retire_declared(change_dir: &Path) -> bool {
    std::fs::read_to_string(change_dir.join(METADATA))
        .map(|text| marker_value(&text) == Some(true))
        .unwrap_or(false)
}

fn marker_value(text: &str) -> Option<bool> {
    for line in text.lines() {
        // Top-level keys only; a nested `retire_capabilities` is somebody else's key.
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        let Some(rest) = line.strip_prefix(MARKER) else {
            continue;
        };
        let Some(value) = rest.trim_start().strip_prefix(':') else {
            continue;
        };
        let value = value
            .split('#')
            .next()
            .unwrap_or("")
            .trim()
            .trim_matches(['"', '\'']);
        return match value {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        };
    }
    None
}

/// Write `retire_capabilities: true` into the change's `.openspec.yaml`. Returns whether the file
/// changed; idempotent, and every other line is kept as written.
pub fn declare_retire(change_dir: &Path) -> io::Result<bool> {
    let path: PathBuf = change_dir.join(METADATA);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let Some(next) = with_marker(&text) else {
        return Ok(false);
    };
    // Temp file + rename: the file is read by the CLI a moment later and may be open elsewhere.
    let tmp = path.with_extension("yaml.cide-tmp");
    std::fs::write(&tmp, next)?;
    std::fs::rename(&tmp, &path)?;
    Ok(true)
}

/// The text with the marker set, or `None` when it already is.
fn with_marker(text: &str) -> Option<String> {
    if marker_value(text) == Some(true) {
        return None;
    }
    let line = format!("{MARKER}: true");
    let mut out = String::with_capacity(text.len() + line.len() + 1);
    let mut replaced = false;
    for piece in text.split_inclusive('\n') {
        let bare = piece.trim_end_matches(['\n', '\r']);
        let top = !bare.starts_with(char::is_whitespace);
        if !replaced
            && top
            && bare
                .strip_prefix(MARKER)
                .is_some_and(|r| r.trim_start().starts_with(':'))
        {
            out.push_str(&line);
            out.push_str(&piece[bare.len()..]);
            if !piece.ends_with('\n') {
                out.push('\n');
            }
            replaced = true;
        } else {
            out.push_str(piece);
        }
    }
    if !replaced {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&line);
        out.push('\n');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marker_is_added_once_and_the_rest_is_kept() {
        let text = "schema: spec-driven\ncreated: 2026-11-04\n";
        let next = with_marker(text).unwrap();
        assert_eq!(
            next,
            "schema: spec-driven\ncreated: 2026-11-04\nretire_capabilities: true\n"
        );
        assert!(with_marker(&next).is_none());
    }

    #[test]
    fn a_false_marker_is_flipped_in_place() {
        let next = with_marker("a: 1\nretire_capabilities: false # no\nb: 2\n").unwrap();
        assert_eq!(next, "a: 1\nretire_capabilities: true\nb: 2\n");
    }

    #[test]
    fn a_missing_newline_and_an_empty_file_are_handled() {
        assert_eq!(
            with_marker("a: 1").unwrap(),
            "a: 1\nretire_capabilities: true\n"
        );
        assert_eq!(with_marker("").unwrap(), "retire_capabilities: true\n");
    }

    #[test]
    fn a_nested_key_is_not_the_marker() {
        assert_eq!(marker_value("x:\n  retire_capabilities: true\n"), None);
        assert_eq!(marker_value("retire_capabilities: \"true\"\n"), Some(true));
    }

    #[test]
    fn declare_and_read_round_trip() {
        let dir = std::env::temp_dir().join(format!("cide-retire-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(METADATA), "schema: spec-driven\n").unwrap();
        assert!(!retire_declared(&dir));
        assert!(declare_retire(&dir).unwrap());
        assert!(retire_declared(&dir));
        assert!(!declare_retire(&dir).unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }
}
