//! One Docker board for the machine, read on demand and broadcast when the daemon moves. (M41)
//!
//! # Why this is keyed by nothing
//!
//! `SpecBoards` is a `HashSet<ProjectId>` because a spec board belongs to a project. A Docker
//! daemon does not: it is a property of the machine, every window shows the same containers, and
//! a per-project cache would be N copies of one answer that disagree while they refresh. So the
//! coalescer here has no key, and `cide://docker-changed` carries a board rather than a project.
//!
//! # The connection is held, and the board is not
//!
//! `SpecBoards`' posture, split. A *board* is always a fresh read — caching one would be caching
//! the thing that changes. The *connection* is held, because opening one costs a socket, a
//! runtime and a version negotiation, and doing that per refresh would make a two-second poll a
//! two-second handshake. It is dropped whenever the endpoint changes or a read fails, so a daemon
//! that goes away and comes back is reconnected to rather than reported dead for ever — which is
//! the whole of what `colima restart` looks like from here.
//!
//! # Why the events subscription and not a timer
//!
//! `docker events` is the daemon telling cide what happened, and it arrives in the same bursts a
//! file watcher does: `docker compose up` on a six-service stack emits some forty events in under
//! a second. So it is coalesced exactly as `SpecBoards` coalesces watcher events, and for the
//! second reason too — the `flushing` flag means only one read is ever in flight, which is what
//! lets the event carry no `rev`. A poll would have to choose an interval that is either too slow
//! to feel live or fast enough to keep a socket busy on an idle machine.
//!
//! The subscription is started by [`DockerState::board_watching`] on the first board read that
//! succeeds, and is dropped with the connection it belongs to. It was described here for a
//! milestone before it existed, which is worth remembering: `mark_changed` had no caller, the
//! panel refreshed only when the user acted, and nothing in the build could see the difference
//! because a comment is not a test. `a_real_daemon`'s event test is.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cide_docker::connect::Endpoint;
use cide_ipc::docker::DockerBoard;
use parking_lot::Mutex;
use tauri::AppHandle;

/// How long a burst has to be quiet before the board is read.
///
/// `SpecBoards`' value, and for a nearer reason: a board is one round trip rather than two
/// subprocesses, but `docker compose up` on a six-service stack emits some forty events in under
/// a second and every one of them would otherwise be a read.
const COALESCE: Duration = Duration::from_millis(250);
/// And the ceiling, so a container in a restart loop still updates about once a second.
const COALESCE_CEILING: Duration = Duration::from_secs(1);
/// How often the flusher wakes while a burst settles.
const COALESCE_TICK: Duration = Duration::from_millis(25);

/// How many lines of history a log pane opens with.
///
/// Enough to see why something crashed, and not so many that opening the pane is a wait: a
/// container that has been up for a week can hold hundreds of megabytes, and `tail: all` on one
/// would stream the lot through the coalescer before the first frame. The rest is a scroll away
/// in `docker logs` and deliberately not here.
const LOG_TAIL: u32 = 2000;

