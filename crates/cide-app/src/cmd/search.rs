//! Content search: the sidebar's ⌕ panel, over a project's roots. (M11)
//!
//! # Why this is a poll and not a subscription
//!
//! The same reason `cmd::picker` is. A search over a large repository produces results for
//! seconds, and pushing an event per batch would put an unbounded number of them on the IPC
//! channel while the user is still typing. Instead the walk runs on a blocking worker and
//! appends to a buffer, and the panel asks for `[offset, offset + limit)` and appends what it
//! gets. [`cide_ipc::SearchFrame::running`] is what tells it whether to ask again.
//!
//! The offset is what keeps that cheap: a poll during a walk that has already found 4 000
//! hits carries the handful found since the last poll, not 4 000 rows a second time.
//!
//! # One search per project
//!
//! Keyed on the project, like the picker's matcher, and with the same consequence: two
//! windows searching the same project share one job, and the second query cancels the first.
//! That is the honest trade for a per-project resource, it is what the picker already does,
//! and the alternative — a job per window — would let a background window keep a walk of a
//! 100k-file repository running for a panel nobody is looking at.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use cide_fs::{Filter, FsError};
use cide_ipc::{ProjectId, SearchFrame, SearchHit, SearchQuery};
use cide_search::content::{Limits, PatternError, Regex, Search, SearchRoot};
use dashmap::DashMap;
use parking_lot::Mutex;
use tauri::State;

use crate::files::FsRegistry;

/// Hits one poll may carry. The panel draws about thirty rows and prefetches past them.
const DEFAULT_LIMIT: u32 = 200;

/// The most any caller can ask for, whatever it says. A frontend bug asking for a million
/// must cost a clamped answer rather than a serialised megabyte — the same rule as
/// `cmd::fs`'s `MAX_ROWS`.
const MAX_LIMIT: u32 = 1_000;

/// Every project's current search.
#[derive(Default)]
pub struct SearchRegistry {
    jobs: DashMap<ProjectId, Arc<Job>>,
}

impl SearchRegistry {
    /// The job answering `query`, starting one if the standing job answers something else.
    fn job_for(&self, project: ProjectId, query: &SearchQuery) -> Arc<Job> {
        // The comparison and the insert are separate statements: `DashMap::insert` while a
        // `Ref` into the same shard is alive is a self-deadlock, and letting the guard live
        // to the end of an `if let` block is how that happens. `FsRegistry::claim` has the
        // same note for the same reason.
        let standing = self
            .jobs
            .get(&project)
            .filter(|job| job.query == *query)
            .map(|job| Arc::clone(job.value()));
        if let Some(job) = standing {
            return job;
        }

        let job = Arc::new(Job::new(query.clone()));
        // The old job is cancelled only after the new one has replaced it, so a poll racing
        // this can find either job answering a query it echoes back — never a cancelled job
        // presented as the live one.
        if let Some(old) = self.jobs.insert(project, Arc::clone(&job)) {
            old.cancel();
        }
        job
    }

    /// Drop the search a project is running, if it has one.
    pub fn cancel(&self, project: ProjectId) -> bool {
        match self.jobs.remove(&project) {
            Some((_, job)) => {
                job.cancel();
                true
            }
            None => false,
        }
    }

    /// Drop every search. [`crate::lifecycle::shutdown`] — the quit path — and
    /// [`crate::cmd::fs::close_all`], which currently has no caller of its own.
    pub fn cancel_all(&self) {
        self.jobs.retain(|_, job| {
            job.cancel();
            false
        });
    }
}

/// One running or finished search, and everything it has found.
struct Job {
    query: SearchQuery,
    /// Set to stop the walk; see [`cide_search::content::Search::cancel`].
    stop: AtomicBool,
    /// Whether the walk has been *dispatched*. Distinct from [`Self::running`], which is
    /// false both before the walk starts and after it ends — polling a finished search would
    /// otherwise start the whole walk again on every poll.
    started: AtomicBool,
    running: AtomicBool,
    /// Live, so a poll can show progress over a repository whose size is not yet known.
    scanned: AtomicU32,
    found: Mutex<Found>,
}

/// The results so far. One lock, held for an append or a page read and never across IO.
#[derive(Default)]
struct Found {
    hits: Vec<SearchHit>,
    files: u32,
    truncated: bool,
    /// An unusable pattern. Set before the walk is even attempted; see
    /// [`cide_ipc::SearchFrame::error`] for why it is not a rejection.
    error: Option<String>,
}

