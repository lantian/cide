//! The session journal's rules: what a pane in the tree says about a conversation, and how a
//! sighting merges into what the journal already holds. (M134)
//!
//! `cide_ipc::sessions` says why the journal exists and why it is keyed by conversation. This
//! module is the pure half — no disk, no clock, no registry — so every rule below is a unit test
//! away. `cide-app`'s `sessions_state` feeds it sightings from two places (the workspace tree
//! after every accepted mutation, the agent registry on every roster flush) and owns the file.
//!
//! # The merge, and the two things it must not do
//!
//! * **List one conversation twice.** A run's mirror tab, a console resumed from the Sessions tab
//!   and a codex console that has just learned its thread id are all a *second sighting* of a
//!   conversation the journal already has. They merge by id, by run, or — for a row whose id was
//!   only cide's handle standing in — by that handle, and the row is re-keyed in place.
//! * **Lose what the first sighting knew.** A run knows its role, task and prompt; its mirror pane
//!   knows none of that. A later sighting fills what is missing and never blanks what is there,
//!   and a pane never demotes a run's kind — see `SessionKind`'s doc.
//!
//! `/clear` is deliberately *not* a merge: the CLI starts a new conversation under a new id, the
//! old one stays resumable, and both are rows — which is what the user would expect to find.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cide_ipc::agents::{Harness, RunState};
use cide_ipc::sessions::{
    PROMPT_CLIP, SessionActivity, SessionEdit, SessionGroup, SessionKind, SessionRecord,
};
use cide_ipc::{Pane, PaneOrigin, SessionId, SessionState, Tab, TabKind, Workspace};
use cide_ipc::{PaneKind, PaneRole};
use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// How many conversations the journal keeps per project. The oldest by `last_seen` go first.
///
/// Five hundred rows of a few hundred bytes each is a small file, and far past what anyone
/// scrolls; a cap at all is so that a project used daily for years does not grow a file that is
/// read whole on every list.
pub const PER_PROJECT: usize = 500;

/// How far `last_seen` may advance before it counts as a change worth writing. The tree mutates on
/// every focus change; a journal write per click would be the debounce doing all the work.
const SEEN_GRANULARITY_MS: u64 = 60_000;

/// The file, whole. Keyed by the project's primary root rather than its `ProjectId`, the way the
/// proposal queue is: a project closed and re-opened is the same project to the user.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Journal {
    /// Private storage locators. Kept out of SessionRecord, which can cross the remote wire.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sources: BTreeMap<String, PathBuf>,
    #[serde(default)]
    pub projects: BTreeMap<String, Vec<SessionRecord>>,
    /// The user's groups and labels, per project root. (M199) See [`Organize`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub organize: BTreeMap<String, Organize>,
}

/// How the user arranged a project's sessions: folders in the *Opened* tree, which conversation
/// sits in which, and the names cide gives conversations. (M199)
///
/// # A side table, not fields on `SessionRecord`
///
/// The record crosses the remote wire — `UserRequest.origin` carries one to a phone — and a
/// grouping is the desktop panel's business; `sources` is beside the rows for the same reason.
/// The price of a side table keyed by conversation id is that the keys must **follow a row that
/// is re-keyed** (a codex stand-in learning its thread id), or the membership silently orphans
/// and the session falls out of its group. `Journal::upsert` carries them.
///
/// # What membership promises
///
/// A grouped conversation stays in its group after it ends instead of dropping into *Closed*, so
/// the journal must also **never prune it** at the cap, and the listing must **never hide it**
/// for having no transcript — both of which it would do to an ordinary ended row. Those three are
/// what "never goes to history" means below the panel.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Organize {
    /// Every group, siblings in the user's order (the order of this list).
    #[serde(default)]
    pub groups: Vec<SessionGroup>,
    /// Conversation id → group id.
    #[serde(default)]
    pub members: BTreeMap<String, String>,
    /// Conversation id → cide's name for it.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

impl Organize {
    fn is_empty(&self) -> bool {
        self.groups.is_empty() && self.members.is_empty() && self.labels.is_empty()
    }

    fn group(&self, id: &str) -> Option<&SessionGroup> {
        self.groups.iter().find(|g| g.id == id)
    }

    /// Whether `id` is `ancestor` or nested anywhere under it. Bounded by the group count, so a
    /// cycle that somehow reached the file cannot hang the walk.
    fn within(&self, id: &str, ancestor: &str) -> bool {
        let mut at = Some(id.to_owned());
        for _ in 0..=self.groups.len() {
            let Some(here) = at else { return false };
            if here == ancestor {
                return true;
            }
            at = self.group(&here).and_then(|g| g.parent.clone());
        }
        false
    }
}

/// How long a group's name may be. A name is one line in a 320px panel; a paste of a paragraph is
/// a mistake, and clipping it here keeps the file from carrying it.
const GROUP_NAME_MAX: usize = 120;

fn refused(why: impl Into<String>) -> CoreError {
    CoreError::SessionGroupRefused(why.into())
}

fn clean_name(name: &str) -> Result<String, CoreError> {
    let flat = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return Err(refused("A group needs a name."));
    }
    Ok(flat.chars().take(GROUP_NAME_MAX).collect())
}

fn key(root: &Path) -> String {
    root.to_string_lossy().into_owned()
}

impl Journal {
    /// A project's rows, newest-seen first.
    pub fn list(&self, root: &Path) -> Vec<SessionRecord> {
        let mut rows = self.projects.get(&key(root)).cloned().unwrap_or_default();
        rows.sort_by(|a, b| {
            b.last_seen_unix_ms
                .cmp(&a.last_seen_unix_ms)
                .then(b.started_unix_ms.cmp(&a.started_unix_ms))
        });
        rows
    }

