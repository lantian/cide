//! Durable decisions and delivery receipts. No task tracker or process ownership lives here.
use crate::{CoreError, Result, persist};
use cide_ipc::{
    Acceptance, TaskRow, TaskStatus, UserRequest, UserRequestKind, UserRequestResponse,
    UserRequestsView,
};
use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
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
}

pub struct Requests {
    path: PathBuf,
    file: Mutex<File>,
    changed: Condvar,
    /// Leases last through the socket write, rather than just through the wait.
    waiters: Mutex<HashMap<String, usize>>,
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
            if let Some(existing) = rows.iter().find(|r| {
                (r.origin.id == request.origin.id
                    || (r.origin.run.is_some() && r.origin.run == request.origin.run)
                    || (r.origin.session.is_some() && r.origin.session == request.origin.session))
                    && r.response.is_none()
                    && r.request == request.request
                    && r.task == request.task
            }) {
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
            if r.response.is_some() {
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
        self.change(root, id, |r| {
            r.delivered = true;
            r.delivery_error = None;
            Ok(())
        })
    }
    pub fn delivery_error(&self, root: &Path, id: &str, error: String) -> Result<UserRequest> {
        self.change(root, id, |r| {
            r.delivery_error = Some(error);
            Ok(())
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
        self.waiters.lock().get(id).copied().unwrap_or(0) > 0
    }
    pub fn release(&self, id: &str) {
        let mut waiters = self.waiters.lock();
        if let Some(count) = waiters.get_mut(id) {
            *count -= 1;
            if *count == 0 {
                waiters.remove(id);
            }
        }
    }
    /// The caller releases the lease after writing its reply, including on write failure.
    pub fn wait(&self, root: &Path, id: &str, cancelled: impl Fn() -> bool) -> Result<UserRequest> {
        *self.waiters.lock().entry(id.to_owned()).or_default() += 1;
        let mut file = self.file.lock();
        loop {
            if cancelled() {
                self.release(id);
                return Err(CoreError::Io(format!(
                    "Waiting interrupted. Request {id} is saved; reconnect with cide_user_request_result and wait=true before continuing dependent work."
                )));
            }
            let request = file
                .projects
                .get(&root.to_string_lossy().into_owned())
                .and_then(|rows| rows.iter().find(|r| r.id == id));
            match request {
                Some(r) if r.response.is_some() && !r.applying => return Ok(r.clone()),
                Some(_) => {
                    self.changed.wait_for(&mut file, Duration::from_millis(200));
                }
                None => {
                    self.release(id);
                    return Err(CoreError::Io("No such request.".into()));
                }
            }
        }
    }
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
        let restored = Requests::load(root.join("requests.json"));
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
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap()
                .response,
            Some(answer.clone())
        );
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
        store.release("request");
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
        let restored = Requests::load(root.join("requests.json"));
        assert!(restored.get(&root, "request").unwrap().response.is_none());
        restored
            .decide(&root, "request", UserRequestResponse::Cancel)
            .unwrap();
        assert_eq!(
            restored.wait(&root, "request", || false).unwrap().response,
            Some(UserRequestResponse::Cancel)
        );
        restored.release("request");
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
        store.release("request");
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
        let restored = Requests::load(root.join("requests.json"));
        assert!(restored.get(&root, "request").unwrap().applying);
        restored.complete_application(&root, "request").unwrap();
        assert!(!restored.wait(&root, "request", || false).unwrap().applying);
        restored.release("request");
        std::fs::remove_dir_all(root).unwrap();
    }
}
