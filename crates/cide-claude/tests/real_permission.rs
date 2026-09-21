//! The parser, against a prompt a real `claude` actually drew. (M72)
//!
//! `#[ignore]`d, and unlike the other ignored tests in this crate it spends **no quota and needs
//! no network** — it needs a file. The corpus in `permission.rs` is hand-written from the shapes
//! Claude Code ships, which is honest and is not the same thing as having seen one:
//!
//! ```sh
//! # with a permission prompt on screen, from another terminal
//! tmux capture-pane -p -t <pane> > /tmp/prompt.txt      # or just copy the pane and paste it
//! CIDE_PROMPT_SCREEN=/tmp/prompt.txt \
//!   cargo test -p cide-claude --test real_permission -- --ignored --nocapture
//! ```
//!
//! It prints what it read, so the answer is checkable by eye against the screen it came from —
//! which is the point. A parser that returns `None` here has found a shape the corpus does not
//! cover, and that file is then a fixture worth adding.
//!
//! # The one fact nobody has measured
//!
//! Whether the CLI selects on the **digit alone** or wants a Return after it.
//! `cide_claude::permission::answer_bytes` sends the digit and nothing else, because
//! `agents.rs`'s `AWAITING_PERMISSION` constant records what the extra byte costs at a selection
//! list: a lone `\r` *is* an answer, so a digit followed by a Return risks the Return being read
//! as a confirmation of whatever is selected once the digit has already acted. If the digit turns
//! out to be insufficient, the fix is a **second** write taken only after re-reading the screen
//! and confirming the digest has not moved — never one write of two bytes.

use cide_ipc::SessionState;

/// The width to render the captured text at.
///
/// Wide enough that a copied prompt is not re-wrapped by the parser's own grid, which would split
/// an option across two rows and make it unreadable for reasons that have nothing to do with the
/// parser.
const COLS: u16 = 200;

#[test]
#[ignore = "needs CIDE_PROMPT_SCREEN pointing at a captured prompt"]
fn a_real_prompt_reads_as_its_options() {
    let Some(path) = std::env::var_os("CIDE_PROMPT_SCREEN") else {
        panic!("set CIDE_PROMPT_SCREEN to a file holding a captured prompt screen");
    };
    let text = std::fs::read_to_string(&path).expect("the capture is readable");
    let lines: Vec<&str> = text.lines().collect();

    let mut vt = vt100::Parser::new(lines.len().max(4) as u16, COLS, 0);
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            vt.process(b"\r\n");
        }
        vt.process(line.as_bytes());
    }
    let capture = cide_pty::screen::capture(vt.screen());

    let prompt = cide_claude::permission::parse(&capture, SessionState::AwaitingPermission);
    match prompt {
        None => panic!(
            "the parser refused this screen. That is the safe direction, and it means the corpus \
             in `permission.rs` does not cover this shape — add it."
        ),
        Some(prompt) => {
            println!("question:");
            for line in &prompt.question {
                println!("  {line}");
            }
            println!("options:");
            for option in &prompt.options {
                let marker = if prompt.selected == Some(option.number) {
                    "*"
                } else {
                    " "
                };
                println!("  {marker} {}. {}", option.number, option.label);
            }
            println!("digest: {}", prompt.digest);

            assert!(!prompt.options.is_empty());
            assert!(
                !prompt.question.is_empty(),
                "a prompt with no question above it is readable but not answerable by a person"
            );
        }
    }
}