    /// Merge one sighting. Answers whether anything worth writing changed.
    ///
    /// Two duties toward [`Organize`] ride along. A re-keyed row takes its group and label with
    /// it. And a **new** conversation under a handle a grouped row already holds — the next
    /// conversation in the same pane, which is what a `/clear` is — joins that group: the user
    /// filed the pane's work, and a `/clear` that silently dropped it out of the folder would look
    /// like the folder losing a session.
    pub fn upsert(&mut self, root: &Path, seen: SessionRecord) -> bool {
        let root_key = key(root);
        let rows = self.projects.entry(root_key.clone()).or_default();
        let organize = self.organize.get_mut(&root_key);
        let found = rows
            .iter()
            .position(|r| r.id == seen.id)
            .or_else(|| {
                seen.run
                    .and_then(|run| rows.iter().position(|r| r.run == Some(run)))
            })
            .or_else(|| {
                // A stand-in row meeting its real id: the same handle, now named.
                seen.session.and_then(|session| {
                    rows.iter()
                        .position(|r| !r.known && r.session == Some(session))
                })
            });
        let Some(index) = found else {
            let Some(organize) = organize else {
                rows.push(seen);
                prune(rows, |_| false);
                return true;
            };
            if let Some(handle) = seen.session {
                let previous = rows
                    .iter()
                    .filter(|r| r.session == Some(handle) && organize.members.contains_key(&r.id))
                    .max_by_key(|r| r.last_seen_unix_ms)
                    .and_then(|r| organize.members.get(&r.id).cloned());
                if let Some(group) = previous {
                    organize.members.insert(seen.id.clone(), group);
                }
            }
            rows.push(seen);
            // Every insert, not only one that inherited: a filed row is never what the cap drops.
            prune(rows, |id| organize.members.contains_key(id));
            return true;
        };
        let was = rows[index].id.clone();
        let changed = merge(&mut rows[index], seen);
        let now = &rows[index].id;
        if *now != was
            && let Some(organize) = self.organize.get_mut(&root_key)
        {
            if let Some(group) = organize.members.remove(&was) {
                organize.members.insert(now.clone(), group);
            }
            if let Some(label) = organize.labels.remove(&was) {
                organize.labels.insert(now.clone(), label);
            }
        }
        changed
    }

    /// A project's groups and labels, or nothing when it has none.
    pub fn organize(&self, root: &Path) -> Organize {
        self.organize.get(&key(root)).cloned().unwrap_or_default()
    }

    /// Apply one grouping edit. `Ok(false)` when it changed nothing (collapsing a folded group);
    /// a refusal carries the sentence the panel shows.
    pub fn edit(&mut self, root: &Path, edit: SessionEdit) -> Result<bool, CoreError> {
        let root_key = key(root);
        let known = |id: &str| {
            self.projects
                .get(&root_key)
                .is_some_and(|rows| rows.iter().any(|r| r.id == id))
        };
        let row_known = match &edit {
            SessionEdit::Assign { conversation, .. } | SessionEdit::Label { conversation, .. } => {
                known(conversation)
            }
            _ => true,
        };
        if !row_known {
            return Err(refused("That session is no longer in the journal."));
        }
        let organize = self.organize.entry(root_key.clone()).or_default();
        let before = organize.clone();
        let missing = || refused("That group no longer exists.");
        match edit {
            SessionEdit::Create { id, name, parent } => {
                let id = id.trim().to_owned();
                if id.is_empty() || id.len() > 64 || organize.group(&id).is_some() {
                    return Err(refused("A group needs a new id."));
                }
                if let Some(parent) = &parent
                    && organize.group(parent).is_none()
                {
                    return Err(missing());
                }
                organize.groups.push(SessionGroup {
                    id,
                    name: clean_name(&name)?,
                    parent,
                    collapsed: false,
                });
            }
            SessionEdit::Rename { id, name } => {
                let name = clean_name(&name)?;
                let group = organize
                    .groups
                    .iter_mut()
                    .find(|g| g.id == id)
                    .ok_or_else(missing)?;
                group.name = name;
            }
            SessionEdit::Delete { id } => {
                let parent = organize.group(&id).ok_or_else(missing)?.parent.clone();
                for g in organize.groups.iter_mut() {
                    if g.parent.as_deref() == Some(id.as_str()) {
                        g.parent = parent.clone();
                    }
                }
                organize.members.retain(|_, g| *g != id || parent.is_some());
                if let Some(parent) = &parent {
                    for g in organize.members.values_mut() {
                        if *g == id {
                            *g = parent.clone();
                        }
                    }
                }
                organize.groups.retain(|g| g.id != id);
            }
            SessionEdit::Move { id, parent, before } => {
                if organize.group(&id).is_none() {
                    return Err(missing());
                }
                if let Some(parent) = &parent {
                    if organize.group(parent).is_none() {
                        return Err(missing());
                    }
                    if organize.within(parent, &id) {
                        return Err(refused("A group cannot go inside itself."));
                    }
                }
                if let Some(before) = &before {
                    let sibling = organize.group(before).ok_or_else(missing)?;
                    if *before == id || sibling.parent != parent {
                        return Err(refused("A group can only be placed beside its siblings."));
                    }
                }
                let at = organize
                    .groups
                    .iter()
                    .position(|g| g.id == id)
                    .expect("found above");
                let mut group = organize.groups.remove(at);
                group.parent = parent;
                let to = before
                    .and_then(|b| organize.groups.iter().position(|g| g.id == b))
                    .unwrap_or(organize.groups.len());
                organize.groups.insert(to, group);
            }
            SessionEdit::Collapse { id, collapsed } => {
                organize
                    .groups
                    .iter_mut()
                    .find(|g| g.id == id)
                    .ok_or_else(missing)?
                    .collapsed = collapsed;
            }
            SessionEdit::CollapseAll { collapsed } => {
                for g in organize.groups.iter_mut() {
                    g.collapsed = collapsed;
                }
            }
            SessionEdit::Assign {
                conversation,
                group,
            } => match group {
                Some(group) => {
                    if organize.group(&group).is_none() {
                        return Err(missing());
                    }
                    organize.members.insert(conversation, group);
                }
                None => {
                    organize.members.remove(&conversation);
                }
            },
            SessionEdit::Label {
                conversation,
                label,
            } => {
                let flat = label
                    .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
                    .filter(|l| !l.is_empty());
                match flat {
                    Some(l) => {
                        organize
                            .labels
                            .insert(conversation, l.chars().take(GROUP_NAME_MAX).collect());
                    }
                    None => {
                        organize.labels.remove(&conversation);
                    }
                }
            }
        }
        let changed = *organize != before;
        if organize.is_empty() {
            self.organize.remove(&root_key);
        }
        Ok(changed)
    }

