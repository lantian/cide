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

use cide_core::claude_cli::{Injected, Injection};
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
///
/// # `injected`: which of these three flags cide still passes, and how it spells them
///
/// [`cide_core::claude_cli::Injected`] is resolved from the user's settings once per spawn and
/// handed in, so that this function and the refusal table agree about the flag set by
/// construction. [`Injected::defaults`] is what the shipped configuration resolves to and what
/// this module was written against.
///
/// Two of the degraded shapes need care, and they are the ones this module exists to prevent:
///
/// * **Fresh with [`Injection::SessionId`] off** is `(minted, vec![])` — no argument at all,
///   and the returned id is still `minted`. The CLI will mint its own and report *that* in its
///   hook frames, so nothing filed under our id exists and resume is gone; what survives is the
///   registry key and the busy/idle chrome, because `cmd::session.rs` routes hooks on
///   `CIDE_SESSION`, which it sets separately and which the child echoes back.
/// * **Fork with [`Injection::ForkSession`] off** must *not* degrade to
///   `--resume <parent> --session-id <minted>`. 2.1.227 rejects exactly that combination —
///   *"--session-id can only be used with --continue or --resume if --fork-session is also
///   specified"* — which is every Resume click failing before a pane appears, and it is the bug
///   that pulled this code out of `cmd/session.rs` in the first place. It falls back to the
///   plain-resume shape instead, parent id and all.
///
/// With [`Injection::Resume`] off, a resume degrades to the fresh shape rather than to
/// `--fork-session` alone: a lone `--fork-session` has nothing to branch from and the CLI
/// rejects it. With [`Injection::SessionId`] off during a fork the pair is
/// `--resume <parent> --fork-session`, which is legal and is the documented use of that flag —
/// it asks the CLI to mint an id instead of reusing the parent's, which is what switching the
/// id injection off asked for.
pub fn conversation(
    minted: SessionId,
    resume: Option<SessionId>,
    fork: bool,
    injected: &Injected,
) -> (SessionId, Vec<String>) {
    // The fresh shape, and the fallback every degraded path lands on. Written once so a
    // disabled `--session-id` cannot mean one thing here and another two branches down.
    let fresh = || match injected.flag(Injection::SessionId) {
        Some(flag) => (minted, vec![flag.to_string(), minted.to_string()]),
        None => (minted, Vec::new()),
    };

    let Some(parent) = resume else {
        return fresh();
    };
    // Nothing to resume *with*: the parent conversation is not being named, so this is a new
    // session however it was asked for. The minted id is honest here because the child really
    // will start a conversation of its own.
    let Some(resume_flag) = injected.flag(Injection::Resume) else {
        return fresh();
    };

    // `--fork-session` is what makes naming an id beside a resume legal, so it is the half the
    // shape hangs on and it is checked first. Without it, `--resume <parent> --session-id
    // <minted>` is the pair 2.1.227 rejects outright — falling through to the plain resume
    // below is the shape the CLI accepts and the one whose meaning is closest.
    if fork && let Some(fork_flag) = injected.flag(Injection::ForkSession) {
        let mut args = vec![
            resume_flag.to_string(),
            parent.to_string(),
            fork_flag.to_string(),
        ];
        // The id is optional here and its absence is legal: `--fork-session` on its own asks
        // the CLI to mint a new id rather than reuse the parent's, which is exactly what a
        // user who switched the id injection off has asked for. The returned id is still
        // `minted` — a fiction the CLI will not honour, and the same fiction the fresh shape
        // returns for the same reason: it is the registry key and what `CIDE_SESSION` carries,
        // not a claim about the transcript.
        if let Some(id_flag) = injected.flag(Injection::SessionId) {
            args.push(id_flag.to_string());
            args.push(minted.to_string());
        }
        return (minted, args);
    }

    // The minted id is discarded. A plain resume *is* the parent conversation, under the
    // parent's id, and pretending otherwise is what broke every consumer downstream of it.
    (parent, vec![resume_flag.to_string(), parent.to_string()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::ClaudeCli;

    /// What the shipped configuration resolves to: all four, spelled as they always were.
    fn all() -> Injected {
        Injected::defaults()
    }

    /// The same, with one injection switched off — resolved through `claude_cli::injected`
    /// rather than built by hand, so these tests exercise the real resolution and a change to
    /// the override rules cannot leave them asserting about a set nothing produces.
    fn without(which: Injection) -> Injected {
        let mut cli = ClaudeCli::default();
        match which {
            Injection::SessionId => cli.inject.session_id.enabled = false,
            Injection::Resume => cli.inject.resume.enabled = false,
            Injection::ForkSession => cli.inject.fork_session.enabled = false,
            Injection::Settings => cli.inject.settings.enabled = false,
        }
        cide_core::claude_cli::injected(&cli).0
    }

    #[test]
    fn a_plain_session_only_names_itself() {
        let id = SessionId::new();
        assert_eq!(
            conversation(id, None, false, &all()),
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
        let (_, args) = conversation(minted, Some(parent), false, &all());
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
        let (id, _) = conversation(minted, Some(parent), false, &all());
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
            conversation(id, Some(parent), true, &all()),
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
            conversation(id, None, true, &all()),
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
        assert_eq!(conversation(minted, None, false, &all()).0, minted);
        assert_eq!(conversation(minted, None, true, &all()).0, minted);
        assert_eq!(conversation(minted, Some(parent), true, &all()).0, minted);
        assert_ne!(conversation(minted, Some(parent), false, &all()).0, minted);
    }

    // --- the injections switched off, one at a time -------------------------------------

    /// With the session id injection off cide names no conversation at all, and the id it
    /// answers with is still the minted one.
    ///
    /// That id is still the registry key and still what `CIDE_SESSION` carries, so the pane's
    /// busy/idle chrome and its close confirm keep working; what is gone is the transcript
    /// filed under it, and with it resume. The Settings toggle says exactly that.
    #[test]
    fn a_fresh_session_with_the_id_injection_off_names_nothing() {
        let id = SessionId::new();
        let (effective, args) = conversation(id, None, false, &without(Injection::SessionId));
        assert_eq!(args, Vec::<String>::new());
        assert_eq!(
            effective, id,
            "the registry key does not move; only the argv changes"
        );
    }

    /// With the resume injection off a restored pane starts a fresh conversation, rather than
    /// emitting a lone `--fork-session` for the CLI to reject.
    #[test]
    fn a_resume_with_the_resume_injection_off_degrades_to_a_fresh_session() {
        let minted = SessionId::new();
        let parent = SessionId::new();
        let off = without(Injection::Resume);

        let (effective, args) = conversation(minted, Some(parent), false, &off);
        assert_eq!(args, vec!["--session-id".to_string(), minted.to_string()]);
        assert_eq!(
            effective, minted,
            "nothing is being continued, so the id the child reports is ours after all"
        );

        // And the fork gesture with it: there is nothing to branch from once the parent is
        // not named.
        let (_, args) = conversation(minted, Some(parent), true, &off);
        assert!(!args.iter().any(|a| a == "--fork-session"), "{args:?}");
        assert!(!args.iter().any(|a| a == "--resume"), "{args:?}");
    }

    /// **The shape 2.1.227 rejects, reached through the new door.**
    ///
    /// A fork with `--fork-session` switched off must not become
    /// `--resume <parent> --session-id <minted>`: the CLI answers *"--session-id can only be
    /// used with --continue or --resume if --fork-session is also specified"* and the pane
    /// never appears. It degrades to the plain resume instead — the parent's conversation,
    /// under the parent's id.
    #[test]
    fn a_fork_with_the_fork_injection_off_is_a_plain_resume_and_never_the_rejected_pair() {
        let minted = SessionId::new();
        let parent = SessionId::new();
        let (effective, args) =
            conversation(minted, Some(parent), true, &without(Injection::ForkSession));
        assert_eq!(args, vec!["--resume".to_string(), parent.to_string()]);
        assert!(
            !args.iter().any(|a| a == "--session-id"),
            "the combination the CLI rejects outright: {args:?}"
        );
        assert_eq!(effective, parent, "a plain resume keeps the parent's id");
    }

    /// The same rejected pair from the other side: the *session id* injection off during a
    /// fork leaves `--resume <parent> --fork-session`, which is legal — `--fork-session` asks
    /// the CLI to mint an id of its own, which is precisely what a caller who switched the id
    /// injection off has asked for.
    #[test]
    fn a_fork_without_the_id_injection_still_branches_and_lets_the_cli_name_it() {
        let minted = SessionId::new();
        let parent = SessionId::new();
        let (_, args) = conversation(minted, Some(parent), true, &without(Injection::SessionId));
        assert_eq!(
            args,
            vec![
                "--resume".to_string(),
                parent.to_string(),
                "--fork-session".to_string()
            ]
        );
    }

    /// A renamed injection is written under its new spelling, in the same position.
    ///
    /// The position is not decoration: `--resume <parent>` and `--session-id <ours>` both take
    /// a uuid, so a shape that put them the wrong way round is still a valid command line that
    /// resumes the wrong conversation.
    #[test]
    fn a_renamed_injection_is_written_under_its_new_spelling() {
        let mut cli = ClaudeCli::default();
        cli.inject.session_id.flag = "--sid".into();
        cli.inject.resume.flag = "--continue-from".into();
        cli.inject.fork_session.flag = "--branch".into();
        let injected = cide_core::claude_cli::injected(&cli).0;

        let minted = SessionId::new();
        let parent = SessionId::new();
        assert_eq!(
            conversation(minted, None, false, &injected).1,
            vec!["--sid".to_string(), minted.to_string()]
        );
        assert_eq!(
            conversation(minted, Some(parent), true, &injected).1,
            vec![
                "--continue-from".to_string(),
                parent.to_string(),
                "--branch".to_string(),
                "--sid".to_string(),
                minted.to_string()
            ]
        );
    }

    /// The settings injection is not this function's business, and nothing here may start
    /// depending on it: `--settings` is folded in at the spawn site, after these arguments.
    #[test]
    fn the_settings_injection_changes_none_of_these_shapes() {
        let minted = SessionId::new();
        let parent = SessionId::new();
        for (with, without_settings) in [
            (
                conversation(minted, None, false, &all()),
                conversation(minted, None, false, &without(Injection::Settings)),
            ),
            (
                conversation(minted, Some(parent), false, &all()),
                conversation(minted, Some(parent), false, &without(Injection::Settings)),
            ),
            (
                conversation(minted, Some(parent), true, &all()),
                conversation(minted, Some(parent), true, &without(Injection::Settings)),
            ),
        ] {
            assert_eq!(with, without_settings);
        }
    }
}
