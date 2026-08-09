//! Pending `openDiff` requests, and every way one can end.
//!
//! # Why this is a module and not a `HashMap`
//!
//! `openDiff` blocks the agent's turn. The CLI sends it and waits; nothing else happens in
//! that conversation until an answer comes back. So the question this module exists to
//! answer is not "how do I store a oneshot" but **"what are all the ways the human can
//! vanish, and does each of them resolve the request?"**
//!
//! The ways, all of which are reachable in normal use:
//!
//! 1. the user answers — accept, accept-with-edits, or reject;
//! 2. the diff tab is closed;
//! 3. the pane showing it is closed;
//! 4. the window is closed, by the app or by the window manager;
//! 5. the project is closed;
//! 6. the app quits, or is signalled;
//! 7. the WebSocket drops — the `claude` child died, or the socket broke;
//! 8. the CLI itself sends `close_tab`, which it does from its own `beforeExit`;
//! 9. a second request arrives for a file already under diff.
//!
//! Miss one and a `claude` session hangs indefinitely with no error, no timeout and nothing
//! on screen explaining it. Every one of those is a method here, and a test.
//!
//! # The default answer is rejection
//!
//! When a request is cancelled rather than answered, it resolves as
//! [`DiffOutcome::Rejected`]. That is the safe direction: a rejected diff leaves the file
//! alone and the agent can say so, whereas defaulting to accepted would write a change
//! nobody approved because a window happened to close.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::oneshot;

use crate::protocol::{DiffOutcome, OpenDiffParams};

/// A diff waiting to be answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRequest {
    /// Ours, not the CLI's: the JSON-RPC id belongs to one connection, and this has to be
    /// addressable from a frontend that knows nothing about connections.
    pub id: String,
    pub params: OpenDiffParams,
    /// The pane whose `claude` asked, resolved from the connection's pid. `None` when the
    /// connection never sent `ide_connected`, which happens if the CLI is older or the
    /// notification is lost — the diff still works, it just cannot be attributed.
    pub pane: Option<String>,
}

/// Why a pending diff was resolved without the user answering it.
///
/// Recorded because "the diff closed by itself" is otherwise indistinguishable from "the
/// user rejected it", and the two want different words on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    /// The CLI asked, via `close_tab` or `closeAllDiffTabs`.
    ClientClosed,
    /// The pane showing it went away.
    PaneClosed,
    /// The window showing it went away.
    WindowClosed,
    /// The project went away.
    ProjectClosed,
    /// The connection dropped — the child died, or the socket broke.
    Disconnected,
    /// The application is shutting down.
    Shutdown,
    /// A newer request replaced this one for the same file.
    Superseded,
}

struct Pending {
    request: DiffRequest,
    reply: oneshot::Sender<DiffOutcome>,
    /// Which connection asked, so a dropped socket can cancel exactly its own requests.
    connection: u64,
}

/// Every `openDiff` currently waiting for an answer.
///
/// Cheap to clone; all clones share one registry.
#[derive(Clone, Default)]
pub struct DiffBroker {
    inner: Arc<Mutex<HashMap<String, Pending>>>,
}

