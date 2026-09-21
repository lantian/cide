//! The mirror, read as structured rows instead of as escape sequences. (M72)
//!
//! Every other way of asking this session what is on its screen — `screen_state`,
//! `reattach_state`, `full_state`, `history_state` — answers in **bytes**, because until M72 the
//! only consumer was xterm.js and bytes are exactly what it wants. A companion application on a
//! phone has no terminal emulator to parse them with, and putting one in a WebView inside that
//! app would be a second rendering stack with its own gesture handling and its own fonts.
//!
//! So: the same grid, said the other way round. `vt100::Cell` already knows a cell's foreground,
//! background, bold, dim, italic, underline, inverse and width, and this walks the grid merging
//! adjacent cells that agree into [`StyleRun`]s.
//!
//! # The three rules that are silent if got wrong
//!
//! 1. **A wide cell emits its text once and its continuation emits nothing.** `vt100` models a
//!    double-width glyph as two cells, the second being `is_wide_continuation`. Emitting that
//!    second cell as a space puts every CJK line one column out from its first wide glyph
//!    onward, and nothing on the screen says so — the text is all there, just sliding.
//! 2. **A run whose background is not the default keeps its trailing spaces.** The trailing blank
//!    run of a line is dropped, because a screen is mostly empty and eighty spaces a row is the
//!    whole payload — but a *selected* or *highlighted* region is spaces with a background, and
//!    trimming those paints a ragged right edge on the one thing the user is looking at.
//! 3. **The viewport is restored under the same lock that moved it.** Paging scrollback moves the
//!    offset of the parser that `reattach_state` and `screen_state` read, so a device paging
//!    history while a pane re-docks would repaint the desktop's terminal from the scrolled
//!    position. [`ViewportGuard`] is what makes that true on a panic as well as on the happy
//!    path.

use cide_ipc::screen::{
    Cursor, ScreenCapture, ScreenColor, ScreenInfo, ScreenLine, ScrollbackCapture, StyleRun, flags,
};

use crate::PtySession;

/// The most scrollback lines one call will return.
///
/// The walk is O(requested) under the mirror's lock, on a parser the coalescer wants back. A
/// reader paging history asks again; a reader asking for five thousand lines at once would stall
/// every sink of that session for the length of the read.
pub const MAX_PAGE_ROWS: u16 = 200;

impl PtySession {
    /// The visible grid, as rows of styled runs.
    ///
    /// One critical section: the lock is taken, the grid is walked, the lock is released, and the
    /// result is serialised by somebody else. Holding it across a serialisation would put a JSON
    /// encoder in front of the thread that feeds every sink of this session.
    pub fn capture_screen(&self) -> ScreenCapture {
        capture(self.vt.lock().screen())
    }

    /// How many lines of scrollback this session is holding.
    pub fn scrollback_depth(&self) -> u32 {
        depth_of(&mut self.vt.lock())
    }

    /// A window onto the retained scrollback, counted in lines from the **oldest**.
    pub fn capture_scrollback(&self, from_top: u32, rows: u16) -> ScrollbackCapture {
        capture_scrollback_of(&mut self.vt.lock(), from_top, rows)
    }
}

/// The visible grid.
///
/// A free function over the screen rather than a method, so it can be driven by a plain
/// `vt100::Parser` with no child, no thread and no pty — which is what lets this crate's own
/// tests exist and what lets `cide-claude` build a permission-prompt fixture out of the text a
/// prompt actually draws. Every rule this module states is in here or in [`capture_row`].
pub fn capture(screen: &vt100::Screen) -> ScreenCapture {
    let (rows, cols) = screen.size();
    let lines = (0..rows)
        .map(|row| capture_row(screen, row, cols))
        .collect();
    let (crow, ccol) = screen.cursor_position();
    ScreenCapture {
        info: ScreenInfo {
            cols,
            rows,
            alt: screen.alternate_screen(),
            app_cursor: screen.application_cursor(),
            bracketed_paste: screen.bracketed_paste(),
            mouse: match (
                screen.mouse_protocol_mode(),
                screen.mouse_protocol_encoding(),
            ) {
                (vt100::MouseProtocolMode::None, _) => cide_ipc::screen::MouseReporting::Off,
                (_, vt100::MouseProtocolEncoding::Sgr) => cide_ipc::screen::MouseReporting::Sgr,
                _ => cide_ipc::screen::MouseReporting::Legacy,
            },
        },
        cursor: (!screen.hide_cursor()).then_some(Cursor {
            row: crow,
            col: ccol,
        }),
        lines,
    }
}

