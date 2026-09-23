//! A semantic keystroke, turned into the bytes a terminal expects. (M72)
//!
//! A device sends *up*; this decides whether the child gets `CSI A` or `SS3 A`. It has to be
//! here rather than there, because the answer depends on DECCKM and on bracketed-paste mode —
//! modes of the child, held by the screen mirror and by nothing else. A phone encoding its own
//! bytes would be guessing at state it cannot see, and the symptom is an arrow key arriving in a
//! TUI as a literal `A`.
//!
//! # This is a second spelling of terminal key encoding, and saying so is the point
//!
//! It is **not** a duplicate of `ui/src/terminal/keys.ts`. That module is cide's own chord
//! handling — the three chords the terminal claims for itself, and Shift+Enter — layered over
//! **xterm.js's** encoder, which is where the desktop's `CSI`/`SS3` decisions actually happen and
//! which cide does not own. What is here is the phone's xterm: the ordinary DEC and xterm
//! encodings, written out because there is no emulator on the other end to write them.
//!
//! One rule is genuinely shared, and both files carry a comment naming the other:
//!
//! > **Shift+Enter is `ESC CR`, never `CSI 13;2u`.**
//!
//! `keys.ts` argues it at length. The short version is that `CSI 13;2u` is the tidier encoding
//! and requires the application to have *requested* that mode — sent unsolicited it arrives as a
//! visible `[13;2u` in the prompt — while `ESC CR` is what `claude /terminal-setup` writes into
//! VS Code's own keybindings, and this is a handshake with one specific program rather than a
//! general terminal feature. [`shift_enter_is_esc_cr`] pins it on this side.
//!
//! # Modifiers
//!
//! xterm's parameterised form: a key that takes modifiers is sent as `CSI 1;<m> X` or
//! `CSI <n>;<m> ~`, where `m` is `1 + (shift?1) + (alt?2) + (ctrl?4)`. With no modifiers the
//! parameter is omitted entirely rather than sent as `1`, because a program matching the plain
//! sequence literally — which plenty do — would not recognise `CSI 1A`.

use cide_ipc::remote::{KeyEvent, KeyName};
use cide_ipc::screen::ScreenInfo;

const ESC: u8 = 0x1b;

/// The bytes for one keystroke, given the child's current modes.
///
/// An empty answer means *nothing to send*: an unmodified `Char` with no text, or a function key
/// outside F1–F12. Refusing by sending nothing rather than by guessing is deliberate — the
/// alternative to an unknown key is a wrong key, and a wrong key in a terminal is an action.
pub fn encode(event: &KeyEvent, modes: &ScreenInfo) -> Vec<u8> {
    let m = modifier(event);
    match event.key {
        KeyName::Char => character(event),

        // There is no modifier encoding for Return in DEC's scheme, so Shift+Enter has to be an
        // agreement rather than a derivation. See the module header.
        KeyName::Enter if event.shift => vec![ESC, b'\r'],
        KeyName::Enter => alt_prefixed(event, vec![b'\r']),

        KeyName::Escape => alt_prefixed(event, vec![ESC]),

        // `CSI Z` is back-tab. Sent for Shift+Tab because that is what readline and every TUI
        // that moves focus backwards listens for.
        KeyName::Tab if event.shift => vec![ESC, b'[', b'Z'],
        KeyName::Tab => alt_prefixed(event, vec![b'\t']),

        // DEL, not BS. A terminal's Backspace has been `0x7f` since the VT220, and a child that
        // wants the other one asks for it through termios rather than through the key.
        KeyName::Backspace if event.ctrl => alt_prefixed(event, vec![0x08]),
        KeyName::Backspace => alt_prefixed(event, vec![0x7f]),

        KeyName::Delete => tilde(3, m),
        KeyName::Insert => tilde(2, m),
        KeyName::PageUp => tilde(5, m),
        KeyName::PageDown => tilde(6, m),

        KeyName::Up => cursor(b'A', m, modes),
        KeyName::Down => cursor(b'B', m, modes),
        KeyName::Right => cursor(b'C', m, modes),
        KeyName::Left => cursor(b'D', m, modes),
        KeyName::Home => cursor(b'H', m, modes),
        KeyName::End => cursor(b'F', m, modes),

        KeyName::Function { n } => function(n, m),
    }
}

