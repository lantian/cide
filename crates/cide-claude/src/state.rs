//! The session state machine, driven by hooks.
//!
//! # What this is for
//!
//! One question, asked in two places: *is this session doing something the user would be
//! upset to interrupt?* The close-confirm setting asks it when a project or the app is about
//! to go away, and the pane chrome asks it to decide what to draw. Getting it wrong in one
//! direction means a confirmation dialog on every close, which trains people to dismiss it;
//! in the other it means killing a `claude` mid-answer without asking.
//!
//! So [`SessionState::is_live`] deliberately means `Busy | AwaitingPermission` and not "the
//! process exists". A session sitting at a prompt is not live in any sense the user cares
//! about, however long it has been running.
//!
//! # Why hooks and not PTY output
//!
//! Watching the terminal for a spinner would be guessing at a TUI that changes between
//! releases. Hooks are the CLI telling us what it is doing, in its own words. The fallback
//! for a CLI too old to emit them, or a hook that never arrives, is that the session stays in
//! whatever state it last reached — which is why every transition below is driven by an event
//! that *has* happened rather than by a timeout that assumes one has not.

use cide_ipc::SessionState;

use crate::hook::HookEvent;

/// Apply one hook event to a session's state.
///
/// Returns `None` when the event says nothing about liveness, so a caller can distinguish
/// "no change" from "changed to the same value" without comparing.
///
/// The subtle one is [`HookEvent::SubagentStop`]: a subagent finishing does **not** end the
/// turn. The main agent is still working and will send `Stop` when it is actually done, so
/// treating `SubagentStop` as the end would mark a session idle in the middle of a
/// multi-agent task — exactly when interrupting it is most costly.
pub fn next_state(current: SessionState, event: HookEvent) -> Option<SessionState> {
    let next = match event {
        // The session exists and is waiting for a prompt. Reached after the splash.
        HookEvent::SessionStart => SessionState::Idle,

        // The turn begins.
        HookEvent::UserPromptSubmit => SessionState::Busy,

        // Tool traffic only confirms the turn is still running. It is recorded because a
        // session that was somehow marked idle mid-turn should correct itself rather than
        // stay wrong until the next prompt.
        HookEvent::PreToolUse | HookEvent::PostToolUse | HookEvent::PostToolBatch => {
            SessionState::Busy
        }

        // Compaction happens inside a turn.
        HookEvent::PreCompact => SessionState::Busy,

        // A subagent finished; the turn has not. See the doc comment.
        HookEvent::SubagentStop => return None,

        /*
         * The turn ended — and *how* it ended is the whole of the finished-work notification.
         *
         * `Stop` after work is the CLI handing the conversation back: it is not merely not-busy,
         * it is waiting on a human, which is what `AwaitingInput` means and what nothing in this
         * workspace produced until now. `AwaitingInput` was defined, handled by the frontend as
         * "waiting on the user", and emitted by no code path at all.
         *
         * That gap is why the notification kept not firing. The webview inferred "finished" by
         * remembering whether it had ever *seen* this session go `Busy` — a per-window bit that
         * `Idle` alone cannot supply. But whether a window sees the `Busy` depends on what the
         * CLI happened to send and on this machine's own dedupe: a session whose first observed
         * hook is a `Stop` goes `Spawning -> Idle` with no `Busy` between, and the bit stays
         * false for ever. Measured on a live instance: nine transitions, eight of them straight
         * to `Idle`, and exactly one session showed `Idle -> Busy -> Idle` — that one, and only
         * that one, raised the marker.
         *
         * Deciding it here removes the inference. This end knows `current`, so `Busy -> Stop` is
         * unambiguous, and the answer is the same in every window because it is on the wire
         * rather than reconstructed from history each window may or may not have witnessed.
         *
         * A `Stop` from anywhere else really is just idle: a session that has not been working
         * has not finished anything, and announcing it is the noise `awaitingRule` was written
         * to avoid.
         */
        HookEvent::Stop => {
            if current == SessionState::Busy {
                SessionState::AwaitingInput
            } else {
                SessionState::Idle
            }
        }

        // Handled by the caller, which can see the payload: only a permission request means
        // `AwaitingPermission`, and an ordinary notification means nothing about liveness.
        HookEvent::Notification => return None,

        // The exit code is not in the hook payload; the PTY reaper supplies it. Reporting 0
        // here would claim a clean exit for a session that may have crashed.
        HookEvent::SessionEnd => return None,
    };

    if next == current { None } else { Some(next) }
}

