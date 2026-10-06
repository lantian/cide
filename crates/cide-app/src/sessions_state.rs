//! The session journal's home in the running process: `sessions.json`, the two feeds that write
//! it, and the listing the Agents panel's **Sessions** tab draws. (M134)
//!
//! `cide_ipc::sessions` says why the journal exists and `cide_core::sessions` owns its rules;
//! this is the plumbing, on `positions_state`'s pattern — one lock, one file, a debounce and a
//! daemon flusher, with the final write made by `lifecycle::shutdown` rather than left to the loop.
//!
//! # The two feeds
//!
//! * **The tree.** `WorkspaceState::apply` hands every accepted snapshot to [`observe_workspace`],
//!   which is the one place that knows the tree moved — `retitle`'s argument, verbatim. Opening a
//!   Claude tab, binding a session to a pane, a `/clear` giving a pane a new conversation: all of
//!   them are ordinary mutations with no journal code near them.
//! * **The run registry.** `AgentRegistry::flush` hands each project whose roster it just rebuilt
//!   to [`observe_runs`]. Reviews ride along, which `agent-runs.json` leaves out.
//!
//! Both run on hot paths — the first on every click that changes the tree — so each is a walk of
//! a few panes or runs and one mutex, and the merge answers whether anything is worth a write
//! (`last_seen` alone is not, until a minute has passed). The file is written on the flusher.
//!
//! # What the listing hides
//!
//! A row the tab could do nothing with: not live, not on screen, and either never named by its
//! harness (a codex console closed before its first turn) or with no transcript on disk (a
//! claude console opened and closed without a word). "Every session that was doing something" is
//! what the user asked for, and a conversation with no transcript did nothing.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use cide_core::persist::{self, Debouncer};
use cide_core::sessions::{Journal, TranscriptHead};
use cide_ipc::agents::Harness;
use cide_ipc::sessions::{SessionListing, SessionRecord, SessionRow, UserInputPage};
use cide_ipc::{PaneId, ProjectId, SessionId, Workspace};
use parking_lot::{Condvar, Mutex};
use tauri::{AppHandle, Manager};

use crate::state::SessionRegistry;
use crate::workspace_state::WorkspaceState;

/// How often the flusher wakes. The debounce is the persist default; the journal has no reader
/// racing the disk, so there is nothing to gain from writing sooner.
const POLL: Duration = Duration::from_millis(500);

pub struct SessionsState {
    journal: Mutex<Journal>,
    path: PathBuf,
    debounce: Debouncer,
    /// Roots whose rows moved since the last flush, so their windows are told once it is written.
    changed: Mutex<BTreeSet<PathBuf>>,
    /// Transcript → how many bytes it held when its head was last read. A transcript that has not
    /// grown has nothing new to say, and one that is not live never grows.
    heads: Mutex<HashMap<PathBuf, u64>>,
    inputs: Mutex<HashMap<PathBuf, cide_core::user_inputs::TranscriptInputs>>,
    announced_inputs: Mutex<HashMap<PathBuf, (u32, u32)>>,
    /// In particular, Codex's rollout lookup walks dated directories. Resolve once rather
    /// than repeating that walk for every live thread at every poll.
    input_paths: Mutex<HashMap<(PathBuf, String), PathBuf>>,
    codex_activations: Mutex<HashMap<SessionId, (PathBuf, i64)>>,
    wake: Condvar,
    wake_pending: Mutex<bool>,
}

fn journal_path() -> PathBuf {
    persist::state_dir().join("sessions.json")
}

impl SessionsState {
    /// Load from disk, or start empty. A corrupt file is moved aside rather than read as empty
    /// and saved over — `persist::read_or_quarantine`.
    pub fn load() -> Self {
        let path = journal_path();
        Self {
            journal: Mutex::new(persist::read_or_quarantine(&path)),
            path,
            debounce: Debouncer::default(),
            changed: Mutex::new(BTreeSet::new()),
            heads: Mutex::new(HashMap::new()),
            inputs: Mutex::new(HashMap::new()),
            announced_inputs: Mutex::new(HashMap::new()),
            input_paths: Mutex::new(HashMap::new()),
            codex_activations: Mutex::new(HashMap::new()),
            wake: Condvar::new(),
            wake_pending: Mutex::new(false),
        }
    }

