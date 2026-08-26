//! Whether a shell pane is sitting at its prompt or running something, from the pty's
//! foreground process group.
//!
//! This is what lets a *shell* pane raise the same "awaiting" signal a Claude pane raises
//! when a turn ends — the pane dot, the tab and project badges, the `Awaiting: n` window
//! title and the task-bar urgency hint. None of those surfaces were ever Claude-specific:
//! they key off a session id, and the only reason a shell pane stayed dark is that a shell
//! session emitted exactly one `SessionState` in its whole life, `Exited`, at death.
//!
//! ## Why the process group and not the output
//!
//! A terminal emulator cannot see a child's exit — the shell is the only process that knows
//! a job finished. The three ways to find out anyway:
//!
//! * **The foreground process group**, which is this module. `tcgetpgrp` on the master
//!   answers "who currently owns the terminal": the shell's own pgid at a prompt, the job's
//!   while one runs, because job control puts each pipeline in its own group. It needs
//!   nothing from the user, works for bash, zsh, fish and anything else with job control,
//!   and cannot be fooled by output. Its blind spot is a shell built or run *without* job
//!   control (`set +m`, some containers) and anything that keeps the shell's own pgid — an
//!   `ssh` or `tmux` that execs in place — where the foreground group never changes and this
//!   module correctly says nothing at all rather than guessing.
//! * **OSC 133 semantic prompt marks**, which are exact and carry the exit code, but need
//!   shell integration installed into the user's rc files. Worth adding later as an override
//!   for a session that emits them; deliberately not a prerequisite, because a feature that
//!   only works after the user edits `.bashrc` is a feature most users never see.
//! * **Output activity heuristics**, which are free and wrong: a `tail -f` is permanently
//!   busy and a `sleep 300 && make` is permanently idle.
//!
//! ## Why a threshold, and why it gates the *start*
//!
//! An announced job's `Finished` is reported as `AwaitingInput`, which raises the marker
//! outright, so announcing a job at all is what makes it notify. A bare `ls` finishing must
//! not flash the dot, both badges and the window title, so the rule below does not announce a
//! job until it has already been running for [`JobWatch::announce_after`] — a job shorter
//! than that produces no `Started` and therefore no `Finished`, and the frontend never hears
//! it existed. Suppressing at the *end* instead is not possible and it is worth writing down
//! why: `AwaitingInput` raises the marker regardless of what the frontend saw before it (the
//! same unconditional reading a finished Claude turn relies on), so a job announced once
//! notifies at its end whatever came after the announcement.
//!
//! ## Why a fullscreen TUI is not a job
//!
//! `vim`, `htop` and `less` all take their own process group and would otherwise notify on
//! quit — an interruption for something the user was, by definition, sitting in front of.
//! A job that engages the alternate screen at any point before it would have been announced
//! is therefore suppressed for its whole life. The signal is already in the session's vt100
//! mirror ([`crate::PtySession::in_alternate_screen`]), so this costs a bool.

use std::time::{Duration, Instant};

/// What a shell pane's foreground has just done, for a consumer that reports session state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobEvent {
    /// A foreground job has now been running for at least the announce threshold.
    ///
    /// Emitted *during* the job rather than at its start — see the module docs. A consumer
    /// maps this to `SessionState::Busy`, which both arms the awaiting rule and keeps the
    /// pane's host out of the eviction sweep for as long as the job runs.
    Started,
    /// The announced job ended and the shell has the terminal back.
    ///
    /// Only ever follows a [`JobEvent::Started`] for the same run of work, so a consumer can
    /// map it straight to `SessionState::AwaitingInput` without checking anything.
    Finished {
        /// How long the whole run of work took, measured from the first observation that saw
        /// a job rather than from the announcement.
        ran_for: Duration,
    },
}

/// What the terminal's foreground looked like at one observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The shell owns the terminal: nothing is running.
    AtPrompt,
    Running {
        /// The first observation that saw this run of work.
        since: Instant,
        /// [`JobEvent::Started`] has been emitted, so an end is worth reporting.
        announced: bool,
        /// A fullscreen TUI held the screen, so this run is not a job anybody is waiting on.
        suppressed: bool,
    },
}

