//! What a permission prompt is offering, read off the screen. (M72)
//!
//! cide has modelled a permission prompt as **one boolean** since M5: [`is_permission_request`]
//! matches three substrings in a `Notification` hook's message, the session moves to
//! `AwaitingPermission`, and that is the whole of what anybody knows. A person at the keyboard
//! needs no more, because the options are on the screen in front of them. A person on a phone
//! needs the options as *data*, or the feature is a picture of a question with a keyboard under
//! it.
//!
//! So this reads the rendered grid. It lives in `cide-claude` because it is knowledge about one
//! specific program: `cide-remote` must not know what Claude is, and `cide-pty` must not know
//! what is on the screen.
//!
//! # The negative corpus is the specification
//!
//! `cide_core::jsonlog`'s rule, and it matters more here. A log line this repository's detector
//! wrongly claims is **destroyed** — bad, and recoverable by scrolling. A prompt this one wrongly
//! claims produces a button that, when tapped, sends a digit into a conversation that approves a
//! tool call. There is no scrolling back from that.
//!
//! Every rule below is therefore a reason to **refuse**, and [`parse`] returns `None` rather than
//! a partial answer. When it refuses, the device shows the raw screen and the keyboard, which is
//! the honest degradation: the user can still answer, by typing, exactly as they would at the
//! desk. A prompt this cannot read is a fixture for the corpus, not a blocked feature.
//!
//! # What it refuses, and why each one
//!
//! * **Anything whose session is not `AwaitingPermission`.** The hook is exact and has already
//!   decided; the screen is never sufficient evidence on its own. A source file being *edited* in
//!   a pane can contain `1. Yes`.
//! * **Numbers that are not contiguous `1..=n`, each exactly once.** A gap means something was
//!   misread, and a repeat means two blocks were run together.
//! * **Fewer than two options, or more than nine.** One option is not a choice, and a two-digit
//!   answer is a different keystroke shape than this can deliver.
//! * **Rows that are not consecutive.** At most one blank line inside the block; a second blank
//!   ends it. Two prompts separated by prose would otherwise read as one.
//! * **An empty label, or one over [`MAX_LABEL`].** Both mean the line was not what it looked
//!   like.
//! * **A block that is not in the bottom half of the screen.** A heuristic, stated as one: the
//!   prompt is the last thing Claude draws before its composer. It biases towards refusing.
//! * **More than one candidate block.** Ambiguity is a refusal and never a choice.
//!
//! # The answer is a digit, and deliberately not a digit and a Return
//!
//! [`answer_bytes`] writes exactly what a person types to pick an option. `agents.rs`'s
//! `AWAITING_PERMISSION` constant records what the extra byte would cost: a lone `\r` at a
//! selection list **is an answer**, which is why a graceful stop is refused there and the run is
//! killed instead. Sending a digit *and* a Return risks the Return being read as a confirmation
//! of whatever is selected once the digit has already acted.
//!
//! Whether the CLI needs the Return at all is the one fact here that has not been measured
//! against a real prompt; `tests/real_permission.rs` is where that gets settled, and until it
//! does this sends the digit alone.
//!
//! The shapes themselves — [`PermissionPrompt`] and [`PromptOption`] — live in `cide_ipc::remote`
//! because they are wire types and because a domain struct, a wire struct and a `From` between
//! them would be three places for a field to go missing.

use cide_ipc::SessionState;
use cide_ipc::remote::{PermissionPrompt, PromptOption};
use cide_ipc::screen::ScreenCapture;

/// The longest a plausible option label can be.
///
/// The longest one Claude ships is *"No, and tell Claude what to do differently"*. Two hundred
/// leaves room for a wrapped or a localised one and still refuses a paragraph that happens to
/// start with a digit and a dot.
pub const MAX_LABEL: usize = 200;

/// Fewest and most options a prompt may offer. See the module header.
pub const MIN_OPTIONS: usize = 2;
pub const MAX_OPTIONS: usize = 9;

/// Read the prompt, or refuse.
pub fn parse(screen: &ScreenCapture, state: SessionState) -> Option<PermissionPrompt> {
    // The hook has already decided this, exactly, from a payload the screen cannot show. Nothing
    // below is evidence without it.
    if state != SessionState::AwaitingPermission {
        return None;
    }

    let lines = plain_lines(screen);
    let blocks = candidate_blocks(&lines);
    // Ambiguity is a refusal. Two numbered blocks on one screen means at least one of them is
    // not a prompt, and there is no way to tell which.
    let [block] = blocks.as_slice() else {
        return None;
    };

    // The prompt is the last thing drawn before the composer. Lenient — half, not the third the
    // box usually sits in — and biased towards refusing, which is the only direction this is
    // allowed to be wrong in.
    if u32::from(block.last_row) * 2 < u32::from(screen.info.rows) {
        return None;
    }

    let question = question_above(&lines, block.first_row);
    let digest = digest_of(&question, &block.options);
    Some(PermissionPrompt {
        question,
        options: block.options.clone(),
        selected: block.selected,
        digest,
    })
}

