//! The two prompts the headless lane is actually used for.
//!
//! [`crate::headless`] has been complete and callerless since it landed. The plan names two
//! uses for it — a commit message from the staged diff, and "explain this selection" from the
//! editor — and this module is those two, built as tested functions rather than as string
//! literals inside a Tauri command.
//!
//! # Why the prompt is a tested function
//!
//! Every failure available here is silent. A commit-message prompt that does not forbid
//! preamble yields `"Here is a commit message:\n\nfix: …"` straight into the user's commit
//! box. One that does not bound the diff sends a 4 MB vendored-lockfile change to a model and
//! bills for it. One that leaks a mention of tools invites a run that reads files it was
//! already given. None of these fail a build, and all of them are cheap to assert.
//!
//! # No tools, and a budget on both
//!
//! Both prompts carry everything the model needs — the diff, the selection — so
//! [`crate::ToolAccess::None`] (the default) costs nothing and removes the possibility of an
//! unattended `Bash` in the user's repository. Both also set `max_budget_usd`, because both
//! are reached by a keystroke: a one-shot that loops is a bill nobody agreed to and no screen
//! in this app would show it happening.

use cide_ipc::HeadlessRequest;

/// How much diff text a commit-message prompt may carry.
///
/// 24 KiB is roughly 300 changed lines with context — comfortably more than a commit a human
/// would write a single message for, and far below anything that threatens a context window
/// or a bill. The failure this bounds is not hypothetical: a `Cargo.lock` or a regenerated
/// `pnpm-lock.yaml` is a single staged file of megabytes, and it carries no information a
/// commit message wants.
pub const DIFF_BUDGET: usize = 24 * 1024;

/// How much selected text "explain this" may carry. A selection larger than this is a file,
/// and the answer to "explain this file" is a conversation in a pane rather than a one-shot.
pub const SELECTION_BUDGET: usize = 16 * 1024;

/// A ceiling on either one-shot, in dollars.
///
/// Deliberately generous relative to the work — a commit message over a large diff measured
/// around $0.08 on 2.1.226 — and deliberately present, because the number that matters is the
/// one a runaway run cannot pass.
pub const BUDGET_USD: f64 = 0.50;

/// Ask for a commit message over a staged diff.
///
/// `branch` is passed when it is known: branch names in this repository routinely carry an
/// issue key, and a message that can reference it is better than one that cannot. It is
/// context, not an instruction — the prompt says so, because a model told about a branch
/// called `fix-login` will otherwise write "fix login" whatever the diff says.
pub fn commit_message(diff: &str, branch: Option<&str>) -> HeadlessRequest {
    let (diff, truncated) = clamp(diff, DIFF_BUDGET);

    let mut prompt = String::with_capacity(diff.len() + 1024);
    prompt.push_str(
        "Write a git commit message for the staged changes below.\n\
         \n\
         Rules:\n\
         - Reply with the commit message and nothing else. No preamble, no explanation, no \
         code fences, no surrounding quotes.\n\
         - First line: imperative mood, under 72 characters, no trailing full stop.\n\
         - If the change needs more than the subject line, add a blank line and then a body \
         of short prose or `- ` bullets explaining *why*, not restating the diff.\n\
         - Describe only what the diff actually changes. Do not speculate about intent you \
         cannot see, and do not mention files you were not shown.\n",
    );
    if let Some(branch) = branch.map(str::trim).filter(|b| !b.is_empty()) {
        prompt.push_str(&format!(
            "\nThe current branch is `{branch}`. Use it only if it carries an issue key worth \
             referencing; do not let its name decide what the message says.\n"
        ));
    }
    if truncated {
        // Said in the prompt rather than only in the log: a model that is told the diff is
        // partial writes a subject line that generalises, while one that is not writes a
        // confident summary of the half it happened to see.
        prompt.push_str(
            "\nThe diff below is TRUNCATED — it is too large to include in full. Write a \
             message that covers what you can see and does not claim to be exhaustive.\n",
        );
    }
    prompt.push_str("\n--- staged diff ---\n");
    prompt.push_str(&diff);

    with_budget(HeadlessRequest::new(prompt))
}