/// Pasted text, bracketed only when the child asked for it.
///
/// Wrapping unconditionally would put a literal `[200~` into every program that never requested
/// the mode, and not wrapping when it was requested takes away the child's only way of telling
/// typed text from pasted — which is exactly how a paste into a TUI submits itself halfway
/// through.
pub fn paste(text: &str, modes: &ScreenInfo) -> Vec<u8> {
    if !modes.bracketed_paste {
        return text.as_bytes().to_vec();
    }
    let mut out = Vec::with_capacity(text.len() + 12);
    out.extend_from_slice(b"\x1b[200~");
    out.extend_from_slice(text.as_bytes());
    out.extend_from_slice(b"\x1b[201~");
    out
}

/// A wheel, as the child asked for it — or **nothing at all**. (M76)
///
/// # Why a scroll has to be encoded here and cannot be a key
///
/// A full-screen program owns the screen and keeps no scrollback: `claude` takes the alternate
/// screen (`CSI ?1049h`, measured) and the terminal therefore has nothing to page back through.
/// The transcript is not in a ring anywhere — it is in the program, which redraws when told to
/// scroll. So a device that wants to see earlier output has to ask the *program*, and the way a
/// terminal asks is a mouse report.
///
/// # The refusal is the whole safety of it
///
/// A mouse report is only input if the program asked for one. To a program that did not, these
/// bytes are **typed characters**: a wheel sent to a shell puts `[<64;40;12M` on its command
/// line, and a wheel sent to a shell at a `sudo` prompt puts it somewhere worse. So this answers
/// empty unless the mirror says tracking is on, and the emptiness is load-bearing rather than
/// defensive — [`ScreenInfo::mouse`] carries the argument from the other end.
///
/// # Buttons 64 and 65, one report per line
///
/// xterm reports a wheel as a *button press* with bit 6 set: 64 is up, 65 is down, and there is
/// no release. One report per line of scroll, because that is what a real wheel emits and what
/// every consumer counts; a single report with a repeat count is not a thing the protocol has.
///
/// The position is the **centre of the grid**, and it is not arbitrary: a program with more than
/// one scrollable region scrolls the one under the pointer, and the centre is the only answer a
/// device with no pointer can give that is inside the region somebody is reading. `1,1` would
/// aim at whatever sits in the top-left corner, which in a TUI is usually a border.
///
/// `lines` is signed: negative scrolls **up**, towards older output, which is the direction a
/// finger drags downwards. Zero sends nothing.
pub fn wheel(lines: i16, modes: &ScreenInfo) -> Vec<u8> {
    use cide_ipc::screen::MouseReporting;
    if lines == 0 || modes.mouse == MouseReporting::Off {
        return Vec::new();
    }
    // Capped for the same reason `keys::encode` refuses a key it does not know: a device asking
    // for four hundred lines in one frame is a bug on that side, and the bytes would be sent to
    // a program that has to redraw for each one.
    let count = usize::from(lines.unsigned_abs().min(MAX_WHEEL_LINES));
    let button: u8 = if lines < 0 { 64 } else { 65 };
    // 1-based, as a terminal counts; the centre row and column of the grid the child is drawing.
    let col = modes.cols / 2 + 1;
    let row = modes.rows / 2 + 1;

    let mut out = Vec::with_capacity(count * 16);
    for _ in 0..count {
        match modes.mouse {
            MouseReporting::Sgr => {
                out.extend_from_slice(format!("\x1b[<{button};{col};{row}M").as_bytes());
            }
            // The original form: three bytes, each offset by 32, which is why it cannot describe
            // a coordinate past 223 — clamped rather than wrapped, because a wrapped column is a
            // click somewhere else.
            MouseReporting::Legacy => {
                out.extend_from_slice(b"\x1b[M");
                out.push(32 + button);
                out.push(32 + u8::try_from(col.min(223)).unwrap_or(223));
                out.push(32 + u8::try_from(row.min(223)).unwrap_or(223));
            }
            MouseReporting::Off => unreachable!("refused above"),
        }
    }
    out
}

/// How many lines one scroll frame may carry. See [`wheel`].
pub const MAX_WHEEL_LINES: u16 = 32;

