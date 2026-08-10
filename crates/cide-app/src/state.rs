//! Process-global runtime state.
//!
//! The registry is the reason detach, re-dock, the tabs-vs-windows setting and
//! survive-window-close are all the *same* mechanism rather than four features: a pane
//! holds a `SessionId`, never a session, so moving or destroying a pane never touches a
//! child process.

use std::sync::Arc;

use cide_ipc::{PaneId, SessionId};
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

/// A sink belonging to one pane's view of one session.
///
/// **The pane is part of the key, and that is the whole point of mirroring.** Keyed on
/// `(session, window)` alone, two panes showing one session in one window are one
/// attachment: the second `record_attachment` detaches the first's sink, so opening a mirror
/// silently blanks the pane being mirrored. `cide_pty` already keeps sinks as a *list* so a
/// session can feed several consumers — this key is what lets the registry name them apart.
///
/// `pane` is optional because a caller that does not name one is still a legal caller: it
/// gets the old window-wide slot, where a second attachment replaces the first. Nothing in
/// the app does that today (`TerminalPane` always names its pane), but a `None` here fails
/// the way the code failed before rather than refusing to attach at all.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AttachmentKey {
    pub session: SessionId,
    pub window: String,
    pub pane: Option<PaneId>,
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
        // Replacing an existing attachment for the same (session, window, pane) detaches the
        // old sink first, so a webview that reloads does not leak a dead sink. Because the
        // pane is in the key, this only ever reclaims a *re-attachment by the same pane* —
        // never a sibling pane mirroring the same session.
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

#[cfg(test)]
mod tests {
    use super::*;
    use cide_pty::{Sink, SpawnSpec};

    /// A child that produces nothing and stays alive, so the assertions are about the
    /// registry's bookkeeping and never about a race with the reaper.
    fn idle_session() -> Arc<PtySession> {
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("sleep 30");
        PtySession::spawn(spec).expect("spawn sh")
    }

    fn null_sink() -> Arc<dyn Sink> {
        Arc::new(|_: &[u8]| true)
    }

    #[test]
    fn two_panes_mirroring_one_session_in_one_window_both_survive() {
        // The bug this pins: `AttachmentKey` was `(session, window)`, so the second pane's
        // `record_attachment` detached the first pane's sink. Mirroring a session into a
        // split therefore blanked the pane being mirrored — the one case mirroring exists
        // for.
        let registry = SessionRegistry::default();
        let id = SessionId::new();
        let session = idle_session();
        registry.insert(id, Arc::clone(&session));

        let left = PaneId::new();
        let right = PaneId::new();
        let left_sink = session.attach(null_sink());
        registry.record_attachment(
            AttachmentKey {
                session: id,
                window: "main".into(),
                pane: Some(left),
            },
            left_sink,
        );
        let right_sink = session.attach(null_sink());
        registry.record_attachment(
            AttachmentKey {
                session: id,
                window: "main".into(),
                pane: Some(right),
            },
            right_sink,
        );

        assert_eq!(session.sink_count(), 2, "the mirror detached its original");

        // And each is separately addressable: an ack from one pane must not be credited to
        // the other, or a mirror that keeps up would unchoke a pane that has stalled.
        session.ack(left_sink, 0);
        for (pane, sink) in [(left, left_sink), (right, right_sink)] {
            let found = registry.attachment(&AttachmentKey {
                session: id,
                window: "main".into(),
                pane: Some(pane),
            });
            assert_eq!(found, Some(sink), "each pane keeps its own sink");
        }

        session.kill();
    }

    #[test]
    fn the_same_pane_re_attaching_reclaims_its_own_sink() {
        // The behaviour the old key was there for, and which must survive the fix: a webview
        // that reloads attaches again for the same pane, and the sink it abandoned has to go
        // rather than accumulate.
        let registry = SessionRegistry::default();
        let id = SessionId::new();
        let session = idle_session();
        registry.insert(id, Arc::clone(&session));

        let pane = PaneId::new();
        let key = AttachmentKey {
            session: id,
            window: "main".into(),
            pane: Some(pane),
        };
        registry.record_attachment(key.clone(), session.attach(null_sink()));
        let second = session.attach(null_sink());
        registry.record_attachment(key.clone(), second);

        assert_eq!(session.sink_count(), 1, "the stale sink was left behind");
        assert_eq!(registry.attachment(&key), Some(second));

        session.kill();
    }

    #[test]
    fn detaching_one_pane_leaves_its_mirror_attached() {
        let registry = SessionRegistry::default();
        let id = SessionId::new();
        let session = idle_session();
        registry.insert(id, Arc::clone(&session));

        let keys: Vec<AttachmentKey> = [PaneId::new(), PaneId::new()]
            .into_iter()
            .map(|pane| AttachmentKey {
                session: id,
                window: "main".into(),
                pane: Some(pane),
            })
            .collect();
        for key in &keys {
            registry.record_attachment(key.clone(), session.attach(null_sink()));
        }

        let gone = registry
            .take_attachment(&keys[0])
            .expect("the closing pane had an attachment");
        session.detach(gone);

        assert_eq!(session.sink_count(), 1);
        assert!(registry.attachment(&keys[0]).is_none());
        assert!(registry.attachment(&keys[1]).is_some());

        session.kill();
    }
}
