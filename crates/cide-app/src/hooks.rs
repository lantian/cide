//! The unix socket every `claude` child's hooks report to.
//!
//! # Shape
//!
//! One listener per process, at `$XDG_RUNTIME_DIR/cide-hooks-<pid>.sock`. Every child is
//! spawned with `CIDE_HOOK_SOCK` pointing at it and an inline `--settings` registering
//! `cide-hook <Event>` for all ten hook points. A hook connects, writes one newline-delimited
//! JSON frame, and disconnects.
//!
//! # Connection per frame, on purpose
//!
//! Each hook invocation is a fresh short-lived process, so a persistent connection is not
//! available to it even in principle. That makes the accept loop trivial and — more usefully
//! — means a hook that dies mid-write costs one truncated line that this reader discards,
//! rather than desynchronising a stream that later frames share.
//!
//! # Blocking threads rather than the async runtime
//!
//! `ide.rs` already owns a tokio runtime for the MCP server, but this listener has no reason
//! to share it: the work per frame is a JSON parse and a map lookup, connections are
//! short-lived, and putting it on OS threads keeps a wedged hook from occupying a runtime
//! worker that the IDE server needs to answer a blocked `openDiff`.
//!
//! # Read in parallel, apply in order
//!
//! Reading is per connection, so a hook that connects and never writes blocks only itself.
//! **Applying is not.** Every frame goes down one channel to one thread, and that is a
//! correctness requirement rather than a simplification:
//!
//! * [`decide`] reads a session's current state and then writes the next one. Two threads
//!   doing that concurrently can both read `Busy`, and whichever writes second wins whatever
//!   it happened to read — so a transition can be computed from a state that no longer holds.
//! * Even with that made atomic, order would still decide the answer. A `Stop` and a
//!   `PostToolUse` are separate short-lived `cide-hook` processes writing to separate sockets;
//!   with a thread each, the OS decides which is applied first. `PostToolUse` applied *after*
//!   `Stop` takes the session from `Idle` back to `Busy` (`cide_claude::next_state` records
//!   tool traffic as liveness on purpose, so a session wrongly marked idle mid-turn corrects
//!   itself), which raises the awaiting marker and then clears it again with nobody at the
//!   keyboard. The user comes back to a task bar that says nothing is waiting.
//!
//! The window is small and the failure is silent and unreproducible, which is the combination
//! that keeps a bug like this alive. One channel closes it for the cost of one thread.

use std::io::{BufRead, BufReader};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use cide_claude::{HookEvent, HookFrame};
use cide_ipc::{SessionId, SessionState};
use dashmap::DashMap;
use tauri::AppHandle;

/// The socket, and every session's last known state.
pub struct HookServer {
    path: PathBuf,
    /// Last known state per session. The state machine needs a "current" to transition from,
    /// and a session with no entry yet is treated as `Spawning`.
    states: Arc<DashMap<SessionId, SessionState>>,
}

