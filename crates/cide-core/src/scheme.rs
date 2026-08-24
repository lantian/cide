//! Importing a VS Code colour theme, and keeping the imported ones on disk. (M24)
//!
//! # What is actually being translated
//!
//! A VS Code theme answers a different question from the one cide asks. It maps **TextMate
//! scopes** — `support.type.property-name.json`, `entity.name.function` — to colours, and a
//! scope is produced by a grammar that cide does not run. cide's editor colours a closed set of
//! *roles* (`cide_ipc::theme::SCHEME_TOKENS`), reached from `@lezer/highlight` tags.
//!
//! So the conversion is a **reverse lookup**: for each of cide's roles, ask the theme what it
//! would have painted a representative scope, and take that colour. [`SCOPES`] is the whole
//! table, and it is the part of this file a future reader will want to edit.
//!
//! # Why this is in Rust and not in the webview
//!
//! Two reasons, and the second is the one that decided it. The frontend is glue — only
//! `cide-app` may link tauri, and a converter is domain logic by any reading. And a converter in
//! TypeScript could not be reached by `cargo test`: the interesting behaviour here is a scope
//! matcher with precedence rules, which is exactly the sort of thing that needs a table of cases
//! rather than a screenshot.
//!
//! # The file is read once
//!
//! What is stored is the **converted** scheme, not the source theme. Re-converting on every
//! launch would mean that a change to [`SCOPES`] silently repaints a buffer somebody was happy
//! with, months after they imported it — a class of surprise this project has been bitten by
//! before with defaults that reached values already on disk. Re-importing is how a user opts
//! into an improved table, and [`ColorScheme::source`] is what tells them where to find the file
//! again.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use cide_ipc::Theme;
use cide_ipc::theme::{
    BUILTIN_SCHEME, ColorScheme, SCHEME_SURFACE, luminance, normalise_hex, scheme_roles,
};
use serde::Deserialize;

use crate::{CoreError, Result, persist};

/// For each of cide's roles, the TextMate scopes that stand for it, most representative first.
///
/// **Order inside a row is preference, not specificity.** The first candidate that the theme has
/// anything to say about wins, so a row runs from the scope that means exactly this role to the
/// broadest one that would still be an acceptable answer. `constant.language` before `constant`
/// is the shape: a theme that separates `true`/`null` from numeric constants is telling us
/// something, and a theme that does not still answers with its general constant colour.
///
/// Surface roles are absent — they come from the theme's `colors` map, not from `tokenColors`.
/// Roles absent here (there are none today, but a role added later may be) fall through
/// [`cide_ipc::theme::fallback`] like any other gap.
const SCOPES: &[(&str, &[&str])] = &[
    (
        "comment",
        &["comment.line", "comment", "punctuation.definition.comment"],
    ),
    (
        "doc",
        &["comment.block.documentation", "comment.documentation"],
    ),
    (
        "keyword",
        &["keyword.control", "keyword", "storage.type", "storage"],
    ),
    // `invalid` is what a theme uses for a lexer error, which is what cide's `control` role also
    // covers beside `?`. Asking for the control keyword first keeps `?` in the keyword family
    // for the majority of themes, which have no `invalid` colour at all.
    (
        "control",
        &["keyword.control.flow", "invalid", "keyword.control"],
    ),
    (
        "constant",
        &["constant.language", "support.constant", "constant"],
    ),
    ("number", &["constant.numeric", "constant"]),
    ("string", &["string.quoted", "string"]),
    (
        "escape",
        &["constant.character.escape", "constant.character"],
    ),
    ("regexp", &["string.regexp", "string.quoted"]),
    (
        "type",
        &[
            "entity.name.type",
            "support.type",
            "entity.name.class",
            "storage.type",
        ],
    ),
    (
        "namespace",
        &["entity.name.namespace", "entity.name.type.module"],
    ),
    (
        "function",
        &[
            "entity.name.function",
            "support.function",
            "meta.function-call",
        ],
    ),
    (
        "macro",
        &["entity.name.function.macro", "support.macro", "meta.macro"],
    ),
    (
        "label",
        &[
            "entity.name.label",
            "constant.other.label",
            "entity.name.tag",
        ],
    ),
    // The JSON key, and the reason this whole change exists. `support.type.property-name` is
    // JSON's own scope in the standard grammar; `meta.object-literal.key` is JavaScript's.
    (
        "property",
        &[
            "support.type.property-name",
            "meta.object-literal.key",
            "variable.other.property",
            "variable.other.member",
        ],
    ),
    (
        "variable",
        &["variable.other.readwrite", "variable.other", "variable"],
    ),
    (
        "attribute",
        &[
            "entity.other.attribute-name",
            "meta.attribute",
            "meta.tag.attribute-name",
        ],
    ),
    ("operator", &["keyword.operator", "punctuation.separator"]),
    (
        "punctuation",
        &["punctuation.separator", "punctuation", "meta.delimiter"],
    ),
    (
        "bracket",
        &[
            "punctuation.definition.bracket",
            "meta.brace",
            "punctuation.section",
            "punctuation",
        ],
    ),
    ("heading", &["markup.heading", "entity.name.section"]),
    ("strong", &["markup.bold"]),
    ("emphasis", &["markup.italic", "markup.quote"]),
    (
        "link",
        &["markup.underline.link", "markup.link", "string.other.link"],
    ),
];

