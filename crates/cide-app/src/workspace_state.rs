//! The app's handle on the domain.
//!
//! `cide-core` owns the workspace tree and knows nothing about Tauri; this is the piece
//! that gives it a home in the running process — one lock, one file path, and the debounce
//! that decides when a burst of mutations becomes a write.
//!
//! Mutation goes through [`WorkspaceState::update`] rather than by handing out the guard,
//! so that advancing `rev`, re-validating and marking the file dirty cannot be forgotten at
//! a call site. A command handler that could skip validation would eventually skip it.
//! `update` also compares the tree before and after: a mutation that changed nothing is not
//! bumped, not persisted and not broadcast, and one that changed something is guaranteed a
//! bump even when its mutator forgot — so every broadcast carries a strictly newer `rev`.

use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use cide_core::persist::{self, Debouncer};
use cide_core::workspace;
use cide_ipc::Workspace;
use parking_lot::Mutex;
use std::sync::OnceLock;
use tauri::{AppHandle, Manager};

/// How often the flusher thread wakes to ask whether a write is owed.
///
/// 250 ms, half [`persist::SAVE_DEBOUNCE`], so a due write waits at most a quarter of a second
/// past its deadline — the relationship `tasks_state::POLL` and `POSITION_DEBOUNCE`/`POLL`
/// already have, one file over.
const POLL: Duration = Duration::from_millis(250);

pub struct WorkspaceState {
    inner: Mutex<Workspace>,
    path: PathBuf,
    debounce: Debouncer,
    /// Set once, during setup, so mutations can broadcast to every window.
    ///
    /// A `OnceLock` rather than a constructor argument because the state is created before
    /// the Tauri app exists — it is loaded from disk so that the very first
    /// `app.get_bootstrap` answers from a real tree rather than from defaults it then has to
    /// correct.
    app: OnceLock<AppHandle>,
}

/// Tell `cide_agents::defs` which binaries Settings → Harness configures, so every availability
/// probe — the Agents panel's roster, a dispatch's refusal, the MR review dialog — asks about the
/// binary a run will actually start rather than a bare name on `PATH`. (M107) See
/// `cide_agents::defs::set_configured_binary` for why this is pushed rather than passed.
fn publish_binaries(settings: &cide_ipc::Settings) {
    cide_agents::defs::set_configured_binary(
        cide_ipc::Harness::Claude,
        &settings.claude.cli.binary,
    );
    cide_agents::defs::set_configured_binary(cide_ipc::Harness::Codex, &settings.codex.cli.binary);
}

impl WorkspaceState {
    /// Load from disk, or start from defaults.
    ///
    /// Never fails: `persist::load` answers with defaults for anything it cannot read, so a
    /// broken file costs a layout rather than a launch.
    pub fn load() -> Self {
        let path = persist::workspace_path();
        let mut workspace = persist::load(&path);

        // A file that parses can still be internally inconsistent — hand-edited, or written
        // by a build whose invariants differed. Starting fresh is recoverable; running on a
        // tree that fails its own validator is not.
        if let Err(error) = workspace::validate(&workspace) {
            tracing::warn!(%error, "loaded workspace failed validation; starting from defaults");
            workspace = Workspace::default();
        }
        publish_binaries(&workspace.settings);

        Self {
            inner: Mutex::new(workspace),
            path,
            debounce: Debouncer::new(persist::SAVE_DEBOUNCE),
            app: OnceLock::new(),
        }
    }

