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
        // Never stale at the moment of conversion — this *is* the server speaking, which is the
        // event that answers staleness. The flag is owned by `cide_core::diagnostics`, which is
        // the only layer that knows what has happened to the file since; see its `dirty` map.
        stale: false,
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

// ==========================================================================================
// Completion. (M25)
// ==========================================================================================

/// cide's cap on one completion reply.
///
/// A `Ctrl+Space` on an empty line in a large workspace legitimately offers thousands of items —
/// rust-analyzer will name every public item of every dependency — and every one of them crosses
/// a Tauri IPC channel as JSON. The cap is high because it must not bite in the case that
/// matters: a list narrowed by even one typed character is two orders of magnitude smaller than
/// this, so the popup the user is actually reading is never truncated.
///
/// Truncating is reported rather than hidden ([`cide_ipc::CompletionAnswer::Items::truncated`]),
/// and it forbids the client's *"further typing filters this list locally"* fast path — the same
/// discipline `MAX_USAGES` follows one crate over, for the same reason: a capped list that
/// claims to be complete is worse than a slow one.
pub const MAX_COMPLETIONS: usize = 1500;

/// A `textDocument/completion` reply in cide's shapes.
pub struct CompletionReply {
    pub items: Vec<cide_ipc::CompletionItem>,
    /// The raw items that [`cide_ipc::CompletionItem::resolve`] indexes name, in that order.
    ///
    /// # Why the originals are kept, and only some of them
    ///
    /// `completionItem/resolve` takes **the item back**, whole — `data`, `label`, `sortText` and
    /// all — because that opaque `data` is how a server finds its way back to what it was about
    /// to compute. cide's converted DTO has thrown most of that away by design, so the original
    /// has to survive somewhere until the user accepts a row.
    ///
    /// Only the rows that actually have something deferred are kept. rust-analyzer attaches
    /// `data` to a minority of a list — the auto-import candidates — so this is typically a small
    /// fraction of `items`, which is what makes holding a couple of replies in memory
    /// unremarkable rather than a leak with a nice name.
    pub resolvable: Vec<serde_json::Value>,
    /// LSP's `isIncomplete`. Absent means `false` — a bare array reply is complete by definition.
    pub incomplete: bool,
    /// [`MAX_COMPLETIONS`] was hit.
    pub truncated: bool,
}

/// A `textDocument/completion` reply, converted.
///
/// # The three shapes, and why they are picked apart by hand
///
/// `CompletionItem[] | CompletionList | null`, which is `location`'s problem again and gets
/// `location`'s answer: `lsp_types` models it with an untagged enum whose failure message names
/// neither the field nor the shape it saw, and a completion list that silently fails to parse is
/// an empty popup that looks exactly like "there is nothing here". Both live servers exercise
/// different branches — gopls answers with a `CompletionList`, rust-analyzer with a bare array —
/// so neither is hypothetical and `real_servers.rs` drives both.
///
/// # Per-item failures drop the item, not the batch
///
/// [`locations`]' rule rather than [`location`]'s, and the argument is stronger here: a reply is a
/// *list*, one malformed entry among nine hundred good ones is a server bug in one row, and
/// failing the batch would turn "nine hundred completions" into "no completions" — the
/// confident-empty-list failure this whole surface is built to avoid. The only per-item
/// requirement is a non-empty `label`, because that is the one field with no fallback.
///
/// # `documentation` is dropped here
///
/// Deliberately, at the point of conversion rather than at the DTO — see
/// [`cide_ipc::CompletionItem`]. It is markdown, it is often a whole doc comment, no surface
/// draws it in this milestone, and a thousand of them is the difference between a payload that
/// crosses the IPC channel in a frame and one that does not.
pub fn completion(value: &serde_json::Value) -> Option<CompletionReply> {
    if value.is_null() {
        return None;
    }
    let (raw, incomplete) = if let Some(array) = value.as_array() {
        (array.as_slice(), false)
    } else {
        (
            value.get("items")?.as_array()?.as_slice(),
            value
                .get("isIncomplete")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        )
    };

    let truncated = raw.len() > MAX_COMPLETIONS;

    /*
     * Sorted before the cap, and that ordering is the whole point of doing it here.
     *
     * `sortText` is a *sort key*, not a score — `"ffff0000"` says nothing on its own — so the
     * only useful thing to hand the client is a rank, and a rank is only meaningful once the
     * list is in order. Capping first would keep the thousand items the server happened to emit
     * first rather than the thousand it considers best, which for rust-analyzer is close to the
     * opposite of what the user wants.
     *
     * `sort_by` and not `sort_unstable_by`: LSP's tie-break for equal `sortText` is the server's
     * own order, and a stable sort is what preserves it.
     */
    let mut ordered: Vec<&serde_json::Value> = raw.iter().collect();
    ordered.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));

    let mut items = Vec::new();
    let mut resolvable: Vec<serde_json::Value> = Vec::new();
    for (rank, raw) in ordered.into_iter().take(MAX_COMPLETIONS).enumerate() {
        let Some(mut item) = one_completion(raw, rank) else {
            continue;
        };
        // The index is assigned here rather than inside `one_completion`, because it names a
        // position in a list that function cannot see — and because a row dropped by the filter
        // above must not consume an index, or every later row's handle would point one item off.
        if needs_resolve(raw, &item) {
            item.resolve = Some(u32::try_from(resolvable.len()).unwrap_or(u32::MAX));
            resolvable.push(raw.clone());
        }
        items.push(item);
    }

    Some(CompletionReply {
        items,
        resolvable,
        incomplete,
        truncated,
    })
}

