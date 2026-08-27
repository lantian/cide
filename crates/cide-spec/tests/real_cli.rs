//! Against the real `openspec`. (M28)
//!
//! Every test here is `#[ignore]`d, which in this workspace means **it spawns a real binary** —
//! the convention `cide-lsp`'s `real_servers.rs` states and `cide-claude`'s live tests follow.
//! Run them deliberately:
//!
//! ```sh
//! cargo test -p cide-spec -- --ignored --nocapture
//! ```
//!
//! A missing binary **panics** rather than skipping. A test that quietly passes when the thing it
//! tests is absent is a test that reports green on a machine where the feature does not work, and
//! these are the only tests in the crate that can notice an upstream change: everything else runs
//! against checked-in fixtures, and a fixture cannot tell you that `npm i -g` moved a field.

use std::path::{Path, PathBuf};

use cide_ipc::{ChangeName, DeltaOperation, SpecBoard, SpecId, SpecWriteOutcome};
use cide_spec::{Openspec, write};

/// A scratch project, removed on drop unless the test failed.
///
/// `cide-git`'s `TempRepo` posture, including the part that matters when something breaks: a
/// panicking test **leaves the directory behind** and prints where, because the interesting
/// evidence for a markdown bug is the markdown.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "cide-spec-{}-{tag}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "[cide-spec tests] leaving {} for inspection",
                self.0.display()
            );
            return;
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The located binary, or a panic naming what is missing.
fn require_cli() {
    if let Err(refusal) = cide_spec::discover::find() {
        panic!("this test needs the real openspec: {}", refusal.sentence());
    }
}

/// An initialised project, and an `Openspec` bound to it.
fn project(tag: &str) -> (Scratch, Openspec) {
    require_cli();
    let scratch = Scratch::new(tag);
    let os = Openspec::open(&scratch.0).expect("the binary was found");
    os.init().expect("openspec init");
    assert!(
        cide_spec::present(&scratch.0),
        "init did not create openspec/"
    );
    (scratch, os)
}

fn write_file(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("a directory");
    }
    std::fs::write(path, text).expect("writable");
}

/// A whole delta file, built without any Rust literal containing `"###`.
fn delta(operation: &str, name: &str, body: &str, scenarios: &[(&str, &str)]) -> String {
    let h = |n: usize| "#".repeat(n);
    let mut out = format!(
        "{} {operation} Requirements\n\n{} Requirement: {name}\n{body}\n",
        h(2),
        h(3)
    );
    for (title, when) in scenarios {
        out.push_str(&format!(
            "\n{} Scenario: {title}\n- **WHEN** {when}\n- **THEN** it works\n",
            h(4)
        ));
    }
    out
}

/// `init`, `new change`, and a delta on disk that the CLI considers valid.
fn seeded(tag: &str) -> (Scratch, Openspec, ChangeName, SpecId) {
    let (scratch, os) = project(tag);
    let change = ChangeName("add-dark-mode".into());
    let spec = SpecId("dark-mode".into());
    os.new_change(&change, Some("Add a dark theme"))
        .expect("openspec new change");

    // `new change` scaffolds only `README.md` and `.openspec.yaml` — the proposal is written by
    // the `propose` command, i.e. by an agent. And `show --json` **refuses** a change with no
    // proposal, so a change folder with only a delta in it is not readable at all. That is why
    // `cmd::spec`'s "new change from this task" writes one: a change cide creates and then cannot
    // display would be a board row that opens onto an error.
    //
    // The Why section has a 50-character floor upstream, which is why this one is a sentence.
    let h = |n: usize| "#".repeat(n);
    write_file(
        &scratch.0.join("openspec/changes/add-dark-mode/proposal.md"),
        &format!(
            "{} Why\n\nThe editor is unreadable at night, and every other tool this team uses \
             already has a dark theme.\n\n{} What Changes\n\n- a theme switch in settings\n\n\
             {} Capabilities\n\n{} New Capabilities\n- `dark-mode`: switching the editor's \
             theme\n\n{} Impact\n\nThe editor and the settings page.\n",
            h(2),
            h(2),
            h(2),
            h(3),
            h(2)
        ),
    );

    write_file(
        &scratch
            .0
            .join("openspec/changes/add-dark-mode/specs/dark-mode/spec.md"),
        &delta(
            "ADDED",
            "Theme switching",
            "The app SHALL switch between a light and a dark theme.",
            &[("The user picks dark", "the user selects the dark theme")],
        ),
    );
    (scratch, os, change, spec)
}

