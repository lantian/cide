//! Editor colour schemes: what colour a token is, as data rather than as CSS. (M24)
//!
//! # Why this is not part of [`crate::Theme`]
//!
//! [`crate::Theme`] is the app's *polarity* — the thing `data-theme` selects, the thing
//! `windows.rs` bakes into `?theme=`, the thing the file icons pick a variant by. It stays two
//! values for ever. A colour scheme is a different axis: it repaints the editor's buffer and
//! nothing else, and there can be any number of them because a user imports them.
//!
//! Keeping them apart is what lets every existing consumer of `Theme` stay untouched, and it is
//! why a scheme carries a [`polarity`](ColorScheme::polarity) rather than replacing one.
//!
//! # Why a map and not a struct with one field per role
//!
//! A struct would give compile-time totality here and lose it everywhere else: the same closed
//! set has to be spelled in `ui/src/editor/scheme.ts` (which is import-free, so it cannot read a
//! generated type), in `ui/src/editor/highlight.css` (a plain stylesheet, because CodeMirror
//! writes the literal class name) and in both palette blocks of `ui/src/styles/tokens.css`.
//! Four copies is the floor, not a choice — none of those files can import another.
//!
//! So [`SCHEME_ROLES`] is the one list, [`ColorScheme::normalise`] is what makes an arbitrary map
//! total against it, and `ui/scripts/check-scheme.mjs` pins all four copies against each other.
//! That is the same shape `check-ui-scale.mjs` uses for the four copies of the base font size.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::Theme;

/// The id of the scheme that is compiled in rather than imported.
///
/// Not a scheme value: `cide`'s colours live in `tokens.css`, in the two palette blocks, because
/// they are what paints when no custom property has been written onto `<html>` at all. Naming it
/// here is what lets Rust refuse to delete it and the frontend know to clear rather than write.
pub const BUILTIN_SCHEME: &str = "cide";

/// Every `--tk-*` custom property, without the prefix. **The closed set, in both directions.**
///
/// Two groups, and the split matters to the contrast gate rather than to the applier: the first
/// six paint the surface a buffer sits on, the rest paint runs of text on top of it. A gate that
/// measured a role against `--panel` instead of against this scheme's own `bg` would be checking
/// a pair that never appears on screen together.
///
/// The role order is `ui/src/editor/highlight.ts`'s `TOKEN_ROLES` order, which is
/// most-specific-first because `HighlightStyle` resolves a tag through its parent chain. Nothing
/// here depends on the order; keeping it the same is what makes the two files diffable.
pub const SCHEME_SURFACE: &[&str] = &["bg", "fg", "sel", "gutter", "caret"];

/// One entry per role in `TOKEN_ROLES`. See [`SCHEME_SURFACE`] for why they are two lists.
pub const SCHEME_TOKENS: &[&str] = &[
    "doc",
    "comment",
    "control",
    "constant",
    "escape",
    "regexp",
    "attribute",
    "string",
    "macro",
    "label",
    "function",
    "namespace",
    "type",
    "keyword",
    "number",
    "property",
    "variable",
    "bracket",
    "punctuation",
    "operator",
    "heading",
    "strong",
    "emphasis",
    "link",
];

/// Surface then tokens, which is the order `check-scheme.mjs` expects to find them declared in.
pub fn scheme_roles() -> Vec<&'static str> {
    SCHEME_SURFACE
        .iter()
        .chain(SCHEME_TOKENS.iter())
        .copied()
        .collect()
}