/// The machine's connection, and the coalescer behind `cide://docker-changed`.
#[derive(Default)]
pub struct DockerState {
    connection: Mutex<Connection>,
    coalesce: Mutex<CoalesceState>,
    /// How many panels are on screen, across every window. (M52)
    ///
    /// # Why the watch has to stop as well as start
    ///
    /// [`Self::ensure_watching`] made the subscription lazy — it exists only once somebody has
    /// opened the panel — and that was half the rule. It never stopped. So a session in which the
    /// Docker tab was opened *once* kept a `GET /events` stream for the rest of its life, and
    /// turned every daemon event into a **full board read**: containers, images, volumes and
    /// networks, four calls, for a panel nobody was looking at. On a machine where anything is
    /// starting and stopping — a `docker compose up`, a CI runner, a devcontainer — that is
    /// continuous.
    ///
    /// It was reported as log noise, which is how it is visible, and it was really work. The
    /// justification in `App.tsx` for subscribing app-wide was the **rail badge**, which had to
    /// stay live while the sidebar was shut; M46 moved the panel into the bottom tool window and
    /// deleted that badge, and nothing outside the panel has read the board since.
    ///
    /// **Not a count.** The latest thing each window said, and when it said it. (M54)
    ///
    /// # Why a counter was wrong, twice
    ///
    /// M52 used an `AtomicUsize` incremented on mount and decremented on unmount. Two orderings
    /// break it, and React's StrictMode produces both by firing `watch(true)`, `watch(false)`,
    /// `watch(true)` as three **concurrent** Tauri commands — `docker_watch` is `async`, so they
    /// run on three workers in no guaranteed order:
    ///
    /// * a decrement that arrives **before** its increment is clamped by `saturating_sub` (there
    ///   to stop a wrap to `usize::MAX`) and is simply **lost**, so the count drifts *up* and the
    ///   subscription is never stopped — measured: mount/unmount/mount ending at 2, not 1;
    ///   and
    /// * a stop whose *action* overtook a start left the count at 1 with no subscription running,
    ///   which never recovers, because nothing re-reads the count.
    ///
    /// Both were reported as the same thing: the panel does not update.
    ///
    /// A counter cannot be fixed by clamping, because the problem is not arithmetic — it is that
    /// an **unordered channel carries edges**. So the wire carries *levels* instead: each window
    /// says what is true for it now, stamped with a sequence number it increments itself, and an
    /// older stamp for that window is discarded. The answer is then a function of the latest
    /// statement from each window, which no ordering can disturb.
    ///
    /// Keyed by window label because every window has its own panel, and one closing must not
    /// take the stream away from another's.
    panels: Mutex<std::collections::HashMap<String, (u64, bool)>>,
    /// Serialises [`Self::watching`], so the count and the subscription cannot disagree. (M54)
    ///
    /// # The race this exists for, which shipped and was reported
    ///
    /// React's StrictMode double-invokes an effect in development, so a panel mounting fires
    /// `watch(true)`, `watch(false)`, `watch(true)` — three **concurrent** Tauri commands, each on
    /// its own worker, in no guaranteed order. The counter is safe under every ordering (+1−1+1),
    /// and the *side effects* were not: a `true` that started the stream could be overtaken by the
    /// `false` that stopped it, leaving the count at 1 with no subscription running. Nothing
    /// re-reads the count, so the panel then never updated again — which is exactly how it was
    /// reported, twice.
    ///
    /// A second mutex rather than widening `connection`: [`Self::connect`] deliberately does not
    /// hold that lock across a version negotiation (120-second timeout), and a board read must
    /// never wait behind a panel mounting.
    watch_gate: Mutex<()>,
}

/// What a change in the number of on-screen panels asks for.
///
/// Split out so the decision can be *tested* — the action needs an `AppHandle` and a daemon, the
/// arithmetic that got this wrong needs neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchAction {
    /// Make sure a subscription exists.
    Start,
    /// The last panel went away; drop it.
    Stop,
}

/// The live event subscription, if one is running.
///
/// Dropped whenever the connection is — an endpoint switch must not leave the old daemon's
/// events arriving under the new one's name.
type Watch = Option<cide_docker::EventWatch>;

#[derive(Default)]
struct Connection {
    /// The endpoint the user chose in the switcher. `None` means "run the ladder", which is not
    /// the same as any particular endpoint it might pick — a machine whose current context
    /// changes under a running cide should follow it, and only an explicit choice should pin.
    chosen: Option<Endpoint>,
    /// The live handle, or `None` when there is none to reuse.
    docker: Option<Arc<cide_docker::Docker>>,
    /// The event subscription for `docker`, started lazily — see [`DockerState::board`].
    watch: Watch,
}

#[derive(Default)]
struct CoalesceState {
    pending: bool,
    first: Option<Instant>,
    last: Option<Instant>,
    flushing: bool,
}

