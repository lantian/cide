//! Review snapshots are shared by the model's read-only status tool and attention policy.
use super::*;
use cide_ipc::{PeerChatReviewState as Phase, PeerChatReviewStatus, PeerChatReviews, SessionState};

fn for_pair(
    app: &AppHandle,
    tab: TabId,
    pair: (SessionId, SessionId),
    paused: bool,
) -> PeerChatReviewStatus {
    let states = crate::emit::last_session_states();
    let state = |id| {
        if app
            .state::<SessionRegistry>()
            .get(id)
            .is_none_or(|s| s.has_exited())
        {
            SessionState::Exited { code: -1 }
        } else {
            states.get(&id).copied().unwrap_or(SessionState::Spawning)
        }
    };
    domain::review::status(
        tab,
        pair,
        app.state::<Runtime>().reviews.lock().current.get(&tab),
        (state(pair.0), state(pair.1)),
        paused,
    )
}

fn collect(app: &AppHandle) -> Vec<PeerChatReviewStatus> {
    let ws = app.state::<WorkspaceState>().snapshot();
    let mut result = Vec::new();
    for project in ws.projects.values() {
        for tab in &project.tabs {
            let Some(chat) = &tab.peer_chat else { continue };
            let (Some(writer), Some(reviewer)) = (
                tab.tree.panes.get(&chat.main).and_then(|p| p.session),
                tab.tree.panes.get(&chat.peer).and_then(|p| p.session),
            ) else {
                continue;
            };
            let status = for_pair(
                app,
                tab.id,
                (writer, reviewer),
                chat.paused || chat.starting,
            );
            if status.request_id.is_some() {
                result.push(status);
            }
        }
    }
    result.sort_by_key(|r| r.tab);
    result
}

/// Read-only: never consumes a response, marks a review complete or sends a prompt.
pub(crate) fn status(
    app: &AppHandle,
    sender: SessionId,
    request: Option<&str>,
) -> Result<PeerChatReviewStatus, String> {
    let (tab, pair, paused) = app.state::<WorkspaceState>().with(|ws| {
        let (project, tab, pane, chat) =
            domain::for_session(ws, sender).ok_or("This session is no longer paired")?;
        if pane != chat.main {
            return Err("Review status is available only to the paired writer");
        }
        let reviewer = workspace::tab(ws, project, tab)
            .ok()
            .and_then(|t| t.tree.panes.get(&chat.peer))
            .and_then(|p| p.session)
            .ok_or("The reviewer has not started")?;
        Ok((tab, (sender, reviewer), chat.paused || chat.starting))
    })?;
    // Native turn/start may begin executing the writer before its acceptance response reaches
    // the delivery thread. Wait only for that in-flight acknowledgement, not for review work:
    // otherwise the writer could yield from the very reply that was supposed to wake it.
    await_submission(&app.state::<Runtime>(), tab, sender, Duration::from_secs(7));
    let result = for_pair(app, tab, pair, paused);
    if request.is_some_and(|id| result.request_id.as_deref() != Some(id)) {
        return Err("Unknown or obsolete request_id for this pair".into());
    }
    Ok(result)
}

fn await_submission(runtime: &Runtime, tab: TabId, sender: SessionId, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mut book = runtime.reviews.lock();
    while book
        .current
        .get(&tab)
        .is_some_and(|r| r.writer == sender && r.state == Phase::ReplyQueued && r.reply_submitting)
    {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        runtime.review_changed.wait_for(&mut book, deadline - now);
    }
    drop(book);
}

pub(crate) fn quiet_writer(app: &AppHandle, session: SessionId) -> bool {
    if matches!(
        crate::emit::last_session_states().get(&session),
        Some(SessionState::AwaitingPermission | SessionState::Paused | SessionState::Exited { .. })
    ) {
        return false;
    }

    app.try_state::<Runtime>().is_some_and(|r| {
        r.review_snapshot.lock().reviews.iter().any(|s| {
            s.writer == session
                && domain::review::pending(s.state)
                && s.blocked_reason.is_none()
                && !matches!(
                    s.writer_state,
                    SessionState::AwaitingPermission
                        | SessionState::Paused
                        | SessionState::Exited { .. }
                )
        })
    })
}

/// Both events and initial pulls use the same versioned snapshot. No workspace or raw
/// session state is rewritten: those states still drive delivery and native permissions.
pub(crate) fn refresh_reviews(app: &AppHandle) {
    let Some(runtime) = app.try_state::<Runtime>() else {
        return;
    };
    let Some(_) = app.try_state::<WorkspaceState>() else {
        return;
    };
    // Serialize collection as well as version assignment. A slow older collector must not
    // overwrite a newer snapshot. Membership moves under this lock; title/event announcements
    // happen after releasing it, so readers can safely inspect the current review snapshot.
    let (snapshot, attention_changed) = {
        let mut held = runtime.review_snapshot.lock();
        let reviews = collect(app);
        if held.reviews == reviews {
            return;
        }
        held.revision += 1;
        held.reviews = reviews;
        // Apply attention membership under this lock as well. An older blocked snapshot
        // must not re-raise a marker after a newer busy/completed snapshot has cleared it.
        // Announcing titles/events happens outside the lock and reads the current set.
        let mut changed = false;
        for review in &held.reviews {
            if let Some(awaiting) = domain::review::attention(review) {
                changed |= crate::windows::set_awaiting(review.writer, awaiting);
            }
        }
        (held.clone(), changed)
    };
    crate::emit::peer_chat_reviews(app, &snapshot);
    if attention_changed {
        crate::cmd::window::awaiting_changed(app, &app.state::<WorkspaceState>());
    }
}

pub(crate) fn review_snapshot(app: &AppHandle) -> PeerChatReviews {
    refresh_reviews(app);
    app.state::<Runtime>().review_snapshot.lock().clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepted_reply_wakes_the_status_reader_without_replaying_a_prompt() {
        let runtime = Arc::new(Runtime::default());
        let tab = TabId::new();
        let pair = (SessionId::new(), SessionId::new());
        let request = Input {
            message: "Review changes".into(),
            kind: Kind::Request,
            request_id: None,
        };
        let reply = Input {
            message: "No issues found".into(),
            kind: Kind::Reply,
            request_id: Some("request".into()),
        };
        runtime
            .reviews
            .lock()
            .apply(tab, pair, pair.0, "request", &request)
            .unwrap();
        // Waiting on the reviewer never parks the status tool.
        await_submission(&runtime, tab, pair.0, Duration::ZERO);
        assert_eq!(
            runtime.reviews.lock().current[&tab].state,
            Phase::RequestQueued
        );
        runtime
            .reviews
            .lock()
            .apply(tab, pair, pair.1, "reply", &reply)
            .unwrap();
        let message = Message {
            id: "reply".into(),
            project: ProjectId::new(),
            tab,
            sender: pair.1,
            pane: PaneId::new(),
            target: pair.0,
            body: reply.message,
            generation: 0,
            kind: Kind::Reply,
            request_id: Some("request".into()),
        };
        runtime.receipt(&message, "writing", "submitting");
        let delivering = runtime.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));
            delivering.receipt(&message, "delivered", "accepted");
        });
        await_submission(&runtime, tab, pair.0, Duration::from_secs(2));
        assert_eq!(runtime.reviews.lock().current[&tab].state, Phase::Completed);
        handle.join().unwrap();
        assert!(
            runtime.lanes.is_empty(),
            "reading status never requeues a reply"
        );
        assert_eq!(runtime.receipts(tab).len(), 1);
    }
}