/// Turns a sequence of foreground-process-group observations into job events.
///
/// Pure and clock-injected so `cargo test -p cide-pty jobs` can drive a whole session's worth
/// of transitions in microseconds; the syscall and the polling live in the coalescer, which
/// is the half that cannot be tested without a real pty.
#[derive(Debug)]
pub struct JobWatch {
    /// The shell's own process group.
    ///
    /// This is the child's pid, and the equality is not an assumption: `portable-pty` calls
    /// `setsid()` in the child before `exec`, so the shell is a session leader and therefore
    /// leads a process group of its own number.
    shell: i32,
    announce_after: Duration,
    phase: Phase,
    /// Whether the shell has ever been seen owning its own terminal.
    ///
    /// A guard for startup only. `bash -l` sources profile scripts before it first prompts,
    /// and anything those fork owns the terminal for as long as it runs; without this, a slow
    /// `nvm` or `direnv` in somebody's `.bash_profile` would be announced as the pane's first
    /// job. Nothing is reported until the prompt has been seen once.
    seen_prompt: bool,
}

impl JobWatch {
    /// Watch `shell` — the pid of the shell this pane spawned — announcing jobs that reach
    /// `announce_after`.
    pub fn new(shell: i32, announce_after: Duration) -> Self {
        Self {
            shell,
            announce_after,
            phase: Phase::AtPrompt,
            seen_prompt: false,
        }
    }

    /// Change the announce threshold on a watch that is already running.
    ///
    /// The threshold is user-tunable now, and a session outlives every pane, tab and window —
    /// most shells are spawned once per app run — so a threshold fixed at spawn would make
    /// the setting look dead in every pane the user already has open. Nothing else is
    /// touched: a job already announced stays announced (nothing downstream can retract a
    /// `Busy`), and a job still unannounced is simply measured against the new number on the
    /// next observation, whichever direction it moved.
    pub fn set_announce_after(&mut self, announce_after: Duration) {
        self.announce_after = announce_after;
    }