/// Does this row have something worth a `completionItem/resolve` before it is accepted?
///
/// Two conditions, and both are needed:
///
/// * **nothing already attached.** A server that computed its edits eagerly has answered the
///   question; asking again would put a round trip in front of a keystroke to learn nothing.
/// * **the server left a `data` blob.** That is what a server attaches when it has deferred
///   something, and it is the handle it needs to find its way back. Resolving an item without
///   one is legal and, for both servers cide ships, pointless.
///
/// The second condition is the one with a trade in it, so it is worth naming: a hypothetical
/// server that defers `additionalTextEdits` and attaches no `data` would have its import edit
/// missed. That server would also be answering `resolve` from the label alone, which no server
/// cide has met does. The alternative — resolving every accepted row — puts a language server
/// between the user and the Tab key for the majority of accepts that have nothing to fetch, and
/// that cost is paid on every single completion rather than in an imagined one.
fn needs_resolve(raw: &serde_json::Value, item: &cide_ipc::CompletionItem) -> bool {
    item.extra_edits.is_empty() && raw.get("data").is_some()
}

/// What this item sorts by: `sortText`, or the label when it sent none.
///
/// The fallback is the spec's — *"when omitted the label is used"* — and it is not optional. A
/// server that sets `sortText` on some items and not others (gopls does) would otherwise sort
/// every unset one into one bucket, which scrambles exactly the rows it left unmarked because it
/// had no opinion about them.
fn sort_key(item: &serde_json::Value) -> (&str, &str) {
    let label = item
        .get("label")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let sort = item
        .get("sortText")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(label);
    // The label is the tie-break, so two items with one `sortText` keep a deterministic order
    // across runs rather than inheriting the server's hash iteration order.
    (sort, label)
}