    /// Start the background flusher. Call once, from `setup`.
    pub fn start_flusher(self: &Arc<Self>, app: AppHandle) {
        let state = Arc::clone(self);
        thread::Builder::new()
            .name("cide-sessions".into())
            .spawn(move || {
                loop {
                    let mut pending = state.wake_pending.lock();
                    if !*pending {
                        state.wake.wait_for(&mut pending, POLL);
                    }
                    *pending = false;
                    drop(pending);
                    state.refresh_live_inputs(&app);
                    if state.debounce.take() {
                        state.write_now();
                        state.announce(&app);
                    }
                }
            })
            .map(|_| ())
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "no sessions flusher; the session journal will be written on quit only");
            });
    }

    /// Write unconditionally. The flusher's write, and the shutdown's.
    pub fn write_now(&self) {
        let bytes = match serde_json::to_vec_pretty(&*self.journal.lock()) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::error!(%error, "could not encode the session journal");
                return;
            }
        };
        if let Err(error) = persist::write_atomic(&self.path, &bytes) {
            tracing::warn!(%error, "could not save the session journal");
        }
    }

    /// Tell every window showing a project whose rows moved.
    fn announce(&self, app: &AppHandle) {
        let roots = std::mem::take(&mut *self.changed.lock());
        if roots.is_empty() {
            return;
        }
        let Some(workspace) = app.try_state::<WorkspaceState>() else {
            return;
        };
        let projects: Vec<ProjectId> = workspace.with(|ws| {
            ws.projects
                .values()
                .filter(|p| p.roots.first().is_some_and(|r| roots.contains(&r.path)))
                .map(|p| p.id)
                .collect()
        });
        for project in projects {
            crate::emit::sessions_changed(app, project);
        }
    }

    fn upsert_all(&self, sightings: impl IntoIterator<Item = (PathBuf, SessionRecord)>) {
        let mut moved = Vec::new();
        {
            let mut journal = self.journal.lock();
            for (root, record) in sightings {
                if journal.upsert(&root, record) {
                    moved.push(root);
                }
            }
        }
        if moved.is_empty() {
            return;
        }
        self.changed.lock().extend(moved);
        self.debounce.note_change();
    }
}

fn state(app: &AppHandle) -> Option<Arc<SessionsState>> {
    app.try_state::<Arc<SessionsState>>()
        .map(|s| Arc::clone(&s))
}

/// Merge what the tree shows. Called from `WorkspaceState::apply` with the accepted snapshot,
/// outside the workspace lock.
pub fn observe_workspace(app: &AppHandle, ws: &Workspace) {
    let Some(sessions) = state(app) else { return };
    let registry = app.try_state::<SessionRegistry>();
    let sightings = cide_core::sessions::observe(ws, persist::now_ms(), |session| {
        registry
            .as_ref()
            .and_then(|r| r.get(session))
            .map(|pty| pty.spawn_cwd().to_path_buf())
    });
    sessions.upsert_all(sightings);
}

/// Merge what the run registry holds for `project`. Called from `AgentRegistry::flush`.
pub fn observe_runs(app: &AppHandle, agents: &crate::agents::AgentRegistry, project: ProjectId) {
    let Some(sessions) = state(app) else { return };
    let Some(workspace) = app.try_state::<WorkspaceState>() else {
        return;
    };
    let Ok(root) = crate::tasks_state::project_root(&workspace, project) else {
        return;
    };
    let sightings = agents.journal_sightings(project, &root, persist::now_ms());
    sessions.upsert_all(sightings.into_iter().map(|record| (root.clone(), record)));
}

/// Where a conversation's transcript is, when its harness keeps one cide can read and it exists.
///
/// Claude and Qwen file theirs under the directory they were started in; codex by thread id
/// alone (`codex_cli::rollout_of`). opencode's is a database, and MiMo's is not known to cide.
pub(crate) fn transcript_path(record: &SessionRecord) -> Option<PathBuf> {
    if !record.known {
        return None;
    }
    let path = match record.harness {
        Harness::Claude | Harness::Qwen => {
            let session = record.id.parse::<uuid::Uuid>().ok().map(SessionId)?;
            crate::lifecycle::transcript_of(record.harness, &record.cwd, session)?
        }
        Harness::Codex => {
            cide_core::codex_cli::rollout_of(&cide_core::codex_cli::codex_home()?, &record.id)?
        }
        Harness::Opencode | Harness::Mimo => return None,
    };
    path.is_file().then_some(path)
}

/// Whether [`transcript_path`] can ever answer for this harness.
fn keeps_transcript(harness: Harness) -> bool {
    matches!(harness, Harness::Claude | Harness::Qwen | Harness::Codex)
}