impl DockerState {
    /// Point at a different daemon, dropping any connection to the old one.
    ///
    /// `None` restores the ladder. Returns nothing: the caller reads a board next, and a
    /// switcher that reported success separately from the board it produced would let the two
    /// disagree on screen.
    pub fn choose(&self, endpoint: Option<Endpoint>) {
        let mut connection = self.connection.lock();
        connection.chosen = endpoint;
        connection.docker = None;
        // Dropped with the connection it belongs to. A watch left running would go on reporting
        // the *old* daemon's events, and the coalescer would turn each one into a read of the
        // new one — a panel that refreshes when a container somewhere else starts.
        connection.watch = None;
    }

    /// The endpoint the switcher has pinned, if any.
    pub fn chosen(&self) -> Option<Endpoint> {
        self.connection.lock().chosen.clone()
    }

    /// A live connection, opening one if there is none.
    ///
    /// # The lock is not held across the connect
    ///
    /// Opening negotiates a version, which is a round trip to a daemon that may be a virtual
    /// machine still booting. Holding the mutex through it would block every other window's
    /// refresh — and the timeout is 120 seconds. So the lock is taken twice, and the race it
    /// leaves is deliberately lost the cheap way: two threads may both open a connection, and
    /// the second simply replaces the first. A duplicate socket that closes immediately costs
    /// nothing; a frozen UI costs the feature.
    fn connect(&self) -> Result<Arc<cide_docker::Docker>, cide_docker::DockerError> {
        let chosen = {
            let connection = self.connection.lock();
            if let Some(docker) = &connection.docker {
                return Ok(Arc::clone(docker));
            }
            connection.chosen.clone()
        };

        let docker = Arc::new(cide_docker::Docker::open(chosen)?);
        self.connection.lock().docker = Some(Arc::clone(&docker));
        Ok(docker)
    }

    /// Drop the held connection, so the next read opens a new one.
    ///
    /// Called on every failure. A daemon that has gone away leaves a handle whose every request
    /// fails identically, and a cide that kept it would go on reporting a dead daemon after
    /// `colima start` — with a Retry button that could not work.
    fn disconnect(&self) {
        let mut connection = self.connection.lock();
        connection.docker = None;
        connection.watch = None;
    }

    /// Read the board, turning every failure into a board rather than an error.
    ///
    /// `cide_app::spec_state::read`'s rule: a panel handed an error has nothing to draw and no
    /// way to say what to do next, while a board that says the daemon is not running has a
    /// sentence and a Retry.
    /// Subscribe to the daemon's events, once per connection.
    ///
    /// # Why this is lazy and not started with the app
    ///
    /// Because opening a socket is a statement that cide is using Docker, and most launches never
    /// open the panel. Starting the watch on the first *successful board read* means the
    /// subscription exists exactly when somebody is looking, and a machine with no daemon is
    /// never connected to at all.
    fn ensure_watching(self: &Arc<Self>, app: &AppHandle, docker: &Arc<cide_docker::Docker>) {
        let mut connection = self.connection.lock();
        if connection.watch.is_some() {
            return;
        }
        let this = Arc::clone(self);
        let app = app.clone();
        connection.watch = Some(docker.watch_events(move || this.mark_changed(&app)));
    }

    /// Say whether a panel is on screen, and stop the subscription when the last one goes.
    ///
    /// Returns the number of watchers left, for the log line and for tests.
    ///
    /// **Stopping drops the watch and keeps the connection.** The stream is what costs something
    /// — an open socket, a thread, and a full board read per event — while the handle is idle
    /// until something asks it a question, and keeping it means reopening the panel is instant
    /// and a container action from anywhere else still works without renegotiating a version.
    pub fn watching(
        self: &Arc<Self>,
        app: &AppHandle,
        window: &str,
        seq: u64,
        watching: bool,
    ) -> usize {
        self.watching_with(window, seq, watching, |action| match action {
            WatchAction::Stop => {
                let mut connection = self.connection.lock();
                if connection.watch.take().is_some() {
                    tracing::debug!("docker: last panel closed, stopping the event subscription");
                }
            }
            WatchAction::Start => {
                // A panel opening while another is already watching needs no new stream, but it
                // does need one to *exist*: the first board read starts it, and a second window
                // that adopts a board over the wire never makes one.
                match self.connect() {
                    Ok(docker) => self.ensure_watching(app, &docker),
                    // Said out loud. A panel that silently fails to subscribe looks exactly like
                    // one whose daemon has nothing to report, and telling the two apart from a log
                    // was the whole difficulty the second time this was reported.
                    Err(error) => {
                        tracing::debug!(%error, "docker: a panel opened but no daemon answered");
                    }
                }
            }
        })
    }

