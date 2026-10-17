//! The review protocol, independent of process readiness and message transport.
//! One current request per pair prevents uncorrelated acknowledgements or an old reply
//! from releasing the writer. Records live outside workspace persistence, just like queues.
use cide_ipc::{
    PeerChatReviewState as State, PeerChatReviewStatus, SessionId, SessionState, TabId,
};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    #[default]
    Note,
    Request,
    Progress,
    Reply,
    Cancel,
    Skip,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub message: String,
    #[serde(default)]
    pub kind: Kind,
    pub request_id: Option<String>,
}

pub fn pending(state: State) -> bool {
    matches!(
        state,
        State::RequestQueued | State::AwaitingReply | State::ReplyQueued
    )
}

#[derive(Debug, Clone)]
pub struct Review {
    pub tab: TabId,
    pub writer: SessionId,
    pub reviewer: SessionId,
    pub request_id: String,
    pub state: State,
    pub detail: Option<String>,
    reply_id: Option<String>,
    pub reply_submitting: bool,
}

#[derive(Default)]
pub struct Book {
    pub current: HashMap<TabId, Review>,
}

impl Book {
    /// Called while enqueueing under the recipient lane's lock. Failed validation must
    /// neither change review state nor send a message. Caller identity is server-bound.
    pub fn apply(
        &mut self,
        tab: TabId,
        pair: (SessionId, SessionId),
        sender: SessionId,
        id: &str,
        input: &Input,
    ) -> Result<(), String> {
        let (writer, reviewer) = pair;
        if input.kind == Kind::Note {
            return if input.request_id.is_none() {
                Ok(())
            } else {
                Err(
                    "Notes do not take request_id; use progress or reply for a review response"
                        .into(),
                )
            };
        }
        let writer_operation = matches!(input.kind, Kind::Request | Kind::Cancel | Kind::Skip);
        if sender != if writer_operation { writer } else { reviewer } {
            return Err("Only the writer requests/cancels/skips review; only the reviewer sends progress or a completed reply".into());
        }
        if input.kind == Kind::Request {
            if input.request_id.is_some() {
                return Err("A request creates a new request_id; do not supply one".into());
            }
            if self
                .current
                .get(&tab)
                .is_some_and(|r| r.writer == writer && r.reviewer == reviewer && pending(r.state))
            {
                return Err("A review is already open. Check cide_peer_chat_status; send notes for context, or explicitly cancel it before starting another review".into());
            }
            self.current.insert(
                tab,
                Review {
                    tab,
                    writer,
                    reviewer,
                    request_id: id.into(),
                    state: State::RequestQueued,
                    detail: None,
                    reply_id: None,
                    reply_submitting: false,
                },
            );
            return Ok(());
        }
        let r = self
            .current
            .get_mut(&tab)
            .filter(|r| {
                r.writer == writer
                    && r.reviewer == reviewer
                    && input.request_id.as_deref() == Some(&r.request_id)
            })
            .ok_or("Unknown or obsolete request_id for this pair")?;
        if matches!(input.kind, Kind::Progress | Kind::Reply)
            && !matches!(r.state, State::RequestQueued | State::AwaitingReply)
        {
            return Err(
                "This review already has a final reply or was interrupted; do not reply again"
                    .into(),
            );
        }
        match input.kind {
            Kind::Reply => {
                r.state = State::ReplyQueued;
                r.reply_id = Some(id.into());
            }
            Kind::Cancel | Kind::Skip => {
                r.state = if input.kind == Kind::Skip {
                    State::Skipped
                } else {
                    State::Cancelled
                };
                r.detail = Some(input.message.clone());
            }
            Kind::Progress => {}
            _ => unreachable!(),
        }
        Ok(())
    }

    /// Check at every delivery boundary, including inside the native Codex forwarder.
    pub fn valid(&self, tab: TabId, request: &str, kind: Kind, message_id: &str) -> bool {
        self.current.get(&tab).is_some_and(|r| {
            r.request_id == request
                && pending(r.state)
                && (kind != Kind::Reply || r.reply_id.as_deref() == Some(message_id))
        })
    }