/// The Sessions tab's rows for `project`, newest first.
pub fn list(app: &AppHandle, project: ProjectId) -> cide_core::Result<SessionListing> {
    let workspace = app
        .try_state::<WorkspaceState>()
        .ok_or_else(|| cide_core::CoreError::Io("cide is shutting down".into()))?;
    let root = crate::tasks_state::project_root(&workspace, project)?;
    let Some(sessions) = state(app) else {
        return Ok(SessionListing { rows: Vec::new() });
    };
    // Which pane shows which conversation, from every key a pane can be found by.
    let panes: HashMap<String, PaneId> = workspace.with(|ws| {
        let mut panes = HashMap::new();
        for sp in cide_core::workspace::session_panes(ws, Some(project)) {
            let pane = sp.pane;
            let keys = [
                pane.conversation.map(|c| c.to_string()),
                pane.continues.as_ref().map(|c| c.id.clone()),
                Some(sp.session.to_string()),
            ];
            for key in keys.into_iter().flatten() {
                panes.entry(key).or_insert(pane.id);
            }
        }
        panes
    });
    let live: std::collections::HashSet<SessionId> = app
        .try_state::<SessionRegistry>()
        .map(|r| r.ids().into_iter().collect())
        .unwrap_or_default();

    let records = sessions.journal.lock().list(&root);
    let mut rows = Vec::with_capacity(records.len());
    let mut heads: Vec<(String, TranscriptHead)> = Vec::new();
    for mut record in records {
        let is_live = record.session.is_some_and(|s| live.contains(&s));
        let pane = panes
            .get(&record.id)
            .or_else(|| record.session.and_then(|s| panes.get(&s.to_string())))
            .copied();
        let transcript = transcript_path(&record);
        let pointless = !record.known || (keeps_transcript(record.harness) && transcript.is_none());
        if !is_live && pane.is_none() && pointless {
            continue;
        }
        if let Some(path) = &transcript
            && let Some(head) = sessions.read_head(path, &record)
        {
            if record.prompt.is_none() {
                record.prompt.clone_from(&head.prompt);
            }
            if head.title.is_some() {
                record.name.clone_from(&head.title);
            }
            heads.push((record.id.clone(), head));
        }
        rows.push(SessionRow {
            searchable: transcript.is_some(),
            record,
            live: is_live,
            pane,
        });
    }
    // Kept, so the next list does not read the transcript again; written on the debounce, and
    // not announced — the rows just returned already carry it, and an event here would have the
    // tab re-ask for the list it is drawing.
    if !heads.is_empty() {
        let mut journal = sessions.journal.lock();
        let mut moved = false;
        for (id, head) in heads {
            moved |= journal.set_head(&root, &id, head);
        }
        drop(journal);
        if moved {
            sessions.debounce.note_change();
        }
    }
    Ok(SessionListing { rows })
}

impl SessionsState {
    fn input_path(&self, record: &SessionRecord) -> Option<PathBuf> {
        let key = (record.cwd.clone(), record.id.clone());
        if let Some(path) = self.input_paths.lock().get(&key).cloned()
            && path.is_file()
        {
            return Some(path);
        }
        let path = transcript_path(record)?;
        self.input_paths.lock().insert(key, path.clone());
        Some(path)
    }

    /// Hooks may precede the transcript append; the ordinary poll retries that race. Waking
    /// this worker must never read a transcript on the hook applier or GTK loop.
    pub(crate) fn wake_inputs(&self) {
        *self.wake_pending.lock() = true;
        self.wake.notify_one();
    }

    fn refresh_live_inputs(&self, app: &AppHandle) {
        self.refresh_codex_identity(app);
        let Some(workspace) = app.try_state::<WorkspaceState>() else {
            return;
        };
        let projects = workspace.with(|ws| {
            if !ws.settings.terminal.show_recap && !ws.settings.terminal.highlight_user_input {
                return Vec::new();
            }
            ws.projects
                .values()
                .filter_map(|p| p.roots.first().map(|r| (p.id, r.path.clone())))
                .collect::<Vec<_>>()
        });
        let Some(registry) = app.try_state::<SessionRegistry>() else {
            return;
        };
        for (project, root) in projects {
            let records = self.journal.lock().list(&root);
            for record in records {
                if !matches!(record.harness, Harness::Claude | Harness::Codex) {
                    continue;
                }
                let Some(session) = record.session else {
                    continue;
                };
                if registry.get(session).is_none_or(|p| p.has_exited()) {
                    continue;
                }
                // A /clear leaves the old journal row pointing to the same live PTY handle.
                // Only the conversation currently held by that handle may keep polling.
                if registry
                    .native_conversation(session)
                    .is_some_and(|c| c.id != record.id)
                    || registry
                        .conversation_of(session)
                        .is_some_and(|(c, _)| c.to_string() != record.id)
                {
                    continue;
                }
                if let Some(path) = self.input_path(&record) {
                    let version = {
                        let mut inputs = self.inputs.lock();
                        let cache = inputs.entry(path.clone()).or_default();
                        cache.refresh(&path).ok().map(|_| cache.version())
                    };
                    if let Some(version) = version
                        && self.announced_inputs.lock().insert(path, version) != Some(version)
                    {
                        crate::emit::user_inputs_changed(app, project, &record.id);
                    }
                }
            }
        }
    }