impl Job {
    fn new(query: SearchQuery) -> Self {
        Self {
            query,
            stop: AtomicBool::new(false),
            started: AtomicBool::new(false),
            running: AtomicBool::new(false),
            scanned: AtomicU32::new(0),
            found: Mutex::new(Found::default()),
        }
    }

    fn cancel(&self) {
        self.stop.store(true, Ordering::Release);
    }

    /// Claim the right to start this job's walk. True for exactly one caller.
    ///
    /// A swap rather than a load and a store: two windows polling the same new query at the
    /// same moment would both see "not started" and run two walks of the same tree, both
    /// appending into one buffer — every hit twice.
    ///
    /// `running` goes up here and not at the point the walk is handed to `spawn_blocking`.
    /// Between the two the caller reads the whole directory list out of the index, which is
    /// milliseconds on a large repository — and a second window polling inside that gap would
    /// see a job that is started, not running and holding no hits, which is indistinguishable
    /// from a finished search that found nothing. It would paint `No results` and stop
    /// polling. The caller puts it back down on the paths that dispatch no walk.
    fn claim(&self) -> bool {
        let first = !self.started.swap(true, Ordering::AcqRel);
        if first {
            self.running.store(true, Ordering::Release);
        }
        first
    }

    /// The blocking half: walk the roots under the project's own ignore rules.
    ///
    /// **Blocking**, for as long as the tree takes. `spawn_blocking` is the only caller.
    fn walk(&self, roots: Vec<SearchRoot>, filter: Arc<Filter>, regex: Regex) {
        // The very object the file tree and the watcher consult — `ProjectFs::filter` — and
        // not a second one built from the same `dir_paths()`. Rebuilding it here cost a `stat`
        // per directory in the project, a few thousand on a large one, at the start of every
        // single search; worse, it was a second place where the ignore decision got made, and
        // the two could only agree for as long as nobody edited one of them.
        //
        // Belt and braces on top of `content`'s own `WalkBuilder`, and worth being precise
        // about: today it excludes nothing the walker has not already excluded, because both
        // are driven by the same `.gitignore` files and the same `hidden`/`git_*` settings — a
        // review replaced this with `admits: None` and every test still passed. What it buys
        // is that the search follows the *tree* rather than a copy of the tree's rules: the
        // day `Filter` grows a rule the walker has no setting for, the search inherits it
        // instead of quietly disagreeing about which files exist.
        let admits = |path: &Path, is_dir: bool| filter.admits(path, is_dir);

        let outcome = Search {
            roots: &roots,
            regex: &regex,
            limits: Limits::default(),
            admits: Some(&admits),
            cancel: &self.stop,
            progress: Some(&self.scanned),
        }
        .run(&mut |batch| {
            let mut found = self.found.lock();
            found.hits.extend_from_slice(batch);
            // One batch is one file's hits, by construction in `content::Visitor`. Counting
            // here rather than deduplicating paths afterwards is what keeps the figure live
            // while the walk runs.
            found.files += 1;
        });

        self.found.lock().truncated = outcome.truncated;
        // Last, and after the results are visible: a panel that saw `running: false` with the
        // final batch not yet appended would stop polling one page short.
        self.running.store(false, Ordering::Release);
    }

    /// One page, plus the counts the panel's header shows.
    fn frame(&self, offset: u32, limit: u32) -> SearchFrame {
        let found = self.found.lock();
        let total = found.hits.len() as u32;
        let start = offset.min(total) as usize;
        let end = (start + limit as usize).min(found.hits.len());
        SearchFrame {
            query: self.query.clone(),
            hits: found.hits[start..end].to_vec(),
            offset: start as u32,
            total,
            files: found.files,
            scanned: self.scanned.load(Ordering::Acquire),
            running: self.running.load(Ordering::Acquire),
            truncated: found.truncated,
            error: found.error.clone(),
        }
    }
}

