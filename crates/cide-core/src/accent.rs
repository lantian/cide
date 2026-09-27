//! The user's accent colour: refusing the ones that cannot be an accent, and fitting the rest
//! to both themes. (Settings → Appearance → Accent colour.)
//!
//! # What is fitted, and why here
//!
//! The accent is not decoration: `tokens.css` uses `--accent` as *text* (links, the active
//! rail button's glyph, a counter's figure on a wash) and as a *fill under a white label* (the
//! primary button, the awaiting counter). A colour picked off a wheel meets neither bar by
//! accident — a pale yellow is invisible as text on white, a mid blue fails as text on the dark
//! panel. So a picked colour becomes two:
//!
//! - `light` — ≥ 4.5:1 against the light theme's `--panel` (white). White is also what a
//!   primary button's label is, so the same number makes it the *ink* fill in both themes,
//!   which is what `--grad-accent-ink` has always been (one red, identical in both themes).
//! - `dark` — ≥ 4.5:1 against the dark theme's `--panel`.
//!
//! The fit keeps the hue and the chroma and walks only OKLCH lightness, so a user who picks a
//! blue gets their blue, darkened or lightened just as far as legibility needs and no further.
//! A colour that already clears the bar is returned exactly as picked.
//!
//! In Rust rather than the webview for `scheme.rs`'s reason: it is domain logic with a table of
//! cases, and `cargo test` is where a table of cases belongs. The webview receives three hexes
//! and does no arithmetic, which is also what lets `public/theme-boot.js` paint the right colour
//! on the first frame without importing anything.
//!
//! # What is refused
//!
//! **Black is danger.** The kit's destructive button is a black fill (`--grad-danger`, the
//! user's call on 2026-09-24, once the accent turned red and a red danger button looked like the
//! primary). An accent near black would make "the act this dialog is for" and "the act that
//! destroys" the same colour again — the exact defect that decision fixed. The user asked for the
//! refusal to cover every *colourless* tone, not only black: a grey primary reads as disabled,
//! and a near-white one vanishes into the light theme's chrome. So the rule is on the colour's
//! OKLCH lightness and chroma, not on a list of hexes.

use cide_ipc::Accent;
use cide_ipc::theme::{luminance, normalise_hex};

/// The light theme's `--panel`. Pinned against `tokens.css` by a test below: this file measures
/// contrast against it, and a panel that moved without this would fit colours to a surface the
/// app no longer draws.
pub const LIGHT_PANEL: &str = "#ffffff";
/// The dark theme's `--panel`, pinned the same way.
pub const DARK_PANEL: &str = "#151518";

/// WCAG AA for normal text. The accent carries 11px labels, so the large-text 3:1 is not the bar.
pub const MIN_CONTRAST: f64 = 4.5;

/// Below this OKLCH chroma a colour is a grey as far as a reader is concerned. 0.06 is where a
/// tone stops reading as "a colour" at a glance on both grounds; the shipped red is 0.21, and the
/// most muted of the presets (teal) is above 0.1.
pub const MIN_CHROMA: f64 = 0.06;
/// Darker than this and a fill reads as black — the danger fill's top stop, `#34343b`, is 0.32.
pub const MIN_LIGHTNESS: f64 = 0.30;
/// Lighter than this and it is a tint of white with nothing to fit towards.
pub const MAX_LIGHTNESS: f64 = 0.95;

/// Why a colour cannot be the accent. The message is what Settings shows under the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    NotAColour(String),
    TooDark,
    Colourless,
    TooLight,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAColour(raw) => write!(f, "{raw:?} is not a #rrggbb colour"),
            Self::TooDark => f.write_str(
                "Too close to black — black is reserved for destructive actions. Pick a lighter colour.",
            ),
            Self::Colourless => f.write_str(
                "A grey cannot be the accent — it reads as disabled next to the black danger fill. Pick a colour with more hue.",
            ),
            Self::TooLight => {
                f.write_str("Too close to white to be seen on the light theme. Pick a deeper colour.")
            }
        }
    }
}

