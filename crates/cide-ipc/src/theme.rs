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
        // `sel` is filled by [`ColorScheme::normalise`] before this table can be consulted — see
        // there — so this entry is the floor and not the path. It stays because the chain has to
        // be total for every role, and a role with no entry is a hang waiting to be written.
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
        // Punctuation falls to the **foreground**, not to the operator role, and that is what VS
        // Code does with punctuation a theme says nothing about. It used to run
        // `bracket → punctuation → operator`, on the reasoning that the three are neighbours —
        // but `operator` is a *keyword* in TextMate and is usually coloured like one, so a theme
        // silent about punctuation had its commas and brackets painted keyword red.
        "bracket" => "punctuation",
        "punctuation" => "fg",
        "operator" => "fg",
        "strong" => "heading",
        "heading" => "fg",
        "emphasis" => "fg",
        "link" => "fg",
        _ => return None,
    })
}

/// How far a selection must sit from the background before it reads as a selection at all.
///
/// Redmean units — see [`distance`]. 72 is the light theme's own `--tk-sel` measured against its
/// background; the dark one is at 124. Below about 50 a selection is a shade the eye does not
/// register as a state change, which is exactly what was reported of an imported theme that
/// carried no `editor.selectionBackground`: *"no selection visible at all, like I'm just moving
/// the cursor."*
pub const MIN_SELECTION_DISTANCE: f64 = 72.0;