#[test]
#[ignore = "spawns the real openspec CLI"]
fn init_does_not_prompt_with_no_tty() {
    // The hazard is invisible until it hangs a command worker: a bare `openspec init` asks which
    // of forty AI tools to configure. `OPEN_SPEC_INTERACTIVE=0` and a null stdin close it twice.
    // If this ever times out, the deadline is doing the job the flags were supposed to.
    let started = std::time::Instant::now();
    let (scratch, _os) = project("init");
    assert!(
        started.elapsed() < cide_spec::cli::MUTATE,
        "init took the whole deadline, which is what a prompt looks like from here"
    );
    assert!(
        scratch.0.join("openspec/project.md").exists() || scratch.0.join("openspec").is_dir(),
        "init produced no openspec/"
    );
}

#[test]
#[ignore = "spawns the real openspec CLI"]
fn init_installs_the_commands_the_panel_offers() {
    // `--tools claude` and never `--tools none`: that workflow is the prose cide deliberately
    // does not write for itself, and the panel's buttons type it.
    //
    // **This test has been wrong twice, and each time it passed while the feature was broken.**
    //
    // It first asserted only that `.claude/commands/opsx/` existed, and stayed green while the
    // panel offered `/opsx:onboard` — which is in the CLI's templates and is *not* installed by
    // the profile `init` uses, so Claude answered `Unknown command`. A test that checks a
    // container rather than its contents reports green on exactly the thing it covers.
    //
    // Then it asserted that directory's *contents*, and stayed green because it was never run:
    // OpenSpec moved the same workflow to Claude Code skills, `.claude/commands/opsx/` stopped
    // being written at all, and every button in the panel refused on every project set up by a
    // current CLI. This is the only test in the workspace that can see that class of change, so
    // it now asks the question the panel asks — *what does this project type to run propose* —
    // through the same function the panel and `spec_run_command` use.
    let (scratch, _os) = project("tools");
    let installed = cide_spec::claude::installed(&scratch.0);
    assert!(
        !installed.is_empty(),
        "`openspec init --tools claude` installed no command surface cide can see. It has \
         moved before — see `cide_spec::claude::SURFACES`, which is where a new one goes."
    );

    // Every command the panel's buttons type has to resolve to a line.
    for offered in ["propose", "explore"] {
        let line = cide_spec::claude::line(&scratch.0, offered);
        assert!(
            line.is_some(),
            "the panel offers `{offered}` and init installed no such command. What it installed: \
             {installed:?}"
        );
    }

    // And the one that caught the first failure, pinned as *absent* — so a future release adding
    // it is a failure that says "you can offer onboard now" rather than a silent chance nobody
    // takes.
    assert!(
        cide_spec::claude::line(&scratch.0, "onboard").is_none(),
        "openspec now installs an `onboard` command by default — the empty-board screen could \
         offer it, which is the guided walk-through this feature wanted and could not have"
    );
}

#[test]
#[ignore = "spawns the real openspec CLI"]
fn the_real_cli_answers_the_shapes_the_parsers_expect() {
    // The test that keeps the checked-in fixtures honest. A fixture cannot notice that `npm i -g`
    // renamed a field; this can, and it names the command when it does.
    let (_scratch, os, change, _spec) = seeded("shapes");

    let board = os.board().expect("list --json");
    let SpecBoard::Ready { changes, .. } = &board else {
        panic!("a set-up project is not Ready: {board:?}");
    };
    assert!(
        changes.iter().any(|c| c.name == change),
        "the change just created is not on the board: {changes:?}"
    );

    let detail = os.change(&change).expect("show/status/validate");
    assert_eq!(detail.name, change);
    assert!(
        detail
            .deltas
            .iter()
            .any(|d| d.operation == DeltaOperation::Added),
        "the ADDED delta did not survive the read: {:?}",
        detail.deltas
    );
    assert!(
        detail
            .artifacts
            .iter()
            .any(|a| a.id == "tasks" || a.generates.contains("tasks")),
        "no tasks artifact was reported: {:?}",
        detail.artifacts
    );
    // `instructions apply --json` is where progress comes from, and `0/0` must not read as done.
    assert!(!detail.progress.complete() || detail.progress.total > 0);
}