/// Which key in the theme's `colors` map fills each surface role.
///
/// `editor.lineHighlightBackground` is **deliberately absent**, and it is the one thing a reader
/// comparing this table against VS Code's own will notice missing. The caret's line is derived
/// from the selection at 40% rather than taken from the theme, because it paints *above* the
/// selection layer — see `EditorSurface.module.css`, which spends thirty lines on why anything
/// opaque there hides the selection on the caret's row. Dropping an alpha channel turns every
/// theme's value into exactly such an opaque colour.
///
/// The second entry of a pair is a fallback key, not a second choice of meaning: `editorCursor`
/// and `editor.selectionHighlightBackground` are the keys themes most often use when the
/// primary one is absent.
const SURFACE_KEYS: &[(&str, &[&str])] = &[
    ("bg", &["editor.background"]),
    ("fg", &["editor.foreground", "foreground"]),
    (
        "sel",
        &[
            "editor.selectionBackground",
            "editor.inactiveSelectionBackground",
        ],
    ),
    (
        "gutter",
        &["editorLineNumber.foreground", "editorGutter.foreground"],
    ),
    ("caret", &["editorCursor.foreground", "editor.foreground"]),
];

// --- the source format ------------------------------------------------------------------------

/// The parts of a VS Code theme this reads. Everything else in the file is ignored.
///
/// Deliberately **not** `deny_unknown_fields`, which is the opposite of the rule the wire DTOs
/// follow, and for the opposite reason: those types describe a contract between two halves of
/// this application, where an unknown field means drift. This one describes a file written by
/// somebody else's tool, where an unknown field means they used a feature we do not need.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VsTheme {
    #[serde(default)]
    name: Option<String>,
    /// `"dark"`, `"light"`, `"hc"` / `"hcDark"` / `"hcLight"`. Absent in plenty of real themes,
    /// which is why [`polarity_of`] can fall back to the background's luminance.
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    colors: BTreeMap<String, String>,
    /// Absent, an array, or — in a handful of older themes — a *string* naming a `.tmTheme`
    /// file beside it. The string form deserialises to `None` here rather than failing the whole
    /// import, so such a theme still yields its `colors` and a scheme built from the fallback
    /// chain. That is a worse scheme than the file could give, and it is a great deal better
    /// than an error dialog.
    #[serde(default, deserialize_with = "token_colors")]
    token_colors: Vec<TokenColor>,
    /// Semantic colours, which VS Code applies *over* `tokenColors` when a language server
    /// supplies semantic tokens. cide requests none (`cide-lsp` sends no `semanticTokens`), so
    /// they are read only as a last resort — see [`resolve_role`].
    #[serde(default)]
    semantic_token_colors: BTreeMap<String, SemanticValue>,
}

/// `"#rrggbb"` or `{ "foreground": "#rrggbb", … }`. Both forms are legal in the same file.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SemanticValue {
    Color(String),
    Settings { foreground: Option<String> },
}

#[derive(Debug, Deserialize)]
struct TokenColor {
    #[serde(default)]
    scope: Scope,
    #[serde(default)]
    settings: TokenSettings,
}

#[derive(Debug, Default, Deserialize)]
struct TokenSettings {
    #[serde(default)]
    foreground: Option<String>,
}

/// A rule's scope: one selector, a comma-separated list of them, or an array.
#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
enum Scope {
    One(String),
    Many(Vec<String>),
    #[default]
    None,
}

impl Scope {
    /// Every selector this rule carries, with the comma lists already split.
    fn selectors(&self) -> Vec<&str> {
        // A free function rather than a closure: a closure's two elided lifetimes are inferred
        // independently, so the borrow from `self` cannot be shown to outlive the return.
        fn split(s: &str) -> impl Iterator<Item = &str> {
            s.split(',').map(str::trim).filter(|p| !p.is_empty())
        }
        match self {
            Scope::One(s) => split(s).collect(),
            Scope::Many(list) => list.iter().flat_map(|s| split(s)).collect(),
            Scope::None => Vec::new(),
        }
    }
}

fn token_colors<'de, D>(de: D) -> std::result::Result<Vec<TokenColor>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Either {
        List(Vec<TokenColor>),
        /// A path to a `.tmTheme`. Accepted and discarded — see the field's own note. The
        /// payload is `IgnoredAny` rather than `String` so nothing reads it by accident, and so
        /// this arm does not have to justify a field it never looks at.
        Path(serde::de::IgnoredAny),
    }
    Ok(match Option::<Either>::deserialize(de)? {
        Some(Either::List(list)) => list,
        _ => Vec::new(),
    })
}

// --- matching ---------------------------------------------------------------------------------

/// How well a theme rule's `selector` answers a question about `candidate`.
///
/// `None` means it does not. Higher is better.
///
/// The **first** rule is TextMate's own: a selector matches a scope when it is a dot-segment
/// prefix of it, and the more segments it shares the more specific it is. `entity.name` answers
/// a question about `entity.name.function`; `entity.name.function.js` does not, because it
/// describes a strictly narrower thing.
///
/// The **second** is a deliberate departure, and it is why the scores are an order of magnitude
/// apart. A great many real themes only ever name language-qualified scopes —
/// `entity.name.function.js`, `support.type.property-name.json` — and never the bare one. Under
/// the strict rule alone those themes convert to almost nothing, because cide asks its questions
/// in the general form. So a selector that sits *under* the candidate is accepted as weak
/// evidence: it is not what the token would be painted, but it is unambiguously what this theme
/// thinks that family of things looks like, which is the question actually being asked here.
///
/// A selector with a descendant part (`meta.function entity.name`) is judged on its **last**
/// component, which is the element such a selector actually matches.
fn score(selector: &str, candidate: &str) -> Option<u32> {
    let leaf = selector.split_whitespace().next_back()?;
    // `-` introduces an exclusion (`source -string`), which cannot be honoured out of context.
    // A rule whose selector is only an exclusion says nothing about any candidate.
    let leaf = leaf.trim_start_matches('&');
    if leaf.is_empty() || leaf.starts_with('-') {
        return None;
    }
    let segments = |s: &str| s.split('.').count() as u32;
    if candidate == leaf {
        return Some(1000 + segments(leaf) * 10);
    }
    if candidate.starts_with(leaf) && candidate.as_bytes().get(leaf.len()) == Some(&b'.') {
        return Some(100 + segments(leaf) * 10);
    }
    if leaf.starts_with(candidate) && leaf.as_bytes().get(candidate.len()) == Some(&b'.') {
        // Weak evidence, and deliberately scored below every prefix match — see above.
        return Some(10);
    }
    None
}

