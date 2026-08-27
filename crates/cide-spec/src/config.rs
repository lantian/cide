//! Reading and **surgically editing** `openspec/config.yaml`. (M28)
//!
//! # Why there is no YAML crate here
//!
//! Two reasons, and the second is the one that decided it.
//!
//! The first is the crate graph: `cide-spec` depends on `cide-ipc`, `cide-core`, `serde`,
//! `serde_json` and `tracing`, and a YAML parser is a new third-party tree in a crate whose whole
//! point ([`crate`]'s header) is that it *adapts* an external tool rather than reimplementing it.
//!
//! The second is the file. `openspec init` writes 922 bytes of which roughly eight hundred are
//! **comments** — three commented example blocks that are the only documentation a user ever sees
//! for `context`, `rules` and `operations`:
//!
//! ```text
//! schema: spec-driven
//!
//! # Project context (optional)
//! # This is shown to AI when creating artifacts.
//! # Example:
//! #   context: |
//! #     Tech stack: TypeScript, React, Node.js
//! ```
//!
//! Every YAML library in the ecosystem is a *loader*: it produces a value, and serialising that
//! value back produces a file with no comments in it, keys in the serialiser's order, block
//! scalars re-folded and the user's blank-line style gone. So a cide that let somebody set a
//! project context from a settings panel would, on the first Save, silently delete the file's
//! documentation and rewrite a committed file into a diff touching every line. That is
//! [`crate::write`]'s argument (see its header) applied to a second file, and it lands harder
//! here because the deleted comments were *instructions*.
//!
//! # What this is instead
//!
//! The same shape as [`crate::block`]: a **line-oriented scanner** that answers one question —
//! *which lines belong to this key* — after which the editor replaces exactly those lines and
//! **copies every other byte**. Nothing is re-serialised. Comments, blank lines, key order,
//! indentation, CRLF and a byte-order mark all survive because nothing ever looks at them.
//!
//! The reader is deliberately smaller than YAML. It handles what `openspec init` writes plus what
//! a person reasonably hand-writes: top-level `key: value`, literal block scalars (`context: |`),
//! mappings nested two and three deep, and `- ` list items. It does **not** handle flow
//! collections spanning lines, anchors, aliases, tags, multi-document streams or complex keys.
//! Where it does not understand something it reads *nothing* rather than guessing — a missing key
//! is [`None`] or an empty list — and, crucially, a key it did not understand is a key it will
//! never rewrite. Tolerance here is not politeness; it is the difference between a panel that
//! shows a field blank and a panel that overwrites a file it misread.
//!
//! # The round-trip guarantee
//!
//! **Writing a value back unchanged changes no byte of the file, and writing a value back changed
//! changes only the lines that spell that value.**
//!
//! The first half is not achieved by hoping the renderer reproduces the input: it is achieved by
//! *not rendering at all*. Every setter parses the value the file currently states, compares it
//! to the value being set, and returns the lines untouched when they are equal — so a Save that
//! changes one rule cannot reflow the block scalar three keys above it, and a panel that writes
//! all four keys on every Save produces an empty `git diff`. [`apply`] then compares the whole
//! rendered text against what it read and does not write the file at all when they match, which
//! is why a no-op Save does not even move the mtime.
//!
//! The second half is what makes the first half safe to rely on: an edit is a splice into a line
//! range, so there is no code path on which a byte outside that range can change.

use std::path::{Path, PathBuf};

use cide_core::persist;

use crate::SpecError;

/// Where the config lives, relative to a project root.
pub const CONFIG_RELATIVE: &str = "openspec/config.yaml";

/// What `openspec init` writes when nothing says otherwise.
pub const DEFAULT_SCHEMA: &str = "spec-driven";

/// The artifacts the default `spec-driven` schema declares.
///
/// **A default and not a closed set.** [`crate`]'s header records why nothing in this crate may
/// assume the artifact list: a schema declares its own artifacts with `generates` globs, so a
/// project can have artifacts cide has never heard of, and `rules:` may perfectly legally name
/// one of them. This constant exists so a settings panel has something to *offer*; the reader and
/// the writer both take an artifact id as a string and neither one consults it.
pub const DEFAULT_ARTIFACTS: [&str; 4] = ["proposal", "design", "tasks", "specs"];

/// The two operations `operations:` gives guidance for.
pub const GUIDED_OPERATIONS: [&str; 2] = ["apply", "archive"];

/// The indentation one nesting level costs, when a level has to be created from nothing.
///
/// Only ever used for a key that is **absent**. An existing key keeps whatever indentation the
/// file already uses, read back off its own lines — a project that indents with four spaces must
/// not acquire a two-space island the first time somebody edits a rule from the panel.
const INDENT_STEP: usize = 2;

/// `<root>/openspec/config.yaml`.
pub fn config_path(root: &Path) -> PathBuf {
    root.join(CONFIG_RELATIVE)
}

/// A project's OpenSpec configuration, as far as this reader understands it.
///
/// Ordered `Vec`s rather than maps, and that is not laziness. Document order is what the file
/// says and what a panel should show; a `BTreeMap` would alphabetise `proposal`, `design`,
/// `tasks` into an order the user never chose, and a `HashMap` would pick a different one on
/// every launch. Nothing here is looked up often enough for the linear scan to matter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// The workflow schema id. [`None`] when the file does not state one, which means
    /// [`DEFAULT_SCHEMA`] — kept as `None` rather than filled in so a writer can tell "unstated"
    /// from "stated, and happens to be the default" and not add a line nobody asked for.
    pub schema: Option<String>,
    /// Free prose injected into every artifact-generation prompt. Newlines and all.
    pub context: Option<String>,
    /// Per-artifact rules, in the order the file states them.
    pub rules: Vec<ArtifactRules>,
    /// Per-operation guidance, in the order the file states them.
    pub operations: Vec<OperationGuidance>,
}

/// One artifact's rules.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactRules {
    pub artifact: String,
    pub rules: Vec<String>,
}

/// One operation's guidance.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OperationGuidance {
    pub operation: String,
    pub guidance: Vec<String>,
}

impl Config {
    /// The schema this project uses, with the default filled in.
    pub fn schema_or_default(&self) -> &str {
        self.schema.as_deref().unwrap_or(DEFAULT_SCHEMA)
    }

    /// The rules stated for one artifact, or nothing.
    pub fn rules_for(&self, artifact: &str) -> &[String] {
        self.rules
            .iter()
            .find(|entry| entry.artifact == artifact)
            .map_or(&[], |entry| entry.rules.as_slice())
    }