#[test]
#[ignore = "spawns the real openspec CLI"]
fn a_requirement_keeps_its_name_and_its_scenario_titles() {
    // Both of these were wrong in the first version and neither was visible from a fixture,
    // because the fixtures were written from the docs rather than from the tool.
    //
    // `openspec show --json` returns a requirement's `text` with its `### Requirement:` header
    // already folded away, and each scenario's `rawText` as the body alone — so the name and
    // every title are simply absent from the wire. Cards rendered nameless, `issuesFor` matched
    // nothing, and an edit would have written back scenarios that had lost their titles.
    let (_scratch, os, change, _spec) = seeded("names");
    let detail = os.change(&change).expect("read");

    let delta = detail
        .deltas
        .first()
        .unwrap_or_else(|| panic!("no deltas: {detail:?}"));
    assert_eq!(
        delta.requirements.len(),
        1,
        "the CLI sends `requirement` AND `requirements` for a one-requirement delta; counting \
         both drew the same card twice: {:?}",
        delta.requirements
    );

    let requirement = &delta.requirements[0];
    assert_eq!(requirement.name, "Theme switching", "the name is recovered");
    assert!(
        requirement.text.contains("SHALL"),
        "and the prose with it: {:?}",
        requirement.text
    );
    assert_eq!(requirement.scenarios.len(), 1);
    assert_eq!(
        requirement.scenarios[0].title, "The user picks dark",
        "a scenario keeps the title its header gives it"
    );
    assert!(
        requirement.scenarios[0].body.contains("**WHEN**"),
        "and its body: {:?}",
        requirement.scenarios[0].body
    );

    // And the raw block is the file's own bytes, which is what the editor edits.
    assert!(
        requirement
            .block
            .starts_with("### Requirement: Theme switching"),
        "the block carries its header: {:?}",
        requirement.block
    );
}

#[test]
#[ignore = "spawns the real openspec CLI"]
fn a_capability_with_several_requirements_is_one_delta_and_not_one_per_requirement() {
    // **`openspec show --json` emits one delta row per requirement, not per file.** A change
    // touching two capabilities with six and four requirements answers with *ten* rows. Reading
    // the delta file once per row and returning everything in it therefore squared: a reader saw
    // the same paragraph fourteen times, with nothing on screen suggesting why.
    //
    // No fixture could have caught this — the shape only appears once a capability has more than
    // one requirement, and every hand-written fixture in this crate had exactly one.
    let (scratch, os, change, _spec) = seeded("grouping");

    let h = |n: usize| "#".repeat(n);
    let mut delta = format!("{} ADDED Requirements\n", h(2));
    for name in ["First", "Second", "Third"] {
        delta.push_str(&format!(
            "\n{} Requirement: {name}\nThe app SHALL {}.\n\n{} Scenario: {name} happens\n\
             - **WHEN** a\n- **THEN** b\n",
            h(3),
            name.to_lowercase(),
            h(4)
        ));
    }
    write_file(
        &scratch
            .0
            .join("openspec/changes/add-dark-mode/specs/dark-mode/spec.md"),
        &delta,
    );

    let detail = os.change(&change).expect("read");
    assert_eq!(
        detail.deltas.len(),
        1,
        "three requirements in one capability are **one** delta, not three: {:?}",
        detail
            .deltas
            .iter()
            .map(|d| (d.spec.as_str(), d.requirements.len()))
            .collect::<Vec<_>>()
    );

    let names: Vec<&str> = detail.deltas[0]
        .requirements
        .iter()
        .map(|requirement| requirement.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["First", "Second", "Third"],
        "each requirement appears exactly once, in document order"
    );
}

