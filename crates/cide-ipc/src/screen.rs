//! A terminal screen as structured rows, for a reader that has no VT emulator. (M72)
//!
//! > *"Need to develop an mobile app (android/ios) that will allowe me to connect to cide and
//! > see projects of opened cide and select the project and see it's activly running claude
//! > (and agents) sessions … In opened session i can type and select answers from claude
//! > session question."*
//!
//! Every terminal in cide is mirrored by a `vt100::Parser` in `cide-pty`, and until now the
//! only way to ask that mirror anything was to ask for **bytes** — `screen_state`,
//! `reattach_state`, `full_state` all emit escape sequences for another terminal emulator to
//! parse. That is exactly right for xterm.js, which is the only consumer the mirror has ever
//! had, and useless for a React Native app, which has no emulator to parse them with and no
//! prospect of one that is not a second rendering stack inside a WebView.
//!
//! So this is the same screen said the other way round: rows of styled runs, a cursor, and the
//! two input modes a caller needs in order to encode a keystroke correctly. It is produced by
//! [`cide-pty`](https://docs.rs/) from the mirror it already keeps, diffed by `cide-remote`, and
//! parsed by `cide-claude` when it wants to know what a permission prompt is offering.
//!
//! # What this is not
//!
//! **A screen model, not a byte recorder.** The mirror is a grid of cells, so everything that
//! is not a cell is already gone by the time this module sees it: OSC 8 hyperlinks (`vt100`
//! drops those from `state_formatted` too, which is why a reattached pane's existing lines are
//! inert), OSC 52 clipboard writes, DEC 2026 synchronised-update framing, sixel and every other
//! image protocol, and the window title. A TUI that depends on the receiving terminal's own
//! quirks will look approximate.
//!
//! Blink and strikethrough are a different case and worth naming separately, because "we lost
//! them" would be a lie: `vt100::Cell` models exactly bold, dim, italic, underline and inverse,
//! so a consumer cannot draw what the mirror never recorded in the first place.
//!
//! # Why the flags are a bitfield and the colours are `Option`
//!
//! Both for the same reason, and it is measured rather than aesthetic. A run is the unit this
//! wire is made of, an active screen is a few hundred of them a second, and the common run is
//! plain text in the default colours — so five boolean keys and two tagged colour objects per
//! run would be most of the payload spelling out *nothing in particular*. A zero `flags` and two
//! absent colours are omitted entirely, which is what [`crate::screen::flags`] and the `Option`s
//! below buy. `#[ts(optional)]` is paired with `skip_serializing_if` on every one of them: on
//! its own it changes the emitted *type* and not what serde writes, which is how a field the
//! TypeScript calls `undefined` arrives as `null` and the next property access throws.
//!
//! The two fields that are skipped but are *not* `Option` — [`StyleRun::flags`] and
//! [`ScreenLine::wrapped`] — carry `#[ts(as = "Option<…>", optional)]`, and that is not
//! decoration. `#[ts(optional)]` alone refuses to compile on a `u8` or a `bool`, so the obvious
//! repair is to drop it, and dropping it is precisely the drift above: serde would go on
//! omitting the field while the generated TypeScript went on calling it required. `as` tells the
//! generator the shape the *wire* has, which is the shape `skip_serializing_if` gives it.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The attribute bits carried in [`StyleRun::flags`].
///
/// A module of constants rather than a `bitflags` dependency: there are six of them, they are
/// never combined arithmetically, and the one consumer that is not Rust reads them out of
/// `contract/protocol.ts`, which `cargo xtask codegen` writes from *this* file. A crate that
/// generated its own TypeScript would be the second spelling.
pub mod flags {
    pub const BOLD: u8 = 1 << 0;
    pub const DIM: u8 = 1 << 1;
    pub const ITALIC: u8 = 1 << 2;
    pub const UNDERLINE: u8 = 1 << 3;
    pub const INVERSE: u8 = 1 << 4;
    /// The run begins with a double-width cell.
    ///
    /// Carried because a consumer laying out a monospace grid must advance two columns for it,
    /// and because the *continuation* cell `vt100` keeps beside it is deliberately not emitted
    /// at all — emitting it as a space would put every CJK line one column out from its first
    /// wide glyph onward, with nothing on screen to say why.
    pub const WIDE: u8 = 1 << 5;
}

/// One cell colour, or the terminal's default when absent.
///
/// An index is kept as an index and never resolved to RGB here. The palette belongs to whoever
/// paints: cide's own editor colours come from `crate::theme`, and a phone reading this over a
/// network is entitled to a different, higher-contrast palette for a five-inch screen. Resolving
/// on this side would take that choice away and make every capture carry one machine's theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum ScreenColor {
    /// One of the 256 palette entries.
    Idx { index: u8 },
    /// A direct colour, as the program asked for it.
    Rgb { r: u8, g: u8, b: u8 },
}

/// A span of adjacent cells sharing one appearance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StyleRun {
    pub text: String,
    /// `None` is the terminal's default foreground.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub fg: Option<ScreenColor>,
    /// `None` is the terminal's default background.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bg: Option<ScreenColor>,
    /// A mask of [`flags`]. Absent means none of them.
    #[serde(default, skip_serializing_if = "is_zero")]
    #[ts(as = "Option<u8>", optional)]
    pub flags: u8,
}

