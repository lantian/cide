//! Reading `cide-marketplace.json` and `cide-extension.json`, and refusing them legibly.
//!
//! # JSON, not markdown with front matter
//!
//! `cide-agents` argues at length for the opposite choice and is right for its case: an agent
//! definition's *body is a system prompt*, a multi-paragraph document, and a document inside a
//! JSON string is one line with `\n` between every sentence — unreadable in an editor and,
//! decisively, unreviewable in a diff.
//!
//! An extension manifest has no body. It is a keyword table, a list of file extensions and a set
//! of flags: every value is a scalar or a list of scalars, nested two or three deep. That is the
//! shape JSON is for, and the shape a hand-rolled restricted grammar would be worst at — the very
//! feature `defs.rs` refuses (nesting) is the one this needs on every line.
//!
//! # What that costs, and what is done about it
//!
//! `serde_json` reports a line and a column for a syntax error, which is what
//! [`ExtProblem::line`] wants, and a *type* error as `"invalid type: string, expected a sequence
//! at line 14 column 21"`, which is exactly as useful. What it does **not** give for free is a
//! good answer to an unknown key: `deny_unknown_fields` says `unknown field \`langauges\`,
//! expected one of ...`, naming every alternative, which for [`Contributions`] is a readable
//! sentence and for a grammar spec is a wall.
//!
//! So the parse happens twice. First into a `serde_json::Value`, where the schema number is read
//! before anything else — deserialising straight into the struct reports a future schema as a
//! missing field, *"which is a message that sends the user looking for the wrong problem"*
//! (`cide-tasks`' words, and its rule) — and where unknown keys are collected as **warnings** with
//! their own line. Then into the typed struct, whose errors are already legible.
//!
//! # The rule this file exists to enforce
//!
//! **What is not understood is refused, never ignored.** An unknown capability greys the
//! extension rather than being dropped, because a build that ignored one would run an extension
//! under less restriction than its author declared and believed. An unknown *key* only warns,
//! because a manifest written for a newer cide should still install what this one understands —
//! the same asymmetry `cide-agents` draws between a permission mode and a tool name.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cide_ipc::ext::{
    Capability, Contributions, ExtProblem, ExtSeverity, PanelLocation, ResolvedExtCommand,
};
use cide_ipc::ids::{ExtensionId, MarketplaceId};
use serde::{Deserialize, Serialize};

/// The manifest cide is currently able to read.
///
/// Written on every file cide creates, and **not enforced on read** beyond a comparison — a file
/// from a future version still loads if serde can make sense of it, because
/// `#[serde(default)]` fills what this build does not know. What a higher number buys is a
/// *sentence*: "this extension needs a newer cide" instead of eleven missing-field warnings.
pub const SCHEMA_VERSION: u32 = 1;

/// The file at a marketplace repository's root.
pub const MARKET_FILE: &str = "cide-marketplace.json";
/// The file at an extension directory's root.
pub const EXT_FILE: &str = "cide-extension.json";

// ==========================================================================================
// Findings.
// ==========================================================================================

pub(crate) fn error(path: &Path, line: Option<u32>, message: impl Into<String>) -> ExtProblem {
    ExtProblem {
        path: path.to_path_buf(),
        line,
        severity: ExtSeverity::Error,
        message: message.into(),
    }
}

pub(crate) fn warning(path: &Path, line: Option<u32>, message: impl Into<String>) -> ExtProblem {
    ExtProblem {
        path: path.to_path_buf(),
        line,
        severity: ExtSeverity::Warning,
        message: message.into(),
    }
}

/// The line a `serde_json` error points at, when it points at one.
///
/// `Error::line()` is 0 for an error that is not positional — an EOF, or one raised by a
/// `Deserialize` impl rather than by the reader — and a `line: Some(0)` would send the panel's
/// click to a line that does not exist.
fn line_of(error: &serde_json::Error) -> Option<u32> {
    let line = error.line();
    if line == 0 {
        None
    } else {
        u32::try_from(line).ok()
    }
}

// ==========================================================================================
// The two manifests, as serde sees them.
// ==========================================================================================

/// `cide-marketplace.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarketFile {
    #[serde(default = "one")]
    schema: u32,
    #[serde(default)]
    name: String,
    #[serde(default)]
    extensions: Vec<MarketRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarketRow {
    id: String,
    /// Where the extension's directory sits, relative to the repository root. Defaults to
    /// `extensions/<id>`, which is the layout every marketplace will use until one does not.
    #[serde(default)]
    path: String,
}

fn one() -> u32 {
    1
}

/// The keys each contribution object may carry.
///
/// Hand-written beside the types they describe, and pinned to them by `unknown_key_tables_are_
/// complete` below, which reflects the real thing by serialising a default and comparing. A table
/// that drifted would warn about a key that works, which is worse than not warning at all: it
/// sends the author to fix something that is not broken.
const LANGUAGE_KEYS: &[&str] = &[
    "id",
    "label",
    "extensions",
    "filenames",
    "fenceAliases",
    "grammar",
    "fold",
    "scratch",
];
const SERVER_KEYS: &[&str] = &[
    "binary",
    "args",
    "languageIds",
    "projectMarkers",
    "projectKind",
    "installHint",
    "declaresWatchedFiles",
    "extraPathHints",
    // M45. **Allowed**, and the question was asked rather than defaulted: this is arbitrary JSON
    // handed to a spawned process, which sounds like a new hole and is not one. A manifest that
    // reaches this table has already declared `process:spawn` and already names the binary and
    // its `args` — anything `initOptions` could express, `args` could express first, and against
    // a far smaller review surface. Refusing it would instead mean an extension contributing a
    // server that *needs* configuration (a YAML server with its own schemas, the case cide's own
    // builtin exists for) could never configure it at all.
    "initOptions",
];
const PANEL_KEYS: &[&str] = &["id", "label", "icon", "location"];
const COMMAND_KEYS: &[&str] = &["id", "title", "keywords"];
const SETTING_KEYS: &[&str] = &["id", "label", "description", "kind"];
const GRAMMAR_KEYS: &[&str] = &[
    "name",
    "keywords",
    "caseInsensitiveKeywords",
    "types",
    "atoms",
    "builtins",
    "lineComment",
    "blockComment",
    "nestedComments",
    "quotes",
    "escapes",
    "tripleQuotes",
    "identifierExtra",
    "capitalisedIsType",
    "callSyntax",
    "controlOperators",
    "rules",
];
const FOLD_KEYS: &[&str] = &[
    "lineComment",
    "blockComment",
    "nestedComments",
    "quotes",
    "escapes",
    "tripleQuotes",
    "brackets",
    "multilineQuotes",
    "extraQuotes",
    "lifetimes",
    "rawStrings",
    "indentBlocks",
    "headingFolds",
    "fencedBlocks",
    "regions",
];