#[test]
#[ignore = "spawns the real openspec CLI"]
fn a_requirement_written_back_unchanged_reads_back_identical() {
    // The guarantee the editor rests on. It is a *semantic* round trip and the byte-level one is
    // stronger still: `block` is the file's own bytes, so writing it back is a no-op splice.
    let (scratch, os, change, spec) = seeded("roundtrip");
    let path = scratch
        .0
        .join("openspec/changes/add-dark-mode/specs/dark-mode/spec.md");
    let before = std::fs::read_to_string(&path).expect("readable");

    let detail = os.change(&change).expect("read");
    let requirement = detail.deltas[0].requirements[0].clone();

    let outcome = write::set_requirement(
        &os,
        &change,
        &spec,
        DeltaOperation::Added,
        &requirement.name,
        &requirement.block,
    )
    .expect("the write path ran");
    assert!(
        matches!(outcome, SpecWriteOutcome::Written { .. }),
        "{outcome:?}"
    );

    assert_eq!(
        std::fs::read_to_string(&path).expect("readable"),
        before,
        "writing a requirement back as itself changed the file"
    );
    let after = os.change(&change).expect("read again");
    assert_eq!(after.deltas[0].requirements[0], requirement);
}

#[test]
#[ignore = "spawns the real openspec CLI"]
fn a_change_archives_and_its_requirements_land_in_the_specs() {
    // The gesture the panel's context menu runs, end to end. Nothing else in the suite drives a
    // real `openspec archive`, and archiving is the one command that rewrites files cide did not
    // open — so "the deltas actually arrive in specs/" is a claim only this can make.
    let (scratch, os, change, _spec) = seeded("archive");

    // A change is only archivable once its checklist is done; `seeded` writes none, so this also
    // exercises the state the menu greys the item in.
    write_file(
        &scratch.0.join("openspec/changes/add-dark-mode/tasks.md"),
        "- [x] Add the theme tokens\n",
    );

    let before = os.change(&change).expect("readable before");
    assert!(
        before.progress.complete(),
        "the fixture's checklist is not finished: {:?}",
        before.progress
    );
    assert!(
        before.validation.valid,
        "the fixture does not validate: {:?}",
        before.validation.issues
    );

    os.archive(&change).expect("openspec archive");

    // The requirement is in the spec now…
    let spec_file = scratch.0.join("openspec/specs/dark-mode/spec.md");
    let merged = std::fs::read_to_string(&spec_file)
        .unwrap_or_else(|e| panic!("{} was not written: {e}", spec_file.display()));
    assert!(
        merged.contains("Theme switching"),
        "the delta did not reach the spec: {merged}"
    );

    // …and the change is out of the active list.
    let board = os.board().expect("list");
    let SpecBoard::Ready { changes, specs, .. } = &board else {
        panic!("not ready: {board:?}");
    };
    assert!(
        !changes.iter().any(|c| c.name == change),
        "the archived change is still listed as active: {changes:?}"
    );
    assert!(
        specs.iter().any(|s| s.id.as_str() == "dark-mode"),
        "the capability is not on the board: {specs:?}"
    );

    /*
     * And it is still readable, which is the half the CLI cannot do.
     *
     * `show` resolves `openspec/changes/<name>/proposal.md`, so from this moment it answers *not
     * found* — and the card of the task that did the work said so, at exactly the point the
     * record became worth keeping. This asserts against the **real** archive layout rather than
     * a fixture, because the date stamp and the directory shape are upstream's and the unit test
     * one file over can only assert cide's reading of what cide wrote.
     */
    assert!(
        os.change(&change).is_err(),
        "if the CLI ever learns to read an archive, the fallback below is dead code"
    );
    let archived = cide_spec::archived_change(&scratch.0, &change)
        .expect("the archived change reads back off the disk");
    match &archived.origin {
        cide_ipc::SpecOrigin::Archived { folder } => assert!(
            folder.ends_with(change.as_str()),
            "the folder is the change with a stamp on it: {folder}"
        ),
        other => panic!("read as {other:?}, not as archived"),
    }
    assert!(
        archived
            .deltas
            .iter()
            .flat_map(|delta| delta.requirements.iter())
            .any(|requirement| requirement.name == "Theme switching"),
        "the archived deltas lost the requirement: {:?}",
        archived.deltas
    );
    assert!(
        archived.artifacts.iter().any(|a| a.id == "proposal"),
        "the documents are listed so the card can open them: {:?}",
        archived.artifacts
    );
}

