//! Process-global runtime state.
//!
//! The registry is the reason detach, re-dock, the tabs-vs-windows setting and
//! survive-window-close are all the *same* mechanism rather than four features: a pane
//! holds a `SessionId`, never a session, so moving or destroying a pane never touches a
//! child process.

use std::sync::Arc;

use cide_ipc::SessionId;
use cide_pty::{PtySession, SinkId};
use dashmap::DashMap;

/// Every live PTY session in the process, keyed by id.
///
/// Sessions are owned here and nowhere else. Windows, tabs and panes merely *attach*;
/// closing any of them detaches a sink, it does not kill a child.
#[derive(Default)]
pub struct SessionRegistry {
    sessions: DashMap<SessionId, Arc<PtySession>>,
    /// Live attachments, so a webview going away can drop exactly its own sink.
    attachments: DashMap<AttachmentKey, SinkId>,
}

/// A sink belonging to one (session, window) pair.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AttachmentKey {
    pub session: SessionId,
    pub window: String,
}

impl SessionRegistry {
    pub fn insert(&self, id: SessionId, session: Arc<PtySession>) {
        self.sessions.insert(id, session);
    }

    pub fn get(&self, id: SessionId) -> Option<Arc<PtySession>> {
        self.sessions.get(&id).map(|r| Arc::clone(r.value()))
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    pub fn ids(&self) -> Vec<SessionId> {
        self.sessions.iter().map(|r| *r.key()).collect()
    }

    pub fn record_attachment(&self, key: AttachmentKey, sink: SinkId) {
        // Replacing an existing attachment for the same (session, window) detaches the old
        // sink first, so a webview that reloads does not leak a dead sink.
        if let Some((_, old)) = self.attachments.remove(&key)
            && let Some(session) = self.get(key.session)
        {
            session.detach(old);
        }
        self.attachments.insert(key, sink);
    }

    /// Look up an attachment without removing it — the credit-ack path, which runs on every
    /// frame and must not disturb the registration it is reporting against.
    pub fn attachment(&self, key: &AttachmentKey) -> Option<SinkId> {
        self.attachments.get(key).map(|v| *v)
    }

    pub fn take_attachment(&self, key: &AttachmentKey) -> Option<SinkId> {
        self.attachments.remove(key).map(|(_, v)| v)
    }

    /// Ask every child to exit. Used on quit: cide does not keep sessions alive in the
    /// background, so quitting the app quits its Claude sessions.
    pub fn kill_all(&self) {
        for entry in self.sessions.iter() {
            entry.value().kill();
        }
    }

    pub fn remove(&self, id: SessionId) -> Option<Arc<PtySession>> {
        self.sessions.remove(&id).map(|(_, v)| v)
    }
}