    /// Drop memberships and labels whose conversation is no longer a row — a label on a row the
    /// cap pruned. Grouped rows are never pruned, so this only ever tidies; it runs before a
    /// write, the way the stale `sources` are dropped.
    pub fn retain_organized(&mut self) {
        let projects = &self.projects;
        self.organize.retain(|root, organize| {
            let ids: std::collections::BTreeSet<&str> = projects
                .get(root)
                .map(|rows| rows.iter().map(|r| r.id.as_str()).collect())
                .unwrap_or_default();
            organize.members.retain(|id, _| ids.contains(id.as_str()));
            organize.labels.retain(|id, _| ids.contains(id.as_str()));
            !organize.is_empty()
        });
    }

    /// Fill in what a row's transcript says: its first prompt, when the row has none yet, and its
    /// `/rename` name, which can change for as long as the conversation is live. `false` when
    /// there was no row or nothing moved.
    pub fn set_head(&mut self, root: &Path, id: &str, head: TranscriptHead) -> bool {
        let Some(row) = self
            .projects
            .get_mut(&key(root))
            .and_then(|rows| rows.iter_mut().find(|r| r.id == id))
        else {
            return false;
        };
        let before = (row.prompt.clone(), row.name.clone());
        if row.prompt.is_none() {
            row.prompt = head.prompt;
        }
        if head.title.is_some() {
            row.name = head.title;
        }
        before != (row.prompt.clone(), row.name.clone())
    }
}

fn merge(row: &mut SessionRecord, seen: SessionRecord) -> bool {
    let before = row.clone();
    if row.kind.is_run() && !seen.kind.is_run() {
        // A run's mirror, or a pane continuing a run: the pane knows which handle is live and
        // nothing else the run did not know better.
        row.session = seen.session.or(row.session);
    } else {
        if seen.known || !row.known {
            row.id = seen.id;
            row.known = seen.known;
        }
        if seen.kind.is_run() && !row.kind.is_run() {
            row.kind = seen.kind;
        }
        row.planning_policy = seen.planning_policy.or(row.planning_policy);
        if seen.planning_policy.is_some() {
            row.planning_phase = seen.planning_phase;
        }
        row.harness = seen.harness;
        if !seen.title.is_empty() {
            row.title = seen.title;
        }
        row.cwd = seen.cwd;
        row.session = seen.session.or(row.session);
        row.run = seen.run.or(row.run);
        row.agent = seen.agent.or(row.agent.take());
        row.task = seen.task.or(row.task.take());
        row.task_title = seen.task_title.or(row.task_title.take());
        row.branch = seen.branch.or(row.branch.take());
        if row.prompt.is_none() {
            row.prompt = seen.prompt;
        }
        row.started_unix_ms = row.started_unix_ms.min(seen.started_unix_ms);
    }
    let seen_at = seen.last_seen_unix_ms.max(row.last_seen_unix_ms);
    let stale = seen_at.saturating_sub(before.last_seen_unix_ms) >= SEEN_GRANULARITY_MS;
    row.last_seen_unix_ms = seen_at;
    let mut compare = row.clone();
    compare.last_seen_unix_ms = before.last_seen_unix_ms;
    compare != before || stale
}

/// Past the cap, drop the oldest-seen rows — never one `keep` names (a grouped conversation; see
/// [`Organize`]). If grouped rows alone exceed the cap the project keeps them all: the user chose
/// each one, and a cap that evicted a filed session would be the journal un-filing it.
fn prune(rows: &mut Vec<SessionRecord>, keep: impl Fn(&str) -> bool) {
    if rows.len() <= PER_PROJECT {
        return;
    }
    rows.sort_by(|a, b| b.last_seen_unix_ms.cmp(&a.last_seen_unix_ms));
    let mut excess = rows.len() - PER_PROJECT;
    let mut at = rows.len();
    while excess > 0 && at > 0 {
        at -= 1;
        if !keep(&rows[at].id) {
            rows.remove(at);
            excess -= 1;
        }
    }
}

/// Whether a run's state means the run is over: finished, cancelled, failed, or interrupted and
/// waiting on Resume or Discard. (M199)
pub fn run_is_over(state: &RunState) -> bool {
    matches!(
        state,
        RunState::Interrupted
            | RunState::Finished { .. }
            | RunState::Cancelled { .. }
            | RunState::Failed { .. }
    )
}

/// The child the Sessions tab calls a conversation **live** in — `live`, the alive child that is
/// still this conversation's (`conversations::live`) — or `None`. (M199)
///
/// One exception, and the bug it fixes: **a run that is over, with no pane showing it, is not
/// live**, even though its child usually still is. A finished run keeps its child for a follow-up
/// turn, and the reaper never ends a session a run names (ADR 0016), so by liveness alone every
/// finished run sat in *Opened* as *Background · Idle* for as long as the registry held it — and
/// Stop could not move it, because stopping a finished run is, rightly, nothing. A pane continuing
/// or watching it makes it live again: someone is looking at it.
pub fn shown_live(
    live: Option<SessionId>,
    run: Option<&RunState>,
    on_screen: bool,
) -> Option<SessionId> {
    if run.is_some_and(run_is_over) && !on_screen {
        return None;
    }
    live
}

/// What a conversation is doing — the one table behind the Sessions tab's glyphs and its waiting
/// counts. (M199) See [`SessionActivity`] for why it lives here and why a question is not in it.
///
/// `run` is the run's state when the conversation is a run's and the registry still holds the run;
/// `state` is the last hook-derived state of its live child; `live` is whether that child is alive.
///
/// A run's state wins — the Subagents tab draws it, and a run row must read the same on both tabs
/// — **except** for an ended run whose conversation a pane has since continued: the run is
/// finished, but the conversation is live and working again, and its glyph should say so.
pub fn activity(
    run: Option<&RunState>,
    state: Option<SessionState>,
    live: bool,
) -> SessionActivity {
    use SessionActivity as A;
    if let Some(run) = run.filter(|r| !(live && run_is_over(r))) {
        return match run {
            RunState::Queued | RunState::Starting => A::Starting,
            RunState::Running => A::Working,
            RunState::Blocked { .. } => A::Blocked,
            RunState::Idle => A::Idle,
            RunState::AwaitingPermission => A::Permission,
            RunState::Paused { .. } => A::Paused,
            RunState::Interrupted | RunState::Cancelled { .. } => A::Stopped,
            RunState::Finished { .. } => A::Finished,
            RunState::Failed { .. } => A::Failed,
        };
    }
    match (live, state) {
        (_, Some(SessionState::Exited { code })) => {
            if code == 0 {
                A::Finished
            } else {
                A::Stopped
            }
        }
        (false, _) => A::Finished,
        (true, Some(SessionState::Spawning | SessionState::Splash)) => A::Starting,
        (true, Some(SessionState::Busy)) => A::Working,
        (true, Some(SessionState::AwaitingPermission)) => A::Permission,
        (true, Some(SessionState::AwaitingInput)) => A::AwaitingInput,
        (true, Some(SessionState::Paused)) => A::Paused,
        // A live child the hooks have not described — opencode, a codex console between jobs —
        // is alive and asking nothing, which is Idle. `Starting` would sit on it for its life.
        (true, Some(SessionState::Idle) | None) => A::Idle,
    }
}