/// Search a project's contents, or ask again for more of a search already running.
///
/// The first call with a given query starts the walk and returns an empty frame with
/// `running: true`; every later call with the same query returns the page at `offset`. A
/// different query — including the same pattern with a different toggle — cancels the
/// standing search and starts a new one.
///
/// `async` over the blocking pool like every handler that touches a disk. The handler itself
/// does no IO: it resolves state, compiles the pattern (microseconds) and hands a walk that
/// can run for seconds to [`tauri::async_runtime::spawn_blocking`] *without awaiting it*.
/// Awaiting would make the first keystroke wait for the whole repository, which is the freeze
/// this design exists to avoid.
#[tauri::command(rename_all = "camelCase")]
pub async fn search_query(
    fs: State<'_, FsRegistry>,
    searches: State<'_, SearchRegistry>,
    project: ProjectId,
    query: SearchQuery,
    offset: Option<u32>,
    limit: Option<u32>,
) -> Result<SearchFrame, FsError> {
    // The roots and the ignore rules both come from the file index. A search over a project
    // cide has not walked would have to invent both, so it says so instead — the same
    // `NoIndex` the picker answers with, which the frontend already knows how to read.
    let project_fs = fs.get(project).ok_or(FsError::NoIndex)?;

    let job = searches.job_for(project, &query);
    let limit = limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let offset = offset.unwrap_or(0);

    // `claim` also marks the job running; see its note. The two paths below that dispatch no
    // walk have to put that back down.
    if job.claim() {
        if !cide_search::content::is_searchable(&job.query) {
            // Nothing to search for. Not an error and not a walk — the panel's empty state.
            job.running.store(false, Ordering::Release);
        } else {
            match cide_search::content::compile(&job.query) {
                Ok(regex) => {
                    let roots: Vec<SearchRoot> = project_fs
                        .roots
                        .iter()
                        .map(|r| SearchRoot {
                            path: r.path.clone(),
                            label: r.label.clone(),
                        })
                        .collect();
                    // Taken here rather than inside the worker: it is one `RwLock` read and an
                    // `Arc` bump, and the worker outlives this call — it must not hold a
                    // reference to a project that can be closed while the walk runs. The
                    // `Arc` is what lets the walk keep using the rules of a project that
                    // closes under it, which is also exactly why closing cancels it.
                    let filter = project_fs.filter();

                    let job = Arc::clone(&job);
                    // Not awaited. The frame below is the empty first one, and the panel
                    // polls for the rest.
                    tauri::async_runtime::spawn_blocking(move || job.walk(roots, filter, regex));
                }
                Err(error) => {
                    // Half of every regex is unparsable while it is being typed. The frame
                    // carries it and the panel shows it in place of the results.
                    job.found.lock().error = Some(pattern_error(&error));
                    job.running.store(false, Ordering::Release);
                }
            }
        }
    }

    Ok(job.frame(offset, limit))
}

/// Stop the search a project is running and forget its results.
///
/// Called when the panel closes or its input is emptied. Idempotent, and safe on a project
/// that never searched: a panel that had to know whether it has a search running before it
/// could stop one would need state this command exists to save it keeping.
#[tauri::command(rename_all = "camelCase")]
pub async fn search_cancel(
    searches: State<'_, SearchRegistry>,
    project: ProjectId,
) -> Result<bool, FsError> {
    Ok(searches.cancel(project))
}

