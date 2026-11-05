//! Ending sessions no pane is showing. (M196)
//!
//! `closeTab` deliberately leaves the tab's children running so Ctrl+Shift+T can re-adopt them,
//! and nothing else ever ended them: the undo stack only remembers sixteen closes, so a child
//! pushed off it had no route back and no owner either. A day of opening and closing OpenSpec
//! tabs measured 54 `claude` processes at about 350 MB each. Closing a project does not catch
//! them, because `cmd::project::sessions_of` walks the tree and these left the tree.
//!
//! # What "detached" means
//!
//! No pane in any project names the session — `workspace::session_panes`, not
//! `PtySession::sink_count`. A parked tab or an evicted host has no sink and is very much still
//! open, so counting sinks would end the conversation behind a tab the user merely switched
//! away from. Sessions that still have a pane are out of scope however long they have sat
//! idle: that one was asked about and declined, because a pane going dead under the user's eyes
//! is a different promise from a closed tab going away.
//!
//! # `AwaitingInput` is not an exemption, and that is the trap in this file
//!
//! It reads like "somebody is waiting for the user", and `cide_claude::next_state` does turn a
//! `Stop` out of `Busy` into it — but that makes it the state of **every Claude session that has
//! finished a turn**. Exempting it (the first draft of this design did), or the
//! `windows::awaiting` set the webview reports from it, would leave the reaper with nothing to
//! do: the population it exists for is exactly finished sessions in closed tabs, and no one can
//! clear the marker on a tab that is not open. So only what `running::pane_is_busy` calls busy
//! holds a session. The cost is real but small: a Codex `request_user_input` also reads
//! `AwaitingInput`, so a question asked in a tab that has been closed for the whole timeout goes
//! unanswered and the conversation resumes from its transcript instead.
//!
//! # Why the undo stack is not an exemption
//!
//! It was, in the first version, because reopening a tab whose child had gone showed a dead pane.
//! That capped the feature at nothing useful: sixteen closed tabs, across all projects, kept
//! their children indefinitely. So the reopen was fixed instead, and a closed tab now costs a
//! `--resume` once its timeout passes. Two things make that safe, and both are required:
//!
//! * [`crate::claude_tab::resume_closed`] respawns the conversation before the tab is back in the
//!   tree. Without it a pane naming a session nobody holds spawns a **fresh** `claude`, which is
//!   worse than a dead pane because it looks fine.
//! * `cide://panes-retired` destroys the parked terminal, in every window. The webview's ledger
//!   would otherwise keep naming the dead child. This is also what frees the terminal's memory.
//!
//! [`wait_out`] covers a reopen pressed while the ladder below is still running.
//!
//! # A thread of its own
//!
//! `spinner`'s shape and for `spinner`'s reasons: there is no app tick, and the ladder below
//! blocks for up to 2.5 s, which must not happen on GTK or on a Tauri runtime worker.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use cide_ipc::{SessionId, Workspace};
use cide_pty::PtySession;
use tauri::{AppHandle, Manager};

use crate::running::{Busy, pane_is_busy};
use crate::state::SessionRegistry;
use crate::workspace_state::WorkspaceState;

/// A minute is the smallest timeout the setting offers, so thirty seconds is at worst a 50 %
/// overshoot on it and invisible on the default. Everything a tick reads is in memory.
const POLL: Duration = Duration::from_secs(30);

/// How long an **exited** child's entry outlives its last pane, instead of the full timeout.
///
/// There is no process to give a chance to: what is left is a screen mirror, kept only so a pane
/// could still adopt it. Not zero, because a tab being rebuilt can drop its reference for a
/// moment. Capped by the timeout so a user who chose one minute is not out-waited.
const EXITED_GRACE: Duration = Duration::from_secs(60);

/// How recently a device must have read or written a session to hold it.
///
/// `remote.rs` reads the registry directly by id, so a phone following a session has neither a
/// pane nor a sink and would otherwise look exactly like nobody. Five minutes because the clock
/// restarts when this lapses, so the session still gets its whole timeout after the phone goes
/// quiet; this only has to outlast the gap between two looks.
const REMOTE_WATCH: Duration = Duration::from_secs(300);