    /// [`Self::watching`] with the action injected, which is how the race is *tested*.
    ///
    /// # Why the action is a parameter rather than inlined
    ///
    /// Because the bug is in the **ordering of the actions**, not of the decisions, and the real
    /// action needs an `AppHandle` and a live daemon. A test that reimplemented the structure
    /// beside it would be a mirror that can drift from what ships — and this exact function is
    /// where drift is invisible. So the test drives *this* code, with a closure that records what
    /// it did; the gate below, the counter and the ordering are all the real ones.
    fn watching_with(
        &self,
        window: &str,
        seq: u64,
        watching: bool,
        act: impl FnOnce(WatchAction),
    ) -> usize {
        // **Held across the bookkeeping *and* the action**, and it is load-bearing beside the
        // sequence numbers rather than instead of them. They make the *state* order-independent;
        // this makes the *action* match the state, because a stop whose action overtook a start
        // would leave a live panel with no subscription and nothing to recover from it.
        let _gate = self.watch_gate.lock();
        let (left, action) = self.decide(window, seq, watching);
        act(action);
        left
    }

    /// Record what one window said, and say what the machine now needs.
    ///
    /// Idempotent and order-independent: the result depends only on the newest statement from
    /// each window, so replaying, reordering or duplicating messages cannot change it. That is
    /// the whole reason this takes a level and a stamp rather than an increment.
    ///
    /// Returns how many windows are watching, and the action — `Start` whenever any window is,
    /// `Stop` when none is. Both are stated on every call rather than only on a transition: the
    /// action is idempotent at the other end (`ensure_watching` returns early if a stream exists,
    /// `watch.take()` on `None` is nothing), and a transition-only answer would be another edge
    /// on an unordered channel, which is the bug this replaced.
    fn decide(&self, window: &str, seq: u64, watching: bool) -> (usize, WatchAction) {
        let mut panels = self.panels.lock();
        match panels.get(window) {
            // A message older than what this window has already said. Discarded, and that is the
            // whole of the ordering fix.
            Some((known, _)) if *known >= seq => {}
            _ => {
                panels.insert(window.to_string(), (seq, watching));
            }
        }
        let watching_now = panels.values().filter(|(_, on)| *on).count();
        let action = if watching_now == 0 {
            WatchAction::Stop
        } else {
            WatchAction::Start
        };
        (watching_now, action)
    }

    /// Whether anything is on screen that would draw a board.
    ///
    /// Read by the tests only since M54 removed the gate in `mark_changed` — see the note there
    /// for why a redundant check that can only fail closed was worse than none.
    #[cfg(test)]
    fn watched(&self) -> bool {
        self.panels.lock().values().any(|(_, on)| *on)
    }

    /// Read the board, and start watching if this is the first time it worked.
    pub fn board_watching(self: &Arc<Self>, app: &AppHandle) -> DockerBoard {
        let board = self.board();
        if matches!(board, DockerBoard::Ready(_))
            && let Ok(docker) = self.connect()
        {
            self.ensure_watching(app, &docker);
        }
        board
    }

    pub fn board(&self) -> DockerBoard {
        let docker = match self.connect() {
            Ok(docker) => docker,
            Err(cide_docker::DockerError::NoEndpoint(hint)) => {
                return DockerBoard::Absent { hint };
            }
            Err(error) => {
                // The context store is read from `~/.docker` and is still there when nothing
                // answers, so the switcher survives a failed connection — see the DTO.
                return DockerBoard::Unusable {
                    reason: error.to_string(),
                    endpoint: String::new(),
                    contexts: cide_docker::context_rows(),
                    context: None,
                };
            }
        };

        match docker.snapshot() {
            Ok(snapshot) => DockerBoard::Ready(Box::new(snapshot)),
            Err(error) => {
                self.disconnect();
                DockerBoard::Unusable {
                    reason: error.to_string(),
                    endpoint: docker.endpoint().as_url(),
                    contexts: docker.contexts(),
                    context: docker.context().map(str::to_string),
                }
            }
        }
    }