/// Validate `raw` and fit it to both themes.
pub fn fit(raw: &str) -> Result<Accent, Refusal> {
    let base = normalise_hex(raw).ok_or_else(|| Refusal::NotAColour(raw.to_string()))?;
    let lch = to_oklch(rgb(&base));
    // Lightness first: black has no chroma either, and "too dark" is the reason that names the
    // danger fill the user reserved it for.
    if lch.l < MIN_LIGHTNESS {
        return Err(Refusal::TooDark);
    }
    if lch.l > MAX_LIGHTNESS && lch.c < MIN_CHROMA {
        return Err(Refusal::TooLight);
    }
    if lch.c < MIN_CHROMA {
        return Err(Refusal::Colourless);
    }
    let light = fit_against(&base, lch, LIGHT_PANEL, -1.0);
    let dark = fit_against(&base, lch, DARK_PANEL, 1.0);
    Ok(Accent { base, light, dark })
}

/// Walk lightness in `direction` until `hex` clears [`MIN_CONTRAST`] against `ground`.
/// Unchanged if it already does.
fn fit_against(hex: &str, lch: Lch, ground: &str, direction: f64) -> String {
    if contrast(hex, ground) >= MIN_CONTRAST {
        return hex.to_string();
    }
    let mut l = lch.l;
    // 0.005 steps: fine enough that the result is the lightest (or darkest) legible version of
    // the hue rather than one visibly past it, and at most 200 iterations.
    loop {
        l = (l + 0.005 * direction).clamp(0.0, 1.0);
        let candidate = from_oklch_in_gamut(Lch { l, ..lch });
        if contrast(&candidate, ground) >= MIN_CONTRAST || l <= 0.0 || l >= 1.0 {
            return candidate;
        }
    }
}

/// WCAG contrast ratio between two hexes.
pub fn contrast(a: &str, b: &str) -> f64 {
    let (la, lb) = (luminance(a).unwrap_or(0.0), luminance(b).unwrap_or(0.0));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

// --- OKLCH ---------------------------------------------------------------------------------
//
// Björn Ottosson's OKLab, the space CSS Color 4's `oklch()` is defined in. Chosen over HSL
// because HSL's lightness is not perceptual: walking HSL L on a blue and on a yellow reaches
// 4.5:1 at wildly different "L", and the fitted colours would drift in apparent saturation.

#[derive(Debug, Clone, Copy)]
struct Lch {
    l: f64,
    c: f64,
    h: f64,
}

fn rgb(hex: &str) -> [f64; 3] {
    let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).unwrap_or(0) as f64 / 255.0;
    [byte(1), byte(3), byte(5)]
}

fn to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn from_linear(c: f64) -> f64 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn to_oklch([r, g, b]: [f64; 3]) -> Lch {
    let (r, g, b) = (to_linear(r), to_linear(g), to_linear(b));
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    let ok_l = 0.210_454_255_3 * l + 0.793_617_785 * m - 0.004_072_046_8 * s;
    let ok_a = 1.977_998_495_1 * l - 2.428_592_205 * m + 0.450_593_709_9 * s;
    let ok_b = 0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766 * s;
    Lch {
        l: ok_l,
        c: (ok_a * ok_a + ok_b * ok_b).sqrt(),
        h: ok_b.atan2(ok_a),
    }
}