/// `cide-extension.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExtFile {
    #[serde(default = "one")]
    schema: u32,
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    contributes: Contributions,
    /// The worker entry point, relative to the extension directory. Absent for a purely
    /// declarative extension, which is a first-class case and not a degenerate one.
    #[serde(default)]
    main: Option<String>,
}

// ==========================================================================================
// A parsed marketplace index.
// ==========================================================================================

/// One row of a marketplace's index, resolved to a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRow {
    pub id: ExtensionId,
    /// Absolute, inside the clone.
    pub dir: PathBuf,
}

/// A marketplace's index as read from its clone.
#[derive(Debug, Clone, Default)]
pub struct MarketIndex {
    pub name: String,
    pub rows: Vec<IndexRow>,
    pub problems: Vec<ExtProblem>,
}

/// Read `cide-marketplace.json` out of a clone.
///
/// Never fails: a marketplace whose index will not parse is an empty marketplace with a problem
/// attached, which is a state the panel draws. Returning an `Err` would make one bad commit in one
/// repository into a dialog on launch.
pub fn read_index(clone: &Path) -> MarketIndex {
    let path = clone.join(MARKET_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return MarketIndex {
                problems: vec![self::error(
                    &path,
                    None,
                    format!(
                        "this repository has no {MARKET_FILE} at its root, so cide cannot tell \
                         what extensions it holds."
                    ),
                )],
                ..MarketIndex::default()
            };
        }
        Err(error) => {
            return MarketIndex {
                problems: vec![self::error(
                    &path,
                    None,
                    format!("could not be read: {error}"),
                )],
                ..MarketIndex::default()
            };
        }
    };

    let mut problems = Vec::new();
    let raw: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            return MarketIndex {
                problems: vec![self::error(&path, line_of(&error), format!("{error}"))],
                ..MarketIndex::default()
            };
        }
    };
    if let Some(problem) = schema_problem(&path, &raw) {
        return MarketIndex {
            problems: vec![problem],
            ..MarketIndex::default()
        };
    }
    problems.extend(unknown_keys(
        &path,
        &raw,
        &["schema", "name", "extensions"],
        "",
    ));

    let file: MarketFile = match serde_json::from_value(raw) {
        Ok(file) => file,
        Err(error) => {
            problems.push(self::error(&path, line_of(&error), format!("{error}")));
            return MarketIndex {
                problems,
                ..MarketIndex::default()
            };
        }
    };

    let mut rows = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for row in file.extensions {
        if !is_safe_segment(&row.id) {
            problems.push(self::error(
                &path,
                None,
                format!(
                    "`{}` is not a usable extension id: lowercase letters, digits and `-`, \
                     starting with a letter or digit, at most 32 characters.",
                    row.id
                ),
            ));
            continue;
        }
        if !seen.insert(row.id.clone()) {
            problems.push(self::error(
                &path,
                None,
                format!("`{}` is listed twice; the second row is ignored.", row.id),
            ));
            continue;
        }
        // A relative path only, and every segment checked: a marketplace that could write
        // `../../..` into `path` could point an install at any directory on the machine, and the
        // install copies whatever it finds there.
        let rel = if row.path.is_empty() {
            format!("extensions/{}", row.id)
        } else {
            row.path.clone()
        };
        match safe_relative(clone, &rel) {
            Some(dir) => rows.push(IndexRow {
                id: ExtensionId(row.id),
                dir,
            }),
            None => problems.push(self::error(
                &path,
                None,
                format!(
                    "`{}` names the directory `{rel}`, which leaves the repository. A path must \
                     be relative and must not contain `..`.",
                    row.id
                ),
            )),
        }
    }

    MarketIndex {
        name: if file.name.is_empty() {
            clone
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("marketplace")
                .to_string()
        } else {
            file.name
        },
        rows,
        problems,
    }
}

// ==========================================================================================
// A parsed extension manifest.
// ==========================================================================================

/// What one `cide-extension.json` says, once cide has understood as much of it as it can.
#[derive(Debug, Clone)]
pub struct Manifest {
    pub id: ExtensionId,
    pub name: String,
    pub version: String,
    pub description: String,
    pub capabilities: Vec<Capability>,
    pub contributes: Contributions,
    /// The worker entry, relative to the extension directory, when there is one.
    pub main: Option<String>,
    /// One sentence, when the extension parsed but is **greyed** — see the module header.
    pub unavailable: Option<String>,
    pub problems: Vec<ExtProblem>,
}