/// What a device's PgUp/PgDn (`ClientBody::ScrollView`) turns into. (M91)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Page {
    /// The normal screen: the history is the **terminal's**, so the desk's pane scrolls its own
    /// scrollback and the device pages its copy. Nothing is written to the child — to a shell,
    /// `ESC[5~` is a key readline ignores, which is exactly how PgUp used to do nothing.
    Desk,
    /// An alternate screen: the program owns the transcript, so the bytes go to it and both ends
    /// see its redraw.
    Program(Vec<u8>),
}

/// Decide [`Page`] from the child's modes, read off the mirror at the moment of the press.
///
/// On an alternate screen that asked for the mouse, a wheel of about a screenful — the rows less
/// two, so a line of context survives the turn, capped at [`MAX_WHEEL_LINES`]. One that did not
/// ask gets the PgUp/PgDn **key** instead, which is what a full-screen pager binds and what this
/// button sent before M91; a wheel there would arrive as typed characters.
pub fn page(pages: i8, modes: &ScreenInfo) -> Page {
    use cide_ipc::screen::MouseReporting;
    if !modes.alt {
        return Page::Desk;
    }
    if pages == 0 {
        return Page::Program(Vec::new());
    }
    if modes.mouse != MouseReporting::Off {
        let per_page = i16::try_from(modes.rows.saturating_sub(2).max(1)).unwrap_or(i16::MAX);
        let lines = per_page.saturating_mul(i16::from(pages));
        let capped = lines.clamp(-(MAX_WHEEL_LINES as i16), MAX_WHEEL_LINES as i16);
        return Page::Program(wheel(capped, modes));
    }
    let key = KeyEvent {
        key: if pages < 0 {
            KeyName::PageUp
        } else {
            KeyName::PageDown
        },
        text: None,
        ctrl: false,
        alt: false,
        shift: false,
    };
    Page::Program(encode(&key, modes).repeat(usize::from(pages.unsigned_abs())))
}

/// xterm's modifier parameter, or `None` when there are no modifiers to report.
fn modifier(event: &KeyEvent) -> Option<u8> {
    let bits = u8::from(event.shift) | (u8::from(event.alt) << 1) | (u8::from(event.ctrl) << 2);
    (bits != 0).then_some(1 + bits)
}

fn alt_prefixed(event: &KeyEvent, mut bytes: Vec<u8>) -> Vec<u8> {
    if event.alt {
        let mut out = vec![ESC];
        out.append(&mut bytes);
        return out;
    }
    bytes
}

fn character(event: &KeyEvent) -> Vec<u8> {
    let Some(text) = event.text.as_deref().filter(|t| !t.is_empty()) else {
        return Vec::new();
    };
    let mut bytes = if event.ctrl {
        match control(text) {
            Some(byte) => vec![byte],
            // Ctrl with a character that has no control code is the character itself. A terminal
            // has nothing else to say, and swallowing it would make Ctrl a key that sometimes
            // eats your typing.
            None => text.as_bytes().to_vec(),
        }
    } else {
        text.as_bytes().to_vec()
    };
    if event.alt {
        let mut out = vec![ESC];
        out.append(&mut bytes);
        return out;
    }
    bytes
}

/// The C0 code for a Ctrl chord, by the ASCII rule: clear the top three bits.
fn control(text: &str) -> Option<u8> {
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        '@' | ' ' => Some(0),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        // Ctrl+? is DEL, which is what a Ctrl+Backspace on some keyboards produces.
        '?' => Some(0x7f),
        _ => None,
    }
}

/// `CSI <final>`, or `SS3 <final>` under DECCKM, or the parameterised form when modified.
///
/// The modified form is always `CSI`, never `SS3`: `SS3` takes no parameters, so there is nowhere
/// to put the modifier, and every terminal that implements modified cursor keys does it this way.
fn cursor(final_byte: u8, m: Option<u8>, modes: &ScreenInfo) -> Vec<u8> {
    match m {
        Some(m) => format!("\x1b[1;{m}{}", final_byte as char).into_bytes(),
        None if modes.app_cursor => vec![ESC, b'O', final_byte],
        None => vec![ESC, b'[', final_byte],
    }
}

fn tilde(n: u8, m: Option<u8>) -> Vec<u8> {
    match m {
        Some(m) => format!("\x1b[{n};{m}~").into_bytes(),
        None => format!("\x1b[{n}~").into_bytes(),
    }
}