    pub fn receipt(
        &mut self,
        tab: TabId,
        request: &str,
        kind: Kind,
        id: &str,
        status: &str,
        detail: &str,
    ) {
        let Some(r) = self
            .current
            .get_mut(&tab)
            .filter(|r| r.request_id == request && pending(r.state))
        else {
            return;
        };
        if kind == Kind::Reply && r.reply_id.as_deref() != Some(id) {
            return;
        }
        if kind == Kind::Reply {
            r.reply_submitting = status == "writing";
        }
        match status {
            "delivered" if kind == Kind::Request && r.state == State::RequestQueued => {
                r.state = State::AwaitingReply
            }
            "delivered" if kind == Kind::Reply => r.state = State::Completed,
            "failed" | "uncertain" | "cancelled" => {
                r.state = match status {
                    "uncertain" => State::Uncertain,
                    "cancelled" => State::Cancelled,
                    _ => State::Failed,
                };
                r.detail = Some(detail.into());
            }
            _ => {}
        }
    }

    pub fn cancel(&mut self, sessions: &[SessionId]) {
        for r in self
            .current
            .values_mut()
            .filter(|r| sessions.contains(&r.writer) || sessions.contains(&r.reviewer))
        {
            if !matches!(r.state, State::Failed | State::Uncertain | State::Cancelled) {
                r.state = State::Cancelled;
                r.detail = Some("The pair paused, changed conversation, restarted or closed. Resume and request review again; previous messages are not replayed.".into());
            }
        }
    }
}

/// A snapshot never infers review completion from runtime idle/busy or from a receipt alone.
pub fn status(
    tab: TabId,
    pair: (SessionId, SessionId),
    current: Option<&Review>,
    states: (SessionState, SessionState),
    paused: bool,
) -> PeerChatReviewStatus {
    let mut status = PeerChatReviewStatus {
        tab,
        writer: pair.0,
        reviewer: pair.1,
        request_id: None,
        state: State::NotRequested,
        writer_state: states.0,
        reviewer_state: states.1,
        blocked_reason: None,
        ready_to_finalize: false,
    };
    if let Some(r) = current.filter(|r| (r.writer, r.reviewer) == pair) {
        status.request_id = Some(r.request_id.clone());
        status.state = r.state;
        status.blocked_reason = r.detail.clone();
    }
    if paused {
        status.blocked_reason = Some("The pair is paused. Resume it and request review again if delivery was cancelled; do not finalize or blindly replay messages.".into());
    } else if matches!(status.reviewer_state, SessionState::Exited { .. }) {
        status.blocked_reason =
            Some("The reviewer exited. Restore the pair and request review again.".into());
    } else if pending(status.state) {
        match status.reviewer_state {
            SessionState::AwaitingPermission => status.blocked_reason = Some("The reviewer needs permission. Ask the user to handle its native permission controls; do not answer them yourself.".into()),
            SessionState::Paused => status.blocked_reason = Some("The reviewer is paused. Resume it before completing review.".into()),
            _ => {}
        }
    }
    status.ready_to_finalize = matches!(status.state, State::Completed | State::Skipped)
        && (status.blocked_reason.is_none() || status.state == State::Skipped);
    // A skip reason documents the user's decision, not a blockage.
    if status.state == State::Skipped {
        status.blocked_reason = None;
    }
    status
}