/// The bytes a person types to pick option `number`.
///
/// One digit. See the module header for the Return that is not here.
pub fn answer_bytes(number: u8) -> Option<Vec<u8>> {
    (1..=MAX_OPTIONS as u8)
        .contains(&number)
        .then(|| vec![b'0' + number])
}

struct Block {
    first_row: u16,
    last_row: u16,
    options: Vec<PromptOption>,
    selected: Option<u8>,
}

/// Every row as plain text, with the box drawing taken off both ends.
///
/// Claude's prompt is inside a rounded box, so an option line arrives as `│ ❯ 1. Yes      │`. The
/// borders are stripped rather than matched around, because the box is decoration that changes
/// between releases and the numbers are not.
fn plain_lines(screen: &ScreenCapture) -> Vec<(u16, String)> {
    screen
        .lines
        .iter()
        .map(|line| {
            let text: String = line.runs.iter().map(|run| run.text.as_str()).collect();
            (line.row, unbox(&text))
        })
        .collect()
}

fn unbox(text: &str) -> String {
    let trimmed = text.trim();
    let trimmed = trimmed.strip_prefix(['│', '|', '┃']).unwrap_or(trimmed);
    let trimmed = trimmed.strip_suffix(['│', '|', '┃']).unwrap_or(trimmed);
    trimmed.trim().to_owned()
}

/// Every run of numbered lines that could be a prompt's options.
fn candidate_blocks(lines: &[(u16, String)]) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut current: Vec<(u16, PromptOption, bool)> = Vec::new();
    // One blank line inside a block is tolerated, because some prompts space their options; a
    // second ends it. Any non-blank line that is not an option ends it at once.
    let mut blanks = 0;

    for (row, text) in lines {
        // A list that runs past nine **poisons** the block it was continuing, rather than ending
        // it. `10.` is not an option this can deliver — one keystroke addresses one digit — but
        // the nine above it would otherwise finish as a perfectly well-formed block, and a
        // caller would be shown nine of ten options with nothing to say so. That is precisely
        // the partial parse this module promises never to return.
        if over_nine(text) {
            current.clear();
            blanks = 0;
            continue;
        }
        if let Some((option, marked)) = numbered(text) {
            blanks = 0;
            current.push((*row, option, marked));
            continue;
        }
        if text.is_empty() && !current.is_empty() {
            blanks += 1;
            if blanks <= 1 {
                continue;
            }
        }
        if let Some(block) = finish(std::mem::take(&mut current)) {
            blocks.push(block);
        }
        blanks = 0;
    }
    if let Some(block) = finish(current) {
        blocks.push(block);
    }
    blocks
}

/// `1. Yes`, `❯ 2. No`, `  3) Something` — and nothing else.
fn numbered(text: &str) -> Option<(PromptOption, bool)> {
    let mut rest = text.trim_start();
    // The highlight marker, when the TUI draws one. Both spellings, because a terminal without
    // the glyph falls back to `>`.
    let marked = rest.starts_with('❯') || rest.starts_with('>');
    if marked {
        rest = rest[rest.chars().next()?.len_utf8()..].trim_start();
    }

    let digit = rest.chars().next()?;
    if !digit.is_ascii_digit() {
        return None;
    }
    let number = digit as u8 - b'0';
    // Zero is not an option number in any prompt Claude draws, and accepting it would make
    // `1..=n` contiguity meaningless.
    if number == 0 {
        return None;
    }
    let rest = &rest[1..];
    // A separator is required. Without it `2024 was a year` is an option numbered 2.
    let rest = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')'))?;
    // And whitespace after it, or `1.5 seconds` is an option too.
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }

    let label = rest.trim().to_owned();
    if label.is_empty() || label.len() > MAX_LABEL {
        return None;
    }
    Some((PromptOption { number, label }, marked))
}

