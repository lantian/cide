//! Experimental pair delivery. Every route is pinned to a live pair and recipient session.
use crate::{state::SessionRegistry, workspace_state::WorkspaceState};
use cide_core::{peer_chat as domain, peer_composer as composer, workspace};
use cide_ipc::{ConsoleHarness, PaneId, PeerChatReceipt, ProjectId, SessionId, TabId};
use cide_pty::PtySession;
use dashmap::{DashMap, DashSet};
use parking_lot::Mutex;
use std::{
    collections::VecDeque,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};

pub(crate) const TOOL: &str = "cide_peer_chat_send";

/// A mobile prompt is one transaction with the same gate as desktop keystrokes.
pub(crate) fn submit_mobile(
    app: &AppHandle,
    session: SessionId,
    pty: &PtySession,
    harness: cide_ipc::Harness,
    text: &str,
) -> Result<(), String> {
    let runtime = app.state::<Runtime>();
    let lane = runtime.lane(session);
    {
        let mut held = lane.lock();
        if held.writing {
            return Err("Another message is being delivered. Try again after it finishes.".into());
        }
        if !cide_core::conversations::composer(harness, &pty.capture_screen())
            .is_some_and(|body| body.trim().is_empty())
        {
            return Err(
                "The desktop input contains a draft or menu. Finish it in Terminal first.".into(),
            );
        }
        held.writing = true;
    }
    struct Release<'a>(&'a Mutex<Lane>);
    impl Drop for Release<'_> {
        fn drop(&mut self) {
            self.0.lock().writing = false;
        }
    }
    let _release = Release(&lane);
    let capture = pty.capture_screen();
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    if text.contains('\n') && !capture.info.bracketed_paste {
        return Err(
            "This harness cannot safely accept a multiline paste here. Use Terminal.".into(),
        );
    }
    pty.write(cide_remote::keys::paste(&text, &capture.info));
    let deadline = Instant::now() + Duration::from_secs(3);
    // The paste and submit must reach distinct reads, and a long paste must be visible first.
    thread::sleep(Duration::from_millis(120));
    loop {
        if pty.has_exited() {
            return Err("Submission is uncertain because the harness stopped. Check history before resending.".into());
        }
        if matches!(
            crate::emit::last_session_states().get(&session),
            Some(cide_ipc::SessionState::AwaitingPermission | cide_ipc::SessionState::Paused)
        ) {
            return Err("Submission is uncertain because the session changed state. Check Terminal before resending.".into());
        }
        if cide_core::conversations::composer(harness, &pty.capture_screen())
            .is_some_and(|body| cide_core::conversations::submitted_body_matches(&body, &text))
        {
            break;
        }
        if Instant::now() >= deadline {
            return Err("The prompt was pasted, but submission could not be verified. Check Terminal before resending.".into());
        }
        thread::sleep(Duration::from_millis(30));
    }
    if let Some(hooks) = app.try_state::<crate::hooks::HookServer>() {
        hooks.note_codex_input(session, b"\r", pty);
    }
    pty.write(vec![b'\r']);
    Ok(())
}

#[derive(Default)]
pub(crate) struct Runtime {
    lanes: DashMap<SessionId, Arc<Mutex<Lane>>>,
    paired: DashSet<SessionId>,
    receipts: Mutex<VecDeque<(TabId, PeerChatReceipt)>>,
    consoles: DashMap<SessionId, Arc<crate::opencode_console::Console>>,
    codex_consoles: DashMap<SessionId, Arc<crate::codex_console::Console>>,
    prompt_armed: DashMap<SessionId, Instant>,
}
#[derive(Default)]
struct Lane {
    queue: VecDeque<Message>,
    running: bool,
    writing: bool,
    generation: u64,
}
struct Message {
    id: String,
    project: ProjectId,
    tab: TabId,
    sender: SessionId,
    pane: PaneId,
    target: SessionId,
    body: String,
    generation: u64,
}