    /// The guidance stated for one operation, or nothing.
    pub fn guidance_for(&self, operation: &str) -> &[String] {
        self.operations
            .iter()
            .find(|entry| entry.operation == operation)
            .map_or(&[], |entry| entry.guidance.as_slice())
    }

    /// Does this file state anything at all beyond a schema?
    ///
    /// What a panel asks to decide between "configured" and "the file `init` wrote".
    pub fn is_bare(&self) -> bool {
        self.context.is_none() && self.rules.is_empty() && self.operations.is_empty()
    }
}

/// One change to make to the file.
///
/// An enum rather than four independent write calls because a settings panel Saves a *form*, and
/// four calls would be four reads, four validations and four atomic renames of one committed
/// file — four chances to lose a race with an agent, and up to four commits' worth of mtime
/// churn for one gesture. [`apply`] takes a slice and writes once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// Set `schema:`.
    Schema(String),
    /// Set `context:`, or remove it entirely.
    Context(Option<String>),
    /// Set one artifact's rules. An empty slice removes that artifact's entry, and removing the
    /// last entry removes the `rules:` key with it — `rules:` alone is YAML `null`, not an empty
    /// mapping, and leaving one behind would be cide inventing a key the user never wrote.
    Rules {
        artifact: String,
        rules: Vec<String>,
    },
    /// Set one operation's guidance. Empty removes it, by the same rule.
    OperationGuidance {
        operation: String,
        guidance: Vec<String>,
    },
}

/// Read a project's config.
///
/// A missing file is [`Config::default`] and **not** an error: "there is no config" and "there is
/// a config that states nothing" are the same thing to every caller, and a project whose
/// `openspec/` predates `config.yaml` is an ordinary project, not a broken one. A file that is
/// there and cannot be read *is* an error, and so is one that is not UTF-8 — the write path would
/// otherwise have to decide what to do with bytes it cannot represent, and the answer to that is
/// never "guess".
pub fn read(root: &Path) -> Result<Config, SpecError> {
    let path = config_path(root);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(error) => {
            return Err(SpecError::Write(format!(
                "{} could not be read: {error}",
                path.display()
            )));
        }
    };
    let text = String::from_utf8(bytes).map_err(|_| {
        SpecError::Write(format!(
            "{} is not UTF-8, so cide will neither read nor rewrite it",
            path.display()
        ))
    })?;
    Ok(read_text(&text))
}

/// Read a config out of text. The whole reader; [`read`] is this plus a file.
///
/// Public because it is the half that is worth testing without a filesystem, and because the
/// panel occasionally has the bytes already.
pub fn read_text(text: &str) -> Config {
    let shape = Shape::of(text);
    let normalised = shape.normalise(text);
    let doc = Document::of(&normalised);
    Config {
        schema: doc.scalar("schema"),
        context: doc.scalar("context"),
        rules: doc.rules(),
        operations: doc.operations(),
    }
}

/// Apply edits to a project's config, atomically, and say whether anything changed.
///
/// Returns `false` when the edits describe what the file already says — see the module header's
/// round-trip guarantee. Nothing is written in that case, so the mtime does not move and a
/// `git status` stays clean.
///
/// Written through [`persist::write_atomic_with_mode`] at [`persist::SHARED_MODE`] for the same
/// reason `.cide/tasks.json` and the delta files are: `openspec/` is committed and read in pull
/// requests, so it is 0o644 and it is never half-written.
pub fn apply(root: &Path, edits: &[Edit]) -> Result<bool, SpecError> {
    let path = config_path(root);
    let before = match std::fs::read(&path) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| {
            SpecError::Write(format!(
                "{} is not UTF-8, so cide will not rewrite it",
                path.display()
            ))
        })?,
        // A project with an `openspec/` and no `config.yaml`. Starting from empty text is right:
        // the edits append the keys they name and the file that results states exactly what the
        // user asked for and nothing else. Writing a scaffold with commented examples would be
        // cide impersonating `openspec init`, and the examples would go stale against a CLI this
        // crate deliberately does not track.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(SpecError::Write(format!(
                "{} could not be read: {error}",
                path.display()
            )));
        }
    };

    let after = edit_text(&before, edits).map_err(|error| match error {
        // Re-phrased with the path, because a refusal about a duplicate key is unactionable
        // without knowing which file holds it.
        SpecError::Write(sentence) => SpecError::Write(format!("{}: {sentence}", path.display())),
        other => other,
    })?;
    if after == before && (path.exists() || after.is_empty()) {
        return Ok(false);
    }

    persist::write_atomic_with_mode(&path, after.as_bytes(), persist::SHARED_MODE).map_err(
        |error| SpecError::Write(format!("{} could not be written: {error}", path.display())),
    )?;
    Ok(true)
}

/// Set `schema:`.
pub fn set_schema(root: &Path, schema: &str) -> Result<bool, SpecError> {
    apply(root, &[Edit::Schema(schema.to_string())])
}

/// Set `context:`, or remove it.
pub fn set_context(root: &Path, context: Option<&str>) -> Result<bool, SpecError> {
    apply(root, &[Edit::Context(context.map(str::to_string))])
}

/// Set one artifact's rules; an empty slice removes them.
pub fn set_rules(root: &Path, artifact: &str, rules: &[String]) -> Result<bool, SpecError> {
    apply(
        root,
        &[Edit::Rules {
            artifact: artifact.to_string(),
            rules: rules.to_vec(),
        }],
    )
}

/// Set one operation's guidance; an empty slice removes it.
pub fn set_operation_guidance(
    root: &Path,
    operation: &str,
    guidance: &[String],
) -> Result<bool, SpecError> {
    apply(
        root,
        &[Edit::OperationGuidance {
            operation: operation.to_string(),
            guidance: guidance.to_vec(),
        }],
    )
}

/// Apply edits to text. The whole editor; [`apply`] is this plus a file.
///
/// Edits are applied in order and each one re-scans, because an edit moves every line below it
/// and a batch computed against stale line numbers would splice into the wrong place. The lists
/// involved have single-digit lengths, so the re-scan is not worth avoiding and being obviously
/// correct is.
pub fn edit_text(text: &str, edits: &[Edit]) -> Result<String, SpecError> {
    let shape = Shape::of(text);
    let normalised = shape.normalise(text);
    let mut doc = Lines::of(&normalised);
    for edit in edits {
        doc.apply(edit)?;
    }
    Ok(shape.restore(&doc.render()))
}

// ---------------------------------------------------------------------------------------------
// The scanner
// ---------------------------------------------------------------------------------------------

