//! What a path *is*, as the properties card asks it. (M70)
//!
//! > *"Need a file properties modal window that will show OS stats and git history if
//! > applicable and other useful stuff"*
//!
//! cide could already show a file's **contents** five ways — editor, image pane, drawing pane,
//! diff, blame — and almost nothing **about** the file. The facts were not missing so much as
//! scattered and thrown away: the status bar knows the language and the line ending, `ImagePane`
//! knows the pixel dimensions, the file tree knows a status letter, the Log tab knows the
//! history, and `document::read` reads the size and the mtime on every single open and keeps
//! neither. Three more — the mode bits, the owner, and the difference between a symlink and the
//! thing it points at — were read nowhere in the workspace at all.
//!
//! This module is the shape of the answer. It is three DTOs rather than one because the card is
//! filled by three commands, and *that* split is the load-bearing decision:
//!
//! - [`FileProperties`] is one `symlink_metadata` and at most one bounded byte scan. It is what
//!   makes the card open instantly.
//! - [`DirSummary`] is a recursive walk with a budget, asked only for a directory.
//! - [`FilePropertiesGit`] is a bounded revision walk, asked only inside a repository.
//!
//! A single command returning all three would make every properties card wait on the slowest
//! half of itself, which for a directory the size of `target/` is not a wait anybody would sit
//! through for a size they did not ask for.
//!
//! # Why the times are milliseconds and not a [`crate::FileStamp`]
//!
//! There is already a timestamp on the wire for a file, and reusing it here would be wrong.
//! [`crate::FileStamp::mtime_nanos`] is nanoseconds since the epoch — about 1.79e18 today,
//! roughly two hundred times `Number.MAX_SAFE_INTEGER` — and it is carried as a **decimal
//! string** for exactly that reason. Its doc comment records what happened when that was not
//! understood: the stamp was silently rounded to a multiple of 256 ns in the webview, the token
//! handed back never equalled the file's real stamp, and every autosave on ext4, btrfs and xfs
//! was refused as a conflict about a file nothing had touched.
//!
//! A stamp is an opaque token the frontend hands back. A properties card does the one thing that
//! token must never be used for: it **formats** the value. So these fields are milliseconds
//! (~1.7e12, comfortably exact as a JavaScript number) and nothing in this feature parses a
//! `FileStamp` string.
//!
//! # And why they are `#[ts(type = "number")]`
//!
//! ts-rs renders an `i64` as a TypeScript **`bigint`**, which is a claim about the *wire* that is
//! not true: Tauri's transport is JSON, serde writes an `i64` as a JSON number, and `JSON.parse`
//! produces a `number`. `ui/src/gitlog/logModel.ts` copes with that mismatch by calling
//! `Number()` at one narrowing point, and explains there why it is lossless.
//!
//! Coping is right for `CommitRow::authored`, which is a general timestamp with no stated range.
//! Here the range is **pinned** — `a_millisecond_time_survives_a_javascript_number` asserts these
//! stay exactly representable past the year 5138 — so the annotation is simply the honest
//! description. A `bigint` in the type would force every reader through a narrowing cast that can
//! only ever succeed, and the first one to forget it gets `NaN` in a date field with nothing
//! logged anywhere.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::RepoId;
use crate::history::CommitRow;

/// What the path turned out to be, decided by `symlink_metadata` and never by the name.
///
/// [`PathKind::Symlink`] wins over the other two: a link to a directory is reported as a link,
/// and [`FileProperties::symlink_target`] says where it goes. The alternative — following first
/// and reporting the destination's kind — is the bug this whole enum exists to prevent, and it
/// is spelled out on [`FileProperties`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PathKind {
    File,
    Dir,
    Symlink,
    /// A socket, fifo, block or character device. Named rather than hidden, because a properties
    /// card that silently renders a fifo as an empty file is a card that answered the wrong
    /// question confidently.
    Other,
}

/// How a text file's lines end, as counted from its bytes.
///
/// The same four answers `ui/src/editor/lineEndings.ts` gives, and deliberately the same rule:
/// whichever of the three break shapes occurs, with [`LineEnding::Mixed`] when more than one
/// does. Two producers for one fact would let the status bar and the properties card disagree
/// about the file six inches apart on the same screen, and the one that is wrong is not
/// predictable from the outside.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LineEnding {
    Lf,
    Crlf,
    Cr,
    Mixed,
    /// No line break anywhere in the file — including the empty file. Distinct from `Lf` on
    /// purpose: claiming an ending a file does not have is a guess, and this row is read by
    /// people deciding whether a tool mangled a checkout.
    None,
}