impl HookServer {
    /// Bind the socket and start accepting.
    ///
    /// Failure costs hooks — no live token figures, no fast buffer reload, and a close
    /// confirm that cannot tell busy from idle — but not the terminal. Claude Code runs
    /// perfectly well with no hooks configured, so this degrades rather than refusing.
    pub fn start(app: AppHandle) -> std::io::Result<Self> {
        let path = socket_path();

        // A socket left by a previous run of this same pid would make `bind` fail with
        // AddrInUse. Removing it is safe: the path carries our pid, so nothing else owns it.
        let _ = std::fs::remove_file(&path);

        let listener = UnixListener::bind(&path)?;
        // The socket is a control channel into this process. 0700 on the parent directory is
        // not guaranteed (XDG_RUNTIME_DIR usually is, /tmp is not), so the socket itself is
        // narrowed rather than trusted to inherit.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }

        let states: Arc<DashMap<SessionId, SessionState>> = Arc::new(DashMap::new());
        let apply_states = Arc::clone(&states);

        // The serialising channel. Unbounded, and deliberately: a hook process is blocked on
        // its own `write` until this side reads the line, so back-pressure here would be
        // back-pressure on `claude` itself — a slow `emit` would stall the CLI mid-turn.
        // Frames are two small strings and a `Value`, and they arrive at the rate a human
        // prompts, not at the rate a terminal prints.
        let (frames, inbox) = std::sync::mpsc::channel::<HookFrame>();

        thread::Builder::new()
            .name("cide-hook-apply".into())
            .spawn(move || {
                // Ends when every sender has gone, which is when the accept loop below has
                // ended and no connection thread is left holding a clone.
                for frame in inbox {
                    apply(&frame, &app, &apply_states);
                }
            })?;

        thread::Builder::new()
            .name("cide-hook-accept".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let frames = frames.clone();
                    // One thread per connection, each one short-lived, and each one *reading*
                    // only. A hook that connects and never writes therefore blocks only
                    // itself — while the frames that do arrive are applied in the order they
                    // were parsed, by the one thread above. See the module comment.
                    let _ = thread::Builder::new()
                        .name("cide-hook-conn".into())
                        .spawn(move || handle(stream, &frames));
                }
            })?;

        tracing::info!(path = %path.display(), "hook socket listening");
        Ok(Self { path, states })
    }

    /// The value for a child's `CIDE_HOOK_SOCK`.
    pub fn socket(&self) -> &std::path::Path {
        &self.path
    }

    pub fn state(&self, session: SessionId) -> SessionState {
        self.states
            .get(&session)
            .map(|s| *s)
            .unwrap_or(SessionState::Spawning)
    }

    /// Sessions the user would mind interrupting.
    ///
    /// `Busy | AwaitingPermission`, not "a process exists" — see `cide_claude::state`. This
    /// is what the close confirm asks, and a definition that warned about every running
    /// session would train people to dismiss the dialog.
    pub fn live_sessions(&self) -> Vec<SessionId> {
        live_in(&self.states)
    }

    /// Forget a session that has gone.
    pub fn forget(&self, session: SessionId) {
        forget_in(&self.states, session);
    }
}

/// The live set of a state map. See [`decide`] for why this is a free function.
///
/// `HookServer` cannot be built without an `AppHandle`, and `tauri`'s mock app is behind a
/// feature this build does not enable — so a test can reach a *method* on the server only by
/// not testing it. The methods above are each one line so that the part which can be wrong
/// lives here, where `forgetting_a_session_takes_it_out_of_the_live_set` can call it.
///
/// The alternative that lost was leaving both as methods and asserting on a bare `DashMap`
/// in the test instead. That is what the test did, and it passed with `forget` gutted to a
/// no-op: asserting that `DashMap::remove` removes says nothing about the code that ships.
fn live_in(states: &DashMap<SessionId, SessionState>) -> Vec<SessionId> {
    states
        .iter()
        .filter(|e| e.value().is_live())
        .map(|e| *e.key())
        .collect()
}

/// Drop a session from a state map. See [`live_in`] for why this is a free function.
fn forget_in(states: &DashMap<SessionId, SessionState>, session: SessionId) {
    states.remove(&session);
}

impl Drop for HookServer {
    fn drop(&mut self) {
        // A socket file outliving its process is a path children would connect to and write
        // into with nothing on the other end.
        let _ = std::fs::remove_file(&self.path);
    }
}

fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join(format!("cide-hooks-{}.sock", std::process::id()))
}

/// Read one connection's frames and hand them to the applier, in the order they parse.
///
/// Parsing stays here, on the connection's own thread, so a malformed or enormous payload
/// costs that hook and nothing else. Only the decision is serialised.
fn handle(stream: UnixStream, frames: &std::sync::mpsc::Sender<HookFrame>) {
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        // A frame we cannot parse is dropped with a note rather than killing the connection:
        // a CLI release that changes a payload should cost one event, not the hook channel.
        match serde_json::from_str::<HookFrame>(&line) {
            // A send that fails means the applier thread has gone, which only happens on the
            // way out. Nothing to report and nowhere left to report it.
            Ok(frame) => {
                let _ = frames.send(frame);
            }
            Err(error) => tracing::debug!(%error, "unparseable hook frame"),
        }
    }
}