impl Runtime {
    pub fn is_paired(&self, id: SessionId) -> bool {
        self.paired.contains(&id)
    }
    pub fn note_console(&self, id: SessionId, console: Arc<crate::opencode_console::Console>) {
        self.consoles.insert(id, console);
    }
    pub fn note_codex_console(&self, id: SessionId, console: Arc<crate::codex_console::Console>) {
        self.codex_consoles.insert(id, console);
    }
    fn lane(&self, id: SessionId) -> Arc<Mutex<Lane>> {
        self.lanes.entry(id).or_default().clone()
    }
    fn receipt(&self, m: &Message, status: &str, detail: &str) {
        let mut receipts = self.receipts.lock();
        if let Some((_, r)) = receipts.iter_mut().find(|(_, r)| r.id == m.id) {
            r.status = status.into();
            r.detail = detail.into();
        } else {
            receipts.push_back((
                m.tab,
                PeerChatReceipt {
                    id: m.id.clone(),
                    sender: m.pane,
                    status: status.into(),
                    detail: detail.into(),
                },
            ));
        }
        while receipts.len() > 128 {
            receipts.pop_front();
        }
    }
    pub fn receipts(&self, tab: TabId) -> Vec<PeerChatReceipt> {
        self.receipts
            .lock()
            .iter()
            .filter(|(t, _)| *t == tab)
            .map(|(_, r)| r.clone())
            .collect()
    }
    fn paused_error(&self, tab: TabId, starting: bool) -> String {
        if starting {
            return "Both agents are still starting; this message was not queued".into();
        }
        let mut detail = "Peer conversation is paused; this message was not queued. Resume conversation in the tab before sending it again.".to_string();
        if let Some(cause) = self
            .receipts(tab)
            .into_iter()
            .rev()
            .find(|r| matches!(r.status.as_str(), "failed" | "uncertain"))
        {
            detail.push_str(" Previous delivery problem: ");
            detail.push_str(&cause.detail);
        }
        detail
    }
    pub fn cancel(&self, sessions: &[SessionId]) {
        for id in sessions {
            if let Some(lane) = self.lanes.get(id) {
                lane.lock().generation += 1;
            }
        }
    }
    pub fn forget(&self, sessions: &[SessionId]) {
        self.cancel(sessions);
        for id in sessions {
            self.paired.remove(id);
            self.prompt_armed.remove(id);
        }
        for id in sessions {
            if let Some((_, console)) = self.consoles.remove(id) {
                console.stop();
            }
            if let Some((_, console)) = self.codex_consoles.remove(id) {
                console.stop();
            }
        }
    }
    /// Serialize manual writes against the small composer transaction, never the readiness wait.
    pub fn manual_write(
        &self,
        app: &AppHandle,
        session: SessionId,
        pty: &PtySession,
        data: Vec<u8>,
    ) -> Result<(), String> {
        let lane = self.lane(session);
        let guard = lane.lock();
        if guard.writing {
            return Err("A peer message is being delivered; input is briefly suspended. Please type again when delivery finishes.".into());
        }
        if let Some(hooks) = app.try_state::<crate::hooks::HookServer>() {
            hooks.note_codex_input(session, &data, pty);
        }
        pty.write(data);
        Ok(())
    }
}

pub(crate) fn send(
    app: &AppHandle,
    sender: SessionId,
    message: &str,
) -> Result<PeerChatReceipt, String> {
    let line = composer::normalize(message)?;
    let (project, tab, pane, target, label) = app.state::<WorkspaceState>().with(|ws| {
        let (project, tab, pane, chat) = domain::for_session(ws, sender)
            .ok_or("This session is no longer in a two-agent chat")?;
        if chat.paused || chat.starting {
            return Err(app.state::<Runtime>().paused_error(tab, chat.starting));
        }
        let other = if pane == chat.main {
            chat.peer
        } else {
            chat.main
        };
        let target = workspace::tab(ws, project, tab)
            .map_err(|e| e.to_string())?
            .tree
            .panes
            .get(&other)
            .and_then(|p| p.session)
            .ok_or("The peer has not started yet")?;
        Ok((
            project,
            tab,
            pane,
            target,
            if pane == chat.main {
                format!("Main ({})", chat.main_harness.program())
            } else {
                format!(
                    "Peer ({})",
                    chat.selection
                        .as_ref()
                        .ok_or("The peer is not configured")?
                        .harness
                        .program()
                )
            },
        ))
    })?;
    let runtime = app.state::<Runtime>();
    let lane = runtime.lane(target);
    let mut guard = lane.lock();
    if guard.queue.len() + usize::from(guard.running) >= 16 {
        return Err("The peer message queue is full".into());
    }
    let m = Message {
        id: SessionId::new().to_string(),
        project,
        tab,
        sender,
        pane,
        target,
        body: format!("Chat from {label}: {line}"),
        generation: guard.generation,
    };
    runtime.receipt(
        &m,
        "queued",
        "Waiting for the receiving agent to finish or leave an empty input",
    );
    let receipt = runtime
        .receipts(tab)
        .into_iter()
        .find(|r| r.id == m.id)
        .ok_or("Could not record peer message")?;
    guard.queue.push_back(m);
    if !guard.running {
        guard.running = true;
        let app = app.clone();
        let lane = lane.clone();
        thread::spawn(move || drain(app, lane));
    }
    Ok(receipt)
}