impl DiffBroker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a request and hand back the future the tool handler awaits.
    ///
    /// A second request for a file already under diff **supersedes** the first rather than
    /// queueing behind it or being refused. Two live diffs for one file would let a user
    /// accept them in either order and write whichever finished last; refusing would hang
    /// the newer agent turn. Superseding resolves the older as rejected, which the older
    /// agent can act on.
    pub fn open(&self, request: DiffRequest, connection: u64) -> oneshot::Receiver<DiffOutcome> {
        let (tx, rx) = oneshot::channel();

        let superseded: Vec<String> = {
            let map = self.inner.lock();
            map.values()
                .filter(|p| p.request.params.new_file_path == request.params.new_file_path)
                .map(|p| p.request.id.clone())
                .collect()
        };
        for id in superseded {
            self.cancel(&id, CancelReason::Superseded);
        }

        self.inner.lock().insert(
            request.id.clone(),
            Pending {
                request,
                reply: tx,
                connection,
            },
        );
        rx
    }

    /// The user answered. Returns false when the request is already gone, which is normal:
    /// a pane can close while the answer is in flight.
    pub fn resolve(&self, id: &str, outcome: DiffOutcome) -> bool {
        let Some(pending) = self.inner.lock().remove(id) else {
            return false;
        };
        // A closed receiver means the agent turn ended some other way — the child died, or
        // the connection dropped. Not an error; there is simply nobody to tell.
        pending.reply.send(outcome).is_ok()
    }

    /// Resolve one request as rejected.
    pub fn cancel(&self, id: &str, reason: CancelReason) -> bool {
        let Some(pending) = self.inner.lock().remove(id) else {
            return false;
        };
        tracing::debug!(id, ?reason, "cancelling a pending diff");
        pending.reply.send(DiffOutcome::Rejected).is_ok()
    }

    /// Cancel by tab name — what `close_tab` carries.
    pub fn cancel_tab(&self, tab_name: &str, reason: CancelReason) -> usize {
        let ids: Vec<String> = {
            let map = self.inner.lock();
            map.values()
                .filter(|p| p.request.params.tab_name == tab_name)
                .map(|p| p.request.id.clone())
                .collect()
        };
        ids.iter().filter(|id| self.cancel(id, reason)).count()
    }

    /// Cancel everything a pane was showing.
    pub fn cancel_pane(&self, pane: &str, reason: CancelReason) -> usize {
        let ids: Vec<String> = {
            let map = self.inner.lock();
            map.values()
                .filter(|p| p.request.pane.as_deref() == Some(pane))
                .map(|p| p.request.id.clone())
                .collect()
        };
        ids.iter().filter(|id| self.cancel(id, reason)).count()
    }

    /// Cancel everything a connection asked for.
    ///
    /// Called when a socket drops. Without it, a `claude` that died mid-diff leaves a
    /// request that nothing will ever answer and a diff tab nothing will ever close.
    pub fn cancel_connection(&self, connection: u64, reason: CancelReason) -> usize {
        let ids: Vec<String> = {
            let map = self.inner.lock();
            map.values()
                .filter(|p| p.connection == connection)
                .map(|p| p.request.id.clone())
                .collect()
        };
        ids.iter().filter(|id| self.cancel(id, reason)).count()
    }

    /// Cancel everything. The shutdown and project-close path.
    pub fn cancel_all(&self, reason: CancelReason) -> usize {
        let ids: Vec<String> = self.inner.lock().keys().cloned().collect();
        ids.iter().filter(|id| self.cancel(id, reason)).count()
    }

    /// Requests still waiting, for the UI and for tests.
    pub fn pending(&self) -> Vec<DiffRequest> {
        self.inner
            .lock()
            .values()
            .map(|p| p.request.clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(id: &str, path: &str) -> DiffRequest {
        DiffRequest {
            id: id.into(),
            params: OpenDiffParams {
                old_file_path: path.into(),
                new_file_path: path.into(),
                new_file_contents: "new".into(),
                tab_name: format!("tab:{path}"),
            },
            pane: Some("pane-1".into()),
        }
    }

    #[tokio::test]
    async fn an_answered_diff_resolves_with_the_users_outcome() {
        let broker = DiffBroker::new();
        let rx = broker.open(request("a", "/f.rs"), 1);

        assert!(broker.resolve(
            "a",
            DiffOutcome::Saved {
                contents: "edited".into()
            }
        ));

        assert_eq!(
            rx.await.expect("resolved"),
            DiffOutcome::Saved {
                contents: "edited".into()
            }
        );
        assert!(broker.is_empty());
    }

    #[tokio::test]
    async fn every_disappearance_resolves_as_rejected() {
        // The table this module exists for. Each row is a way the human can vanish; none of
        // them may leave the agent waiting, and none may default to accepting an edit.
        /// One way the human can vanish: a name, and the call that makes it happen.
        type Disappearance = (&'static str, Box<dyn Fn(&DiffBroker, &str)>);
        let cases: Vec<Disappearance> = vec![
            (
                "tab closed by the client",
                Box::new(|b: &DiffBroker, _id| {
                    b.cancel_tab("tab:/f.rs", CancelReason::ClientClosed);
                }),
            ),
            (
                "pane closed",
                Box::new(|b: &DiffBroker, _id| {
                    b.cancel_pane("pane-1", CancelReason::PaneClosed);
                }),
            ),
            (
                "connection dropped",
                Box::new(|b: &DiffBroker, _id| {
                    b.cancel_connection(1, CancelReason::Disconnected);
                }),
            ),
            (
                "app shutting down",
                Box::new(|b: &DiffBroker, _id| {
                    b.cancel_all(CancelReason::Shutdown);
                }),
            ),
            (
                "cancelled by id",
                Box::new(|b: &DiffBroker, id| {
                    b.cancel(id, CancelReason::WindowClosed);
                }),
            ),
        ];

        for (name, cancel) in cases {
            let broker = DiffBroker::new();
            let rx = broker.open(request("a", "/f.rs"), 1);
            cancel(&broker, "a");
            assert_eq!(
                rx.await.expect("resolved"),
                DiffOutcome::Rejected,
                "{name}: a cancelled diff must reject, never accept"
            );
            assert!(broker.is_empty(), "{name}: left a pending request behind");
        }
    }

    #[tokio::test]
    async fn a_second_diff_for_one_file_supersedes_the_first() {
        let broker = DiffBroker::new();
        let first = broker.open(request("a", "/f.rs"), 1);
        let second = broker.open(request("b", "/f.rs"), 1);

        // The older turn is told no rather than left waiting, and only one diff is live.
        assert_eq!(first.await.expect("resolved"), DiffOutcome::Rejected);
        assert_eq!(broker.len(), 1);

        broker.resolve("b", DiffOutcome::TabClosed);
        assert_eq!(second.await.expect("resolved"), DiffOutcome::TabClosed);
    }

    #[tokio::test]
    async fn a_diff_for_a_different_file_is_untouched() {
        let broker = DiffBroker::new();
        let keep = broker.open(request("a", "/one.rs"), 1);
        let _replace = broker.open(request("b", "/two.rs"), 1);

        assert_eq!(broker.len(), 2, "different files coexist");
        broker.cancel_all(CancelReason::Shutdown);
        assert_eq!(keep.await.expect("resolved"), DiffOutcome::Rejected);
    }

    #[test]
    fn resolving_an_unknown_id_is_not_an_error() {
        // A pane can close while an answer is in flight; the answer arrives for a request
        // that is already gone, and that is ordinary rather than exceptional.
        let broker = DiffBroker::new();
        assert!(!broker.resolve("ghost", DiffOutcome::Rejected));
        assert!(!broker.cancel("ghost", CancelReason::PaneClosed));
    }

    #[tokio::test]
    async fn cancelling_a_dropped_receiver_does_not_panic() {
        let broker = DiffBroker::new();
        let rx = broker.open(request("a", "/f.rs"), 1);
        drop(rx);

        // False because nobody was listening, not because anything went wrong: the agent
        // turn ended some other way.
        assert!(!broker.cancel("a", CancelReason::Disconnected));
        assert!(broker.is_empty());
    }
}