/// Read and validate one extension's manifest.
///
/// `Err` is reserved for *"there is no extension here"* — a missing or unparseable file, or one
/// whose id is unusable as a path segment. Everything else is an `Ok` carrying problems, because a
/// row the user can see and read a reason from is worth more than a row that is not drawn.
pub fn read_manifest(dir: &Path) -> Result<Manifest, ExtProblem> {
    let path = dir.join(EXT_FILE);
    let text = std::fs::read_to_string(&path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            error(&path, None, format!("no {EXT_FILE} here."))
        } else {
            error(&path, None, format!("could not be read: {err}"))
        }
    })?;

    let raw: serde_json::Value =
        serde_json::from_str(&text).map_err(|err| error(&path, line_of(&err), format!("{err}")))?;
    if let Some(problem) = schema_problem(&path, &raw) {
        return Err(problem);
    }

    let mut problems = unknown_keys(
        &path,
        &raw,
        &[
            "schema",
            "id",
            "name",
            "version",
            "description",
            "capabilities",
            "contributes",
            "main",
        ],
        "",
    );
    if let Some(contributes) = raw.get("contributes") {
        problems.extend(unknown_keys(
            &path,
            contributes,
            &[
                "languages",
                "languageServers",
                "panels",
                "commands",
                "settings",
            ],
            "contributes.",
        ));
        // And one level into each array. Not a general recursive walk: the schema is three deep
        // and finite, and a walker driven by a description of it would be more code than the
        // description. What this catches is the class the top-level pass cannot — `lineComents`
        // inside a grammar, which parses to a grammar with no line comment and a file that folds
        // wrongly for a reason nothing on screen explains.
        for (key, known) in [
            ("languages", LANGUAGE_KEYS),
            ("languageServers", SERVER_KEYS),
            ("panels", PANEL_KEYS),
            ("commands", COMMAND_KEYS),
            ("settings", SETTING_KEYS),
        ] {
            let Some(rows) = contributes.get(key).and_then(serde_json::Value::as_array) else {
                continue;
            };
            for (at, row) in rows.iter().enumerate() {
                problems.extend(unknown_keys(
                    &path,
                    row,
                    known,
                    &format!("contributes.{key}[{at}]."),
                ));
                if key == "languages" {
                    if let Some(grammar) = row.get("grammar") {
                        problems.extend(unknown_keys(
                            &path,
                            grammar,
                            GRAMMAR_KEYS,
                            &format!("contributes.languages[{at}].grammar."),
                        ));
                    }
                    if let Some(fold) = row.get("fold") {
                        problems.extend(unknown_keys(
                            &path,
                            fold,
                            FOLD_KEYS,
                            &format!("contributes.languages[{at}].fold."),
                        ));
                    }
                }
            }
        }
    }

    let file: ExtFile =
        serde_json::from_value(raw).map_err(|err| error(&path, line_of(&err), format!("{err}")))?;

    if !is_safe_segment(&file.id) {
        return Err(error(
            &path,
            None,
            format!(
                "`{}` is not a usable extension id: lowercase letters, digits and `-`, starting \
                 with a letter or digit, at most 32 characters. It is joined onto a path and \
                 prefixes every command this extension contributes.",
                file.id
            ),
        ));
    }

    // An unknown capability greys the extension. It is the one refusal that must not be a
    // warning: a capability this build does not know is a restriction it cannot enforce, and
    // running the extension anyway would run it under less restriction than its author declared.
    let mut capabilities = Vec::new();
    let mut unavailable = None;
    for name in &file.capabilities {
        match Capability::parse(name) {
            Some(cap) => {
                if !capabilities.contains(&cap) {
                    capabilities.push(cap);
                }
            }
            None => {
                let known = Capability::ALL
                    .iter()
                    .map(|c| c.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                let message = format!(
                    "asks for `{name}`, which this cide does not know. Known capabilities are: \
                     {known}."
                );
                problems.push(error(&path, None, message.clone()));
                unavailable.get_or_insert(message);
            }
        }
    }
    capabilities.sort();

    let mut manifest = Manifest {
        id: ExtensionId(file.id),
        name: if file.name.is_empty() {
            String::new()
        } else {
            file.name
        },
        version: if file.version.is_empty() {
            "0.0.0".to_string()
        } else {
            file.version
        },
        description: file.description,
        capabilities,
        contributes: file.contributes,
        main: file.main.filter(|m| !m.is_empty()),
        unavailable,
        problems,
    };
    if manifest.name.is_empty() {
        manifest.name = manifest.id.as_str().to_string();
    }
    validate_contributions(&path, &mut manifest);
    Ok(manifest)
}