/// How many lines are retained.
///
/// Zero while the alternate screen is engaged, and that is `vt100`'s own answer rather than a
/// special case here: scrollback belongs to the normal buffer, and a fullscreen TUI's grid has
/// none. A caller that wants to *say* so to a user has more context than this crate does.
pub(crate) fn depth_of(vt: &mut vt100::Parser) -> u32 {
    let guard = ViewportGuard { vt };
    guard.vt.screen_mut().set_scrollback(usize::MAX);
    guard.vt.screen().scrollback() as u32
}

/// A window onto the retained scrollback, counted in lines from the **oldest**.
///
/// Counted from the top because the scrollback grows from the bottom: an offset from the end
/// names a different line every time the child prints anything, and a reader paging backwards
/// through one would see rows repeat and rows vanish with nothing wrong anywhere.
pub(crate) fn capture_scrollback_of(
    vt: &mut vt100::Parser,
    from_top: u32,
    rows: u16,
) -> ScrollbackCapture {
    let guard = ViewportGuard { vt };
    let cols = guard.vt.screen().size().1;

    guard.vt.screen_mut().set_scrollback(usize::MAX);
    let depth = guard.vt.screen().scrollback() as u32;

    let wanted = u32::from(rows.min(MAX_PAGE_ROWS));
    let first = from_top.min(depth);
    let last = (first + wanted).min(depth);

    let mut lines = Vec::with_capacity((last - first) as usize);
    for index in first..last {
        // `set_scrollback(n)` counts back from the live screen, so the oldest retained line is at
        // `depth` and the newest at 1 — the same arithmetic `scrollback_rows` does with a
        // reversed range. Row 0 of the scrolled viewport is the line wanted.
        let offset = (depth - index) as usize;
        guard.vt.screen_mut().set_scrollback(offset);
        let mut line = capture_row(guard.vt.screen(), 0, cols);
        line.row = (index - first) as u16;
        lines.push(line);
    }

    ScrollbackCapture {
        from_top: first,
        depth,
        lines,
    }
}

/// Puts the mirror's viewport back, however the walk ends.
///
/// The offset is a property of the *shared* parser, not of this read: `reattach_state` and
/// `screen_state` render from wherever it is left. A `set_scrollback(0)` at the end of the happy
/// path is not enough, because `parking_lot` does not poison — a panic mid-walk would simply
/// unlock a mirror stuck in the past, and the next pane to re-dock would paint history.
struct ViewportGuard<'a> {
    vt: &'a mut vt100::Parser,
}

impl Drop for ViewportGuard<'_> {
    fn drop(&mut self) {
        self.vt.screen_mut().set_scrollback(0);
    }
}

fn capture_row(screen: &vt100::Screen, row: u16, cols: u16) -> ScreenLine {
    let mut runs: Vec<StyleRun> = Vec::new();

    for col in 0..cols {
        let Some(cell) = screen.cell(row, col) else {
            break;
        };
        if cell.is_wide_continuation() {
            // Rule 1. The glyph was emitted with the cell that owns it.
            continue;
        }
        let style = Style::of(cell);
        let contents = cell.contents();
        // An unwritten cell has no contents at all, and a grid needs a column for it.
        let text = if contents.is_empty() { " " } else { contents };

        match runs.last_mut() {
            Some(last) if style.matches(last) => last.text.push_str(text),
            _ => runs.push(StyleRun {
                text: text.to_owned(),
                fg: style.fg,
                bg: style.bg,
                flags: style.flags,
            }),
        }
    }

    trim_trailing_blank(&mut runs);
    ScreenLine {
        row,
        wrapped: screen.row_wrapped(row),
        runs,
    }
}

/// Rule 2. Shorten the line to its last **visible** cell.
///
/// A screen is mostly empty, and an unwritten cell is a space in the default style, so without
/// this every row carries its width in spaces and the payload is mostly nothing. Not a blanket
/// `trim_end`: a space with a background, an underline or the inverse attribute is something the
/// user can see, and trimming those paints a ragged right edge on a selection or a highlighted
/// region — which is precisely the thing they are looking at when they notice.
///
/// A loop rather than one step, because the trailing blank may be spread over several runs: a
/// foreground colour is invisible on a space, so `a\x1b[31m   ` is two runs of which the second
/// is as empty as the first's tail.
fn trim_trailing_blank(runs: &mut Vec<StyleRun>) {
    while let Some(last) = runs.last_mut() {
        if last.bg.is_some() || last.flags & (flags::INVERSE | flags::UNDERLINE) != 0 {
            return;
        }
        let keep = last.text.trim_end_matches(' ').len();
        if keep == last.text.len() {
            return;
        }
        if keep == 0 {
            runs.pop();
        } else {
            last.text.truncate(keep);
            return;
        }
    }
}

/// Everything about a cell that decides which run it belongs to.
struct Style {
    fg: Option<ScreenColor>,
    bg: Option<ScreenColor>,
    flags: u8,
}