/// F1–F12, in the shapes xterm sends them.
///
/// F1–F4 are `SS3` and the rest are `CSI … ~`, which is an inconsistency of the standard rather
/// than of this function; the numbering skips 16 and 22 for the same reason.
fn function(n: u8, m: Option<u8>) -> Vec<u8> {
    let ss3 = |final_byte: u8| match m {
        Some(m) => format!("\x1b[1;{m}{}", final_byte as char).into_bytes(),
        None => vec![ESC, b'O', final_byte],
    };
    match n {
        1 => ss3(b'P'),
        2 => ss3(b'Q'),
        3 => ss3(b'R'),
        4 => ss3(b'S'),
        5 => tilde(15, m),
        6 => tilde(17, m),
        7 => tilde(18, m),
        8 => tilde(19, m),
        9 => tilde(20, m),
        10 => tilde(21, m),
        11 => tilde(23, m),
        12 => tilde(24, m),
        // Not guessed at. See [`encode`].
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modes(app_cursor: bool, bracketed_paste: bool) -> ScreenInfo {
        ScreenInfo {
            cols: 80,
            rows: 24,
            alt: false,
            app_cursor,
            bracketed_paste,
            mouse: cide_ipc::screen::MouseReporting::Off,
        }
    }

    fn tracking(mouse: cide_ipc::screen::MouseReporting) -> ScreenInfo {
        ScreenInfo {
            mouse,
            ..modes(false, false)
        }
    }

    #[test]
    fn a_page_on_the_normal_screen_scrolls_the_desk_and_writes_nothing() {
        assert_eq!(page(-1, &modes(false, false)), Page::Desk);
        assert_eq!(
            page(1, &tracking(cide_ipc::screen::MouseReporting::Sgr)),
            Page::Desk
        );
    }

    #[test]
    fn a_page_on_an_alternate_screen_with_the_mouse_is_a_screenful_of_wheel() {
        let alt = ScreenInfo {
            alt: true,
            rows: 12,
            ..tracking(cide_ipc::screen::MouseReporting::Sgr)
        };
        let Page::Program(bytes) = page(-1, &alt) else {
            panic!("an alternate screen is the program's")
        };
        // Ten lines up (twelve rows less two), each one SGR wheel-up at the centre.
        assert_eq!(bytes, "\x1b[<64;41;7M".repeat(10).into_bytes());

        // A tall screen is capped rather than asking for a hundred redraws.
        let tall = ScreenInfo { rows: 200, ..alt };
        let Page::Program(bytes) = page(1, &tall) else {
            panic!("an alternate screen is the program's")
        };
        assert_eq!(
            bytes,
            "\x1b[<65;41;101M"
                .repeat(usize::from(MAX_WHEEL_LINES))
                .into_bytes()
        );
    }

    #[test]
    fn a_page_on_an_alternate_screen_without_the_mouse_is_the_key() {
        let alt = ScreenInfo {
            alt: true,
            ..modes(false, false)
        };
        assert_eq!(page(-2, &alt), Page::Program(b"\x1b[5~\x1b[5~".to_vec()));
        assert_eq!(page(1, &alt), Page::Program(b"\x1b[6~".to_vec()));
    }

    fn key(key: KeyName) -> KeyEvent {
        KeyEvent {
            key,
            text: None,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    fn typed(text: &str) -> KeyEvent {
        KeyEvent {
            key: KeyName::Char,
            text: Some(text.to_owned()),
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    fn bytes(event: &KeyEvent, app_cursor: bool) -> String {
        String::from_utf8_lossy(&encode(event, &modes(app_cursor, false))).into_owned()
    }

    #[test]
    fn ordinary_typing_is_its_own_utf8() {
        assert_eq!(bytes(&typed("a"), false), "a");
        assert_eq!(bytes(&typed("é"), false), "é");
        assert_eq!(bytes(&typed("😀"), false), "😀");
    }

    #[test]
    fn a_control_chord_is_the_ascii_rule() {
        let mut ctrl_c = typed("c");
        ctrl_c.ctrl = true;
        assert_eq!(encode(&ctrl_c, &modes(false, false)), vec![0x03]);

        let mut ctrl_d = typed("D");
        ctrl_d.ctrl = true;
        assert_eq!(encode(&ctrl_d, &modes(false, false)), vec![0x04]);

        for (text, byte) in [
            ("@", 0x00),
            (" ", 0x00),
            ("[", 0x1b),
            ("\\", 0x1c),
            ("]", 0x1d),
            ("^", 0x1e),
            ("_", 0x1f),
            ("?", 0x7f),
        ] {
            let mut event = typed(text);
            event.ctrl = true;
            assert_eq!(
                encode(&event, &modes(false, false)),
                vec![byte],
                "ctrl+{text}"
            );
        }
    }

    /// Ctrl is a modifier, not a filter. A chord with no control code must still type its
    /// character, or Ctrl becomes a key that sometimes eats what you pressed.
    #[test]
    fn a_control_chord_with_no_code_is_the_character() {
        let mut event = typed("é");
        event.ctrl = true;
        assert_eq!(bytes(&event, false), "é");
    }

    #[test]
    fn alt_is_an_escape_prefix() {
        let mut event = typed("b");
        event.alt = true;
        assert_eq!(encode(&event, &modes(false, false)), vec![ESC, b'b']);

        let mut both = typed("c");
        both.alt = true;
        both.ctrl = true;
        assert_eq!(encode(&both, &modes(false, false)), vec![ESC, 0x03]);
    }

    /// The rule shared with `ui/src/terminal/keys.ts`, which carries the long argument. `CSI
    /// 13;2u` would be the tidier encoding and arrives as visible junk in a prompt that never
    /// asked for the mode.
    #[test]
    fn shift_enter_is_esc_cr() {
        let mut event = key(KeyName::Enter);
        event.shift = true;
        assert_eq!(encode(&event, &modes(false, false)), vec![ESC, b'\r']);
        assert_eq!(
            encode(&key(KeyName::Enter), &modes(false, false)),
            vec![b'\r']
        );
    }

    /// The mode the phone cannot see, and the reason the encoder is on this side at all.
    #[test]
    fn a_cursor_key_follows_decckm() {
        assert_eq!(bytes(&key(KeyName::Up), false), "\x1b[A");
        assert_eq!(bytes(&key(KeyName::Up), true), "\x1bOA");
        assert_eq!(bytes(&key(KeyName::Left), true), "\x1bOD");
        assert_eq!(bytes(&key(KeyName::Home), true), "\x1bOH");
    }

    /// The parameter is omitted when there is nothing to say, because a program matching the
    /// plain sequence literally would not recognise `CSI 1A`. And a modified key is `CSI` even
    /// under DECCKM, because `SS3` has nowhere to put a parameter.
    #[test]
    fn a_modified_cursor_key_is_the_parameterised_form() {
        let mut shifted = key(KeyName::Right);
        shifted.shift = true;
        assert_eq!(bytes(&shifted, false), "\x1b[1;2C");
        assert_eq!(
            bytes(&shifted, true),
            "\x1b[1;2C",
            "SS3 cannot carry a modifier"
        );

        let mut ctrl = key(KeyName::Left);
        ctrl.ctrl = true;
        assert_eq!(bytes(&ctrl, false), "\x1b[1;5D");

        let mut all = key(KeyName::Up);
        all.shift = true;
        all.alt = true;
        all.ctrl = true;
        assert_eq!(bytes(&all, false), "\x1b[1;8A");
    }

    #[test]
    fn the_editing_keys_are_tilde_sequences() {
        assert_eq!(bytes(&key(KeyName::Delete), false), "\x1b[3~");
        assert_eq!(bytes(&key(KeyName::Insert), false), "\x1b[2~");
        assert_eq!(bytes(&key(KeyName::PageUp), false), "\x1b[5~");
        assert_eq!(bytes(&key(KeyName::PageDown), false), "\x1b[6~");

        let mut ctrl_end = key(KeyName::PageDown);
        ctrl_end.ctrl = true;
        assert_eq!(bytes(&ctrl_end, false), "\x1b[6;5~");
    }

    #[test]
    fn backspace_is_del_and_tab_has_a_back_tab() {
        assert_eq!(
            encode(&key(KeyName::Backspace), &modes(false, false)),
            vec![0x7f]
        );
        let mut ctrl = key(KeyName::Backspace);
        ctrl.ctrl = true;
        assert_eq!(encode(&ctrl, &modes(false, false)), vec![0x08]);

        assert_eq!(
            encode(&key(KeyName::Tab), &modes(false, false)),
            vec![b'\t']
        );
        let mut shifted = key(KeyName::Tab);
        shifted.shift = true;
        assert_eq!(bytes(&shifted, false), "\x1b[Z");
    }

    #[test]
    fn the_function_keys_are_xterms_shapes_and_nothing_else_is_guessed() {
        assert_eq!(bytes(&key(KeyName::Function { n: 1 }), false), "\x1bOP");
        assert_eq!(bytes(&key(KeyName::Function { n: 4 }), false), "\x1bOS");
        assert_eq!(bytes(&key(KeyName::Function { n: 5 }), false), "\x1b[15~");
        assert_eq!(bytes(&key(KeyName::Function { n: 12 }), false), "\x1b[24~");
        // The alternative to an unknown key is a wrong key, and a wrong key is an action.
        assert!(encode(&key(KeyName::Function { n: 13 }), &modes(false, false)).is_empty());
        assert!(encode(&key(KeyName::Function { n: 0 }), &modes(false, false)).is_empty());
    }

    #[test]
    fn a_char_with_no_text_sends_nothing() {
        assert!(encode(&key(KeyName::Char), &modes(false, false)).is_empty());
        assert!(encode(&typed(""), &modes(false, false)).is_empty());
    }

    /// Both halves matter. Wrapping unconditionally puts a literal `[200~` into a program that
    /// never asked; not wrapping when it did asks takes away its only way to tell a paste from
    /// typing, which is how a paste into a TUI submits itself halfway through.
    #[test]
    fn a_paste_is_bracketed_only_when_the_child_asked() {
        assert_eq!(paste("hello", &modes(false, false)), b"hello");
        assert_eq!(
            String::from_utf8_lossy(&paste("hello", &modes(false, true))),
            "\x1b[200~hello\x1b[201~"
        );
    }

    /// The refusal, and it is the whole point of the function.
    ///
    /// A program that never asked for mouse reports reads these bytes as **typed characters**:
    /// a wheel aimed at a shell puts `[<64;40;12M` on its command line. Asserted as emptiness
    /// rather than as an error type, because emptiness is what every caller here already treats
    /// as *nothing to send*.
    #[test]
    fn a_wheel_is_refused_where_nobody_asked_for_the_mouse() {
        use cide_ipc::screen::MouseReporting;
        assert!(wheel(-3, &tracking(MouseReporting::Off)).is_empty());
        assert!(!wheel(-3, &tracking(MouseReporting::Sgr)).is_empty());
        // And a scroll of nothing is nothing, whatever the mode.
        assert!(wheel(0, &tracking(MouseReporting::Sgr)).is_empty());
    }

    /// Button 64 is up and 65 is down, one report per line, at the centre of the grid.
    #[test]
    fn a_wheel_is_one_report_per_line_aimed_at_the_middle() {
        use cide_ipc::screen::MouseReporting;
        let up = wheel(-2, &tracking(MouseReporting::Sgr));
        assert_eq!(
            String::from_utf8_lossy(&up),
            "\x1b[<64;41;13M\x1b[<64;41;13M"
        );
        let down = wheel(1, &tracking(MouseReporting::Sgr));
        assert_eq!(String::from_utf8_lossy(&down), "\x1b[<65;41;13M");
    }

    /// The original encoding, for a program that enabled tracking without asking for SGR.
    #[test]
    fn a_legacy_wheel_is_three_offset_bytes() {
        use cide_ipc::screen::MouseReporting;
        let up = wheel(-1, &tracking(MouseReporting::Legacy));
        assert_eq!(up, vec![0x1b, b'[', b'M', 32 + 64, 32 + 41, 32 + 13]);
    }

    /// A device asking for four hundred lines is a bug on that side; the child would have to
    /// redraw for every one of them.
    #[test]
    fn a_wheel_is_capped() {
        use cide_ipc::screen::MouseReporting;
        let huge = wheel(-400, &tracking(MouseReporting::Sgr));
        assert_eq!(
            huge.len(),
            usize::from(MAX_WHEEL_LINES) * "\x1b[<64;41;13M".len()
        );
    }
}