    /// Do one thing to one container, and report the daemon's own words when it refuses.
    pub fn act(
        &self,
        id: &str,
        action: cide_ipc::docker::ContainerAction,
    ) -> Result<(), cide_docker::DockerError> {
        let docker = self.connect()?;
        let result = docker.act(id, action);
        // A transport failure means this handle is finished; a *refusal* ("You cannot remove a
        // running container") means the daemon is perfectly well and answered. Dropping the
        // connection for the second would turn every ordinary refusal into a reconnect.
        if matches!(
            result,
            Err(cide_docker::DockerError::Unreachable(_)
                | cide_docker::DockerError::Unreadable { .. })
        ) {
            self.disconnect();
        }
        result
    }

    /// How much disk one volume uses, or `None` when the daemon did not measure it. (M57)
    ///
    /// One volume rather than the map, because that is what the caller draws — and the daemon
    /// caches the walk between calls, so selecting a second volume costs about a second rather
    /// than another eight. See `cide_docker::api::volume_usage` for why this is never part of a
    /// board read.
    pub fn volume_size(&self, name: &str) -> Result<Option<i64>, cide_docker::DockerError> {
        let docker = self.connect()?;
        let result = docker.volume_usage();
        if matches!(
            result,
            Err(cide_docker::DockerError::Unreachable(_)
                | cide_docker::DockerError::Unreadable { .. })
        ) {
            self.disconnect();
        }
        Ok(result?.get(name).copied())
    }

    /// Remove an image, a volume or a network. (M56)
    ///
    /// The connection is dropped on a transport failure and **kept on a refusal**, [`Self::act`]'s
    /// rule and for a sharper reason here: refusing is the *expected* outcome of this call
    /// whenever something is using the thing, so treating it as a broken connection would make
    /// the ordinary case reconnect.
    pub fn remove(
        &self,
        target: &cide_ipc::docker::Removable,
    ) -> Result<(), cide_docker::DockerError> {
        let docker = self.connect()?;
        let result = docker.remove(target);
        if matches!(
            result,
            Err(cide_docker::DockerError::Unreachable(_)
                | cide_docker::DockerError::Unreadable { .. })
        ) {
            self.disconnect();
        }
        result
    }

    /// Open an exec or a log follow on a container. (M42)
    ///
    /// Goes through [`Self::connect`] like every read, so a pane opened while the daemon was down
    /// reconnects rather than failing against a stale handle — and a failure here drops the
    /// connection for the same reason a failed read does.
    pub fn open_stream(
        self: &Arc<Self>,
        container: &str,
        stream: cide_ipc::docker::DockerStream,
        geometry: cide_pty::Geometry,
    ) -> Result<std::sync::Arc<cide_pty::PtySession>, cide_docker::DockerError> {
        let docker = self.connect()?;
        let opened = match stream {
            // No command: `cide_docker::session` asks the container which shell it has rather
            // than guessing, which is the difference between a usable pane on an Alpine image
            // and `exec: "bash": executable file not found`.
            cide_ipc::docker::DockerStream::Exec => docker.exec_session(container, &[], geometry),
            cide_ipc::docker::DockerStream::Logs => {
                docker.logs_session(container, LOG_TAIL, geometry)
            }
        };
        if matches!(
            opened,
            Err(cide_docker::DockerError::Unreachable(_)
                | cide_docker::DockerError::Unreadable { .. })
        ) {
            self.disconnect();
        }
        opened
    }

    /// The raw `inspect` document for one thing. (M43)
    pub fn inspect(
        &self,
        target: &cide_ipc::docker::InspectTarget,
    ) -> Result<String, cide_docker::DockerError> {
        self.connect()?.inspect(target)
    }

