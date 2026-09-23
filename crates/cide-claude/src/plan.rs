//! Which option on a plan-approval prompt means *yes, proceed*. (M79)
//!
//! The spinner opens a `claude` in plan mode with nobody at the keyboard, so somebody has to
//! answer the approval that plan mode ends at. This decides **which** answer, given a prompt
//! [`crate::permission::parse`] has already read off the screen.
//!
//! # The hook road was tried first, and cannot work
//!
//! A `PreToolUse` hook answering `permissionDecision: "allow"` for `ExitPlanMode` is the exact
//! way to do this — it names one tool, reads no screen and has no timing. It was built and
//! measured against the shipped CLI (2.1.278) by `cide-agents`' `real_plan` test, and the prompt
//! drew anyway. The reason is in the binary: `ExitPlanMode` carries its own `checkPermissions`
//! returning `behavior: "ask"` unconditionally outside the teammate path, plus a
//! `requiresUserInteraction()` that answers `true`. A hook decision does not override a tool
//! that declares it needs a human. `cide-hook`'s guard module carries the same note where the
//! next person would go looking.
//!
//! # So cide types — and every gate that makes that defensible is here or above it
//!
//! `cide_app::agents`' `AWAITING_PERMISSION` note is blunt about this: a lone `\r` at a selection
//! list **is an answer**, which is why a graceful stop refuses to say anything to a run in
//! `AwaitingPermission` and kills it instead. Nothing here contradicts that. What is refused
//! there is writing *blind* — text aimed at a prompt nobody has read. Here the prompt has been
//! read, by a parser whose own header says every rule in it is a reason to refuse, and the answer
//! names the option by its words.
//!
//! Four gates, and dropping any one of them turns this into the thing the stop code refuses:
//!
//! 1. **Only a session cide spawned for the spinner.** The caller's, in `cide_app::claude_tab` —
//!    tracked by id, so a pane, a run or the console can never reach this.
//! 2. **Only in `AwaitingPermission`.** [`crate::permission::parse`]'s first rule, and the one
//!    that makes a screen evidence at all: the hook is exact and has already decided.
//! 3. **Only the plan question**, matched on its own words. Every other prompt in the CLI —
//!    a file write, a command, a fetch — falls through and is left for a person.
//! 4. **Only an option that says yes**, found by **label** and never by position. The numbering
//!    is the CLI's and it has changed before; a hardcoded `1` would silently become whatever
//!    that release put first.
//!
//! # What the prompt actually looks like
//!
//! Measured, 2.1.278:
//!
//! ```text
//! Claude has written up a plan and is ready to execute. Would you like to proceed?
//!  ❯ 1. Yes, and use auto mode
//!    2. Yes, manually approve edits
//!    3. Tell Claude what to change
//! ```
//!
//! Both `Yes` options proceed; they differ in what happens *afterwards*. The spinner's run is
//! unattended, so [`AUTO_HINTS`] prefers the one that does not stop at the next edit — an
//! unattended child parked on a second prompt is the failure this whole feature exists to avoid.
//! When no option says `auto`, the first that says yes is taken, because proceeding under prompts
//! beats not proceeding.

use cide_ipc::remote::{PermissionPrompt, PromptOption};

/// The words that identify a plan approval, on its **options**.
///
/// # Why the options and not the question — measured, and it cost a turn
///
/// The obvious gate is the question, and the first cut used it. Against 2.1.278 it never fired,
/// because [`crate::permission::parse`] hands back an **empty** question for this prompt. Its
/// `question_above` walks up from the options and stops at the first blank line, and the CLI
/// draws one:
///
/// ```text
///    Claude has written up a plan and is ready to execute. Would you like to
///    proceed?
///                                      ← blank
///    ❯ 1. Yes, and use auto mode
/// ```
///
/// That blank is not a bug in the parser — a blank line genuinely does separate one block from
/// another, and widening the walk would make a phone's prompt card start quoting the paragraph
/// above it. So this reads what is reliably there.
///
/// It is also the **better** gate on its own merits, which is why the question check below is a
/// fallback rather than the other half of an `&&`. The wording moves between releases — this one
/// screen carries *two* phrasings of the same question, `Ready to code?` above the plan and
/// `…Would you like to proceed?` below it — while the options have to keep saying what they do,
/// because a person reads them to choose. And they are specific: a tool approval offers *"Yes,
/// and don't ask again"* or *"Yes, allow all edits during this session"*, neither of which is
/// either of these.
const PLAN_OPTION_MARKS: &[&str] = &["auto mode", "manually approve edits"];