/// The colour this theme would paint `candidate`, or `None`.
///
/// Ties go to the **later** rule, which is VS Code's own precedence: a theme that restates a
/// scope further down the file is correcting itself. `>=` rather than `>` is that rule.
fn colour_for(theme: &VsTheme, candidate: &str) -> Option<String> {
    let mut best: Option<(u32, String)> = None;
    for rule in &theme.token_colors {
        let Some(fg) = rule.settings.foreground.as_deref() else {
            continue;
        };
        let Some(hex) = normalise_hex(fg) else {
            continue;
        };
        for selector in rule.scope.selectors() {
            let Some(points) = score(selector, candidate) else {
                continue;
            };
            if best.as_ref().is_none_or(|(seen, _)| points >= *seen) {
                best = Some((points, hex.clone()));
            }
        }
    }
    best.map(|(_, hex)| hex)
}

/// One role's colour, from `tokenColors` first and `semanticTokenColors` as a last resort.
fn resolve_role(theme: &VsTheme, role: &str, candidates: &[&str]) -> Option<String> {
    for candidate in candidates {
        if let Some(hex) = colour_for(theme, candidate) {
            return Some(hex);
        }
    }
    // A theme that is *only* semantic — rare, but they exist, and they convert to nothing at all
    // without this. The key set is a different vocabulary from TextMate's and happens to line up
    // with cide's role names for the handful that matter.
    let semantic = theme.semantic_token_colors.get(role)?;
    let raw = match semantic {
        SemanticValue::Color(hex) => hex.as_str(),
        SemanticValue::Settings { foreground } => foreground.as_deref()?,
    };
    normalise_hex(raw)
}

/// `dark` or `light`, from the theme's own declaration or from what it paints behind text.
fn polarity_of(theme: &VsTheme) -> Theme {
    if let Some(kind) = theme.kind.as_deref() {
        let kind = kind.to_ascii_lowercase();
        if kind.contains("light") {
            return Theme::Light;
        }
        if kind.contains("dark") {
            return Theme::Dark;
        }
    }
    // No declaration. The background is the honest tiebreak — it is the largest area of the
    // screen the scheme is responsible for.
    theme
        .colors
        .get("editor.background")
        .and_then(|hex| luminance(hex))
        .map(|l| if l > 0.5 { Theme::Light } else { Theme::Dark })
        .unwrap_or(Theme::Dark)
}

/// A filename-safe id, derived from the theme's display name.
///
/// Not a hash and not a counter: the id is what `EditorSettings::color_scheme_*` stores and what
/// the user sees in `~/.config/cide/schemes/`, so re-importing an updated copy of the same theme
/// must land on the same id and replace it rather than accumulating `one-dark-2`.
fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.extend(ch.to_lowercase());
        } else {
            pending_dash = true;
        }
    }
    if out.is_empty() {
        // Every other branch of this function is about being recognisable; this one is about
        // still producing a legal filename when the name was entirely non-ASCII.
        out.push_str("imported");
    }
    // `cide` is the compiled-in scheme and is not a file. A theme genuinely called "cide" would
    // otherwise overwrite the picker's own default entry.
    if out == BUILTIN_SCHEME {
        out.push_str("-imported");
    }
    out
}

// --- the public road --------------------------------------------------------------------------

/// Convert a parsed VS Code theme into a cide scheme. Total: it always answers.
pub fn convert(raw: &str, name_hint: &str, source: Option<String>) -> Result<ColorScheme> {
    let theme: VsTheme = serde_json::from_str(&strip_jsonc(raw))
        .map_err(|e| CoreError::Serde(format!("not a VS Code colour theme: {e}")))?;

    let mut colors = BTreeMap::new();
    for (role, keys) in SURFACE_KEYS {
        if let Some(hex) = keys
            .iter()
            .find_map(|key| theme.colors.get(*key).and_then(|v| normalise_hex(v)))
        {
            colors.insert((*role).to_string(), hex);
        }
    }
    for (role, candidates) in SCOPES {
        if let Some(hex) = resolve_role(&theme, role, candidates) {
            colors.insert((*role).to_string(), hex);
        }
    }

    let name = theme
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| name_hint.to_string());
    let mut scheme = ColorScheme {
        id: slug(&name),
        name,
        polarity: polarity_of(&theme),
        colors,
        source,
    };
    // The one call that makes the map total and closed. Everything above is allowed to be
    // sparse; nothing downstream is.
    scheme.normalise();
    Ok(scheme)
}