/// One line of what was asked: whitespace collapsed, at most [`PROMPT_CLIP`] characters.
pub fn clip_prompt(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= PROMPT_CLIP {
        return flat;
    }
    let mut out: String = flat.chars().take(PROMPT_CLIP - 1).collect();
    out.push('…');
    out
}

/// Every agent conversation the tree shows, with the project root each belongs under.
///
/// Agent panes only (`PaneKind::Claude`, which covers every harness through `Pane::harness`):
/// a shell's scrollback is not a conversation anything can reopen. `cwd_of` is where the caller
/// knows a session's spawn directory — a worker tab stands in its worktree, and only the running
/// child knows which — and `None` falls back to the directory the pane records, then the root.
pub fn observe(
    ws: &Workspace,
    now_ms: u64,
    cwd_of: impl Fn(SessionId) -> Option<PathBuf>,
) -> Vec<(PathBuf, SessionRecord)> {
    let mut out = Vec::new();
    for project in ws.projects.values() {
        let Some(root) = project.roots.first().map(|r| r.path.clone()) else {
            continue;
        };
        for tab in &project.tabs {
            for pane in tab.tree.panes.values() {
                if let Some(record) = of_pane(pane, Some(tab), &root, now_ms, &cwd_of) {
                    out.push((root.clone(), record));
                }
            }
        }
        for pane in project.detached.values() {
            if let Some(record) = of_pane(pane, None, &root, now_ms, &cwd_of) {
                out.push((root.clone(), record));
            }
        }
    }
    out
}

fn of_pane(
    pane: &Pane,
    tab: Option<&Tab>,
    root: &Path,
    now_ms: u64,
    cwd_of: &impl Fn(SessionId) -> Option<PathBuf>,
) -> Option<SessionRecord> {
    if pane.kind != PaneKind::Claude {
        return None;
    }
    let session = pane.session?;
    let harness = pane.harness.unwrap_or(Harness::Claude);
    let (id, known) = if let Some(conversation) = pane.conversation {
        (conversation.to_string(), true)
    } else if let Some(continues) = &pane.continues {
        (continues.id.clone(), true)
    } else {
        // Claude and Qwen take cide's handle as their `--session-id`, so the handle *is* the
        // conversation. The others name their own, and until they have, this is a stand-in.
        (
            session.to_string(),
            matches!(harness, Harness::Claude | Harness::Qwen),
        )
    };
    let cwd = pane
        .continues
        .as_ref()
        .map(|c| c.cwd.clone())
        .or_else(|| cwd_of(session))
        .unwrap_or_else(|| root.to_path_buf());
    let kind = match pane.origin {
        Some(PaneOrigin::Worker) => SessionKind::Worker,
        Some(PaneOrigin::Planner) => SessionKind::Planner,
        Some(PaneOrigin::Reviewer) => SessionKind::Reviewer,
        None => match tab.map(|t| &t.kind) {
            Some(TabKind::ClaudeHome) | None if pane.role == PaneRole::Primary => {
                SessionKind::Console
            }
            Some(TabKind::ClaudeHome) => SessionKind::ConsoleSplit,
            _ => SessionKind::Tab,
        },
    };
    let title = match tab.map(|t| &t.kind) {
        Some(TabKind::ClaudeFull { title, .. }) if !title.is_empty() => title.clone(),
        _ => pane.title.clone(),
    };
    let branch = worktree_name(root, &cwd);
    Some(SessionRecord {
        planning_policy: None,
        planning_phase: None,
        id,
        known,
        kind,
        harness,
        title,
        name: None,
        cwd,
        session: Some(session),
        run: None,
        agent: None,
        task: None,
        task_title: None,
        prompt: None,
        branch,
        started_unix_ms: now_ms,
        last_seen_unix_ms: now_ms,
    })
}

/// The worktree `cwd` is, when it is one of the project's `.cide/worktrees/<name>`.
pub fn worktree_name(root: &Path, cwd: &Path) -> Option<String> {
    let rest = cwd.strip_prefix(root).ok()?;
    let mut parts = rest.components();
    let (a, b, name) = (parts.next()?, parts.next()?, parts.next()?);
    (a.as_os_str() == ".cide" && b.as_os_str() == "worktrees")
        .then(|| name.as_os_str().to_string_lossy().into_owned())
}

// --- reading a transcript ----------------------------------------------------------------
//
// `cide-app`'s `lifecycle::transcript_exists` says cide never opens a transcript, and until M134
// that was true. The Sessions tab needs two things only a transcript holds — what a console
// conversation was first asked, and (when the user asks for it) whether a conversation ever said
// some word — so these two readers are the exception, and they keep the old rule's spirit:
// read-only, bounded, and every way of being wrong (a format change, a line that does not parse,
// a file that is not there) answers *nothing*, never an error. A wrong "nothing" costs the user
// one row without its prompt line or one missed search hit; a transcript layout cide misread into
// a failure would cost them the tab.
//
// The three CLIs file one JSON object per line and name the speaker differently:
//
// * claude and qwen: `{"type":"user","message":{"content": "…" | [{"type":"text","text":"…"}]}}`,
//   with `isMeta: true` on lines the CLI wrote itself, and qwen's `message.parts[].text`;
// * codex: `event_msg` with `user_message.message`, or (0.160) `item_completed` with a
//   `UserMessage` item's `content[].text`, for what the user typed. Its `response_item` user
//   messages also carry AGENTS.md and approval-review preambles, which are not what anybody
//   would call the first prompt. Read the completed user item, not that model-input mirror.