/// Check what an extension declares against what cide can actually do with it.
///
/// Everything here **drops the offending contribution and keeps the rest**. An extension that
/// contributes a good language and a bad panel gets the language: dropping both would punish the
/// half that worked, and refusing to load would take away the row that explains why.
fn validate_contributions(path: &Path, manifest: &mut Manifest) {
    let declared: BTreeSet<Capability> = manifest.capabilities.iter().copied().collect();

    // A language server is a process, and a process is `process:spawn`. Checked here rather than
    // at spawn so the sentence names the manifest that asked, and checked *again* at spawn
    // because neither is allowed to be the only one — `cide_agents::valid_name` and
    // `cide_git::worktree::validate_agent` guard one join between them on the same rule.
    if !manifest.contributes.language_servers.is_empty()
        && !declared.contains(&Capability::ProcessSpawn)
    {
        manifest.problems.push(error(
            path,
            None,
            format!(
                "contributes {} language server(s) but does not ask for `process:spawn`. \
                 Running a program is a permission the user grants at install, so the servers \
                 are ignored.",
                manifest.contributes.language_servers.len()
            ),
        ));
        manifest.contributes.language_servers.clear();
    }

    // A panel is drawn from a view model a worker produces. Without a worker there is nothing to
    // draw, and an empty panel behind a rail button is worse than no button.
    if !manifest.contributes.panels.is_empty() && manifest.main.is_none() {
        manifest.problems.push(error(
            path,
            None,
            "contributes panels but has no `main`, so nothing would ever fill them.".to_string(),
        ));
        manifest.contributes.panels.clear();
    }
    if !manifest.contributes.commands.is_empty() && manifest.main.is_none() {
        manifest.problems.push(error(
            path,
            None,
            "contributes commands but has no `main`, so nothing would run them.".to_string(),
        ));
        manifest.contributes.commands.clear();
    }

    let mut seen_panels: BTreeSet<String> = BTreeSet::new();
    manifest.contributes.panels.retain(|panel| {
        if !is_safe_segment(&panel.id) {
            manifest.problems.push(error(
                path,
                None,
                format!(
                    "panel `{}`: an id must be lowercase letters, digits and `-`. It becomes part \
                     of a sidebar view name.",
                    panel.id
                ),
            ));
            return false;
        }
        if !seen_panels.insert(panel.id.clone()) {
            manifest.problems.push(error(
                path,
                None,
                format!("panel `{}` is declared twice.", panel.id),
            ));
            return false;
        }
        if panel.location == PanelLocation::Sidebar && panel.icon.is_none() {
            manifest.problems.push(warning(
                path,
                None,
                format!(
                    "panel `{}` sits on the activity rail and has no `icon`, so its button will \
                     be blank.",
                    panel.id
                ),
            ));
        }
        match &panel.icon {
            Some(icon) if !is_svg_path(icon) => {
                manifest.problems.push(error(
                    path,
                    None,
                    format!(
                        "panel `{}`: `icon` must be an SVG path `d` — digits, spaces and the \
                         path commands. The rail renders it into an attribute, so anything else \
                         is refused rather than escaped.",
                        panel.id
                    ),
                ));
                false
            }
            _ => true,
        }
    });

    // A setting's id is stored as a key in `extensions.json` and read back by the worker, so it
    // has to be a stable, unambiguous string — but it is not a path and not a command, so the rule
    // is the command one rather than the path one.
    let mut seen_settings: BTreeSet<String> = BTreeSet::new();
    let problems = &mut manifest.problems;
    manifest.contributes.settings.retain(|setting| {
        if !is_command_segment(&setting.id) {
            problems.push(error(
                path,
                None,
                format!(
                    "setting `{}`: an id must be letters, digits, `-` and `.`, starting with a \
                     letter. It is the key this value is stored under and the name the extension \
                     reads it by, so it cannot be renamed once shipped.",
                    setting.id
                ),
            ));
            return false;
        }
        if !seen_settings.insert(setting.id.clone()) {
            problems.push(error(
                path,
                None,
                format!("setting `{}` is declared twice.", setting.id),
            ));
            return false;
        }
        if setting.label.trim().is_empty() {
            problems.push(error(
                path,
                None,
                format!(
                    "setting `{}` has no label, so the Settings page would draw a control with \
                     nothing beside it.",
                    setting.id
                ),
            ));
            return false;
        }
        match &setting.kind {
            cide_ipc::ext::SettingKind::Choice { default, choices } => {
                if choices.is_empty() {
                    problems.push(error(
                        path,
                        None,
                        format!("setting `{}` is a choice with no choices.", setting.id),
                    ));
                    return false;
                }
                // A default naming no choice is the one defect here that is *silent*: `coerce`
                // would fall back to it, the control would show nothing selected, and the value
                // the worker read would not be in the list it was offered.
                if !choices.iter().any(|c| &c.value == default) {
                    problems.push(error(
                        path,
                        None,
                        format!(
                            "setting `{}`: the default `{default}` is not one of its choices.",
                            setting.id
                        ),
                    ));
                    return false;
                }
                true
            }
            cide_ipc::ext::SettingKind::Number { default, min, max } => {
                if !default.is_finite() {
                    problems.push(error(
                        path,
                        None,
                        format!("setting `{}`: the default is not a number.", setting.id),
                    ));
                    return false;
                }
                if let (Some(low), Some(high)) = (min, max)
                    && low > high
                {
                    problems.push(error(
                        path,
                        None,
                        format!(
                            "setting `{}`: `min` ({low}) is above `max` ({high}), so no value is \
                             legal.",
                            setting.id
                        ),
                    ));
                    return false;
                }
                // A default outside its own band is a warning rather than a refusal: `coerce`
                // clamps it, so the setting works — the author has simply written down two
                // numbers that disagree, and the clamped one is the honest answer.
                let clamped = min.map_or(*default, |low| default.max(low));
                let clamped = max.map_or(clamped, |high| clamped.min(high));
                if (clamped - *default).abs() > f64::EPSILON {
                    problems.push(warning(
                        path,
                        None,
                        format!(
                            "setting `{}`: the default {default} is outside its own min/max, so \
                             it reads as {clamped}.",
                            setting.id
                        ),
                    ));
                }
                true
            }
            _ => true,
        }
    });

    let mut seen_commands: BTreeSet<String> = BTreeSet::new();
    manifest.contributes.commands.retain(|command| {
        if !is_command_segment(&command.id) {
            manifest.problems.push(error(
                path,
                None,
                format!(
                    "command `{}`: an id must be letters, digits, `-` and `.`, starting with a \
                     letter. It is appended to `ext.<marketplace>.<extension>.` and a user's \
                     keymap.json may name the result.",
                    command.id
                ),
            ));
            return false;
        }
        if !seen_commands.insert(command.id.clone()) {
            manifest.problems.push(error(
                path,
                None,
                format!("command `{}` is declared twice.", command.id),
            ));
            return false;
        }
        true
    });

    let mut seen_langs: BTreeSet<String> = BTreeSet::new();
    let problems = &mut manifest.problems;
    manifest.contributes.languages.retain(|lang| {
        if !is_safe_segment(&lang.id) {
            problems.push(error(
                path,
                None,
                format!(
                    "language `{}`: an id must be lowercase letters, digits and `-`. It is also \
                     the `languageId` its documents carry to a language server.",
                    lang.id
                ),
            ));
            return false;
        }
        if !seen_langs.insert(lang.id.clone()) {
            problems.push(error(
                path,
                None,
                format!("language `{}` is declared twice.", lang.id),
            ));
            return false;
        }
        if lang.extensions.is_empty() && lang.filenames.is_empty() {
            problems.push(error(
                path,
                None,
                format!(
                    "language `{}` claims no file extensions and no file names, so no file would \
                     ever resolve to it.",
                    lang.id
                ),
            ));
            return false;
        }
        // A zero-width match takes the buffer down rather than mis-colouring it — see
        // `cide_ipc::lang::GrammarRule`. Refused here, at install, and not at the tenth token of
        // the first file somebody opens.
        let mut ok = true;
        for (at, rule) in lang.grammar.rules.iter().enumerate() {
            if rule.pattern.is_empty() {
                problems.push(error(
                    path,
                    None,
                    format!(
                        "language `{}` rule {at}: the pattern is empty. A rule that matches \
                         without consuming a character stops the tokenizer.",
                        lang.id
                    ),
                ));
                ok = false;
            }
            if rule.tag.is_empty() {
                problems.push(error(
                    path,
                    None,
                    format!("language `{}` rule {at}: `tag` is empty.", lang.id),
                ));
                ok = false;
            }
        }
        for scratch in &lang.scratch {
            if !lang.extensions.iter().any(|e| e.ext == scratch.ext) {
                problems.push(warning(
                    path,
                    None,
                    format!(
                        "language `{}` offers `.{}` in the scratch picker but does not claim that \
                         extension, so the file it creates would open as plain text.",
                        lang.id, scratch.ext
                    ),
                ));
            }
        }
        ok
    });

    // The languages this manifest contributes, plus cide's own builtins: a server bound to
    // `typescript` or `json` is the *point* of an extension that enriches a builtin language
    // with an LSP, and warning "it will only run if something else does" about a language
    // cide itself supplies would be a permanent falsehood on every install.
    let mut known: BTreeSet<String> = manifest
        .contributes
        .languages
        .iter()
        .map(|l| l.id.clone())
        .collect();
    known.extend(cide_ipc::lang::builtins().into_iter().map(|l| l.id));
    let problems = &mut manifest.problems;
    manifest.contributes.language_servers.retain(|server| {
        if server.binary.is_empty() || server.binary.contains('/') || server.binary.contains('\\') {
            problems.push(error(
                path,
                None,
                format!(
                    "language server `{}`: `binary` must be a bare program name, looked up on the \
                     child's PATH. A manifest that could name an absolute path could name \
                     anything.",
                    server.binary
                ),
            ));
            return false;
        }
        if server.language_ids.is_empty() {
            problems.push(error(
                path,
                None,
                format!(
                    "language server `{}` claims no `languageIds`, so no file would ever be sent \
                     to it.",
                    server.binary
                ),
            ));
            return false;
        }
        // A warning, not a refusal: an extension may legitimately drive a server for a language
        // some *other* extension contributes, and refusing that would make install order matter.
        for id in &server.language_ids {
            if !known.contains(id.as_str()) {
                problems.push(warning(
                    path,
                    None,
                    format!(
                        "language server `{}` claims `{id}`, which this extension does not \
                         contribute. It will only run if something else does.",
                        server.binary
                    ),
                ));
            }
        }
        true
    });
}