    /// Run a Compose action on a stack, and answer with what it printed. (M43)
    ///
    /// # Why the working directory comes from the caller and is not derived
    ///
    /// It is `com.docker.compose.project.working_dir`, read off the stack's own containers —
    /// which is the only record of where the stack was brought up from. Deriving it from the
    /// open project would be a guess, and a wrong one is `docker compose down` run in the wrong
    /// directory: with `-p` passed it stops the right stack anyway, and without a directory that
    /// exists the command refuses outright. `None` runs it in cide's own cwd, which is what
    /// happens for a stack whose label is missing.
    pub fn compose(
        &self,
        working_dir: Option<&std::path::Path>,
        project: &str,
        action: cide_ipc::docker::ComposeAction,
        files: &[String],
    ) -> Result<String, cide_docker::DockerError> {
        // Deliberately **not** through `connect()`: a Compose action is a CLI invocation and
        // needs no daemon handle. Requiring one would make the buttons fail while cide happened
        // to be between connections, for a command that would have worked.
        cide_docker::compose::act(working_dir, project, action, files)
    }

    /// List one directory inside a container. (M44)
    ///
    /// Never `Err` for a container reason — see `cide_ipc::docker::ContainerListing`. A
    /// distroless image with no shell is an ordinary outcome and answers `Unusable` with the
    /// sentence, because an empty tree is a lie about a filesystem that is full.
    pub fn list_dir(&self, container: &str, path: &str) -> cide_ipc::docker::ContainerListing {
        match self.connect() {
            Ok(docker) => docker.list_dir(container, path),
            Err(error) => cide_ipc::docker::ContainerListing::Unusable {
                reason: error.to_string(),
            },
        }
    }

    /// Read one file out of a container. (M44)
    pub fn read_file(
        &self,
        container: &str,
        path: &str,
    ) -> Result<String, cide_docker::DockerError> {
        self.connect()?.read_file(container, path)
    }

    /// The same file as **bytes**, for saving rather than showing. (M47)
    ///
    /// Binary is allowed here and refused by `read_file`, and the difference is the point: a
    /// binary is exactly the thing somebody downloads.
    pub fn read_bytes(
        &self,
        container: &str,
        path: &str,
    ) -> Result<Vec<u8>, cide_docker::DockerError> {
        self.connect()?.read_bytes(container, path)
    }

    /// A directory as the tar `docker cp` would produce. (M47)
    pub fn read_archive(
        &self,
        container: &str,
        path: &str,
    ) -> Result<Vec<u8>, cide_docker::DockerError> {
        self.connect()?.read_archive(container, path)
    }

    /// What the detail pane shows for one thing. (M47)
    ///
    /// Never `Err` for a Docker reason — `DockerDetail::Missing` is the shape for a thing that is
    /// gone, which on this surface is an ordinary outcome rather than a failure.
    pub fn detail(
        &self,
        target: &cide_ipc::docker::InspectTarget,
    ) -> cide_ipc::docker::DockerDetail {
        match self.connect() {
            Ok(docker) => docker.detail(target),
            Err(error) => cide_ipc::docker::DockerDetail::Missing {
                reason: error.to_string(),
            },
        }
    }

    /// Replace a container with one that differs only in what the edit names. (M47)
    pub fn recreate(
        &self,
        id: &str,
        edit: &cide_ipc::docker::ContainerEdit,
    ) -> Result<String, cide_docker::DockerError> {
        self.connect()?.recreate(id, edit)
    }