#[test]
#[ignore = "spawns the real openspec CLI"]
fn a_root_resolved_from_an_ancestor_is_refused() {
    // The failure this prevents is a plausible board belonging to somebody else: OpenSpec walks
    // parent directories, and an agent's worktree with no openspec/ of its own would otherwise
    // report the parent repository's progress as that run's.
    let (scratch, _os, _change, _spec) = seeded("ancestor");
    let nested = scratch.0.join("worktrees/dev");
    std::fs::create_dir_all(&nested).expect("a nested directory");

    let inner = Openspec::open(&nested).expect("the binary was found");
    let error = inner
        .board()
        .expect_err("the ancestor's board must be refused");
    let sentence = error.to_string();
    assert!(
        sentence.contains("another project's specs") || sentence.contains("rather than to"),
        "the refusal does not explain what went wrong: {sentence}"
    );
}

#[test]
#[ignore = "spawns the real openspec CLI"]
fn a_proposed_change_is_immediately_readable() {
    // The bug this closes was found by these tests and not by review: `openspec new change`
    // writes only README.md and .openspec.yaml, and `show --json` refuses a change with no
    // proposal — so a panel offering "new change from this task" would have created a board row
    // that opens onto an error.
    require_cli();
    let scratch = Scratch::new("propose");
    let os = Openspec::open(&scratch.0).expect("the binary was found");
    os.init().expect("openspec init");

    let change = ChangeName("add-retry-bar".into());
    os.propose(&change, "Add the retry bar", "")
        .expect("propose");

    // Readable at all is the claim: before `propose` wrote a proposal, this call refused.
    let detail = os
        .change(&change)
        .expect("a proposed change is readable at once");
    assert_eq!(detail.name, change);

    // And the document it wrote is one the validator accepts, with the user's own words in it.
    // Read from disk rather than from `show --json`, which carries deltas and no prose at all.
    let proposal =
        std::fs::read_to_string(scratch.0.join("openspec/changes/add-retry-bar/proposal.md"))
            .expect("the proposal was written");
    assert!(
        proposal.contains("\n## Capabilities"),
        "a heading came out indented, which makes it a paragraph: {proposal:?}"
    );
    assert!(
        proposal.contains("Add the retry bar"),
        "the user's own title is not in the proposal: {proposal}"
    );
}

#[test]
#[ignore = "spawns the real openspec CLI"]
fn everything_cide_writes_validates() {
    // The differential test. Anything cide's structured editor produces must be something the
    // real validator accepts — that is the whole promise of letting a panel edit a committed file.
    let (scratch, os, change, spec) = seeded("writes");
    let path = scratch
        .0
        .join("openspec/changes/add-dark-mode/specs/dark-mode/spec.md");

    let h = |n: usize| "#".repeat(n);
    let replacement = format!(
        "{} Requirement: Theme switching\nThe app SHALL switch themes, defaulting to the system \
         setting.\n\n{} Scenario: The user picks dark\n- **WHEN** the user selects the dark \
         theme\n- **THEN** every open editor repaints\n\n{} Scenario: The system decides\n\
         - **WHEN** no theme has been chosen\n- **THEN** the app follows the system\n",
        h(3),
        h(4),
        h(4)
    );

    let outcome = write::set_requirement(
        &os,
        &change,
        &spec,
        DeltaOperation::Added,
        "Theme switching",
        &replacement,
    )
    .expect("the write path ran");
    match outcome {
        SpecWriteOutcome::Written { .. } => {}
        other => panic!("the write did not land: {other:?}"),
    }

    let after = std::fs::read_to_string(&path).expect("readable");
    assert!(
        after.contains("The system decides"),
        "the added scenario is not in the file: {after}"
    );
    let verdict = os.validate(Some(&change)).expect("validate");
    assert!(
        verdict.valid,
        "cide wrote a file the validator refuses: {:?}",
        verdict.issues
    );
}