/// A yielded writer waits on its reviewer; an actionable blockage waits on the user.
/// Busy/exited writers clear any previous review alert. Permission prompts are left to
/// the native state/acknowledgement fold rather than raised repeatedly by snapshots.
pub fn attention(review: &PeerChatReviewStatus) -> Option<bool> {
    match review.writer_state {
        SessionState::Busy
        | SessionState::Spawning
        | SessionState::Splash
        | SessionState::Paused
        | SessionState::Exited { .. } => Some(false),
        SessionState::AwaitingPermission => None,
        SessionState::Idle | SessionState::AwaitingInput => {
            if review.blocked_reason.is_some() {
                Some(true)
            } else if pending(review.state) {
                Some(false)
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(kind: Kind, id: Option<&str>) -> Input {
        Input {
            message: "Findings or context".into(),
            kind,
            request_id: id.map(str::to_owned),
        }
    }
    #[test]
    fn only_a_correlated_final_reply_delivered_to_the_writer_completes_review() {
        let mut book = Book::default();
        let tab = TabId::new();
        let pair = (SessionId::new(), SessionId::new());
        book.apply(tab, pair, pair.0, "r1", &input(Kind::Request, None))
            .unwrap();
        book.receipt(tab, "r1", Kind::Request, "r1", "delivered", "accepted");
        assert_eq!(book.current[&tab].state, State::AwaitingReply);
        book.apply(tab, pair, pair.1, "p", &input(Kind::Progress, Some("r1")))
            .unwrap();
        book.receipt(tab, "r1", Kind::Progress, "p", "delivered", "progress");
        assert_eq!(book.current[&tab].state, State::AwaitingReply);
        assert!(
            book.apply(tab, pair, pair.1, "bad", &input(Kind::Reply, Some("old")))
                .is_err()
        );
        assert!(
            book.apply(tab, pair, pair.0, "bad", &input(Kind::Reply, Some("r1")))
                .is_err()
        );
        book.apply(tab, pair, pair.1, "answer", &input(Kind::Reply, Some("r1")))
            .unwrap();
        assert_eq!(book.current[&tab].state, State::ReplyQueued);
        assert!(
            book.apply(
                tab,
                pair,
                pair.1,
                "duplicate",
                &input(Kind::Reply, Some("r1"))
            )
            .is_err()
        );
        book.receipt(
            tab,
            "r1",
            Kind::Request,
            "r1",
            "delivered",
            "late request receipt",
        );
        assert_eq!(book.current[&tab].state, State::ReplyQueued);
        book.receipt(tab, "r1", Kind::Reply, "answer", "delivered", "accepted");
        assert_eq!(book.current[&tab].state, State::Completed);
    }
    #[test]
    fn cancellation_skip_replacement_and_failed_delivery_never_accept_old_replies() {
        let mut book = Book::default();
        let tab = TabId::new();
        let pair = (SessionId::new(), SessionId::new());
        book.apply(tab, pair, pair.0, "r1", &input(Kind::Request, None))
            .unwrap();
        assert!(
            book.apply(tab, pair, pair.0, "r2", &input(Kind::Request, None))
                .is_err()
        );
        assert!(
            book.apply(tab, pair, pair.1, "skip", &input(Kind::Skip, Some("r1")))
                .is_err()
        );
        book.apply(tab, pair, pair.0, "skip", &input(Kind::Skip, Some("r1")))
            .unwrap();
        assert!(!book.valid(tab, "r1", Kind::Request, "r1"));
        book.apply(tab, pair, pair.0, "r2", &input(Kind::Request, None))
            .unwrap();
        assert!(
            book.apply(tab, pair, pair.1, "old", &input(Kind::Reply, Some("r1")))
                .is_err()
        );
        book.receipt(tab, "r1", Kind::Reply, "old", "delivered", "old receipt");
        assert_eq!(book.current[&tab].state, State::RequestQueued);
        book.receipt(tab, "r2", Kind::Request, "r2", "uncertain", "Do not resend");
        assert_eq!(book.current[&tab].state, State::Uncertain);
        book.cancel(&[pair.0]);
        assert_eq!(book.current[&tab].state, State::Uncertain);
        book.apply(tab, pair, pair.0, "r3", &input(Kind::Request, None))
            .unwrap();
        book.cancel(&[pair.1]);
        assert_eq!(book.current[&tab].state, State::Cancelled);
        assert!(
            book.apply(
                tab,
                (pair.0, SessionId::new()),
                pair.0,
                "skip",
                &input(Kind::Skip, Some("r3"))
            )
            .is_err()
        );
    }
    #[test]
    fn legacy_notes_and_strict_message_arguments_remain_compatible() {
        let parsed: Input = serde_json::from_str(r#"{"message":"note"}"#).unwrap();
        assert_eq!(parsed.kind, Kind::Note);
        assert!(serde_json::from_str::<Input>(r#"{"message":"note","target":"other"}"#).is_err());
    }

    #[test]
    fn runtime_readiness_never_substitutes_for_review_completion_and_skip_is_explicit() {
        let tab = TabId::new();
        let pair = (SessionId::new(), SessionId::new());
        let mut book = Book::default();
        assert!(
            !status(
                tab,
                pair,
                None,
                (SessionState::Busy, SessionState::Idle),
                false
            )
            .ready_to_finalize
        );
        book.apply(tab, pair, pair.0, "review", &input(Kind::Request, None))
            .unwrap();
        for phase in [
            SessionState::Busy,
            SessionState::Idle,
            SessionState::AwaitingInput,
            SessionState::AwaitingPermission,
            SessionState::Paused,
            SessionState::Exited { code: -1 },
        ] {
            let view = status(
                tab,
                pair,
                book.current.get(&tab),
                (SessionState::AwaitingInput, phase),
                false,
            );
            assert!(!view.ready_to_finalize);
            assert_eq!(
                view.blocked_reason.is_some(),
                matches!(
                    phase,
                    SessionState::AwaitingPermission
                        | SessionState::Paused
                        | SessionState::Exited { .. }
                )
            );
        }
        let mut reply = input(Kind::Reply, Some("review"));
        reply.message = "No issues found. Validation and limitations checked.".into();
        book.apply(tab, pair, pair.1, "reply", &reply).unwrap();
        assert!(
            !status(
                tab,
                pair,
                book.current.get(&tab),
                (SessionState::Busy, SessionState::Idle),
                false
            )
            .ready_to_finalize
        );
        book.receipt(tab, "review", Kind::Reply, "reply", "delivered", "accepted");
        assert!(
            status(
                tab,
                pair,
                book.current.get(&tab),
                (SessionState::Busy, SessionState::Busy),
                false
            )
            .ready_to_finalize
        );
        assert!(
            !status(
                tab,
                pair,
                book.current.get(&tab),
                (SessionState::Busy, SessionState::Idle),
                true
            )
            .ready_to_finalize
        );
        book.cancel(&[pair.0]);
        assert!(
            !status(
                tab,
                pair,
                book.current.get(&tab),
                (SessionState::Busy, SessionState::Idle),
                false
            )
            .ready_to_finalize
        );
        book.apply(
            tab,
            pair,
            pair.0,
            "skip",
            &input(Kind::Skip, Some("review")),
        )
        .unwrap();
        assert!(
            status(
                tab,
                pair,
                book.current.get(&tab),
                (SessionState::Busy, SessionState::Exited { code: -1 }),
                true
            )
            .ready_to_finalize
        );
        assert!(
            !status(
                tab,
                (pair.0, SessionId::new()),
                book.current.get(&tab),
                (SessionState::Busy, SessionState::Idle),
                false
            )
            .ready_to_finalize
        );
    }

    #[test]
    fn review_attention_clears_a_yield_and_notifies_only_actionable_blockages() {
        let tab = TabId::new();
        let pair = (SessionId::new(), SessionId::new());
        let mut book = Book::default();
        book.apply(tab, pair, pair.0, "request", &input(Kind::Request, None))
            .unwrap();
        let view =
            |writer, reviewer| status(tab, pair, book.current.get(&tab), (writer, reviewer), false);
        assert_eq!(
            attention(&view(SessionState::AwaitingInput, SessionState::Busy)),
            Some(false)
        );
        assert_eq!(
            attention(&view(
                SessionState::AwaitingInput,
                SessionState::AwaitingPermission
            )),
            Some(true)
        );
        assert_eq!(
            attention(&view(SessionState::Busy, SessionState::AwaitingPermission)),
            Some(false)
        );
        assert_eq!(
            attention(&view(SessionState::AwaitingPermission, SessionState::Busy)),
            None
        );
        assert_eq!(
            attention(&view(
                SessionState::Exited { code: 0 },
                SessionState::AwaitingPermission
            )),
            Some(false)
        );
    }
}