/// A regex parse failure, on one line.
///
/// `regex`'s `Display` is a multi-line diagram with a caret under the offending byte. It is
/// excellent in a terminal and unreadable in a 252px panel, so only its first line survives.
fn pattern_error(error: &PatternError) -> String {
    error
        .to_string()
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("the pattern could not be parsed")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::SearchMode;

    fn query(pattern: &str) -> SearchQuery {
        SearchQuery {
            pattern: pattern.to_string(),
            mode: SearchMode::Literal,
            case_sensitive: false,
            whole_word: false,
        }
    }

    /// A job with `n` hits already found, as a walk would have left it.
    fn job_with(n: u32) -> Job {
        let job = Job::new(query("needle"));
        let mut found = job.found.lock();
        for i in 0..n {
            found.hits.push(SearchHit {
                path: format!("/root/f{i}.rs"),
                rel: format!("f{i}.rs"),
                line: i + 1,
                text: "needle".to_string(),
                start: 0,
                end: 6,
            });
        }
        found.files = n;
        drop(found);
        job
    }

    #[test]
    fn a_page_is_the_window_the_caller_asked_for() {
        let job = job_with(10);
        let frame = job.frame(4, 3);
        assert_eq!(frame.offset, 4);
        assert_eq!(frame.hits.len(), 3);
        assert_eq!(frame.hits[0].rel, "f4.rs");
        assert_eq!(frame.total, 10, "the total is the whole list, not the page");
    }

    /// The panel polls from an advancing offset, and the walk may have found nothing new.
    #[test]
    fn a_page_past_the_end_is_empty_rather_than_an_error() {
        let job = job_with(3);
        let frame = job.frame(99, 10);
        assert!(frame.hits.is_empty());
        assert_eq!(frame.offset, 3, "clamped, so the panel cannot skip a hit");
        assert_eq!(frame.total, 3);
    }

    #[test]
    fn a_new_query_replaces_the_standing_job_and_stops_it() {
        let registry = SearchRegistry::default();
        let project = ProjectId::new();

        let first = registry.job_for(project, &query("alpha"));
        let same = registry.job_for(project, &query("alpha"));
        assert!(
            Arc::ptr_eq(&first, &same),
            "polling the same query must not restart the search"
        );

        let second = registry.job_for(project, &query("beta"));
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(
            first.stop.load(Ordering::Acquire),
            "the walk nobody is waiting for has to be told to stop"
        );
    }

    /// The same pattern under a different toggle is a different search.
    #[test]
    fn a_toggle_is_part_of_the_query_identity() {
        let registry = SearchRegistry::default();
        let project = ProjectId::new();

        let insensitive = registry.job_for(project, &query("alpha"));
        let mut sensitive = query("alpha");
        sensitive.case_sensitive = true;
        let after = registry.job_for(project, &sensitive);
        assert!(!Arc::ptr_eq(&insensitive, &after));
    }

    /// A claimed job is *running* from that instant, not from the moment its walk is handed
    /// to `spawn_blocking`.
    ///
    /// The gap between the two is the handler reading every directory out of the index. A
    /// second window polling inside it used to get `started, not running, no hits`, which the
    /// panel reads as a finished search with no results — it paints `No results` and stops
    /// polling, for a search that is about to produce thousands.
    #[test]
    fn a_claimed_job_is_running_before_its_walk_is_dispatched() {
        let job = Job::new(query("alpha"));
        assert!(!job.frame(0, 10).running, "not until somebody claims it");
        assert!(job.claim());
        assert!(
            job.frame(0, 10).running,
            "a poll racing the dispatch must not see a search that looks finished"
        );
    }

    /// The walk is dispatched once, however many windows poll it.
    #[test]
    fn a_finished_search_is_not_walked_again_by_the_next_poll() {
        let job = Job::new(query("alpha"));
        assert!(job.claim(), "the first caller starts the walk");
        assert!(
            !job.claim(),
            "a concurrent poll does not start a second one"
        );
        job.running.store(false, Ordering::Release);
        assert!(
            !job.claim(),
            "and neither does a poll after the walk has finished"
        );
    }

    #[test]
    fn cancelling_a_project_stops_its_walk_and_forgets_its_hits() {
        let registry = SearchRegistry::default();
        let project = ProjectId::new();
        let job = registry.job_for(project, &query("alpha"));

        assert!(registry.cancel(project));
        assert!(registry.jobs.is_empty(), "the hits go with the job");
        assert!(job.stop.load(Ordering::Acquire));
        assert!(!registry.cancel(project), "and it is idempotent");
    }

    /// Closing a project stops its search there and then.
    ///
    /// The seam is `cmd::fs`'s, but the observable is this module's — and the version this
    /// replaced left the walk running until the *next* query against any project happened to
    /// sweep it, which for a user who closes a project and stops searching is never.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn closing_a_project_cancels_the_search_over_it() {
        let dir = cide_fs::testing::scratch("cmd-search-close");
        let fs = FsRegistry::default();
        let project = ProjectId::new();
        // An entry in the registry is what "open" means here; the walk itself is beside the
        // point, so the claim is dropped un-run rather than walked.
        drop(
            fs.claim(project, vec![dir.to_path_buf()])
                .expect("a project nobody has indexed claims"),
        );

        let searches = SearchRegistry::default();
        let job = searches.job_for(project, &query("alpha"));
        assert!(!job.stop.load(Ordering::Acquire), "it starts running");

        let existed = crate::cmd::fs::close_project(&fs, &searches, project)
            .await
            .expect("closing a project is not an error");
        assert!(existed, "the project was open");
        assert!(
            job.stop.load(Ordering::Acquire),
            "closing left the walk running — a walker thread per core reading a tree nobody \
             has open, holding its hits, until some later query swept it"
        );
        assert!(searches.jobs.is_empty(), "and the hits go with it");
    }

    /// The quit path: every project's walk, not just the focused one's.
    #[test]
    fn cancel_all_stops_every_project() {
        let registry = SearchRegistry::default();
        let jobs: Vec<Arc<Job>> = (0..3)
            .map(|_| registry.job_for(ProjectId::new(), &query("alpha")))
            .collect();

        registry.cancel_all();
        assert!(registry.jobs.is_empty());
        assert!(jobs.iter().all(|job| job.stop.load(Ordering::Acquire)));
    }

    /// Two windows polling one new query at the same instant must dispatch one walk.
    ///
    /// The single-threaded test above cannot see this: `claim` written as a load and a store
    /// passes it, because nothing there ever interleaves the two. Here every thread is parked
    /// on the same barrier and released together, so a load-then-store loses the race within a
    /// few hundred rounds — and each loser would have run a second walk of the same tree,
    /// appending into one buffer, showing the user every hit twice.
    #[test]
    fn exactly_one_of_many_racing_pollers_claims_the_walk() {
        use std::sync::Barrier;
        use std::sync::atomic::AtomicU32;

        const THREADS: usize = 8;
        const ROUNDS: usize = 500;

        for round in 0..ROUNDS {
            let job = Arc::new(Job::new(query("alpha")));
            let barrier = Arc::new(Barrier::new(THREADS));
            let winners = Arc::new(AtomicU32::new(0));

            let threads: Vec<_> = (0..THREADS)
                .map(|_| {
                    let job = Arc::clone(&job);
                    let barrier = Arc::clone(&barrier);
                    let winners = Arc::clone(&winners);
                    std::thread::spawn(move || {
                        barrier.wait();
                        if job.claim() {
                            winners.fetch_add(1, Ordering::Relaxed);
                        }
                    })
                })
                .collect();
            for thread in threads {
                thread.join().expect("a claiming thread");
            }

            assert_eq!(
                winners.load(Ordering::Acquire),
                1,
                "round {round}: {THREADS} pollers raced and {} started a walk; every extra one \
                 walks the whole tree again into the same buffer",
                winners.load(Ordering::Acquire)
            );
            assert!(
                job.frame(0, 1).running,
                "round {round}: the winner must leave the job running, or a losing poller \
                 reads `started, not running, no hits` as a finished search"
            );
        }
    }

    /// The glue this module exists for, end to end minus Tauri's argument extraction: a real
    /// tree, the real ignore rules, and the pages the panel would read.
    ///
    /// `Job::walk` is what `search_query` hands to `spawn_blocking`, so driving it directly
    /// covers everything except `State`, which cannot be built outside a Tauri app —
    /// `cmd::fs`'s tests are split the same way and for the same reason.
    #[test]
    fn a_walk_fills_the_job_and_the_panel_reads_it_a_page_at_a_time() {
        let dir = cide_fs::testing::scratch("cmd-search");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join(".gitignore"), "target/\n").unwrap();
        for f in 0..4 {
            std::fs::write(dir.join(format!("src/f{f}.rs")), "// zqneedle\n").unwrap();
        }
        // Must not be found: the shared filter refuses it, exactly as the file tree does.
        std::fs::write(dir.join("target/out.log"), "zqneedle\n").unwrap();

        let job = Job::new(query("zqneedle"));
        assert!(job.claim(), "which is also what marks it running");
        let roots = vec![SearchRoot::new(dir.path())];
        let dirs = [dir.to_path_buf(), dir.join("src")];
        // Built the way `ProjectFs` builds it, which is what `ProjectFs::filter` now hands the
        // walk — the accessor itself needs an indexed project and is covered in `cmd::fs`.
        let filter = Arc::new(Filter::build(
            &[dir.to_path_buf()],
            dirs.iter().map(|p| p.as_path()),
        ));
        job.walk(
            roots,
            Arc::clone(&filter),
            cide_search::content::compile(&job.query).unwrap(),
        );

        let first = job.frame(0, 3);
        assert_eq!(first.hits.len(), 3);
        assert_eq!(first.total, 4, "four files, one hit each");
        assert_eq!(first.files, 4);
        assert!(!first.running, "the walk sets this down as it finishes");
        assert!(first.error.is_none());
        assert_eq!(first.scanned, 4, "the gitignored file was never opened");

        let second = job.frame(3, 3);
        assert_eq!(
            second.hits.len(),
            1,
            "the panel appends from where it left off"
        );
        assert_eq!(second.offset, 3);

        let every: Vec<String> = job.frame(0, 100).hits.into_iter().map(|h| h.rel).collect();
        assert!(
            every.iter().all(|rel| rel.starts_with("src/")),
            "a hit inside target/ is a hit the tree cannot open: {every:?}"
        );
        // Which of the two agreeing gates did that: the walker's own `.gitignore` handling.
        // Naming it here rather than crediting the shared `Filter` — `admits: None` passes
        // this test, and the file-level filter call has its own test in
        // `cide-search/tests/content_walk.rs`, where a closure can be made to disagree.
        assert!(
            !filter.admits(&dir.join("target/out.log"), false),
            "and the shared filter agrees with it, which is the invariant that has to hold"
        );
    }

    #[test]
    fn a_parse_failure_is_one_line() {
        let mut bad = query("fn (");
        bad.mode = SearchMode::Regex;
        let error =
            cide_search::content::compile(&bad).expect_err("an unbalanced group must not compile");
        let message = pattern_error(&error);
        assert!(!message.contains('\n'), "{message:?}");
        assert!(!message.is_empty());
    }
}