/// Strip `//` and `/* */` comments, and trailing commas, from a theme file.
///
/// VS Code reads its own themes as JSONC and a surprising number of published themes rely on it.
/// `serde_json` does not, so a comment is the difference between a theme importing and an error
/// dialog. Written as a character scan rather than a regex because a `//` inside a string
/// literal — every colour is a `#`-prefixed string, and URLs appear in `name` fields — must not
/// be treated as a comment.
fn strip_jsonc(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => {
                in_string = true;
                out.push(ch);
            }
            '/' if chars.peek() == Some(&'/') => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = '\0';
                for next in chars.by_ref() {
                    if prev == '*' && next == '/' {
                        break;
                    }
                    prev = next;
                }
                // A space, not nothing: `1/**/2` is two tokens and must not become `12`.
                out.push(' ');
            }
            _ => out.push(ch),
        }
    }
    // Trailing commas, once the comments that could have hidden them are gone.
    let mut cleaned = String::with_capacity(out.len());
    let bytes: Vec<char> = out.chars().collect();
    let mut in_string = false;
    let mut escaped = false;
    for (at, ch) in bytes.iter().copied().enumerate() {
        if in_string {
            cleaned.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            cleaned.push(ch);
            continue;
        }
        if ch == ',' {
            let next = bytes[at + 1..].iter().copied().find(|c| !c.is_whitespace());
            if matches!(next, Some(']') | Some('}')) {
                continue;
            }
        }
        cleaned.push(ch);
    }
    cleaned
}

/// The most a single archive entry may decompress to, and the most themes one `.vsix` may carry.
///
/// A colour theme is a few kilobytes of JSON and a manifest smaller than that, so both bounds sit
/// two orders of magnitude above anything real. They are here because a `.vsix` is an *archive*
/// the user downloaded, and `zip` being correct about the format says nothing about the size of
/// what it hands back: a few hundred bytes of entry can inflate to gigabytes, and a directory
/// listing a thousand themes would run the converter a thousand times. Neither is a likely
/// attack on an IDE; both are a cheap line each.
const MAX_ENTRY_BYTES: u64 = 4 * 1024 * 1024;
const MAX_THEMES: usize = 32;

/// Read a colour theme from disk. A `.vsix` may carry several; a `.json` is always one.
///
/// **Dispatched on the file's first four bytes, not on its extension.** `PK\x03\x04` is a ZIP,
/// whatever it is called — and a theme file is JSON, whatever *it* is called. Real files arrive
/// both ways: a `.vsix` renamed to `.zip` by a browser, a theme saved as `.jsonc`, a
/// `-color-theme.json` with no extension left after a download manager finished with it. The
/// extension is a hint the user did not necessarily control; the magic number is the fact.
pub fn import(path: &Path) -> Result<Vec<ColorScheme>> {
    let bytes =
        fs::read(path).map_err(|e| CoreError::Io(format!("reading {}: {e}", path.display())))?;
    if bytes.starts_with(b"PK\x03\x04") {
        return import_vsix(path, bytes);
    }
    let raw = String::from_utf8(bytes)
        .map_err(|_| CoreError::Serde(format!("{} is not text", path.display())))?;
    let hint = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Imported".into());
    Ok(vec![convert(
        &raw,
        &hint,
        Some(path.to_string_lossy().into_owned()),
    )?])
}

/// One row of a VS Code extension manifest's `contributes.themes`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThemeContribution {
    /// The name shown in VS Code's own picker. Preferred over the theme file's `name`, because a
    /// pack's files are often called `dark.json` while the label says *Min Dark*.
    #[serde(default)]
    label: Option<String>,
    /// Relative to the manifest, so relative to `extension/` inside the archive.
    path: String,
}

#[derive(Debug, Deserialize)]
struct VsixManifest {
    #[serde(default)]
    contributes: VsixContributes,
}

#[derive(Debug, Default, Deserialize)]
struct VsixContributes {
    #[serde(default)]
    themes: Vec<ThemeContribution>,
}

/// Pull every colour theme out of a `.vsix`.
///
/// A `.vsix` is a plain ZIP with the extension rooted at `extension/`, so the road is: read
/// `extension/package.json`, follow `contributes.themes[].path`, convert each.
///
/// **Every theme in the package, not one.** Themes ship in light/dark pairs far more often than
/// not — the file that prompted this carries *Min Dark* and *Min Light* — and the setting is
/// keyed by polarity, so importing one of a pair leaves the other theme on `cide` and the user
/// back at the file dialog. One gesture, both variants, and the caller selects whichever matches
/// the window it is showing.
///
/// A package that declares no themes falls back to scanning `extension/themes/` for `.json`
/// files. That is not a guess at what the user meant: it is the convention every published theme
/// follows, and the alternative — refusing a file that visibly contains themes because its
/// manifest is shaped unusually — is the sort of refusal that reads as a broken importer.
fn import_vsix(path: &Path, bytes: Vec<u8>) -> Result<Vec<ColorScheme>> {
    let source = path.to_string_lossy().into_owned();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| {
        CoreError::Serde(format!("{} is not a readable .vsix: {e}", path.display()))
    })?;

    // The manifest names the themes, but it is allowed to be absent or shaped oddly — see above.
    let declared: Vec<ThemeContribution> = match read_entry(&mut archive, "extension/package.json")
    {
        Some(raw) => serde_json::from_str::<VsixManifest>(&strip_jsonc(&raw))
            .map(|m| m.contributes.themes)
            .unwrap_or_default(),
        None => Vec::new(),
    };

    let mut wanted: Vec<(String, Option<String>)> = declared
        .into_iter()
        .map(|t| (join_entry("extension", &t.path), t.label))
        .collect();
    if wanted.is_empty() {
        // `file_names` order is the archive's, which is not guaranteed stable across writers, so
        // the list is sorted — two imports of one file must produce the same ids in the same
        // order or the caller's "select the first match" becomes a coin toss.
        let mut found: Vec<String> = archive
            .file_names()
            .filter(|name| {
                name.starts_with("extension/themes/")
                    && name.to_ascii_lowercase().ends_with(".json")
            })
            .map(str::to_string)
            .collect();
        found.sort();
        wanted = found.into_iter().map(|name| (name, None)).collect();
    }

    let mut schemes = Vec::new();
    for (entry, label) in wanted.into_iter().take(MAX_THEMES) {
        let Some(raw) = read_entry(&mut archive, &entry) else {
            // A manifest naming a file the archive does not contain is that package's bug, and it
            // must not cost the themes beside it. Logged rather than returned for `load_all`'s
            // reason: a partial answer is worth more here than a total failure.
            tracing::warn!(entry, "a .vsix names a theme file it does not contain");
            continue;
        };
        let hint = label.unwrap_or_else(|| {
            std::path::Path::new(&entry)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Imported".into())
        });
        match convert(&raw, &hint, Some(format!("{source}!{entry}"))) {
            Ok(mut scheme) => {
                // The manifest's label beats the theme file's own `name`. A pack's files often say
                // `"name": "Default Dark+"` inside while the label the user recognises — the one
                // VS Code's picker shows — is in the manifest.
                if let Some(label) = declared_label(&scheme, &hint) {
                    scheme.name = label;
                    scheme.id = slug(&scheme.name);
                }
                schemes.push(scheme)
            }
            Err(error) => tracing::warn!(entry, %error, "skipping an unreadable theme in a .vsix"),
        }
    }

    if schemes.is_empty() {
        return Err(CoreError::Serde(format!(
            "{} contains no colour themes — looked for contributes.themes in \
             extension/package.json, then for extension/themes/*.json",
            path.display()
        )));
    }
    Ok(schemes)
}