fn touched() -> &'static Mutex<HashMap<SessionId, Instant>> {
    static TOUCHED: OnceLock<Mutex<HashMap<SessionId, Instant>>> = OnceLock::new();
    TOUCHED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A companion device just looked at or typed into this session.
pub(crate) fn note_remote(session: SessionId) {
    touched()
        .lock()
        .expect("the remote stamps are never poisoned")
        .insert(session, Instant::now());
}

fn ending() -> &'static Mutex<HashSet<SessionId>> {
    static ENDING: OnceLock<Mutex<HashSet<SessionId>>> = OnceLock::new();
    ENDING.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Wait until the reaper has finished with this session, if it has started on it.
///
/// A reopen pressed during the ladder would otherwise see a live child, adopt it, and watch it
/// die: the dead pane this module promises not to cause. Bounded a little past the ladder's 2.5 s,
/// because a child that outlives it stays registered and is then simply adopted.
pub(crate) async fn wait_out(session: SessionId) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline
        && ending()
            .lock()
            .expect("the ending set is never poisoned")
            .contains(&session)
    {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// What one tick knows about one session, gathered so the decision can be tested without an app.
#[derive(Debug, Clone, Copy, Default)]
struct Facts {
    /// A pane in the tree names it, as its session or as the conversation it moved onto.
    referenced: bool,
    /// Any run names it, **in any state**. Not `AgentRegistry::owns_session`, which goes false
    /// when a run ends: a finished run's retained entry is what its History row opens onto, and
    /// removing it replaced an opencode run's rendered log with a fresh `claude` (see
    /// `TerminalPane.tsx`). Runs end through their owners and keep their own records.
    run: bool,
    /// One half of a two-agent chat, which `peer_chat::stop_pair` ends.
    paired: bool,
    /// Mid-turn, frozen, or holding a permission prompt: `running::pane_is_busy`.
    busy: bool,
    remote: bool,
    exited: bool,
}

impl Facts {
    fn held(&self) -> bool {
        self.referenced || self.run || self.paired || self.busy || self.remote
    }
}

/// When each session was first seen with nothing holding it.
///
/// Kept here because nothing else records it: the registry knows when a child was forked, not when
/// its last pane went. Starts empty on every launch, which is right — a restart spawns only what
/// the tree names.
#[derive(Debug, Default)]
struct Clock {
    since: HashMap<SessionId, Instant>,
}

impl Clock {
    /// Whether this session's time is up. Anything holding it restarts the wait, so the timeout
    /// means *continuously* unreferenced and not busy, and a turn finishing in a closed tab gets
    /// the whole of it rather than being ended the moment it stops.
    fn due(&mut self, session: SessionId, facts: Facts, now: Instant, timeout: Duration) -> bool {
        if facts.held() {
            self.since.remove(&session);
            return false;
        }
        let since = *self.since.entry(session).or_insert(now);
        let wait = if facts.exited {
            EXITED_GRACE.min(timeout)
        } else {
            timeout
        };
        now.duration_since(since) >= wait
    }

    /// Drop sessions the registry no longer has, so the map cannot outgrow the process.
    fn retain(&mut self, live: &HashSet<SessionId>) {
        self.since.retain(|session, _| live.contains(session));
    }
}

/// Every session a pane names. A set because panes can mirror one session.
fn referenced(ws: &Workspace) -> HashSet<SessionId> {
    let mut out: HashSet<SessionId> = cide_core::workspace::session_panes(ws, None)
        .into_iter()
        .flat_map(|found| [Some(found.session), found.pane.conversation])
        .flatten()
        .collect();
    // Always in the tree too, since tab 0 cannot close. Said outright anyway: this one ending by
    // accident is the most expensive mistake this file could make.
    out.extend(ws.projects.values().map(|project| project.primary_session));
    out
}

/// Start the reaper. Called once, from `setup`.
///
/// A failure to spawn costs the feature and not the launch — `spinner::start`'s rule.
pub fn start(app: AppHandle) {
    thread::Builder::new()
        .name("cide-reaper".into())
        .spawn(move || {
            let mut clock = Clock::default();
            loop {
                thread::sleep(POLL);
                tick(&app, &mut clock);
            }
        })
        .map(|_| ())
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "no reaper thread; closed tabs keep their sessions until quit");
        });
}

