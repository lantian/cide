//! LSP's shapes into cide's.
//!
//! # The encoding decision, made once and commented so a tidying pass cannot undo it
//!
//! LSP's `Position.character` is a **UTF-16 code unit** offset (we declare `utf-16` in the
//! handshake, which is the only encoding every server supports). `cide_ipc::Diagnostic::column`
//! is 1-based UTF-16 too, so the conversion is `character + 1` and nothing else.
//!
//! It is tempting to "fix" this into Unicode scalar values, because that is what a human means by
//! a column number. Don't: the consumer is CodeMirror, which is itself UTF-16-native, so the
//! unconverted offset is what makes *jumping to the problem* land on the right character. A
//! scalar-value column would fix the digits printed after the message and break the navigation —
//! and it would need the document's text, which is in the webview, for files that have no tab
//! open at all.
//!
//! The digits are wrong only on a line containing a non-BMP character (an emoji), where LSP
//! counts a surrogate pair as two. That is the price, and it is the right way round.

use std::path::{Path, PathBuf};

use cide_ipc::{Diagnostic, DiagnosticKind, Severity};

/// `file:///home/u/x.rs` → `/home/u/x.rs`.
///
/// Hand-rolled rather than pulling in a `url` crate for one job, the same call
/// `cide_ide_mcp::lockfile` makes about its own small (de)serialization. Percent-decoding is
/// required and not optional: a path with a space arrives as `%20`, and a client that skipped it
/// would fail to match exactly the paths users complain about.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file://host/path` is legal and names another machine's file. We only serve local paths, so
    // an authority we did not write is refused rather than silently treated as a relative path.
    let rest = match rest.strip_prefix('/') {
        Some(after) => format!("/{after}"),
        None => return None,
    };
    Some(PathBuf::from(percent_decode(&rest)))
}

/// `/home/u/x.rs` → `file:///home/u/x.rs`.
pub fn path_to_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            // The unreserved set, plus the separators a path needs to keep readable. Everything
            // else is escaped — a `#` or `?` left raw would truncate the URI at the server.
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    // A path that decodes to invalid UTF-8 is one we cannot name to the frontend anyway — the
    // same posture `cide-fs` takes when it drops non-UTF-8 paths rather than lossily converting.
    String::from_utf8_lossy(&out).into_owned()
}

/// One LSP diagnostic into cide's, given the paths already resolved.
///
/// `rel` is passed rather than computed because working it out needs the project's roots and
/// their labels, which this crate does not have and should not.
pub fn diagnostic(
    lsp: &lsp_types::Diagnostic,
    abs_path: &str,
    rel: &str,
    default_source: &str,
) -> Diagnostic {
    Diagnostic {
        path: rel.to_string(),
        abs_path: abs_path.to_string(),
        // LSP is 0-based in both axes; everything downstream is 1-based. See the module docs for
        // why the column is *not* otherwise converted.
        line: lsp.range.start.line + 1,
        column: lsp.range.start.character + 1,
        end_line: lsp.range.end.line + 1,
        end_column: lsp.range.end.character + 1,
        severity: severity(lsp.severity),
        // A language server's findings are semantic even when they are about syntax: it is the
        // *editor's* tree-sitter pass that owns `Syntax`, and the per-editor "syntax only"
        // highlighting level exists to fall back to something that still works when a file is too
        // broken for the server to say anything useful.
        kind: DiagnosticKind::Semantic,
        message: lsp.message.clone(),
        // `source` names the *producer*, and a server may attribute to a sub-tool: rust-analyzer
        // reports clippy's lints with `source: "clippy"`, and a user who turns clippy off in
        // Settings must be able to. Falling back to the server's own name means every diagnostic
        // is attributable, which is what makes the per-source filter total.
        source: lsp
            .source
            .clone()
            .unwrap_or_else(|| default_source.to_string()),
        code: lsp.code.as_ref().map(|code| match code {
            lsp_types::NumberOrString::Number(n) => n.to_string(),
            lsp_types::NumberOrString::String(s) => s.clone(),
        }),
    }
}