/// The name to use, when the packaging says something the theme file does not.
///
/// Returns `None` when the hint is what `convert` already chose, so an unlabelled theme keeps its
/// own `name` and its id does not churn.
fn declared_label(scheme: &ColorScheme, hint: &str) -> Option<String> {
    (scheme.name != hint && !hint.trim().is_empty()).then(|| hint.to_string())
}

/// One entry's contents as text, or `None` if it is absent, too large, or not UTF-8.
///
/// Looked up case-insensitively on a second pass. ZIP entry names are byte strings and a handful
/// of packagers on case-insensitive filesystems write `Extension/Package.json`; VS Code reads
/// those, so refusing them would be refusing files that work everywhere else.
fn read_entry<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<String> {
    let exact = archive.index_for_name(name);
    let index = exact.or_else(|| {
        let lower = name.to_ascii_lowercase();
        archive
            .file_names()
            .find(|candidate| candidate.to_ascii_lowercase() == lower)
            .map(str::to_string)
            .and_then(|found| archive.index_for_name(&found))
    })?;

    let entry = archive.by_index(index).ok()?;
    if entry.size() > MAX_ENTRY_BYTES {
        tracing::warn!(
            name,
            size = entry.size(),
            "a .vsix entry is too large to be a theme"
        );
        return None;
    }
    // Read through `take` as well as checking `size()`: the header's declared size is data from
    // the archive, and a mismatched one is exactly how a zip bomb is written.
    use std::io::Read as _;
    let mut text = String::new();
    entry.take(MAX_ENTRY_BYTES).read_to_string(&mut text).ok()?;
    Some(text)
}

/// Resolve a manifest-relative theme path against the archive root.
///
/// `./themes/x.json`, `themes/x.json` and `/themes/x.json` all appear in published packages.
/// `..` is dropped rather than followed: nothing here writes a file, so this is not zip-slip —
/// it is that an entry name is a flat string and climbing out of `extension/` can only ever miss.
fn join_entry(root: &str, relative: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in relative.split(['/', '\\']) {
        match part {
            "" | "." | ".." => continue,
            other => parts.push(other),
        }
    }
    format!("{root}/{}", parts.join("/"))
}

/// Where one scheme is stored.
fn path_for(id: &str) -> PathBuf {
    persist::schemes_dir().join(format!("{id}.json"))
}

/// Write a scheme, replacing any previous import with the same id.
pub fn save(scheme: &ColorScheme) -> Result<()> {
    let dir = persist::schemes_dir();
    fs::create_dir_all(&dir)
        .map_err(|e| CoreError::Io(format!("creating {}: {e}", dir.display())))?;
    let json = serde_json::to_vec_pretty(scheme)
        .map_err(|e| CoreError::Serde(format!("serialising a colour scheme: {e}")))?;
    persist::write_atomic(&path_for(&scheme.id), &json)
}

/// Write every scheme an import produced, and answer with the ones that landed.
///
/// A failure on one is logged and skipped rather than returned, so a two-theme package with one
/// unwritable file still gives the user the other. The caller reports what came back; an empty
/// answer is the error case, and `import` has already refused a file with no themes in it.
pub fn save_all(schemes: Vec<ColorScheme>) -> Vec<ColorScheme> {
    schemes
        .into_iter()
        .filter(|scheme| match save(scheme) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(id = %scheme.id, %error, "could not save an imported colour scheme");
                false
            }
        })
        .collect()
}

/// Delete an imported scheme. Removing one that is not there is not an error — the caller's
/// intent is satisfied either way, and a picker showing a stale row is exactly how this is
/// reached.
pub fn remove(id: &str) -> Result<()> {
    match fs::remove_file(path_for(id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(CoreError::Io(format!(
            "removing the {id} colour scheme: {e}"
        ))),
    }
}

/// Every imported scheme, by id.
///
/// **Reads never fail**, on the rule `persist::load_workspace` already follows: this runs during
/// bootstrap, and a scheme file somebody hand-edited into invalid JSON must not be able to stop
/// a window opening. A file that will not parse is skipped and logged; the others still load.
pub fn load_all() -> Vec<ColorScheme> {
    let dir = persist::schemes_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
            tracing::warn!(path = %path.display(), "could not read a colour scheme");
            continue;
        };
        match serde_json::from_str::<ColorScheme>(&raw) {
            Ok(mut scheme) => {
                // Normalised on the way in as well as on import: this file is hand-editable by
                // design, and the applier is allowed to trust that every key is a role.
                scheme.normalise();
                found.push(scheme);
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "skipping an unreadable colour scheme")
            }
        }
    }
    // By display name, which is the order the picker wants and the only one that is stable
    // across machines — `read_dir` is not ordered.
    found.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    found
}