/// What a derived selection is tinted towards, per polarity.
///
/// **Chroma, not lightness, and that is the whole trick.** Contrast against a selection is
/// `(ink + 0.05) / (selection + 0.05)`, so any selection brighter than the background taxes every
/// ink at once — the failure `tokens.css` argues out at length for cide's own dark scheme. A deep
/// saturated blue sits at roughly the luminance of a dark editor's background while being
/// obviously a different colour, so a tint towards it buys visibility for almost no contrast.
/// Measured across real themes, a dark scheme keeps 93–99% of its ink contrast this way where
/// blending towards the foreground kept far less and was still invisible.
///
/// A light background cannot play that trick — there is nothing above white — so the light tint
/// is a mid blue and the cost is real but bounded, the same 0.84-ish the built-in light scheme
/// already pays.
const SELECTION_TINT_DARK: &str = "#0d20a0";
const SELECTION_TINT_LIGHT: &str = "#4a90ff";

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

        // The selection last, because deriving one needs to know every ink it will be painted
        // under — and it is *validated*, not merely filled.
        //
        // Filling was the first attempt and was not enough. A theme is free to carry no
        // `editor.selectionBackground` at all (Min Dark does), in which case the chain above has
        // just answered `bg` and the selection is invisible; but a theme can also carry one that
        // is useless in practice — an alpha-only tint whose opaque form is its own background, or
        // a value that simply sits too close to it. Both produce the same report, so both get the
        // same answer: if it does not read as a selection, replace it.
        //
        // Overriding a value the theme did state is a real intrusion and worth naming. It is
        // taken because the alternative is a feature that silently does not work — a user
        // dragging across code and seeing the caret move and nothing else — and because
        // `ColorScheme` is a *rendering* of a theme rather than a copy of one.
        let bg = self.colors["bg"].clone();
        let current = self
            .colors
            .get("sel")
            .cloned()
            .unwrap_or_else(|| bg.clone());
        if distance(&current, &bg).is_none_or(|d| d < MIN_SELECTION_DISTANCE) {
            let derived = self.derive_selection(&bg);
            self.colors.insert("sel".into(), derived);
        }
    }

    /// The gentlest tint towards [`SELECTION_TINT_DARK`]/`_LIGHT` that still reads as a selection.
    ///
    /// *Gentlest* is the whole criterion: every step away from the background costs some ink some
    /// contrast, so the answer is the first candidate that clears [`MIN_SELECTION_DISTANCE`]
    /// rather than the most visible one available. Stepping in 2% increments is finer than any
    /// eye can resolve and bounds the loop at 33 iterations.
    ///
    /// If no tint reaches the bar — a background already so blue that a blue tint cannot move it
    /// — the strongest candidate is returned. Visibility is the reported failure; a selection
    /// that is hard to read beats one that is not there.
    fn derive_selection(&self, bg: &str) -> String {
        let tint = match self.polarity {
            Theme::Dark => SELECTION_TINT_DARK,
            Theme::Light => SELECTION_TINT_LIGHT,
        };
        let mut strongest = blend(bg, tint, 0.02);
        for step in 1..=33 {
            let candidate = blend(bg, tint, f64::from(step) * 0.02);
            if distance(&candidate, bg).is_some_and(|d| d >= MIN_SELECTION_DISTANCE) {
                return candidate;
            }
            strongest = candidate;
        }
        strongest
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

/// `fraction` of the way from `from` towards `towards`, in sRGB. Both must be hex; `from` is
/// returned unchanged if either is not.
///
/// sRGB rather than a perceptual space on purpose: this is the same arithmetic CSS's
/// `color-mix(in srgb, …)` does, and `EditorSurface.module.css` composites the caret's line with
/// exactly that function. Two blends of the same pair that disagreed would be a selection and an
/// active line that do not line up.
pub fn blend(from: &str, towards: &str, fraction: f64) -> String {
    let (Some(a), Some(b)) = (channels(from), channels(towards)) else {
        return from.to_string();
    };
    let f = fraction.clamp(0.0, 1.0);
    let mix = |i: usize| (a[i] as f64 + (b[i] as f64 - a[i] as f64) * f).round() as u8;
    format!("#{:02x}{:02x}{:02x}", mix(0), mix(1), mix(2))
}

/// The three channels of a normalised hex colour.
fn channels(hex: &str) -> Option<[u8; 3]> {
    let body = normalise_hex(hex)?;
    let bytes = body.as_bytes();
    let at = |i: usize| u8::from_str_radix(std::str::from_utf8(&bytes[i..i + 2]).ok()?, 16).ok();
    Some([at(1)?, at(3)?, at(5)?])
}

/// How far apart two colours *look*, or `None` if either is not a hex colour.
///
/// Thiadmer Riemersma's redmean weighted RGB distance, the same metric `check-theme.mjs` and
/// `check-scheme.mjs` use, restated here because those are JavaScript and this is not. Range is
/// roughly 0…765.
///
/// **Not a contrast ratio, and the difference decides the answer.** Contrast is a ratio of
/// luminance and asks whether ink can be read on a ground. Whether a *fill* is visible is a
/// different question: the built-in dark selection is 1.12:1 against its background — no contrast
/// to speak of — and perfectly obvious, because it is a different colour. A contrast ratio would
/// rate it as invisible and rate the value it replaced as fine.
pub fn distance(a: &str, b: &str) -> Option<f64> {
    let (x, y) = (channels(a)?, channels(b)?);
    let mean = (f64::from(x[0]) + f64::from(y[0])) / 2.0;
    let d = |i: usize| f64::from(x[i]) - f64::from(y[i]);
    Some(
        ((2.0 + mean / 256.0) * d(0) * d(0)
            + 4.0 * d(1) * d(1)
            + (2.0 + (255.0 - mean) / 256.0) * d(2) * d(2))
        .sqrt(),
    )
}

/// Flatten `#rrggbbaa` (or `#rgba`) onto `ground`, giving the opaque colour that would be drawn.
///
/// **Compositing rather than dropping the alpha, which is what this used to do.** VS Code themes
/// write `editor.selectionBackground` with an alpha channel far more often than not, and the two
/// answers are nothing alike: `#ffffff20` composited over a dark editor is a barely-lifted grey,
/// which is what the theme meant, while `#ffffff` with the alpha discarded is a white block over
/// the text. The stored value is still six hex digits — `ColorScheme` has no notion of
/// translucency and `check-scheme.mjs` takes contrast ratios of every role — so what is being
/// kept is the *rendering* the theme asked for rather than its notation.
///
/// An opaque or malformed input passes through [`normalise_hex`] unchanged.
pub fn composite(raw: &str, ground: &str) -> Option<String> {
    let body = raw.trim().strip_prefix('#')?;
    let alpha = match body.len() {
        4 => u8::from_str_radix(&body[3..4].repeat(2), 16).ok()?,
        8 => u8::from_str_radix(&body[6..8], 16).ok()?,
        _ => return normalise_hex(raw),
    };
    let over = normalise_hex(raw)?;
    Some(blend(ground, &over, f64::from(alpha) / 255.0))
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
        // `doc` → `comment` is one hop.
        assert_eq!(scheme.colors["doc"], "#888888");
        // `bracket` → `punctuation` → `fg`, and **not** through `operator`, even though this
        // scheme states one. VS Code paints punctuation a theme is silent about with the
        // foreground; routing it through the operator role painted Min Dark's commas and
        // brackets in that theme's keyword red. See `fallback`.
        assert_eq!(scheme.colors["bracket"], "#ffffff");
        assert_eq!(scheme.colors["operator"], "#777777");
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

    /// A theme with no `editor.selectionBackground` — Min Dark is a real one — must not get an
    /// invisible selection. Reported as *"no selection visible at all, like I'm just moving the
    /// cursor"*.
    #[test]
    fn a_scheme_with_no_selection_gets_a_visible_one() {
        let mut scheme = dark_scheme(&[("bg", "#1f1f1f"), ("fg", "#7d7d7d")]);
        scheme.normalise();
        let sel = &scheme.colors["sel"];
        let apart = distance(sel, "#1f1f1f").expect("hex");
        assert!(
            apart >= MIN_SELECTION_DISTANCE,
            "derived {sel} is only {apart:.0} from the background"
        );
    }

    /// A selection the theme *did* state, but which is useless in practice, is replaced on the
    /// same terms. An alpha-only tint whose opaque form is its own background is the common way
    /// to reach this; a theme that simply picked a near-background value is the other.
    #[test]
    fn a_selection_too_close_to_the_background_is_replaced() {
        let mut scheme = dark_scheme(&[("bg", "#1f1f1f"), ("fg", "#d0d0d0"), ("sel", "#212121")]);
        scheme.normalise();
        assert_ne!(scheme.colors["sel"], "#212121");
        assert!(distance(&scheme.colors["sel"], "#1f1f1f").expect("hex") >= MIN_SELECTION_DISTANCE);
    }

    /// A usable selection is left exactly as the theme wrote it. The override above is an
    /// intrusion and must not happen to a theme that got it right.
    #[test]
    fn a_usable_selection_is_left_alone() {
        let mut scheme = dark_scheme(&[("bg", "#1f1f1f"), ("fg", "#d0d0d0"), ("sel", "#264f78")]);
        scheme.normalise();
        assert_eq!(scheme.colors["sel"], "#264f78");
    }

    /// The derived selection must be visible **without** costing the inks their contrast — the
    /// failure that took three rounds to get right for the built-in dark scheme. A tint towards a
    /// deep blue is what buys both at once.
    #[test]
    fn a_derived_selection_preserves_ink_contrast() {
        for (bg, fg) in [
            ("#1f1f1f", "#7d7d7d"),
            ("#282c34", "#abb2bf"),
            ("#000000", "#dddddd"),
        ] {
            let mut scheme = dark_scheme(&[("bg", bg), ("fg", fg)]);
            scheme.normalise();
            let sel = &scheme.colors["sel"];
            let ratio = |a: &str, b: &str| {
                let (x, y) = (luminance(a).unwrap(), luminance(b).unwrap());
                (x.max(y) + 0.05) / (x.min(y) + 0.05)
            };
            let kept = ratio(fg, sel) / ratio(fg, bg);
            assert!(
                kept >= 0.85,
                "{bg}: the derived {sel} costs {:.0}% of the ink's contrast",
                (1.0 - kept) * 100.0
            );
        }
    }

    #[test]
    fn alpha_is_composited_onto_the_ground_rather_than_dropped() {
        // 60% of #264f78 over #1e1e1e — what VS Code paints, not #264f78.
        assert_eq!(
            composite("#264f7899", "#1e1e1e").as_deref(),
            Some("#233b54")
        );
        // A translucent white is a lifted grey, not a white block over the text.
        assert_eq!(
            composite("#ffffff20", "#000000").as_deref(),
            Some("#202020")
        );
        // Fully opaque and fully transparent are the two ends.
        assert_eq!(
            composite("#264f78ff", "#1e1e1e").as_deref(),
            Some("#264f78")
        );
        assert_eq!(
            composite("#264f7800", "#1e1e1e").as_deref(),
            Some("#1e1e1e")
        );
        // No alpha channel: unchanged.
        assert_eq!(composite("#264f78", "#1e1e1e").as_deref(), Some("#264f78"));
        assert_eq!(composite("not a colour", "#1e1e1e"), None);
    }

    #[test]
    fn distance_sees_a_colour_change_that_contrast_cannot() {
        // The built-in dark selection against its background: no contrast to speak of, and
        // obviously a different colour. A ratio would call it invisible.
        let (a, b) = (luminance("#0d1560").unwrap(), luminance("#151518").unwrap());
        let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
        assert!(ratio < 1.2, "{ratio}");
        assert!(distance("#0d1560", "#151518").unwrap() > MIN_SELECTION_DISTANCE);
    }

    /// A dark scheme carrying only the roles named, for the tests above.
    fn dark_scheme(pairs: &[(&str, &str)]) -> ColorScheme {
        ColorScheme {
            id: "t".into(),
            name: "T".into(),
            polarity: Theme::Dark,
            colors: pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            source: None,
        }
    }

    #[test]
    fn blending_matches_css_color_mix_in_srgb() {
        assert_eq!(blend("#000000", "#ffffff", 0.5), "#808080");
        assert_eq!(blend("#000000", "#ffffff", 0.0), "#000000");
        assert_eq!(blend("#000000", "#ffffff", 1.0), "#ffffff");
        // Out of range is clamped rather than extrapolated into a colour that is not one.
        assert_eq!(blend("#000000", "#ffffff", 4.0), "#ffffff");
        // A non-colour in either position leaves the first unchanged rather than inventing one.
        assert_eq!(blend("#123456", "not a colour", 0.5), "#123456");
    }

    #[test]
    fn luminance_orders_black_below_white() {
        let black = luminance("#000000").expect("black");
        let white = luminance("#ffffff").expect("white");
        assert!(black < 0.01 && white > 0.99, "{black} .. {white}");
    }
}