/// How much of a transcript the first-prompt reader looks at. The first user message is near the
/// top; a transcript whose first megabyte holds none has nothing a row could show.
const FIRST_PROMPT_BYTES: u64 = 1024 * 1024;

/// What a transcript's opening says about it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptHead {
    /// The first thing the user typed that was not a command or a wrapper.
    pub prompt: Option<String>,
    /// A `/rename` title, when the CLI filed one (claude's `custom-title` line).
    pub title: Option<String>,
}

/// Read the head of a transcript. Never fails: a reader error ends the read with what it had.
pub fn transcript_head(reader: impl std::io::BufRead) -> TranscriptHead {
    use std::io::BufRead as _;
    let mut head = TranscriptHead::default();
    for line in reader.take(FIRST_PROMPT_BYTES).lines() {
        let Ok(line) = line else { break };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value["type"] == "custom-title"
            && let Some(title) = value["customTitle"].as_str()
            && !title.trim().is_empty()
        {
            head.title = Some(title.trim().to_owned());
        }
        if head.prompt.is_none() {
            head.prompt = user_texts(&value)
                .into_iter()
                .find(|text| is_typed(text))
                .map(|text| clip_prompt(&text));
        }
    }
    head
}

/// Whether a user message is something a person typed rather than a wrapper the CLI or cide put
/// in the user's turn: `<command-name>`, `<local-command-caveat>`, `<environment_context>`, …
fn is_typed(text: &str) -> bool {
    let text = text.trim_start();
    !text.is_empty() && !text.starts_with('<') && !text.starts_with("Caveat:")
}

/// The texts of a user message line, in any of the three shapes above; empty for anything else.
pub(crate) fn user_texts(value: &serde_json::Value) -> Vec<String> {
    if value["type"] == "user" && value["isMeta"] != true {
        return texts_of(&value["message"]);
    }
    if value["type"] == "event_msg" && value["payload"]["type"] == "user_message" {
        return value["payload"]["message"]
            .as_str()
            .map(|s| vec![s.to_owned()])
            .unwrap_or_default();
    }
    if value["type"] == "event_msg"
        && value["payload"]["type"] == "item_completed"
        && value["payload"]["item"]["type"] == "UserMessage"
    {
        return texts_of(&value["payload"]["item"]);
    }
    Vec::new()
}

fn texts_of(message: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(text) = message["content"].as_str() {
        out.push(text.to_owned());
    }
    for key in ["content", "parts"] {
        if let Some(items) = message[key].as_array() {
            for item in items {
                let kind = item["type"].as_str().unwrap_or("text");
                if matches!(kind, "text" | "input_text")
                    && let Some(text) = item["text"].as_str()
                {
                    out.push(text.to_owned());
                }
            }
        }
    }
    out
}

/// Every string a line carries, for the search: messages, tool inputs and results alike —
/// "anything that was said in it" is the question the user asked. Except the bookkeeping: a
/// search for `user` that matched every line's `"type":"user"` would find every transcript.
fn strings_of(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(items) => items.iter().for_each(|v| strings_of(v, out)),
        serde_json::Value::Object(map) => map
            .iter()
            .filter(|(key, _)| !BOOKKEEPING.contains(&key.as_str()))
            .for_each(|(_, v)| strings_of(v, out)),
        _ => {}
    }
}

/// Keys whose values are the CLI's own records of a line rather than anything said in it.
const BOOKKEEPING: &[&str] = &[
    "type",
    "role",
    "uuid",
    "parentUuid",
    "sessionId",
    "session_id",
    "id",
    "tool_use_id",
    "toolUseID",
    "requestId",
    "timestamp",
    "cwd",
    "gitBranch",
    "version",
    "model",
    "userType",
    "signature",
    "stop_reason",
];

/// How much text a snippet shows before and after the match, in characters.
const SNIPPET_BEFORE: usize = 40;
const SNIPPET_AFTER: usize = 90;

/// The first place a transcript says `needle`, case-insensitively, as a one-line snippet.
///
/// `needle` must already be lowercase. The raw line is tested first and parsed only when it
/// matches: most lines do not, and parsing every line of a long transcript is the whole cost of
/// the search. The raw test is sound only for a needle JSON does not escape — no quote, no
/// backslash, no control character — so any other needle parses every line.
pub fn transcript_find(reader: impl std::io::BufRead, needle: &str) -> Option<String> {
    if needle.is_empty() {
        return None;
    }
    let raw_ok = !needle
        .chars()
        .any(|c| c == '"' || c == '\\' || c.is_control());
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if raw_ok && !line.to_lowercase().contains(needle) {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let mut strings = Vec::new();
        strings_of(&value, &mut strings);
        for text in strings {
            if let Some(snippet) = snippet(&text, needle) {
                return Some(snippet);
            }
        }
    }
    None
}