    /// `/resume` applies settings without firing a model-turn hook in Codex 0.160. Its process
    /// keeps writers for visited threads, so use their latest explicit settings activation,
    /// never directory-wide mtime. This also catches a picker selection and returning to an
    /// already-open thread. All stat/JSON/process inspection stays on this background worker.
    fn refresh_codex_identity(&self, app: &AppHandle) {
        let Some(registry) = app.try_state::<SessionRegistry>() else {
            return;
        };
        let Some(hooks) = app.try_state::<crate::hooks::HookServer>() else {
            return;
        };
        let live = registry.ids();
        self.codex_activations
            .lock()
            .retain(|id, _| live.contains(id));
        for session in live {
            if registry.harness_of(session) != Some(Harness::Codex) {
                continue;
            }
            let Some(pty) = registry.get(session).filter(|p| !p.has_exited()) else {
                continue;
            };
            let Some(pid) = pty.child_pid() else { continue };
            let paths = cide_core::codex_active::owned_rollouts(pid);
            let mut activations = Vec::new();
            {
                let mut inputs = self.inputs.lock();
                for path in paths {
                    let cache = inputs.entry(path.clone()).or_default();
                    if cache.refresh(&path).is_ok()
                        && let Some(stamp) = cache.activated_at()
                    {
                        activations.push((path, stamp));
                    }
                }
            }
            let Some((path, stamp)) = cide_core::codex_active::latest(activations) else {
                continue;
            };
            let previous = self
                .codex_activations
                .lock()
                .insert(session, (path.clone(), stamp));
            let Some(thread) = cide_core::codex_active::thread_of(&path)
                .and_then(|id| id.parse::<SessionId>().ok())
            else {
                continue;
            };
            // The child's CODEX_HOME may differ from the IDE's. Keep its observed native
            // path so the recap reads this same rollout rather than searching another home.
            self.input_paths.lock().insert(
                (pty.spawn_cwd().to_path_buf(), thread.to_string()),
                path.clone(),
            );
            // A startup snapshot cannot undo a just-applied hook or /clear. A later settings
            // activation may explicitly resume even a cleared thread, including the same id.
            let changed = previous
                .as_ref()
                .is_some_and(|previous| *previous != (path.clone(), stamp));
            if registry
                .conversation_of(session)
                .is_none_or(|(current, since)| current != thread && stamp >= since as i64)
                && (!registry.codex_cleared(session) || changed)
            {
                hooks.codex_resumed(session, thread, &path, stamp);
            }
        }
    }

    pub(crate) fn input_page(&self, record: &SessionRecord, before: Option<u32>) -> UserInputPage {
        if matches!(record.harness, Harness::Claude | Harness::Codex)
            && let Some(path) = self.input_path(record)
        {
            let mut inputs = self.inputs.lock();
            let cache = inputs.entry(path.clone()).or_default();
            if cache.refresh(&path).is_ok() {
                return cache.page(before);
            }
        }
        UserInputPage {
            inputs: Vec::new(),
            total: 0,
            has_previous: false,
            available: false,
            generation: 0,
        }
    }

    /// A transcript's head, unless it has not grown since it was last read — or it has, but the
    /// row already has its prompt and the file cannot have a new name yet (only a `/rename` in a
    /// live conversation adds one, and that grows the file too).
    fn read_head(&self, path: &Path, record: &SessionRecord) -> Option<TranscriptHead> {
        let len = std::fs::metadata(path).ok()?.len();
        let mut heads = self.heads.lock();
        if heads.get(path) == Some(&len) {
            return None;
        }
        heads.insert(path.to_path_buf(), len);
        drop(heads);
        if record.prompt.is_some() && record.harness != Harness::Claude {
            return None;
        }
        let file = std::fs::File::open(path).ok()?;
        Some(cide_core::sessions::transcript_head(
            std::io::BufReader::new(file),
        ))
    }

    /// One row, by conversation id.
    pub(crate) fn record(&self, root: &Path, id: &str) -> Option<SessionRecord> {
        self.journal
            .lock()
            .list(root)
            .into_iter()
            .find(|r| r.id == id)
    }
}

/// The managed state, for the commands.
pub(crate) fn handle(app: &AppHandle) -> Option<Arc<SessionsState>> {
    state(app)
}