/// Who owns the path, in whatever detail this platform can answer.
///
/// The numbers are always present on unix and the names are best-effort: a uid with no passwd
/// entry is the normal case inside a container, and the row still has to render. The frontend's
/// rule is *name when there is one, number always* — `ivan (1000)` — so a missing name costs a
/// word and never the fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Owner {
    pub uid: u32,
    pub gid: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub group: Option<String>,
}

/// What one pass over a text file's bytes found.
///
/// `None` on [`FileProperties::text`] is not silence — [`FileProperties::text_skipped`] carries
/// the sentence saying which of the three reasons applied. An omitted row tells the reader
/// nothing they can act on; "too large to count (over 32 MiB)" tells them the file is fine and
/// the card is the thing with a limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TextFacts {
    /// Lines as a text editor counts them: one more than the number of breaks, and therefore
    /// never zero for a non-empty file. A file ending in a newline has a final empty line, which
    /// is what CodeMirror shows and what `ui/src/editor/lineEndings.ts::countLines` returns.
    ///
    /// `number`, on the same reasoning as the times above and with a tighter bound: a line needs
    /// at least one byte and `document::MAX_FILE_BYTES` is 32 MiB, so this cannot exceed ~33.5
    /// million however pathological the file.
    #[ts(type = "number")]
    pub lines: u64,
    pub ending: LineEnding,
    /// The bytes are valid UTF-8. False is reachable and is worth showing: `document::read`
    /// **refuses** a non-UTF-8 file rather than re-decoding it, so this row is the only place in
    /// cide that explains why a file will not open.
    pub utf8: bool,
}

/// Everything one `symlink_metadata` and one bounded scan can say about a path. (M70)
///
/// # `symlink_metadata`, never `metadata`
///
/// [`crate::FileStamp`]'s producer, `cide_core::document::stamp_at`, calls `fs::canonicalize`
/// first — correct there, because a buffer's identity is the file it really edits — and it makes
/// that function unusable here. A properties card built on it would show, under a symlink's own
/// name and path, the size, mode, owner and mtime **of something else**, with nothing on screen
/// to suggest the substitution. So this reads the link itself, reports `PathKind::Symlink`, and
/// resolves the destination only into [`Self::symlink_target`], which is a row the reader can
/// see and follow.
///
/// # Fields that are `None` off unix
///
/// `mode`, `mode_string`, `owner` and `changed_unix_ms` have no Windows equivalent and are
/// `None` there rather than faked. `docs/platforms.md` records it. Each is paired with
/// `skip_serializing_if` beside its `#[ts(optional)]`: on its own, `#[ts(optional)]` changes
/// only the *emitted TypeScript type* and not what serde writes, so the field arrives as `null`
/// while the frontend believes it is `undefined` — `=== undefined` is false, the next property
/// access throws, and that is precisely how the Docker panel died on
/// `null is not an object`. The pairing is a rule in this crate, not a preference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileProperties {
    /// The path as it was asked about — **not** canonicalised, so the card's heading matches the
    /// row that was right-clicked rather than resolving to somewhere the user never named.
    pub path: PathBuf,
    /// The last component. Computed here so the card and the tree cannot disagree about what a
    /// path with a trailing slash, or a root, is called.
    pub name: String,
    pub kind: PathKind,
    /// Bytes, as the *link* reports for a symlink. A directory's own entry size is reported here
    /// too and is not its contents — that is [`DirSummary::bytes`], which costs a walk.
    #[ts(type = "number")]
    pub len: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "number")]
    pub modified_unix_ms: Option<i64>,
    /// Inode change time. Unix only, and a different fact from `modified`: a `chmod` moves this
    /// and not that, which is the question somebody reading a permissions row usually has.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "number")]
    pub changed_unix_ms: Option<i64>,
    /// Birth time, where the filesystem records one. ext4 does, many do not, and `None` means
    /// *not recorded* rather than *unknown* — so the row is absent instead of showing the epoch.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "number")]
    pub created_unix_ms: Option<i64>,
    /// Exactly `Permissions::readonly()`, with `FileDoc::writable`'s caveat: it reads the mode
    /// and nothing else, so a file owned by somebody else at 0644, or any file on a read-only
    /// mount, still reads as writable here.
    pub readonly: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mode: Option<u32>,
    /// `-rw-r--r--`, rendered in Rust so there is **one producer**. The frontend could format it
    /// from `mode` and then there would be two, which for a ten-character string with setuid,
    /// setgid and sticky corner cases is two answers that agree until they do not.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mode_string: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub owner: Option<Owner>,
    /// Where a [`PathKind::Symlink`] points, verbatim — relative if it was written relative,
    /// because that is what the link says and resolving it would hide a broken one.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub symlink_target: Option<PathBuf>,
    /// Whether the target of a symlink exists. A dangling link is worth naming on the card; it
    /// is the state that makes every other row about it confusing.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub symlink_broken: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub text: Option<TextFacts>,
    /// Why [`Self::text`] is `None`, as a sentence for the card. See [`TextFacts`].
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub text_skipped: Option<String>,
}

