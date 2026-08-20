//! Cancellation for the commit-log walk: one job per tool tab, and the flag `cide-git` borrows.
//! (M20)
//!
//! # Why the registry is here and not in `cide-git`
//!
//! `cide-git`'s `lib.rs` states the rule in its own words under *No cached state*: every entry
//! point opens what it needs and drops it, because a handle held across calls goes stale the
//! moment anything outside cide touches the repository. A registry of running walks is exactly
//! the state that rule forbids, so it lives on this side of the wire and the crate merely
//! *borrows* an `&AtomicBool` for the length of one call — [`cide_git::log::LogWalk`].
//!
//! `&AtomicBool` and not `&dyn Fn() -> bool`: see that module's header, which argues it at
//! length. The short version is that a closure would need `Send + Sync` threaded through every
//! signature so this registry could set it from a Tauri worker while the walk polls it, it buys
//! nothing an atomic load does not, and `cide_search::content::Search::cancel` — which
//! [`super::search`] drives — is already the house shape for exactly this.
//!
//! # One job per *tab*, which is where this differs from [`SearchRegistry`]
//!
//! [`super::search::SearchRegistry`]'s header argues for one job per **project**, and it is right
//! to: there is one search box, two windows searching one project are asking one question, and a
//! job per window would let a background window keep walking a 100k-file repository for a panel
//! nobody is looking at.
//!
//! The Log surface is not shaped like that. A project has the Log tab *and* N History tabs open at
//! once, each with its own repository scope, path filter, ref selection and cursor, and all of
//! them are on screen in the same panel — switching between them is a click, not a new question.
//! Keyed by project, opening a History tab on one file would cancel the Log's walk mid-page, and
//! every user gesture in the panel would fight every other one. So the key is
//! `(ProjectId, ToolTabId)`: one live walk per tab, and tabs do not disturb each other.
//!
//! Two *windows* showing the same project's same tab still share one job, and that is the same
//! trade `SearchRegistry` makes deliberately — with the same mitigation, that an identical query
//! is *reused* rather than replaced, so two windows asking the same question run one walk and
//! neither cancels the other.
//!
//! # No generation counter
//!
//! The job's identity **is** the generation. A caller holds an `Arc<LogJob>` and `Arc::ptr_eq`
//! against the map's current entry answers "is mine still the live one" exactly; a `u64` beside
//! the map would be a second source of truth for "which query is current", and the two can
//! disagree — which is a page from the superseded walk being drawn as though it were the answer.
//!
//! # Blame cannot be cancelled, and must not pretend to be
//!
//! Do not add a `cancel` field to [`cide_ipc::history::BlameRequest`] or route `git_blame`
//! through anything here. libgit2 exposes **no hook** inside `git_blame_file` — no callback, no
//! progress structure, no interrupt — so a blame already running cannot be stopped by anything,
//! and a control that does nothing is worse than no control at all: the user presses it, the
//! spinner keeps turning, and the next thing they conclude is that cide is wedged.
//! `cide_git::blame::MAX_BLAME_BYTES` bounds the *input* instead, which is the honest version of
//! the same guarantee — the work is bounded even though it cannot be interrupted — and
//! `cmd::git::git_blame` says so at its own definition. This paragraph exists so that the next
//! reader who notices the asymmetry between log and blame finds the answer here rather than going
//! looking for a hook that libgit2 does not have.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cide_ipc::history::LogQuery;
use cide_ipc::{ProjectId, ToolTabId};
use dashmap::DashMap;
use tauri::State;

/// Every tool tab's current log walk.
///
/// Entries live only for as long as a walk is in flight: [`LogRegistry::finish`] takes the job
/// back out when its walk returns. See that method for why the alternative — leaving finished
/// jobs in the map — lost.
#[derive(Default)]
pub struct LogRegistry {
    jobs: DashMap<(ProjectId, ToolTabId), Arc<LogJob>>,
}