/// A file's line endings and byte-order mark, so they can be put back.
///
/// A near-copy of `crate::write::Shape`, deliberately not shared *yet*. The two are the same four
/// lines and the same argument — a cide that silently converted a CRLF file to LF would produce a
/// diff touching every line of a file somebody else maintains — but they are reached by different
/// paths (one splices markdown, one splices YAML) and hoisting them into a common module is a
/// change to `write.rs`. When a third caller appears, hoist all three at once; two copies of four
/// lines is cheaper than a premature seam, and the round-trip tests on both sides are what keep
/// them honest.
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

/// A document as a vector of lines, remembering whether the last one ended in a newline.
///
/// The final newline is carried rather than inferred because both states are real files and the
/// wrong guess is a one-character diff on a committed file. A `split('\n')` alone cannot tell
/// `"a"` from `"a\n"`.
#[derive(Debug, Clone)]
struct Lines {
    lines: Vec<String>,
    final_newline: bool,
}

impl Lines {
    fn of(text: &str) -> Self {
        if text.is_empty() {
            return Self {
                lines: Vec::new(),
                final_newline: false,
            };
        }
        let final_newline = text.ends_with('\n');
        let body = text.strip_suffix('\n').unwrap_or(text);
        Self {
            lines: body.split('\n').map(str::to_string).collect(),
            final_newline,
        }
    }

    fn render(&self) -> String {
        let mut out = self.lines.join("\n");
        if self.final_newline && !self.lines.is_empty() {
            out.push('\n');
        }
        out
    }
}

/// A read-only view over normalised lines, for the reader's half.
struct Document<'a> {
    lines: Vec<&'a str>,
}

impl<'a> Document<'a> {
    fn of(text: &'a str) -> Self {
        Self {
            lines: if text.is_empty() {
                Vec::new()
            } else {
                text.strip_suffix('\n')
                    .unwrap_or(text)
                    .split('\n')
                    .collect()
            },
        }
    }

    /// The lines of a top-level key, if it is stated.
    ///
    /// **Last wins**, which is what a YAML loader does with a duplicate key and therefore what
    /// `openspec` itself would see. The editor refuses a duplicate outright rather than picking;
    /// see [`Lines::region`].
    fn region(&self, key: &str) -> Option<Region> {
        regions(&self.lines, 0, key).pop()
    }

    /// A top-level key's value read as a scalar — plain, quoted, or a literal block.
    fn scalar(&self, key: &str) -> Option<String> {
        let region = self.region(key)?;
        Some(read_scalar(&self.lines, &region))
    }

    fn rules(&self) -> Vec<ArtifactRules> {
        let Some(region) = self.region("rules") else {
            return Vec::new();
        };
        let body = &self.lines[region.start + 1..region.end];
        let Some(indent) = base_indent(body) else {
            return Vec::new();
        };
        entries(body, indent)
            .into_iter()
            .map(|entry| ArtifactRules {
                artifact: entry.key,
                rules: list_items(&body[entry.children.clone()]),
            })
            .collect()
    }

    fn operations(&self) -> Vec<OperationGuidance> {
        let Some(region) = self.region("operations") else {
            return Vec::new();
        };
        let body = &self.lines[region.start + 1..region.end];
        let Some(indent) = base_indent(body) else {
            return Vec::new();
        };
        entries(body, indent)
            .into_iter()
            .map(|entry| {
                let children = &body[entry.children.clone()];
                // `guidance:` is a level deeper than the operation name. Read through it rather
                // than around it: an `operations: { apply: { <something else> } }` that this
                // build has never heard of must come back as *no guidance*, not as whatever list
                // items happen to be lying under the operation.
                let guidance = base_indent(children)
                    .map(|deeper| entries(children, deeper))
                    .unwrap_or_default()
                    .into_iter()
                    .find(|entry| entry.key == "guidance")
                    .map(|entry| list_items(&children[entry.children.clone()]))
                    .unwrap_or_default();
                OperationGuidance {
                    operation: entry.key,
                    guidance,
                }
            })
            .collect()
    }
}

/// A half-open range of line indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Region {
    start: usize,
    end: usize,
}

/// One key inside a mapping, and where its children are.
#[derive(Debug, Clone)]
struct Entry {
    key: String,
    /// Index of the `key:` line, relative to the slice it was found in.
    line: usize,
    /// The lines beneath it, relative to the same slice, with trailing blanks trimmed off.
    children: std::ops::Range<usize>,
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with('#')
}

/// Split `key: rest` at an indentation, if that is what this line is.
///
/// Refuses a list item, a comment and a blank, and refuses an empty key. The key is taken up to
/// the first `:` unquoted — quoted keys exist in YAML and do not exist in this file, and reading
/// one as a key whose name includes its quotes is a key that never matches anything, which is the
/// safe direction: it is skipped, not misidentified.
fn split_key(line: &str, indent: usize) -> Option<(String, String)> {
    if is_blank(line) || is_comment(line) || indent_of(line) != indent {
        return None;
    }
    let trimmed = line.trim_start();
    if trimmed.starts_with('-') {
        return None;
    }
    let colon = trimmed.find(':')?;
    let key = trimmed[..colon].trim();
    if key.is_empty() {
        return None;
    }
    Some((key.to_string(), trimmed[colon + 1..].to_string()))
}

/// Does what follows the colon mean "my value is on the lines below"?
///
/// Three cases: nothing at all (`rules:`), a comment (`rules:  # per artifact`), and a block
/// scalar header (`context: |`). Anything else is an inline value, and an inline value's key owns
/// exactly one line. That distinction is load-bearing: without it, `schema: spec-driven` followed
/// by a stray indented line would claim that line, and setting the schema would delete it.
fn owns_lines_below(rest: &str) -> bool {
    let trimmed = rest.trim();
    trimmed.is_empty() || trimmed.starts_with('#') || block_header(rest).is_some()
}

/// The block-scalar header on `key: |`, `key: >-`, `key: |2`, if there is one.
///
/// Returned as written so it can be put back verbatim: the chomping indicator decides whether the
/// value ends in a newline, and rewriting `|-` as `|` would change what the file means for a
/// reason no diff explains.
fn block_header(rest: &str) -> Option<String> {
    let trimmed = rest.trim_start();
    let mut chars = trimmed.chars();
    if !matches!(chars.next(), Some('|' | '>')) {
        return None;
    }
    let mut header = String::from(&trimmed[..1]);
    for c in chars {
        match c {
            '-' | '+' | '0'..='9' => header.push(c),
            ' ' | '\t' => break,
            '#' => break,
            // Anything else means this was not a header at all — `context: |pipe` is a plain
            // scalar starting with a pipe, however unlikely.
            _ => return None,
        }
    }
    // Whatever follows must be blank or a comment; `context: | x` is not a header either.
    let tail = trimmed[header.len()..].trim();
    if tail.is_empty() || tail.starts_with('#') {
        Some(header)
    } else {
        None
    }
}