/// What a directory contains, from a walk with a budget. (M70)
///
/// # The budget, and why `truncated` is not optional
///
/// `cide_fs::copy::preview_merge` already settled this argument for the paste dialog and the
/// reasoning transfers unchanged: a count that stops early is **honest** when it says so and
/// simply wrong when it does not. `.cide/worktrees/`, `target/` and `node_modules/` are all
/// ordinary things to right-click, and a walk with no budget on one of them is a properties card
/// that hangs.
///
/// So the walk stops at its limit, sets this flag, and the card renders *at least 3.4 MiB*. An
/// exact-looking number that is really a lower bound is worse than no number at all in a panel
/// whose only job is being believed — `PasteCollision::truncated`'s sentence, in a second place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DirSummary {
    #[ts(type = "number")]
    pub files: u64,
    #[ts(type = "number")]
    pub dirs: u64,
    /// Sum of the entries' own sizes. Not disk usage: no block rounding, no sparse-file
    /// correction, so this is `du --apparent-size -b` and not `du`. The card says "Size", which
    /// is the number people compare against a file manager.
    #[ts(type = "number")]
    pub bytes: u64,
    /// The walk hit its budget. Every count above is a lower bound.
    pub truncated: bool,
}

/// The git half of the card, for a path inside a repository. (M70)
///
/// `None` from the command means *this path is in no repository cide has open* — the card then
/// draws no Git block at all, rather than an empty one making a claim about a file git has never
/// heard of.
///
/// # Why this is not built from `git_log`
///
/// `git_log` keys its cancellation flag by `(project, tab: ToolTabId)` so that a superseded walk
/// stops within one commit instead of scanning to its budget for a page nobody will draw. A
/// modal has no tool tab. Handing it a borrowed id — the Log tab's, or a sentinel — would cancel
/// that tab's in-flight page every time somebody opened a properties card, which is the
/// `DOCKER_TAB` hazard in a new place. So the command walks `cide_git::log` directly, bounded
/// small enough that there is nothing worth cancelling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FilePropertiesGit {
    pub repo: RepoId,
    /// Repo-relative, slash-separated — resolved by `cide_git::repo::locate`, never by prefix
    /// arithmetic in TypeScript, because the innermost repository wins and a project root is not
    /// always a repository root.
    pub rel_path: String,
    /// The oldest commit touching this path: the card's *tracked since*. A separate walk from
    /// [`Self::recent`], and the expensive one — it cannot stop early, so it carries its own
    /// budget and comes back `None` when that runs out. `None` therefore means *not found
    /// within the budget*, which the card words as such rather than as "untracked".
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub first_commit: Option<CommitRow>,
    /// Whether [`Self::first_commit`] is `None` because the walk gave up rather than because
    /// there is no history. Without this the card cannot tell *never committed* from *too much
    /// history to say*, and those are opposite answers.
    pub first_truncated: bool,
    /// The newest commit touching this path. This is `recent[0]` and is repeated as its own
    /// field so the card's summary line has one producer whatever the list is doing.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_commit: Option<CommitRow>,
    /// Newest first, at most `RECENT_LIMIT`.
    pub recent: Vec<CommitRow>,
    /// There is more history than [`Self::recent`] holds — read from asking for one row more
    /// than is shown, never from `recent.len()` reaching the limit, which cannot tell a history
    /// of exactly ten from a history of eleven.
    pub more: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bare() -> FileProperties {
        FileProperties {
            path: PathBuf::from("/tmp/a.txt"),
            name: "a.txt".into(),
            kind: PathKind::File,
            len: 0,
            modified_unix_ms: None,
            changed_unix_ms: None,
            created_unix_ms: None,
            readonly: false,
            mode: None,
            mode_string: None,
            owner: None,
            symlink_target: None,
            symlink_broken: None,
            text: None,
            text_skipped: None,
        }
    }

    /// An absent optional is an **absent key**, not `null`. (M70)
    ///
    /// `cide_ipc::docker`'s rule, in a DTO with nine optionals. The bug it pins is worth
    /// restating because it is invisible from Rust: `#[ts(optional)]` changes the *emitted
    /// TypeScript* — `owner?: Owner` — and nothing at all about what serde writes. Without
    /// `skip_serializing_if` the wire carries `"owner": null` while the type promises the key is
    /// absent, `=== undefined` is false, and the next property access throws. That is exactly
    /// how the Docker panel died on `null is not an object`.
    ///
    /// Asserted over the whole struct rather than field by field, because the failure is
    /// per-field and a test naming `owner` alone passes while `text` is still wrong.
    #[test]
    fn an_absent_optional_is_an_absent_key_and_never_null() {
        let json = serde_json::to_string(&bare()).expect("serialise");
        assert!(
            !json.contains("null"),
            "no optional may reach the wire as null: {json}"
        );
        for key in [
            "modifiedUnixMs",
            "changedUnixMs",
            "createdUnixMs",
            "mode",
            "modeString",
            "owner",
            "symlinkTarget",
            "symlinkBroken",
            "text",
            "textSkipped",
        ] {
            assert!(
                !json.contains(&format!("\"{key}\"")),
                "`{key}` is absent when it has no value, not present-and-null: {json}"
            );
        }
    }

    /// The mirror, so the guard above cannot be satisfied by never writing the fields at all.
    #[test]
    fn a_present_optional_still_arrives() {
        let props = FileProperties {
            mode: Some(0o644),
            mode_string: Some("-rw-r--r--".into()),
            modified_unix_ms: Some(1_700_000_000_000),
            owner: Some(Owner {
                uid: 1000,
                gid: 100,
                user: Some("ivan".into()),
                // Nested, and absent: a present value must not drag its own optionals onto the
                // wire as nulls either.
                group: None,
            }),
            text: Some(TextFacts {
                lines: 3,
                ending: LineEnding::Lf,
                utf8: true,
            }),
            ..bare()
        };
        let json = serde_json::to_string(&props).expect("serialise");
        assert!(json.contains(r#""modeString":"-rw-r--r--""#), "{json}");
        assert!(json.contains(r#""user":"ivan""#), "{json}");
        assert!(json.contains(r#""ending":"lf""#), "{json}");
        assert!(!json.contains("\"group\""), "{json}");
        assert!(!json.contains("null"), "{json}");
    }

    /// A time that fits in a JavaScript number, which is the whole reason these are not
    /// [`crate::FileStamp`]s. (M70)
    ///
    /// `FileStamp::mtime_nanos` is ~1.79e18 and is carried as a decimal *string* because that is
    /// about two hundred times `Number.MAX_SAFE_INTEGER` — its doc comment records the autosave
    /// outage that followed from missing it. A properties card **formats** its times rather than
    /// handing them back as a token, so they are milliseconds, and this pins that they are still
    /// exactly representable a long way past today.
    #[test]
    fn a_millisecond_time_survives_a_javascript_number() {
        const MAX_SAFE: i64 = 9_007_199_254_740_991;
        // Year 5138, which is when a millisecond clock would start rounding.
        let far_future_ms = 100_000_000_000_000_i64;
        assert!(far_future_ms < MAX_SAFE);

        let json = serde_json::to_string(&FileProperties {
            modified_unix_ms: Some(far_future_ms),
            ..bare()
        })
        .expect("serialise");
        // A number on the wire, not a string: the frontend does arithmetic on it to format a
        // date, and a quoted value would silently become string concatenation.
        assert!(
            json.contains(&format!(r#""modifiedUnixMs":{far_future_ms}"#)),
            "{json}"
        );
    }

    /// `DirSummary::truncated` is not optional, and defaults to the honest answer.
    ///
    /// [`DirSummary`] argues the rest. The counts are lower bounds when this is set, and a
    /// `Default` that started life `true` would make every exact walk claim it gave up.
    #[test]
    fn a_fresh_dir_summary_claims_nothing_and_admits_nothing() {
        let s = DirSummary::default();
        assert_eq!((s.files, s.dirs, s.bytes), (0, 0, 0));
        assert!(!s.truncated);
        let json = serde_json::to_string(&s).expect("serialise");
        assert!(json.contains(r#""truncated":false"#), "{json}");
    }
}