fn snippet(text: &str, needle: &str) -> Option<String> {
    // Searched char by char over the lowercased text, so the offset found is a *character*
    // index that means the same thing in the original — a byte offset would not, for the few
    // characters whose lowercase is a different length.
    let lower: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let original: Vec<char> = text.chars().collect();
    let chars: &[char] = if lower.len() == original.len() {
        &original
    } else {
        &lower
    };
    let needle: Vec<char> = needle.chars().collect();
    let at = lower
        .windows(needle.len())
        .position(|w| w == needle.as_slice())?;
    let start = at.saturating_sub(SNIPPET_BEFORE);
    let end = (at + needle.len() + SNIPPET_AFTER).min(chars.len());
    let body: String = chars[start..end].iter().collect();
    let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(format!(
        "{}{flat}{}",
        if start > 0 { "…" } else { "" },
        if end < chars.len() { "…" } else { "" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_locators_are_private_to_the_journal_and_old_journals_load() {
        let old: Journal = serde_json::from_value(serde_json::json!({"projects": {}})).unwrap();
        assert!(old.sources.is_empty());
        let mut journal = old;
        journal.sources.insert(
            "conversation".into(),
            PathBuf::from("/private/custom-home/session.jsonl"),
        );
        let row = record("conversation", SessionKind::Console);
        assert!(!serde_json::to_string(&row).unwrap().contains("/private/"));
        assert!(
            serde_json::to_string(&journal)
                .unwrap()
                .contains("/private/")
        );
    }

    #[test]
    fn the_head_skips_wrappers_and_meta_lines_and_finds_the_title() {
        let transcript = [
            r#"{"type":"custom-title","customTitle":"design pass"}"#,
            r#"{"type":"user","isMeta":true,"message":{"content":"meta line"}}"#,
            r#"{"type":"user","message":{"content":"<command-name>/clear</command-name>"}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"x"}]}}"#,
            r#"not json"#,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"Add a  sessions\ntab"}]}}"#,
            r#"{"type":"user","message":{"content":"later"}}"#,
        ]
        .join("\n");
        let head = transcript_head(transcript.as_bytes());
        assert_eq!(head.prompt.as_deref(), Some("Add a sessions tab"));
        assert_eq!(head.title.as_deref(), Some("design pass"));
    }

    #[test]
    fn a_codex_head_is_what_the_user_typed_not_the_preamble() {
        let transcript = [
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"AGENTS.md instructions"}]}}"#,
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"fix the build"}}"#,
        ]
        .join("\n");
        assert_eq!(
            transcript_head(transcript.as_bytes()).prompt.as_deref(),
            Some("fix the build")
        );
    }

    #[test]
    fn a_search_finds_text_the_json_escaped_and_ignores_keys() {
        let transcript = [
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"The Gate\nruns \"check\" first"}]}}"#,
        ]
        .join("\n");
        let hit = transcript_find(transcript.as_bytes(), "\"check\"").expect("quoted needle");
        assert!(hit.contains("\"check\""), "{hit}");
        let hit = transcript_find(transcript.as_bytes(), "the gate").expect("case-insensitive");
        assert!(hit.starts_with("The Gate"), "{hit}");
        assert!(
            transcript_find(transcript.as_bytes(), "assistant").is_none(),
            "a key or a type is not something said"
        );
    }

    #[test]
    fn a_snippet_is_trimmed_around_the_match() {
        let long = format!("{} needle {}", "a ".repeat(100), "b ".repeat(100));
        let s = snippet(&long, "needle").expect("found");
        assert!(
            s.starts_with('…') && s.ends_with('…') && s.contains("needle"),
            "{s}"
        );
    }

    fn record(id: &str, kind: SessionKind) -> SessionRecord {
        SessionRecord {
            planning_policy: None,
            planning_phase: None,
            id: id.into(),
            known: true,
            kind,
            harness: Harness::Claude,
            title: "t".into(),
            name: None,
            cwd: PathBuf::from("/p"),
            session: None,
            run: None,
            agent: None,
            task: None,
            task_title: None,
            prompt: None,
            branch: None,
            started_unix_ms: 1_000,
            last_seen_unix_ms: 1_000,
        }
    }

    const ROOT: &str = "/p";

    #[test]
    fn a_second_sighting_keeps_the_first_start_and_changes_nothing() {
        let mut j = Journal::default();
        assert!(j.upsert(Path::new(ROOT), record("a", SessionKind::Console)));
        let mut again = record("a", SessionKind::Tab);
        again.started_unix_ms = 5_000;
        again.last_seen_unix_ms = 5_000;
        assert!(
            !j.upsert(Path::new(ROOT), again),
            "a focus change is not news"
        );
        let rows = j.list(Path::new(ROOT));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].started_unix_ms, 1_000);
        assert_eq!(
            rows[0].kind,
            SessionKind::Console,
            "first seen wins between pane kinds"
        );
    }

    #[test]
    fn a_pane_never_demotes_a_run_nor_blanks_what_it_knew() {
        let mut j = Journal::default();
        let mut run = record("c", SessionKind::Subagent);
        run.agent = Some("coder".into());
        run.prompt = Some("do it".into());
        j.upsert(Path::new(ROOT), run);
        let mut mirror = record("c", SessionKind::Tab);
        mirror.session = Some(SessionId(uuid::Uuid::from_u128(7)));
        assert!(j.upsert(Path::new(ROOT), mirror));
        let row = &j.list(Path::new(ROOT))[0];
        assert_eq!(row.kind, SessionKind::Subagent);
        assert_eq!(row.agent.as_deref(), Some("coder"));
        assert_eq!(row.prompt.as_deref(), Some("do it"));
        assert_eq!(row.session, Some(SessionId(uuid::Uuid::from_u128(7))));
    }

    #[test]
    fn a_stand_in_is_re_keyed_when_the_harness_names_its_conversation() {
        let mut j = Journal::default();
        let handle = SessionId(uuid::Uuid::from_u128(1));
        let mut stand_in = record(&handle.to_string(), SessionKind::Console);
        stand_in.known = false;
        stand_in.harness = Harness::Codex;
        stand_in.session = Some(handle);
        j.upsert(Path::new(ROOT), stand_in);
        let mut named = record("thread-9", SessionKind::Console);
        named.harness = Harness::Codex;
        named.session = Some(handle);
        assert!(j.upsert(Path::new(ROOT), named));
        let rows = j.list(Path::new(ROOT));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "thread-9");
        assert!(rows[0].known);
    }

    #[test]
    fn a_clear_is_a_second_conversation_not_a_rename() {
        let mut j = Journal::default();
        let handle = SessionId(uuid::Uuid::from_u128(2));
        let mut before = record(&handle.to_string(), SessionKind::Console);
        before.session = Some(handle);
        j.upsert(Path::new(ROOT), before);
        let mut after = record("after-clear", SessionKind::Console);
        after.session = Some(handle);
        j.upsert(Path::new(ROOT), after);
        assert_eq!(j.list(Path::new(ROOT)).len(), 2);
    }

    #[test]
    fn a_run_is_found_by_its_run_id_before_its_conversation_is_named() {
        let mut j = Journal::default();
        let run = cide_ipc::RunId(uuid::Uuid::from_u128(3));
        let mut queued = record("run:x", SessionKind::Subagent);
        queued.known = false;
        queued.run = Some(run);
        j.upsert(Path::new(ROOT), queued);
        let mut named = record("ses_1", SessionKind::Subagent);
        named.run = Some(run);
        j.upsert(Path::new(ROOT), named);
        let rows = j.list(Path::new(ROOT));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "ses_1");
    }

    #[test]
    fn the_oldest_rows_go_past_the_cap() {
        let mut j = Journal::default();
        for i in 0..(PER_PROJECT as u64 + 3) {
            let mut r = record(&format!("s{i}"), SessionKind::Tab);
            r.last_seen_unix_ms = i;
            j.upsert(Path::new(ROOT), r);
        }
        let rows = j.list(Path::new(ROOT));
        assert_eq!(rows.len(), PER_PROJECT);
        assert!(rows.iter().all(|r| r.id != "s0" && r.id != "s2"));
    }

    fn group_edit(j: &mut Journal, edit: SessionEdit) -> Result<bool, CoreError> {
        j.edit(Path::new(ROOT), edit)
    }

    fn create(j: &mut Journal, id: &str, parent: Option<&str>) {
        group_edit(
            j,
            SessionEdit::Create {
                id: id.into(),
                name: format!("  group\n{id} "),
                parent: parent.map(Into::into),
            },
        )
        .expect("created");
    }

    fn assign(j: &mut Journal, conversation: &str, group: Option<&str>) {
        group_edit(
            j,
            SessionEdit::Assign {
                conversation: conversation.into(),
                group: group.map(Into::into),
            },
        )
        .expect("assigned");
    }

    #[test]
    fn an_old_journal_loads_with_no_groups_and_an_empty_organize_is_not_written() {
        let old: Journal = serde_json::from_value(serde_json::json!({"projects": {}})).unwrap();
        assert!(old.organize.is_empty());
        let mut j = old;
        j.upsert(Path::new(ROOT), record("a", SessionKind::Console));
        create(&mut j, "g", None);
        assert_eq!(j.organize(Path::new(ROOT)).groups[0].name, "group g");
        group_edit(&mut j, SessionEdit::Delete { id: "g".into() }).unwrap();
        assert!(
            !serde_json::to_string(&j).unwrap().contains("organize"),
            "a project with nothing arranged writes no entry"
        );
    }

    #[test]
    fn a_group_is_refused_a_blank_name_a_duplicate_id_and_an_unknown_parent() {
        let mut j = Journal::default();
        create(&mut j, "g", None);
        let blank = SessionEdit::Create {
            id: "h".into(),
            name: " \n ".into(),
            parent: None,
        };
        assert!(matches!(
            group_edit(&mut j, blank),
            Err(CoreError::SessionGroupRefused(_))
        ));
        let dup = SessionEdit::Create {
            id: "g".into(),
            name: "x".into(),
            parent: None,
        };
        assert!(group_edit(&mut j, dup).is_err());
        let orphan = SessionEdit::Create {
            id: "h".into(),
            name: "x".into(),
            parent: Some("nope".into()),
        };
        assert!(group_edit(&mut j, orphan).is_err());
        let ghost = SessionEdit::Assign {
            conversation: "not-a-row".into(),
            group: Some("g".into()),
        };
        assert!(
            group_edit(&mut j, ghost).is_err(),
            "an id the journal does not hold"
        );
    }

    #[test]
    fn a_group_cannot_move_into_itself_and_is_placed_before_a_sibling() {
        let mut j = Journal::default();
        create(&mut j, "a", None);
        create(&mut j, "b", Some("a"));
        create(&mut j, "c", None);
        let into_child = SessionEdit::Move {
            id: "a".into(),
            parent: Some("b".into()),
            before: None,
        };
        assert!(group_edit(&mut j, into_child).is_err());
        let into_self = SessionEdit::Move {
            id: "a".into(),
            parent: Some("a".into()),
            before: None,
        };
        assert!(group_edit(&mut j, into_self).is_err());
        let before_a = SessionEdit::Move {
            id: "c".into(),
            parent: None,
            before: Some("a".into()),
        };
        assert!(group_edit(&mut j, before_a).unwrap());
        let order: Vec<_> = j
            .organize(Path::new(ROOT))
            .groups
            .into_iter()
            .map(|g| g.id)
            .collect();
        assert_eq!(order, ["c", "a", "b"]);
        let beside_a_stranger = SessionEdit::Move {
            id: "c".into(),
            parent: None,
            before: Some("b".into()),
        };
        assert!(
            group_edit(&mut j, beside_a_stranger).is_err(),
            "b is not a top-level sibling"
        );
    }

    #[test]
    fn deleting_a_group_moves_what_it_held_up_a_level() {
        let mut j = Journal::default();
        for id in ["x", "y", "z"] {
            j.upsert(Path::new(ROOT), record(id, SessionKind::Tab));
        }
        create(&mut j, "top", None);
        create(&mut j, "mid", Some("top"));
        create(&mut j, "leaf", Some("mid"));
        assign(&mut j, "x", Some("mid"));
        assign(&mut j, "y", Some("top"));
        group_edit(&mut j, SessionEdit::Delete { id: "mid".into() }).unwrap();
        let o = j.organize(Path::new(ROOT));
        assert_eq!(o.members.get("x").map(String::as_str), Some("top"));
        assert_eq!(
            o.group("leaf").and_then(|g| g.parent.as_deref()),
            Some("top")
        );
        group_edit(&mut j, SessionEdit::Delete { id: "top".into() }).unwrap();
        let o = j.organize(Path::new(ROOT));
        assert!(
            o.members.is_empty(),
            "at the top level its sessions go loose"
        );
        assert_eq!(o.group("leaf").and_then(|g| g.parent.clone()), None);
    }

    #[test]
    fn a_re_keyed_row_keeps_its_group_and_label() {
        let mut j = Journal::default();
        let handle = SessionId(uuid::Uuid::from_u128(11));
        let mut stand_in = record(&handle.to_string(), SessionKind::Console);
        stand_in.known = false;
        stand_in.harness = Harness::Codex;
        stand_in.session = Some(handle);
        j.upsert(Path::new(ROOT), stand_in);
        create(&mut j, "g", None);
        assign(&mut j, &handle.to_string(), Some("g"));
        group_edit(
            &mut j,
            SessionEdit::Label {
                conversation: handle.to_string(),
                label: Some("build fix".into()),
            },
        )
        .unwrap();
        let mut named = record("thread-1", SessionKind::Console);
        named.harness = Harness::Codex;
        named.session = Some(handle);
        j.upsert(Path::new(ROOT), named);
        let o = j.organize(Path::new(ROOT));
        assert_eq!(o.members.get("thread-1").map(String::as_str), Some("g"));
        assert_eq!(
            o.labels.get("thread-1").map(String::as_str),
            Some("build fix")
        );
        assert!(!o.members.contains_key(&handle.to_string()));
    }

    #[test]
    fn a_clear_in_a_grouped_pane_stays_in_the_group() {
        let mut j = Journal::default();
        let handle = SessionId(uuid::Uuid::from_u128(12));
        let mut first = record(&handle.to_string(), SessionKind::Tab);
        first.session = Some(handle);
        j.upsert(Path::new(ROOT), first);
        create(&mut j, "g", None);
        assign(&mut j, &handle.to_string(), Some("g"));
        let mut after = record("after-clear", SessionKind::Tab);
        after.session = Some(handle);
        j.upsert(Path::new(ROOT), after);
        let o = j.organize(Path::new(ROOT));
        assert_eq!(o.members.get("after-clear").map(String::as_str), Some("g"));
        let mut stranger = record("elsewhere", SessionKind::Tab);
        stranger.session = Some(SessionId(uuid::Uuid::from_u128(13)));
        j.upsert(Path::new(ROOT), stranger);
        assert!(
            !j.organize(Path::new(ROOT))
                .members
                .contains_key("elsewhere")
        );
    }

    #[test]
    fn a_grouped_row_survives_the_cap() {
        let mut j = Journal::default();
        j.upsert(Path::new(ROOT), record("filed", SessionKind::Tab));
        create(&mut j, "g", None);
        assign(&mut j, "filed", Some("g"));
        for i in 0..(PER_PROJECT as u64 + 3) {
            let mut r = record(&format!("s{i}"), SessionKind::Tab);
            r.last_seen_unix_ms = 2_000 + i;
            j.upsert(Path::new(ROOT), r);
        }
        let rows = j.list(Path::new(ROOT));
        assert_eq!(rows.len(), PER_PROJECT);
        assert!(
            rows.iter().any(|r| r.id == "filed"),
            "the oldest row, but filed"
        );
        assert!(rows.iter().all(|r| r.id != "s0" && r.id != "s3"));
    }

    #[test]
    fn retaining_drops_labels_of_pruned_rows_only() {
        let mut j = Journal::default();
        j.upsert(Path::new(ROOT), record("kept", SessionKind::Tab));
        group_edit(
            &mut j,
            SessionEdit::Label {
                conversation: "kept".into(),
                label: Some("mine".into()),
            },
        )
        .unwrap();
        j.organize
            .get_mut(ROOT)
            .unwrap()
            .labels
            .insert("gone".into(), "stale".into());
        j.retain_organized();
        let o = j.organize(Path::new(ROOT));
        assert!(o.labels.contains_key("kept") && !o.labels.contains_key("gone"));
    }

    #[test]
    fn collapsing_twice_is_no_change() {
        let mut j = Journal::default();
        create(&mut j, "g", None);
        let fold = || SessionEdit::Collapse {
            id: "g".into(),
            collapsed: true,
        };
        assert!(group_edit(&mut j, fold()).unwrap());
        assert!(!group_edit(&mut j, fold()).unwrap());
        assert!(group_edit(&mut j, SessionEdit::CollapseAll { collapsed: false }).unwrap());
    }

    #[test]
    fn a_finished_run_nobody_shows_is_not_live_though_its_child_is() {
        let child = Some(SessionId(uuid::Uuid::from_u128(21)));
        let finished = RunState::Finished { code: 0 };
        assert_eq!(shown_live(child, Some(&finished), false), None);
        assert_eq!(
            shown_live(child, Some(&finished), true),
            child,
            "a pane watching or continuing it"
        );
        assert_eq!(
            shown_live(child, Some(&RunState::Idle), false),
            child,
            "an idle run waits for more and is live"
        );
        assert_eq!(
            shown_live(child, None, false),
            child,
            "not a run: liveness alone"
        );
        assert_eq!(shown_live(None, Some(&RunState::Running), true), None);
        assert_eq!(
            activity(Some(&finished), Some(SessionState::Idle), false),
            SessionActivity::Finished,
            "and it reads as finished, not idle"
        );
    }

    #[test]
    fn the_activity_table() {
        use SessionActivity as A;
        let exited = |code| Some(SessionState::Exited { code });
        // A run's state wins…
        assert_eq!(activity(Some(&RunState::Running), None, true), A::Working);
        assert_eq!(activity(Some(&RunState::Queued), None, false), A::Starting);
        assert_eq!(
            activity(Some(&RunState::Blocked { reason: "x".into() }), None, true),
            A::Blocked
        );
        assert_eq!(
            activity(Some(&RunState::AwaitingPermission), None, true),
            A::Permission
        );
        assert_eq!(
            activity(Some(&RunState::Failed { reason: "x".into() }), None, false),
            A::Failed
        );
        assert_eq!(
            activity(Some(&RunState::Interrupted), None, false),
            A::Stopped
        );
        // …except over a pane continuing an ended run.
        assert_eq!(
            activity(
                Some(&RunState::Finished { code: 0 }),
                Some(SessionState::Busy),
                true
            ),
            A::Working
        );
        // A pane's session, from its hooks.
        assert_eq!(
            activity(None, Some(SessionState::AwaitingInput), true),
            A::AwaitingInput
        );
        assert_eq!(
            activity(None, Some(SessionState::Splash), true),
            A::Starting
        );
        assert_eq!(activity(None, None, true), A::Idle, "undescribed but alive");
        assert_eq!(activity(None, None, false), A::Finished);
        assert_eq!(activity(None, exited(0), false), A::Finished);
        assert_eq!(activity(None, exited(137), false), A::Stopped);
        assert_eq!(
            activity(None, Some(SessionState::Busy), false),
            A::Finished,
            "a stale Busy on a dead child is not work"
        );
    }

    #[test]
    fn a_prompt_is_one_clipped_line() {
        assert_eq!(clip_prompt("  fix\n\nthe  bug "), "fix the bug");
        let long = "x".repeat(PROMPT_CLIP + 10);
        assert_eq!(clip_prompt(&long).chars().count(), PROMPT_CLIP);
    }

    #[test]
    fn a_worktree_is_named_from_its_path() {
        let root = Path::new("/p");
        assert_eq!(
            worktree_name(root, Path::new("/p/.cide/worktrees/coder-t-1")).as_deref(),
            Some("coder-t-1")
        );
        assert_eq!(worktree_name(root, Path::new("/p")), None);
        assert_eq!(worktree_name(root, Path::new("/elsewhere")), None);
    }
}
