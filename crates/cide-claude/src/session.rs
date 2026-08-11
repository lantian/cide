//! The conversation arguments for a `claude` child, and the id that child will report.
//!
//! # Why this is a module rather than three `.arg()` calls at the spawn site
//!
//! It was three `.arg()` calls at the spawn site, in `cide-app`'s `cmd/session.rs`, and the
//! order and composition of those flags is exactly the kind of thing a patch release of an
//! undocumented CLI can retract. It did:
//!
//! > `Error: --session-id can only be used with --continue or --resume if --fork-session is
//! > also specified.`
//!
//! — 2.1.227, on every Resume click, for a combination that 2.1.226 accepted and that a
//! comment in that file recorded as verified. Here it is a pure function with unit tests for
//! the shapes and an `#[ignore]`d integration test (`tests/real_session_args.rs`) that puts
//! those exact shapes in front of the installed binary. That is the difference between an
//! assertion about the CLI and a *check* of it: the second one can be re-run in one command
//! the next time a pane will not start.
//!
//! # The rule the flags encode
//!
//! `claude --help` on 2.1.227 documents `--fork-session` as *"When resuming, create a new
//! session ID instead of reusing the original (use with --resume or --continue)"*. Read the
//! other way round, which is the way that matters here: **without `--fork-session`, a resumed
//! conversation keeps the parent's id.** So the id is not always ours to choose, and
//! [`conversation`] returns the one the child will actually use rather than leaving every
//! caller to assume.

use cide_ipc::SessionId;

/// The arguments for a Claude child, and the session id it will report.
///
/// ```text
/// fresh           --session-id <minted>
/// resume          --resume <parent>                          ← and nothing else
/// resume + fork   --resume <parent> --fork-session --session-id <minted>
/// ```
///
/// # The returned id is the load-bearing half
///
/// On a plain resume the minted id is fiction: the child announces the *parent's* id in every
/// hook frame, appends to the parent's transcript, and is resumable next launch only under the
/// parent's id. A caller that keys its registry, its pane binding and its hook routing on the
/// minted id therefore gets a session that exists and that nothing can find — no status line,
/// no token figures, no busy-vs-idle close confirm — and writes an id into its saved workspace
/// with no transcript behind it, so the *next* launch finds nothing to resume either.
///
/// Returning the effective id is what makes "the hook-learned id is authoritative" true by
/// construction instead of aspirational: in all three shapes above, this id **is** the one the
/// CLI will report, so there is no correction left for a hook to teach anybody.
///
/// # Fork
///
/// `--fork-session` beside `--session-id` is the one shape in which naming an id while
/// resuming is still legal, and the id we pass is honoured: the fork inherits the parent's
/// history, the parent's transcript survives untouched, and both stay independently resumable.
///
/// `fork` without `resume` is meaningless — there is nothing to branch from — and is treated
/// as a plain new session rather than passed through to be rejected by the CLI.
pub fn conversation(
    minted: SessionId,
    resume: Option<SessionId>,
    fork: bool,
) -> (SessionId, Vec<String>) {
    let Some(parent) = resume else {
        return (minted, vec!["--session-id".into(), minted.to_string()]);
    };
    if fork {
        return (
            minted,
            vec![
                "--resume".into(),
                parent.to_string(),
                "--fork-session".into(),
                "--session-id".into(),
                minted.to_string(),
            ],
        );
    }
    // The minted id is discarded. A plain resume *is* the parent conversation, under the
    // parent's id, and pretending otherwise is what broke every consumer downstream of it.
    (parent, vec!["--resume".into(), parent.to_string()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_session_only_names_itself() {
        let id = SessionId::new();
        assert_eq!(
            conversation(id, None, false),
            (id, vec!["--session-id".to_string(), id.to_string()])
        );
    }

    #[test]
    fn a_plain_resume_passes_no_session_id_at_all() {
        // The bug, exactly: `claude --resume <parent> --session-id <minted>` is rejected by
        // 2.1.227 with "--session-id can only be used with --continue or --resume if
        // --fork-session is also specified", which is every Resume click failing before a
        // pane ever appears. `tests/real_session_args.rs` puts this in front of the binary.
        let minted = SessionId::new();
        let parent = SessionId::new();
        let (_, args) = conversation(minted, Some(parent), false);
        assert_eq!(args, vec!["--resume".to_string(), parent.to_string()]);
        assert!(
            !args.iter().any(|a| a == "--session-id"),
            "a plain resume must not name a session id: {args:?}"
        );
    }

    #[test]
    fn a_plain_resume_keeps_the_parents_id_and_throws_the_minted_one_away() {
        let minted = SessionId::new();
        let parent = SessionId::new();
        let (id, _) = conversation(minted, Some(parent), false);
        assert_eq!(id, parent);
        assert_ne!(id, minted);
    }

    #[test]
    fn forking_branches_from_the_parent_and_keeps_our_id() {
        // Order matters and a wrong one fails silently: `--resume <parent>` and
        // `--session-id <ours>` both take a uuid, so a swap is still a valid command line
        // that resumes the wrong conversation.
        let id = SessionId::new();
        let parent = SessionId::new();
        assert_eq!(
            conversation(id, Some(parent), true),
            (
                id,
                vec![
                    "--resume".to_string(),
                    parent.to_string(),
                    "--fork-session".to_string(),
                    "--session-id".to_string(),
                    id.to_string(),
                ]
            )
        );
    }

    #[test]
    fn forking_with_nothing_to_fork_from_is_an_ordinary_new_session() {
        // Rather than passing `--fork-session` alone for the CLI to reject. A split that asked
        // to branch a project with no primary session should still give the user a pane.
        let id = SessionId::new();
        assert_eq!(
            conversation(id, None, true),
            (id, vec!["--session-id".to_string(), id.to_string()])
        );
    }

    #[test]
    fn only_a_plain_resume_answers_with_an_id_it_was_not_given() {
        // What `session_spawn`'s duplicate guard is protecting. A plain resume is the only
        // shape whose id is not freshly minted, so it is the only one that can name a session
        // the registry already holds — and inserting over a live entry would replace the only
        // handle to a running child. If that ever stops being true, the guard is in the wrong
        // place and this is what notices.
        let minted = SessionId::new();
        let parent = SessionId::new();
        assert_eq!(conversation(minted, None, false).0, minted);
        assert_eq!(conversation(minted, None, true).0, minted);
        assert_eq!(conversation(minted, Some(parent), true).0, minted);
        assert_ne!(conversation(minted, Some(parent), false).0, minted);
    }
}