fn tick(app: &AppHandle, clock: &mut Clock) {
    let (Some(state), Some(registry), Some(hooks), Some(agents), Some(peers)) = (
        app.try_state::<WorkspaceState>(),
        app.try_state::<SessionRegistry>(),
        app.try_state::<crate::hooks::HookServer>(),
        app.try_state::<std::sync::Arc<crate::agents::AgentRegistry>>(),
        app.try_state::<crate::peer_chat::Runtime>(),
    ) else {
        return;
    };
    // A snapshot rather than a `with`: this reads several registries, and `spinner::tick`'s rule
    // is that no lock is held across another.
    let ws = state.snapshot();
    let minutes = ws.settings.reap_detached_after_minutes;
    if minutes == 0 {
        // Off means off, exited entries included, and turning it back on starts a fresh wait
        // rather than ending everything that was detached in the meantime.
        clock.since.clear();
        return;
    }
    let timeout = Duration::from_secs(u64::from(minutes) * 60);
    let named = referenced(&ws);
    let now = Instant::now();

    let ids = registry.ids();
    let live: HashSet<SessionId> = ids.iter().copied().collect();
    clock.retain(&live);
    let recent: HashSet<SessionId> = {
        let mut stamps = touched()
            .lock()
            .expect("the remote stamps are never poisoned");
        stamps
            .retain(|session, at| live.contains(session) && now.duration_since(*at) < REMOTE_WATCH);
        stamps.keys().copied().collect()
    };

    let mut due: Vec<(SessionId, std::sync::Arc<PtySession>)> = Vec::new();
    for id in ids {
        let Some(pty) = registry.get(id) else {
            continue;
        };
        let exited = pty.has_exited();
        let facts = Facts {
            referenced: named.contains(&id),
            run: agents.names_session(id),
            paired: peers.is_paired(id),
            busy: pane_is_busy(
                !exited,
                false,
                hooks.state(id),
                Busy::Claimed,
                registry.has_opening_prompt(id),
            ),
            remote: recent.contains(&id),
            exited,
        };
        if clock.due(id, facts, now, timeout) {
            due.push((id, pty));
        }
    }
    if due.is_empty() {
        return;
    }

    // The graceful path, as `cmd::project::stop_closed_sessions` walks it, so a transcript gets
    // its chance to flush. One ladder for the whole batch: it is per rung and not per child.
    ending()
        .lock()
        .expect("the ending set is never poisoned")
        .extend(due.iter().map(|(id, _)| *id));
    let handles: Vec<_> = due.iter().map(|(_, pty)| pty.clone()).collect();
    crate::lifecycle::stop_children(&handles, crate::lifecycle::Ladder::default());
    let logs = app.try_state::<std::sync::Arc<crate::logring::JsonLogRing>>();
    let mut ended = Vec::new();
    for (id, pty) in due {
        // Removal is not optional: a retained entry is what `sessionIsHeld` adopts, and the
        // reopened conversation would show `— exited —` in place of a `--resume`.
        if !pty.has_exited() {
            tracing::warn!(session = %id, "a detached session outlived the ladder; trying again next tick");
            continue;
        }
        registry.remove(id);
        clock.since.remove(&id);
        if let Some(logs) = &logs {
            logs.forget(id);
        }
        tracing::info!(session = %id, minutes, "ended a session no pane has shown");
        ended.push(id);
    }
    // Before the set clears: a reopen released by `wait_out` must find the parked terminal retired.
    if !ended.is_empty() {
        crate::emit::panes_retired(app, ended, Vec::new());
    }
    ending()
        .lock()
        .expect("the ending set is never poisoned")
        .clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(30 * 60);

    /// Nothing holding it, at each of the three moments the wait is measured against.
    fn runs_out(facts: Facts) -> (bool, bool) {
        let mut clock = Clock::default();
        let session = SessionId::new();
        let start = Instant::now();
        let first = clock.due(session, facts, start, TIMEOUT);
        let later = clock.due(session, facts, start + TIMEOUT, TIMEOUT);
        (first, later)
    }

    #[test]
    fn a_session_nothing_holds_is_due_only_after_the_whole_timeout() {
        let mut clock = Clock::default();
        let session = SessionId::new();
        let start = Instant::now();
        let free = Facts::default();
        assert!(
            !clock.due(session, free, start, TIMEOUT),
            "the wait starts now"
        );
        assert!(!clock.due(
            session,
            free,
            start + TIMEOUT - Duration::from_secs(1),
            TIMEOUT
        ));
        assert!(clock.due(session, free, start + TIMEOUT, TIMEOUT));
    }

    #[test]
    fn everything_that_holds_a_session_holds_it_for_good() {
        let held = [
            (
                "a pane names it",
                Facts {
                    referenced: true,
                    ..Facts::default()
                },
            ),
            (
                "a run names it",
                Facts {
                    run: true,
                    ..Facts::default()
                },
            ),
            (
                "half of a chat",
                Facts {
                    paired: true,
                    ..Facts::default()
                },
            ),
            (
                "mid-turn",
                Facts {
                    busy: true,
                    ..Facts::default()
                },
            ),
            (
                "a phone is reading it",
                Facts {
                    remote: true,
                    ..Facts::default()
                },
            ),
            // A finished run opened from History reads this entry.
            (
                "exited, but a run names it",
                Facts {
                    run: true,
                    exited: true,
                    ..Facts::default()
                },
            ),
        ];
        for (why, facts) in held {
            assert_eq!(runs_out(facts), (false, false), "{why}");
        }
    }

    #[test]
    fn being_held_restarts_the_wait_rather_than_pausing_it() {
        let mut clock = Clock::default();
        let session = SessionId::new();
        let start = Instant::now();
        let free = Facts::default();
        let busy = Facts {
            busy: true,
            ..Facts::default()
        };
        clock.due(session, free, start, TIMEOUT);
        // A turn that ends at the 29th minute must not be killed a minute later.
        let ended = start + Duration::from_secs(29 * 60);
        assert!(!clock.due(session, busy, ended, TIMEOUT));
        assert!(!clock.due(session, free, ended, TIMEOUT));
        assert!(!clock.due(
            session,
            free,
            ended + TIMEOUT - Duration::from_secs(1),
            TIMEOUT
        ));
        assert!(clock.due(session, free, ended + TIMEOUT, TIMEOUT));
    }

    #[test]
    fn an_exited_session_gets_only_the_short_grace() {
        let mut clock = Clock::default();
        let session = SessionId::new();
        let start = Instant::now();
        let gone = Facts {
            exited: true,
            ..Facts::default()
        };
        assert!(!clock.due(session, gone, start, TIMEOUT));
        assert!(clock.due(session, gone, start + EXITED_GRACE, TIMEOUT));
    }

    #[test]
    fn the_exit_grace_never_outlasts_a_shorter_timeout() {
        let mut clock = Clock::default();
        let session = SessionId::new();
        let start = Instant::now();
        let short = Duration::from_secs(30);
        let gone = Facts {
            exited: true,
            ..Facts::default()
        };
        clock.due(session, gone, start, short);
        assert!(clock.due(session, gone, start + short, short));
    }

    #[test]
    fn a_session_the_registry_lost_is_forgotten() {
        let mut clock = Clock::default();
        let kept = SessionId::new();
        let lost = SessionId::new();
        let start = Instant::now();
        clock.due(kept, Facts::default(), start, TIMEOUT);
        clock.due(lost, Facts::default(), start, TIMEOUT);
        clock.retain(&HashSet::from([kept]));
        assert!(clock.since.contains_key(&kept));
        assert!(!clock.since.contains_key(&lost));
    }

    /// The trap in the module doc, pinned against the real predicate and the real state machine:
    /// a finished turn must leave a session reapable, or the reaper never fires on the sessions
    /// it exists for.
    #[test]
    fn a_finished_turn_does_not_hold_a_session() {
        use cide_claude::{HookEvent, next_state};
        use cide_ipc::SessionState;
        let finished = next_state(SessionState::Busy, HookEvent::Stop)
            .expect("a Stop out of Busy is a transition");
        assert_eq!(finished, SessionState::AwaitingInput);
        assert!(!pane_is_busy(true, false, finished, Busy::Claimed, false));
        assert!(pane_is_busy(
            true,
            false,
            SessionState::Busy,
            Busy::Claimed,
            false
        ));
        assert!(pane_is_busy(
            true,
            false,
            SessionState::AwaitingPermission,
            Busy::Claimed,
            false
        ));
    }
}