/// One running log walk: the question it is answering, and the flag that stops it.
///
/// Far thinner than `search::Job`, and the difference is the shape of the two surfaces rather
/// than an omission. A search is a *poll*: the walk runs unattended, appends into a buffer, and
/// the panel asks for `[offset, offset + limit)` until `running` goes false, so the job has to
/// own the results. A log page is a plain request and response — `git_log` awaits its own walk
/// and hands the page straight back — so there is nothing here for a second caller to read, and
/// a results buffer would be a copy of a value that has already been returned.
pub struct LogJob {
    /// What this job is answering. Compared, never executed from here: it is how
    /// [`LogRegistry::job_for`] tells "the same request again" from "a new one".
    query: LogQuery,
    /// Set to stop the walk; handed to `cide-git` as [`cide_git::log::LogWalk::cancel`].
    stop: AtomicBool,
}

impl LogJob {
    fn new(query: LogQuery) -> Self {
        Self {
            query,
            stop: AtomicBool::new(false),
        }
    }

    /// The flag `cide-git` polls. Borrowed for the length of one walk and never stored there.
    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.stop
    }

    fn cancel(&self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl LogRegistry {
    /// The job answering `query` on this tab, starting one if the standing job answers something
    /// else.
    ///
    /// Both details below are `SearchRegistry::job_for`'s, verbatim and for the same reasons.
    pub fn job_for(&self, project: ProjectId, tab: ToolTabId, query: &LogQuery) -> Arc<LogJob> {
        let key = (project, tab);
        // The comparison and the insert are separate statements: `DashMap::insert` while a
        // `Ref` into the same shard is alive is a self-deadlock, and letting the guard live
        // to the end of an `if let` block is how that happens. `FsRegistry::claim` has the
        // same note for the same reason.
        let standing = self
            .jobs
            .get(&key)
            .filter(|job| job.query == *query)
            .map(|job| Arc::clone(job.value()));
        if let Some(job) = standing {
            return job;
        }

        let job = Arc::new(LogJob::new(query.clone()));
        // The old job is cancelled only after the new one has replaced it, so a poll racing
        // this can find either job answering a query it echoes back — never a cancelled job
        // presented as the live one.
        if let Some(old) = self.jobs.insert(key, Arc::clone(&job)) {
            old.cancel();
        }
        job
    }

    /// Take a finished walk's job back out, **if it is still the live one**.
    ///
    /// `Arc::ptr_eq` and not a bare `remove`: a newer request may have replaced this job while
    /// the walk was running, and removing *that* entry would leave the new walk with a flag
    /// nothing can reach — a cancel that silently does nothing, which is the failure this whole
    /// module exists to avoid.
    ///
    /// Removing at all is the choice worth naming. The alternative — leave finished jobs in the
    /// map and let close and quit sweep them — is simpler and was rejected: the map would then
    /// hold one entry per tool tab *ever opened*, each carrying a cloned [`LogQuery`] whose
    /// resume token can be tens of kilobytes of frontier, and it would only ever be as bounded as
    /// the frontend's discipline about calling `git_log_cancel` when a tab closes. Removing here
    /// makes the map's size proportional to walks **in flight**, which is a property this side of
    /// the wire can keep on its own.
    ///
    /// The one thing it costs: when two windows shared a job, the first walk to finish removes
    /// it, and the second becomes uncancellable for its remaining moments. They are running the
    /// same query and will finish within moments of each other, which is why that is the cheap
    /// side of the trade.
    pub fn finish(&self, project: ProjectId, tab: ToolTabId, job: &Arc<LogJob>) {
        self.jobs
            .remove_if(&(project, tab), |_, live| Arc::ptr_eq(live, job));
    }

    /// Stop the walk one tab is running, if it has one.
    ///
    /// Idempotent, and safe on a tab that never walked: `false` simply means there was nothing to
    /// stop. A caller that had to know whether a walk was in flight before it could stop one
    /// would need the state this method exists to save it keeping — `search_cancel`'s argument,
    /// and it applies harder here, because [`Self::finish`] means "no entry" is the *usual* state
    /// between two pages.
    pub fn cancel_tab(&self, project: ProjectId, tab: ToolTabId) -> bool {
        match self.jobs.remove(&(project, tab)) {
            Some((_, job)) => {
                job.cancel();
                true
            }
            None => false,
        }
    }

    /// Stop every walk over one project. The project-close path, [`crate::cmd::fs::fs_close`].
    ///
    /// A `retain` and not a collect-then-remove: the keys are `(ProjectId, ToolTabId)` pairs, so
    /// there is no way to name a project's entries without scanning, and gathering them into a
    /// `Vec` first would mean holding an iterator over the map while removing from it — the
    /// self-deadlock `FsRegistry::indexed` documents.
    pub fn cancel_project(&self, project: ProjectId) -> usize {
        let mut stopped = 0;
        self.jobs.retain(|(id, _), job| {
            if *id != project {
                return true;
            }
            job.cancel();
            stopped += 1;
            false
        });
        stopped
    }

    /// Stop every walk. [`crate::lifecycle::shutdown`] — the quit path.
    pub fn cancel_all(&self) {
        self.jobs.retain(|_, job| {
            job.cancel();
            false
        });
    }
}

/// Stop the log walk a tool tab is running.
///
/// **Not** `async` and **not** `spawn_blocking`, which is the whole point of it — the reason
/// `cmd::diagnostics::diagnostics_usages_cancel` gives for itself, and it applies here even more
/// directly. This takes one map entry and stores one atomic. Handing it to the blocking pool
/// would queue it behind the pool's current work, and on a busy pool the item in front of it is
/// very often *the walk it was called to cancel* — a cancel that waits for the thing it is
/// cancelling to finish is not a cancel.
///
/// Called when a tab closes, when the filter box supersedes its own query, and when the panel
/// unmounts. Idempotent, and `false` for a tab with nothing running is the ordinary answer rather
/// than a problem: see [`LogRegistry::cancel_tab`].
#[tauri::command(rename_all = "camelCase")]
pub fn git_log_cancel(logs: State<'_, LogRegistry>, project: ProjectId, tab: ToolTabId) -> bool {
    logs.cancel_tab(project, tab)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::RepoId;
    use cide_ipc::history::{LogCursor, LogRefs, LogScope, Simplify};

    /// The same question every time it is called with the same arguments — which the repository
    /// id has to be pinned for. A fresh `RepoId::new()` per call would make every query differ
    /// from every other, and the reuse path below would look as though it worked while never
    /// being taken.
    fn query(path: Option<&str>) -> LogQuery {
        LogQuery {
            scope: LogScope::One { repo: fixed_repo() },
            refs: LogRefs::Head,
            cursor: LogCursor::Newest,
            path: path.map(str::to_string),
            follow: false,
            simplify: Simplify::Default,
            first_parent: false,
            author: None,
            text: None,
            limit: 100,
            scan_limit: 20_000,
            graph: true,
            graph_lanes: 12,
        }
    }

    /// One id for the whole module, minted once.
    fn fixed_repo() -> RepoId {
        static REPO: std::sync::OnceLock<RepoId> = std::sync::OnceLock::new();
        *REPO.get_or_init(RepoId::new)
    }

    #[test]
    fn asking_the_same_question_twice_does_not_restart_the_walk() {
        let registry = LogRegistry::default();
        let (project, tab) = (ProjectId::new(), ToolTabId::new());

        let first = registry.job_for(project, tab, &query(None));
        let same = registry.job_for(project, tab, &query(None));
        assert!(
            Arc::ptr_eq(&first, &same),
            "two windows on one tab asking one question must run one walk"
        );
        assert!(!first.stop.load(Ordering::Acquire));
    }

    #[test]
    fn a_new_query_replaces_the_standing_job_and_stops_it() {
        let registry = LogRegistry::default();
        let (project, tab) = (ProjectId::new(), ToolTabId::new());

        let first = registry.job_for(project, tab, &query(None));
        let second = registry.job_for(project, tab, &query(Some("src/main.rs")));
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(
            first.stop.load(Ordering::Acquire),
            "the walk nobody is waiting for has to be told to stop"
        );
        assert!(!second.stop.load(Ordering::Acquire));
    }

    /// The one thing this registry does that `SearchRegistry` deliberately does not.
    ///
    /// Keyed by project, opening a History tab would cancel the Log tab's walk — both are on
    /// screen in the same panel at the same time, so that is not a trade, it is a bug.
    #[test]
    fn a_second_tab_does_not_cancel_the_first() {
        let registry = LogRegistry::default();
        let project = ProjectId::new();
        let (log_tab, history_tab) = (ToolTabId::new(), ToolTabId::new());

        let log = registry.job_for(project, log_tab, &query(None));
        let history = registry.job_for(project, history_tab, &query(Some("src/main.rs")));

        assert!(!Arc::ptr_eq(&log, &history));
        assert!(
            !log.stop.load(Ordering::Acquire),
            "the Log tab's page is still on screen; nothing asked for it to stop"
        );
    }

    #[test]
    fn finishing_takes_the_job_out_but_only_if_it_is_still_the_live_one() {
        let registry = LogRegistry::default();
        let (project, tab) = (ProjectId::new(), ToolTabId::new());

        let first = registry.job_for(project, tab, &query(None));
        let second = registry.job_for(project, tab, &query(Some("a.rs")));

        registry.finish(project, tab, &first);
        assert!(
            registry.jobs.contains_key(&(project, tab)),
            "the superseded walk finishing must not take the live job's flag with it — the new \
             walk would then be uncancellable"
        );

        registry.finish(project, tab, &second);
        assert!(registry.jobs.is_empty(), "and the live one does come out");
    }

    #[test]
    fn cancelling_a_tab_stops_its_walk_and_is_idempotent() {
        let registry = LogRegistry::default();
        let (project, tab) = (ProjectId::new(), ToolTabId::new());
        let job = registry.job_for(project, tab, &query(None));

        assert!(registry.cancel_tab(project, tab));
        assert!(job.stop.load(Ordering::Acquire));
        assert!(registry.jobs.is_empty());
        assert!(
            !registry.cancel_tab(project, tab),
            "a tab closing does not have to know whether it had a walk running"
        );
    }

    /// Closing a project stops every tab's walk, not just the active one.
    #[test]
    fn cancelling_a_project_stops_every_tab_and_leaves_other_projects_alone() {
        let registry = LogRegistry::default();
        let (mine, other) = (ProjectId::new(), ProjectId::new());
        let tabs: Vec<Arc<LogJob>> = (0..3)
            .map(|_| registry.job_for(mine, ToolTabId::new(), &query(None)))
            .collect();
        let theirs = registry.job_for(other, ToolTabId::new(), &query(None));

        assert_eq!(registry.cancel_project(mine), 3);
        assert!(tabs.iter().all(|job| job.stop.load(Ordering::Acquire)));
        assert!(
            !theirs.stop.load(Ordering::Acquire),
            "closing one project must not stop another project's log"
        );
        assert_eq!(registry.jobs.len(), 1);
    }

    /// The quit path: every project's walk, not just the focused one's.
    #[test]
    fn cancel_all_stops_every_project() {
        let registry = LogRegistry::default();
        let jobs: Vec<Arc<LogJob>> = (0..3)
            .map(|_| registry.job_for(ProjectId::new(), ToolTabId::new(), &query(None)))
            .collect();

        registry.cancel_all();
        assert!(registry.jobs.is_empty());
        assert!(jobs.iter().all(|job| job.stop.load(Ordering::Acquire)));
    }

    /// The flag the walk polls is the flag the registry sets — the one seam that cannot be
    /// checked by reading either side alone.
    #[test]
    fn the_borrowed_flag_is_the_one_cancel_sets() {
        let registry = LogRegistry::default();
        let (project, tab) = (ProjectId::new(), ToolTabId::new());
        let job = registry.job_for(project, tab, &query(None));

        // Exactly what `git_log` hands to `cide_git::log::log_cancellable`.
        let borrowed = job.cancel_flag();
        assert!(!borrowed.load(Ordering::Acquire));
        registry.cancel_tab(project, tab);
        assert!(borrowed.load(Ordering::Acquire));
    }
}
