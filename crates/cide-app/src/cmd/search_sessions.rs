//! Runtime-only Search Everywhere sessions. Pinning transfers ownership from a window's
//! modal to the project; no result set enters workspace.json or a workspace broadcast.
use super::*;
use crate::files::ProjectFs;
use cide_ipc::{
    PickerFrame, SearchDescriptor, SearchSession, SearchSessionFrame, SearchSessionId,
    SearchSource, SymbolFrame,
};

pub(super) struct Session {
    id: SearchSessionId,
    project: ProjectId,
    owner: String,
    descriptor: SearchDescriptor,
    pinned: AtomicBool,
    run: Mutex<(u32, Arc<Run>)>,
}

struct Run {
    budget: Mutex<Option<std::sync::Weak<cide_fs::budget::Budget>>>,
    incomplete: AtomicBool,
    stop: AtomicBool,
    file_running: AtomicBool,
    symbol_running: AtomicBool,
    files: Mutex<Option<PickerFrame>>,
    symbols: Mutex<Option<SymbolFrame>>,
    text: Arc<Job>,
    errors: Mutex<Vec<String>>,
}

impl Run {
    fn new(descriptor: &SearchDescriptor) -> Self {
        Self {
            budget: Mutex::new(None),
            incomplete: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            file_running: AtomicBool::new(matches!(
                descriptor.source,
                SearchSource::All | SearchSource::Files
            )),
            symbol_running: AtomicBool::new(matches!(
                descriptor.source,
                SearchSource::All | SearchSource::Symbols
            )),
            files: Mutex::new(None),
            symbols: Mutex::new(None),
            text: Arc::new(Job::new(descriptor.query.clone())),
            errors: Mutex::new(Vec::new()),
        }
    }
    fn cancel(&self) {
        self.stop.store(true, Ordering::Release);
        self.text.cancel();
    }
}

impl Session {
    fn summary(&self) -> SearchSession {
        SearchSession {
            id: self.id,
            project: self.project,
            descriptor: self.descriptor.clone(),
            generation: self.run.lock().0,
            pinned: self.pinned.load(Ordering::Acquire),
        }
    }
    pub(super) fn cancel(&self) {
        self.run.lock().1.cancel();
    }

    fn frame(&self, offset: u32) -> SearchSessionFrame {
        // Capture generation and run together: Refresh swaps both, and a page must never
        // label the old hits with the new generation.
        let (generation, run) = self.run.lock().clone();
        let mut summary = self.summary();
        summary.generation = generation;
        if run
            .budget
            .lock()
            .as_ref()
            .is_some_and(|weak| weak.upgrade().is_none_or(|budget| budget.cancelled()))
        {
            return SearchSessionFrame {
                session: summary,
                incomplete: true,
                files: None,
                symbols: None,
                text: None,
                running: false,
                errors: vec!["Index changed. Refresh this search.".into()],
            };
        }
        let text = matches!(
            self.descriptor.source,
            SearchSource::All | SearchSource::Text
        )
        .then(|| run.text.frame(offset, DEFAULT_LIMIT));
        let running = run.file_running.load(Ordering::Acquire)
            || run.symbol_running.load(Ordering::Acquire)
            || text.as_ref().is_some_and(|frame| frame.running);
        SearchSessionFrame {
            incomplete: run.incomplete.load(Ordering::Acquire),
            session: summary,
            files: run.files.lock().clone(),
            symbols: run.symbols.lock().clone(),
            text,
            running,
            errors: run.errors.lock().clone(),
        }
    }
}

impl SearchRegistry {
    fn session(&self, project: ProjectId, id: SearchSessionId) -> Result<Arc<Session>, String> {
        self.sessions
            .lock()
            .iter()
            .find(|session| session.id == id && session.project == project)
            .cloned()
            .ok_or_else(|| "This search is closed.".to_string())
    }

    fn create_session(
        &self,
        project: ProjectId,
        owner: String,
        descriptor: SearchDescriptor,
    ) -> Arc<Session> {
        let run = Arc::new(Run::new(&descriptor));
        let session = Arc::new(Session {
            id: SearchSessionId::new(),
            project,
            owner,
            descriptor,
            pinned: AtomicBool::new(false),
            run: Mutex::new((0, run)),
        });
        // One unpinned search per modal/window. A delayed Start request can otherwise leave
        // an unreachable job behind after the user has already typed another query.
        let mut sessions = self.sessions.lock();
        sessions.retain(|old| {
            let keep = old.owner != session.owner || old.pinned.load(Ordering::Acquire);
            if !keep {
                old.cancel();
            }
            keep
        });
        sessions.push(Arc::clone(&session));
        session
    }