/// What a role falls back to when a source theme said nothing about it.
///
/// **Resolution is total.** A VS Code theme is free to define eleven scopes and stop, and the
/// answer to that must be a usable scheme rather than an error or a hole — the same bargain
/// [`crate::ext::SettingKind`]'s coercion makes, for the same reason: the value came from a file
/// somebody else wrote, so refusing it means the feature does not work for real inputs.
///
/// Each chain ends at a role that [`ColorScheme::normalise`] can always answer, and `fg` is the
/// floor. A cycle here would hang the resolver, so the table is walked with a step budget.
pub fn fallback(role: &str) -> Option<&'static str> {
    Some(match role {
        // Surface. `bg` and `fg` have no fallback and are required of every scheme.
        //
        // There is deliberately no `line` role. The caret's line is *derived* from `sel` in CSS,
        // at 40%, and `EditorSurface.module.css` carries the thirty-line argument for why: it is
        // painted **above** the selection layer, so anything opaque there hides the selection on
        // the caret's row — the bug that rule exists to fix — and only a tint of the selection's
        // own colour composites back to the selection exactly. A theme's
        // `editor.lineHighlightBackground` is an arbitrary opaque colour once its alpha is
        // dropped, so honouring it would reintroduce that bug for every imported scheme.
        "sel" => "bg",
        "gutter" => "comment",
        "caret" => "fg",
        // A documentation comment is a comment before it is anything else.
        "doc" => "comment",
        "comment" => "fg",
        "control" => "keyword",
        // An escape and a regexp are the two things inside a string that are not the string.
        // Falling back to `constant` rather than to `string` is deliberate: a theme that says
        // nothing about them is better served by a colour that contrasts with the string it sits
        // in than by one that disappears into it.
        "escape" => "constant",
        "regexp" => "string",
        "constant" => "number",
        "attribute" => "property",
        "string" => "fg",
        // `macroName` and `labelName` are both "a name that is invoked", which is what `function`
        // means to a theme that has not thought about either.
        "macro" => "function",
        "label" => "function",
        "function" => "fg",
        "namespace" => "type",
        "type" => "fg",
        "keyword" => "fg",
        "number" => "fg",
        "property" => "fg",
        "variable" => "fg",
        // Punctuation is the one chain that is three deep, and it runs towards the quietest of
        // the three rather than away from it.
        "bracket" => "punctuation",
        "punctuation" => "operator",
        "operator" => "fg",
        "strong" => "heading",
        "heading" => "fg",
        "emphasis" => "fg",
        "link" => "fg",
        _ => return None,
    })
}

/// One imported colour scheme, resolved and total.
///
/// What is stored on disk and what rides [`crate::Bootstrap`] are the same value: the frontend
/// writes [`colors`](Self::colors) onto `<html>` as `--tk-<role>` custom properties and every
/// surface that highlights code follows for nothing, because `highlight.css` reads those
/// properties and `HighlightStyle` was built with `class:` rather than `color:`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ColorScheme {
    /// Stable, filename-safe, and what [`crate::EditorSettings`] stores. Derived from the source
    /// theme's own name at import.
    pub id: String,
    /// What the picker shows — the theme's own `name`, kept verbatim so a user recognises it.
    pub name: String,
    /// Which [`Theme`] this scheme belongs under. A scheme paints the buffer's *background*, so
    /// a dark scheme under a light window would be an inverted rectangle in a white app; the
    /// setting is keyed by polarity precisely so that state is unreachable.
    pub polarity: Theme,
    /// Role → `#rrggbb`. Total against [`scheme_roles`] after [`ColorScheme::normalise`].
    pub colors: BTreeMap<String, String>,
    /// Where it was imported from, for the picker's subtitle. Advisory: the file is read once,
    /// at import, and never again — see the module header of `cide_core::scheme`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source: Option<String>,
}

impl ColorScheme {
    /// Make an arbitrary map total against [`scheme_roles`], and drop anything not in it.
    ///
    /// Dropping unknown keys is not tidiness: `colors` is written onto `<html>` as custom
    /// properties, so a key that survived here would become a `--tk-<whatever>` nobody reads —
    /// or, worse, a name that collides with a chrome token the day one is added. The applier is
    /// therefore allowed to trust that every key it sees is a role.
    pub fn normalise(&mut self) {
        let roles = scheme_roles();
        self.colors
            .retain(|role, value| roles.contains(&role.as_str()) && normalise_hex(value).is_some());
        for value in self.colors.values_mut() {
            if let Some(hex) = normalise_hex(value) {
                *value = hex;
            }
        }

        // `fg` and `bg` have no fallback and every other chain ends at one of them, so they are
        // filled first and from a constant. A scheme that reached here without them came from a
        // theme with no `editor.foreground` at all, which is legal and rare.
        let polarity = self.polarity;
        self.colors.entry("fg".into()).or_insert_with(|| {
            match polarity {
                Theme::Dark => "#d7d7dd",
                Theme::Light => "#1b1b1f",
            }
            .into()
        });
        self.colors.entry("bg".into()).or_insert_with(|| {
            match polarity {
                Theme::Dark => "#151518",
                Theme::Light => "#ffffff",
            }
            .into()
        });

        for role in roles {
            if self.colors.contains_key(role) {
                continue;
            }
            // Walked with a budget rather than recursively: `fallback` is a hand-written table
            // and a cycle introduced by an edit would otherwise hang a command thread. The bound
            // is the table's own size, so no legal chain can hit it.
            let mut at = role;
            let mut value = None;
            for _ in 0..SCHEME_SURFACE.len() + SCHEME_TOKENS.len() {
                let Some(next) = fallback(at) else { break };
                if let Some(found) = self.colors.get(next) {
                    value = Some(found.clone());
                    break;
                }
                at = next;
            }
            let value = value.unwrap_or_else(|| self.colors["fg"].clone());
            self.colors.insert(role.into(), value);
        }
    }
}