fn drain(app: AppHandle, lane: Arc<Mutex<Lane>>) {
    loop {
        let next = {
            let mut g = lane.lock();
            if g.queue.is_empty() {
                g.running = false;
                return;
            }
            g.queue.pop_front()
        };
        let Some(m) = next else {
            return;
        };
        let outcome = deliver(&app, &lane, &m);
        lane.lock().writing = false;
        let runtime = app.state::<Runtime>();
        match outcome {
            Ok(()) => runtime.receipt(
                &m,
                "delivered",
                "Accepted by the receiving agent; a reply is not guaranteed",
            ),
            Err((uncertain, detail)) => {
                let cancelled = lane.lock().generation != m.generation;
                runtime.receipt(
                    &m,
                    if uncertain {
                        "uncertain"
                    } else if cancelled {
                        "cancelled"
                    } else {
                        "failed"
                    },
                    &detail,
                );
                if !cancelled {
                    let sessions = app
                        .state::<WorkspaceState>()
                        .update(|ws| {
                            // Late failures from a retired child must not pause a replacement pair.
                            let Some((_, tab, pane, chat)) = domain::for_session(ws, m.sender)
                            else {
                                return Ok(Vec::new());
                            };
                            if tab != m.tab || pane != m.pane || chat.paused {
                                return Ok(Vec::new());
                            }
                            let ids = workspace::tab(ws, m.project, m.tab)?
                                .tree
                                .panes
                                .values()
                                .filter_map(|p| p.session)
                                .collect::<Vec<_>>();
                            domain::set_paused(ws, m.project, m.tab, true)?;
                            Ok(ids)
                        })
                        .unwrap_or_default();
                    runtime.cancel(&sessions);
                }
            }
        }
    }
}

fn target(app: &AppHandle, m: &Message) -> Result<(Arc<PtySession>, ConsoleHarness), String> {
    let harness = app.state::<WorkspaceState>().with(|ws| {
        let (_, tab, pane, chat) =
            domain::for_session(ws, m.sender).ok_or("The sender no longer belongs to this pair")?;
        if tab != m.tab || pane != m.pane || chat.paused {
            return Err("The pair closed, restarted, or paused".to_string());
        }
        let t = workspace::tab(ws, m.project, m.tab).map_err(|e| e.to_string())?;
        let other = if pane == chat.main {
            chat.peer
        } else {
            chat.main
        };
        if t.tree.panes.get(&other).and_then(|p| p.session) != Some(m.target) {
            return Err("The peer session changed; message cancelled".into());
        }
        Ok(if other == chat.main {
            chat.main_harness
        } else {
            chat.selection
                .as_ref()
                .ok_or("Peer is not configured")?
                .harness
        })
    })?;
    let pty = app
        .state::<SessionRegistry>()
        .get(m.target)
        .filter(|s| !s.has_exited())
        .ok_or("The peer process exited")?;
    Ok((pty, harness))
}