    /// Note that the daemon moved, and start a flusher if none is waiting.
    ///
    /// Called from the events thread, so it does no I/O and takes no lock but its own.
    pub fn mark_changed(self: &Arc<Self>, app: &AppHandle) {
        /*
         * **There is deliberately no "is anybody watching" gate here.** (M54)
         *
         * M52 added one, as belt and braces beside the subscription being dropped. It is
         * redundant — no subscription means no callbacks, which is self-enforcing — and it is
         * redundant in the dangerous direction: it can only fail *closed*. When the counter and
         * the subscription disagreed (the race `watch_gate` now prevents), this turned a
         * recoverable state into permanent silence, and silence is the one symptom that gives a
         * user nothing to report but "it does not update".
         *
         * The worst case without it is a handful of extra reads from an event already in flight
         * when the last panel closed. That is a bounded cost with a visible cause; the gate's
         * worst case was unbounded and invisible.
         */
        if !self.mark() {
            return;
        }
        let app = app.clone();
        let this = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("cide-docker-emit".to_string())
            .spawn(move || this.flush(&app));
        if let Err(error) = spawned {
            // The flag is already set, so failing quietly would leave the machine unable to ever
            // start another flusher. Clear it, and say so.
            tracing::warn!(%error, "could not start the docker board flusher");
            self.coalesce.lock().flushing = false;
        }
    }

    fn mark(&self) -> bool {
        let mut state = self.coalesce.lock();
        let now = Instant::now();
        state.pending = true;
        state.first.get_or_insert(now);
        state.last = Some(now);
        if state.flushing {
            return false;
        }
        state.flushing = true;
        true
    }

    fn take_due(&self) -> Option<bool> {
        let mut state = self.coalesce.lock();
        let (first, last) = (state.first?, state.last?);
        if last.elapsed() < COALESCE && first.elapsed() < COALESCE_CEILING {
            return None;
        }
        state.first = None;
        state.last = None;
        state.flushing = false;
        Some(std::mem::take(&mut state.pending))
    }

