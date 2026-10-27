//! Durable decisions and delivery receipts. No task tracker or process ownership lives here.
use crate::{CoreError, Result, persist};
use cide_ipc::{
    Acceptance, TaskRow, TaskStatus, UserRequest, UserRequestKind, UserRequestResponse,
    UserRequestsView,
};
use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

/// Count decisions still owed by the user, keeping delivery-pending task links deduplicated.
pub fn waiting_count(requests: &[UserRequest], tasks: &[TaskRow]) -> usize {
    let pending: Vec<_> = requests.iter().filter(|r| !r.delivered).collect();
    pending.iter().filter(|r| r.response.is_none()).count()
        + tasks
            .iter()
            .filter(|t| {
                (t.question.is_some()
                    || (t.acceptance == Some(Acceptance::User) && t.status == TaskStatus::Review))
                    && !pending.iter().any(|r| {
                        r.task.as_ref() == Some(&t.id)
                            && matches!(r.request, UserRequestKind::Question { .. })
                    })
            })
            .count()
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct File {
    rev: u64,
    projects: BTreeMap<String, Vec<UserRequest>>,
    /// Persist before sending: after a crash, reconcile the original transcript rather
    /// than blindly submitting the same answer a second time. Older profiles have none.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    delivery_attempts: BTreeSet<String>,
    /// An adapter lost its pending call. A later *native pending event*, with the
    /// same identity, may reopen it; ordinary chat delivery must never do so.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    retired_native: BTreeMap<String, bool>,
    /// A tool reply can finish after its model turn has ended (Codex exec cells).
    /// These are provisional receipts, never permission to replay a submitted turn.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    tool_replies: BTreeMap<String, BTreeSet<Option<String>>>,
}

pub enum AnswerSubmission {
    /// Nothing was sent; a busy receiver or a new blocking waiter can be retried.
    Waiting,
    /// The harness accepted a user turn in the original conversation.
    Confirmed,
    /// Input was submitted, but the transcript must confirm receipt.
    Submitted,
    /// The transport confirms that it wrote nothing. Safe to retry at a later boundary.
    NotSent(String),
}

/// The app supplies transport and transcript reads; the durable receipt belongs here.
pub trait AnswerTarget {
    fn received(&self, request: &UserRequest) -> Result<bool>;
    /// Call `begin` immediately before sending. A false claim means another delivery or
    /// blocking waiter owns this answer; no input may be written in that case.
    fn submit(
        &self,
        request: &UserRequest,
        begin: &dyn Fn() -> Result<bool>,
    ) -> Result<AnswerSubmission>;
}

pub struct Requests {
    path: PathBuf,
    file: Mutex<File>,
    changed: Condvar,
    /// Leases last through the socket write, rather than just through the wait.
    waiters: Mutex<HashMap<String, BTreeSet<uuid::Uuid>>>,
}

/// One call owns one lease through the socket write. Dropping an error, cancelled
/// call, or unwritten reply cannot release a different call's wait on the same id.
pub struct WaitedRequest {
    pub request: UserRequest,
    store: Arc<Requests>,
    token: uuid::Uuid,
}

impl std::ops::Deref for WaitedRequest {
    type Target = UserRequest;
    fn deref(&self) -> &UserRequest {
        &self.request
    }
}

impl Drop for WaitedRequest {
    fn drop(&mut self) {
        let mut waiters = self.store.waiters.lock();
        if let Some(tokens) = waiters.get_mut(&self.request.id) {
            tokens.remove(&self.token);
            if tokens.is_empty() {
                waiters.remove(&self.request.id);
            }
        }
    }
}

impl Requests {
    pub fn load(path: PathBuf) -> Self {
        Self {
            file: Mutex::new(persist::read_or_quarantine(&path)),
            path,
            changed: Condvar::new(),
            waiters: Mutex::new(HashMap::new()),
        }
    }
    fn update<T>(&self, f: impl FnOnce(&mut File) -> Result<T>) -> Result<T> {
        let mut file = self.file.lock();
        let before = file.clone();
        let outcome = f(&mut file);
        if outcome.is_err() {
            *file = before;
            return outcome;
        }
        file.rev += 1;
        let saved = serde_json::to_vec(&*file)
            .map_err(|e| CoreError::Io(e.to_string()))
            .and_then(|bytes| persist::write_atomic(&self.path, &bytes));
        if let Err(error) = saved {
            *file = before;
            return Err(error);
        }
        self.changed.notify_all();
        outcome
    }
    pub fn view(&self, root: &Path) -> UserRequestsView {
        let file = self.file.lock();
        UserRequestsView {
            rev: file.rev,
            requests: file
                .projects
                .get(&root.to_string_lossy().into_owned())
                .cloned()
                .unwrap_or_default(),
        }
    }
    pub fn get(&self, root: &Path, id: &str) -> Result<UserRequest> {
        self.view(root)
            .requests
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| CoreError::Io("This request is not in this project.".into()))
    }
    pub fn create(&self, root: &Path, request: UserRequest) -> Result<UserRequest> {
        self.update(|file| {
            let rows = file
                .projects
                .entry(root.to_string_lossy().into_owned())
                .or_default();
            // A retry after a transport loss must not create another decision.
            if let Some(index) = rows.iter().position(|r| {
                if let (
                    UserRequestKind::Questions { source: old, .. },
                    UserRequestKind::Questions { source: new, .. },
                ) = (&r.request, &request.request)
                {
                    return old == new;
                }
                (r.origin.id == request.origin.id
                    || (r.origin.run.is_some() && r.origin.run == request.origin.run)
                    || (r.origin.session.is_some() && r.origin.session == request.origin.session))
                    && r.response.is_none()
                    && r.request == request.request
                    && r.task == request.task
            }) {
                let existing = &mut rows[index];
                if matches!(existing.request, UserRequestKind::Questions { .. })
                    && existing.request != request.request
                {
                    return Err(CoreError::Io(
                        "The native question changed after it was displayed.".into(),
                    ));
                }
                if matches!(existing.request, UserRequestKind::Questions { .. })
                    && let Some(unanswered) = file.retired_native.remove(&existing.id)
                {
                    existing.delivered = false;
                    existing.delivery_error = None;
                    if unanswered {
                        existing.response = None;
                    }
                    file.delivery_attempts.remove(&existing.id);
                }
                return Ok(existing.clone());
            }
            rows.push(request.clone());
            Ok(request)
        })
    }
    pub fn decide(
        &self,
        root: &Path,
        id: &str,
        response: UserRequestResponse,
    ) -> Result<UserRequest> {
        self.change(root, id, |r| {
            if r.response.is_some() || r.delivered {
                return Err(CoreError::Io(
                    "This request has already received a decision.".into(),
                ));
            }
            validate_response(&r.request, &response)?;
            r.applying = matches!(response, UserRequestResponse::Approve { .. })
                || (r.task.is_some() && matches!(r.request, UserRequestKind::Question { .. }));
            r.response = Some(response);
            Ok(())
        })
    }
    pub fn complete_application(&self, root: &Path, id: &str) -> Result<UserRequest> {
        self.change(root, id, |r| {
            r.applying = false;
            Ok(())
        })
    }
    pub fn reset_application(&self, root: &Path, id: &str, error: String) -> Result<UserRequest> {
        self.change(root, id, |r| {
            r.applying = false;
            r.response = None;
            r.delivery_error = Some(error);
            Ok(())
        })
    }
    pub fn resolve_origin(
        &self,
        root: &Path,
        id: &str,
        origin: cide_ipc::sessions::SessionRecord,
    ) -> Result<UserRequest> {
        self.change(root, id, |r| {
            if !r.origin.known {
                r.origin = origin;
            }
            Ok(())
        })
    }
    pub fn delivered(&self, root: &Path, id: &str) -> Result<UserRequest> {
        self.update(|file| {
            let r = request_mut(file, root, id)?;
            r.delivered = true;
            r.delivery_error = None;
            let request = r.clone();
            file.delivery_attempts.remove(id);
            file.retired_native.remove(id);
            file.tool_replies.remove(id);
            Ok(request)
        })
    }

    /// Retire a saved decision from Waiting without losing its answer. Like native
    /// retirement, this ends delivery retries; it does not recall input already sent.
    pub fn dismiss(&self, root: &Path, id: &str) -> Result<UserRequest> {
        self.update(|file| {
            let request = request_mut(file, root, id)?;
            if request.response.is_none() {
                return Err(CoreError::Io(
                    "Answer or cancel this request before removing it from Waiting.".into(),
                ));
            }
            if request.applying {
                return Err(CoreError::Io(
                    "Wait for the saved decision to finish applying before removing it.".into(),
                ));
            }
            request.delivered = true;
            request.delivery_error = None;
            let request = request.clone();
            file.delivery_attempts.remove(id);
            // A repeated native event must not reopen an explicitly dismissed call.
            file.retired_native.remove(id);
            file.tool_replies.remove(id);
            Ok(request)
        })
    }

    /// Save before returning the tool result. A successful socket write alone does
    /// not prove the model consumed it; reconcile a model-visible output instead.
    pub fn tool_reply(&self, root: &Path, id: &str, turn: Option<String>) -> Result<()> {
        self.update(|file| {
            let request = request_mut(file, root, id)?;
            if !request.delivered && request.response.is_some() && !request.applying {
                file.tool_replies.entry(id.into()).or_default().insert(turn);
            }
            Ok(())
        })
    }

    pub fn tool_reply_turns(&self, id: &str) -> BTreeSet<Option<String>> {
        self.file
            .lock()
            .tool_replies
            .get(id)
            .cloned()
            .unwrap_or_default()
    }

    /// Complete only a confirmed delivery. An uncertain submission survives restart and
    /// is reconciled against the saved conversation before another send is considered.
    pub fn deliver_answer(&self, root: &Path, id: &str, target: &dyn AnswerTarget) -> Result<bool> {
        let request = self.get(root, id)?;
        if request.delivered {
            return Ok(true);
        }
        if request.response.is_none() || request.applying || self.has_waiter(id) {
            return Ok(false);
        }
        if target.received(&request)? {
            self.delivered(root, id)?;
            return Ok(true);
        }
        if self.file.lock().delivery_attempts.contains(id) {
            return Ok(false);
        }
        let claimed = std::sync::atomic::AtomicBool::new(false);
        let begin = || {
            if self.has_waiter(id) {
                return Ok(false);
            }
            let started = self.update(|file| {
                let request = request_mut(file, root, id)?;
                if request.delivered || request.response.is_none() || request.applying {
                    return Ok(false);
                }
                request.delivery_error = Some(
                    "The answer is saved. Waiting for the original conversation to confirm receipt.".into(),
                );
                Ok(file.delivery_attempts.insert(id.to_owned()))
            })?;
            claimed.store(started, std::sync::atomic::Ordering::Release);
            Ok(started)
        };
        match target.submit(&request, &begin)? {
            AnswerSubmission::Confirmed => {
                self.delivered(root, id)?;
                Ok(true)
            }
            AnswerSubmission::Submitted => Ok(false),
            outcome @ (AnswerSubmission::Waiting | AnswerSubmission::NotSent(_)) => {
                let error = match outcome {
                    AnswerSubmission::NotSent(error) => Some(error),
                    _ => None,
                };
                if claimed.load(std::sync::atomic::Ordering::Acquire) {
                    self.update(|file| {
                        request_mut(file, root, id)?.delivery_error = error;
                        file.delivery_attempts.remove(id);
                        Ok(())
                    })?;
                }
                Ok(false)
            }
        }
    }
    pub fn delivery_error(&self, root: &Path, id: &str, error: String) -> Result<UserRequest> {
        self.change(root, id, |r| {
            r.delivery_error = Some(error);
            Ok(())
        })
    }
    /// A native call disappeared or was answered in its own UI. Never turn its saved
    /// answer into a follow-up message, including after restarting the IDE.
    pub fn retire_native(&self, root: &Path, id: &str, reason: &str) -> Result<UserRequest> {
        self.update(|file| {
            let r = request_mut(file, root, id)?;
            if !matches!(r.request, UserRequestKind::Questions { .. }) {
                return Err(CoreError::Io("This is not a native question.".into()));
            }
            if r.delivered {
                return Ok(r.clone());
            }
            r.delivered = true;
            r.applying = false;
            r.delivery_error = Some(reason.into());
            let unanswered = r.response.is_none();
            if unanswered {
                r.response = Some(UserRequestResponse::Reject {
                    note: reason.into(),
                });
            }
            let request = r.clone();
            file.delivery_attempts.remove(id);
            file.retired_native.insert(id.into(), unanswered);
            Ok(request)
        })
    }
    pub fn claim_native(&self, root: &Path, id: &str) -> Result<bool> {
        if self.file.lock().delivery_attempts.contains(id) {
            return Ok(false);
        }
        self.update(|file| {
            let r = request_mut(file, root, id)?;
            if r.delivered
                || r.response.is_none()
                || !matches!(r.request, UserRequestKind::Questions { .. })
            {
                return Ok(false);
            }
            Ok(file.delivery_attempts.insert(id.to_owned()))
        })
    }
    fn change(
        &self,
        root: &Path,
        id: &str,
        f: impl FnOnce(&mut UserRequest) -> Result<()>,
    ) -> Result<UserRequest> {
        self.update(|file| {
            let r = file
                .projects
                .get_mut(&root.to_string_lossy().into_owned())
                .and_then(|rows| rows.iter_mut().find(|r| r.id == id))
                .ok_or_else(|| CoreError::Io("This request is no longer available.".into()))?;
            f(r)?;
            Ok(r.clone())
        })
    }
    pub fn has_waiter(&self, id: &str) -> bool {
        self.waiters
            .lock()
            .get(id)
            .is_some_and(|tokens| !tokens.is_empty())
    }
    /// Keep the returned guard until the reply is written, including on write failure.
    pub fn wait(
        self: &Arc<Self>,
        root: &Path,
        id: &str,
        cancelled: impl Fn() -> bool,
    ) -> Result<WaitedRequest> {
        let mut waited = WaitedRequest {
            request: self.get(root, id)?,
            store: Arc::clone(self),
            token: uuid::Uuid::new_v4(),
        };
        self.waiters
            .lock()
            .entry(id.to_owned())
            .or_default()
            .insert(waited.token);
        let mut file = self.file.lock();
        loop {
            if cancelled() {
                return Err(CoreError::Io(format!(
                    "Waiting interrupted. Request {id} is saved; reconnect with cide_user_request_result and wait=true before continuing dependent work."
                )));
            }
            let request = file
                .projects
                .get(&root.to_string_lossy().into_owned())
                .and_then(|rows| rows.iter().find(|r| r.id == id));
            match request {
                Some(r) if r.response.is_some() && !r.applying => {
                    waited.request = r.clone();
                    return Ok(waited);
                }
                Some(_) => {
                    self.changed.wait_for(&mut file, Duration::from_millis(200));
                }
                None => {
                    return Err(CoreError::Io("No such request.".into()));
                }
            }
        }
    }
}