/// What one frame means for the frontend.
///
/// Routing and emitting used to be the same function, which is why the only test in this
/// module asserted that a fresh `DashMap` is empty: every branch of the decision needed an
/// `AppHandle`, and `tauri`'s mock app lives behind a feature this build does not enable.
/// Splitting the decision out makes all four branches reachable from a unit test, and leaves
/// [`apply`] with nothing in it that can be wrong.
#[derive(Debug, Clone, PartialEq)]
enum Effect {
    /// The statusline's own JSON, forwarded whole.
    Status {
        session: String,
        payload: serde_json::Value,
    },
    /// Files a tool touched — the fast buffer-reload path.
    Tool { session: String, paths: Vec<String> },
    /// A state transition the frontend should hear about.
    State {
        session: String,
        state: SessionState,
    },
}

/// Route one frame: update state, emit what the frontend needs.
fn apply(frame: &HookFrame, app: &AppHandle, states: &DashMap<SessionId, SessionState>) {
    for effect in decide(frame, states) {
        match effect {
            Effect::Status { session, payload } => {
                crate::emit::session_status(app, &session, payload)
            }
            Effect::Tool { session, paths } => crate::emit::session_tool(app, &session, paths),
            Effect::State { session, state } => crate::emit::session_state(app, &session, state),
        }
    }
}