/// Words that identify the plan question, when [`crate::permission::parse`] could read one.
///
/// A fallback for a release that changes the option labels but keeps the question, and for the
/// case where no blank line separates the two. Each entry is a set of words that must **all**
/// appear: `plan` alone matches a prompt about editing a file called `plan.md`, and `proceed`
/// alone matches a command confirmation.
const QUESTION_MARKS: &[&[&str]] = &[&["plan", "proceed"], &["ready", "code"]];

/// What an unattended run prefers, in order.
///
/// `auto` first: the measured labels are *"Yes, and use auto mode"* and *"Yes, manually approve
/// edits"*, and the second one parks the child on the next edit — which for a run with nobody
/// watching is the same as not having proceeded, except that it also holds a slot.
const AUTO_HINTS: &[&str] = &["auto"];

/// Which option to answer with, or `None` to leave the prompt for a person.
///
/// Pure over a prompt somebody else already read, so every rule above is a table row in the tests
/// rather than a claim about a live TUI that would cost a model turn to reproduce.
#[must_use]
pub fn approval(prompt: &PermissionPrompt) -> Option<u8> {
    let yeses: Vec<&PromptOption> = prompt
        .options
        .iter()
        .filter(|option| says_yes(&option.label))
        .collect();
    // No option that proceeds is not a plan prompt whatever anything else says, and answering
    // the rest would be picking *"Tell Claude what to change"* with no feedback to give.
    let first = *yeses.first()?;

    if !is_plan_prompt(prompt, &yeses) {
        return None;
    }

    Some(
        yeses
            .iter()
            .find(|option| {
                let label = option.label.to_lowercase();
                AUTO_HINTS.iter().any(|hint| label.contains(hint))
            })
            .map_or(first.number, |option| option.number),
    )
}

/// Is this the prompt plan mode ends at? See [`PLAN_OPTION_MARKS`] for why it looks here first.
fn is_plan_prompt(prompt: &PermissionPrompt, yeses: &[&PromptOption]) -> bool {
    let by_option = yeses.iter().any(|option| {
        let label = option.label.to_lowercase();
        PLAN_OPTION_MARKS.iter().any(|mark| label.contains(mark))
    });
    if by_option {
        return true;
    }
    let question = prompt.question.join(" ").to_lowercase();
    !question.is_empty()
        && QUESTION_MARKS
            .iter()
            .any(|marks| marks.iter().all(|mark| question.contains(mark)))
}

/// The keystrokes that pick option `target`, as a person would press them. (M79)
///
/// # Why not the digit — measured, 2.1.278
///
/// [`crate::permission::answer_bytes`] writes one digit and deliberately no Return, and its
/// header flags the open question: *"Whether the CLI needs the Return at all is the one fact
/// here that has not been measured against a real prompt."* Measured, on the plan prompt: **the
/// digit alone does nothing.** The run answered `1`, the highlight stayed where it was, and the
/// prompt was still on screen three minutes later. The list is navigated and confirmed — its own
/// footer says so on the trust prompt: *"Enter to confirm · Esc to cancel"*.
///
/// So this moves the highlight to the option and confirms it, which is the gesture the UI is
/// built around. It is also the *safer* composition than digit-then-Return, which is the obvious
/// repair: if a future CLI makes the digit act, digit-then-Return sends a Return into whatever
/// the digit opened — the exact hazard `answer_bytes` refuses to risk. An arrow that lands on an
/// option already highlighted is a no-op; there is no ordering here that answers twice.
///
/// `selected` is where the TUI says the highlight is, and `None` means it marked nothing — then
/// no move is made and the Return confirms whatever is highlighted, which is the CLI's own
/// default. That is a weaker claim than the caller wants, so [`crate::permission::parse`]
/// supplying `selected` is what makes this exact; a prompt with no marker at all is left to the
/// caller to refuse if it cares.
///
/// # Why the arrow's spelling is a parameter
///
/// In application-cursor mode (`DECCKM`) Down is `ESC O B`, and in normal mode `ESC [ B` —
/// `ui/src/terminal/keys.ts` branches on exactly this for every arrow. A hardcoded spelling is
/// read by nothing and looks identical to a prompt that cannot be answered, which cost a run to
/// find out on the CLI's workspace-trust list. The caller reads the mode off the mirror
/// (`ScreenInfo::app_cursor`) rather than guessing.
///
/// Answers one write per keystroke so the caller can pace them: a TUI still painting reads a
/// burst as one chunk, which is `cide_app::agents::type_submitted_line`'s whole subject.
#[must_use]
pub fn keystrokes(selected: Option<u8>, target: u8, app_cursor: bool) -> Vec<Vec<u8>> {
    let (down, up): (&[u8], &[u8]) = if app_cursor {
        (b"\x1bOB", b"\x1bOA")
    } else {
        (b"\x1b[B", b"\x1b[A")
    };
    let mut out: Vec<Vec<u8>> = Vec::new();
    if let Some(from) = selected {
        let (key, steps) = if target >= from {
            (down, target - from)
        } else {
            (up, from - target)
        };
        for _ in 0..steps {
            out.push(key.to_vec());
        }
    }
    // A terminal sends `\r` for Enter — `ui/src/terminal/keys.ts`. A `\n` is a different key.
    out.push(b"\r".to_vec());
    out
}