    pub(super) fn cancel_project_sessions(&self, project: ProjectId) {
        self.sessions.lock().retain(|session| {
            if session.project != project {
                return true;
            }
            session.cancel();
            false
        });
    }

    /// Closing a window releases only its modal. Pinned searches belong to the project.
    pub fn cancel_search_window(&self, owner: &str) {
        self.sessions.lock().retain(|session| {
            let keep = session.owner != owner || session.pinned.load(Ordering::Acquire);
            if !keep {
                session.cancel();
            }
            keep
        });
    }

    fn close_session(&self, project: ProjectId, id: SearchSessionId, unpinned_only: bool) {
        self.sessions.lock().retain(|session| {
            if session.project != project
                || session.id != id
                || (unpinned_only && session.pinned.load(Ordering::Acquire))
            {
                return true;
            }
            session.cancel();
            false
        });
    }

    fn pin_session(
        &self,
        project: ProjectId,
        id: SearchSessionId,
    ) -> Result<SearchSession, String> {
        // Transfer ownership under the registry lock so a concurrent modal cancellation
        // cannot remove the session between lookup and pinning.
        let sessions = self.sessions.lock();
        let session = sessions
            .iter()
            .find(|s| s.project == project && s.id == id)
            .ok_or("This search is closed.")?;
        if session.descriptor.query.pattern.trim().is_empty() {
            return Err("Type a query before pinning it.".into());
        }
        session.pinned.store(true, Ordering::Release);
        Ok(session.summary())
    }

    fn restart_session(
        &self,
        project: ProjectId,
        id: SearchSessionId,
        fs: Arc<ProjectFs>,
    ) -> Result<SearchSession, String> {
        // Serialize replacement with close and other refreshes. Launch the exact run that
        // was created here, even if another refresh replaces it before workers are scheduled.
        let sessions = self.sessions.lock();
        let session = sessions
            .iter()
            .find(|s| s.project == project && s.id == id)
            .ok_or("This search is closed.")?;
        let run = Arc::new(Run::new(&session.descriptor));
        {
            let mut state = session.run.lock();
            state.1.cancel();
            *state = (state.0.wrapping_add(1), Arc::clone(&run));
        }
        launch(&session.descriptor, run, fs);
        Ok(session.summary())
    }
}