/// Linear sRGB, possibly out of gamut.
fn oklch_to_linear(lch: Lch) -> [f64; 3] {
    let (a, b) = (lch.c * lch.h.cos(), lch.c * lch.h.sin());
    let l = (lch.l + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
    let m = (lch.l - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
    let s = (lch.l - 0.089_484_177_5 * a - 1.291_485_548 * b).powi(3);
    [
        4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s,
        -1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s,
        -0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701 * s,
    ]
}

fn in_gamut(rgb: [f64; 3]) -> bool {
    rgb.iter().all(|c| (-1e-4..=1.0 + 1e-4).contains(c))
}

/// The colour at `lch`, with chroma reduced (hue and lightness kept) until it is displayable.
/// Reducing chroma rather than clipping channels is what keeps a walked blue *blue*: clipping
/// shifts hue, and at the lightness a dark-theme fit reaches it would shift it towards cyan.
fn from_oklch_in_gamut(lch: Lch) -> String {
    let mut linear = oklch_to_linear(lch);
    if !in_gamut(linear) {
        let (mut lo, mut hi) = (0.0, lch.c);
        for _ in 0..24 {
            let mid = (lo + hi) / 2.0;
            if in_gamut(oklch_to_linear(Lch { c: mid, ..lch })) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        linear = oklch_to_linear(Lch { c: lo, ..lch });
    }
    let byte = |c: f64| (from_linear(c.clamp(0.0, 1.0)) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        byte(linear[0]),
        byte(linear[1]),
        byte(linear[2])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The swatches Settings offers (`ui/src/settings/accentPresets.ts`). Every one of them must
    /// fit: a preset the app refuses is a button that errors.
    const PRESETS: &[&str] = &[
        "#dc1f2b", "#2563eb", "#4f46e5", "#7c3aed", "#db2777", "#ea580c", "#0d9488", "#16a34a",
    ];

    fn token(block_start: &str, name: &str) -> String {
        let css = include_str!("../../../ui/src/styles/tokens.css");
        let block = &css[css.find(block_start).expect("theme block")..];
        let at = block.find(&format!("{name}:")).expect("token") + name.len() + 1;
        block[at..block[at..].find(';').unwrap() + at]
            .trim()
            .to_string()
    }

    #[test]
    fn the_panels_this_measures_against_are_the_ones_tokens_css_draws() {
        assert_eq!(token("[data-theme='light'] {", "--panel"), LIGHT_PANEL);
        assert_eq!(token("[data-theme='dark'] {", "--panel"), DARK_PANEL);
    }

    #[test]
    fn every_preset_fits_and_the_default_is_kept_as_picked_on_light() {
        for preset in PRESETS {
            let accent = fit(preset).unwrap_or_else(|e| panic!("{preset}: {e}"));
            assert!(
                contrast(&accent.light, LIGHT_PANEL) >= MIN_CONTRAST,
                "{preset}"
            );
            assert!(
                contrast(&accent.dark, DARK_PANEL) >= MIN_CONTRAST,
                "{preset}"
            );
        }
        // The shipped red clears 4.5:1 on white already, so it comes back untouched.
        assert_eq!(fit("#DC1F2B").unwrap().light, "#dc1f2b");
    }

    #[test]
    fn black_greys_and_white_are_refused() {
        assert_eq!(fit("#000000"), Err(Refusal::TooDark));
        assert_eq!(fit("#111"), Err(Refusal::TooDark));
        assert_eq!(fit("#3a0000"), Err(Refusal::TooDark), "near-black red");
        assert_eq!(fit("#808080"), Err(Refusal::Colourless));
        assert_eq!(
            fit("#6b7280"),
            Err(Refusal::Colourless),
            "slate is still a grey"
        );
        assert_eq!(fit("#fafafa"), Err(Refusal::TooLight));
        assert_eq!(fit("#ffffff"), Err(Refusal::TooLight));
        assert!(matches!(fit("red"), Err(Refusal::NotAColour(_))));
    }

    #[test]
    fn a_pale_colour_is_darkened_for_light_and_kept_for_dark() {
        let accent = fit("#fde047").expect("a yellow is a colour");
        assert_ne!(accent.light, "#fde047");
        assert!(contrast(&accent.light, LIGHT_PANEL) >= MIN_CONTRAST);
        assert_eq!(accent.dark, "#fde047", "already legible on the dark panel");
    }

    #[test]
    fn a_deep_colour_is_lightened_for_dark() {
        let accent = fit("#1e3a8a").expect("navy is a colour");
        assert_eq!(accent.light, "#1e3a8a");
        assert_ne!(accent.dark, "#1e3a8a");
        assert!(contrast(&accent.dark, DARK_PANEL) >= MIN_CONTRAST);
    }

    /// The property the whole module exists for, over a sweep rather than a few picks: whatever
    /// is accepted comes back legible in both themes, and keeps its hue.
    #[test]
    fn every_accepted_colour_is_legible_in_both_themes() {
        for h in (0..360).step_by(10) {
            for (l, c) in [
                (0.4, 0.12),
                (0.55, 0.2),
                (0.7, 0.15),
                (0.85, 0.1),
                (0.93, 0.07),
            ] {
                let hex = from_oklch_in_gamut(Lch {
                    l,
                    c,
                    h: (h as f64).to_radians(),
                });
                let Ok(accent) = fit(&hex) else { continue };
                assert!(
                    contrast(&accent.light, LIGHT_PANEL) >= MIN_CONTRAST,
                    "{hex} light"
                );
                assert!(
                    contrast(&accent.dark, DARK_PANEL) >= MIN_CONTRAST,
                    "{hex} dark"
                );
            }
        }
    }
}