    /// Fold one observation in, and say whether it changed anything worth reporting.
    ///
    /// `foreground` is `tcgetpgrp`'s answer and `None` means it had none — a pty that has
    /// gone away, or a platform that does not answer. That is *no information*, deliberately
    /// distinct from "the shell is at its prompt": treating a failed call as a finished job
    /// would announce one every time a pane closed.
    ///
    /// `alternate_screen` is the vt100 mirror's, read at the same instant.
    pub fn observe(
        &mut self,
        foreground: Option<i32>,
        alternate_screen: bool,
        now: Instant,
    ) -> Option<JobEvent> {
        let foreground = foreground?;

        if foreground == self.shell {
            self.seen_prompt = true;
            let ended = match self.phase {
                Phase::Running {
                    since,
                    announced: true,
                    ..
                } => Some(JobEvent::Finished {
                    ran_for: now.saturating_duration_since(since),
                }),
                _ => None,
            };
            self.phase = Phase::AtPrompt;
            return ended;
        }

        // Something other than the shell owns the terminal.
        if !self.seen_prompt {
            return None;
        }

        match &mut self.phase {
            Phase::AtPrompt => {
                self.phase = Phase::Running {
                    since: now,
                    announced: false,
                    suppressed: alternate_screen,
                };
                None
            }
            Phase::Running {
                since,
                announced,
                suppressed,
            } => {
                if *announced || *suppressed {
                    // A pgid that changes from one job to the next without the shell being
                    // seen in between is deliberately *not* a new run: `make && make test`
                    // is one wait, and reporting it as two would notify in the middle of
                    // work the user is still waiting on.
                    return None;
                }
                if alternate_screen {
                    *suppressed = true;
                    return None;
                }
                if now.saturating_duration_since(*since) < self.announce_after {
                    return None;
                }
                *announced = true;
                Some(JobEvent::Started)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHELL: i32 = 4242;
    const JOB: i32 = 4243;
    const AFTER: Duration = Duration::from_secs(10);

    fn watch() -> (JobWatch, Instant) {
        let mut w = JobWatch::new(SHELL, AFTER);
        let t0 = Instant::now();
        // Every test starts from a shell that has prompted at least once, which is the
        // state a pane is in the moment a user can type into it.
        assert_eq!(w.observe(Some(SHELL), false, t0), None);
        (w, t0)
    }

    #[test]
    fn a_long_job_is_announced_once_and_reported_when_it_ends() {
        let (mut w, t0) = watch();
        assert_eq!(w.observe(Some(JOB), false, t0), None, "a job just started");
        assert_eq!(
            w.observe(Some(JOB), false, t0 + Duration::from_secs(9)),
            None,
            "still short of the threshold",
        );
        assert_eq!(
            w.observe(Some(JOB), false, t0 + AFTER),
            Some(JobEvent::Started),
            "the threshold is reached exactly",
        );
        assert_eq!(
            w.observe(Some(JOB), false, t0 + Duration::from_secs(20)),
            None,
            "and is announced once, not on every poll",
        );
        assert_eq!(
            w.observe(Some(SHELL), false, t0 + Duration::from_secs(30)),
            Some(JobEvent::Finished {
                ran_for: Duration::from_secs(30)
            }),
            "the prompt coming back ends it, timed from the first sighting",
        );
    }

    #[test]
    fn a_short_job_is_never_mentioned() {
        // The whole reason the threshold gates the *start*: `awaitingRule.ts` cannot retract
        // a `Busy`, so an `ls` that was announced would raise the marker however fast it ran.
        let (mut w, t0) = watch();
        assert_eq!(w.observe(Some(JOB), false, t0), None);
        assert_eq!(
            w.observe(Some(SHELL), false, t0 + Duration::from_millis(40)),
            None,
            "a job that never reached the threshold ends silently",
        );
    }

    #[test]
    fn back_to_back_jobs_are_one_wait() {
        let (mut w, t0) = watch();
        assert_eq!(w.observe(Some(JOB), false, t0), None);
        assert_eq!(
            w.observe(Some(JOB), false, t0 + AFTER),
            Some(JobEvent::Started)
        );
        assert_eq!(
            w.observe(Some(JOB + 1), false, t0 + Duration::from_secs(11)),
            None,
            "the second half of `make && make test` is not a second notification",
        );
        assert_eq!(
            w.observe(Some(SHELL), false, t0 + Duration::from_secs(20)),
            Some(JobEvent::Finished {
                ran_for: Duration::from_secs(20)
            }),
        );
    }

    #[test]
    fn a_fullscreen_tui_is_suppressed_for_its_whole_life() {
        let (mut w, t0) = watch();
        assert_eq!(w.observe(Some(JOB), true, t0), None, "vim takes the screen");
        assert_eq!(
            w.observe(Some(JOB), true, t0 + Duration::from_secs(600)),
            None,
            "ten minutes in an editor is not a job to announce",
        );
        assert_eq!(
            w.observe(Some(SHELL), false, t0 + Duration::from_secs(601)),
            None,
            "and quitting it notifies nobody",
        );
    }

    #[test]
    fn a_job_that_goes_fullscreen_later_is_suppressed_too() {
        // `git log` paging into `less` is the ordinary case: the pgid is taken first and the
        // alternate screen arrives a beat later.
        let (mut w, t0) = watch();
        assert_eq!(w.observe(Some(JOB), false, t0), None);
        assert_eq!(
            w.observe(Some(JOB), true, t0 + Duration::from_millis(250)),
            None
        );
        assert_eq!(
            w.observe(Some(JOB), false, t0 + Duration::from_secs(30)),
            None,
            "suppression outlives the alternate screen — leaving it is not a fresh job",
        );
        assert_eq!(
            w.observe(Some(SHELL), false, t0 + Duration::from_secs(31)),
            None
        );
    }

    #[test]
    fn a_tui_started_after_a_job_was_announced_still_reports() {
        // The mirror image: a long build that ends by paging its output has already been
        // announced, and the user is genuinely waiting on it.
        let (mut w, t0) = watch();
        assert_eq!(w.observe(Some(JOB), false, t0), None);
        assert_eq!(
            w.observe(Some(JOB), false, t0 + AFTER),
            Some(JobEvent::Started)
        );
        assert_eq!(
            w.observe(Some(JOB), true, t0 + Duration::from_secs(11)),
            None
        );
        assert!(matches!(
            w.observe(Some(SHELL), false, t0 + Duration::from_secs(12)),
            Some(JobEvent::Finished { .. })
        ));
    }

    #[test]
    fn nothing_is_announced_before_the_first_prompt() {
        let mut w = JobWatch::new(SHELL, AFTER);
        let t0 = Instant::now();
        // A slow `direnv` in `.bash_profile`, before the shell has ever prompted.
        assert_eq!(w.observe(Some(JOB), false, t0), None);
        assert_eq!(
            w.observe(Some(JOB), false, t0 + Duration::from_secs(30)),
            None
        );
        assert_eq!(
            w.observe(Some(SHELL), false, t0 + Duration::from_secs(31)),
            None,
            "the pane's first prompt is not the end of a job",
        );
        // …and from there on it works normally.
        assert_eq!(
            w.observe(Some(JOB), false, t0 + Duration::from_secs(40)),
            None
        );
        assert_eq!(
            w.observe(Some(JOB), false, t0 + Duration::from_secs(50)),
            Some(JobEvent::Started),
        );
    }

    #[test]
    fn an_unanswered_call_is_not_a_finished_job() {
        // `tcgetpgrp` answers nothing for a pty that has gone away, which is exactly what a
        // pane being closed looks like. Reporting that as a finished job would notify on
        // every close.
        let (mut w, t0) = watch();
        assert_eq!(w.observe(Some(JOB), false, t0), None);
        assert_eq!(
            w.observe(Some(JOB), false, t0 + AFTER),
            Some(JobEvent::Started)
        );
        assert_eq!(w.observe(None, false, t0 + Duration::from_secs(11)), None);
        assert_eq!(
            w.observe(Some(SHELL), false, t0 + Duration::from_secs(12)),
            Some(JobEvent::Finished {
                ran_for: Duration::from_secs(12)
            }),
            "and the state it was in survives the gap",
        );
    }

    #[test]
    fn a_lowered_threshold_reaches_a_job_already_running() {
        // The live half of the setting: the user drops the threshold while a build runs, and
        // the running build is announced against the new number rather than the one its pane
        // was spawned with.
        let (mut w, t0) = watch();
        assert_eq!(w.observe(Some(JOB), false, t0), None);
        w.set_announce_after(Duration::from_secs(2));
        assert_eq!(
            w.observe(Some(JOB), false, t0 + Duration::from_secs(3)),
            Some(JobEvent::Started),
            "three seconds in is past the new threshold, though short of the old one",
        );
    }

    #[test]
    fn a_raised_threshold_quiets_a_job_that_would_have_been_announced() {
        let (mut w, t0) = watch();
        assert_eq!(w.observe(Some(JOB), false, t0), None);
        w.set_announce_after(Duration::from_secs(60));
        assert_eq!(
            w.observe(Some(JOB), false, t0 + AFTER),
            None,
            "the old threshold no longer announces anything",
        );
        assert_eq!(
            w.observe(Some(SHELL), false, t0 + Duration::from_secs(30)),
            None,
            "and a job that was never announced ends silently",
        );
    }

    #[test]
    fn a_shell_without_job_control_says_nothing_at_all() {
        // The documented blind spot: the foreground group never leaves the shell, so every
        // observation is "at the prompt" and the pane behaves exactly as it did before this
        // module existed. Silence is the correct degrade — the alternative is guessing.
        let (mut w, t0) = watch();
        for s in 0..60 {
            assert_eq!(
                w.observe(Some(SHELL), false, t0 + Duration::from_secs(s)),
                None,
            );
        }
    }
}