fn launch(descriptor: &SearchDescriptor, run: Arc<Run>, fs: Arc<ProjectFs>) {
    *run.budget.lock() = Some(Arc::downgrade(&fs.budget));
    run.incomplete.fetch_or(fs.incomplete(), Ordering::AcqRel);
    if run.stop.load(Ordering::Acquire) || fs.budget.cancelled() {
        return;
    }
    if run.file_running.load(Ordering::Acquire) {
        let run = Arc::clone(&run);
        let fs = Arc::clone(&fs);
        let query = descriptor.query.pattern.clone();
        let libraries = descriptor.libraries;
        tauri::async_runtime::spawn_blocking(move || {
            if libraries {
                fs.index_libraries();
            }
            loop {
                if run.stop.load(Ordering::Acquire) || fs.budget.cancelled() {
                    break;
                }
                // Read progress first: a walk finishing between progress and snapshot is
                // harmless; the opposite order could freeze a partial snapshot forever.
                let indexing = fs.is_indexing() || (libraries && fs.is_indexing_libraries());
                let mut candidates = fs.matcher().candidates();
                if libraries {
                    candidates.extend(fs.library_matcher().candidates());
                }
                let mut frame = match cide_search::rank_files(
                    &query,
                    &candidates,
                    &fs.roots
                        .iter()
                        .map(|root| root.path.clone())
                        .collect::<Vec<_>>(),
                    cide_search::MAX_FRAME,
                ) {
                    Ok(frame) => frame,
                    Err(error) => {
                        run.errors.lock().push(error);
                        break;
                    }
                };
                frame.running = indexing;
                frame.incomplete = fs.incomplete();
                run.incomplete.fetch_or(fs.incomplete(), Ordering::AcqRel);
                if run.stop.load(Ordering::Acquire) || fs.budget.cancelled() {
                    break;
                }
                *run.files.lock() = Some(frame);
                if !indexing {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(120));
            }
            run.file_running.store(false, Ordering::Release);
        });
    }
    if run.symbol_running.load(Ordering::Acquire) {
        let run = Arc::clone(&run);
        let fs = Arc::clone(&fs);
        let query = descriptor.query.pattern.clone();
        tauri::async_runtime::spawn_blocking(move || {
            fs.index_symbols();
            run.incomplete.fetch_or(fs.incomplete(), Ordering::AcqRel);
            loop {
                if run.stop.load(Ordering::Acquire) || fs.budget.cancelled() {
                    break;
                }
                let indexing = fs.is_symbol_indexing();
                let candidates = fs.symbol_matcher().candidates();
                let raw = cide_search::rank(&query, &candidates, cide_search::MAX_FRAME);
                let store = fs.symbol_store().read();
                let items = raw
                    .items
                    .into_iter()
                    .filter_map(|row| store.resolve(&row.value, &row.text, row.indices))
                    .collect();
                let frame = SymbolFrame {
                    items,
                    matched: raw.matched,
                    total: raw.total,
                    running: indexing,
                    query: raw.query,
                    truncated: store.truncated || fs.incomplete(),
                };
                drop(store);
                if run.stop.load(Ordering::Acquire) || fs.budget.cancelled() {
                    break;
                }
                *run.symbols.lock() = Some(frame);
                if !indexing {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(120));
            }
            run.symbol_running.store(false, Ordering::Release);
        });
    }
    if matches!(descriptor.source, SearchSource::All | SearchSource::Text) {
        run.text.claim();
        let job = Arc::clone(&run.text);
        tauri::async_runtime::spawn_blocking(move || {
            if !cide_search::content::is_searchable(&job.query) {
                job.running.store(false, Ordering::Release);
                return;
            }
            let roots = fs
                .roots
                .iter()
                .map(|r| SearchRoot {
                    path: r.path.clone(),
                    label: r.label.clone(),
                })
                .collect::<Vec<_>>();
            match plan(&job.query, &roots) {
                Ok(plan) => job.walk(
                    roots,
                    fs.filter(),
                    plan.regex,
                    plan.scope,
                    plan.include,
                    Some(&fs),
                ),
                Err(problem) => {
                    job.found.lock().error = Some(problem);
                    job.running.store(false, Ordering::Release);
                }
            }
        });
    }
}