/// `#rgb`, `#rrggbb` and `#rrggbbaa` in, lowercase `#rrggbb` out. Anything else is `None`.
///
/// The alpha is **dropped rather than composited**, and that is a decision worth naming: VS Code
/// writes `editor.selectionBackground` and `editor.lineHighlightBackground` with an alpha channel
/// far more often than not, and compositing it against the theme's own background here would
/// bake one answer for a value CSS is perfectly able to blend itself. Dropping it keeps every
/// stored value the same shape — six hex digits — which is what lets `check-scheme.mjs` take a
/// contrast ratio of any role without first having to know which ones may be translucent.
pub fn normalise_hex(raw: &str) -> Option<String> {
    let body = raw.trim().strip_prefix('#')?;
    if !body.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let six = match body.len() {
        3 => body.chars().flat_map(|c| [c, c]).collect::<String>(),
        4 => body
            .chars()
            .take(3)
            .flat_map(|c| [c, c])
            .collect::<String>(),
        6 | 8 => body[..6].to_string(),
        _ => return None,
    };
    Some(format!("#{}", six.to_ascii_lowercase()))
}

/// Relative luminance, per WCAG. Used to guess a polarity when a theme does not declare one, and
/// by the contrast helper below.
pub fn luminance(hex: &str) -> Option<f64> {
    let body = normalise_hex(hex)?;
    let bytes = body.as_bytes();
    let channel = |at: usize| -> f64 {
        let pair = std::str::from_utf8(&bytes[at..at + 2]).unwrap_or("00");
        let raw = u8::from_str_radix(pair, 16).unwrap_or(0) as f64 / 255.0;
        if raw <= 0.03928 {
            raw / 12.92
        } else {
            ((raw + 0.055) / 1.055).powf(2.4)
        }
    };
    Some(0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_forms_normalise() {
        assert_eq!(normalise_hex("#ABC").as_deref(), Some("#aabbcc"));
        assert_eq!(normalise_hex(" #1E1E1E ").as_deref(), Some("#1e1e1e"));
        // The alpha is dropped, not composited — see the function's own note.
        assert_eq!(normalise_hex("#1e1e1e80").as_deref(), Some("#1e1e1e"));
        assert_eq!(normalise_hex("#abcd").as_deref(), Some("#aabbcc"));
        assert_eq!(normalise_hex("rebeccapurple"), None);
        assert_eq!(normalise_hex("#12345"), None);
        assert_eq!(normalise_hex("#gggggg"), None);
    }

    /// The property the applier is allowed to trust: every role present, nothing else present.
    #[test]
    fn normalise_is_total_and_closed() {
        let mut scheme = ColorScheme {
            id: "t".into(),
            name: "T".into(),
            polarity: Theme::Dark,
            colors: BTreeMap::from([
                ("fg".into(), "#ffffff".into()),
                ("bg".into(), "#000000".into()),
                ("comment".into(), "#888888".into()),
                ("operator".into(), "#777777".into()),
                // Neither of these is a role; both must be gone afterwards.
                ("editor.background".into(), "#123456".into()),
                ("keyword".into(), "not a colour".into()),
            ]),
            source: None,
        };
        scheme.normalise();

        let roles = scheme_roles();
        assert_eq!(scheme.colors.len(), roles.len());
        for role in &roles {
            assert!(scheme.colors.contains_key(*role), "{role} is missing");
        }
        assert!(!scheme.colors.contains_key("editor.background"));
        // `doc` → `comment` is one hop; `bracket` → `punctuation` → `operator` is two.
        assert_eq!(scheme.colors["doc"], "#888888");
        assert_eq!(scheme.colors["bracket"], "#777777");
        // A value that is not a colour is dropped and then refilled from the chain, rather than
        // surviving as itself and reaching the DOM.
        assert_eq!(scheme.colors["keyword"], "#ffffff");
    }

    /// Every chain terminates. Written as a test rather than trusted because `fallback` is a
    /// hand-maintained table and a typo in it is a hang, not a wrong colour.
    #[test]
    fn every_fallback_chain_reaches_a_root() {
        for role in scheme_roles() {
            let mut at = role;
            let mut steps = 0;
            while let Some(next) = fallback(at) {
                assert!(
                    scheme_roles().contains(&next),
                    "{at} falls back to {next}, which is not a role"
                );
                at = next;
                steps += 1;
                assert!(steps < 16, "the chain from {role} does not terminate");
            }
            assert!(
                at == "fg" || at == "bg",
                "the chain from {role} ends at {at} rather than at fg or bg"
            );
        }
    }

    #[test]
    fn luminance_orders_black_below_white() {
        let black = luminance("#000000").expect("black");
        let white = luminance("#ffffff").expect("white");
        assert!(black < 0.01 && white > 0.99, "{black} .. {white}");
    }
}