/// One item. `None` drops just this row — see [`completion`].
fn one_completion(item: &serde_json::Value, rank: usize) -> Option<cide_ipc::CompletionItem> {
    let str_at = |key: &str| item.get(key).and_then(serde_json::Value::as_str);

    let label = str_at("label")?.trim();
    if label.is_empty() {
        return None;
    }

    /*
     * The insert text, resolved once, from LSP's three sources in the spec's own order of
     * precedence: `textEdit.newText` beats `insertText` beats `label`.
     *
     * Doing this here rather than in the webview is not tidiness. The precedence is a protocol
     * rule, the fallback to `label` is the case that keeps a minimal server usable at all, and a
     * client that got the order wrong would insert `push(…)` — the *display* label, ellipsis
     * included — into the user's source. That failure is invisible in review and obvious in a
     * file, which is the shape of bug this crate converts eagerly to avoid.
     */
    let edit = item.get("textEdit");
    let replace = edit.and_then(text_edit);
    let insert = match edit
        .and_then(|e| e.get("newText"))
        .and_then(serde_json::Value::as_str)
    {
        Some(text) => text,
        None => str_at("insertText").unwrap_or(label),
    };

    /*
     * An `InsertReplaceEdit` is refused, and refusing it is the point.
     *
     * cide declares `insertReplaceSupport: false`, so a conforming server may not send one — it
     * carries `insert` and `replace` ranges where a `TextEdit` carries a single `range`. Reading
     * `newText` off it and then finding no `range` would leave `replace: None`, and the client
     * would splice the text over whatever *it* thinks the current word is, at an offset the
     * server never named. Dropping the row instead is visible (an item that is missing) rather
     * than silent (an item that corrupts a line), and `session.rs` says this is the second of the
     * two places that change if the capability is ever flipped on.
     */
    if let Some(edit) = edit
        && edit.get("range").is_none()
        && edit.get("insert").is_some()
    {
        return None;
    }

    let snippet = item
        .get("insertTextFormat")
        .and_then(serde_json::Value::as_u64)
        == Some(2);

    // Both spellings, because the field was deprecated in favour of the tag in LSP 3.15 and
    // servers still send either — see the handshake's `deprecatedSupport` note.
    let deprecated = item
        .get("deprecated")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        || item
            .get("tags")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|tags| {
                tags.iter()
                    .filter_map(serde_json::Value::as_u64)
                    .any(|tag| tag == 1)
            });

    Some(cide_ipc::CompletionItem {
        // The label is what the popup draws; `filterText` is what typing is matched against, and
        // for rust-analyzer they routinely differ (`push(…)` against `push`). Collapsing them is
        // what makes a popup stop narrowing as the user types.
        filter_text: str_at("filterText").unwrap_or(label).to_string(),
        label: label.to_string(),
        // `labelDetails.detail` and nothing else: it is the string drawn *against* the label, and
        // for an auto-import row it is the only visible warning that accepting also edits the top
        // of the file. Falling back to `detail` here would put a type in that position, which is
        // where the popup draws a sentence.
        detail: item
            .get("labelDetails")
            .and_then(|d| d.get("detail"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        // The type, from either spelling. `labelDetails.description` is where a modern server
        // puts it once the client declares `labelDetailsSupport`; `detail` is where every server
        // has always put it, and is the fallback that keeps a minimal one's popup readable.
        description: item
            .get("labelDetails")
            .and_then(|d| d.get("description"))
            .and_then(serde_json::Value::as_str)
            .or_else(|| str_at("detail"))
            .map(str::to_string),
        kind: completion_kind(item.get("kind").and_then(serde_json::Value::as_u64)),
        insert: insert.to_string(),
        snippet,
        replace,
        // `usize` → `u32`: unreachable past `MAX_COMPLETIONS`, and saturating rather than
        // wrapping so that a future cap raised past `u32::MAX` degrades to "ranked last"
        // instead of to "ranked first".
        sort: u32::try_from(rank).unwrap_or(u32::MAX),
        extra_edits: item
            .get("additionalTextEdits")
            .and_then(serde_json::Value::as_array)
            .map(|edits| edits.iter().filter_map(text_edit).collect())
            .unwrap_or_default(),
        // Filled in by `completion`, which is the only place that knows the index — see there.
        resolve: None,
        deprecated,
    })
}

/// The `additionalTextEdits` of a `completionItem/resolve` reply.
///
/// `None` only for a reply that is not an object at all. An object with no `additionalTextEdits`
/// is `Some(vec![])` and means what it says: the server resolved the item and it needs no extra
/// edits. Collapsing the two would make "the server said nothing to add" indistinguishable from
/// "the reply was unreadable", and only the first is safe to accept on.
pub fn resolved_edits(value: &serde_json::Value) -> Option<Vec<cide_ipc::CompletionEdit>> {
    if !value.is_object() {
        return None;
    }
    Some(
        value
            .get("additionalTextEdits")
            .and_then(serde_json::Value::as_array)
            .map(|edits| edits.iter().filter_map(text_edit).collect())
            .unwrap_or_default(),
    )
}

/// One LSP `TextEdit` in cide's units. `None` for any shape that is not one.
fn text_edit(edit: &serde_json::Value) -> Option<cide_ipc::CompletionEdit> {
    let range = edit.get("range")?;
    // 0-based on the wire, 1-based in cide; the column stays UTF-16 because the consumer is
    // CodeMirror. The same conversion `one_location` makes, and it must stay the same one — an
    // import edit landing a line off is a `use` statement inside a function body.
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
    Some(cide_ipc::CompletionEdit {
        line,
        column,
        end_line,
        end_column,
        // Absent `newText` is an empty string, not a failure: a pure deletion is a legal edit and
        // gopls emits them when it rewrites an import block.
        text: edit
            .get("newText")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

/// LSP's `CompletionItemKind` number into cide's badge vocabulary.
///
/// The merges are stated once, here, rather than in the renderer — see
/// [`cide_ipc::CompletionKind`] for why the two enums are not one. An unknown number becomes
/// `Other` rather than dropping the row: the spec grows, and a completion you cannot label is
/// still a completion.
fn completion_kind(kind: Option<u64>) -> cide_ipc::CompletionKind {
    use cide_ipc::CompletionKind as K;
    match kind {
        Some(1) => K::Text,
        Some(2) => K::Method,
        Some(3) => K::Function,
        Some(4) => K::Constructor,
        Some(5) => K::Field,
        Some(6) => K::Variable,
        // `Class` and `Struct` are one badge: Rust has only the second, Go only the first, and no
        // language cide targets shows both in one list.
        Some(7) | Some(22) => K::Struct,
        Some(8) => K::Interface,
        Some(9) => K::Module,
        Some(10) => K::Property,
        // `Unit` — a measurement suffix in a stylesheet. No cide language emits one, and it has
        // no badge worth inventing.
        Some(11) => K::Other,
        // `Value` and `Constant` are indistinguishable to a reader of a popup.
        Some(12) | Some(21) => K::Constant,
        Some(13) => K::Enum,
        Some(14) => K::Keyword,
        Some(15) => K::Snippet,
        // `Color` — a swatch this popup does not draw.
        Some(16) => K::Other,
        Some(17) => K::File,
        // `Reference` is "a variable, indirectly" everywhere it is actually used.
        Some(18) => K::Variable,
        Some(19) => K::Folder,
        Some(20) => K::EnumMember,
        Some(23) => K::Event,
        Some(24) => K::Operator,
        Some(25) => K::TypeParameter,
        _ => K::Other,
    }
}