#[tauri::command(rename_all = "camelCase")]
pub async fn search_session_start(
    window: tauri::WebviewWindow,
    fs: State<'_, FsRegistry>,
    searches: State<'_, SearchRegistry>,
    project: ProjectId,
    descriptor: SearchDescriptor,
) -> Result<SearchSession, String> {
    let project_fs = fs.get(project).ok_or("Project files are still opening.")?;
    let session = searches.create_session(project, window.label().to_string(), descriptor);
    // Project closure can race the lookup above. If it already swept this registry before
    // create_session acquired the lock, do not leave an unreachable new job behind.
    if !fs
        .get(project)
        .is_some_and(|current| Arc::ptr_eq(&current, &project_fs))
    {
        searches.close_session(project, session.id, false);
        return Err("The project was closed.".into());
    }
    launch(
        &session.descriptor,
        Arc::clone(&session.run.lock().1),
        project_fs,
    );
    Ok(session.summary())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn search_session_page(
    searches: State<'_, SearchRegistry>,
    project: ProjectId,
    session: SearchSessionId,
    offset: Option<u32>,
) -> Result<SearchSessionFrame, String> {
    Ok(searches
        .session(project, session)?
        .frame(offset.unwrap_or(0)))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn search_session_pin(
    fs: State<'_, FsRegistry>,
    searches: State<'_, SearchRegistry>,
    project: ProjectId,
    session: SearchSessionId,
) -> Result<SearchSession, String> {
    let descriptor = searches.session(project, session)?.descriptor.clone();
    if descriptor.source == SearchSource::Text {
        let fs = fs.get(project).ok_or("Project files are still opening.")?;
        let roots = fs
            .roots
            .iter()
            .map(|r| SearchRoot {
                path: r.path.clone(),
                label: r.label.clone(),
            })
            .collect::<Vec<_>>();
        plan(&descriptor.query, &roots).map_err(|problem| match problem {
            SearchProblem::Pattern { detail }
            | SearchProblem::Scope { detail }
            | SearchProblem::Include { detail } => detail,
        })?;
    }
    searches.pin_session(project, session)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn search_session_list(
    searches: State<'_, SearchRegistry>,
    project: ProjectId,
) -> Result<Vec<SearchSession>, String> {
    Ok(searches
        .sessions
        .lock()
        .iter()
        .filter(|s| s.project == project && s.pinned.load(Ordering::Acquire))
        .map(|s| s.summary())
        .collect())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn search_session_rerun(
    fs: State<'_, FsRegistry>,
    searches: State<'_, SearchRegistry>,
    project: ProjectId,
    session: SearchSessionId,
) -> Result<SearchSession, String> {
    let fs = fs.get(project).ok_or("Project files are still opening.")?;
    searches.restart_session(project, session, fs)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn search_session_cancel(
    searches: State<'_, SearchRegistry>,
    project: ProjectId,
    session: SearchSessionId,
) -> Result<(), String> {
    searches.close_session(project, session, true);
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn search_session_close(
    searches: State<'_, SearchRegistry>,
    project: ProjectId,
    session: SearchSessionId,
) -> Result<(), String> {
    searches.close_session(project, session, false);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn descriptor(pattern: &str) -> SearchDescriptor {
        SearchDescriptor {
            source: SearchSource::Text,
            libraries: false,
            query: SearchQuery {
                pattern: pattern.into(),
                mode: cide_ipc::SearchMode::Literal,
                case_sensitive: false,
                whole_word: false,
                scope: String::new(),
                include: String::new(),
            },
        }
    }
    #[test]
    fn pin_transfers_ownership_and_duplicate_queries_get_distinct_tabs() {
        let registry = SearchRegistry::default();
        let project = ProjectId::new();
        let first = registry.create_session(project, "main".into(), descriptor("alpha"));
        registry.pin_session(project, first.id).unwrap();
        let second = registry.create_session(project, "main".into(), descriptor("alpha"));
        assert_ne!(first.id, second.id);
        registry.close_session(project, first.id, true);
        assert!(registry.session(project, first.id).is_ok());
        registry.cancel_search_window("main");
        assert!(registry.session(project, first.id).is_ok());
        assert!(registry.session(project, second.id).is_err());
        assert!(!first.run.lock().1.stop.load(Ordering::Acquire));
        registry.cancel(project);
        assert!(registry.sessions.lock().is_empty());
        assert!(first.run.lock().1.stop.load(Ordering::Acquire));
    }
    #[test]
    fn closing_one_search_does_not_cancel_another_window_or_project() {
        let registry = SearchRegistry::default();
        let project = ProjectId::new();
        let a = registry.create_session(project, "one".into(), descriptor("a"));
        let b = registry.create_session(project, "two".into(), descriptor("b"));
        let c = registry.create_session(ProjectId::new(), "three".into(), descriptor("c"));
        registry.close_session(project, b.id, false);
        assert!(!a.run.lock().1.stop.load(Ordering::Acquire));
        assert!(b.run.lock().1.stop.load(Ordering::Acquire));
        assert!(!c.run.lock().1.stop.load(Ordering::Acquire));
        registry.cancel_all();
        assert!(a.run.lock().1.stop.load(Ordering::Acquire));
        assert!(c.run.lock().1.stop.load(Ordering::Acquire));
    }
    #[test]
    fn completed_text_search_still_pages_all_results() {
        let registry = SearchRegistry::default();
        let project = ProjectId::new();
        let s = registry.create_session(project, "main".into(), descriptor("a"));
        s.run.lock().1.text.found.lock().hits = (0..450)
            .map(|line| SearchHit {
                path: "/test".into(),
                rel: "test".into(),
                line,
                text: "a".into(),
                start: 0,
                end: 1,
            })
            .collect();
        let a = s.frame(0);
        assert!(!a.running);
        assert_eq!(a.text.unwrap().hits.len(), 200);
        assert_eq!(s.frame(200).text.unwrap().hits.len(), 200);
        assert_eq!(s.frame(400).text.unwrap().hits.len(), 50);
    }

    struct QuietEvents;
    impl crate::files::FsEvents for QuietEvents {
        fn status(&self, _: ProjectId, _: &cide_ipc::FsStatus) {}
        fn changed(&self, _: ProjectId, _: &cide_ipc::FsChange) {}
    }

    async fn settled(session: &Session) -> SearchSessionFrame {
        for _ in 0..500 {
            let frame = session.frame(0);
            if !frame.running {
                return frame;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!(
            "search did not finish: {}",
            session.descriptor.query.pattern
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn all_providers_are_isolated_and_refresh_replaces_only_its_run() {
        use cide_search::Matcher as _;
        let dir = cide_fs::testing::scratch("search-sessions");
        std::fs::write(dir.join("alpha.rs"), "fn alpha() {}\n").unwrap();
        std::fs::write(dir.join("beta.rs"), "fn beta() {}\n").unwrap();
        let project = ProjectId::new();
        let files = FsRegistry::default();
        files
            .claim(
                project,
                vec![dir.to_path_buf()],
                cide_fs::Visibility::CONSERVATIVE,
            )
            .unwrap()
            .run(Arc::new(QuietEvents), project);
        let fs = files.get(project).unwrap();
        let registry = SearchRegistry::default();
        let mut query = descriptor("alpha");
        query.source = SearchSource::All;
        let alpha = registry.create_session(project, "main".into(), query);
        launch(
            &alpha.descriptor,
            Arc::clone(&alpha.run.lock().1),
            Arc::clone(&fs),
        );
        registry.pin_session(project, alpha.id).unwrap();
        let mut query = descriptor("beta");
        query.source = SearchSource::All;
        let beta = registry.create_session(project, "main".into(), query);
        launch(
            &beta.descriptor,
            Arc::clone(&beta.run.lock().1),
            Arc::clone(&fs),
        );
        // Legacy consumers can use the shared picker without changing either session.
        fs.matcher().query("does-not-match");
        for (session, expected) in [(&alpha, "alpha"), (&beta, "beta")] {
            let frame = settled(session).await;
            let files = frame.files.unwrap();
            assert_eq!(files.items.len(), 1);
            assert!(files.items[0].value.ends_with(&format!("{expected}.rs")));
            let symbols = frame.symbols.unwrap();
            assert_eq!(symbols.items.len(), 1);
            assert_eq!(symbols.items[0].name, expected);
            assert_eq!(frame.text.unwrap().hits.len(), 1);
        }
        let old = Arc::clone(&alpha.run.lock().1);
        let next = registry
            .restart_session(project, alpha.id, Arc::clone(&fs))
            .unwrap();
        assert_eq!(next.id, alpha.id);
        assert_eq!(next.generation, 1);
        assert!(next.pinned);
        assert!(old.stop.load(Ordering::Acquire));
        assert_eq!(settled(&alpha).await.session.generation, 1);
        assert_eq!(beta.summary().generation, 0);
        registry.close_session(project, alpha.id, false);
        assert!(registry.session(project, beta.id).is_ok());
        registry.cancel(project);
        files.close_all();
    }

    #[test]
    fn racing_pin_and_modal_cancellation_never_report_a_lost_pin_as_success() {
        for _ in 0..40 {
            let registry = SearchRegistry::default();
            let project = ProjectId::new();
            let session = registry.create_session(project, "main".into(), descriptor("alpha"));
            std::thread::scope(|scope| {
                let pin = scope.spawn(|| registry.pin_session(project, session.id));
                let cancel = scope.spawn(|| registry.close_session(project, session.id, true));
                let pinned = pin.join().unwrap();
                cancel.join().unwrap();
                if pinned.is_ok() {
                    assert!(registry.session(project, session.id).is_ok());
                } else {
                    assert!(registry.session(project, session.id).is_err());
                }
            });
        }
    }
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ignored_and_external_sources_share_all_three_search_providers() {
        let dir = cide_fs::testing::scratch("catalog-ignored");
        let external = cide_fs::testing::scratch("catalog-external");
        std::fs::write(dir.join(".gitignore"), ".cache/\n").unwrap();
        std::fs::create_dir(dir.join(".cache")).unwrap();
        std::fs::write(dir.join(".cache/needle.rs"), "fn needle() {}\n").unwrap();
        std::fs::write(external.join("needle.rs"), "fn needle() {}\n").unwrap();
        std::os::unix::fs::symlink(external.path(), dir.join(".cache/drive")).unwrap();
        let project = ProjectId::new();
        let files = FsRegistry::default();
        files
            .claim(
                project,
                vec![dir.to_path_buf()],
                cide_fs::Visibility {
                    hidden: true,
                    ignored: true,
                },
            )
            .unwrap()
            .run(Arc::new(QuietEvents), project);
        let registry = SearchRegistry::default();
        let mut query = descriptor("needle");
        query.source = SearchSource::All;
        let session = registry.create_session(project, "main".into(), query);
        launch(
            &session.descriptor,
            session.run.lock().1.clone(),
            files.get(project).unwrap(),
        );
        let frame = settled(&session).await;
        assert_eq!(frame.files.unwrap().items.len(), 2);
        assert_eq!(frame.symbols.unwrap().items.len(), 2);
        assert_eq!(frame.text.unwrap().hits.len(), 2);
        assert!(!frame.incomplete);
        registry.cancel(project);
        files.close_all();
    }
}