// ==========================================================================================
// Shared shape rules.
// ==========================================================================================

/// The character set that makes a string safe to `Path::join` and safe to read.
///
/// `cide_agents::valid_name`'s rule, kept identical on purpose: lowercase letters, digits and
/// `-`, starting with a letter or a digit, at most 32 characters. It is path safety and not
/// style — the value is joined onto a directory under `$XDG_STATE_HOME` and, for an extension,
/// becomes a segment of every command id it contributes.
#[must_use]
pub fn is_safe_segment(name: &str) -> bool {
    if name.is_empty() || name.len() > 32 {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().expect("non-empty");
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A command id's tail: letters, digits, `-` and `.`, starting with a letter.
///
/// Wider than [`is_safe_segment`] because it is not a path, and because the ids already in
/// `cide-core::commands` are `sidebar.git`, `file.reveal`, `claude.send` — a dotted namespace an
/// extension author will reach for on the first try.
#[must_use]
pub fn is_command_segment(id: &str) -> bool {
    if id.is_empty() || id.len() > 64 {
        return false;
    }
    if !id.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return false;
    }
    if id.ends_with('.') || id.contains("..") {
        return false;
    }
    id.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

/// Whether a string is an SVG path `d` and nothing else.
///
/// A whitelist, because this value is written into an attribute the rail renders. The alternative
/// — escaping it on the way out — puts the safety in the one place a future refactor of a
/// presentational component is most likely to move.
#[must_use]
pub fn is_svg_path(d: &str) -> bool {
    !d.is_empty()
        && d.len() <= 4096
        && d.chars()
            .all(|c| matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | ',' | '-' | '+' | ' ' | '\t' | '\n' | '\r'))
        && d.chars()
            .filter(|c| c.is_ascii_alphabetic())
            .all(|c| "MmLlHhVvCcSsQqTtAaZz".contains(c))
}

/// Resolve a manifest-supplied relative path inside a root, or refuse it.
///
/// Refuses anything absolute, anything with a `..`, and anything with a Windows drive or a
/// backslash — checked on the *string*, before any filesystem call, because `Path::join` with an
/// absolute right-hand side silently discards the left one. That is the failure this function
/// exists to prevent: `clone.join("/etc")` is `/etc`, and it does not error.
#[must_use]
pub fn safe_relative(root: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() || rel.starts_with('/') || rel.starts_with('\\') || rel.contains('\\') {
        return None;
    }
    if rel.len() >= 2 && rel.as_bytes()[1] == b':' {
        return None;
    }
    let mut out = root.to_path_buf();
    for segment in rel.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            return None;
        }
        out.push(segment);
    }
    if out == root { None } else { Some(out) }
}

/// Refuse a manifest whose `schema` is newer than this build understands.
///
/// Read out of the raw `Value` before the typed parse, on `cide-tasks`' rule: deserialising
/// straight into the struct reports a future schema as a *missing field*, which is a message that
/// sends the user looking for the wrong problem.
fn schema_problem(path: &Path, raw: &serde_json::Value) -> Option<ExtProblem> {
    let schema = raw.get("schema")?.as_u64()?;
    if schema > u64::from(SCHEMA_VERSION) {
        return Some(error(
            path,
            None,
            format!(
                "is schema {schema}; this cide understands up to {SCHEMA_VERSION}. Update cide, \
                 or use an older version of this extension."
            ),
        ));
    }
    None
}