    /// Start the background flusher. Call once, from `setup`.
    ///
    /// # Why this exists, and what was broken until it did (M73)
    ///
    /// [`Self::flush_if_due`] documented itself as "called from the app's tick and before
    /// quitting" and was called by **neither**: there is no app tick, so the 500 ms debounce
    /// was inert and `workspace.json` was written exactly once per run, at teardown. Two
    /// stores had already worked around it by polling themselves — `positions_state` and
    /// `tasks_state` both say so in their headers — and the workspace, the one file whose loss
    /// costs a layout nobody can re-derive, was the one still relying on a clean exit.
    ///
    /// What that cost, measured on the machine this was found on: an instance three hours into
    /// a session had a provider and a model pool configured through Settings that existed in
    /// its own memory and nowhere on disk. `run.sh`'s SIGTERM would have saved them; a crash,
    /// an OOM kill or a `SIGKILL` would have lost every setting and every layout change of the
    /// session, with nothing anywhere having reported a failure — the write had simply never
    /// been asked for.
    ///
    /// A thread rather than a Tauri async task, and a daemon rather than a joined one, for
    /// `PositionsState::start_flusher`'s reasons exactly: the work is a blocking file write,
    /// and [`crate::lifecycle::shutdown`] performs the final write itself rather than relying
    /// on this loop to notice. It holds an `AppHandle` rather than the state, because this
    /// state is managed by value and the handle is the only `'static` road back to it.
    pub fn start_flusher(app: AppHandle) {
        thread::Builder::new()
            .name("cide-workspace".into())
            .spawn(move || {
                loop {
                    thread::sleep(POLL);
                    // `try_state` rather than `state`: the handle outlives nothing here, but a
                    // panic on a teardown race would be a panic in a thread nobody joins, for
                    // a write the shutdown is about to make anyway.
                    if let Some(state) = app.try_state::<Self>() {
                        state.flush_if_due();
                    }
                }
            })
            // A failure to spawn costs the debounce, not the file: the shutdown flush still
            // runs, which is exactly the behaviour this whole session had before.
            .map(|_| ())
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "no workspace flusher; the workspace will be written on quit only");
            });
    }

    /// Give the state a handle to broadcast through. Called once, from `setup`.
    pub fn attach_app(&self, app: AppHandle) {
        let _ = self.app.set(app);
    }

    /// Read the tree. Clones, because a `MutexGuard` must not be held across an await or
    /// handed to serde while another command waits on it.
    pub fn snapshot(&self) -> Workspace {
        self.inner.lock().clone()
    }

    /// Decide something about the tree without another mutation landing mid-decision.
    ///
    /// `snapshot` is enough for anything that only reads. This exists for decisions that
    /// also consult state outside the workspace and must agree with it — window reconciling
    /// compares the workspace's window roles against the OS windows that actually exist, and
    /// between a snapshot and that enumeration a detach can commit and open a window, which
    /// then looks like one the workspace never named.
    ///
    /// The closure must not mutate, block, or call back into this state. It runs under the
    /// lock, so anything that re-enters deadlocks.
    pub fn with<T>(&self, f: impl FnOnce(&Workspace) -> T) -> T {
        f(&self.inner.lock())
    }

    pub fn rev(&self) -> u64 {
        self.inner.lock().rev
    }

    /// Mutate under the lock, then validate and mark the file dirty.
    ///
    /// A mutation that leaves the tree invalid is rolled back and reported: `cide-core`'s
    /// operations are individually invariant-preserving, so reaching here means either a
    /// compound edit that was not, or a bug — and neither should be persisted.
    ///
    /// A mutation that changed **nothing** is also not broadcast. That is not an
    /// optimisation nicety: `pane_focus` lands on every click including clicks on the
    /// already-focused pane, and `note_conversation` repeats the same id on every hook
    /// frame of a busy Claude turn — before this guard each of those cost every window a
    /// full snapshot parse and a full re-render, which is where "the app lags while an
    /// agent is working" came from. And a mutation that changed something without bumping
    /// `rev` has the bump applied here, which is what makes the module doc's "cannot be
    /// forgotten at a call site" true rather than aspirational: every broadcast carries a
    /// strictly newer `rev`, so the frontend can drop non-newer snapshots as duplicates.
    pub fn update<T>(
        &self,
        f: impl FnOnce(&mut Workspace) -> cide_core::Result<T>,
    ) -> cide_core::Result<T> {
        let mut guard = self.inner.lock();
        let before = guard.clone();

        let outcome = f(&mut guard);
        if outcome.is_ok()
            && let Err(error) = workspace::validate(&guard)
        {
            *guard = before;
            tracing::error!(%error, "rejected a mutation that broke a workspace invariant");
            return Err(error);
        }
        if outcome.is_err() {
            // An operation that reports failure should not have changed anything, but
            // restoring costs one clone and removes the question.
            *guard = before;
            return outcome;
        }

        // `before` is already paid for as the rollback copy; reuse it to answer "did this
        // mutation change anything?" — one structural compare, no second clone. `rev` is
        // masked out of the question in both directions: a mutator that bumps gratuitously
        // cannot make a no-op look like news, and one that forgot to bump cannot hide a
        // change.
        let rev_before = before.rev;
        let mut content_before = before;
        content_before.rev = guard.rev;
        if *guard == content_before {
            // A genuine no-op. Nothing to persist, nothing to broadcast, no titles to
            // move. Un-bumping keeps `rev` meaning "number of accepted changes", so a
            // closure that read `ws.rev` after a gratuitous bump would return a revision
            // that never broadcasts — mutators must bump only when they change something
            // (`activate_project` and `activate_tab` are the pattern), and the layout
            // commands read `state.rev()` after this returns, which is always honest.
            guard.rev = rev_before;
            return outcome;
        }
        // Settings → Harness's binaries, handed to the harness probe whenever they move. (M107)
        if content_before.settings.claude.cli.binary != guard.settings.claude.cli.binary
            || content_before.settings.codex.cli.binary != guard.settings.codex.cli.binary
        {
            publish_binaries(&guard.settings);
        }
        if guard.rev == rev_before {
            // Changed, but the mutator never bumped — true of every layout mutation in
            // `cmd/pane.rs` (`focus`, `maximize`, `set_ratio`, `distribute`, `swap`),
            // which historically leaned on the frontend accepting equal-rev snapshots.
            // Enforcing the bump here is what lets the frontend's `applySnapshot` treat
            // "not strictly newer" as "duplicate, drop it".
            guard.rev += 1;
        }

        self.debounce.note_change();

        // Broadcast while still holding the lock, so two concurrent mutations cannot emit
        // their snapshots in the opposite order to the one they were applied in. The clone
        // is the price of that; the alternative is a frontend that can be told about rev 8
        // and then rev 7 and has to sort it out from the number alone.
        let snapshot = guard.clone();
        drop(guard);
        if let Some(app) = self.app.get() {
            crate::emit::workspace_changed(app, &snapshot);
            // The OS window title is a **level over this tree**, so the one place that knows
            // the tree just moved is the one place that can hold it true. A title now names
            // the active project *and its active tab* (`cmd::window::title_for`), and the
            // events that change either — activating a header tab, switching tabs, opening or
            // closing a file, a `ClaudeFull` pane being renamed — are ordinary mutations with
            // no window code anywhere near them. Retitling from each of those call sites is
            // the version of this that goes stale the first time somebody adds a mutation and
            // does not know they have to.
            //
            // Run on every *accepted change*, which is still what makes it a level: every
            // fact a title reads lives in this tree, and a suppressed no-op moved no fact,
            // so skipping it there loses nothing. The other title inputs (the awaiting set,
            // the focus set) recompute through their own paths — `window_set_awaiting` and
            // `focus_changed` — not through here. `retitle` reads its own two mutexes and
            // never re-enters this state, and the workspace guard is dropped above, so
            // there is no lock to deadlock on.
            //
            // Note that this is *after* the broadcast and outside the lock, deliberately: a
            // GTK call is the slowest thing in this function and the webviews should not wait
            // behind it for the snapshot they are about to render.
            crate::cmd::window::retitle(app, &snapshot);
            // A pane was opened, closed, split, torn out, re-docked or had a session bound to
            // it; or a project was opened or closed. Every one of those moves the header's
            // running counts, and the argument for marking here is `retitle`'s above, verbatim:
            // this is the one place that knows the tree moved, and marking from each mutation
            // site is the version that goes stale the first time somebody adds one.
            //
            // Cheap and coalesced — `running::mark` takes one mutex and returns — so it is safe
            // on the path every gesture in the app runs through.
            crate::running::mark(app);
        }
        outcome
    }

    /// Write if the debounce has elapsed. Called from [`Self::start_flusher`]'s thread.
    ///
    /// It said "from the app's tick" until M73 and there has never been an app tick — see
    /// `start_flusher` for what that cost.
    pub fn flush_if_due(&self) {
        if self.debounce.take() {
            self.write_now();
        }
    }

    /// Write unconditionally. Used on quit, where waiting out a debounce would lose the
    /// last change the user made.
    pub fn write_now(&self) {
        let workspace = self.inner.lock().clone();
        if let Err(error) = persist::save_atomic(&self.path, &workspace) {
            tracing::error!(path = %self.path.display(), %error, "failed to save the workspace");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::Theme;

    /// A state with no `AppHandle` attached, so `update` skips the emit/retitle tail —
    /// which is exactly what lets the rev discipline be tested without a Tauri app.
    ///
    /// Holds one open project rather than `Workspace::default()`, because `update`
    /// re-validates after every closure and a default workspace has no shell window yet.
    fn state() -> WorkspaceState {
        let mut ws = Workspace::default();
        workspace::open_project(
            &mut ws,
            vec![std::path::PathBuf::from("/tmp/cide-test")],
            None,
        )
        .expect("a rooted project opens");
        WorkspaceState {
            inner: Mutex::new(ws),
            path: std::env::temp_dir().join("cide-workspace-state-test.json"),
            debounce: Debouncer::new(persist::SAVE_DEBOUNCE),
            app: OnceLock::new(),
        }
    }

    /// **The debounce is only a debounce if something drains it.** (M73)
    ///
    /// The behavioural half of `start_flusher`'s argument: a mutation is not on the disk
    /// immediately, is on it once the debounce has elapsed and a flush is asked for, and a
    /// second flush with nothing new does not rewrite the file. Until M73 the middle step had
    /// no caller in the whole binary, so every line of this was true and none of it ever ran.
    #[test]
    fn a_mutation_reaches_the_disk_on_the_debounce_and_not_before() {
        let mut s = state();
        // Its own file: every other test here shares one path and writes to none of it, and a
        // test that asserts on mtimes must not be readable by another test's `remove_file`.
        s.path = std::env::temp_dir().join("cide-workspace-flush-test.json");
        let _ = std::fs::remove_file(&s.path);

        s.update(|ws| {
            // `Dark`, because `Theme::default()` is `Light` and `update` suppresses a mutation
            // that changed nothing — a no-op notes no change, owes no write, and would make
            // every assertion below pass against a file that was never written.
            ws.settings.theme = Theme::Dark;
            Ok(())
        })
        .expect("a theme change is a change");

        s.flush_if_due();
        assert!(
            !s.path.exists(),
            "a write inside the debounce is the per-gesture cost the debounce exists to avoid"
        );

        std::thread::sleep(persist::SAVE_DEBOUNCE);
        s.flush_if_due();
        let written = persist::load(&s.path);
        assert_eq!(
            written.settings.theme,
            Theme::Dark,
            "and then it is on the disk"
        );
        assert_eq!(written.rev, s.rev());

        // Nothing owed, nothing written: the debouncer is taken, not merely read.
        let stamp = std::fs::metadata(&s.path)
            .and_then(|meta| meta.modified())
            .expect("written above");
        std::thread::sleep(persist::SAVE_DEBOUNCE);
        s.flush_if_due();
        assert_eq!(
            std::fs::metadata(&s.path)
                .and_then(|meta| meta.modified())
                .expect("still there"),
            stamp,
            "an idle flusher must not rewrite the file on every poll"
        );

        let _ = std::fs::remove_file(&s.path);
    }

    /// **And something does drain it.** (M73)
    ///
    /// The structural half, and the assertion that would have caught the original defect: the
    /// flusher is a thread started from `setup`, which no test can call. `flush_if_due` said it
    /// was "called from the app's tick" for three milestones while there was no tick and no
    /// caller — a sentence in a doc comment is not a caller, and only a source assertion can
    /// tell the two apart. Comments stripped first, because the paragraph above says the name.
    #[test]
    fn the_workspace_flusher_is_started_from_setup() {
        let lib = crate::srcgrep::without_comments(include_str!("lib.rs"));
        assert!(
            lib.contains("WorkspaceState::start_flusher("),
            "nothing starts the workspace flusher, so `workspace.json` is written once per run \
             and a session that does not exit cleanly loses every change it made"
        );
    }

    #[test]
    fn a_no_op_mutation_leaves_rev_untouched() {
        let s = state();
        let before = s.rev();
        // The closure runs and succeeds but changes nothing — the `pane_focus`-on-the-
        // focused-pane shape.
        s.update(|_ws| Ok(())).expect("a no-op succeeds");
        assert_eq!(s.rev(), before, "a no-op must not look like news");
    }

    #[test]
    fn a_gratuitous_bump_with_no_change_is_undone() {
        let s = state();
        let before = s.rev();
        s.update(|ws| {
            workspace::bump(ws);
            Ok(())
        })
        .expect("succeeds");
        assert_eq!(
            s.rev(),
            before,
            "rev counts accepted changes, not bump calls"
        );
    }

    #[test]
    fn a_change_that_forgot_to_bump_is_bumped_exactly_once() {
        let s = state();
        let before = s.rev();
        s.update(|ws| {
            // A real content change with no `bump` — the historical shape of every
            // layout mutation in `cmd/pane.rs`.
            ws.settings.theme = match ws.settings.theme {
                Theme::Dark => Theme::Light,
                Theme::Light => Theme::Dark,
            };
            Ok(())
        })
        .expect("succeeds");
        assert_eq!(s.rev(), before + 1, "every accepted change advances rev");
    }

    #[test]
    fn a_change_that_bumped_itself_is_not_bumped_again() {
        let s = state();
        let before = s.rev();
        s.update(|ws| {
            ws.settings.theme = match ws.settings.theme {
                Theme::Dark => Theme::Light,
                Theme::Light => Theme::Dark,
            };
            workspace::bump(ws);
            Ok(())
        })
        .expect("succeeds");
        assert_eq!(s.rev(), before + 1);
    }

    #[test]
    fn a_failing_mutation_still_rolls_back() {
        let s = state();
        let before = s.with(|ws| ws.clone());
        let out: cide_core::Result<()> = s.update(|ws| {
            ws.settings.theme = Theme::Dark;
            workspace::bump(ws);
            Err(cide_core::CoreError::Invariant("deliberate".into()))
        });
        assert!(out.is_err());
        assert_eq!(
            s.with(|ws| ws.clone()),
            before,
            "failure restores everything, rev included"
        );
    }
}