/// Whether a `Notification` payload is a permission request.
///
/// The CLI sends `Notification` for several things — idle nudges, permission prompts — and
/// only one of them blocks the turn on a human. There is no typed field for this, so the
/// message text is what there is to go on; a false negative costs a confirmation prompt that
/// does not fire, which is better than a false positive that fires on every notification.
pub fn is_permission_request(payload: &serde_json::Value) -> bool {
    let Some(message) = payload.get("message").and_then(|m| m.as_str()) else {
        return false;
    };
    let lowered = message.to_ascii_lowercase();
    lowered.contains("permission") || lowered.contains("approve") || lowered.contains("allow")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_prompt_starts_a_turn_and_stop_ends_it() {
        let s = SessionState::Idle;
        let s = next_state(s, HookEvent::UserPromptSubmit).expect("idle -> busy");
        assert_eq!(s, SessionState::Busy);
        assert!(s.is_live());

        // `AwaitingInput`, not `Idle`, and this test used to pin the weaker answer.
        //
        // A `Stop` that ends work is the CLI handing the conversation back — waiting on a human,
        // which is exactly what `AwaitingInput` means. Saying `Idle` here left the webview to
        // infer "finished" from whether it had happened to witness the `Busy`, and a window that
        // had not could never raise the marker. `AwaitingInput` was defined and handled and
        // produced by nothing at all until this arm.
        let s = next_state(s, HookEvent::Stop).expect("busy -> awaiting input");
        assert_eq!(s, SessionState::AwaitingInput);
        assert!(
            !s.is_live(),
            "a finished turn is not work in flight, so it must not prompt on close"
        );

        // ...and a `Stop` with no work behind it is still just idle. A session that has not been
        // asked anything has not finished anything, and announcing it is the noise the whole
        // `ranATurn` idea existed to avoid.
        assert_eq!(
            next_state(SessionState::Idle, HookEvent::Stop),
            None,
            "idle -> Stop moves nothing"
        );
        assert_eq!(
            next_state(SessionState::Spawning, HookEvent::Stop),
            Some(SessionState::Idle),
            "a session whose first observed hook is a Stop has finished nothing"
        );
    }

    #[test]
    fn a_subagent_finishing_does_not_end_the_turn() {
        // The regression this test exists for: treating SubagentStop as the end marks a
        // session idle in the middle of a multi-agent task, so closing the project mid-work
        // would not warn — the exact moment the warning matters most.
        assert_eq!(
            next_state(SessionState::Busy, HookEvent::SubagentStop),
            None
        );
        assert!(SessionState::Busy.is_live());
    }

    #[test]
    fn tool_traffic_keeps_a_turn_busy_and_corrects_a_wrong_idle() {
        assert_eq!(next_state(SessionState::Busy, HookEvent::PreToolUse), None);
        assert_eq!(
            next_state(SessionState::Idle, HookEvent::PostToolUse),
            Some(SessionState::Busy),
            "tool traffic on an idle session means the state was wrong"
        );
        assert_eq!(
            next_state(SessionState::Idle, HookEvent::PostToolBatch),
            Some(SessionState::Busy)
        );
    }

    #[test]
    fn compaction_is_part_of_a_turn() {
        assert_eq!(
            next_state(SessionState::Idle, HookEvent::PreCompact),
            Some(SessionState::Busy)
        );
    }

    #[test]
    fn session_end_does_not_invent_an_exit_code() {
        // The code comes from the PTY reaper, which knows it. Claiming 0 here would report a
        // clean exit for a session that crashed.
        assert_eq!(next_state(SessionState::Busy, HookEvent::SessionEnd), None);
    }

    #[test]
    fn only_a_permission_notification_counts_as_one() {
        assert!(is_permission_request(
            &json!({"message": "Claude needs your permission to use Bash"})
        ));
        assert!(is_permission_request(
            &json!({"message": "Allow this edit?"})
        ));
        assert!(!is_permission_request(
            &json!({"message": "Claude is waiting for your input"})
        ));
        assert!(!is_permission_request(&json!({})));
    }

    #[test]
    fn a_no_op_transition_reports_no_change() {
        // So a caller can skip an event emission rather than broadcasting a revision that
        // changed nothing — the status bar should not repaint on every tool call.
        assert_eq!(next_state(SessionState::Idle, HookEvent::Stop), None);
        assert_eq!(
            next_state(SessionState::Busy, HookEvent::UserPromptSubmit),
            None
        );
    }
}