/// A busy receiver is normal conversation flow, not a transport failure. Keep its
/// bounded queue pending until ready; cancellation and retired sessions are checked
/// on every poll. This wait never writes input or holds the manual input gate.
fn wait_for_receiver<T>(
    mut check: impl FnMut() -> Result<(), String>,
    mut claim: impl FnMut() -> Result<Option<T>, String>,
) -> Result<T, String> {
    loop {
        check()?;
        if let Some(receiver) = claim()? {
            return Ok(receiver);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn deliver(app: &AppHandle, lane: &Mutex<Lane>, m: &Message) -> Result<(), (bool, String)> {
    let check = || -> Result<(Arc<PtySession>, ConsoleHarness), String> {
        if lane.lock().generation != m.generation {
            return Err("Pending message cancelled".into());
        }
        target(app, m)
    };
    let (pty, harness) = check().map_err(|e| (false, e))?;
    enum Receiver {
        Terminal,
        OpenCode(Arc<crate::opencode_console::Console>),
        Codex(Arc<crate::codex_console::Console>),
    }
    let receiver = wait_for_receiver(
        || check().map(|_| ()),
        || {
            if harness == ConsoleHarness::Opencode {
                let runtime = app.state::<Runtime>();
                let console = runtime
                    .consoles
                    .get(&m.target)
                    .map(|c| c.clone())
                    .ok_or("OpenCode console is no longer available")?;
                if console.peer_ready()? {
                    lane.lock().writing = true;
                    runtime.receipt(m, "writing", "Delivering to OpenCode");
                    return Ok(Some(Receiver::OpenCode(console)));
                }
            } else if harness == ConsoleHarness::Codex {
                let console = app.state::<Runtime>().codex_consoles.get(&m.target)
                    .map(|c| c.clone()).ok_or("Codex native delivery is unavailable. Restart this pair with an updated Codex CLI.")?;
                if console.receiver()?.is_some() {
                    return Ok(Some(Receiver::Codex(console)));
                }
            } else {
                // The readiness wait never holds the manual input gate. Once empty,
                // claim it under the same lock used by manual writes.
                let mut g = lane.lock();
                if composer::composer(harness, &pty.capture_screen())
                    .is_some_and(|(text, col)| col == 2 && composer::empty(harness, &text))
                {
                    g.writing = true;
                    drop(g);
                    app.state::<Runtime>().receipt(
                        m,
                        "writing",
                        "Delivering peer message; input briefly suspended",
                    );
                    return Ok(Some(Receiver::Terminal));
                }
            }
            Ok(None)
        },
    )
    .map_err(|e| (false, e))?;
    if let Receiver::OpenCode(console) = receiver {
        check().map_err(|e| (false, e))?;
        return console.peer_prompt(&m.body).map_err(|e| {
            (
                true,
                format!("OpenCode submission is uncertain; do not resend: {e}"),
            )
        });
    }
    if let Receiver::Codex(console) = receiver {
        loop {
            check().map_err(|e| (false, e))?;
            if let Some(receiver) = console.receiver().map_err(|e| (false, e))? {
                // The forwarding loop rechecks readiness alongside native user requests.
                // A manual submit that wins the race leaves this message queued, untouched.
                app.state::<Runtime>()
                    .receipt(m, "writing", "Submitting directly to Codex");
                let delivery_app = app.clone();
                let lane = app.state::<Runtime>().lane(m.target);
                let pinned = Message {
                    id: m.id.clone(),
                    project: m.project,
                    tab: m.tab,
                    sender: m.sender,
                    pane: m.pane,
                    target: m.target,
                    body: String::new(),
                    generation: m.generation,
                };
                let valid = Box::new(move || {
                    if lane.lock().generation != pinned.generation {
                        return Err("Pending message cancelled".into());
                    }
                    target(&delivery_app, &pinned).map(|_| ())
                });
                match console.peer_prompt(receiver, m.body.clone(), valid)? {
                    true => return Ok(()),
                    false => app.state::<Runtime>().receipt(
                        m,
                        "queued",
                        "Waiting for Codex to finish its current turn",
                    ),
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    let mut marker = format!("[cide:{}]", SessionId::new());
    while m.body.contains(&marker) {
        marker = format!("[cide:{}]", SessionId::new());
    }
    let mut witness_bytes = 0;
    for (input, marked, witness) in composer::marked_chunks(&m.body, &marker) {
        witness_bytes = witness - marker.len();
        check().map_err(|e| (true, e))?;
        pty.write(input);
        wait_body(
            app,
            m,
            harness,
            &marked,
            witness,
            Duration::from_millis(100),
        )?;
    }
    check().map_err(|e| (true, e))?;
    pty.write(vec![127; marker.len()]);
    wait_body(
        app,
        m,
        harness,
        &m.body,
        witness_bytes,
        Duration::from_millis(500),
    )?;
    check().map_err(|e| (true, e))?;
    // Submission is a separate transaction. After this point an error can mean accepted.
    pty.write(vec![b'\r']);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        check().map_err(|e| (true, e))?;
        if composer::composer(harness, &pty.capture_screen())
            .is_some_and(|(text, col)| col == 2 && composer::empty(harness, &text))
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err((
                true,
                "Submission was not confirmed. Do not resend; inspect the receiving pane.".into(),
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_body(
    app: &AppHandle,
    m: &Message,
    harness: ConsoleHarness,
    expected: &str,
    witness_bytes: usize,
    settle: Duration,
) -> Result<(), (bool, String)> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut stable = None;
    loop {
        let (pty, _) = target(app, m).map_err(|e| (true, e))?;
        let matches =
            composer::matches_capture(harness, &pty.capture_screen(), expected, witness_bytes);
        if matches {
            let since = stable.get_or_insert_with(Instant::now);
            if since.elapsed() >= settle {
                return Ok(());
            }
        } else {
            stable = None;
        }
        if Instant::now() >= deadline {
            return Err((true, "Peer composer verification failed; submit withheld. Inspect and clear the receiving composer before resuming.".into()));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Only a non-empty normal composer submit on the main side resumes a restored pair.
pub(crate) fn note_input(app: &AppHandle, session: SessionId, data: &str, pty: &PtySession) {
    // Native Codex confirms an actual user turn over its protocol. A newline in a
    // permission dialog or draft is never evidence that a task was submitted.
    if app.state::<Runtime>().codex_consoles.contains_key(&session) {
        return;
    }
    if !data.contains('\r') {
        return;
    }
    let resume = app.state::<WorkspaceState>().with(|ws| {
        let (project, tab, pane, chat) = domain::for_session(ws, session)?;
        if pane != chat.main || !chat.paused {
            return None;
        }
        if chat.main_harness == ConsoleHarness::Opencode {
            if app
                .try_state::<crate::hooks::HookServer>()
                .is_some_and(|h| {
                    matches!(
                        h.state(session),
                        cide_ipc::SessionState::Idle | cide_ipc::SessionState::AwaitingInput
                    )
                })
            {
                app.state::<Runtime>()
                    .prompt_armed
                    .insert(session, Instant::now());
            }
            return None;
        }
        let (text, _) = composer::composer(chat.main_harness, &pty.capture_screen())?;
        (!composer::empty(chat.main_harness, &text) && !text.trim().is_empty())
            .then_some((project, tab))
    });
    if let Some((project, tab)) = resume {
        let _ = app
            .state::<WorkspaceState>()
            .update(|ws| domain::set_paused(ws, project, tab, false));
    }
}

/// Only an acknowledged native user turn on the writer side resumes the pair.
pub(crate) fn note_codex_prompt(app: &AppHandle, session: SessionId) {
    let _ = app.state::<WorkspaceState>().update(|ws| {
        if let Some((project, tab, pane, chat)) = domain::for_session(ws, session)
            && pane == chat.main
            && chat.paused
            && !chat.starting
        {
            domain::set_paused(ws, project, tab, false)?;
        }
        Ok(())
    });
}

pub(crate) fn conversation_changed(app: &AppHandle, session: SessionId) {
    let sessions = app
        .state::<WorkspaceState>()
        .update(|ws| {
            let Some((project, tab, _, _)) = domain::for_session(ws, session) else {
                return Ok(Vec::new());
            };
            let ids = workspace::tab(ws, project, tab)?
                .tree
                .panes
                .values()
                .filter_map(|p| p.session)
                .collect::<Vec<_>>();
            domain::set_paused(ws, project, tab, true)?;
            Ok(ids)
        })
        .unwrap_or_default();
    app.state::<Runtime>().cancel(&sessions);
}

/// Native OpenCode user-message events are evidence of a new task, not a dialog answer.
pub(crate) fn note_native_prompt(app: &AppHandle, session: SessionId) {
    if app
        .state::<Runtime>()
        .prompt_armed
        .remove(&session)
        .is_none_or(|(_, since)| since.elapsed() >= Duration::from_secs(5))
    {
        return;
    }
    let state = app.state::<WorkspaceState>();
    let _ = state.update(|ws| {
        let Some((project, tab, pane, chat)) = domain::for_session(ws, session) else {
            return Ok(());
        };
        if pane == chat.main && chat.paused && !chat.starting {
            domain::set_paused(ws, project, tab, false)?;
        }
        Ok(())
    });
}

/// Bind before the MCP bridge boots. On failed startup restore the previous transcript identity.
pub(crate) struct Binding {
    app: AppHandle,
    project: ProjectId,
    tab: TabId,
    pane: cide_ipc::Pane,
    session: SessionId,
    committed: bool,
}
impl Binding {
    pub fn start(
        app: &AppHandle,
        pane: PaneId,
        session: SessionId,
        harness: ConsoleHarness,
    ) -> Result<Self, String> {
        let state = app.state::<WorkspaceState>();
        let (project, tab, previous, retired) = state
            .update(|ws| {
                let (project, tab, _) = domain::for_pane(ws, pane).ok_or_else(|| {
                    cide_core::CoreError::Invariant("The pair closed during startup".into())
                })?;
                let previous = workspace::tab(ws, project, tab)?.tree.panes[&pane].clone();
                if previous.session.is_some_and(|id| {
                    app.state::<SessionRegistry>()
                        .get(id)
                        .is_some_and(|s| !s.has_exited())
                }) {
                    return Err(cide_core::CoreError::Invariant(
                        "This paired panel already has a live session".into(),
                    ));
                }
                let retired = workspace::tab(ws, project, tab)?
                    .tree
                    .panes
                    .values()
                    .filter_map(|p| p.session)
                    .collect::<Vec<_>>();
                workspace::bind_session(ws, project, tab, pane, session, Some(harness.harness()))?;
                domain::set_paused(ws, project, tab, true)?;
                Ok((project, tab, previous, retired))
            })
            .map_err(|e| e.to_string())?;
        app.state::<Runtime>().cancel(&retired);
        app.state::<Runtime>().paired.insert(session);
        Ok(Self {
            app: app.clone(),
            project,
            tab,
            pane: previous,
            session,
            committed: false,
        })
    }
    pub fn commit(&mut self) {
        self.committed = true;
    }
}
impl Drop for Binding {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        self.app.state::<Runtime>().paired.remove(&self.session);
        let _ = self.app.state::<WorkspaceState>().update(|ws| {
            if let Ok(t) = workspace::tab_mut(ws, self.project, self.tab)
                && let Some(p) = t.tree.panes.get_mut(&self.pane.id)
                && p.session == Some(self.session)
            {
                *p = self.pane.clone();
                if let Some(chat) = &mut t.peer_chat {
                    chat.paused = true;
                }
                workspace::bump(ws);
            }
            Ok(())
        });
    }
}

/// Capture descendants while the paired child still owns them; never sweep unrelated work.
pub(crate) fn stop_pair(app: &AppHandle, sessions: &[SessionId]) {
    app.state::<Runtime>().forget(sessions);
    for id in sessions {
        if let Some(pty) = app.state::<SessionRegistry>().get(*id) {
            let tree = pty
                .child_pid()
                .map(cide_core::process_tree::capture)
                .unwrap_or_default();
            pty.kill();
            let _ = std::thread::Builder::new()
                .name("cide-peer-stop".into())
                .spawn(move || {
                    tree.finish(
                        Duration::from_millis(500),
                        "a two-agent chat session".into(),
                    )
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pausing_cancels_a_wait_without_claiming_the_input() {
        let lane = Mutex::new(Lane::default());
        let mut polls = 0;
        let result = wait_for_receiver(
            || {
                if lane.lock().generation == 0 {
                    Ok(())
                } else {
                    Err("cancelled".into())
                }
            },
            || {
                assert!(
                    !lane.lock().writing,
                    "waiting must leave manual input usable"
                );
                polls += 1;
                if polls == 3 {
                    lane.lock().generation += 1;
                }
                Ok(None::<()>)
            },
        );
        assert_eq!(result.unwrap_err(), "cancelled");
        assert_eq!(polls, 3);
        assert!(!lane.lock().writing);
    }

    #[test]
    fn paused_send_identifies_the_prior_delivery_problem_and_no_queueing() {
        let runtime = Runtime::default();
        let tab = TabId::new();
        runtime.receipts.lock().push_back((
            tab,
            PeerChatReceipt {
                id: "failed".into(),
                sender: PaneId::new(),
                status: "uncertain".into(),
                detail: "Composer verification failed".into(),
            },
        ));
        runtime.receipts.lock().push_back((
            TabId::new(),
            PeerChatReceipt {
                id: "unrelated".into(),
                sender: PaneId::new(),
                status: "failed".into(),
                detail: "Another tab's problem".into(),
            },
        ));
        let error = runtime.paused_error(tab, false);
        assert!(error.contains("not queued"));
        assert!(error.contains("Resume conversation"));
        assert!(error.contains("Composer verification failed"));
        assert!(!error.contains("Another tab"));
        assert!(
            !runtime
                .paused_error(tab, true)
                .contains("Composer verification failed")
        );
        assert!(runtime.lanes.is_empty());
    }

    #[test]
    #[ignore = "quota-free; exercises a real 41-second receiver wait"]
    fn a_busy_receiver_can_take_longer_than_forty_seconds() {
        let started = Instant::now();
        let result = wait_for_receiver(
            || Ok(()),
            || Ok((started.elapsed() >= Duration::from_secs(41)).then_some("ready")),
        );
        assert_eq!(result.unwrap(), "ready");
        assert!(started.elapsed() >= Duration::from_secs(41));
    }
}