/// Every key of an object that this build does not know, as warnings.
///
/// A warning and never a refusal: a manifest written for a newer cide should still install what
/// this one understands. The key is *named*, together with what was expected, because
/// `langauges` next to `languages` is the whole population of this class of bug and a message
/// that does not print the string is a message that does not help.
fn unknown_keys(
    path: &Path,
    raw: &serde_json::Value,
    known: &[&str],
    prefix: &str,
) -> Vec<ExtProblem> {
    let Some(map) = raw.as_object() else {
        return Vec::new();
    };
    map.keys()
        .filter(|key| !known.contains(&key.as_str()))
        .map(|key| {
            warning(
                path,
                None,
                format!(
                    "`{prefix}{key}` is not something this cide understands, and is ignored. \
                     Expected one of: {}.",
                    known.join(", ")
                ),
            )
        })
        .collect()
}

/// The palette id an extension's command is published under.
///
/// `ext.<marketplace>.<extension>.<id>`, and the shape is the contract `ui/src/keys/dispatch.ts`
/// tests with a single prefixed `case` — which is what keeps `check:commands`' rule ("every id is
/// either handled or carries an `unavailable` reason") true for a set that is not known at build
/// time.
#[must_use]
pub fn command_id(market: &MarketplaceId, ext: &ExtensionId, id: &str) -> String {
    format!("ext.{market}.{ext}.{id}")
}

/// The `ActivityView` a contributed panel is addressed by.
///
/// `ext:` and not `ext.`, deliberately: the command namespace is dotted and the view namespace is
/// not, so a string in a log or a bug report says which of the two it is without context.
#[must_use]
pub fn panel_view(market: &MarketplaceId, ext: &ExtensionId, panel: &str) -> String {
    format!("ext:{market}.{ext}.{panel}")
}