/// The role list, for callers that want to check a scheme is total without importing the DTO.
pub fn roles() -> Vec<&'static str> {
    scheme_roles()
}

/// The surface half of the role list.
pub fn surface_roles() -> &'static [&'static str] {
    SCHEME_SURFACE
}

#[cfg(test)]
mod tests {
    use super::*;

    const THEME: &str = r##"{
      // A comment, because published themes have them.
      "name": "Test Dark",
      "type": "dark",
      "colors": {
        "editor.background": "#1e1e1e",
        "editor.foreground": "#d4d4d4",
        "editor.selectionBackground": "#264f7899",
        "editorLineNumber.foreground": "#858585"
      },
      "tokenColors": [
        { "scope": "comment", "settings": { "foreground": "#6A9955" } },
        { "scope": ["string", "string.quoted"], "settings": { "foreground": "#ce9178" } },
        { "scope": "constant.numeric", "settings": { "foreground": "#b5cea8" } },
        { "scope": "keyword.control, storage.type", "settings": { "foreground": "#c586c0" } },
        { "scope": "support.type.property-name.json", "settings": { "foreground": "#9cdcfe" } },
        { "scope": "entity.name.function", "settings": { "foreground": "#dcdcaa" } },
      ]
    }"##;

    fn converted() -> ColorScheme {
        convert(THEME, "fallback", None).expect("convert")
    }

    #[test]
    fn surface_comes_from_the_colors_map() {
        let s = converted();
        assert_eq!(s.colors["bg"], "#1e1e1e");
        assert_eq!(s.colors["fg"], "#d4d4d4");
        // The alpha is dropped rather than composited.
        assert_eq!(s.colors["sel"], "#264f78");
        assert_eq!(s.colors["gutter"], "#858585");
    }

    #[test]
    fn roles_come_from_token_colors() {
        let s = converted();
        assert_eq!(s.colors["comment"], "#6a9955");
        assert_eq!(s.colors["string"], "#ce9178");
        assert_eq!(s.colors["number"], "#b5cea8");
        assert_eq!(s.colors["keyword"], "#c586c0");
        assert_eq!(s.colors["function"], "#dcdcaa");
    }

    /// The headline: this theme names only `support.type.property-name.json`, one segment
    /// *below* the scope cide asks about. The weak-evidence rule is what makes a JSON key
    /// something other than body text.
    #[test]
    fn a_language_qualified_scope_still_answers() {
        assert_eq!(converted().colors["property"], "#9cdcfe");
    }

    #[test]
    fn gaps_are_filled_from_the_fallback_chain() {
        let s = converted();
        // Nothing in the theme mentions a doc comment, a macro or a bracket.
        assert_eq!(s.colors["doc"], s.colors["comment"]);
        assert_eq!(s.colors["macro"], s.colors["function"]);
        assert_eq!(s.colors["bracket"], s.colors["operator"]);
        for role in roles() {
            assert!(s.colors.contains_key(role), "{role} is missing");
        }
    }

    #[test]
    fn jsonc_comments_and_trailing_commas_survive() {
        // Both appear in `THEME`; that it parsed at all is the assertion. This pins the two
        // cases that a naive `serde_json::from_str` rejects.
        assert_eq!(converted().name, "Test Dark");
        let inline = r##"{ "name": "A // B", "colors": { "editor.background": "#000000" } }"##;
        assert_eq!(convert(inline, "x", None).expect("convert").name, "A // B");
    }

    #[test]
    fn polarity_falls_back_to_the_background() {
        let dark = r##"{ "name": "D", "colors": { "editor.background": "#101014" } }"##;
        let light = r##"{ "name": "L", "colors": { "editor.background": "#fdfdfd" } }"##;
        assert_eq!(convert(dark, "x", None).unwrap().polarity, Theme::Dark);
        assert_eq!(convert(light, "x", None).unwrap().polarity, Theme::Light);
    }

    #[test]
    fn a_later_rule_wins_a_tie() {
        let theme = r##"{ "name": "T", "tokenColors": [
            { "scope": "comment", "settings": { "foreground": "#111111" } },
            { "scope": "comment", "settings": { "foreground": "#222222" } }
        ] }"##;
        assert_eq!(
            convert(theme, "x", None).unwrap().colors["comment"],
            "#222222"
        );
    }

    /// A prefix match must beat the weak under-the-candidate rule regardless of file order,
    /// or a theme that names both would answer with the language-specific one.
    #[test]
    fn a_prefix_match_beats_weak_evidence() {
        let theme = r##"{ "name": "T", "tokenColors": [
            { "scope": "entity.name.function.js", "settings": { "foreground": "#111111" } },
            { "scope": "entity.name", "settings": { "foreground": "#222222" } }
        ] }"##;
        assert_eq!(
            convert(theme, "x", None).unwrap().colors["function"],
            "#222222"
        );
    }

    #[test]
    fn an_exclusion_selector_answers_nothing() {
        assert_eq!(score("-comment", "comment"), None);
        assert_eq!(
            score("meta.function entity.name", "entity.name.function"),
            Some(120)
        );
    }

    /// Build a `.vsix` in memory: a ZIP with `extension/` at its root.
    ///
    /// Constructed rather than vendored. A real package is ~170 KB of somebody else's
    /// MIT-licensed work, and checking one into this repository to assert on two of its files
    /// would be carrying a binary blob for a test that reads six strings out of it. Stored
    /// entries rather than deflated, because what is under test is the road through the archive
    /// and not `flate2` — `a_real_package_imports` below is the one that exercises inflation,
    /// against a file the reader supplies.
    fn vsix(entries: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Write as _;
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, body) in entries {
            writer.start_file(*name, options).expect("start");
            writer.write_all(body.as_bytes()).expect("write");
        }
        writer.finish().expect("finish").into_inner()
    }

    const DARK: &str = r##"{ "name": "Ignored", "type": "dark",
        "colors": { "editor.background": "#1f1f1f", "editor.foreground": "#b9b9b9" },
        "tokenColors": [{ "scope": "string", "settings": { "foreground": "#94a1ad" } }] }"##;
    const LIGHT: &str = r##"{ "name": "Ignored", "type": "light",
        "colors": { "editor.background": "#ffffff", "editor.foreground": "#404040" },
        "tokenColors": [{ "scope": "string", "settings": { "foreground": "#3d7b7f" } }] }"##;

    /// **A `.vsix` yields every theme it declares, not one.**
    ///
    /// This is the report the feature grew from: a user downloaded a `.vsix`, the picker would
    /// not take it, and there was nothing on screen to say the theme was a `.json` inside the
    /// archive. Importing the pair in one gesture is the other half — the setting is keyed by
    /// polarity, so one of two leaves the other theme on the builtin.
    #[test]
    fn a_vsix_yields_every_theme_it_declares() {
        let bytes = vsix(&[
            (
                "extension/package.json",
                r##"{ "name": "min-theme", "contributes": { "themes": [
                    { "label": "Min Dark",  "uiTheme": "vs-dark", "path": "./themes/min-dark.json" },
                    { "label": "Min Light", "uiTheme": "vs",      "path": "./themes/min-light.json" }
                ] } }"##,
            ),
            ("extension/themes/min-dark.json", DARK),
            ("extension/themes/min-light.json", LIGHT),
        ]);
        let dir = tempdir();
        let path = dir.join("min-theme-1.0.0.vsix");
        std::fs::write(&path, bytes).expect("write");

        let schemes = import(&path).expect("import");
        assert_eq!(schemes.len(), 2, "both variants");

        // The manifest's label wins over the theme file's own `name`, which both files here set
        // to "Ignored" precisely so the assertion means something.
        assert_eq!(schemes[0].name, "Min Dark");
        assert_eq!(schemes[0].id, "min-dark");
        assert_eq!(schemes[0].polarity, Theme::Dark);
        assert_eq!(schemes[0].colors["bg"], "#1f1f1f");
        assert_eq!(schemes[0].colors["string"], "#94a1ad");

        assert_eq!(schemes[1].name, "Min Light");
        assert_eq!(schemes[1].polarity, Theme::Light);
        assert_eq!(schemes[1].colors["bg"], "#ffffff");

        // The source records which entry it came from, so the picker can say where it was read.
        assert!(
            schemes[0]
                .source
                .as_deref()
                .unwrap()
                .ends_with("!extension/themes/min-dark.json"),
            "{:?}",
            schemes[0].source
        );
        std::fs::remove_dir_all(dir).ok();
    }

    /// A package whose manifest says nothing still imports, by convention.
    ///
    /// Refusing a file that visibly contains themes because its manifest is shaped unusually is
    /// the sort of refusal that reads as a broken importer — which is the failure this whole
    /// change exists to remove.
    #[test]
    fn a_vsix_with_no_declared_themes_falls_back_to_the_conventional_directory() {
        let bytes = vsix(&[
            ("extension/package.json", r##"{ "name": "odd" }"##),
            ("extension/themes/b-dark.json", DARK),
            ("extension/themes/a-light.json", LIGHT),
            // Not a theme, and not under `themes/`. Must not be tried.
            ("extension/tsconfig.json", r##"{ "compilerOptions": {} }"##),
        ]);
        let dir = tempdir();
        let path = dir.join("odd.vsix");
        std::fs::write(&path, bytes).expect("write");

        let schemes = import(&path).expect("import");
        assert_eq!(schemes.len(), 2);
        // Sorted by entry name, not by the archive's order, so two imports of one file agree.
        assert_eq!(schemes[0].name, "a-light");
        assert_eq!(schemes[1].name, "b-dark");
        std::fs::remove_dir_all(dir).ok();
    }

    /// The dispatch is on the magic number, so a theme saved under any name still imports and an
    /// archive under any name still unpacks.
    #[test]
    fn the_file_type_is_read_from_its_bytes_and_not_its_extension() {
        let dir = tempdir();

        let renamed_archive = dir.join("min-theme.zip");
        std::fs::write(&renamed_archive, vsix(&[("extension/themes/x.json", DARK)]))
            .expect("write");
        assert_eq!(import(&renamed_archive).expect("import").len(), 1);

        // A theme with no extension at all — what a download manager sometimes leaves behind.
        let bare = dir.join("one-dark-color-theme");
        std::fs::write(&bare, DARK).expect("write");
        let schemes = import(&bare).expect("import");
        assert_eq!(schemes.len(), 1);
        assert_eq!(schemes[0].colors["bg"], "#1f1f1f");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn an_archive_with_no_themes_says_what_it_looked_for() {
        let dir = tempdir();
        let path = dir.join("empty.vsix");
        std::fs::write(&path, vsix(&[("extension/README.md", "# nothing here")])).expect("write");

        let error = import(&path).expect_err("no themes");
        let said = error.to_string();
        assert!(said.contains("contributes.themes"), "{said}");
        assert!(said.contains("extension/themes"), "{said}");
        std::fs::remove_dir_all(dir).ok();
    }

    /// **Against a package the reader supplies**, because everything above builds its own archive
    /// with stored entries and therefore never inflates a byte.
    ///
    /// `#[ignore]`d on this repository's own terms — it needs a file that is not in the tree —
    /// and run deliberately:
    ///
    /// ```sh
    /// CIDE_VSIX=~/Downloads/min-theme-1.0.0.vsix \
    ///   cargo test -p cide-core a_real_package -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs a .vsix on disk; set CIDE_VSIX"]
    fn a_real_package_imports() {
        let Some(path) = std::env::var_os("CIDE_VSIX") else {
            panic!("set CIDE_VSIX to a .vsix file");
        };
        let schemes = import(std::path::Path::new(&path)).expect("import");
        assert!(!schemes.is_empty(), "no themes found");
        for scheme in &schemes {
            println!(
                "{:<24} {:<8} bg={} fg={} key={} string={} number={}",
                scheme.name,
                format!("{:?}", scheme.polarity),
                scheme.colors["bg"],
                scheme.colors["fg"],
                scheme.colors["property"],
                scheme.colors["string"],
                scheme.colors["number"],
            );
            for role in roles() {
                assert!(scheme.colors.contains_key(role), "{role} missing");
            }
        }
    }

    /// A directory this process owns, named after the test that asked for it.
    ///
    /// Hand-rolled rather than a `tempfile` dependency: three of these tests write one file each,
    /// and `cide-core` carries no dev-dependency for it today.
    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cide-scheme-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn slugs_are_filename_safe_and_stable() {
        assert_eq!(slug("One Dark Pro"), "one-dark-pro");
        assert_eq!(slug("Solarized (light)"), "solarized-light");
        assert_eq!(slug("тема"), "imported");
        // The compiled-in scheme's id is not available to an import.
        assert_eq!(slug("cide"), "cide-imported");
    }

    /// **The acceptance criterion of the whole feature**, against a theme shaped like the ones
    /// people actually download.
    ///
    /// Six roles appear in a JSON buffer — the key, a string, a number, `true`/`null`,
    /// punctuation and brackets — and before M24 two of them were body text and three were one
    /// colour. If an import cannot give a real theme's answer for all six then nothing else in
    /// this file matters, so it is asserted directly rather than inferred from the parts.
    ///
    /// The scope forms are the ones published themes use, deliberately including the two awkward
    /// ones: a comma-separated selector list in a single string, and a language-qualified scope
    /// (`support.type.property-name.json`) that cide asks about in its general form.
    #[test]
    fn a_real_theme_gives_a_json_buffer_six_colours() {
        let theme = r##"{
          "name": "Nightish",
          "type": "dark",
          "colors": { "editor.background": "#282c34", "editor.foreground": "#abb2bf" },
          "tokenColors": [
            { "scope": "comment", "settings": { "foreground": "#5c6370", "fontStyle": "italic" } },
            { "scope": "string", "settings": { "foreground": "#98c379" } },
            { "scope": "constant.numeric", "settings": { "foreground": "#d19a66" } },
            { "scope": "constant.language", "settings": { "foreground": "#56b6c2" } },
            { "scope": "keyword, storage.type", "settings": { "foreground": "#c678dd" } },
            { "scope": "support.type.property-name.json", "settings": { "foreground": "#e06c75" } },
            { "scope": "entity.name.function", "settings": { "foreground": "#61afef" } },
            { "scope": "punctuation.separator", "settings": { "foreground": "#abb2bf" } },
            { "scope": "meta.brace", "settings": { "foreground": "#7f848e" } }
          ]
        }"##;
        let s = convert(theme, "x", None).expect("convert");

        let json_roles = [
            "property",
            "string",
            "number",
            "constant",
            "punctuation",
            "bracket",
        ];
        let colours: Vec<&str> = json_roles.iter().map(|r| s.colors[*r].as_str()).collect();
        let distinct: std::collections::BTreeSet<&&str> = colours.iter().collect();
        assert_eq!(
            distinct.len(),
            json_roles.len(),
            "a JSON buffer should show six different colours, got {colours:?}"
        );

        // And the four *semantic* ones are not plain text, which is the specific bug: cide's
        // `propertyName` painted `--text`, and `.cm-editor` is also `--text`, so the key was
        // pixel-identical to prose.
        //
        // `punctuation` and `bracket` are deliberately not in this list. Most real themes paint
        // punctuation with `editor.foreground` exactly — this one does — and that is a choice
        // the theme is entitled to make. What matters is that the six read apart from *each
        // other*, which the assertion above covers.
        for role in ["property", "string", "number", "constant"] {
            assert_ne!(
                s.colors[role], s.colors["fg"],
                "{role} is the same colour as prose"
            );
        }

        assert_eq!(s.colors["property"], "#e06c75");
        assert_eq!(s.colors["constant"], "#56b6c2");
        assert_eq!(s.colors["bracket"], "#7f848e");
        assert_eq!(s.colors["bg"], "#282c34");
        assert_eq!(s.polarity, Theme::Dark);
        assert_eq!(s.id, "nightish");
    }

    #[test]
    fn a_file_that_is_not_a_theme_is_refused_rather_than_guessed() {
        assert!(convert("not json at all", "x", None).is_err());
    }

    /// A theme whose `tokenColors` is a path to a `.tmTheme` still imports, on its `colors`
    /// alone. The alternative is an error dialog for a file VS Code opens happily.
    #[test]
    fn a_tmtheme_reference_does_not_fail_the_import() {
        let theme = r##"{ "name": "T", "type": "light", "tokenColors": "./T.tmTheme",
                         "colors": { "editor.background": "#ffffff", "editor.foreground": "#222222" } }"##;
        let s = convert(theme, "x", None).expect("convert");
        assert_eq!(s.colors["bg"], "#ffffff");
        assert_eq!(s.colors["keyword"], "#222222");
    }
}