/// Does this label offer to go ahead?
///
/// Anchored at the start, because *"No, and tell Claude what to do differently"* — the longest
/// label the CLI ships — contains no `yes` but a future one might, and *"Yes"* appearing anywhere
/// in a sentence is not the same as an option that is one.
fn says_yes(label: &str) -> bool {
    let label = label.trim().to_lowercase();
    label == "yes" || label.starts_with("yes,") || label.starts_with("yes ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(question: &str, options: &[&str]) -> PermissionPrompt {
        PermissionPrompt {
            question: vec![question.to_string()],
            options: options
                .iter()
                .enumerate()
                .map(|(i, label)| PromptOption {
                    number: u8::try_from(i + 1).expect("few options"),
                    label: (*label).to_string(),
                })
                .collect(),
            selected: Some(1),
            digest: "d".into(),
        }
    }

    /// The prompt as 2.1.278 actually draws it, copied off a real run — **question and all,
    /// which is to say without one.**
    ///
    /// `permission::parse` hands back an empty question here because the CLI puts a blank line
    /// between the sentence and the options and `question_above` stops there. This fixture is
    /// the whole reason `PLAN_OPTION_MARKS` exists; a version of it carrying the question would
    /// pass against a matcher that could never fire against the real thing, which is what the
    /// first cut did.
    fn measured() -> PermissionPrompt {
        prompt(
            "",
            &[
                "Yes, and use auto mode",
                "Yes, manually approve edits",
                "Tell Claude what to change",
            ],
        )
    }

    #[test]
    fn the_measured_prompt_is_answered_with_the_unattended_option() {
        assert_eq!(approval(&measured()), Some(1));
    }

    /// The same prompt as the CLI *also* words it, when the question does reach the parser.
    /// Both phrasings appeared on one screen: `Ready to code?` above the plan, and
    /// `…Would you like to proceed?` below it.
    #[test]
    fn either_phrasing_of_the_question_is_recognised_when_it_is_readable() {
        for question in [
            "Claude has written up a plan and is ready to execute. Would you like to proceed?",
            "Ready to code?",
        ] {
            let case = prompt(question, &["Yes", "Tell Claude what to change"]);
            assert_eq!(approval(&case), Some(1), "{question:?}");
        }
    }

    /// **By label, never by position.** The numbering is the CLI's and has moved before; a
    /// hardcoded `1` would silently become whatever the next release happens to put first —
    /// which here is the option that asks a question rather than the one that proceeds.
    #[test]
    fn the_answer_follows_the_words_and_not_the_number() {
        let reordered = prompt(
            "",
            &[
                "Tell Claude what to change",
                "Yes, manually approve edits",
                "Yes, and use auto mode",
            ],
        );
        assert_eq!(approval(&reordered), Some(3));
    }

    /// With no `auto` option, proceeding under prompts beats not proceeding.
    #[test]
    fn the_first_yes_is_taken_when_none_of_them_is_automatic() {
        let plain = prompt(
            "Claude has written up a plan. Would you like to proceed?",
            &["Yes", "No, keep planning"],
        );
        assert_eq!(approval(&plain), Some(1));
    }

    /// **The negative corpus, and it is the specification.**
    ///
    /// `permission.rs`' header makes the argument and it binds harder here, because this
    /// function's output is a keystroke into a live conversation. Every row is a prompt that
    /// must be left for a person — and `None` costs nothing but a tab sitting with its pane
    /// marked, which is exactly what `autoSpinAcceptPlan: false` asks for anyway.
    #[test]
    fn every_other_prompt_in_the_cli_is_left_for_a_person() {
        let cases = [
            (
                "a file write",
                prompt(
                    "Do you want to make this edit to config.toml?",
                    &["Yes", "Yes, allow all edits during this session", "No"],
                ),
            ),
            (
                "a command",
                prompt(
                    "Do you want to proceed with running rm -rf build?",
                    &[
                        "Yes",
                        "Yes, and don't ask again",
                        "No, tell Claude what to do differently",
                    ],
                ),
            ),
            (
                "a fetch",
                prompt("Do you want to fetch example.com?", &["Yes", "No"]),
            ),
            (
                // The word `plan` in a filename is not a plan approval. This is the row that
                // makes the question match two marks rather than one.
                "an edit to a file that happens to be called plan.md",
                prompt("Do you want to make this edit to plan.md?", &["Yes", "No"]),
            ),
            (
                // The question matches, but nothing offers to go ahead — so it is not the
                // prompt this thinks it is, and answering would be picking feedback at random.
                "a plan question with no option that proceeds",
                prompt(
                    "Claude has written up a plan. Would you like to proceed?",
                    &["Tell Claude what to change", "No, keep planning"],
                ),
            ),
        ];
        for (why, case) in cases {
            assert_eq!(approval(&case), None, "answered a prompt about {why}");
        }
    }

    /// The highlight is moved to the option and confirmed — the gesture the list is built for.
    #[test]
    fn the_option_is_reached_by_moving_and_confirming() {
        // Already highlighted: nothing but the confirm, so a no-op move cannot answer twice.
        assert_eq!(keystrokes(Some(1), 1, false), vec![b"\r".to_vec()]);
        // Two below.
        assert_eq!(
            keystrokes(Some(1), 3, false),
            vec![b"\x1b[B".to_vec(), b"\x1b[B".to_vec(), b"\r".to_vec()]
        );
        // And above, because nothing guarantees the CLI highlights the first one.
        assert_eq!(
            keystrokes(Some(3), 2, false),
            vec![b"\x1b[A".to_vec(), b"\r".to_vec()]
        );
    }

    /// **The arrow's spelling follows the mode the program asked for.**
    ///
    /// A hardcoded `ESC [ B` is read by nothing in application-cursor mode, and a TUI ignoring a
    /// key looks exactly like a prompt that cannot be answered — which is how a real run spent
    /// four minutes blaming the feature for the CLI's workspace-trust list.
    #[test]
    fn the_arrow_follows_the_cursor_mode() {
        assert_eq!(
            keystrokes(Some(1), 2, true),
            vec![b"\x1bOB".to_vec(), b"\r".to_vec()]
        );
        assert_eq!(
            keystrokes(Some(1), 2, false),
            vec![b"\x1b[B".to_vec(), b"\r".to_vec()]
        );
    }

    /// With no marker the highlight cannot be moved, so the confirm takes the CLI's own default.
    #[test]
    fn an_unmarked_list_is_only_confirmed() {
        assert_eq!(keystrokes(None, 2, false), vec![b"\r".to_vec()]);
    }

    /// A `No` option is never read as a yes, however it is worded.
    #[test]
    fn a_refusal_is_never_mistaken_for_an_approval() {
        for label in [
            "No",
            "No, keep planning",
            "No, and tell Claude what to do differently",
            // The trap: a refusal whose text contains the word.
            "No, tell Claude yes was a mistake",
            "Tell Claude what to change",
        ] {
            assert!(!says_yes(label), "{label:?} was read as an approval");
        }
        for label in ["Yes", "yes", "  Yes, and use auto mode", "Yes and proceed"] {
            assert!(says_yes(label), "{label:?} was not read as an approval");
        }
    }
}