/// Build the palette rows for one extension.
pub(crate) fn resolved_commands(
    market: &MarketplaceId,
    ext: &ExtensionId,
    contributes: &Contributions,
) -> Vec<ResolvedExtCommand> {
    contributes
        .commands
        .iter()
        .map(|command| ResolvedExtCommand {
            id: command_id(market, ext, &command.id),
            title: command.title.clone(),
            keywords: command.keywords.clone(),
            extension: cide_ipc::ext::ExtensionRef {
                marketplace: market.clone(),
                extension: ext.clone(),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicU64, Ordering};

    /// A scratch directory, from `std::env::temp_dir()` keyed by pid and a counter.
    fn scratch(tag: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "cide-ext-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp dir");
        path
    }

    fn plant(dir: &Path, json: &str) {
        std::fs::write(dir.join(EXT_FILE), json).expect("plant");
    }

    #[test]
    fn a_minimal_manifest_loads() {
        let dir = scratch("minimal");
        plant(
            &dir,
            r#"{ "schema": 1, "id": "sql", "name": "SQL", "version": "1.0.0" }"#,
        );
        let manifest = read_manifest(&dir).expect("loads");
        assert_eq!(manifest.id.as_str(), "sql");
        assert_eq!(manifest.name, "SQL");
        assert!(
            manifest.main.is_none(),
            "a declarative extension is a first-class case"
        );
        assert!(manifest.unavailable.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The rule the module exists to enforce, in the direction that matters: a capability this
    /// build cannot name is one it cannot enforce, so the extension is greyed rather than run.
    #[test]
    fn an_unknown_capability_greys_the_extension_and_lists_the_known_ones() {
        let dir = scratch("cap");
        plant(
            &dir,
            r#"{ "id": "x", "capabilities": ["fs:read", "network:all"] }"#,
        );
        let manifest = read_manifest(&dir).expect("still loads");
        let reason = manifest.unavailable.expect("greyed");
        assert!(reason.contains("network:all"), "{reason}");
        assert!(
            reason.contains("process:spawn"),
            "the known list is in the sentence: {reason}"
        );
        assert_eq!(
            manifest.capabilities,
            vec![Capability::FsRead],
            "the ones cide does understand are still recorded"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// And in the other direction: an unknown *key* only warns, because a manifest written for a
    /// newer cide should still install what this one understands.
    #[test]
    fn an_unknown_key_warns_and_names_itself() {
        let dir = scratch("key");
        plant(&dir, r#"{ "id": "x", "contributes": { "langauges": [] } }"#);
        let manifest = read_manifest(&dir).expect("loads");
        assert!(
            manifest.unavailable.is_none(),
            "a typo must not stop an install"
        );
        let problem = manifest
            .problems
            .iter()
            .find(|p| p.message.contains("contributes.langauges"))
            .expect("named");
        assert_eq!(problem.severity, ExtSeverity::Warning);
        assert!(
            problem.message.contains("languages"),
            "and says what was expected"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_future_schema_is_refused_with_a_sentence_about_the_schema() {
        let dir = scratch("schema");
        plant(&dir, r#"{ "schema": 99, "id": "x" }"#);
        let problem = read_manifest(&dir).expect_err("refused");
        assert!(problem.message.contains("schema 99"), "{}", problem.message);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_syntax_error_carries_its_line() {
        let dir = scratch("syntax");
        plant(&dir, "{\n  \"id\": \"x\",\n  oops\n}\n");
        let problem = read_manifest(&dir).expect_err("refused");
        assert_eq!(problem.line, Some(3), "{problem:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Servers are processes, and a process is a permission the user grants at install.
    #[test]
    fn a_server_without_process_spawn_is_dropped_and_the_rest_survives() {
        let dir = scratch("server");
        plant(
            &dir,
            r#"{
              "id": "yaml",
              "contributes": {
                "languages": [ { "id": "yaml", "label": "YAML",
                                 "extensions": [ { "ext": "yaml" } ],
                                 "grammar": { "name": "YAML" } } ],
                "languageServers": [ { "binary": "yaml-language-server",
                                       "languageIds": ["yaml"],
                                       "projectKind": "any",
                                       "installHint": "npm i -g yaml-language-server" } ]
              }
            }"#,
        );
        let manifest = read_manifest(&dir).expect("loads");
        assert!(manifest.contributes.language_servers.is_empty());
        assert_eq!(
            manifest.contributes.languages.len(),
            1,
            "dropping the language too would punish the half that worked"
        );
        assert!(
            manifest
                .problems
                .iter()
                .any(|p| p.message.contains("process:spawn"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A server may enrich a *builtin* language — that is how JS/TS gets an LSP at all, since
    /// re-declaring `typescript` would displace the builtin grammar. The warning exists for a
    /// language nothing supplies, and a builtin id is supplied by cide itself.
    #[test]
    fn a_server_for_a_builtin_language_earns_no_warning_and_an_unknown_one_still_does() {
        let dir = scratch("builtin-server");
        plant(
            &dir,
            r#"{
              "id": "web", "capabilities": ["process:spawn"],
              "contributes": { "languageServers": [
                { "binary": "typescript-language-server", "args": ["--stdio"],
                  "languageIds": ["typescript"],
                  "projectKind": "TypeScript project",
                  "installHint": "npm i -g typescript-language-server typescript" },
                { "binary": "imaginary-ls", "languageIds": ["imaginary"],
                  "projectKind": "any", "installHint": "-" } ] }
            }"#,
        );
        let manifest = read_manifest(&dir).expect("loads");
        assert_eq!(
            manifest.contributes.language_servers.len(),
            2,
            "both are declared; ownership of the language decides warnings, not registration"
        );
        assert!(
            !manifest
                .problems
                .iter()
                .any(|p| p.message.contains("typescript")),
            "a builtin id is supplied by cide itself, and \"it will only run if something \
             else does\" would be a permanent falsehood: {:?}",
            manifest.problems
        );
        assert!(
            manifest
                .problems
                .iter()
                .any(|p| p.message.contains("`imaginary`")),
            "the warning still fires for a language nothing supplies"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_binary_with_a_path_separator_is_refused() {
        let dir = scratch("binary");
        plant(
            &dir,
            r#"{
              "id": "x", "capabilities": ["process:spawn"],
              "contributes": { "languageServers": [
                { "binary": "/usr/bin/evil", "languageIds": ["x"],
                  "projectKind": "any", "installHint": "-" } ] }
            }"#,
        );
        let manifest = read_manifest(&dir).expect("loads");
        assert!(manifest.contributes.language_servers.is_empty());
        assert!(
            manifest
                .problems
                .iter()
                .any(|p| p.message.contains("bare program name"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A path from a manifest never leaves the repository. Checked on the string, before any
    /// filesystem call, because `Path::join` with an absolute right-hand side silently discards
    /// the left one.
    /// The refusals a setting definition can earn, and the one that only warns.
    #[test]
    fn a_setting_definition_is_checked_against_its_own_kind() {
        let dir = scratch("settings");
        plant(
            &dir,
            r#"{
              "id": "x", "main": "main.js",
              "contributes": { "settings": [
                { "id": "good", "label": "Good", "kind": { "type": "toggle", "default": true } },
                { "id": "nolabel", "label": "  ", "kind": { "type": "toggle", "default": true } },
                { "id": "empty-choice", "label": "E", "kind": { "type": "choice", "default": "a", "choices": [] } },
                { "id": "stray-default", "label": "S", "kind": { "type": "choice", "default": "z",
                    "choices": [ { "value": "a" } ] } },
                { "id": "backwards", "label": "B", "kind": { "type": "number", "default": 5, "min": 10, "max": 2 } },
                { "id": "outside", "label": "O", "kind": { "type": "number", "default": 5, "min": 10, "max": 20 } },
                { "id": "good", "label": "Twice", "kind": { "type": "toggle", "default": false } }
              ] }
            }"#,
        );
        let manifest = read_manifest(&dir).expect("loads");
        let kept: Vec<&str> = manifest
            .contributes
            .settings
            .iter()
            .map(|s| s.id.as_str())
            .collect();
        assert_eq!(
            kept,
            vec!["good", "outside"],
            "a broken definition is dropped and the rest survive; a default outside its own band \
             only warns, because `coerce` clamps it and the setting works"
        );
        let said = |needle: &str| manifest.problems.iter().any(|p| p.message.contains(needle));
        assert!(said("no label"), "{:?}", manifest.problems);
        assert!(said("choice with no choices"));
        assert!(said("is not one of its choices"));
        assert!(said("no value is legal"));
        assert!(said("reads as 10"), "the clamped value is named");
        assert!(said("declared twice"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `coerce` is the one place a value is decided, so every road to one gets the same answer.
    #[test]
    fn a_stored_value_is_coerced_against_the_kind_it_was_declared_with() {
        use cide_ipc::ext::{SettingChoice, SettingDef, SettingKind};
        use serde_json::json;

        let toggle = SettingDef {
            id: "t".into(),
            label: "T".into(),
            description: None,
            kind: SettingKind::Toggle { default: true },
        };
        assert_eq!(
            toggle.coerce(None),
            json!(true),
            "absent reads as the default"
        );
        assert_eq!(toggle.coerce(Some(&json!(false))), json!(false));
        assert_eq!(
            toggle.coerce(Some(&json!("no"))),
            json!(true),
            "a hand-edited file holding the wrong type falls back rather than reaching a worker"
        );

        let number = SettingDef {
            id: "n".into(),
            label: "N".into(),
            description: None,
            kind: SettingKind::Number {
                default: 50.0,
                min: Some(1.0),
                max: Some(100.0),
            },
        };
        assert_eq!(
            number.coerce(Some(&json!(9000))),
            json!(100.0),
            "clamped up"
        );
        assert_eq!(number.coerce(Some(&json!(-3))), json!(1.0), "and down");
        assert_eq!(
            number.coerce(Some(&json!("50"))),
            json!(50.0),
            "a string is not a number"
        );

        let choice = SettingDef {
            id: "c".into(),
            label: "C".into(),
            description: None,
            kind: SettingKind::Choice {
                default: "a".into(),
                choices: vec![
                    SettingChoice {
                        value: "a".into(),
                        label: None,
                    },
                    SettingChoice {
                        value: "b".into(),
                        label: Some("Bee".into()),
                    },
                ],
            },
        };
        assert_eq!(choice.coerce(Some(&json!("b"))), json!("b"));
        assert_eq!(
            choice.coerce(Some(&json!("gone"))),
            json!("a"),
            "a value naming a choice that no longer exists — which is what an update that \
             narrowed the list leaves behind — falls back rather than being handed on"
        );
    }

    #[test]
    fn a_relative_path_cannot_escape_its_root() {
        let root = Path::new("/srv/clone");
        assert_eq!(
            safe_relative(root, "extensions/sql"),
            Some(PathBuf::from("/srv/clone/extensions/sql"))
        );
        for attempt in [
            "../etc",
            "/etc",
            "extensions/../../etc",
            "C:\\Windows",
            "a\\b",
        ] {
            assert!(safe_relative(root, attempt).is_none(), "{attempt}");
        }
    }

    /// The rail renders this value into an attribute, so it is a whitelist rather than an escape.
    #[test]
    fn an_icon_is_a_path_and_nothing_else() {
        assert!(is_svg_path("M4 4h16v16H4z"));
        assert!(is_svg_path("M12 2 L2 7 l10 5 10-5z"));
        assert!(!is_svg_path("\"/><script>alert(1)</script>"));
        assert!(!is_svg_path("url(#x)"));
        assert!(!is_svg_path(""));
    }

    /// The key tables above are hand-written; this is what keeps them true.
    ///
    /// Serialising a value is the cheapest reflection available without a derive. Every optional
    /// field is filled in, and that is not tidiness: these types carry
    /// `skip_serializing_if = "Option::is_none"`, so a default value omits exactly the keys the
    /// table most needs to cover. A table that drifted would warn about a key that works — which
    /// sends the author to fix something that is not broken, and is worse than not warning at
    /// all.
    #[test]
    fn unknown_key_tables_are_complete() {
        fn keys(value: &serde_json::Value) -> Vec<String> {
            value
                .as_object()
                .expect("an object")
                .keys()
                .cloned()
                .collect()
        }
        let check = |name: &str, table: &[&str], value: serde_json::Value| {
            let mut actual = keys(&value);
            actual.sort();
            let mut expected: Vec<String> = table.iter().map(ToString::to_string).collect();
            expected.sort();
            assert_eq!(actual, expected, "{name}");
        };
        check(
            "LANGUAGE_KEYS",
            LANGUAGE_KEYS,
            serde_json::to_value(cide_ipc::lang::LanguageDef {
                id: String::new(),
                label: String::new(),
                extensions: vec![],
                filenames: vec![],
                fence_aliases: vec![],
                grammar: cide_ipc::lang::GrammarSpecDto::default(),
                fold: cide_ipc::lang::FoldSpecDto::default(),
                scratch: vec![],
            })
            .expect("serialise"),
        );
        check(
            "SERVER_KEYS",
            SERVER_KEYS,
            serde_json::to_value(cide_ipc::lang::LanguageServerDef {
                binary: String::new(),
                args: vec![],
                language_ids: vec![],
                project_markers: vec![],
                project_kind: String::new(),
                install_hint: String::new(),
                declares_watched_files: false,
                extra_path_hints: vec![],
                init_options: None,
            })
            .expect("serialise"),
        );
        check(
            "PANEL_KEYS",
            PANEL_KEYS,
            serde_json::to_value(cide_ipc::ext::PanelDef {
                id: String::new(),
                label: String::new(),
                icon: Some(String::new()),
                location: PanelLocation::Sidebar,
            })
            .expect("serialise"),
        );
        check(
            "SETTING_KEYS",
            SETTING_KEYS,
            serde_json::to_value(cide_ipc::ext::SettingDef {
                id: String::new(),
                label: String::new(),
                description: Some(String::new()),
                kind: cide_ipc::ext::SettingKind::Toggle { default: false },
            })
            .expect("serialise"),
        );
        check(
            "COMMAND_KEYS",
            COMMAND_KEYS,
            serde_json::to_value(cide_ipc::ext::ExtCommandDef {
                id: String::new(),
                title: String::new(),
                keywords: vec![],
            })
            .expect("serialise"),
        );
        check(
            "GRAMMAR_KEYS",
            GRAMMAR_KEYS,
            serde_json::to_value(cide_ipc::lang::GrammarSpecDto {
                line_comment: Some(String::new()),
                block_comment: Some((String::new(), String::new())),
                quotes: Some(String::new()),
                escapes: Some(true),
                identifier_extra: Some(String::new()),
                control_operators: Some(String::new()),
                ..cide_ipc::lang::GrammarSpecDto::default()
            })
            .expect("serialise"),
        );
        check(
            "FOLD_KEYS",
            FOLD_KEYS,
            serde_json::to_value(cide_ipc::lang::FoldSpecDto {
                line_comment: Some(String::new()),
                block_comment: Some((String::new(), String::new())),
                nested_comments: Some(false),
                quotes: Some(String::new()),
                escapes: Some(true),
                triple_quotes: Some(false),
                brackets: Some(String::new()),
                multiline_quotes: Some(String::new()),
                extra_quotes: Some(String::new()),
                ..cide_ipc::lang::FoldSpecDto::default()
            })
            .expect("serialise"),
        );
    }

    #[test]
    fn an_index_refuses_a_row_that_leaves_the_repository() {
        let clone = scratch("index");
        std::fs::write(
            clone.join(MARKET_FILE),
            r#"{ "schema": 1, "name": "m",
                 "extensions": [ { "id": "ok" }, { "id": "bad", "path": "../../etc" } ] }"#,
        )
        .expect("plant");
        let index = read_index(&clone);
        assert_eq!(index.rows.len(), 1);
        assert_eq!(index.rows[0].dir, clone.join("extensions").join("ok"));
        assert!(
            index
                .problems
                .iter()
                .any(|p| p.message.contains("leaves the repository"))
        );
        let _ = std::fs::remove_dir_all(&clone);
    }

    /// A missing index is a marketplace with a problem, never an error that stops a launch.
    #[test]
    fn a_repository_with_no_index_is_empty_and_says_why() {
        let clone = scratch("noindex");
        let index = read_index(&clone);
        assert!(index.rows.is_empty());
        assert!(index.problems[0].message.contains(MARKET_FILE));
        let _ = std::fs::remove_dir_all(&clone);
    }
}
