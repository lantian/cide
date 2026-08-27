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
    /// The highest write sequence number applied per writer. See [`Self::accept_write`].
    applied_write: DashMap<WriterKey, u64>,
    /// When each live session's child was forked. See [`Self::started`].
    started: DashMap<SessionId, std::time::SystemTime>,
}

/// Who minted a write sequence number.
///
/// **The window is in the key and it has to be.** The counter lives in
/// `ui/src/ipc/client.ts`, which is per *webview*: every window runs its own copy of the
/// module and starts counting at 1. Keyed on the session alone, a second webview writing to
/// a session someone has already typed into arrives with numbers below the watermark and
/// every one of them is discarded as a replay — silently, because a duplicate is `Ok(())`.
///
/// That is not hypothetical. `session.attach`'s own doc says a detach into a new window
/// attaches the new pane *before* detaching the old one, so the two webviews genuinely share
/// a session; mirroring does the same on purpose. Tearing out a pane the user had typed 40
/// characters into would have eaten their next 40 keystrokes.
///
/// Deduplication is still exact, because the retry it defends against is Tauri re-sending
/// the *same message from the same page* — same window, same counter.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct WriterKey {
    session: SessionId,
    /// The window label, as `tauri::Window::label`. One JS context, one counter.
    window: String,
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
        // Stamped on every insert, restarts included — `claude.restart` forks a new child under
        // the same [`SessionId`], and the whole point of the stamp is that the new one *has*
        // read whatever was on disk by then. A registry that kept the first time would go on
        // refusing a conversation the user had already restarted.
        self.started.insert(id, std::time::SystemTime::now());
        self.sessions.insert(id, session);
    }

    /// When this session's current child was forked, wall-clock.
    ///
    /// # What reads it, and why it is wall-clock rather than an `Instant`
    ///
    /// `spec_run_command`, to compare against the mtime of the skill file it is about to type
    /// the name of. Claude Code reads a project's skills and slash commands **once, at
    /// startup**, so a conversation older than the file cannot know the command and typing it
    /// answers `Unknown command` — which is what enabling OpenSpec from the panel did to the
    /// console pane that came up with the window. The comparison is against a file's `mtime`,
    /// which is a `SystemTime`, so this is one too; an `Instant` cannot be compared with one at
    /// all.
    ///
    /// A clock the user moves backwards makes this answer *older than the file* and cide refuses
    /// a conversation that would have worked. That is the safe direction — the refusal names the
    /// restart, and a restart fixes it either way.
    pub fn started(&self, id: SessionId) -> Option<std::time::SystemTime> {
        self.started.get(&id).map(|at| *at)
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
        // The dedupe watermarks go with the session — one per window that ever wrote to it.
        // A retry arriving after this point is rejected by `session_write`'s registry lookup
        // long before it reaches a watermark, so keeping them would only be dead entries.
        self.applied_write.retain(|k, _| k.session != id);
        self.started.remove(&id);
        self.sessions.remove(&id).map(|(_, v)| v)
    }

    /// Whether a write bearing this sequence number should be applied.
    ///
    /// **Tauri's IPC is at-least-once, and `session_write` is the one command where that is
    /// visible to the user.** `tauri-2.11.5/scripts/ipc-protocol.js` attaches its rejection
    /// handler as the second argument of the second `.then`, so it also catches a failure of
    /// `response.json()` / `arrayBuffer()` — a rejection raised *after* the Rust command has
    /// already run. It then sets `customProtocolIpcFailed = true` permanently and re-sends the
    /// identical message over `postMessage`. For a command whose entire effect is a side
    /// effect, replaying it types the character twice.
    ///
    /// Sequence numbers are per `(session, window)` — see [`WriterKey`] for why the window is
    /// not optional — and strictly increasing, which is sound because writes from one page
    /// are totally ordered end to end: `invoke` reaches `__TAURI_INTERNALS__.invoke` with no
    /// await, the `ipc` scheme handler runs `webview.on_message` inline on the GTK main loop,
    /// `session_write` is synchronous so `tauri-macros` uses `body_blocking` rather than
    /// spawning, and `PtySession::write` is a non-blocking send into a single mpsc drained by
    /// one writer thread. Nothing can arrive out of order, so "already seen" is decidable from
    /// one watermark instead of a window.
    ///
    /// `None` means the caller did not number its write, and is accepted: the old wire shape
    /// still works, with the old at-least-once behaviour. Rejecting it would break every
    /// caller outside `ui/src/ipc/client.ts` — `cide-headless`, and anything driving the app
    /// from a script — to protect a case they do not hit.
    pub fn accept_write(&self, id: SessionId, window: &str, seq: Option<u64>) -> bool {
        let Some(seq) = seq else { return true };
        let key = WriterKey {
            session: id,
            window: window.to_string(),
        };
        let mut watermark = self.applied_write.entry(key).or_insert(0);
        if seq <= *watermark {
            return false;
        }
        *watermark = seq;
        true
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

    /// The two window labels the tests below use. Real ones, in shape: `WindowLabel::shell()`
    /// and the `pane:<uuid>` a detached pane gets.
    const SHELL: &str = "shell";
    const DETACHED: &str = "pane:4f0e6f6a-0000-4000-8000-000000000001";

    /// **Tearing a pane out into its own window must not eat the keystrokes typed before it.**
    ///
    /// The sequence counter lives in `ui/src/ipc/client.ts`, which is per webview: the new
    /// window starts at 1 while the session's watermark is already at whatever the old window
    /// reached. And the two really do overlap — `session.attach` attaches the detached pane's
    /// sink *before* the original detaches, so this is the ordinary detach path, not a corner.
    ///
    /// Keyed on the session alone this test fails on its first assertion after the detach, and
    /// fails silently in the app: a dropped write is `Ok(())`, so the user sees keystrokes
    /// vanish with nothing in the log.
    #[test]
    fn a_second_window_writing_to_one_session_starts_its_own_count() {
        let registry = SessionRegistry::default();
        let id = SessionId::new();

        // Forty characters typed into the pane while it lived in the shell window.
        for seq in 1..=40 {
            assert!(registry.accept_write(id, SHELL, Some(seq)));
        }

        // The pane is torn out. Same session, new webview, counter back at 1.
        assert!(
            registry.accept_write(id, DETACHED, Some(1)),
            "the detached window's first keystroke must reach the child"
        );
        assert!(registry.accept_write(id, DETACHED, Some(2)));

        // The retry guard still works inside each window.
        assert!(
            !registry.accept_write(id, DETACHED, Some(2)),
            "and only once"
        );
        assert!(
            !registry.accept_write(id, SHELL, Some(40)),
            "the original window's watermark is untouched by the new one"
        );
        assert!(registry.accept_write(id, SHELL, Some(41)));
    }

    /// Removing a session clears every window's watermark for it, not just one.
    ///
    /// `retain` rather than `remove`: there is an entry per window that ever wrote to the
    /// session, and leaving the others behind would be a slow leak keyed on a session id that
    /// can never be issued again.
    #[test]
    fn removing_a_session_drops_all_of_its_watermarks() {
        let registry = SessionRegistry::default();
        let id = SessionId::new();

        assert!(registry.accept_write(id, SHELL, Some(5)));
        assert!(registry.accept_write(id, DETACHED, Some(5)));
        assert_eq!(registry.applied_write.len(), 2);

        registry.remove(id);
        assert_eq!(
            registry.applied_write.len(),
            0,
            "both windows' watermarks go with the session"
        );
    }

    /// A replayed write is refused, which is the whole point of the sequence number.
    ///
    /// The replay is not hypothetical: Tauri's own `ipc-protocol.js` re-sends the identical
    /// message after a body-decode failure that happens *after* the command has run. For
    /// `session_write` that is a keystroke typed twice — the reported `pwwdwd` shape, from a
    /// completely different cause than the input path.
    #[test]
    fn a_replayed_write_is_applied_once() {
        let registry = SessionRegistry::default();
        let id = SessionId::new();

        assert!(
            registry.accept_write(id, SHELL, Some(1)),
            "the first write lands"
        );
        assert!(
            !registry.accept_write(id, SHELL, Some(1)),
            "the retry of that same write must not reach the child"
        );
        assert!(
            registry.accept_write(id, SHELL, Some(2)),
            "the next write lands"
        );
    }

    /// Out-of-order numbers are refused too, not merely equal ones.
    ///
    /// Writes are totally ordered end to end (see `accept_write`), so a lower number arriving
    /// later can only be a replay of something already applied. Accepting it because it is
    /// "different" would reintroduce the duplicate through the door the fix closed.
    #[test]
    fn a_write_older_than_the_watermark_is_refused() {
        let registry = SessionRegistry::default();
        let id = SessionId::new();

        assert!(registry.accept_write(id, SHELL, Some(7)));
        assert!(!registry.accept_write(id, SHELL, Some(3)));
        assert!(!registry.accept_write(id, SHELL, Some(7)));
        assert!(registry.accept_write(id, SHELL, Some(8)));
    }

    /// Two sessions do not share a watermark.
    ///
    /// Panes mirroring one session *in one window* share a counter on the frontend and must;
    /// two *different* sessions must not, or the second one's first keystroke is dropped as a
    /// replay of the first one's.
    #[test]
    fn sequence_numbers_are_per_session() {
        let registry = SessionRegistry::default();
        let one = SessionId::new();
        let two = SessionId::new();

        assert!(registry.accept_write(one, SHELL, Some(1)));
        assert!(
            registry.accept_write(two, SHELL, Some(1)),
            "a different session starts its own count"
        );
    }

    /// A caller that numbers nothing keeps the old behaviour rather than being blocked.
    ///
    /// `cide-headless` and anything scripting the app send no `seq`, and rejecting them to
    /// protect a retry path they do not use would be the fix breaking working callers.
    #[test]
    fn an_unnumbered_write_is_always_applied() {
        let registry = SessionRegistry::default();
        let id = SessionId::new();

        assert!(registry.accept_write(id, SHELL, None));
        assert!(registry.accept_write(id, SHELL, None));
        // And it does not disturb a numbered stream that is also running.
        assert!(registry.accept_write(id, SHELL, Some(1)));
        assert!(!registry.accept_write(id, SHELL, Some(1)));
    }
}