#[test]
#[ignore = "spawns the real openspec CLI"]
fn a_write_that_would_regress_validation_is_rolled_back() {
    // The after-check is a *subset* rule, so this has to introduce a new problem rather than
    // merely leave an old one. A block with no scenario is refused by the pre-check before it can
    // reach disk — which is the point — so the rollback path is exercised through a block the
    // pre-check passes and the validator does not.
    let (scratch, os, change, spec) = seeded("regress");
    let path = scratch
        .0
        .join("openspec/changes/add-dark-mode/specs/dark-mode/spec.md");
    let before = std::fs::read_to_string(&path).expect("readable");

    let h = |n: usize| "#".repeat(n);
    // A scenario with no WHEN/THEN body at all: shaped like one, empty to the validator.
    let hollow = format!(
        "{} Requirement: Theme switching\nThe app SHALL switch themes.\n\n{} Scenario:\n",
        h(3),
        h(4)
    );
    let outcome = write::set_requirement(
        &os,
        &change,
        &spec,
        DeltaOperation::Added,
        "Theme switching",
        &hollow,
    );

    match outcome {
        // Either door is correct — refused before the write, or written and rolled back — and
        // both must leave the file exactly as it was. That is the property under test.
        Ok(SpecWriteOutcome::Regressed { issues }) => {
            assert!(!issues.is_empty(), "a regression with no issues to show");
        }
        Ok(SpecWriteOutcome::Written { .. }) => {
            let verdict = os.validate(Some(&change)).expect("validate");
            assert!(
                verdict.valid,
                "a hollow scenario was written and the validator refuses it: {:?}",
                verdict.issues
            );
        }
        Ok(other) => panic!("unexpected outcome: {other:?}"),
        Err(_) => {}
    }

    let after = std::fs::read_to_string(&path).expect("readable");
    if !matches!(os.validate(Some(&change)).map(|v| v.valid), Ok(true)) {
        assert_eq!(before, after, "a refused write left the file changed");
    }
}

#[test]
#[ignore = "spawns the real openspec CLI"]
fn a_concurrent_write_is_refused_rather_than_clobbered() {
    // An agent in a worktree edits these same files. Losing that race must be a refusal.
    let (scratch, os, change, spec) = seeded("concurrent");
    let path = scratch
        .0
        .join("openspec/changes/add-dark-mode/specs/dark-mode/spec.md");

    // Make the stamp cide is about to take stale by the time it writes: `set_requirement` reads,
    // validates (a subprocess, so tens of milliseconds), then compares. Writing here from another
    // thread during that window is the real shape of the race.
    let racer = {
        let path = path.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(120));
            let mut text = std::fs::read_to_string(&path).expect("readable");
            text.push_str("\nAn agent wrote this.\n");
            std::fs::write(&path, text).expect("writable");
        })
    };

    let h = |n: usize| "#".repeat(n);
    let replacement = format!(
        "{} Requirement: Theme switching\nThe app SHALL switch themes now.\n\n\
         {} Scenario: The user picks dark\n- **WHEN** a\n- **THEN** b\n",
        h(3),
        h(4)
    );
    let outcome = write::set_requirement(
        &os,
        &change,
        &spec,
        DeltaOperation::Added,
        "Theme switching",
        &replacement,
    );
    racer.join().expect("the racing writer finished");

    match outcome {
        Ok(SpecWriteOutcome::Conflicted { .. }) => {
            let after = std::fs::read_to_string(&path).expect("readable");
            assert!(
                after.contains("An agent wrote this."),
                "the conflict was reported and the other writer's bytes were lost anyway"
            );
        }
        // The race is timing-dependent: if cide's write landed first, the agent's append is on
        // top of it and nothing was clobbered either. What must never happen is cide's write
        // landing *over* the agent's.
        Ok(SpecWriteOutcome::Written { .. }) => {
            let after = std::fs::read_to_string(&path).expect("readable");
            assert!(
                after.contains("An agent wrote this.") || after.contains("switch themes now"),
                "neither writer's bytes survived: {after}"
            );
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
}