/// LSP's four severities, and the answer for a server that sends none.
///
/// The spec makes `severity` optional and says the client decides. `Error` rather than `Info`,
/// because under-reporting a real error is the failure that costs the user something — and every
/// server we target sets it anyway.
fn severity(from: Option<lsp_types::DiagnosticSeverity>) -> Severity {
    match from {
        Some(lsp_types::DiagnosticSeverity::WARNING) => Severity::Warning,
        Some(lsp_types::DiagnosticSeverity::INFORMATION) => Severity::Info,
        Some(lsp_types::DiagnosticSeverity::HINT) => Severity::Hint,
        _ => Severity::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uri_round_trips_through_a_path() {
        let path = Path::new("/home/u/work/cide/src/lib.rs");
        assert_eq!(uri_to_path(&path_to_uri(path)).as_deref(), Some(path));
    }

    #[test]
    fn a_path_with_a_space_survives_both_directions() {
        // The case a client without percent-coding fails on, and exactly the paths users have.
        let path = Path::new("/home/u/My Projects/a b.rs");
        let uri = path_to_uri(path);
        assert!(uri.contains("%20"), "{uri}");
        assert_eq!(uri_to_path(&uri).as_deref(), Some(path));
    }

    #[test]
    fn a_path_with_a_hash_is_escaped_rather_than_truncating_the_uri() {
        let path = Path::new("/tmp/a#b/c.rs");
        let uri = path_to_uri(path);
        assert!(uri.contains("%23"), "{uri}");
        assert_eq!(uri_to_path(&uri).as_deref(), Some(path));
    }

    #[test]
    fn a_non_ascii_path_survives() {
        let path = Path::new("/home/u/日本/lib.rs");
        assert_eq!(uri_to_path(&path_to_uri(path)).as_deref(), Some(path));
    }

    #[test]
    fn a_uri_that_is_not_a_local_file_is_refused() {
        // `untitled:` buffers and `file://host/share` both reach a client that asks for
        // everything. Neither names a path on this machine.
        assert!(uri_to_path("untitled:Untitled-1").is_none());
        assert!(uri_to_path("file://otherhost/share/x.rs").is_none());
        assert!(uri_to_path("https://example.com/x.rs").is_none());
    }

    fn lsp_diagnostic() -> lsp_types::Diagnostic {
        lsp_types::Diagnostic {
            range: lsp_types::Range {
                start: lsp_types::Position {
                    line: 11,
                    character: 4,
                },
                end: lsp_types::Position {
                    line: 11,
                    character: 9,
                },
            },
            severity: Some(lsp_types::DiagnosticSeverity::ERROR),
            code: Some(lsp_types::NumberOrString::String("E0308".into())),
            source: Some("rust-analyzer".into()),
            message: "mismatched types".into(),
            ..Default::default()
        }
    }

    #[test]
    fn zero_based_lsp_positions_become_one_based_cide_ones() {
        let d = diagnostic(
            &lsp_diagnostic(),
            "/repo/src/a.rs",
            "src/a.rs",
            "rust-analyzer",
        );
        assert_eq!((d.line, d.column), (12, 5));
        assert_eq!((d.end_line, d.end_column), (12, 10));
    }

    #[test]
    fn a_numeric_code_becomes_a_string_rather_than_being_dropped() {
        // gopls sends numbers for some codes. Dropping them would lose the only stable identifier
        // a user can search for.
        let mut lsp = lsp_diagnostic();
        lsp.code = Some(lsp_types::NumberOrString::Number(1006));
        let d = diagnostic(&lsp, "/a", "a", "gopls");
        assert_eq!(d.code.as_deref(), Some("1006"));
    }

    #[test]
    fn a_sub_tools_own_name_survives_so_it_can_be_filtered() {
        // rust-analyzer reports clippy under `source: "clippy"`, and a user who turns clippy off
        // must be able to. Overwriting it with the server's name would make that impossible.
        let mut lsp = lsp_diagnostic();
        lsp.source = Some("clippy".into());
        assert_eq!(
            diagnostic(&lsp, "/a", "a", "rust-analyzer").source,
            "clippy"
        );
    }

    #[test]
    fn a_diagnostic_with_no_source_is_attributed_to_the_server_that_sent_it() {
        // Never left empty: `source` is a filter axis, and an unattributable diagnostic could not
        // be turned off.
        let mut lsp = lsp_diagnostic();
        lsp.source = None;
        assert_eq!(diagnostic(&lsp, "/a", "a", "gopls").source, "gopls");
    }

    #[test]
    fn a_missing_severity_is_an_error_rather_than_a_hint() {
        let mut lsp = lsp_diagnostic();
        lsp.severity = None;
        assert_eq!(diagnostic(&lsp, "/a", "a", "x").severity, Severity::Error);
    }

    #[test]
    fn every_lsp_severity_maps_onto_one_of_ours() {
        use lsp_types::DiagnosticSeverity as S;
        for (from, want) in [
            (S::ERROR, Severity::Error),
            (S::WARNING, Severity::Warning),
            (S::INFORMATION, Severity::Info),
            (S::HINT, Severity::Hint),
        ] {
            let mut lsp = lsp_diagnostic();
            lsp.severity = Some(from);
            assert_eq!(diagnostic(&lsp, "/a", "a", "x").severity, want, "{from:?}");
        }
    }
}

/// One `Location`, whole, in cide's units: 1-based lines, 1-based UTF-16 columns.
///
/// The end is carried because two callers need it and neither can reconstruct it. `probe` compares
/// the caret against the returned range to decide *"is the caret already on the declaration"*, and
/// a usage row selects the occurrence it sends you to rather than poking a bare caret at it. The
/// end used to be thrown away here — see [`location`], which still does, deliberately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loc {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

/// One `Location` value — not an array — in cide's units.
///
/// Every dead end is a `None`: a missing `uri`, a URI naming no local file, a missing `range` (what
/// a `LocationLink` looks like from here), or a number that does not fit a `u32`.
fn one_location(one: &serde_json::Value) -> Option<Loc> {
    let path = uri_to_path(one.get("uri")?.as_str()?)?;
    let range = one.get("range")?;
    // 0-based on the wire, 1-based everywhere in cide. The column stays a UTF-16 code-unit offset
    // for the reason the module header gives — the consumer is CodeMirror.
    let point = |at: &serde_json::Value| -> Option<(u32, u32)> {
        let line = u32::try_from(at.get("line")?.as_u64()?)
            .ok()?
            .wrapping_add(1);
        let column = u32::try_from(at.get("character")?.as_u64()?)
            .ok()?
            .wrapping_add(1);
        Some((line, column))
    };
    let (line, column) = point(range.get("start")?)?;
    let (end_line, end_column) = point(range.get("end")?)?;
    Some(Loc {
        path,
        line,
        column,
        end_line,
        end_column,
    })
}

/// One `Location` from a `textDocument/definition` reply, in cide's units.
///
/// # Why this parses `Value` rather than taking an `lsp_types::Location`
///
/// The reply is `Location | Location[] | null` — three shapes behind one field, which `lsp_types`
/// models with an untagged enum that reports "data did not match any variant" and names neither
/// the field nor the shape it saw. A definition that silently fails to parse is the exact failure
/// this feature is supposed to end, so the shapes are picked apart by hand and each dead end is a
/// distinguishable `None` at a known line.
///
/// The client declares `definition: { linkSupport: false }`, so a conforming server may not send
/// `LocationLink`. This reads `range` only and does not look at `targetSelectionRange`; if that
/// capability is ever flipped, this function is the second place that has to change, and
/// `the_handshake_declares_what_the_client_actually_does` is the test that says so.
///
/// Deliberately still `(path, line, column)` and not a [`Loc`]. Go to definition puts a *bare
/// caret* on the target and says why (`goToDefinition.ts`: the meaning of a definition's `range.end`
/// varies by server — rust-analyzer sends the whole item for some kinds and the name for others),
/// so widening this return would hand every caller an end that only one of them may believe.
pub fn location(value: &serde_json::Value) -> Option<(PathBuf, u32, u32)> {
    let one = first_location(value)?;
    Some((one.path, one.line, one.column))
}

/// The first `Location` of a `Location | Location[] | null` reply, end included.
///
/// A server may offer several (a trait method with many impls); picking the first is what an editor
/// with no disambiguation UI can honestly do, and it is what the caller's comment about "one
/// answer" depends on.
pub fn first_location(value: &serde_json::Value) -> Option<Loc> {
    let one = if value.is_array() {
        value.as_array()?.first()?
    } else {
        value
    };
    one_location(one)
}

/// Every `Location` of a `textDocument/references` reply.
///
/// # Why the failures are per-location here and whole-reply in [`location`]
///
/// A definition has one answer, so a shape we cannot read means "no answer" and the honest result
/// is `None`. A references reply is a *list*, and rust-analyzer routinely mixes `rust-analyzer://`
/// targets (a macro expansion, a std item with no local source) into a list of perfectly ordinary
/// files. Failing the batch on one of those would turn "41 usages, one of them in a place you
/// cannot open" into "no usages", which is the confident-empty-list failure the whole diagnostics
/// surface is built to avoid. So each entry is dropped on its own and the rest ship.
///
/// `null` and `[]` are **not** the same and the caller must keep them apart: `null` is "there is no
/// symbol under the caret", `[]` is "this symbol is used nowhere". `None` here means the first;
/// `Some(vec![])` means the second.
pub fn locations(value: &serde_json::Value) -> Option<Vec<Loc>> {
    if value.is_null() {
        return None;
    }
    let array = value.as_array()?;
    Some(array.iter().filter_map(one_location).collect())
}

#[cfg(test)]
mod location_tests {
    use super::*;
    use serde_json::json;

    fn loc(line: u64, character: u64) -> serde_json::Value {
        json!({
            "uri": "file:///tmp/a.rs",
            "range": { "start": { "line": line, "character": character },
                       "end":   { "line": line, "character": character + 4 } },
        })
    }

    #[test]
    fn a_single_location_converts_to_one_based() {
        let (path, line, column) = location(&loc(41, 8)).expect("parsed");
        assert_eq!(path, PathBuf::from("/tmp/a.rs"));
        assert_eq!((line, column), (42, 9));
    }

    #[test]
    fn an_array_takes_the_first() {
        let value = json!([loc(0, 0), loc(99, 0)]);
        assert_eq!(location(&value).expect("parsed").1, 1);
    }

    #[test]
    fn null_and_an_empty_array_are_both_no_answer() {
        // The two ways a server says "I could not resolve this", and they must not be
        // distinguishable to the caller — both become `NotFound`, never a jump to line 1.
        assert!(location(&json!(null)).is_none());
        assert!(location(&json!([])).is_none());
    }

    #[test]
    fn a_location_link_reply_is_refused_rather_than_landing_on_line_one() {
        // What a server would send if `linkSupport` were ever flipped to true. It has no `range`,
        // so this must be `None` — the alternative is a confident jump to the wrong place.
        let link = json!([{
            "targetUri": "file:///tmp/a.rs",
            "targetRange": { "start": { "line": 4, "character": 0 },
                             "end": { "line": 4, "character": 1 } },
            "targetSelectionRange": { "start": { "line": 4, "character": 3 },
                                      "end": { "line": 4, "character": 7 } },
        }]);
        assert!(location(&link).is_none());
    }

    #[test]
    fn a_non_file_uri_is_refused() {
        // rust-analyzer answers with `rust-analyzer://` URIs for items in the standard library
        // when the source is not available locally. There is no path to open, and inventing one
        // would be a jump into a file that does not exist.
        let value = json!({
            "uri": "rust-analyzer:///std/vec.rs",
            "range": { "start": { "line": 1, "character": 1 },
                       "end": { "line": 1, "character": 2 } },
        });
        assert!(location(&value).is_none());
    }

    #[test]
    fn the_first_location_carries_the_end_the_triple_throws_away() {
        // What the declaration-or-reference discriminator runs on. Without the end there is no
        // containment test and the whole gesture falls back to guessing.
        let one = first_location(&loc(41, 8)).expect("parsed");
        assert_eq!((one.line, one.column), (42, 9));
        assert_eq!((one.end_line, one.end_column), (42, 13));
    }

    #[test]
    fn a_references_reply_of_null_is_not_an_empty_list() {
        // The distinction the popup's two sentences are built on. `null` is "there is no symbol
        // under the caret"; `[]` is "this symbol is used nowhere". Collapsing them tells a user
        // who clicked a keyword that their function is unused.
        assert!(locations(&json!(null)).is_none());
        assert_eq!(locations(&json!([])), Some(Vec::new()));
    }

    #[test]
    fn one_unreadable_location_does_not_lose_the_whole_list() {
        // rust-analyzer mixes `rust-analyzer://` targets into an otherwise ordinary list when a
        // usage sits inside a macro expansion. Failing the batch would turn "41 usages" into "no
        // usages", which is the confident-empty-list failure this crate exists to avoid.
        let value = json!([
            loc(0, 0),
            { "uri": "rust-analyzer:///std/vec.rs",
              "range": { "start": { "line": 1, "character": 1 },
                         "end": { "line": 1, "character": 2 } } },
            loc(9, 4),
        ]);
        let rows = locations(&value).expect("a list");
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[1].line, 10);
    }

    #[test]
    fn a_references_reply_that_is_not_a_list_at_all_is_no_answer() {
        // A server answering with a bare object where the spec says `Location[] | null`. Reading
        // it as an empty list would print "used nowhere" about a reply we did not understand.
        assert!(locations(&loc(1, 1)).is_none());
    }
}