    fn flush(&self, app: &AppHandle) {
        loop {
            std::thread::sleep(COALESCE_TICK);
            if self.take_due().is_none() {
                continue;
            }
            let board = self.board();
            /*
             * One line per emit, at debug. (M52)
             *
             * Added because "the panel does not auto-update" has five links — the daemon's event
             * stream, `watch_events`, this coalescer, the emit, and the webview's listener — and
             * until now the log said nothing about any of them. `bollard` is capped at `Info`
             * since M52 and used to bury this; with it quiet, one line here is the difference
             * between bisecting the chain and guessing at it.
             */
            tracing::debug!(
                rows = match &board {
                    DockerBoard::Ready(ready) => ready.containers.len(),
                    _ => 0,
                },
                "docker: board changed, emitting"
            );
            crate::emit::docker_changed(app, &board);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flush must **re-arm**: the event after one has to start another.
    ///
    /// The single failure that would produce exactly "the panel updates once and then never
    /// again", and it is invisible — `mark` returns `false` while `flushing` is set, so a
    /// `take_due` that forgot to clear the flag would drop every subsequent event silently, with
    /// the event stream, the emit and the listener all working perfectly.
    ///
    /// No `AppHandle` and no daemon: this is the coalescer's own arithmetic, which is the part
    /// that can be got wrong without anything failing.
    #[test]
    fn a_flush_rearms_so_the_next_event_is_not_dropped() {
        let state = DockerState::default();

        assert!(state.mark(), "the first event starts a flusher");
        assert!(
            !state.mark(),
            "and a second one inside the window joins it rather than starting a rival",
        );

        // Nothing is due yet — the whole point of the 250 ms window.
        assert!(state.take_due().is_none());

        // Wait the window out, exactly as `flush` does.
        std::thread::sleep(COALESCE + Duration::from_millis(50));
        assert_eq!(
            state.take_due(),
            Some(true),
            "the pending flag is taken, so the flush knows there was something to report",
        );

        // **The assertion this test exists for.**
        assert!(
            state.mark(),
            "the next event must start a new flusher — a `take_due` that left `flushing` set \
             would make the panel update once and then never again, with every other link in the \
             chain working",
        );

        // And an empty window reports nothing rather than emitting on a timer.
        let idle = DockerState::default();
        assert!(idle.take_due().is_none(), "no event, nothing due");
    }

    /// The count and the action must **agree**, under every interleaving. (M54)
    ///
    /// # The bug this reproduces
    ///
    /// StrictMode double-invokes an effect, so a panel mounting fires `watch(true)`,
    /// `watch(false)`, `watch(true)` as three concurrent Tauri commands on three workers. The
    /// counter is safe under any ordering (+1−1+1 = 1); the *side effects* were not. A `false`
    /// whose stop ran last left the count at 1 with no subscription — and nothing re-reads the
    /// count, so the panel never updated again. Reported twice as "the panel does not update".
    ///
    /// Driven through `watching_with` — the **real** function, with only the action injected —
    /// so the gate, the bookkeeping and the ordering are the ones that ship. The first cut of this
    /// test drove `decide` alone and passed with the gate deleted, which is worth recording: it
    /// was measuring the order of the *decisions*, and the bug was in the order of the *actions*.
    #[test]
    fn the_count_and_the_last_action_agree_under_concurrent_mounts() {
        for _ in 0..200 {
            let state = Arc::new(DockerState::default());
            let order = Mutex::new(Vec::<(bool, WatchAction)>::new());

            // Stands in for the subscription: `true` is "a stream is running".
            let subscribed = Mutex::new(false);

            std::thread::scope(|scope| {
                // Exactly what StrictMode produces, all three at once — and each carries the
                // stamp the frontend gave it, which is what survives being reordered.
                for (seq, wants) in [(1u64, true), (2, false), (3, true)] {
                    let state = Arc::clone(&state);
                    let (order, subscribed) = (&order, &subscribed);
                    scope.spawn(move || {
                        let outcome = state.watching_with("shell:1", seq, wants, |action| {
                            /*
                             * A yield *between* deciding and acting, which is what the real
                             * action does at length: `connect()` negotiates a version over a
                             * socket. This is the window the bug lived in — without the gate the
                             * three actions land in an order unrelated to the three decisions.
                             */
                            std::thread::yield_now();
                            *subscribed.lock() = action == WatchAction::Start;
                            order.lock().push((wants, action));
                        });
                        let _ = outcome;
                    });
                }
            });

            let log = order.lock();
            assert_eq!(
                state.panels.lock().get("shell:1").map(|(_, on)| *on),
                Some(true),
                "the newest statement from the window wins whatever order they arrived in — a \
                 counter lost the decrement here and drifted to 2: {log:?}",
            );
            /*
             * **The invariant, and the whole point.** One watcher and no subscription is a live
             * panel that will never update again — nothing re-reads the count, so there is no way
             * back. It is the state the user reported twice, and it is what the gate prevents.
             */
            assert!(
                *subscribed.lock(),
                "the count says a panel is watching and no subscription is running: {log:?}",
            );
        }
    }

    /// Two windows, and a stale message from either one.
    ///
    /// The counter this replaces could not express either case correctly: a decrement arriving
    /// before its increment was clamped away and lost, and there was nothing to tell a stale
    /// message from a current one. Levels plus a per-window stamp make both trivial.
    #[test]
    fn a_window_speaks_only_for_itself_and_a_stale_message_is_ignored() {
        let state = DockerState::default();
        let decide = |window, seq, on| state.decide(window, seq, on);

        assert_eq!(decide("shell:1", 1, true), (1, WatchAction::Start));
        assert_eq!(
            decide("shell:2", 1, true),
            (2, WatchAction::Start),
            "two windows showing the panel are two watchers",
        );
        assert_eq!(
            decide("shell:1", 2, false),
            (1, WatchAction::Start),
            "and the first closing must not take the stream from the second",
        );
        assert_eq!(
            decide("shell:2", 2, false),
            (0, WatchAction::Stop),
            "the last one closing stops it",
        );

        // The ordering fix on its own: a message that lost a race is discarded rather than
        // applied late. Without it a mount that arrived after its own unmount would be undone.
        assert_eq!(
            decide("shell:1", 1, true),
            (0, WatchAction::Stop),
            "a stamp this window has already passed says nothing",
        );
        assert_eq!(
            decide("shell:1", 3, true),
            (1, WatchAction::Start),
            "and a newer one is heard",
        );

        // Idempotent, which is what lets every call state an action rather than only transitions.
        assert_eq!(decide("shell:1", 4, true), (1, WatchAction::Start));
        assert!(state.watched());
    }
}