/// `10.`, `11)` — a numbered line this cannot address with one keystroke.
fn over_nine(text: &str) -> bool {
    let mut rest = text.trim_start();
    if rest.starts_with('❯') || rest.starts_with('>') {
        let Some(first) = rest.chars().next() else {
            return false;
        };
        rest = rest[first.len_utf8()..].trim_start();
    }
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    // Exactly two, and the bound matters in both directions. Fewer is an option this *can*
    // deliver. More is a year or an amount — `2024. A year in review` is prose, and treating it
    // as a list index would let an ordinary sentence discard a prompt sitting below it. A list
    // long enough to reach three digits was already poisoned at `10.`.
    if digits.len() != 2 {
        return false;
    }
    let after = &rest[digits.len()..];
    let Some(after) = after.strip_prefix('.').or_else(|| after.strip_prefix(')')) else {
        return false;
    };
    after.starts_with(char::is_whitespace)
}

fn finish(rows: Vec<(u16, PromptOption, bool)>) -> Option<Block> {
    if rows.len() < MIN_OPTIONS || rows.len() > MAX_OPTIONS {
        return None;
    }
    // Contiguous from one, each exactly once. A gap means a line was misread; a repeat means two
    // blocks were run together.
    for (index, (_, option, _)) in rows.iter().enumerate() {
        if usize::from(option.number) != index + 1 {
            return None;
        }
    }
    let first_row = rows.first()?.0;
    let last_row = rows.last()?.0;
    let selected = rows
        .iter()
        .find(|(_, _, marked)| *marked)
        .map(|(_, option, _)| option.number);
    Some(Block {
        first_row,
        last_row,
        options: rows.into_iter().map(|(_, option, _)| option).collect(),
        selected,
    })
}

/// The prose immediately above the options.
///
/// Stops at the first blank line going up, and at the box's own top or bottom rule, so the
/// question is the sentence that belongs to these options rather than everything else on screen.
fn question_above(lines: &[(u16, String)], first_row: u16) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (row, text) in lines.iter().rev() {
        if *row >= first_row {
            continue;
        }
        if text.is_empty() || is_rule(text) {
            break;
        }
        out.push(text.clone());
    }
    out.reverse();
    out
}

/// A box's horizontal rule, in any of the shapes a rounded or square border draws.
fn is_rule(text: &str) -> bool {
    !text.is_empty()
        && text.chars().all(|c| {
            matches!(
                c,
                '─' | '━' | '╭' | '╮' | '╰' | '╯' | '┌' | '┐' | '└' | '┘' | '-' | '+'
            )
        })
}

/// A hash of everything the person is being asked.
///
/// `blake3` and not the screen's own line digests: this one guards an answer against a prompt
/// that moved, so it is a safety check rather than a repaint optimisation, and it wants a real
/// hash. It covers the question *and* the labels, because two prompts can offer the same three
/// options about different commands — which is exactly the pair that must not be confused.
fn digest_of(question: &[String], options: &[PromptOption]) -> String {
    let mut hasher = blake3::Hasher::new();
    for line in question {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    hasher.update(b"--\n");
    for option in options {
        hasher.update(&[option.number]);
        hasher.update(option.label.as_bytes());
        hasher.update(b"\n");
    }
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A screen, from the text that draws it.
    ///
    /// The corpus is written this way rather than as serialised rows because a fixture that reads
    /// as the screen it is can be checked by eye against a real prompt, and one that reads as a
    /// page of JSON cannot. The parser sees exactly what `cide-pty` would hand it.
    fn screen(rows: u16, text: &str) -> ScreenCapture {
        render(rows, text.trim_matches('\n'))
    }

    /// Exactly these lines, from row zero. No trimming — a leading blank line is a blank row,
    /// which is the whole mechanism [`at_bottom`] uses.
    fn render(rows: u16, text: &str) -> ScreenCapture {
        let mut vt = vt100::Parser::new(rows, 72, 0);
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                vt.process(b"\r\n");
            }
            vt.process(line.as_bytes());
        }
        cide_pty::screen::capture(vt.screen())
    }

    /// The prompt where Claude draws it: at the bottom, with its composer's two rows under it.
    ///
    /// Not `screen` with padding in front — that one trims leading newlines so a raw string can
    /// start on its own line, which ate the padding on the first attempt and made every positive
    /// case fail the position rule at once.
    fn at_bottom(rows: u16, text: &str) -> ScreenCapture {
        let body = text.trim_matches('\n');
        let used = body.lines().count() as u16;
        let padding = "\n".repeat(usize::from(rows.saturating_sub(used + 2)));
        render(rows, &format!("{padding}{body}"))
    }

    fn read(capture: &ScreenCapture) -> Option<PermissionPrompt> {
        parse(capture, SessionState::AwaitingPermission)
    }

    // --- positive corpus ------------------------------------------------------------------

    const BASH: &str = r"