fn is_zero(flags: &u8) -> bool {
    *flags == 0
}

/// Where the cursor is. Absent from a capture means hidden, not at the origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Cursor {
    pub row: u16,
    pub col: u16,
}

/// One row of the grid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScreenLine {
    pub row: u16,
    /// The line continues into the next one because the text ran off the edge, rather than
    /// because the program asked for a new line.
    ///
    /// A consumer narrower than the capture needs this to re-wrap without inventing breaks the
    /// program never wrote: a soft-wrapped paragraph may be re-flowed and two separate lines
    /// may not.
    #[serde(default, skip_serializing_if = "is_false")]
    #[ts(as = "Option<bool>", optional)]
    pub wrapped: bool,
    pub runs: Vec<StyleRun>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl ScreenLine {
    /// A cheap non-cryptographic hash of everything that decides how this line looks.
    ///
    /// The one producer of it, in `cide-ipc` for [`crate::TaskRow::of`]'s stated reason: two
    /// crates need the number and neither may depend on the other. `cide-pty` has the lines and
    /// `cide-remote` has the per-watcher cache it compares them against, and a second
    /// implementation of "has this line changed" would be two plausible answers to a question
    /// whose wrong answer is a row that silently stops repainting.
    ///
    /// **Deliberately not on the wire.** It is the sender's bookkeeping, not the receiver's: a
    /// consumer is told *which* lines changed and never has to decide. Keeping it off also keeps
    /// a `u64` out of a JSON payload, where it would have to be a string or a `bigint` to survive
    /// at all — see [`crate::properties`] for the same trap in a different field.
    ///
    /// FNV-1a rather than `blake3`, which this crate does not depend on and does not want to:
    /// this compares two values that are both in this process, so a collision costs one
    /// unrepainted row until the next change, and nothing here is adversarial. The screen digest
    /// that guards a permission answer *is* adversarial and is a real hash, computed elsewhere.
    pub fn digest(&self) -> u64 {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        let mut hash = OFFSET;
        let mut eat = |byte: u8| {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(PRIME);
        };
        eat(u8::from(self.wrapped));
        for run in &self.runs {
            for byte in run.text.as_bytes() {
                eat(*byte);
            }
            // A separator, so two runs that differ only in where the boundary falls — "ab"+"c"
            // against "a"+"bc" — do not hash alike. They render differently the moment their
            // colours differ, which is the only reason they are two runs at all.
            eat(0xff);
            eat(run.flags);
            colour_bytes(run.fg, &mut eat);
            colour_bytes(run.bg, &mut eat);
        }
        hash
    }
}

fn colour_bytes(colour: Option<ScreenColor>, eat: &mut impl FnMut(u8)) {
    match colour {
        None => eat(0),
        Some(ScreenColor::Idx { index }) => {
            eat(1);
            eat(index);
        }
        Some(ScreenColor::Rgb { r, g, b }) => {
            eat(2);
            eat(r);
            eat(g);
            eat(b);
        }
    }
}

/// Everything about a grid that is not its contents.
///
/// Split out of [`ScreenCapture`] because an incremental update carries it too — a consumer must
/// be able to tell a repaint of the same grid from a *different* grid — and two structs each
/// spelling out five fields is two places for them to disagree about what a screen is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScreenInfo {
    pub cols: u16,
    pub rows: u16,
    /// The alternate screen is engaged — a fullscreen TUI is drawing.
    ///
    /// Consumers need it for three separate decisions: there is no scrollback to page through,
    /// re-wrapping would destroy a layout drawn in columns, and a transition either way is a
    /// change of grid rather than of contents, so a diff across one is meaningless.
    pub alt: bool,
    /// DECCKM: the cursor keys must be encoded `SS3 A` rather than `CSI A`.
    ///
    /// On the capture because it is the only thing that knows, and because a caller encoding a
    /// keystroke from a phone has no other way to find out. Guessing it is how an arrow key
    /// arrives in a TUI as a literal `A`.
    pub app_cursor: bool,
    /// The program asked for bracketed paste, so pasted text must be wrapped in `CSI 200~` /
    /// `CSI 201~` and a caller must not wrap it when it did not.
    pub bracketed_paste: bool,
    /// How the program wants the mouse reported, `None` when it has not asked at all. (M76)
    ///
    /// On the capture for [`Self::app_cursor`]'s reason, and the consequence of guessing it is
    /// worse here than there. A program that never enabled mouse reporting reads whatever a
    /// wheel would have encoded as **typed input**: scrolling a phone would put
    /// `[<64;40;12M` on a shell's command line. So a caller with a wheel to send has to be told,
    /// and `cide_remote::keys::wheel` refuses rather than guessing.
    ///
    /// The *encoding* rides with the mode because the two are set by separate escape sequences
    /// and a program may enable tracking without asking for SGR — in which case the report is
    /// the original single-byte form, which cannot describe a column past 223.
    #[serde(default)]
    pub mouse: MouseReporting,
}