/// Every region a key occupies at an indentation, in document order.
///
/// A key's region is its own line plus, when the key [`owns_lines_below`], every following line
/// that is more indented — with blank lines and comment lines at or left of the key's own
/// indentation counted only when a more-indented line follows them.
///
/// That last rule is the whole reason the comment blocks survive. In the file `openspec init`
/// writes, `schema: spec-driven` is followed by a blank line and then six comment lines that
/// document `context`. Counting them as part of `schema` would mean setting the schema deleted
/// the documentation for the key below it — silently, in a committed file, and only visible in a
/// pull request nobody reads carefully. Counting them only when an indented line confirms them
/// means the region ends at the last line that is unambiguously the key's own, and everything
/// between it and the next key is copied.
fn regions(lines: &[&str], indent: usize, key: &str) -> Vec<Region> {
    let mut found = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Some((name, rest)) = split_key(line, indent) else {
            continue;
        };
        if name != key {
            continue;
        }
        let mut end = index + 1;
        if owns_lines_below(&rest) {
            for (offset, below) in lines.iter().enumerate().skip(index + 1) {
                if is_blank(below) {
                    continue;
                }
                if indent_of(below) > indent {
                    end = offset + 1;
                    continue;
                }
                if is_comment(below) {
                    continue;
                }
                break;
            }
        }
        found.push(Region { start: index, end });
    }
    found
}

/// The indentation a mapping's keys sit at, taken off the lines themselves.
///
/// The minimum indent over the body's real lines. Comments are excluded from the vote because
/// they are routinely written flush left inside an indented block, and one of them would drag the
/// answer to zero and make every key in the mapping invisible.
fn base_indent(body: &[&str]) -> Option<usize> {
    body.iter()
        .filter(|line| !is_blank(line) && !is_comment(line))
        .map(|line| indent_of(line))
        .min()
}

/// The keys of a mapping, with each one's children.
fn entries(body: &[&str], indent: usize) -> Vec<Entry> {
    let heads: Vec<(usize, String)> = body
        .iter()
        .enumerate()
        .filter_map(|(index, line)| split_key(line, indent).map(|(key, _)| (index, key)))
        .collect();
    heads
        .iter()
        .enumerate()
        .map(|(position, (index, key))| {
            let start = index + 1;
            let mut end = heads
                .get(position + 1)
                .map_or(body.len(), |(next, _)| *next);
            while end > start && is_blank(body[end - 1]) {
                end -= 1;
            }
            Entry {
                key: key.clone(),
                line: *index,
                children: start..end.max(start),
            }
        })
        .collect()
}

/// The `- ` items in a slice of lines, as scalars.
///
/// Every list item at any indentation, because both `key:\n  - a` and `key:\n- a` are the same
/// YAML and a reader that insisted on one of them would come back empty from a file that is
/// perfectly valid. Lines that are not list items are skipped rather than refused — that is what
/// makes a `guidance:` sub-key readable through the same helper.
fn list_items(lines: &[&str]) -> Vec<String> {
    lines
        .iter()
        .filter(|line| !is_blank(line) && !is_comment(line))
        .filter_map(|line| {
            let trimmed = line.trim_start();
            let rest = trimmed.strip_prefix('-')?;
            if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
                // `-foo` is a scalar beginning with a dash, not a list item.
                return None;
            }
            Some(scalar_value(rest))
        })
        .collect()
}

/// Read a key's value, whether it is inline or a block below.
fn read_scalar(lines: &[&str], region: &Region) -> String {
    let Some((_, rest)) = split_key(lines[region.start], indent_of(lines[region.start])) else {
        return String::new();
    };
    if let Some(header) = block_header(&rest) {
        return read_block(&lines[region.start + 1..region.end], header_indent(&header));
    }
    scalar_value(&rest)
}

/// The explicit indentation a block header states, if it states one.
///
/// `context: |2` means the block's content is indented two spaces *whatever the first line looks
/// like*, and honouring it is not pedantry: the indicator exists precisely for a block whose first
/// content line is itself indented, which is the one case where guessing from that line is
/// guaranteed to be wrong. Read as an absolute column because every block this module addresses
/// hangs off a top-level key, whose own indentation is zero.
fn header_indent(header: &str) -> Option<usize> {
    let digits: String = header.chars().filter(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// A literal block scalar's text, dedented.
///
/// The indentation stripped is the header's explicit indicator when there is one, and otherwise
/// the *first non-blank line's*, which is YAML's rule. Deeper indentation inside the block is
/// content and is kept — a context that lists a tech stack as an indented sub-list must come back
/// as one, and a line that is *less* indented than the block is dedented to nothing rather than
/// sliced at a byte offset that would eat its first characters.
///
/// Trailing blank lines are dropped. That matches clip chomping (`|`), which is the header
/// `openspec init`'s own example uses and the only one anybody writes by hand, and it makes the
/// value comparable: `"a\n\n"` and `"a"` are the same context and a panel that showed them as
/// different would offer a Save that changed nothing visible.
fn read_block(body: &[&str], explicit: Option<usize>) -> String {
    let Some(indent) = explicit.or_else(|| {
        body.iter()
            .find(|line| !is_blank(line))
            .map(|line| indent_of(line))
    }) else {
        return String::new();
    };
    let mut out: Vec<String> = body
        .iter()
        .map(|line| {
            if is_blank(line) {
                String::new()
            } else if indent_of(line) >= indent {
                line[indent..].to_string()
            } else {
                line.trim_start().to_string()
            }
        })
        .collect();
    while out.last().is_some_and(|line| line.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

/// One inline scalar: double-quoted, single-quoted, or plain up to a comment.
///
/// The ` #` rule is YAML's and it is not the same as "strip everything after a hash": a comment
/// has to be preceded by whitespace, so `schema: spec#driven` is a schema called `spec#driven`
/// and not a schema called `spec`. Getting that backwards would silently truncate a value on
/// read and then write the truncation back.
fn scalar_value(raw: &str) -> String {
    let trimmed = raw.trim_start();
    match trimmed.chars().next() {
        Some('"') => unescape_double(&trimmed[1..]),
        Some('\'') => unescape_single(&trimmed[1..]),
        _ => {
            let mut end = trimmed.len();
            let bytes = trimmed.as_bytes();
            for (index, byte) in bytes.iter().enumerate() {
                if *byte == b'#'
                    && index > 0
                    && (bytes[index - 1] == b' ' || bytes[index - 1] == b'\t')
                {
                    end = index;
                    break;
                }
            }
            trimmed[..end].trim_end().to_string()
        }
    }
}

/// The body of a double-quoted scalar, up to its closing quote.
fn unescape_double(body: &str) -> String {
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('0') => out.push('\0'),
                Some('b') => out.push('\u{8}'),
                Some('f') => out.push('\u{c}'),
                Some('x') => {
                    let hex: String = chars.by_ref().take(2).collect();
                    match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        Some(c) => out.push(c),
                        // An escape this reader does not understand is put back as written
                        // rather than dropped: showing `\xZZ` in a panel is a user who can see
                        // what is in their file, and dropping it is a value that silently
                        // changes the moment anything is saved.
                        None => {
                            out.push_str("\\x");
                            out.push_str(&hex);
                        }
                    }
                }
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        Some(c) => out.push(c),
                        None => {
                            out.push_str("\\u");
                            out.push_str(&hex);
                        }
                    }
                }
                Some(other) => out.push(other),
                None => out.push('\\'),
            },
            other => out.push(other),
        }
    }
    out
}