/// Decide what a frame means, recording any transition in `states`.
///
/// Pure but for that one write, which is the point: everything a hook can be — an unknown
/// event, an unknown session, a statusline, a tool call, a permission prompt — is decided
/// here where a test can see it.
fn decide(frame: &HookFrame, states: &DashMap<SessionId, SessionState>) -> Vec<Effect> {
    // The statusline is not a hook event and carries no `session_id` in the same shape; it is
    // handled first so it does not fall through the state machine.
    if frame.event == "statusline" {
        return match frame.session_id() {
            Some(session) => vec![Effect::Status {
                session: session.to_owned(),
                payload: frame.payload.clone(),
            }],
            None => Vec::new(),
        };
    }

    let Some(event) = frame.kind() else {
        tracing::debug!(event = %frame.event, "hook event this build does not know");
        return Vec::new();
    };

    let Some(raw) = frame.session_id() else {
        return Vec::new();
    };
    let Ok(session) = raw.parse::<SessionId>() else {
        // A session uuid we did not mint. Reachable when a user runs `claude` by hand inside
        // a cide shell: it inherits `CIDE_HOOK_SOCK` and reports here, but no pane owns it.
        tracing::debug!(session = raw, "hook from a session this app does not own");
        return Vec::new();
    };

    let mut effects = Vec::new();

    // Files first, so a reload is not held up behind a state transition that may not happen.
    if matches!(event, HookEvent::PostToolUse | HookEvent::PostToolBatch) {
        effects.push(Effect::Tool {
            session: raw.to_owned(),
            paths: frame.touched_paths(),
        });
    }

    let current = states
        .get(&session)
        .map(|s| *s)
        .unwrap_or(SessionState::Spawning);

    // A permission request is a `Notification` whose text says so; the state machine cannot
    // see the payload, so that one case is decided here.
    let next = if event == HookEvent::Notification {
        cide_claude::is_permission_request(&frame.payload)
            .then_some(SessionState::AwaitingPermission)
            .filter(|s| *s != current)
    } else {
        cide_claude::next_state(current, event)
    };

    if let Some(next) = next {
        states.insert(session, next);
        // The first of three lines that make the attention chain traceable end to end.
        //
        // Three reports of "the finished-turn notification does nothing" have each been chased
        // by reading code, and each time every link read correctly. The chain crosses a process
        // boundary the log cannot see into — hook -> here -> the webview -> back through
        // `window_set_awaiting` -> `retitle` — and a successful frame logged nothing at all, so
        // silence proved nothing about where it stopped. These three say which link is quiet:
        // no line here means the hooks are not arriving; a line here with none from
        // `window_set_awaiting` means the webview is not hearing the event or not reporting it;
        // both, with a zero count in `retitle`, means the window-to-session mapping is wrong.
        tracing::info!(session = raw, ?next, "hook: session state changed");
        effects.push(Effect::State {
            session: raw.to_owned(),
            state: next,
        });
    }

    effects
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    type States = DashMap<SessionId, SessionState>;

    /// A frame as the CLI sends it: an event name and a payload naming a session.
    fn frame(event: &str, session: &str) -> HookFrame {
        HookFrame::new(event, json!({ "session_id": session }))
    }

    fn state_of(states: &States, session: SessionId) -> Option<SessionState> {
        states.get(&session).map(|s| *s)
    }

    #[test]
    fn the_socket_path_names_this_process() {
        // Two cide processes must not fight over one socket, and a stale socket from a
        // previous run must be identifiable as removable.
        let p = socket_path();
        let name = p
            .file_name()
            .expect("has a name")
            .to_string_lossy()
            .into_owned();
        assert!(name.contains(&std::process::id().to_string()));
        assert!(name.ends_with(".sock"));
    }

    #[test]
    fn a_statusline_frame_is_forwarded_whole_and_never_enters_the_state_machine() {
        // The statusline is not a hook event. Routed through `kind()` it would parse as
        // nothing and be dropped, and the status bar's token and cost figures — which have
        // no other source — would silently stop arriving.
        let states = States::new();
        let session = SessionId::new();
        let payload = json!({
            "session_id": session.to_string(),
            "model": { "display_name": "Sonnet" },
            "cost": { "total_cost_usd": 0.42 },
        });

        let effects = decide(&HookFrame::new("statusline", payload.clone()), &states);

        assert_eq!(
            effects,
            vec![Effect::Status {
                session: session.to_string(),
                payload,
            }],
            "the payload is forwarded untouched, not destructured"
        );
        assert!(
            states.is_empty(),
            "a statusline says nothing about whether the session is busy"
        );
    }

    #[test]
    fn a_statusline_with_no_session_is_dropped() {
        let states = States::new();
        assert!(decide(&HookFrame::new("statusline", json!({})), &states).is_empty());
    }

    #[test]
    fn a_turn_starts_and_ends_on_the_session_that_reported_it() {
        // The per-session claim: two panes are two conversations, and a prompt in one must
        // not make the other look busy to the close confirm.
        let states = States::new();
        let busy = SessionId::new();
        let idle = SessionId::new();
        states.insert(idle, SessionState::Idle);

        let effects = decide(&frame("UserPromptSubmit", &busy.to_string()), &states);
        assert_eq!(
            effects,
            vec![Effect::State {
                session: busy.to_string(),
                state: SessionState::Busy,
            }]
        );
        assert_eq!(state_of(&states, busy), Some(SessionState::Busy));
        assert_eq!(
            state_of(&states, idle),
            Some(SessionState::Idle),
            "the other session was not touched"
        );

        let effects = decide(&frame("Stop", &busy.to_string()), &states);
        assert_eq!(
            effects,
            vec![Effect::State {
                session: busy.to_string(),
                state: SessionState::Idle,
            }]
        );
        assert_eq!(state_of(&states, busy), Some(SessionState::Idle));
    }

    #[test]
    fn a_subagent_stopping_leaves_the_turn_running() {
        // The transition that must *not* happen. Marking a session idle when a subagent
        // finishes means closing a project mid-task does not warn — the moment the warning
        // matters most.
        let states = States::new();
        let session = SessionId::new();
        states.insert(session, SessionState::Busy);

        assert!(decide(&frame("SubagentStop", &session.to_string()), &states).is_empty());
        assert_eq!(state_of(&states, session), Some(SessionState::Busy));
    }

    #[test]
    fn a_tool_reports_its_files_before_its_state() {
        // Ordering is load-bearing: the editor reload is the thing a user sees, and it must
        // not queue behind a transition that may not even be emitted.
        let states = States::new();
        let session = SessionId::new();
        let f = HookFrame::new(
            "PostToolUse",
            json!({
                "session_id": session.to_string(),
                "tool_input": { "file_path": "/w/src/main.rs" },
            }),
        );

        assert_eq!(
            decide(&f, &states),
            vec![
                Effect::Tool {
                    session: session.to_string(),
                    paths: vec!["/w/src/main.rs".to_string()],
                },
                Effect::State {
                    session: session.to_string(),
                    state: SessionState::Busy,
                },
            ]
        );
    }

    #[test]
    fn only_a_notification_that_asks_for_permission_changes_the_state() {
        // The one branch the state machine cannot decide, because it needs the payload text.
        let states = States::new();
        let session = SessionId::new();
        states.insert(session, SessionState::Busy);

        let chatter = HookFrame::new(
            "Notification",
            json!({ "session_id": session.to_string(), "message": "Compacting the transcript" }),
        );
        assert!(
            decide(&chatter, &states).is_empty(),
            "ordinary notifications say nothing about liveness"
        );
        assert_eq!(state_of(&states, session), Some(SessionState::Busy));

        let asking = HookFrame::new(
            "Notification",
            json!({
                "session_id": session.to_string(),
                "message": "Claude needs your permission to use Bash",
            }),
        );
        assert_eq!(
            decide(&asking, &states),
            vec![Effect::State {
                session: session.to_string(),
                state: SessionState::AwaitingPermission,
            }]
        );

        // A second prompt while already waiting is not news. Without the filter every
        // re-notification would re-emit, and the pane would flicker its badge.
        assert!(decide(&asking, &states).is_empty());
        assert_eq!(
            state_of(&states, session),
            Some(SessionState::AwaitingPermission)
        );
    }

    #[test]
    fn an_unknown_session_is_ignored_rather_than_tracked() {
        // A `claude` a user starts by hand in a cide shell inherits CIDE_HOOK_SOCK and
        // reports here. It has no pane, so it must not appear in `live_sessions` and block a
        // close confirm on something the user never opened.
        let states = States::new();

        assert!(decide(&frame("UserPromptSubmit", "not-a-uuid"), &states).is_empty());
        assert!(states.is_empty(), "an unowned session was tracked");

        // Nor may a frame with no session at all create an entry.
        assert!(decide(&HookFrame::new("Stop", json!({})), &states).is_empty());
        assert!(states.is_empty());
    }

    #[test]
    fn an_event_this_build_does_not_know_costs_one_frame_and_no_state() {
        // A CLI release that adds a hook point must not be able to move a session's state by
        // falling through to some default.
        let states = States::new();
        let session = SessionId::new();
        states.insert(session, SessionState::Busy);

        assert!(decide(&frame("SomethingNew", &session.to_string()), &states).is_empty());
        assert_eq!(state_of(&states, session), Some(SessionState::Busy));
    }

    #[test]
    fn only_a_busy_or_asking_session_blocks_a_close() {
        // What `live_sessions` filters on, asserted against the states `decide` actually
        // produces and through the function the server actually calls — `is_live()` on a
        // literal `SessionState` would assert about the enum rather than about this module.
        let states = States::new();
        let session = SessionId::new();

        decide(&frame("SessionStart", &session.to_string()), &states);
        assert_eq!(state_of(&states, session), Some(SessionState::Idle));
        assert!(
            live_in(&states).is_empty(),
            "an idle session must not raise a close confirm"
        );

        decide(&frame("UserPromptSubmit", &session.to_string()), &states);
        assert_eq!(
            live_in(&states),
            vec![session],
            "a turn is worth warning about"
        );
    }

    #[test]
    fn forgetting_a_session_takes_it_out_of_the_live_set() {
        // `HookServer::forget` was dead code until the exit watcher called it. Without it a
        // `claude` that dies mid-turn is remembered as Busy for the life of the process and
        // the close confirm warns about interrupting a session that is already gone.
        //
        // Two sessions, because the claim is not "the map can be emptied" — it is that
        // forgetting takes out *the one that died* and leaves the other one blocking a close.
        let states = States::new();
        let dying = SessionId::new();
        let survivor = SessionId::new();
        decide(&frame("UserPromptSubmit", &dying.to_string()), &states);
        decide(&frame("UserPromptSubmit", &survivor.to_string()), &states);
        assert_eq!(live_in(&states).len(), 2);

        forget_in(&states, dying);

        assert_eq!(
            live_in(&states),
            vec![survivor],
            "a forgotten session still blocks a close, or forgetting took the wrong one"
        );
        assert!(
            state_of(&states, dying).is_none(),
            "the dead session is still tracked"
        );
    }

    /// Why the frames have to be applied in the order they arrive.
    ///
    /// `PostToolUse` records liveness on purpose — a session somehow marked idle mid-turn
    /// corrects itself rather than staying wrong until the next prompt. That is right, and it
    /// is exactly what makes ordering load-bearing at the end of a turn: `Stop` then
    /// `PostToolUse` leaves the session **Busy**, which is a finished turn reported as a
    /// running one. The awaiting marker goes up on the `Stop` and comes straight back down,
    /// with nobody at the keyboard, and the user returns to a task bar that says nothing is
    /// waiting for them.
    ///
    /// Nothing about the two frames prevents it: each is a separate short-lived `cide-hook`
    /// process on a separate socket, and before the serialising channel there was a thread per
    /// connection deciding the order. This pins the consequence, so the channel cannot be
    /// refactored away as an unnecessary hop.
    #[test]
    fn a_tool_frame_applied_after_stop_reports_a_finished_turn_as_a_running_one() {
        let session = SessionId::new();
        let id = session.to_string();

        let in_order = States::new();
        for event in ["UserPromptSubmit", "PostToolUse", "Stop"] {
            decide(&frame(event, &id), &in_order);
        }
        assert_eq!(
            state_of(&in_order, session),
            Some(SessionState::Idle),
            "in arrival order the turn ends idle, which is what raises the marker"
        );

        let reordered = States::new();
        for event in ["UserPromptSubmit", "Stop", "PostToolUse"] {
            decide(&frame(event, &id), &reordered);
        }
        assert_eq!(
            state_of(&reordered, session),
            Some(SessionState::Busy),
            "and swapping the last two loses the finished turn entirely — this is the \
             failure the single applier thread exists to prevent, not a quirk of the fixture"
        );
    }

    /// The connection half of that: every frame a socket carries reaches the applier, once,
    /// in the order it was parsed.
    ///
    /// Driven through a real `UnixStream` because the ordering claim is about what `handle`
    /// does with a stream of lines, and a test that called `send` itself would assert about
    /// `std::sync::mpsc`. The unparseable line in the middle is here for the other half of the
    /// contract: a CLI release that changes a payload costs one frame, not the connection and
    /// not the frames behind it.
    #[test]
    fn every_frame_on_a_connection_reaches_the_applier_in_arrival_order() {
        use std::io::Write;

        let (mut writer, reader) = UnixStream::pair().expect("a socket pair");
        let (frames, inbox) = std::sync::mpsc::channel::<HookFrame>();
        let session = SessionId::new().to_string();

        for line in [
            serde_json::to_string(&frame("UserPromptSubmit", &session)).expect("serialises"),
            "{not json".to_string(),
            serde_json::to_string(&frame("PostToolUse", &session)).expect("serialises"),
            serde_json::to_string(&frame("Stop", &session)).expect("serialises"),
        ] {
            writeln!(writer, "{line}").expect("writes");
        }
        // EOF, so `handle` returns rather than blocking this thread for ever.
        drop(writer);

        handle(reader, &frames);
        drop(frames);

        let seen: Vec<String> = inbox.into_iter().map(|f| f.event).collect();
        assert_eq!(
            seen,
            vec!["UserPromptSubmit", "PostToolUse", "Stop"],
            "order is the whole point, and the bad line must cost exactly itself"
        );
    }
}