/// What a program has asked for by way of mouse reports. Mirrors `vt100`'s two modes, flattened
/// to what a sender of a **wheel** needs: whether to send anything, and in which encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum MouseReporting {
    /// Nothing asked for. A wheel must not be sent — see [`ScreenInfo::mouse`].
    #[default]
    Off,
    /// Tracking on, reports in the original `CSI M` form with byte-offset coordinates.
    Legacy,
    /// Tracking on, reports in the SGR form (`CSI < b ; x ; y M`), which is what anything
    /// modern asks for and what has no coordinate ceiling.
    Sgr,
}

/// A terminal's visible grid at one instant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScreenCapture {
    pub info: ScreenInfo,
    /// `None` when the cursor is hidden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cursor: Option<Cursor>,
    pub lines: Vec<ScreenLine>,
}

/// A window onto the retained scrollback, counted from the oldest line.
///
/// `from_top` and `depth` rather than "the last N lines" because the scrollback grows from the
/// bottom while a page is being read: an offset from the end names a different line every time
/// the child prints anything, and a reader paging backwards would see rows repeat and rows
/// vanish with nothing wrong anywhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScrollbackCapture {
    pub from_top: u32,
    /// How many lines are retained in total, so a reader can size a scrollbar and know when it
    /// has reached the beginning.
    pub depth: u32,
    pub lines: Vec<ScreenLine>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> StyleRun {
        StyleRun {
            text: text.to_owned(),
            fg: None,
            bg: None,
            flags: 0,
        }
    }

    fn line(runs: Vec<StyleRun>) -> ScreenLine {
        ScreenLine {
            row: 0,
            wrapped: false,
            runs,
        }
    }

    #[test]
    fn a_plain_run_spends_no_bytes_on_its_defaults() {
        let json = serde_json::to_string(&run("hello")).expect("serialises");
        assert_eq!(json, r#"{"text":"hello"}"#);
    }

    #[test]
    fn an_absent_colour_round_trips_as_the_default() {
        let json = serde_json::to_string(&run("x")).expect("serialises");
        let back: StyleRun = serde_json::from_str(&json).expect("parses");
        assert_eq!(back.fg, None);
        assert_eq!(back.bg, None);
        assert_eq!(back.flags, 0);
    }

    /// The whole point of the field: it must move when anything visible moves.
    #[test]
    fn the_digest_moves_with_every_visible_difference() {
        let base = line(vec![run("hello")]);
        let baseline = base.digest();

        let mut text = base.clone();
        text.runs[0].text = "hellp".to_owned();
        assert_ne!(text.digest(), baseline, "text");

        let mut bold = base.clone();
        bold.runs[0].flags = flags::BOLD;
        assert_ne!(bold.digest(), baseline, "flags");

        let mut fg = base.clone();
        fg.runs[0].fg = Some(ScreenColor::Idx { index: 1 });
        assert_ne!(fg.digest(), baseline, "fg");

        let mut bg = base.clone();
        bg.runs[0].bg = Some(ScreenColor::Idx { index: 1 });
        assert_ne!(bg.digest(), baseline, "bg");

        let mut wrapped = base.clone();
        wrapped.wrapped = true;
        assert_ne!(wrapped.digest(), baseline, "wrapped");

        // Indexed 1 and RGB (1,0,0) are different colours and must not hash alike.
        let mut idx = base.clone();
        idx.runs[0].fg = Some(ScreenColor::Idx { index: 1 });
        let mut rgb = base.clone();
        rgb.runs[0].fg = Some(ScreenColor::Rgb { r: 1, g: 0, b: 0 });
        assert_ne!(idx.digest(), rgb.digest(), "indexed against direct");
    }

    /// Where the boundary between two runs falls is a real difference, because two runs only
    /// exist when something about them differs.
    #[test]
    fn the_digest_separates_runs() {
        let one = line(vec![run("ab"), run("c")]);
        let other = line(vec![run("a"), run("bc")]);
        assert_ne!(one.digest(), other.digest());
    }

    /// A line's own row number is not part of how it *looks*, and the diff keys on the row
    /// anyway. Hashing it would make a scrolled screen report every line as changed.
    #[test]
    fn the_digest_ignores_the_row_number() {
        let mut moved = line(vec![run("hello")]);
        let baseline = moved.digest();
        moved.row = 17;
        assert_eq!(moved.digest(), baseline);
    }

    #[test]
    fn a_hidden_cursor_is_absent_rather_than_at_the_origin() {
        let capture = ScreenCapture {
            info: ScreenInfo {
                cols: 80,
                rows: 24,
                alt: false,
                app_cursor: false,
                bracketed_paste: false,
                mouse: MouseReporting::Off,
            },
            cursor: None,
            lines: vec![],
        };
        let json = serde_json::to_string(&capture).expect("serialises");
        assert!(!json.contains("cursor"), "{json}");
        let back: ScreenCapture = serde_json::from_str(&json).expect("parses");
        assert_eq!(back.cursor, None);
    }
}