/// The body of a single-quoted scalar. `''` is the only escape YAML gives it.
fn unescape_single(body: &str) -> String {
    let mut out = String::new();
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            if chars.peek() == Some(&'\'') {
                chars.next();
                out.push('\'');
                continue;
            }
            break;
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// The renderer
// ---------------------------------------------------------------------------------------------

/// The characters that make a plain scalar mean something other than itself.
///
/// Anything starting with one of these, or holding a `: ` or a ` #`, has to be quoted or YAML
/// reads it as a mapping, a list, an alias, a directive or a comment. The list is CommonMark-ish
/// only by coincidence — it is YAML 1.2's `c-indicator` set plus the two in-line traps.
const INDICATORS: [char; 19] = [
    '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%', '@', '`',
];

/// Does this value have to be quoted to survive being written down?
///
/// Errs towards quoting. A value quoted that need not have been is a cosmetic difference in a
/// file the user is about to read; a value *not* quoted that needed to be is a file `openspec`
/// refuses to load, or worse, loads as something else. The resolvable-plain-scalar cases
/// (`true`, `null`, `1.5`, `~`) are in here for the second reason: a rule that reads `- no` is
/// the boolean `false` to a YAML loader and the word "no" to the person who typed it.
fn needs_quoting(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    if value.trim() != value {
        return true;
    }
    if value.contains('\n') || value.contains('\r') || value.contains('\t') {
        return true;
    }
    if value.starts_with(|c: char| INDICATORS.contains(&c)) {
        return true;
    }
    if value.contains(": ") || value.ends_with(':') || value.contains(" #") {
        return true;
    }
    let lower = value.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "true" | "false" | "yes" | "no" | "on" | "off" | "null" | "~"
    ) {
        return true;
    }
    // A bare number, which would come back as a number rather than as this string.
    value.parse::<f64>().is_ok()
}