/// Ask for an explanation of a selected region of a file.
///
/// The line numbers are 1-based, matching what the user has in the gutter. They are in the
/// prompt because an explanation that can say "line 41" is usable and one that says "the
/// third statement" is not.
pub fn explain_selection(
    path: &str,
    language: Option<&str>,
    start_line: u32,
    end_line: u32,
    text: &str,
) -> HeadlessRequest {
    let (text, truncated) = clamp(text, SELECTION_BUDGET);
    let language = language.map(str::trim).filter(|l| !l.is_empty());

    let mut prompt = String::with_capacity(text.len() + 1024);
    prompt.push_str(
        "Explain the selected code below.\n\
         \n\
         Rules:\n\
         - Reply with the explanation and nothing else. No preamble and no restatement of the \
         question.\n\
         - Lead with one sentence saying what it does. Then, only if they earn their place, \
         short notes on how it works, what it assumes, and anything that looks like a bug.\n\
         - Refer to lines by the numbers given below, which are the numbers in the file.\n\
         - You have been given the selection and nothing else. Where an answer depends on \
         code you cannot see, say so rather than inventing it.\n",
    );
    if truncated {
        prompt.push_str(
            "\nThe selection below is TRUNCATED. Explain what you can see and say that it is \
             only part of the selection.\n",
        );
    }
    prompt.push_str(&format!(
        "\n--- {path}, lines {start_line}-{end_line}{} ---\n",
        language.map(|l| format!(" ({l})")).unwrap_or_default()
    ));
    prompt.push_str(&text);

    with_budget(HeadlessRequest::new(prompt))
}

/// The budget both one-shots carry. One function so neither can be given a prompt and no cap.
fn with_budget(mut request: HeadlessRequest) -> HeadlessRequest {
    request.max_budget_usd = Some(BUDGET_USD);
    request
}