impl Style {
    fn of(cell: &vt100::Cell) -> Self {
        let mut flags = 0;
        for (set, bit) in [
            (cell.bold(), flags::BOLD),
            (cell.dim(), flags::DIM),
            (cell.italic(), flags::ITALIC),
            (cell.underline(), flags::UNDERLINE),
            (cell.inverse(), flags::INVERSE),
            // Width is part of the run key on purpose: a consumer laying out a monospace grid
            // advances two columns per glyph of a wide run, and a run that mixed widths would
            // give it no way to know how far to advance without measuring the text itself.
            (cell.is_wide(), flags::WIDE),
        ] {
            if set {
                flags |= bit;
            }
        }
        Self {
            fg: colour_of(cell.fgcolor()),
            bg: colour_of(cell.bgcolor()),
            flags,
        }
    }

    fn matches(&self, run: &StyleRun) -> bool {
        self.fg == run.fg && self.bg == run.bg && self.flags == run.flags
    }
}

fn colour_of(colour: vt100::Color) -> Option<ScreenColor> {
    match colour {
        vt100::Color::Default => None,
        vt100::Color::Idx(index) => Some(ScreenColor::Idx { index }),
        vt100::Color::Rgb(r, g, b) => Some(ScreenColor::Rgb { r, g, b }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parser(text: &[u8]) -> vt100::Parser {
        let mut vt = vt100::Parser::new(6, 20, 100);
        vt.process(text);
        vt
    }

    fn plain(line: &ScreenLine) -> String {
        line.runs.iter().map(|r| r.text.as_str()).collect()
    }

    #[test]
    fn a_run_ends_where_the_appearance_changes() {
        let vt = parser(b"ab\x1b[31mcd\x1b[1mef\x1b[m");
        let line = &capture(vt.screen()).lines[0];
        let texts: Vec<&str> = line.runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts, ["ab", "cd", "ef"]);
        assert_eq!(line.runs[0].fg, None);
        assert_eq!(line.runs[1].fg, Some(ScreenColor::Idx { index: 1 }));
        assert_eq!(line.runs[1].flags, 0);
        assert_eq!(line.runs[2].flags, flags::BOLD);
    }

    #[test]
    fn a_direct_colour_stays_direct() {
        let vt = parser(b"\x1b[38;2;10;20;30mx\x1b[m");
        let line = &capture(vt.screen()).lines[0];
        assert_eq!(
            line.runs[0].fg,
            Some(ScreenColor::Rgb {
                r: 10,
                g: 20,
                b: 30
            })
        );
    }

    /// Rule 1. The failure this prevents is a CJK line sliding one column right per glyph, with
    /// every character present and nothing logged.
    #[test]
    fn a_wide_glyph_is_emitted_once_and_its_continuation_never() {
        let vt = parser("a漢字b".as_bytes());
        let line = &capture(vt.screen()).lines[0];
        assert_eq!(plain(line), "a漢字b");

        let wide: Vec<&StyleRun> = line
            .runs
            .iter()
            .filter(|r| r.flags & flags::WIDE != 0)
            .collect();
        assert_eq!(wide.len(), 1, "the wide glyphs did not form one run");
        assert_eq!(wide[0].text, "漢字");
        // Width is part of the run key precisely so a consumer can advance two columns per glyph
        // without measuring the text itself, which means no run may mix the two.
        assert!(
            line.runs
                .iter()
                .all(|r| r.text.chars().all(char::is_alphabetic)),
            "a run mixed widths: {:?}",
            line.runs
        );
    }

    /// Rule 2, both halves: the empty tail of a line costs nothing, and a highlighted one is not
    /// the empty tail of a line.
    #[test]
    fn a_blank_tail_is_dropped_and_a_highlighted_one_is_kept() {
        let vt = parser(b"hi");
        let line = &capture(vt.screen()).lines[0];
        assert_eq!(plain(line), "hi", "eighteen spaces were shipped");

        let vt = parser(b"hi\x1b[44m    \x1b[m");
        let line = &capture(vt.screen()).lines[0];
        assert_eq!(plain(line), "hi    ");
        assert_eq!(
            line.runs.last().expect("a run").bg,
            Some(ScreenColor::Idx { index: 4 }),
            "the highlighted spaces were trimmed away"
        );

        // Underline and inverse are visible on a space too.
        let vt = parser(b"hi\x1b[4m   \x1b[m");
        assert_eq!(plain(&capture(vt.screen()).lines[0]), "hi   ");
    }

    #[test]
    fn an_empty_row_has_no_runs_at_all() {
        let vt = parser(b"hi");
        let capture = capture(vt.screen());
        assert!(capture.lines[1].runs.is_empty());
        assert_eq!(capture.lines.len(), 6);
    }

    #[test]
    fn a_soft_wrapped_row_says_so() {
        // Twenty-two characters into a twenty-column grid.
        let vt = parser(b"aaaaaaaaaaaaaaaaaaaabb");
        let capture = capture(vt.screen());
        assert!(capture.lines[0].wrapped, "the wrap flag was lost");
        assert!(!capture.lines[1].wrapped);
        assert_eq!(plain(&capture.lines[1]), "bb");
    }

    #[test]
    fn the_cursor_is_absent_when_hidden_rather_than_at_the_origin() {
        let vt = parser(b"hi");
        let shown = capture(vt.screen());
        assert_eq!(shown.cursor, Some(Cursor { row: 0, col: 2 }));

        let vt = parser(b"hi\x1b[?25l");
        assert_eq!(capture(vt.screen()).cursor, None);
    }

    /// The two modes a caller needs in order to encode a keystroke correctly, and which nothing
    /// outside the mirror knows.
    #[test]
    fn the_input_modes_come_across() {
        let vt = parser(b"");
        let plain = capture(vt.screen());
        assert!(!plain.info.app_cursor);
        assert!(!plain.info.bracketed_paste);
        assert!(!plain.info.alt);

        let vt = parser(b"\x1b[?1h\x1b[?2004h\x1b[?1049h");
        let fancy = capture(vt.screen());
        assert!(fancy.info.app_cursor, "DECCKM was not reported");
        assert!(fancy.info.bracketed_paste);
        assert!(fancy.info.alt);
    }

    fn with_history() -> vt100::Parser {
        let mut vt = vt100::Parser::new(3, 20, 100);
        for n in 0..10 {
            vt.process(format!("line {n}\r\n").as_bytes());
        }
        vt
    }

    #[test]
    fn scrollback_is_read_from_the_oldest_line_forwards() {
        let mut vt = with_history();
        let depth = depth_of(&mut vt);
        assert!(depth >= 7, "only {depth} lines were retained");

        let page = capture_scrollback_of(&mut vt, 0, 3);
        assert_eq!(page.from_top, 0);
        assert_eq!(page.depth, depth);
        assert_eq!(page.lines.len(), 3);
        assert_eq!(plain(&page.lines[0]), "line 0");
        assert_eq!(plain(&page.lines[1]), "line 1");
        assert_eq!(page.lines[0].row, 0);
        assert_eq!(page.lines[2].row, 2);

        let next = capture_scrollback_of(&mut vt, 3, 3);
        assert_eq!(plain(&next.lines[0]), "line 3");
        assert_eq!(next.lines[0].row, 0, "a page is numbered from its own top");
    }

    #[test]
    fn a_page_is_capped_and_a_page_past_the_end_is_empty() {
        let mut vt = with_history();
        let page = capture_scrollback_of(&mut vt, 0, u16::MAX);
        assert!(page.lines.len() <= MAX_PAGE_ROWS as usize);
        assert_eq!(page.lines.len(), page.depth as usize);

        let past = capture_scrollback_of(&mut vt, 10_000, 10);
        assert!(past.lines.is_empty());
        assert_eq!(past.from_top, past.depth);
    }

    /// Rule 3, and the one that damages something other than this read.
    ///
    /// The offset belongs to the parser `reattach_state` and `screen_state` render from. Left
    /// moved, a device paging history while a pane re-docks repaints the desktop's terminal from
    /// the scrolled position — and the assertion that catches it is not `scrollback() == 0` but
    /// the bytes, because that is what the other readers actually produce.
    #[test]
    fn paging_puts_the_viewport_back_byte_for_byte() {
        let mut vt = with_history();
        let before = vt.screen().contents_formatted();

        let _ = capture_scrollback_of(&mut vt, 2, 4);

        assert_eq!(vt.screen().scrollback(), 0);
        assert_eq!(
            vt.screen().contents_formatted(),
            before,
            "the mirror was left looking at the past"
        );
    }

    /// A panic mid-walk must not leave the mirror scrolled. `parking_lot` does not poison, so
    /// without the guard the next reader of this session paints history and nothing says why.
    #[test]
    fn a_panic_mid_walk_still_puts_the_viewport_back() {
        let mut vt = with_history();
        let before = vt.screen().contents_formatted();

        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let guard = ViewportGuard { vt: &mut vt };
            guard.vt.screen_mut().set_scrollback(4);
            panic!("mid-walk");
        }));
        assert!(caught.is_err());

        assert_eq!(vt.screen().scrollback(), 0);
        assert_eq!(vt.screen().contents_formatted(), before);
    }

    /// An alternate screen has no scrollback of its own, so there is nothing to refuse here —
    /// the answer is honestly zero, and saying so to a person is a caller's job.
    #[test]
    fn an_alternate_screen_reports_no_history() {
        let mut vt = with_history();
        vt.process(b"\x1b[?1049h");
        assert_eq!(depth_of(&mut vt), 0);
        assert!(capture_scrollback_of(&mut vt, 0, 10).lines.is_empty());
    }
}