/// A value as a YAML inline scalar, quoted only when it has to be.
fn inline_scalar(value: &str) -> String {
    if !needs_quoting(value) {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A literal block scalar's lines, for a value that has to keep its own line breaks.
///
/// The indentation indicator is not decoration. A block whose first content line begins with a
/// space is ambiguous to YAML — it cannot tell the block's indentation from the content's — and
/// the loader either refuses the file or silently eats the space. `|2` states the indentation
/// explicitly, so a context whose first line is indented survives.
///
/// A blank line inside the block is written as a genuinely empty line rather than as the
/// indentation followed by nothing, because trailing whitespace is the kind of byte a linter, an
/// editor's save hook or a reviewer removes, and a file that acquires it on every save is a file
/// that fights its own repository.
fn block_lines(key: &str, header: Option<&str>, indent: usize, value: &str) -> Vec<String> {
    let first_is_indented = value
        .split('\n')
        .next()
        .is_some_and(|line| line.starts_with(' ') || line.starts_with('\t'));
    // YAML's indentation indicator is a single digit, so a block that needs one cannot be
    // indented ten spaces. Clamped rather than refused: the alternative is a settings panel that
    // cannot save a context because of the file's indentation style.
    let indent = if first_is_indented {
        indent.clamp(1, 9)
    } else {
        indent
    };
    let header = match header {
        Some(existing) if !first_is_indented => existing.to_string(),
        Some(existing) if existing.contains(|c: char| c.is_ascii_digit()) => existing.to_string(),
        Some(existing) => {
            let chomp: String = existing
                .chars()
                .filter(|c| *c == '-' || *c == '+')
                .collect();
            format!("{}{indent}{chomp}", &existing[..1])
        }
        None if first_is_indented => format!("|{indent}"),
        None => "|".to_string(),
    };
    let pad = " ".repeat(indent);
    let mut out = vec![format!("{key}: {header}")];
    for line in value.split('\n') {
        if line.is_empty() {
            out.push(String::new());
        } else {
            out.push(format!("{pad}{line}"));
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// The editor
// ---------------------------------------------------------------------------------------------

impl Lines {
    fn view(&self) -> Vec<&str> {
        self.lines.iter().map(String::as_str).collect()
    }

    /// The one region a top-level key occupies, or a refusal if it occupies two.
    ///
    /// **A duplicate key is refused rather than resolved**, [`crate::block::Missing::Ambiguous`]'s
    /// argument exactly. A YAML loader takes the last one, so editing the first would produce a
    /// file whose visible change has no effect, and editing the last would leave a stale earlier
    /// key that a future reader might take instead. Neither is something to do silently to a
    /// committed file; the user has a two-second fix and cide has none.
    fn region(&self, key: &str) -> Result<Option<Region>, SpecError> {
        let view = self.view();
        let mut found = regions(&view, 0, key);
        match found.len() {
            0 => Ok(None),
            1 => Ok(found.pop()),
            n => Err(SpecError::Write(format!(
                "`{key}` is stated {n} times at the top level. cide will not guess which one you \
                 meant — a YAML reader takes the last, so editing the first would change nothing \
                 and editing the last would leave the earlier one contradicting it. Delete the \
                 duplicate first."
            ))),
        }
    }

    fn apply(&mut self, edit: &Edit) -> Result<(), SpecError> {
        match edit {
            Edit::Schema(schema) => self.set_top_scalar("schema", Some(schema)),
            Edit::Context(context) => self.set_top_scalar("context", context.as_deref()),
            Edit::Rules { artifact, rules } => self.set_nested("rules", artifact, None, rules),
            Edit::OperationGuidance {
                operation,
                guidance,
            } => self.set_nested("operations", operation, Some("guidance"), guidance),
        }
    }

    /// Set or clear a top-level scalar key.
    fn set_top_scalar(&mut self, key: &str, value: Option<&str>) -> Result<(), SpecError> {
        let region = self.region(key)?;
        match (region, value) {
            (None, None) => Ok(()),
            (Some(region), None) => {
                self.remove(region);
                Ok(())
            }
            (Some(region), Some(value)) => {
                let view = self.view();
                // The round-trip guarantee, and it lives here rather than in a comparison of
                // rendered bytes: what the file already *says* is compared to what is being set,
                // and equal values leave the lines alone. Rendering and comparing would churn a
                // block scalar whose blank lines carry trailing spaces, or one written with a
                // chomping indicator this renderer would not have chosen, and the diff would be
                // a file somebody else maintains changing for no reason a reviewer can see.
                if read_scalar(&view, &region) == value {
                    return Ok(());
                }
                let existing_rest = split_key(view[region.start], 0).map(|(_, rest)| rest);
                let header = existing_rest.as_deref().and_then(block_header);
                let indent = self
                    .lines
                    .get(region.start + 1..region.end)
                    .and_then(|body| {
                        body.iter()
                            .find(|line| !is_blank(line))
                            .map(|line| indent_of(line))
                    })
                    .unwrap_or(INDENT_STEP);
                let replacement = render_scalar(key, header.as_deref(), indent, value);
                self.splice(region, replacement);
                Ok(())
            }
            (None, Some(value)) => {
                // A key that is not there gets appended at the end, separated by one blank line.
                // Appended and not inserted next to the comment block that documents it: finding
                // "the right place" means matching prose, and a scanner that guessed wrong would
                // put `context:` in the middle of the commented example for `rules` — where it
                // is valid YAML, invisible to the reader, and wrong.
                let replacement = render_scalar(key, None, INDENT_STEP, value);
                self.append(replacement);
                Ok(())
            }
        }
    }

    /// Set or clear a list nested one or two levels under a top-level key.
    ///
    /// `rules` nests the list directly under the artifact name; `operations` puts a `guidance:`
    /// level in between. One function for both because the difference really is one optional
    /// level, and two near-identical copies of this bookkeeping would drift.
    fn set_nested(
        &mut self,
        top: &str,
        name: &str,
        inner: Option<&str>,
        items: &[String],
    ) -> Result<(), SpecError> {
        let Some(region) = self.region(top)? else {
            if items.is_empty() {
                return Ok(());
            }
            let mut block = vec![format!("{top}:")];
            block.extend(render_nested(
                name,
                inner,
                INDENT_STEP,
                INDENT_STEP,
                items,
                &[],
            ));
            self.append(block);
            return Ok(());
        };

        let view = self.view();
        let body = &view[region.start + 1..region.end];
        let step = base_indent(body).unwrap_or(INDENT_STEP);
        // The nesting step this file already uses, so a four-space file stays a four-space file
        // and a file whose list items sit flush with their key keeps them flush.
        //
        // Measured over the body's lines that are **not** this mapping's own keys, which is the
        // part that has to be right in both directions. Measuring over every line reports a step
        // of zero for any file at all, because a key line sits at exactly `step`; measuring over
        // lines strictly deeper than `step` cannot see the flush style at all and silently
        // re-indents it. `- one` at the key's own indentation is legal YAML and is what a person
        // who has written YAML before tends to type.
        let inner_step = body
            .iter()
            .filter(|line| !is_blank(line) && !is_comment(line) && split_key(line, step).is_none())
            .map(|line| indent_of(line))
            .min()
            .map_or(INDENT_STEP, |deeper| deeper.saturating_sub(step));

        let found = entries(body, step).into_iter().find(|e| e.key == name);
        // What the entry states now, read before anything is decided. The `Edit` that clears a
        // list and the `Edit` that writes an identical one both have to be measured against this
        // and not against `items.is_empty()`, which is the bug the corpus caught: `operations`
        // holding a shape this reader does not understand reads back as an operation with *no*
        // guidance, and a panel writing every field on Save would then have "cleared" it — taking
        // the anchors and the whole key with it. Unchanged is untouched, and empty-unchanged is
        // the case where that matters most, because the value it would destroy is the one this
        // reader could not see.
        let current = found.as_ref().map(|entry| {
            let children = &body[entry.children.clone()];
            match inner {
                None => list_items(children),
                Some(inner) => base_indent(children)
                    .map(|deeper| entries(children, deeper))
                    .unwrap_or_default()
                    .into_iter()
                    .find(|e| e.key == inner)
                    .map(|e| list_items(&children[e.children.clone()]))
                    .unwrap_or_default(),
            }
        });
        if current.as_deref() == Some(items) {
            return Ok(());
        }
        match (found, items.is_empty()) {
            (None, true) => Ok(()),
            (None, false) => {
                // Appended at the end of the mapping. `region.end` already excludes the trailing
                // blank lines and the comments that belong to whatever comes next, so this lands
                // immediately after the last entry and before them.
                let block = render_nested(name, inner, step, inner_step, items, &[]);
                let at = Region {
                    start: region.end,
                    end: region.end,
                };
                self.splice(at, block);
                Ok(())
            }
            (Some(entry), true) => {
                let absolute = Region {
                    start: region.start + 1 + entry.line,
                    end: region.start + 1 + entry.children.end,
                };
                self.remove(absolute);
                // A mapping whose last entry has gone is not an empty mapping — `rules:` on its
                // own is YAML `null`. Take the key with it, so clearing everything a panel set
                // returns the file to what it was rather than leaving a key the user never wrote.
                if let Some(region) = self.region(top)? {
                    let view = self.view();
                    let body = &view[region.start + 1..region.end];
                    if base_indent(body).is_none() {
                        self.remove(region);
                    }
                }
                Ok(())
            }
            (Some(entry), false) => {
                let children = &body[entry.children.clone()];
                // A comment written inside a list cannot be re-attached to an item once the list
                // has changed — the item it explained may not exist any more — so it is collected
                // and re-emitted at the head of the entry, at the indentation it had. Moved, and
                // never lost: a comment somebody wrote to explain a rule is the last thing an
                // edit to that rule should silently delete, and `# why proposals stay short` sat
                // exactly there in the corpus.
                let carried: Vec<String> = children
                    .iter()
                    .filter(|line| is_comment(line))
                    .map(|line| line.to_string())
                    .collect();
                // The indentation this entry's own items already use, so replacing one rule in a
                // file indented some other way does not leave an island. A slice of children
                // holds no key at `step` by construction — `entries` carves at exactly those —
                // so every line here is deeper or flush, and both are measured.
                let existing_step = children
                    .iter()
                    .filter(|line| !is_blank(line) && !is_comment(line))
                    .map(|line| indent_of(line))
                    .min()
                    .map_or(inner_step, |deeper| deeper.saturating_sub(step));
                let block = render_nested(name, inner, step, existing_step, items, &carried);
                let absolute = Region {
                    start: region.start + 1 + entry.line,
                    end: region.start + 1 + entry.children.end,
                };
                self.splice(absolute, block);
                Ok(())
            }
        }
    }

    /// Replace a line range with new lines.
    fn splice(&mut self, region: Region, replacement: Vec<String>) {
        let end = region.end.min(self.lines.len());
        let start = region.start.min(end);
        self.lines.splice(start..end, replacement);
    }

    /// Remove a line range, and one of the two blank lines it leaves back to back.
    ///
    /// Without that collapse, a panel that sets a context and then clears it leaves the file one
    /// blank line longer every time round, which is a diff on a committed file produced by doing
    /// and undoing nothing. With it, [`Self::append`]'s single separating blank line and this are
    /// inverses, and set-then-clear is a no-op.
    fn remove(&mut self, region: Region) {
        let end = region.end.min(self.lines.len());
        let start = region.start.min(end);
        self.lines.drain(start..end);
        if start >= self.lines.len() {
            // The region was the last thing in the file, so the blank line that separated it from
            // what came before is now a trailing blank line. Trimmed only in this case: a file
            // that genuinely ends in blank lines and has a key removed from its *middle* keeps
            // them, because they are then nothing to do with the edit.
            while self.lines.last().is_some_and(|line| is_blank(line)) {
                self.lines.pop();
            }
        } else if start > 0 && is_blank(&self.lines[start - 1]) && is_blank(&self.lines[start]) {
            self.lines.remove(start);
        }
        // A file emptied entirely should be empty, not one blank line.
        if self.lines.iter().all(|line| is_blank(line)) {
            self.lines.clear();
            self.final_newline = false;
        }
    }

    /// Put new lines at the end of the file, one blank line clear of what is there.
    fn append(&mut self, block: Vec<String>) {
        let had_lines = !self.lines.is_empty();
        if had_lines && !self.lines.last().is_some_and(|line| is_blank(line)) {
            self.lines.push(String::new());
        }
        self.lines.extend(block);
        // A file that had no final newline and has just grown a key keeps its convention; a file
        // that was empty gets one, because a YAML file with no trailing newline is a diff hunk
        // marked "\ No newline at end of file" for the rest of its life.
        if !had_lines {
            self.final_newline = true;
        }
    }
}

/// A top-level scalar's lines: inline when it fits on one, a literal block when it does not.
///
/// An existing block header wins over the shorter form even for a value that would now fit on one
/// line. Somebody who wrote `context: |` meant prose, and turning it into `context: Some words`
/// the first time it is shortened is a reformat of a file cide was asked to *edit*.
fn render_scalar(key: &str, header: Option<&str>, indent: usize, value: &str) -> Vec<String> {
    if value.contains('\n') || header.is_some() {
        block_lines(key, header, indent, value)
    } else {
        vec![format!("{key}: {}", inline_scalar(value))]
    }
}

/// One entry of a nested mapping, with its list.
///
/// `carried` is the comment lines that were inside the entry being replaced, written back
/// verbatim just above the items. See the call site for why they move rather than staying put.
fn render_nested(
    name: &str,
    inner: Option<&str>,
    indent: usize,
    step: usize,
    items: &[String],
    carried: &[String],
) -> Vec<String> {
    let pad = " ".repeat(indent);
    let mut out = vec![format!("{pad}{}:", inline_key(name))];
    let item_indent = match inner {
        Some(inner) => {
            out.push(format!(
                "{}{}:",
                " ".repeat(indent + step),
                inline_key(inner)
            ));
            indent + step + step
        }
        None => indent + step,
    };
    out.extend(carried.iter().cloned());
    for item in items {
        out.push(format!(
            "{}- {}",
            " ".repeat(item_indent),
            inline_scalar(item)
        ));
    }
    out
}

/// A mapping key, quoted if it would otherwise not mean itself.
///
/// Artifact ids and operation names are `[a-z-]` in practice, but they arrive from a panel and
/// this is the one place that could turn a typed name into structure.
fn inline_key(key: &str) -> String {
    if needs_quoting(key) {
        inline_scalar(key)
    } else {
        key.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_comment_block_documenting_the_next_key_is_never_part_of_the_previous_one() {
        // The whole file `openspec init` writes is this shape, and counting the comments as part
        // of `schema` would mean setting the schema deleted the documentation below it.
        let text =
            "schema: spec-driven\n\n# Project context (optional)\n# Example:\n#   context: |\n";
        let doc = Document::of(text);
        let region = doc.region("schema").expect("stated");
        assert_eq!(region, Region { start: 0, end: 1 });
    }

    #[test]
    fn a_key_owns_the_indented_lines_below_it_only_when_it_states_no_inline_value() {
        // `schema: spec-driven` followed by a stray indented line must not claim it.
        let text = "schema: spec-driven\n  stray\nrules:\n  proposal:\n    - one\n";
        let doc = Document::of(text);
        assert_eq!(doc.region("schema").expect("stated").end, 1);
        let rules = doc.region("rules").expect("stated");
        assert_eq!(rules, Region { start: 2, end: 5 });
    }

    #[test]
    fn a_comment_inside_a_mapping_is_kept_when_a_deeper_line_confirms_it() {
        let text = "rules:\n# about proposals\n  proposal:\n    - one\n\n# next key\nschema: x\n";
        let doc = Document::of(text);
        // The comment at column zero is inside `rules` because an indented line follows it…
        assert_eq!(
            doc.region("rules").expect("stated"),
            Region { start: 0, end: 4 }
        );
        // …and the one that only precedes another top-level key is not.
        assert_eq!(doc.rules().len(), 1);
    }

    #[test]
    fn a_block_scalar_keeps_its_internal_blank_lines_and_its_deeper_indentation() {
        let text = "context: |\n  Tech stack: TypeScript\n\n    - React\n  Domain: shopping\n";
        let config = read_text(text);
        assert_eq!(
            config.context.as_deref(),
            Some("Tech stack: TypeScript\n\n  - React\nDomain: shopping")
        );
    }

    #[test]
    fn a_hash_is_only_a_comment_when_whitespace_precedes_it() {
        assert_eq!(scalar_value(" spec#driven"), "spec#driven");
        assert_eq!(scalar_value(" spec-driven # the default"), "spec-driven");
        assert_eq!(scalar_value(" \"a # b\""), "a # b");
        assert_eq!(scalar_value(" 'it''s'"), "it's");
    }

    #[test]
    fn a_value_that_would_not_mean_itself_is_quoted_and_one_that_would_is_not() {
        // `- no` is the boolean false to a YAML loader and the word "no" to whoever typed it.
        assert_eq!(inline_scalar("no"), "\"no\"");
        assert_eq!(inline_scalar("1.5"), "\"1.5\"");
        assert_eq!(inline_scalar("- dash first"), "\"- dash first\"");
        assert_eq!(inline_scalar("key: value"), "\"key: value\"");
        assert_eq!(inline_scalar("trailing "), "\"trailing \"");
        // …and the ordinary case stays readable.
        assert_eq!(inline_scalar("spec-driven"), "spec-driven");
        assert_eq!(
            inline_scalar("Always include a \"Non-goals\" section"),
            "Always include a \"Non-goals\" section"
        );
    }

    #[test]
    fn a_block_whose_first_line_is_indented_gets_an_explicit_indentation_indicator() {
        // Without it YAML cannot tell the block's indentation from the content's, and the loader
        // either refuses the file or silently eats the space.
        let lines = block_lines("context", None, 2, "  indented first\nplain");
        assert_eq!(lines[0], "context: |2");
        assert_eq!(lines[1], "    indented first");
        // Reading it back gives the value that was written.
        let text = format!("{}\n", lines.join("\n"));
        assert_eq!(
            read_text(&text).context.as_deref(),
            Some("  indented first\nplain")
        );
    }

    #[test]
    fn a_duplicate_top_level_key_is_refused_rather_than_resolved() {
        // A YAML reader takes the last, so editing the first would change nothing visible.
        let text = "schema: a\nschema: b\n";
        assert_eq!(read_text(text).schema.as_deref(), Some("b"), "last wins");
        let error = edit_text(text, &[Edit::Schema("c".into())]).expect_err("refused");
        assert!(error.to_string().contains("stated 2 times"), "{error}");
    }

    #[test]
    fn setting_a_value_to_what_the_file_already_says_touches_nothing() {
        let text = "schema: spec-driven\n\n# a comment\ncontext: |\n  One.\n  Two.\n";
        let same = edit_text(
            text,
            &[
                Edit::Schema("spec-driven".into()),
                Edit::Context(Some("One.\nTwo.".into())),
            ],
        )
        .expect("edited");
        assert_eq!(same, text);
    }

    #[test]
    fn appending_and_clearing_a_key_are_inverses() {
        // Otherwise a panel that sets a context and then clears it leaves the file one blank line
        // longer every time round — a diff produced by doing and undoing nothing.
        let text = "schema: spec-driven\n\n# Project context (optional)\n";
        let with = edit_text(text, &[Edit::Context(Some("Hello.".into()))]).expect("edited");
        assert!(with.contains("context: Hello."), "{with}");
        assert!(with.contains("# Project context (optional)"), "{with}");
        let without = edit_text(&with, &[Edit::Context(None)]).expect("edited");
        assert_eq!(without, text);
    }

    #[test]
    fn removing_the_last_nested_entry_removes_the_key_that_held_it() {
        // `rules:` alone is YAML null, not an empty mapping.
        let text = "schema: x\n\nrules:\n  proposal:\n    - one\n";
        let cleared = edit_text(
            text,
            &[Edit::Rules {
                artifact: "proposal".into(),
                rules: Vec::new(),
            }],
        )
        .expect("edited");
        assert_eq!(cleared, "schema: x\n");
    }

    #[test]
    fn a_files_own_indentation_is_reused_rather_than_normalised() {
        // A four-space file must not grow a two-space island the first time a rule is edited.
        let text = "rules:\n    proposal:\n        - one\n";
        let edited = edit_text(
            text,
            &[
                Edit::Rules {
                    artifact: "proposal".into(),
                    rules: vec!["two".into()],
                },
                Edit::Rules {
                    artifact: "design".into(),
                    rules: vec!["three".into()],
                },
            ],
        )
        .expect("edited");
        assert_eq!(
            edited,
            "rules:\n    proposal:\n        - two\n    design:\n        - three\n"
        );
    }

    #[test]
    fn operations_read_through_guidance_and_not_around_it() {
        // A shape this build has never heard of must come back as no guidance, rather than as
        // whatever list items happen to be lying under the operation.
        let known = "operations:\n  apply:\n    guidance:\n      - Keep it short\n";
        assert_eq!(
            read_text(known).guidance_for("apply"),
            ["Keep it short".to_string()]
        );
        let unknown = "operations:\n  apply:\n    something-else:\n      - Not guidance\n";
        assert!(read_text(unknown).guidance_for("apply").is_empty());
        // …and the operation is still listed, so a panel can show that it is configured.
        assert_eq!(read_text(unknown).operations.len(), 1);
    }

    #[test]
    fn a_list_item_is_read_at_either_of_the_two_indentations_yaml_allows() {
        let flush = "rules:\n  proposal:\n  - one\n  - two\n";
        let indented = "rules:\n  proposal:\n    - one\n    - two\n";
        let expected = ["one".to_string(), "two".to_string()];
        assert_eq!(read_text(flush).rules_for("proposal"), expected);
        assert_eq!(read_text(indented).rules_for("proposal"), expected);
    }

    #[test]
    fn a_file_this_reader_does_not_understand_yields_nothing_and_is_left_alone() {
        // Reading nothing is the safe direction: a key that was not understood is a key that is
        // never rewritten, so a misread cannot become a lost value.
        let text = "%YAML 1.2\n---\nrules: {proposal: [one, two]}\nnonsense\n";
        let config = read_text(text);
        assert!(config.rules_for("proposal").is_empty());
        assert_eq!(
            edit_text(text, &[Edit::Context(None)]).expect("edited"),
            text,
            "clearing a key that is not there rewrites nothing"
        );
    }

    #[test]
    fn crlf_and_a_bom_survive_an_edit() {
        let text = "\u{feff}schema: spec-driven\r\n\r\n# a comment\r\n";
        let edited = edit_text(text, &[Edit::Schema("custom".into())]).expect("edited");
        assert!(edited.starts_with('\u{feff}'), "{edited:?}");
        assert!(edited.contains("schema: custom\r\n"), "{edited:?}");
        assert!(edited.contains("# a comment"), "{edited:?}");
        assert!(!edited.replace("\r\n", "").contains('\n'), "{edited:?}");
    }
}