fn request_mut<'a>(file: &'a mut File, root: &Path, id: &str) -> Result<&'a mut UserRequest> {
    file.projects
        .get_mut(&root.to_string_lossy().into_owned())
        .and_then(|rows| rows.iter_mut().find(|r| r.id == id))
        .ok_or_else(|| CoreError::Io("This request is no longer available.".into()))
}

fn validate_response(kind: &UserRequestKind, response: &UserRequestResponse) -> Result<()> {
    let invalid = |text: &str| CoreError::Io(text.to_owned());
    match (kind, response) {
        (_, UserRequestResponse::Cancel | UserRequestResponse::Reject { .. }) => Ok(()),
        (
            UserRequestKind::Question { question },
            UserRequestResponse::Answer { text, selected_ids },
        ) => question
            .validate_answer(selected_ids, text)
            .map_err(CoreError::Io),
        (
            UserRequestKind::Questions { questions, .. },
            UserRequestResponse::Answers { answers },
        ) => {
            if questions.len() != answers.len() {
                return Err(invalid("Answer every question before submitting."));
            }
            let mut seen = BTreeSet::new();
            for answer in answers {
                let question = questions
                    .iter()
                    .find(|q| q.id == answer.id)
                    .ok_or_else(|| invalid("Unknown question."))?;
                if !seen.insert(&answer.id) {
                    return Err(invalid("Answer each question once."));
                }
                question
                    .question
                    .validate_answer(&answer.selected_ids, &answer.text)
                    .map_err(CoreError::Io)?;
                if !question.custom && !answer.text.trim().is_empty() {
                    return Err(invalid("This question only accepts the offered choices."));
                }
            }
            Ok(())
        }
        (
            UserRequestKind::MoveTasks { tasks, .. },
            UserRequestResponse::Approve { selected_ids, .. },
        ) => {
            if selected_ids.is_empty()
                || selected_ids.iter().enumerate().any(|(i, id)| {
                    selected_ids[..i].contains(id) || !tasks.iter().any(|t| &t.id == id)
                })
            {
                return Err(invalid(
                    "Select at least one proposed task, without duplicates.",
                ));
            }
            Ok(())
        }
        _ => Err(invalid("This response does not match the request.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::{TaskAuthor, TaskId, TaskQuestion, TaskStatus, UserRequestTask};
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        thread,
    };
    fn fixture() -> (PathBuf, Arc<Requests>, UserRequest) {
        let root = std::env::temp_dir().join(format!("cide-user-request-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let store = Arc::new(Requests::load(root.join("requests.json")));
        let origin = serde_json::from_value(serde_json::json!({ "id": "conversation", "known": true, "kind": "planner", "harness": "codex", "title": "Plan", "cwd": root, "startedUnixMs": 1, "lastSeenUnixMs": 1 })).unwrap();
        let r = UserRequest {
            id: "request".into(),
            title: "Which?".into(),
            message: "Which?".into(),
            task: None,
            author: TaskAuthor::Orchestrator,
            request: UserRequestKind::Question {
                question: TaskQuestion::Text("Which?".into()),
            },
            origin,
            wait: true,
            created_unix_ms: 1,
            response: None,
            applying: false,
            delivered: false,
            delivery_error: None,
        };
        (root, store, r)
    }
    fn waiting_task(id: &str, question: Option<&str>) -> TaskRow {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": id, "status": "review", "acceptance": "user",
            "question": question, "createdUnixMs": 1, "updatedUnixMs": 1,
            "commentCount": 0, "attachmentCount": 0
        }))
        .unwrap()
    }

    #[test]
    fn each_waiting_call_holds_only_its_own_reply_lease() {
        let (root, store, request) = fixture();
        store.create(&root, request).unwrap();
        store
            .decide(&root, "request", UserRequestResponse::Cancel)
            .unwrap();
        let first = store.wait(&root, "request", || false).unwrap();
        let second = store.wait(&root, "request", || false).unwrap();
        drop(first);
        assert!(store.has_waiter("request"));
        assert!(store.wait(&root, "request", || true).is_err());
        assert!(
            store.has_waiter("request"),
            "cancellation must not release another reply"
        );
        drop(second);
        assert!(!store.has_waiter("request"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_late_codex_tool_display_resumes_once_but_a_consumed_result_does_not() {
        use crate::conversations::Transcript;
        use serde_json::json;
        struct Receiver<'a> {
            store: &'a Requests,
            transcript: Transcript,
            harness: FakeHarness,
        }
        impl AnswerTarget for Receiver<'_> {
            fn received(&self, request: &UserRequest) -> Result<bool> {
                Ok(self
                    .transcript
                    .received_tool_answer(request, &self.store.tool_reply_turns(&request.id))
                    || self.harness.received(request)?)
            }
            fn submit(
                &self,
                request: &UserRequest,
                begin: &dyn Fn() -> Result<bool>,
            ) -> Result<AnswerSubmission> {
                self.harness.submit(request, begin)
            }
        }
        for consumed in [false, true] {
            let (root, store, request) = fixture();
            store.create(&root, request).unwrap();
            let response = UserRequestResponse::Answer {
                text: "На этой же странице, но следующим рядом".into(),
                selected_ids: vec![],
            };
            store.decide(&root, "request", response.clone()).unwrap();
            let waiter = store.wait(&root, "request", || false).unwrap();
            store
                .tool_reply(&root, "request", Some("asking-turn".into()))
                .unwrap();
            drop(waiter); // socket accepted the reply, including after the turn ended
            assert!(!store.get(&root, "request").unwrap().delivered);
            // Provisional receipts survive an IDE restart; already delivered rows do not reopen.
            let restored = Requests::load(root.join("requests.json"));
            let mut receiver = Receiver {
                store: &restored,
                transcript: Transcript::default(),
                harness: FakeHarness::new(&root),
            };
            receiver.harness.native.store(true, Ordering::Release);
            receiver.harness.acknowledge.store(false, Ordering::Release);
            let result = json!({"id":"request", "pending":false, "response":response});
            receiver.transcript.feed(&json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"asking-turn"}}), "start");
            if consumed {
                receiver.transcript.feed(&json!({"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"exec","output":format!("Script completed\nOutput:\n{}", json!({"content":[{"type":"text","text":result.to_string()}]}))}}), "consumed");
            }
            receiver.transcript.feed(&json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"asking-turn"}}), "end");
            receiver.transcript.feed(&json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"McpToolCall","result":result}}}), "display-only");
            if !consumed {
                assert!(
                    !restored
                        .deliver_answer(&root, "request", &receiver)
                        .unwrap()
                );
                assert_eq!(receiver.harness.sends.load(Ordering::Acquire), 0);
            }
            receiver.harness.ready.store(true, Ordering::Release);
            assert!(
                restored
                    .deliver_answer(&root, "request", &receiver)
                    .unwrap()
            );
            assert!(
                restored
                    .deliver_answer(&root, "request", &receiver)
                    .unwrap()
            );
            assert_eq!(
                receiver.harness.sends.load(Ordering::Acquire),
                usize::from(!consumed)
            );
            assert!(restored.tool_reply_turns("request").is_empty());
            let again = Requests::load(root.join("requests.json"));
            assert!(again.deliver_answer(&root, "request", &receiver).unwrap());
            assert_eq!(
                receiver.harness.sends.load(Ordering::Acquire),
                usize::from(!consumed)
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    fn native(request: &mut UserRequest) {
        request.request = UserRequestKind::Questions {
            source: cide_ipc::NativeQuestionSource {
                session: cide_ipc::SessionId::new(),
                generation: cide_ipc::SessionId::new(),
                conversation: "native-thread".into(),
                request: "native-call".into(),
            },
            questions: vec![
                cide_ipc::UserRequestQuestion {
                    id: "first".into(),
                    header: "First".into(),
                    question: "First?".into(),
                    custom: true,
                },
                cide_ipc::UserRequestQuestion {
                    id: "second".into(),
                    header: "Second".into(),
                    question: "Second?".into(),
                    custom: true,
                },
            ],
        };
    }
    fn native_answer(id: &str) -> cide_ipc::UserRequestAnswer {
        cide_ipc::UserRequestAnswer {
            id: id.into(),
            text: "Answer".into(),
            selected_ids: vec![],
        }
    }

    #[test]
    fn native_batches_validate_the_whole_set_and_deduplicate_only_the_native_call() {
        let (root, store, mut request) = fixture();
        native(&mut request);
        store.create(&root, request.clone()).unwrap();
        let mut retry = request.clone();
        retry.id = "retry".into();
        assert_eq!(store.create(&root, retry).unwrap().id, "request");
        for answers in [
            vec![native_answer("first")],
            vec![native_answer("first"), native_answer("first")],
            vec![native_answer("first"), native_answer("unknown")],
        ] {
            assert!(
                store
                    .decide(&root, "request", UserRequestResponse::Answers { answers })
                    .is_err()
            );
        }
        store
            .decide(
                &root,
                "request",
                UserRequestResponse::Answers {
                    answers: vec![native_answer("second"), native_answer("first")],
                },
            )
            .unwrap();
        assert!(store.claim_native(&root, "request").unwrap());
        assert!(!store.claim_native(&root, "request").unwrap());
        let restored = Arc::new(Requests::load(root.join("requests.json")));
        assert!(
            !restored.claim_native(&root, "request").unwrap(),
            "uncertain writes must not be replayed after restart"
        );
        assert_eq!(
            restored.create(&root, request.clone()).unwrap().id,
            "request",
            "re-emitted native events must reuse even answered requests"
        );
        let mut other = request;
        other.id = "other".into();
        if let UserRequestKind::Questions { source, .. } = &mut other.request {
            source.request = "other-call".into();
        }
        store.create(&root, other).unwrap();
        assert_eq!(
            store.view(&root).requests.len(),
            2,
            "identical wording in a separate call is a new decision"
        );
        store
            .retire_native(&root, "request", "The native call ended")
            .unwrap();
        assert!(store.get(&root, "request").unwrap().delivered);
        assert!(
            store
                .decide(&root, "request", UserRequestResponse::Cancel)
                .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_choices_reject_unoffered_custom_answers() {
        let (root, store, mut request) = fixture();
        native(&mut request);
        if let UserRequestKind::Questions { questions, .. } = &mut request.request {
            questions[0].custom = false;
        }
        store.create(&root, request).unwrap();
        assert!(
            store
                .decide(
                    &root,
                    "request",
                    UserRequestResponse::Answers {
                        answers: vec![native_answer("first"), native_answer("second")]
                    }
                )
                .is_err()
        );
        assert!(store.get(&root, "request").unwrap().response.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_a_matching_native_pending_event_can_reopen_a_retired_batch() {
        for answered in [false, true] {
            let (root, store, mut request) = fixture();
            native(&mut request);
            store.create(&root, request.clone()).unwrap();
            if answered {
                store
                    .decide(
                        &root,
                        "request",
                        UserRequestResponse::Answers {
                            answers: vec![native_answer("first"), native_answer("second")],
                        },
                    )
                    .unwrap();
                assert!(store.claim_native(&root, "request").unwrap());
            }
            store
                .retire_native(&root, "request", "Disconnected")
                .unwrap();
            let restored = Arc::new(Requests::load(root.join("requests.json")));
            assert!(restored.get(&root, "request").unwrap().delivered);
            let reopened = restored.create(&root, request.clone()).unwrap();
            assert!(!reopened.delivered);
            assert_eq!(
                reopened.response.is_some(),
                answered,
                "a saved user answer survives, an implicit expiry does not become a user decision"
            );
            if answered {
                assert!(restored.claim_native(&root, "request").unwrap());
            }
            restored.delivered(&root, "request").unwrap();
            // A stale background snapshot must not turn confirmed delivery into a replayable expiry.
            restored
                .retire_native(&root, "request", "Late snapshot")
                .unwrap();
            assert!(restored.create(&root, request).unwrap().delivered);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    struct FakeHarness {
        transcript: PathBuf,
        ready: AtomicBool,
        acknowledge: AtomicBool,
        fail: AtomicBool,
        reject: AtomicBool,
        native: AtomicBool,
        sends: std::sync::atomic::AtomicUsize,
    }
    impl FakeHarness {
        fn new(root: &Path) -> Self {
            Self {
                transcript: root.join("harness.json"),
                ready: AtomicBool::new(false),
                acknowledge: AtomicBool::new(true),
                fail: AtomicBool::new(false),
                reject: AtomicBool::new(false),
                native: AtomicBool::new(false),
                sends: std::sync::atomic::AtomicUsize::new(0),
            }
        }
        fn acknowledge(&self, request: &UserRequest) {
            let value = serde_json::json!({
                "conversation": request.origin.id, "harness": request.origin.harness,
                "request": request.id, "response": request.response,
            });
            std::fs::write(&self.transcript, value.to_string()).unwrap();
        }
    }
    impl AnswerTarget for FakeHarness {
        fn received(&self, request: &UserRequest) -> Result<bool> {
            let value = std::fs::read(&self.transcript)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
            Ok(value.is_some_and(|v| {
                v["conversation"] == request.origin.id
                    && v["request"] == request.id
                    && v["response"] == serde_json::to_value(&request.response).unwrap()
            }))
        }
        fn submit(
            &self,
            request: &UserRequest,
            begin: &dyn Fn() -> Result<bool>,
        ) -> Result<AnswerSubmission> {
            if !self.ready.load(Ordering::Acquire) || !begin()? {
                return Ok(AnswerSubmission::Waiting);
            }
            if self.reject.load(Ordering::Acquire) {
                return Ok(AnswerSubmission::NotSent(
                    "The native receiver became busy".into(),
                ));
            }
            self.sends.fetch_add(1, Ordering::AcqRel);
            if self.acknowledge.load(Ordering::Acquire) {
                self.acknowledge(request);
            }
            if self.fail.load(Ordering::Acquire) {
                return Err(CoreError::Io(
                    "Connection ended before acknowledgment".into(),
                ));
            }
            Ok(if self.native.load(Ordering::Acquire) {
                AnswerSubmission::Confirmed
            } else {
                AnswerSubmission::Submitted
            })
        }
    }

    #[test]
    fn dismissing_a_saved_decision_survives_restart_without_retrying_delivery() {
        let (root, store, mut request) = fixture();
        request.wait = false;
        request.task = Some(TaskId("t-20".into()));
        store.create(&root, request.clone()).unwrap();
        let mut other = request;
        other.id = "other".into();
        other.task = None;
        store.create(&root, other.clone()).unwrap();
        store
            .decide(&root, "request", UserRequestResponse::Cancel)
            .unwrap();
        store.complete_application(&root, "request").unwrap();
        store
            .delivery_error(&root, "request", "The original run is unavailable".into())
            .unwrap();
        let before = store.view(&root).rev;
        let dismissed = store.dismiss(&root, "request").unwrap();
        assert!(dismissed.delivered);
        assert_eq!(dismissed.response, Some(UserRequestResponse::Cancel));
        assert!(dismissed.delivery_error.is_none());
        assert!(store.view(&root).rev > before);
        assert_eq!(store.get(&root, "other").unwrap(), other);
        let restored = Requests::load(root.join("requests.json"));
        assert!(restored.get(&root, "request").unwrap().delivered);
        let target = FakeHarness::new(&root);
        target.ready.store(true, Ordering::Release);
        assert!(restored.deliver_answer(&root, "request", &target).unwrap());
        assert_eq!(target.sends.load(Ordering::Acquire), 0);
        assert_eq!(waiting_count(&restored.view(&root).requests, &[]), 1);
        // A second window may still have the removed row open.
        restored.dismiss(&root, "request").unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dismissal_refuses_unanswered_applying_and_wrong_project_requests() {
        let (root, store, mut request) = fixture();
        request.task = Some(TaskId("t-20".into()));
        store.create(&root, request).unwrap();
        let before = store.view(&root);
        assert!(store.dismiss(&root, "request").is_err());
        assert_eq!(store.view(&root), before);
        store
            .decide(&root, "request", UserRequestResponse::Cancel)
            .unwrap();
        let before = store.view(&root);
        assert!(store.dismiss(&root, "request").is_err());
        assert!(
            store
                .dismiss(&root.join("other-project"), "request")
                .is_err()
        );
        assert_eq!(store.view(&root), before);
        store.complete_application(&root, "request").unwrap();
        store.dismiss(&root, "request").unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dismissed_native_decisions_do_not_reopen_on_repeated_events() {
        let (root, store, mut request) = fixture();
        native(&mut request);
        store.create(&root, request.clone()).unwrap();
        store
            .decide(&root, "request", UserRequestResponse::Cancel)
            .unwrap();
        assert!(store.claim_native(&root, "request").unwrap());
        store.dismiss(&root, "request").unwrap();
        assert!(store.file.lock().delivery_attempts.is_empty());
        let restored = Requests::load(root.join("requests.json"));
        assert!(restored.create(&root, request).unwrap().delivered);
        assert!(!restored.claim_native(&root, "request").unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_native_acknowledgement_completes_delivery_without_waiting_for_export() {
        let (root, store, request) = fixture();
        store.create(&root, request).unwrap();
        store
            .decide(&root, "request", UserRequestResponse::Cancel)
            .unwrap();
        let target = FakeHarness::new(&root);
        target.ready.store(true, Ordering::Release);
        target.acknowledge.store(false, Ordering::Release);
        target.native.store(true, Ordering::Release);
        assert!(store.deliver_answer(&root, "request", &target).unwrap());
        assert!(store.deliver_answer(&root, "request", &target).unwrap());
        assert_eq!(target.sends.load(Ordering::Acquire), 1);
        assert!(store.get(&root, "request").unwrap().delivered);
        assert!(store.file.lock().delivery_attempts.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn automatic_and_worker_answers_wait_for_receipt_in_the_asking_harness() {
        use cide_ipc::{Harness, sessions::SessionKind};
        for harness in [Harness::Claude, Harness::Codex, Harness::Opencode] {
            for kind in [
                SessionKind::Planner,
                SessionKind::Reviewer,
                SessionKind::Worker,
                SessionKind::Console,
            ] {
                let (root, store, mut request) = fixture();
                request.wait = false;
                request.origin.harness = harness;
                request.origin.kind = kind;
                store.create(&root, request).unwrap();
                store
                    .decide(
                        &root,
                        "request",
                        UserRequestResponse::Answer {
                            text: "Use blue — синий".into(),
                            selected_ids: vec![],
                        },
                    )
                    .unwrap();
                let target = FakeHarness::new(&root);
                assert!(!store.deliver_answer(&root, "request", &target).unwrap());
                assert_eq!(
                    target.sends.load(Ordering::Acquire),
                    0,
                    "busy conversation must wait"
                );
                target.ready.store(true, Ordering::Release);
                assert!(
                    !store.deliver_answer(&root, "request", &target).unwrap(),
                    "queueing input is not receipt"
                );
                assert!(!store.get(&root, "request").unwrap().delivered);
                assert!(store.deliver_answer(&root, "request", &target).unwrap());
                assert!(store.deliver_answer(&root, "request", &target).unwrap());
                assert_eq!(target.sends.load(Ordering::Acquire), 1);
                assert!(
                    store
                        .get(&root, "request")
                        .unwrap()
                        .delivery_error
                        .is_none()
                );
                std::fs::remove_dir_all(root).unwrap();
            }
        }
    }

    #[test]
    fn an_uncertain_submission_survives_restart_without_resending_or_crossing_conversations() {
        let (root, store, request) = fixture();
        store.create(&root, request).unwrap();
        store
            .decide(&root, "request", UserRequestResponse::Cancel)
            .unwrap();
        let target = FakeHarness::new(&root);
        target.ready.store(true, Ordering::Release);
        target.acknowledge.store(false, Ordering::Release);
        target.fail.store(true, Ordering::Release);
        assert!(store.deliver_answer(&root, "request", &target).is_err());
        let restored = Arc::new(Requests::load(root.join("requests.json")));
        assert!(!restored.deliver_answer(&root, "request", &target).unwrap());
        assert_eq!(target.sends.load(Ordering::Acquire), 1);
        let mut wrong = restored.get(&root, "request").unwrap();
        wrong.origin.id = "replacement-conversation".into();
        target.acknowledge(&wrong);
        assert!(!restored.deliver_answer(&root, "request", &target).unwrap());
        target.acknowledge(&restored.get(&root, "request").unwrap());
        assert!(restored.deliver_answer(&root, "request", &target).unwrap());
        assert_eq!(target.sends.load(Ordering::Acquire), 1);
        assert!(restored.file.lock().delivery_attempts.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn definite_non_submissions_retry_and_a_blocking_waiter_keeps_ownership() {
        let (root, store, request) = fixture();
        store.create(&root, request).unwrap();
        store
            .decide(&root, "request", UserRequestResponse::Cancel)
            .unwrap();
        let target = FakeHarness::new(&root);
        target.ready.store(true, Ordering::Release);
        let waited = store.wait(&root, "request", || false).unwrap();
        assert!(!store.deliver_answer(&root, "request", &target).unwrap());
        assert_eq!(target.sends.load(Ordering::Acquire), 0);
        // A failed socket write releases the waiter without marking delivery complete.
        drop(waited);
        target.reject.store(true, Ordering::Release);
        assert!(!store.deliver_answer(&root, "request", &target).unwrap());
        assert!(store.file.lock().delivery_attempts.is_empty());
        assert!(
            store
                .get(&root, "request")
                .unwrap()
                .delivery_error
                .is_some()
        );
        target.reject.store(false, Ordering::Release);
        assert!(!store.deliver_answer(&root, "request", &target).unwrap());
        assert!(store.deliver_answer(&root, "request", &target).unwrap());
        assert_eq!(target.sends.load(Ordering::Acquire), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saved_answers_stop_waiting_for_the_user_before_delivery() {
        let (root, store, mut request) = fixture();
        request.task = Some(TaskId("t-1".into()));
        request.wait = false;
        let mut tasks = vec![waiting_task("t-1", Some("Which?"))];
        store.create(&root, request).unwrap();
        assert_eq!(waiting_count(&store.view(&root).requests, &tasks), 1);
        let answer = UserRequestResponse::Answer {
            text: "Proceed".into(),
            selected_ids: vec![],
        };
        store.decide(&root, "request", answer.clone()).unwrap();
        assert!(store.get(&root, "request").unwrap().applying);
        assert_eq!(waiting_count(&store.view(&root).requests, &tasks), 0);
        let restored = Arc::new(Requests::load(root.join("requests.json")));
        assert_eq!(
            waiting_count(&restored.view(&root).requests, &tasks),
            0,
            "restart must preserve the saved decision's count"
        );
        restored
            .delivery_error(&root, "request", "Agent busy".into())
            .unwrap();
        assert_eq!(waiting_count(&restored.view(&root).requests, &tasks), 0);
        restored
            .reset_application(&root, "request", "Try again".into())
            .unwrap();
        assert_eq!(waiting_count(&restored.view(&root).requests, &tasks), 1);
        restored.decide(&root, "request", answer).unwrap();
        restored.complete_application(&root, "request").unwrap();
        restored.delivered(&root, "request").unwrap();
        tasks[0].question = None;
        tasks[0].status = TaskStatus::Doing;
        assert_eq!(waiting_count(&restored.view(&root).requests, &tasks), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn waiting_counts_only_open_decisions_and_unlinked_tasks() {
        let (root, _, mut answered) = fixture();
        answered.task = Some(TaskId("t-1".into()));
        let mut open = answered.clone();
        open.id = "open".into();
        open.task = None;
        answered.response = Some(UserRequestResponse::Cancel);
        let mut delivered = open.clone();
        delivered.delivered = true;
        let mut automatic = waiting_task("t-4", None);
        automatic.acceptance = None;
        let tasks = vec![
            waiting_task("t-1", Some("Linked")),
            waiting_task("t-2", Some("Standalone")),
            waiting_task("t-3", None),
            automatic,
        ];
        assert_eq!(waiting_count(&[], &tasks), 3);
        assert_eq!(
            waiting_count(&[answered, open, delivered], &tasks),
            3,
            "one open request, one standalone question, one manual review"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn a_wait_blocks_until_a_durable_answer_and_rejects_a_second_window() {
        let (root, store, request) = fixture();
        store.create(&root, request.clone()).unwrap();
        assert_eq!(store.create(&root, request).unwrap().id, "request");
        assert_eq!(store.view(&root).requests.len(), 1);
        let (tx, rx) = mpsc::channel();
        let (worker, path) = (Arc::clone(&store), root.clone());
        let thread = thread::spawn(move || {
            tx.send(worker.wait(&path, "request", || false)).unwrap();
        });
        assert!(
            rx.recv_timeout(Duration::from_millis(40)).is_err(),
            "posting a question must not return an answer"
        );
        let answer = UserRequestResponse::Answer {
            text: "Proceed".into(),
            selected_ids: vec![],
        };
        store.decide(&root, "request", answer.clone()).unwrap();
        let waited = rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
        assert_eq!(waited.response, Some(answer.clone()));
        assert!(store.decide(&root, "request", answer.clone()).is_err());
        assert_eq!(
            Requests::load(root.join("requests.json"))
                .get(&root, "request")
                .unwrap()
                .response,
            Some(answer)
        );
        assert!(
            store.has_waiter("request"),
            "delivery owns the lease until the socket write"
        );
        store.delivered(&root, "request").unwrap();
        drop(waited);
        thread.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn disconnect_keeps_the_question_and_can_reconnect_after_restart() {
        let (root, store, request) = fixture();
        store.create(&root, request).unwrap();
        let cancelled = AtomicBool::new(true);
        assert!(
            store
                .wait(&root, "request", || cancelled.load(Ordering::Acquire))
                .is_err()
        );
        assert!(!store.has_waiter("request"));
        let restored = Arc::new(Requests::load(root.join("requests.json")));
        assert!(restored.get(&root, "request").unwrap().response.is_none());
        restored
            .decide(&root, "request", UserRequestResponse::Cancel)
            .unwrap();
        assert_eq!(
            restored.wait(&root, "request", || false).unwrap().response,
            Some(UserRequestResponse::Cancel)
        );

        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn a_linked_question_waits_until_its_task_answer_is_durable() {
        let (root, store, mut request) = fixture();
        request.task = Some(TaskId("t-1".into()));
        store.create(&root, request).unwrap();
        let answered = store
            .decide(
                &root,
                "request",
                UserRequestResponse::Answer {
                    text: "A".into(),
                    selected_ids: vec![],
                },
            )
            .unwrap();
        assert!(answered.applying);
        let (tx, rx) = mpsc::channel();
        let (worker, path) = (Arc::clone(&store), root.clone());
        let waiter = thread::spawn(move || {
            tx.send(worker.wait(&path, "request", || false)).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(40)).is_err());
        store.complete_application(&root, "request").unwrap();
        assert!(
            !rx.recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap()
                .applying
        );

        waiter.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_selected_batch_is_checkpointed_and_not_returned_before_application() {
        let (root, store, mut request) = fixture();
        request.request = UserRequestKind::MoveTasks {
            milestone: "look".into(),
            goal: TaskId("t-1".into()),
            tasks: vec![UserRequestTask {
                id: TaskId("t-2".into()),
                title: "A".into(),
                status: TaskStatus::Inbox,
                parent: None,
            }],
        };
        store.create(&root, request).unwrap();
        assert!(
            store
                .decide(
                    &root,
                    "request",
                    UserRequestResponse::Approve {
                        selected_ids: vec![TaskId("t-99".into())],
                        note: String::new()
                    }
                )
                .is_err()
        );
        let approved = store
            .decide(
                &root,
                "request",
                UserRequestResponse::Approve {
                    selected_ids: vec![TaskId("t-2".into())],
                    note: String::new(),
                },
            )
            .unwrap();
        assert!(approved.applying);
        let restored = Arc::new(Requests::load(root.join("requests.json")));
        assert!(restored.get(&root, "request").unwrap().applying);
        restored.complete_application(&root, "request").unwrap();
        assert!(!restored.wait(&root, "request", || false).unwrap().applying);

        std::fs::remove_dir_all(root).unwrap();
    }
}