╭──────────────────────────────────────────────────────────────────╮
│ Bash command                                                     │
│                                                                  │
│   rm -rf build                                                   │
│   Remove the build directory                                     │
│                                                                  │
│ Do you want to proceed?                                          │
│ ❯ 1. Yes                                                         │
│   2. Yes, and don't ask again for rm commands in this project    │
│   3. No, and tell Claude what to do differently (esc)            │
╰──────────────────────────────────────────────────────────────────╯
";

    #[test]
    fn a_bash_prompt_reads_as_its_three_options() {
        let prompt = read(&at_bottom(24, BASH)).expect("a prompt");
        assert_eq!(prompt.options.len(), 3);
        assert_eq!(prompt.options[0].number, 1);
        assert_eq!(prompt.options[0].label, "Yes");
        assert_eq!(
            prompt.options[1].label,
            "Yes, and don't ask again for rm commands in this project"
        );
        assert_eq!(prompt.selected, Some(1));
        assert_eq!(prompt.question, vec!["Do you want to proceed?".to_owned()]);
    }

    #[test]
    fn a_two_option_prompt_is_a_prompt() {
        let capture = at_bottom(
            24,
            r"
│ Write to src/main.rs?                                            │
│ ❯ 1. Yes                                                         │
│   2. No, and tell Claude what to do differently (esc)            │
",
        );
        let prompt = read(&capture).expect("a prompt");
        assert_eq!(prompt.options.len(), 2);
        assert_eq!(prompt.question, vec!["Write to src/main.rs?".to_owned()]);
    }

    /// Nothing requires the TUI to mark a row, and a terminal without the glyph falls back to
    /// `>`. Both are read, and neither is required.
    #[test]
    fn the_highlight_is_optional_and_has_two_spellings() {
        let unmarked = at_bottom(24, "│ Proceed?    │\n│   1. Yes    │\n│   2. No     │");
        assert_eq!(read(&unmarked).expect("a prompt").selected, None);

        let ascii = at_bottom(24, "│ Proceed?    │\n│   1. Yes    │\n│ > 2. No     │");
        assert_eq!(read(&ascii).expect("a prompt").selected, Some(2));
    }

    #[test]
    fn a_question_that_wraps_is_kept_whole_and_in_order() {
        let capture = at_bottom(
            24,
            r"
│ Claude wants to run a command that will delete files it did not  │
│ create, in a directory outside the project root. This cannot be  │
│ undone. Do you want to proceed?                                  │
│ ❯ 1. Yes                                                         │
│   2. No                                                          │
",
        );
        let prompt = read(&capture).expect("a prompt");
        assert_eq!(prompt.question.len(), 3);
        assert!(prompt.question[0].starts_with("Claude wants to run"));
        assert!(prompt.question[2].ends_with("Do you want to proceed?"));
    }

    /// Nine is the most a single keystroke can deliver, and the parser takes it.
    #[test]
    fn nine_options_are_read_and_ten_are_not() {
        let nine: String = (1..=9).map(|n| format!("│ {n}. option {n} │\n")).collect();
        assert_eq!(
            read(&at_bottom(30, &format!("│ Pick one │\n{nine}")))
                .expect("a prompt")
                .options
                .len(),
            9
        );

        // And a list of ten is refused **whole**. Reading its first nine would be nine of ten
        // options with nothing on screen to say the tenth exists.
        let ten: String = (1..=10).map(|n| format!("│ {n}. option {n} │\n")).collect();
        assert!(read(&at_bottom(30, &format!("│ Pick one │\n{ten}"))).is_none());
        // `over_nine` sees the line after the box is stripped, which is what `plain_lines`
        // hands it.
        assert!(
            over_nine("10. option 10"),
            "the terminator is what refuses it"
        );
        assert!(over_nine("❯ 12) another"));
        assert!(!over_nine("1. option 1"));
        assert!(!over_nine("2024. A year"), "a year is not an option number");
    }

    // --- the negative corpus, which is the specification ----------------------------------

    /// The gate that does the most work, and the only one that is not a heuristic.
    ///
    /// A shell's own `select` menu is *structurally identical* to a permission prompt, and so is
    /// an ordered list in a file somebody is editing. Nothing on the screen tells them apart —
    /// what does is that a shell pane and an editor never reach `AwaitingPermission`, because
    /// that state comes from a hook payload the screen cannot show.
    #[test]
    fn nothing_is_a_prompt_unless_the_hook_said_so() {
        let capture = at_bottom(24, "│ Proceed? │\n│ 1. Yes   │\n│ 2. No    │");
        assert!(read(&capture).is_some(), "the fixture is a prompt");

        for state in [
            SessionState::Busy,
            SessionState::Idle,
            SessionState::AwaitingInput,
            SessionState::Spawning,
            SessionState::Paused,
            SessionState::Exited { code: 0 },
        ] {
            assert!(parse(&capture, state).is_none(), "{state:?}");
        }
    }

    #[test]
    fn a_gap_in_the_numbering_is_a_refusal() {
        let capture = at_bottom(24, "│ Proceed? │\n│ 1. a │\n│ 2. b │\n│ 4. d │");
        assert!(read(&capture).is_none());
    }

    #[test]
    fn a_repeated_number_is_a_refusal() {
        let capture = at_bottom(24, "│ Proceed? │\n│ 1. a │\n│ 2. b │\n│ 2. c │");
        assert!(read(&capture).is_none());
    }

    /// Ambiguity is a refusal, never a choice. There is no way to tell which of two blocks the
    /// hook was talking about.
    #[test]
    fn two_numbered_blocks_on_one_screen_are_a_refusal() {
        let capture = at_bottom(
            24,
            r"
Steps:
  1. build
  2. test

Do you want to proceed?
  1. Yes
  2. No
",
        );
        assert!(read(&capture).is_none());
    }

    #[test]
    fn one_option_is_not_a_choice() {
        assert!(read(&at_bottom(24, "│ Proceed? │\n│ 1. Yes │")).is_none());
    }

    /// The prompt is the last thing Claude draws before its composer. A numbered list at the top
    /// of a long screen is somebody's document.
    #[test]
    fn a_block_high_up_a_long_screen_is_a_refusal() {
        let capture = screen(
            40,
            "Release checklist:\n  1. tag it\n  2. push it\n  3. announce it\n",
        );
        assert!(read(&capture).is_none());
    }

    /// The separator and the space after it are what stop prose from being an option.
    #[test]
    fn a_number_without_a_separator_is_not_an_option() {
        assert!(numbered("2024 was a year").is_none());
        assert!(numbered("1.5 seconds elapsed").is_none());
        assert!(numbered("0. zero").is_none(), "zero would break contiguity");
        assert!(numbered("1.").is_none(), "an empty label");
        assert!(numbered(&format!("1. {}", "x".repeat(MAX_LABEL + 1))).is_none());

        assert!(numbered("1. Yes").is_some());
        assert!(numbered("2) No").is_some());
        assert!(numbered("  ❯ 3. Maybe").is_some());
    }

    // --- the digest -----------------------------------------------------------------------

    /// The guard on every answer, and what it has to cover.
    ///
    /// Two prompts can offer *the same three options* about different commands — which is exactly
    /// the pair that must never be confused, because the second one arrives while a thumb is
    /// travelling towards the first one's button.
    #[test]
    fn the_digest_separates_two_prompts_that_offer_the_same_options() {
        let read_one = at_bottom(24, "│ Read src/main.rs? │\n│ 1. Yes │\n│ 2. No │");
        let delete = at_bottom(24, "│ Run rm -rf /?     │\n│ 1. Yes │\n│ 2. No │");
        let a = read(&read_one).expect("a prompt");
        let b = read(&delete).expect("a prompt");
        assert_ne!(a.digest, b.digest, "two questions hashed alike");
        assert_eq!(a.options, b.options, "the fixtures do share their options");

        // And it is stable: the same screen read twice is the same digest.
        assert_eq!(read(&read_one).expect("a prompt").digest, a.digest);
    }

    #[test]
    fn the_digest_moves_when_a_label_does() {
        let before = read(&at_bottom(24, "│ Proceed? │\n│ 1. Yes │\n│ 2. No │")).expect("a prompt");
        let after =
            read(&at_bottom(24, "│ Proceed? │\n│ 1. Yes │\n│ 2. Never │")).expect("a prompt");
        assert_ne!(before.digest, after.digest);
    }

    // --- the answer -----------------------------------------------------------------------

    /// One digit, and no Return. `agents.rs`'s `AWAITING_PERMISSION` records what the extra byte
    /// costs at a selection list: it *is* an answer.
    #[test]
    fn an_answer_is_one_digit_and_nothing_else() {
        assert_eq!(answer_bytes(1), Some(vec![b'1']));
        assert_eq!(answer_bytes(9), Some(vec![b'9']));
        assert_eq!(answer_bytes(0), None);
        assert_eq!(answer_bytes(10), None);
        for n in 1..=9u8 {
            assert_eq!(answer_bytes(n).expect("a digit").len(), 1);
        }
    }
}