/// Cut `text` to at most `budget` **bytes**, on a character boundary, keeping the front.
///
/// Bytes rather than characters because the thing being bounded is what crosses a pipe and
/// what a model is billed for, and both are measured in bytes.
///
/// The front, not the end: a unified diff's information density is highest at the top of each
/// file's hunk list, and a truncated tail is at least a well-formed prefix of a diff, while a
/// truncated head is a fragment beginning mid-hunk that a model has to guess the shape of.
///
/// Returns whether anything was cut, so the caller can say so in the prompt.
pub fn clamp(text: &str, budget: usize) -> (String, bool) {
    if text.len() <= budget {
        return (text.to_string(), false);
    }
    // Walk back to a character boundary. Slicing a `str` at a byte index inside a multi-byte
    // character panics, and a diff of a file with an accented identifier in it is an ordinary
    // thing to stage.
    let mut cut = budget;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    (text[..cut].to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_commit_prompt_forbids_the_preamble_that_would_land_in_the_commit_box() {
        // The one failure that reaches the user's screen verbatim: the text goes straight
        // into the commit message field, so "Here is a commit message:" is a bug they have
        // to delete by hand every time.
        let request = commit_message("diff --git a/x b/x\n+one\n", None);
        let prompt = request.prompt.to_lowercase();
        assert!(prompt.contains("nothing else"), "{prompt}");
        assert!(prompt.contains("no preamble"), "{prompt}");
        assert!(prompt.contains("no code fences") || prompt.contains("code fences"));
    }

    #[test]
    fn the_diff_is_in_the_prompt_and_the_prompt_is_not_in_the_argv() {
        // The two halves of the same rule. `headless::argv` asserts the second; this asserts
        // the diff actually travels, which is the thing a refactor could quietly drop.
        let request = commit_message(
            "diff --git a/src/main.rs b/src/main.rs\n+fn main() {}\n",
            None,
        );
        assert!(request.prompt.contains("+fn main() {}"));
        assert!(request.prompt.contains("--- staged diff ---"));
    }

    #[test]
    fn a_branch_is_context_and_is_marked_as_such() {
        let request = commit_message("+x\n", Some("feature/CIDE-91-detach"));
        assert!(request.prompt.contains("feature/CIDE-91-detach"));
        // Without this qualifier the branch name becomes the message.
        assert!(request.prompt.contains("do not let its name decide"));
    }

    #[test]
    fn an_empty_or_whitespace_branch_is_left_out_entirely() {
        for branch in [Some(""), Some("   "), None] {
            let request = commit_message("+x\n", branch);
            assert!(
                !request.prompt.contains("current branch"),
                "an empty branch produced a sentence about one"
            );
        }
    }

    #[test]
    fn an_oversized_diff_is_cut_and_the_prompt_says_so() {
        // A regenerated lockfile is a single staged file of megabytes. Sending it whole is a
        // bill; sending it cut without saying so is a confident summary of an arbitrary
        // prefix.
        let huge = "x".repeat(DIFF_BUDGET * 3);
        let request = commit_message(&huge, None);
        assert!(request.prompt.contains("TRUNCATED"));
        assert!(
            request.prompt.len() < DIFF_BUDGET + 4096,
            "the diff was not bounded: {} bytes",
            request.prompt.len()
        );
    }

    #[test]
    fn a_diff_that_fits_is_not_labelled_truncated() {
        let request = commit_message("+small\n", None);
        assert!(!request.prompt.contains("TRUNCATED"));
    }

    #[test]
    fn clamping_never_splits_a_character() {
        // A panic here would turn "your diff was large" into a crash inside the git panel.
        let text = "é".repeat(100); // two bytes each
        for budget in 0..40 {
            let (cut, truncated) = clamp(&text, budget);
            assert!(cut.len() <= budget);
            assert!(truncated);
            assert!(cut.chars().all(|c| c == 'é'));
        }
        assert_eq!(clamp("hello", 5), ("hello".to_string(), false));
    }

    #[test]
    fn an_explanation_names_the_file_and_the_real_line_numbers() {
        let request = explain_selection(
            "src/lib.rs",
            Some("rust"),
            41,
            48,
            "fn answer() -> u32 { 42 }",
        );
        assert!(request.prompt.contains("src/lib.rs"));
        assert!(request.prompt.contains("lines 41-48"));
        assert!(request.prompt.contains("(rust)"));
        assert!(request.prompt.contains("fn answer() -> u32 { 42 }"));
    }

    #[test]
    fn an_explanation_is_told_it_cannot_see_the_rest_of_the_file() {
        // Tools are off, so the model genuinely has only the selection. Without this line it
        // writes about the surrounding code as though it had read it.
        let request = explain_selection("a.rs", None, 1, 2, "let x = f();");
        assert!(request.prompt.contains("code you cannot see"));
    }

    #[test]
    fn an_unknown_language_leaves_the_header_clean() {
        let request = explain_selection("notes", None, 1, 1, "x");
        assert!(request.prompt.contains("--- notes, lines 1-1 ---"));
        let request = explain_selection("notes", Some("  "), 1, 1, "x");
        assert!(request.prompt.contains("--- notes, lines 1-1 ---"));
    }

    #[test]
    fn both_one_shots_carry_a_budget() {
        // Both are reached by a keystroke, and neither has a UI that could show a run that
        // kept going. A cap is the only thing standing between a loop and a bill.
        assert_eq!(commit_message("+x", None).max_budget_usd, Some(BUDGET_USD));
        assert_eq!(
            explain_selection("a", None, 1, 1, "x").max_budget_usd,
            Some(BUDGET_USD)
        );
    }

    #[test]
    fn neither_one_shot_pins_a_model() {
        // The user's own default is the right choice for a feature they did not ask to
        // configure — and pinning `opus` here would quietly make every commit message the
        // most expensive one available.
        assert_eq!(commit_message("+x", None).model, None);
        assert_eq!(explain_selection("a", None, 1, 1, "x").model, None);
    }
}
