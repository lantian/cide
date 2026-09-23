//! `cide-headless` — run and inspect a cide workspace without a window.
//!
//! Its real job is architectural: this binary links `cide-core`, `cide-ipc`, `cide-pty`,
//! `cide-tasks` and `cide-agents`, and **must never** be able to link `tauri`. If someone puts
//! domain logic in the app crate, this stops building — a cheaper guard than a code-review
//! convention.
//!
//! `run` spawns a program on a PTY and streams it to stdout, which exercises the spawn
//! path, the coalescer and the sink trait. The inspection subcommands render the domain as
//! text: `tree` is how a human checks that a persisted layout is what they think it is,
//! and how M5's restore work gets debugged.
//!
//! `tasks` and `agents` read a project's `.cide/` through the same loaders the panels use, and
//! their second reason to exist is the paragraph above: M18 added `cide-tasks` and `cide-agents`
//! and this binary linked neither, so for one milestone the proof covered less than it claimed
//! to. That is also why neither subcommand prints a value it built itself — a `Debug` of a
//! hand-constructed roster would link a crate and exercise nothing, and the first `AppHandle` to
//! appear in either of those two crates has to be able to break this build.
//!
//! Every renderer here is a pure `&T -> String`, so the tests assert on the exact output
//! rather than on the shape of some intermediate structure.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cide_agents::{AgentProblem, Catalog, Isolation, ProjectAgents, Severity, defs};
use cide_core::layout;
use cide_ipc::keymap::{Command, KeymapLayer, ResolvedBinding};
use cide_ipc::workspace::{LayoutNode, PaneTree, Project, Tab, TabKind, WindowRole, Workspace};
use cide_ipc::{Axis, PaneId, PaneKind, PaneRole, ProjectId, TaskFile, TaskRow, TaskStatus};
use cide_pty::{Geometry, PtySession, Sink, SpawnSpec};

const USAGE: &str = "\
cide-headless — run and inspect a cide workspace without a window

usage:
  cide-headless run [program] [args...]   spawn a program on a PTY, stream it to stdout
  cide-headless tree [--path FILE]        render a persisted workspace as a tree
  cide-headless demo                      render the built-in demo workspace
  cide-headless commands                  list the command registry, grouped
  cide-headless keymap                    list resolved bindings with their layer
  cide-headless tasks <root>              render a project's .cide/tasks.json
  cide-headless agents <root>             render its subagent roles and config
  cide-headless spec <root> [change]      render its openspec/ board, or one change in full
  cide-headless ext                       render marketplaces, extensions and contributions
  cide-headless docker                    resolve a Docker daemon and render what it holds
  cide-headless properties <path>         what the properties card would say about a path
  cide-headless remote                    what a remote listener would bind, and who is paired
  cide-headless version                   print this binary's version";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // A bare invocation used to mean "run $SHELL". Now that the subcommands are explicit,
    // guessing would silently spawn a shell for someone who mistyped `tree`.
    let Some((subcommand, rest)) = args.split_first() else {
        usage_and_exit();
    };

    match subcommand.as_str() {
        "run" => run(rest),
        "tree" => tree(rest),
        "demo" => demo(),
        "commands" => commands(),
        "keymap" => keymap(),
        "tasks" => tasks(rest),
        "agents" => agents(rest),
        "spec" => spec(rest),
        "ext" => ext(),
        "docker" => docker(rest),
        "properties" => properties(rest),
        "remote" => remote(),
        "help" | "-h" | "--help" => emit(&format!("{USAGE}\n")),
        "version" | "-V" | "--version" => emit(&format!("{}\n", version())),
        other => {
            eprintln!("cide-headless: unknown subcommand `{other}`");
            usage_and_exit();
        }
    }
}

/// What the properties card would say about a path, without a window. (M70)
///
/// The card is three commands deep in `cide-app` and every one of its facts is a *claim about a
/// file* — the shape where a wrong answer looks exactly like a right one. This is how a claim is
/// checked against the tool that owns it:
///
/// ```text
/// cide-headless properties src/main.rs   # then compare with `stat` and `git log`
/// ```
///
/// `cide-headless docker` and `cide-headless spec <root>` exist for the same reason, and the
/// reason is the same one: a feature whose failure mode is a plausible sentence needs a way to
/// print that sentence beside the truth.
///
/// # Why there is no git half here
///
/// It would need `cide-git`, and `cide-git` is `git2` with `vendored-openssl` — which is exactly
/// why `docs/platforms.md`'s Darwin type-check runs the workspace excluding `cide-app`,
/// `cide-git` and `xtask`. Linking it here would cost a fourth exclusion and with it this
/// binary's macOS coverage, permanently, to save typing `git log` in the next shell over.
///
/// The trade is easy because the halves are not equally exposed. `cide_git::properties` has
/// eight tests against real repositories and `git log --follow` as a direct oracle beside it.
/// The stat half has neither: its hazard is a `metadata` that silently follows a symlink, and
/// the only external oracle for that is `stat`, run by hand, against this.
fn properties(args: &[String]) {
    let Some(raw) = args.first() else {
        eprintln!("cide-headless: properties needs a path");
        usage_and_exit();
    };
    let path = cide_core::properties::absolutise(std::path::Path::new(raw));

    let props = match cide_core::properties::read(&path) {
        Ok(props) => props,
        Err(e) => {
            eprintln!("cide-headless: {e}");
            std::process::exit(1);
        }
    };

    let mut out = String::new();
    out.push_str(&format!("{}\n", props.path.display()));
    out.push_str(&format!("  name         {}\n", props.name));
    out.push_str(&format!("  kind         {:?}\n", props.kind));
    out.push_str(&format!("  size         {} B\n", props.len));
    out.push_str(&format!("  readonly     {}\n", props.readonly));
    if let Some(mode) = props.mode_string.as_deref() {
        out.push_str(&format!("  permissions  {mode}\n"));
    }
    if let Some(owner) = props.owner.as_ref() {
        out.push_str(&format!(
            "  owner        {}({}) {}({})\n",
            owner.user.as_deref().unwrap_or("?"),
            owner.uid,
            owner.group.as_deref().unwrap_or("?"),
            owner.gid,
        ));
    }
    for (label, at) in [
        ("modified", props.modified_unix_ms),
        ("changed", props.changed_unix_ms),
        ("created", props.created_unix_ms),
    ] {
        // Seconds beside the milliseconds, because that is what `stat %Y` prints and the whole
        // point of this subcommand is being diffable against it.
        match at {
            Some(ms) => out.push_str(&format!("  {label:<12} {ms} ms  ({} s)\n", ms / 1000)),
            None => out.push_str(&format!("  {label:<12} not recorded\n")),
        }
    }
    if let Some(target) = props.symlink_target.as_ref() {
        out.push_str(&format!(
            "  target       {} ({})\n",
            target.display(),
            if props.symlink_broken == Some(true) {
                "broken"
            } else {
                "resolves"
            },
        ));
    }
    match (props.text.as_ref(), props.text_skipped.as_deref()) {
        (Some(text), _) => out.push_str(&format!(
            "  lines        {} · {:?} · {}\n",
            text.lines,
            text.ending,
            if text.utf8 { "UTF-8" } else { "not UTF-8" },
        )),
        (None, Some(why)) => out.push_str(&format!("  lines        {why}\n")),
        (None, None) => {}
    }

    if matches!(props.kind, cide_ipc::PathKind::Dir) {
        match cide_core::properties::dir_summary(&path) {
            Ok(s) => out.push_str(&format!(
                "  entries      {} file(s), {} folder(s), {} B{}\n",
                s.files,
                s.dirs,
                s.bytes,
                if s.truncated {
                    "  (at least — the walk hit its budget)"
                } else {
                    ""
                },
            )),
            Err(e) => out.push_str(&format!("  entries      could not walk: {e}\n")),
        }
    }

    emit(&out);
}

/// `cide-headless 0.7.1-dev`.
///
/// The version is the crate's, and every workspace member inherits `[workspace.package]`'s,
/// so this is the same number `cide --version` prints (`cide_app::cli::version`) and it moves
/// with `scripts/bump-version.sh`. It names *this* binary rather than `cide`: the two are
/// different executables, and a line somebody pastes into a report should say which one
/// answered.
fn version() -> String {
    format!("cide-headless {}", env!("CARGO_PKG_VERSION"))
}

/// Prints usage to stderr and exits 2, the conventional "you invoked me wrongly" code.
fn usage_and_exit() -> ! {
    eprintln!("{USAGE}");
    std::process::exit(2);
}

/// Prints an error and exits 1: the invocation was fine, the work was not.
fn fail(message: impl std::fmt::Display) -> ! {
    eprintln!("cide-headless: {message}");
    std::process::exit(1);
}

/// Writes rendered output to stdout.
///
/// Not `print!`: that panics when the reader goes away, so `cide-headless tree | head` would
/// answer a backtrace and exit 101 rather than the rows it was asked for. A closed pipe is
/// how a pager or `head` ends, and ending with it is the whole response it deserves.
fn emit(text: &str) {
    let mut out = std::io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => std::process::exit(0),
        Err(e) => fail(e),
    }
}

// ---------------------------------------------------------------------------- run

/// Spawns `program` on a PTY and streams its output to stdout until it exits.
fn run(args: &[String]) {
    let mut args = args.iter();
    // The same answer `cmd::session::session_spawn` gives a shell pane — one ladder for the
    // whole workspace, so this binary cannot quietly be more (or less) right than the app.
    let program = args
        .next()
        .cloned()
        .unwrap_or_else(|| cide_core::shell::shell().to_string_lossy().into_owned());

    let cwd = std::env::current_dir().unwrap_or_else(|_| "/".into());
    let mut spec = SpawnSpec::new(&program, cwd)
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .env("TERM_PROGRAM", "cide")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("COLUMNS")
        .env_remove("LINES")
        .geometry(Geometry::new(120, 40, 8, 17));
    for a in args {
        spec = spec.arg(a);
    }

    let session = match PtySession::spawn(spec) {
        Ok(s) => s,
        Err(e) => fail(e),
    };

    let sink: Arc<dyn Sink> = Arc::new(|bytes: &[u8]| {
        let mut out = std::io::stdout().lock();
        out.write_all(bytes).is_ok() && out.flush().is_ok()
    });
    session.attach(sink);

    while !session.has_exited() {
        std::thread::sleep(Duration::from_millis(25));
    }
    // Let the coalescer flush its tail before the process goes away.
    std::thread::sleep(Duration::from_millis(50));
}

// -------------------------------------------------------------------- inspection

/// Renders a persisted workspace: the default one, or the file given to `--path`.
fn tree(args: &[String]) {
    let mut path: Option<PathBuf> = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let value = match arg.as_str() {
            "--path" => args.next().map(String::as_str),
            other => match other.strip_prefix("--path=") {
                Some(v) => Some(v),
                None => {
                    eprintln!("cide-headless: unexpected argument `{other}`");
                    usage_and_exit();
                }
            },
        };
        // An empty value is `--path=` with nothing after it, which is the same mistake as
        // `--path` with nothing after it and deserves the same answer rather than a report
        // that the file named by the empty string does not exist.
        let Some(value) = value.filter(|v| !v.is_empty()) else {
            eprintln!("cide-headless: --path needs a file");
            usage_and_exit();
        };
        path = Some(PathBuf::from(value));
    }

    let path = path.unwrap_or_else(cide_core::persist::workspace_path);
    // Reading the bytes here rather than leaving that to `load` is what keeps looking at a
    // path non-destructive: `load` moves aside anything it cannot read, so handing it a
    // directory or an unreadable file would rename the very thing the user asked about.
    // Rendering an empty tree instead would look like a workspace that lost its projects,
    // which is the one answer this command must never give.
    match std::fs::read_to_string(&path) {
        Ok(raw) if !raw.trim().is_empty() => {}
        Ok(_) => fail(format!("{} is empty", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fail(format!(
            "no workspace file at {} — `cide-headless demo` needs none",
            path.display()
        )),
        Err(e) => fail(format!("{}: {e}", path.display())),
    }

    let workspace = cide_core::persist::load(&path);
    // `load` answers with defaults for a file it cannot parse, having first moved it aside,
    // and it reports that through `tracing`, which this binary installs no subscriber for.
    // The file no longer being there is the observable trace of that happening. Quarantining
    // is best-effort in `persist`, so a file it could neither parse nor move still renders as
    // an empty workspace; only a non-destructive read in `persist` can close that gap.
    if matches!(path.try_exists(), Ok(false)) {
        fail(format!(
            "{} was unreadable and has been moved aside as a `.corrupt-<n>` sibling",
            path.display()
        ));
    }

    emit(&format!("{}\n", path.display()));
    emit(&render_workspace(&workspace));
}

/// Renders the built-in demo workspace, which needs no file on disk.
fn demo() {
    emit(&render_workspace(&cide_core::workspace::demo_workspace()));
}

/// Lists the command registry, grouped as the palette groups it.
fn commands() {
    emit(&render_commands(cide_core::commands::registry()));
}

/// Lists the resolved keymap: every binding with the layer that contributed it.
///
/// The user's overrides are layered in, so this shows what the app would actually run
/// rather than what it ships with.
fn keymap() {
    let path = cide_core::persist::keymap_path();
    let user = match cide_core::keymap::load_user(&path) {
        Ok(user) => user,
        // A broken override file is worth naming, but it must not stop the defaults from
        // being inspectable — those are what the app falls back to as well.
        Err(e) => {
            eprintln!("cide-headless: {}: {e}", path.display());
            Vec::new()
        }
    };
    emit(&render_keymap(&cide_core::keymap::resolve(&user)));
    emit(&render_macos_menu_conflicts());
}

/// The chords that resolve cleanly and can never fire on macOS, appended to every keymap dump.
///
/// Printed on **every** host, not only on a Mac, and that is the point. This is a fact about the
/// keymap that is invisible from the platform it is developed on: `cide_core::keymap` produces a
/// binding, Settings → Keymap lists it, `check:keys` proves it resolves — and AppKit answers the
/// accelerator from `Menu::default` before the web view is ever asked, so the command never runs
/// there. A tool whose whole job is *"inspect the keymap without a window"* should say so.
///
/// It is also what stops `macos_menu_conflicts` being a function only its own test calls, which
/// is this project's signature defect wearing a different hat. Two lines on a Linux dump is a
/// price worth paying for that.
fn render_macos_menu_conflicts() -> String {
    let conflicts = cide_core::keymap::macos_menu_conflicts();
    if conflicts.is_empty() {
        return String::new();
    }
    let mut out = String::from("\nunreachable on macOS (the system menu bar answers first):\n");
    for c in conflicts {
        out.push_str(&format!(
            "  {:<14} {:<24} taken by {}\n",
            c.key, c.command, c.menu_item
        ));
    }
    out
}

// ------------------------------------------------------------------- tasks, agents

/// The project root a `tasks` or `agents` invocation names.
///
/// Required rather than defaulting to the current directory, for the reason `main` refuses a
/// bare invocation: a guessed root would print somebody's shell cwd as though they had asked
/// about it, and both of these subcommands answer "there is nothing here" for a directory that
/// simply is not a project.
fn project_root(args: &[String], subcommand: &str) -> PathBuf {
    let [root] = args else {
        eprintln!("cide-headless: `{subcommand}` takes exactly one project root");
        usage_and_exit();
    };
    // The answer `--path` gives an empty value, for the same reason: `''` is a mistake rather
    // than a directory, and reporting that the project at the empty string has no tracker sends
    // the reader looking at their disk instead of at their command line.
    if root.is_empty() {
        eprintln!("cide-headless: `{subcommand}` needs a project root");
        usage_and_exit();
    }
    // Taken as written, relative included, exactly as `tree --path` takes it: that resolves
    // against the shell's cwd, which is what someone typing `agents .` means.
    let root = PathBuf::from(root);
    // Refused here rather than left to the loaders, because neither of them can refuse it: both
    // are documented never to fail, so `cide_tasks::read` answers a missing directory with
    // `Absent` and `cide_agents::load_project` with an empty roster — and a mistyped path would
    // render as a real project that happens to have no tracker and no roles.
    if !root.is_dir() {
        fail(format!(
            "{} is not a directory — `{subcommand}` takes a project root",
            root.display()
        ));
    }
    root
}

/// Renders a project's task tracker, `<root>/.cide/tasks.json`.
///
/// The tracker is a committed file with several writers — two cide windows and every dispatched
/// agent — and `.cide/` is invisible to cide's own file tree by design, so short of opening it
/// in another editor this is the only way to read it. That is the same job `tree` does for
/// `workspace.json`, one crate over.
fn tasks(args: &[String]) {
    let root = project_root(args, "tasks");
    let path = cide_tasks::tasks_path(&root);

    // `cide_tasks::read`, not `TaskStore::open`, for the reason `tree` reads its own bytes rather
    // than handing the path to `persist::load`: `open` goes through a `load` that moves an
    // unparseable file aside, and looking at a file must never be the thing that renames it.
    // `read` promises the opposite in as many words — it never renames anything and never fails —
    // and it hands back the four states separately, which is exactly what there is to print. A
    // store would also build a debouncer and a write path that only make sense to a caller that
    // is going to mutate, and this is a one-shot that never writes.
    match cide_tasks::read(&path) {
        cide_tasks::ReadOutcome::Ready { file, .. } => {
            emit(&format!("{}\n", path.display()));
            emit(&render_board(&file));
        }
        // Not an error, and not a blank screen either: most projects have never had a tracker,
        // and that has to be a sentence or a reader is left guessing which of "no tasks" and "I
        // could not read it" they are looking at.
        cide_tasks::ReadOutcome::Absent => emit(&format!(
            "{}\nno tracker here — the file is written when the first task is created, and it is \
             committed with the code\n",
            path.display()
        )),
        // The two unreadable shapes exit non-zero: the invocation was fine and the work was not,
        // which is what `fail` is for. `conflicted` is carried through because it is the one
        // distinction that changes what the reader should do next.
        cide_tasks::ReadOutcome::Refused { error } => fail(format!("{}: {error}", path.display())),
        cide_tasks::ReadOutcome::Unparseable { error, conflicted } => fail(format!(
            "{}: {error}{}",
            path.display(),
            if conflicted {
                " — the file still has both sides of a merge in it"
            } else {
                ""
            }
        )),
    }
}

/// Renders a project's OpenSpec board — its capabilities and the changes in flight. (M28)
///
/// The third reason this binary exists, after the no-tauri proof and reading `.cide/`: this one
/// **spawns a subprocess**, and whether it can find `openspec` on the PATH a given launch has is
/// the single most likely thing to be wrong on a user's machine. Reproducing that from a terminal
/// — where the PATH is the shell's, not a desktop launcher's — is how the difference gets
/// established without a GUI in the way.
///
/// It also links `cide-spec`, which is the standing rule this file's header states: a domain
/// crate that cannot be linked without a webview is a domain crate in the wrong place.
fn spec(args: &[String]) {
    // `spec <root>` is the board; `spec <root> <change>` is one change in full. The second form
    // exists because the board and a card are read by *different* CLI calls — `list` for one,
    // `show`/`status`/`instructions`/`validate` for the other — so a project whose board renders
    // and whose card does not is an ordinary state, and this is the only way to see which of the
    // four is the one that refuses.
    let (root_args, change) = match args {
        [root, change] => (std::slice::from_ref(root), Some(change.clone())),
        _ => (args, None),
    };
    let root = project_root(root_args, "spec");

    // cide's own answer, and it has to be: `openspec list --json` in a directory with no
    // `openspec/` exits 0 with an empty list, so the CLI cannot tell "not set up" from "set up
    // and empty". Answered before the binary is even looked for, because a project that does not
    // use OpenSpec is not a machine that is missing a tool.
    if !cide_spec::present(&root) {
        emit(&format!(
            "{}\nno openspec here — `openspec init` creates it, and it is committed with the code\n",
            cide_spec::spec_path(&root).display()
        ));
        return;
    }

    let os = match cide_spec::Openspec::open(&root) {
        Ok(os) => os,
        // The refusal is a sentence naming the install command and the launcher-PATH difference.
        // Printed whole rather than summarised: it is the only thing on screen that can explain
        // why a machine with `openspec` installed reports that it has not.
        Err(error) => fail(error),
    };
    emit(&format!("{}\n", os.binary().display()));
    let Some(change) = change else {
        match os.board() {
            Ok(board) => emit(&render_spec(&board)),
            Err(error) => fail(error),
        }
        return;
    };
    match os.change(&cide_ipc::ChangeName(change)) {
        Ok(detail) => emit(&render_change(&detail)),
        Err(error) => fail(error),
    }
}

/// One change as text: what it edits, how far it has got, and whether it validates.
fn render_change(change: &cide_ipc::SpecChange) -> String {
    let mut out = format!("{}  {}\n", change.name, change.title);
    out.push_str(&format!(
        "  {}/{} task(s)  {}\n",
        change.progress.completed,
        change.progress.total,
        if change.validation.valid {
            "valid".to_string()
        } else {
            format!("{} issue(s)", change.validation.issues.len())
        }
    ));
    for artifact in &change.artifacts {
        out.push_str(&format!(
            "  artifact {}  [{:?}]  {} file(s)\n",
            artifact.id,
            artifact.state,
            artifact.existing.len()
        ));
    }
    for delta in &change.deltas {
        out.push_str(&format!(
            "  delta {} {}  {} requirement(s)\n",
            delta.spec,
            delta.operation.header(),
            delta.requirements.len()
        ));
        for requirement in &delta.requirements {
            // The name is the field the CLI's JSON does not carry — recovered from the file by
            // `cide_spec::block::parse`. An empty one here is the symptom that reads as a
            // nameless card in the panel.
            out.push_str(&format!(
                "    {}  ({} scenario(s))\n",
                if requirement.name.is_empty() {
                    "<no name recovered>"
                } else {
                    &requirement.name
                },
                requirement.scenarios.len()
            ));
        }
    }
    for issue in &change.validation.issues {
        out.push_str(&format!("  {} {}\n", issue.level, issue.message));
    }
    out
}

/// A spec board as text.
///
/// Pure `&T -> String`, this file's rule, so the test below asserts on the exact output rather
/// than on the shape of something in the middle.
fn render_spec(board: &cide_ipc::SpecBoard) -> String {
    match board {
        cide_ipc::SpecBoard::Absent { hint, path } => {
            format!("{}\n{hint}\n", path.display())
        }
        cide_ipc::SpecBoard::Unusable { reason } => format!("{reason}\n"),
        cide_ipc::SpecBoard::Ready {
            changes,
            specs,
            commands,
            ..
        } => {
            let mut out = format!(
                "{} capability(s), {} change(s) in flight\n",
                specs.len(),
                changes.len()
            );
            // Printed because it is the one question a user cannot answer from the panel when it
            // goes wrong: the panel's buttons refused on every project for a release, because
            // OpenSpec moved `/opsx:propose` to `/openspec-propose` and cide had the old spelling
            // compiled in. An empty list here is that failure, visible, with no window running.
            if commands.is_empty() {
                out.push_str(
                    "  no OpenSpec commands for Claude Code — `openspec init --tools claude` \
                     installs them\n",
                );
            } else {
                out.push_str("  commands");
                for command in commands {
                    out.push_str(&format!(" {}", command.line));
                }
                out.push('\n');
            }
            for spec in specs {
                out.push_str(&format!(
                    "  spec   {}  {} requirement(s)\n",
                    spec.id, spec.requirement_count
                ));
            }
            for change in changes {
                // `0/0` is printed as itself and never as "complete": a change whose tracked file
                // exists and holds no checkboxes is not a finished one, which is the rule the
                // Review hop turns on.
                out.push_str(&format!(
                    "  change {}  {}/{} task(s)  [{}]\n",
                    change.name, change.completed_tasks, change.total_tasks, change.status
                ));
            }
            out
        }
    }
}

/// Renders a project's subagent roster and the config that decides whether any of it may run.
///
/// A definition file with a typo in it is the case this exists for. `cide_agents::defs` answers
/// one with an `AgentProblem` rather than a refusal, so the role is still in the roster and the
/// only thing that says why it is greyed is a sentence — which, until now, nothing outside a
/// running window could print.
fn agents(args: &[String]) {
    let root = project_root(args, "agents");
    // Stat'ed before the load, and only for the sentence in the config block below: `config::load`
    // answers with the defaults for a file that is absent *and* for one that will not parse.
    let config_present = matches!(
        cide_agents::config::config_path(&root).try_exists(),
        Ok(true)
    );
    // `load_project` and not a pair of hand-rolled reads: it is the call `cmd/agents.rs` makes,
    // against the real directories and the real `installed` probe, so this prints what the panel
    // would draw rather than a second answer free to drift from it. It is also what puts the real
    // loader — not a constructed value — inside the no-tauri proof.
    let roster = cide_agents::load_project(&root);
    emit(&render_roster(&root, config_present, &roster));
}

/// Renders a task file: a header, then one row per task in file order.
///
/// File order is the panel's order and the order an agent reads, so sorting here — by status,
/// say — would draw a board that nothing else in the product draws.
fn render_board(file: &TaskFile) -> String {
    if file.tasks.is_empty() {
        // A tracker that exists and holds nothing is a real state — every task done and deleted —
        // and it is not the state above. An empty table would leave the two indistinguishable.
        return format!(
            "rev {}  schema {}  no tasks — the file is here and holds nothing, which is not the \
             same as a project that never had one\n",
            file.rev, file.schema_version
        );
    }

    let rows: Vec<Row> = file
        .tasks
        .iter()
        .map(|task| Row {
            label: task.id.to_string(),
            cells: vec![
                status_name(task.status).into(),
                // The role the task is *for*, never who is running it now — see `Task::agent`.
                task.agent
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                task.title.clone(),
                comment_count(task),
            ],
        })
        .collect();

    format!(
        "rev {}  schema {}  {} task(s)\n\n{}",
        file.rev,
        file.schema_version,
        file.tasks.len(),
        render_rows(&rows)
    )
}

/// The `serde` spelling of a status, which is what the file on disk holds.
fn status_name(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Todo => "todo",
        TaskStatus::Doing => "doing",
        TaskStatus::Review => "review",
        TaskStatus::Done => "done",
    }
}

/// `3 comments`, or nothing at all when there are none.
///
/// Blank rather than `0 comments`, the way a pane with no session prints no session column: this
/// column exists so that the tasks carrying a conversation are findable at a glance, and a column
/// of zeroes is precisely what stops that working.
fn comment_count(task: &TaskRow) -> String {
    // The index's own count since M68, not `comments.len()` — this reads `.cide/tasks.json` and
    // nothing else, which is the point of it. A version that opened every task's content file to
    // count would turn `cide-headless tasks` from one read into one per task, and the number it
    // printed would be identical.
    match task.comment_count {
        0 => String::new(),
        1 => "1 comment".to_string(),
        n => format!("{n} comments"),
    }
}

/// The three blocks of `agents`: the roles, the config, and everything wrong with the files.
fn render_roster(root: &Path, config_present: bool, roster: &ProjectAgents) -> String {
    let mut out = render_roles(root, &roster.catalog);
    out.push('\n');
    out.push_str(&render_agents_config(root, config_present, roster));
    out.push('\n');
    out.push_str(&render_problems(&roster.catalog.problems));
    out
}

/// One row per role: name, harness, whether it may be dispatched, and the file it was read from.
fn render_roles(root: &Path, catalog: &Catalog) -> String {
    if catalog.agents.is_empty() {
        // Both directories, because the global one is the half a user forgets they have — and a
        // project with no roles is far more often a directory that is not where they think.
        // `global_dir` reads `XDG_CONFIG_HOME`, which makes this the one renderer here that is
        // not a pure function of its arguments; the sentence is worth nothing without the real
        // path, and the test asserts against the same call.
        return format!(
            "no roles — nothing was read from {} or {}\n",
            defs::project_dir(root).display(),
            defs::global_dir().display()
        );
    }

    let mut rows = vec![Row::plain(format!("{} role(s)", catalog.agents.len()))];
    for agent in &catalog.agents {
        rows.push(Row {
            label: format!("  {}", agent.def.id),
            cells: vec![
                defs::harness_name(agent.def.harness).into(),
                if agent.is_available() {
                    "available".into()
                } else {
                    "unavailable".to_string()
                },
                // The single most useful field for a support question, per its own doc: a role
                // behaving unexpectedly is nearly always a role read from a file the user forgot.
                agent.origin.display().to_string(),
            ],
        });
        // The sentence, on a line of its own and as a structural row, so that its length takes no
        // part in the alignment of every other role. That it is a sentence at all is the whole
        // point of `AgentDef::unavailable` — a greyed row with nothing saying why is the state
        // this project has already paid for two dozen times over.
        if let Some(reason) = &agent.def.unavailable {
            rows.push(Row::plain(format!("    {reason}")));
        }
        // A shadowed definition must never be silent: the user edits the global file, sees nothing
        // change, and has no way to discover that the project's own copy has been winning.
        if let Some(shadowed) = &agent.shadows {
            rows.push(Row::plain(format!("    shadows {}", shadowed.display())));
        }
    }
    render_rows(&rows)
}

/// The `agents` block of `.cide/config.json`, under the path it was read from.
///
/// Keys are spelled as the file spells them rather than prose, because the next thing anyone
/// reading this does is edit that file.
fn render_agents_config(root: &Path, present: bool, roster: &ProjectAgents) -> String {
    // `config::load` answers with the defaults for a file that is absent and for one that will
    // not parse, and reports the difference only through `tracing`, which this binary installs no
    // subscriber for. So the two are separated the one way available from out here — does the
    // file exist — and a file that exists while every value is a default is called out, because
    // that is exactly what an unparseable config looks like from the outside.
    let state = if !present {
        "no file — subagents are off, which is the default"
    } else if roster.config == cide_agents::CideConfig::default() {
        "every value below is a default, which is also how an unparseable file reads"
    } else {
        "read"
    };
    let config = &roster.config.agents;
    let mut rows = vec![Row::plain(format!(
        "config  {}  ({state})",
        cide_agents::config::config_path(root).display()
    ))];
    for (key, value) in [
        ("enabled", roster.enabled().to_string()),
        ("maxConcurrent", config.max_concurrent.to_string()),
        ("harness", defs::harness_name(config.harness).to_string()),
        ("isolation", isolation_name(config.isolation).to_string()),
        (
            "allowDangerousPermissions",
            config.allow_dangerous_permissions.to_string(),
        ),
        ("nudgeOrchestrator", config.nudge_orchestrator.to_string()),
        // Every remaining key of `AgentsConfig`, and the list is now the whole struct on
        // purpose. It had drifted three milestones behind — `autoDispatch` (M30),
        // `skipPermissions` and `stopGraceSecs` (M67) were all settable, all load-bearing and
        // none of them printed, so the one tool for the question *what does this checkout
        // actually say* answered it incompletely. A partial answer here is worse than none:
        // somebody reading it concludes the key is absent from the file.
        ("autoDispatch", config.auto_dispatch.to_string()),
        // The key as the file states it, then what it resolves to. The old `skipPermissions`
        // is printed too, because it is what an unset `permissionMode` falls back to.
        (
            "permissionMode",
            config
                .permission_mode
                .clone()
                .unwrap_or_else(|| "(unset)".to_string()),
        ),
        (
            "skipPermissions",
            config
                .skip_permissions
                .map_or_else(|| "(unset)".to_string(), |skip| skip.to_string()),
        ),
        ("unattended", format!("{:?}", config.unattended())),
        ("stopGraceSecs", config.stop_grace_secs.to_string()),
        ("finishInNewTab", config.finish_in_new_tab.to_string()),
        ("autoSpin", config.auto_spin.to_string()),
        // The **clamped** dwell as well as the stored one when they differ, because the stored
        // number is what the file says and the clamped one is what the timer will use, and a
        // hand-edited `5` in a file is exactly the case somebody runs this to understand.
        (
            "autoSpinAfterSecs",
            match config.spin_after().as_secs() {
                clamped if clamped == u64::from(config.auto_spin_after_secs) => clamped.to_string(),
                clamped => format!("{} (clamped to {clamped})", config.auto_spin_after_secs),
            },
        ),
        (
            "autoSpinAcceptPlan",
            config.auto_spin_accept_plan.to_string(),
        ),
        // The prompt as it will be *sent*, so a blank in the file reads as the shipped default
        // rather than as an empty prompt — `AgentsConfig::spin_prompt`'s decision, shown rather
        // than restated.
        ("autoSpinPrompt", clip_prompt(config.spin_prompt())),
    ] {
        rows.push(Row {
            label: format!("  {key}"),
            cells: vec![value],
        });
    }
    render_rows(&rows)
}

/// The `serde` spelling of an isolation mode, which is what the file on disk holds.
/// A prompt on one line of a terminal table.
///
/// The default is several hundred characters, and a row that wide reflows the whole table into
/// something unreadable. The head is what identifies which prompt this is.
fn clip_prompt(prompt: &str) -> String {
    const BUDGET: usize = 60;
    if prompt.chars().count() <= BUDGET {
        return prompt.to_string();
    }
    // `chars`, not bytes: a byte slice of a UTF-8 prompt panics on a boundary.
    let kept: String = prompt.chars().take(BUDGET).collect();
    format!("{}…", kept.trim_end())
}

fn isolation_name(isolation: Isolation) -> &'static str {
    match isolation {
        Isolation::Worktree => "worktree",
        Isolation::Shared => "shared",
    }
}

/// Every finding, with the file and — when the finding has one — the line to open it at.
fn render_problems(problems: &[AgentProblem]) -> String {
    if problems.is_empty() {
        return "no problems in the definition files\n".to_string();
    }
    let mut rows = vec![Row::plain(format!("{} problem(s)", problems.len()))];
    for problem in problems {
        rows.push(Row {
            // No line is not a missing line: "these two files declare the same name" belongs to
            // neither file's line 4, and inventing one would send the reader to an innocent line.
            label: match problem.line {
                Some(line) => format!("  {}:{line}", problem.path.display()),
                None => format!("  {}", problem.path.display()),
            },
            cells: vec![
                severity_name(problem.severity).into(),
                problem.message.clone(),
            ],
        });
    }
    render_rows(&rows)
}

/// Whether a finding stopped something. An unknown tool warns; an unknown permission mode greys
/// the role, and drawn identically they would be indistinguishable.
fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}

// ----------------------------------------------------------------------- columns

/// One output line: a tree-prefixed label plus columns padded to a width shared by every
/// row in the same block, so pane attributes line up whatever depth their pane sits at.
struct Row {
    label: String,
    cells: Vec<String>,
}

impl Row {
    /// A structural row — a project, a tab, a split, a group heading. Carrying no cells is
    /// what keeps it from stretching the column the attributes start at.
    fn plain(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            cells: Vec::new(),
        }
    }
}

/// Display width of a string, counted the same way `{:<width$}` pads it.
fn width(s: &str) -> usize {
    s.chars().count()
}

/// One row's text with its control characters spelled out.
///
/// Titles reach this renderer from a workspace file and from file names, either of which may
/// hold a newline or a tab on Linux. Passed through, one would break its row in two and let a
/// title forge a column — a rendered `focused` belonging to no pane.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            out.extend(c.escape_debug());
        } else {
            out.push(c);
        }
    }
    out
}

/// Lays rows out into aligned columns, trimming the trailing padding so no line carries
/// whitespace a diff would flag.
fn render_rows(rows: &[Row]) -> String {
    // Escaped before anything is measured rather than on the way out: a control character
    // surviving to the print would make every width computed for its row a lie.
    let rows: Vec<Row> = rows
        .iter()
        .map(|row| Row {
            label: escape(&row.label),
            cells: row.cells.iter().map(|c| escape(c)).collect(),
        })
        .collect();

    let mut label_width = 0;
    let mut cell_widths: Vec<usize> = Vec::new();
    for row in rows.iter().filter(|r| !r.cells.is_empty()) {
        label_width = label_width.max(width(&row.label));
        for (i, cell) in row.cells.iter().enumerate() {
            if cell_widths.len() <= i {
                cell_widths.push(0);
            }
            cell_widths[i] = cell_widths[i].max(width(cell));
        }
    }

    let mut out = String::new();
    for row in rows {
        let mut line = if row.cells.is_empty() {
            row.label.clone()
        } else {
            format!("{:<label_width$}", row.label)
        };
        for (cell, w) in row.cells.iter().zip(&cell_widths) {
            line.push_str("  ");
            line.push_str(&format!("{cell:<w$}"));
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------- workspace tree

/// Renders the whole workspace: a header, one block per project, then the window map.
///
/// Columns are aligned per project rather than globally, so one project with long pane
/// titles does not push every other project's attributes off to the right.
fn render_workspace(ws: &Workspace) -> String {
    let mut out = format!(
        "workspace rev {}  schema {}  {} project(s)\n",
        ws.rev,
        ws.schema_version,
        ws.projects.len()
    );

    for project in ws.projects.values() {
        out.push('\n');
        let mut rows = vec![Row::plain(project_label(project))];
        for (i, tab) in project.tabs.iter().enumerate() {
            push_tab(&mut rows, project, tab, i + 1 == project.tabs.len());
        }
        out.push_str(&render_rows(&rows));
    }

    if !ws.windows.is_empty() {
        out.push('\n');
        out.push_str(&render_windows(ws));
    }
    out
}

/// `cide  ~/work/cide  (2 roots)` — the root count only when there is more than one, since
/// a single-root project is the norm and the suffix would be noise on every line.
fn project_label(project: &Project) -> String {
    let mut label = format!("{}  {}", project.name, project.display_path);
    if project.roots.len() > 1 {
        label.push_str(&format!("  ({} roots)", project.roots.len()));
    }
    label
}

/// Appends a tab row and the rows of its pane tree.
fn push_tab(rows: &mut Vec<Row>, project: &Project, tab: &Tab, is_last: bool) {
    let mut label = format!("{} ▣ {}", connector(is_last), tab.kind.title());
    let mut flags: Vec<String> = Vec::new();
    if !tab.kind.closable() {
        flags.push("[pinned]".into());
    }
    if tab.id == project.active_tab {
        flags.push("[active]".into());
    }
    if let TabKind::File { dirty: true, .. } = tab.kind {
        flags.push("[dirty]".into());
    }
    // The point of this command is catching a layout that will not restore, so a broken
    // tree is reported here rather than left to be inferred from odd-looking rows.
    if let Err(e) = layout::validate(&tab.tree) {
        flags.push(format!("[invalid: {e}]"));
    }
    if !flags.is_empty() {
        label.push_str("  ");
        label.push_str(&flags.join(" "));
    }
    rows.push(Row::plain(label));

    push_node(rows, &tab.tree, &tab.tree.root, spacer(is_last), true);
}

/// Appends the rows for one layout node and, recursively, its children.
fn push_node(rows: &mut Vec<Row>, tree: &PaneTree, node: &LayoutNode, prefix: &str, is_last: bool) {
    match node {
        LayoutNode::Leaf { pane } => rows.push(Row {
            label: format!("{prefix}{} {}", connector(is_last), leaf_label(tree, *pane)),
            cells: leaf_cells(tree, *pane),
        }),
        LayoutNode::Split {
            axis, a, b, ratio, ..
        } => {
            rows.push(Row::plain(format!(
                "{prefix}{} split {} {ratio:.2}",
                connector(is_last),
                axis_name(*axis)
            )));
            let prefix = format!("{prefix}{}", spacer(is_last));
            push_node(rows, tree, a, &prefix, false);
            push_node(rows, tree, b, &prefix, true);
        }
    }
}

/// The elbow drawn in front of a child row.
fn connector(is_last: bool) -> &'static str {
    if is_last { "└─" } else { "├─" }
}

/// What the children of a row are indented by: a trunk unless the row was the last child.
fn spacer(is_last: bool) -> &'static str {
    if is_last { "   " } else { "│  " }
}

/// `1 cide : claude` — the pane's depth-first number and its title.
fn leaf_label(tree: &PaneTree, pane: PaneId) -> String {
    let number = match layout::pane_index(tree, pane) {
        Some(n) => n.to_string(),
        None => "?".into(),
    };
    match tree.panes.get(&pane) {
        Some(p) => format!("{number} {}", p.title),
        // A leaf with no entry in `panes` breaks the tree's central invariant. Rendering it
        // is how the reader finds out; panicking would hide it behind a backtrace.
        None => format!("{number} <no pane {}>", short(pane)),
    }
}

/// The aligned attribute columns of a pane: kind, role, state flags, session.
fn leaf_cells(tree: &PaneTree, pane: PaneId) -> Vec<String> {
    let Some(p) = tree.panes.get(&pane) else {
        return Vec::new();
    };

    let mut flags: Vec<&str> = Vec::new();
    if tree.focused == pane {
        flags.push("focused");
    }
    if tree.maximized == Some(pane) {
        flags.push("maximized");
    }

    vec![
        kind_name(p.kind).into(),
        match p.role {
            PaneRole::Primary => "primary".into(),
            PaneRole::Auxiliary => String::new(),
        },
        flags.join(" "),
        // Eight characters is enough to correlate a pane with a row of `ps`; the rest of
        // the uuid is noise at a glance.
        match p.session {
            Some(s) => format!("session {}", short(s)),
            None => String::new(),
        },
    ]
}

/// The first eight characters of an id, which is what makes a tree scannable.
fn short(id: impl std::fmt::Display) -> String {
    id.to_string().chars().take(8).collect()
}

fn axis_name(axis: Axis) -> &'static str {
    match axis {
        Axis::Row => "row",
        Axis::Col => "col",
    }
}

fn kind_name(kind: PaneKind) -> &'static str {
    match kind {
        PaneKind::Claude => "claude",
        PaneKind::Shell => "shell",
        PaneKind::Diff => "diff",
        PaneKind::Editor => "editor",
    }
}

/// The window map: which OS window is showing what. Restoring a detached pane is the
/// easiest thing to get wrong, so the ids it depends on are printed rather than implied.
fn render_windows(ws: &Workspace) -> String {
    let mut rows = vec![Row::plain("windows")];
    for (label, role) in &ws.windows {
        let (kind, detail) = match role {
            WindowRole::Shell { projects, active } => {
                let names: Vec<String> = projects.iter().map(|p| project_name(ws, *p)).collect();
                let summary = match active {
                    Some(id) => format!(
                        "projects {}  active {}",
                        names.join(", "),
                        project_name(ws, *id)
                    ),
                    // An empty shell is the first-launch frame, not an error worth hiding.
                    None => "no projects open".to_string(),
                };
                ("shell", summary)
            }
            WindowRole::DetachedPane { project, tab, pane } => (
                "detached pane",
                format!(
                    "project {}  tab {}  pane {}",
                    project_name(ws, *project),
                    short(*tab),
                    short(*pane)
                ),
            ),
            WindowRole::DetachedTab { project, tab } => (
                "detached tab",
                format!(
                    "project {}  tab {}",
                    project_name(ws, *project),
                    short(*tab)
                ),
            ),
        };
        rows.push(Row {
            label: format!("  {}", short_label(label.as_str())),
            cells: vec![kind.into(), detail],
        });
    }
    render_rows(&rows)
}

/// `shell:0c1d63d9` — a window label with its uuid cut down to a recognisable prefix.
fn short_label(label: &str) -> String {
    match label.split_once(':') {
        Some((prefix, id)) => format!("{prefix}:{}", short(id)),
        None => label.to_string(),
    }
}

/// A project's name, or a marker when a window points at a project that is not open — a
/// dangling reference is exactly the sort of thing this command exists to surface.
fn project_name(ws: &Workspace, id: ProjectId) -> String {
    match ws.projects.get(&id) {
        Some(p) => p.name.clone(),
        None => format!("<no project {}>", short(id)),
    }
}

// -------------------------------------------------------------- commands, keymap

/// Renders the command registry grouped by palette group, in registry order.
fn render_commands(registry: &[Command]) -> String {
    // Groups keep the order the registry declares them in, which is the order the palette
    // shows: sorting here would misrepresent what a user sees.
    let mut groups: Vec<(&str, Vec<&Command>)> = Vec::new();
    for command in registry {
        match groups.iter().position(|(name, _)| *name == command.group) {
            Some(i) => groups[i].1.push(command),
            None => groups.push((&command.group, vec![command])),
        }
    }

    let mut rows = Vec::new();
    for (name, commands) in &groups {
        rows.push(Row::plain(*name));
        for command in commands {
            rows.push(Row {
                label: format!("  {}", command.id),
                cells: vec![command.title.clone(), when(command.when.as_deref())],
            });
        }
    }
    format!(
        "{}\n{} command(s) in {} group(s)\n",
        render_rows(&rows),
        registry.len(),
        groups.len()
    )
}

/// Renders resolved bindings with the layer each one came from.
fn render_keymap(bindings: &[ResolvedBinding]) -> String {
    let rows: Vec<Row> = bindings
        .iter()
        .map(|b| Row {
            label: b.key.clone(),
            cells: vec![
                b.command.clone(),
                layer_name(b.layer).into(),
                when(b.when.as_deref()),
            ],
        })
        .collect();
    format!("{}\n{} binding(s)\n", render_rows(&rows), bindings.len())
}

/// A `when` clause rendered for a column, blank when the entry is unconditional.
fn when(expr: Option<&str>) -> String {
    match expr {
        Some(e) => format!("when {e}"),
        None => String::new(),
    }
}

fn layer_name(layer: KeymapLayer) -> &'static str {
    match layer {
        KeymapLayer::Default => "default",
        KeymapLayer::Platform => "platform",
        KeymapLayer::User => "user",
    }
}

/// Renders the extension registry: marketplaces, what is installed, and what it all resolves to.
///
/// The one subcommand here that takes no root, because extensions are not a fact about a project —
/// `cide_ext::config`'s header argues that at length. It reads the user's real
/// `$XDG_CONFIG_HOME/cide/extensions.json`, their real clones and their real installed trees, so
/// what it prints is what a window would draw.
///
/// It is also this milestone's half of the no-tauri proof: `ExtStore::new` is the whole read path,
/// and an `AppHandle` reached for anywhere under it stops this binary compiling.
fn ext() {
    let store = cide_ext::ExtStore::new();
    emit(&render_extensions(&store.snapshot()));
}

fn render_extensions(snapshot: &cide_ipc::ext::ExtensionSnapshot) -> String {
    let mut out = format!("rev {}\n", snapshot.rev);

    out.push_str("\nmarketplaces\n");
    if snapshot.marketplaces.is_empty() {
        // Not "none" on its own: a fresh install has no marketplaces and that is the ordinary
        // state, not a fault, so the line says what to do about it rather than reporting a zero.
        out.push_str("  none connected — add one in Settings ▸ Extensions, or by path.\n");
    } else {
        let rows: Vec<Row> = snapshot
            .marketplaces
            .iter()
            .map(|market| Row {
                label: market.id.to_string(),
                cells: vec![
                    match &market.state {
                        cide_ipc::ext::MarketplaceState::Ready { head, .. } => {
                            head.chars().take(8).collect()
                        }
                        cide_ipc::ext::MarketplaceState::Working { what } => what.clone(),
                        cide_ipc::ext::MarketplaceState::Missing => "not cloned".into(),
                        cide_ipc::ext::MarketplaceState::Failed { error } => error.clone(),
                    },
                    if market.authenticated {
                        "remote".into()
                    } else {
                        "local".into()
                    },
                    format!("{} extension(s)", market.entries.len()),
                    market.source.clone(),
                ],
            })
            .collect();
        out.push_str(&render_rows(&rows));
    }

    out.push_str("\ninstalled\n");
    if snapshot.extensions.is_empty() {
        out.push_str("  none\n");
    } else {
        let rows: Vec<Row> = snapshot
            .extensions
            .iter()
            .map(|ext| Row {
                label: ext.id.to_string(),
                cells: vec![
                    ext.version.clone(),
                    if ext.enabled { "on" } else { "off" }.into(),
                    // The greyed reason, which is the whole point of printing this without a
                    // window: an extension that loaded and cannot run says so in a sentence, and
                    // until this existed nothing outside a running window could show it.
                    ext.unavailable.clone().unwrap_or_default(),
                ],
            })
            .collect();
        out.push_str(&render_rows(&rows));
    }

    out.push_str("\nlanguages\n");
    let rows: Vec<Row> = snapshot
        .resolved
        .languages
        .iter()
        .map(|binding| Row {
            label: binding.def.id.clone(),
            cells: vec![
                source_name(&binding.source),
                binding
                    .def
                    .extensions
                    .iter()
                    .map(|e| format!(".{}", e.ext))
                    .collect::<Vec<_>>()
                    .join(" "),
                // What it displaced, which is the answer to "why is my .sql coloured like that".
                binding
                    .supersedes
                    .as_ref()
                    .map(|prior| format!("was {}", source_name(prior)))
                    .unwrap_or_default(),
            ],
        })
        .collect();
    out.push_str(&render_rows(&rows));

    out.push_str("\nlanguage servers\n");
    let rows: Vec<Row> = snapshot
        .resolved
        .servers
        .iter()
        .map(|binding| Row {
            label: binding.def.binary.clone(),
            cells: vec![
                source_name(&binding.source),
                binding.def.language_ids.join(" "),
                binding.def.args.join(" "),
            ],
        })
        .collect();
    out.push_str(&render_rows(&rows));

    let problems: Vec<&cide_ipc::ext::ExtProblem> = snapshot
        .problems
        .iter()
        .chain(snapshot.resolved.conflicts.iter())
        .chain(snapshot.marketplaces.iter().flat_map(|m| m.problems.iter()))
        .chain(snapshot.extensions.iter().flat_map(|e| e.problems.iter()))
        .collect();
    if !problems.is_empty() {
        out.push_str("\nproblems\n");
        for problem in problems {
            // Path and line, always, and in the form an editor's Go to line understands. That is
            // `cide-agents`' rule for a malformed definition and it is the whole reason a refusal
            // is worth printing rather than counting.
            let at = problem
                .line
                .map(|line| format!(":{line}"))
                .unwrap_or_default();
            out.push_str(&format!(
                "  {:?} {}{}: {}\n",
                problem.severity,
                problem.path.display(),
                at,
                problem.message
            ));
        }
    }
    out
}

fn source_name(source: &cide_ipc::ext::ContributionSource) -> String {
    match source {
        cide_ipc::ext::ContributionSource::Builtin => "builtin".into(),
        cide_ipc::ext::ContributionSource::Extension { extension } => extension.to_string(),
    }
}

/// Resolve a Docker daemon and render what it holds. (M41)
///
/// `docker` alone runs the whole ladder; `docker <endpoint>` connects to one directly, which is
/// how the connection switcher's road is exercised without a window.
///
/// # Why the ladder is printed even when it succeeds
///
/// Because the interesting failure is not "no Docker" — it is cide choosing a *different* daemon
/// from the one `docker ps` talks to, which looks like an empty board and reads like a bug in the
/// panel. Printing every context and every socket that was found, with the chosen one marked,
/// turns that into something a user can see in one line. `cide_spec`'s `spec <root>` prints the
/// resolved binary for the same reason.
fn docker(args: &[String]) {
    let endpoint = match args {
        [] => None,
        [value] => match cide_docker::connect::Endpoint::parse(value) {
            Ok(endpoint) => Some(endpoint),
            Err(refusal) => fail(refusal.sentence()),
        },
        _ => {
            eprintln!("cide-headless: `docker` takes at most one endpoint");
            usage_and_exit();
        }
    };

    let probes = cide_docker::connect::probe();
    let mut out = String::new();
    for context in &probes.contexts {
        out.push_str(&format!(
            "context  {}{}  {}\n",
            if context.current { "*" } else { " " },
            context.name,
            context.endpoint.as_url()
        ));
    }
    for socket in &probes.sockets {
        out.push_str(&format!("socket    {}\n", socket.display()));
    }
    if !out.is_empty() {
        out.push('\n');
    }

    out.push_str(&render_docker(&cide_docker::board(endpoint)));
    emit(&out);
}

/// A board as text.
fn render_docker(board: &cide_ipc::docker::DockerBoard) -> String {
    use cide_ipc::docker::DockerBoard;
    match board {
        // Printed whole rather than summarised, `spec`'s reason: the sentence is the only thing
        // on screen that can explain why a machine running Docker reports that it is not.
        DockerBoard::Absent { hint } => format!("{hint}\n"),
        DockerBoard::Unusable {
            reason, endpoint, ..
        } => {
            if endpoint.is_empty() {
                format!("{reason}\n")
            } else {
                format!("{endpoint}\n{reason}\n")
            }
        }
        DockerBoard::Ready(snapshot) => {
            let mut out = format!(
                "{}  {} api {}{}\n\n",
                snapshot.endpoint,
                snapshot.server,
                snapshot.api_version,
                snapshot
                    .context
                    .as_ref()
                    .map(|name| format!("  context {name}"))
                    .unwrap_or_default()
            );

            out.push_str(&format!("{} container(s)\n", snapshot.containers.len()));
            for container in &snapshot.containers {
                let ports: Vec<String> = container
                    .ports
                    .iter()
                    .filter_map(|port| {
                        port.public
                            .map(|public| format!("{public}->{}/{}", port.private, port.protocol))
                    })
                    .collect();
                out.push_str(&format!(
                    "  {:<10} {:<24} {:<28} {}{}{}\n",
                    container.state,
                    truncate(&container.name, 24),
                    truncate(&container.image, 28),
                    container.status,
                    container
                        .health
                        .as_ref()
                        .map(|h| format!("  [{h}]"))
                        .unwrap_or_default(),
                    if ports.is_empty() {
                        String::new()
                    } else {
                        format!("  {}", ports.join(" "))
                    }
                ));
                if let Some(compose) = &container.compose {
                    out.push_str(&format!(
                        "           compose {}/{}\n",
                        compose.project, compose.service
                    ));
                }
            }

            out.push_str(&format!("\n{} image(s)\n", snapshot.images.len()));
            for image in &snapshot.images {
                out.push_str(&format!(
                    "  {:<40} {:>10}  {}\n",
                    // An untagged image is a dangling layer, which is the whole basis of a
                    // prune — so it is named as one rather than left blank.
                    if image.tags.is_empty() {
                        "<dangling>".to_string()
                    } else {
                        truncate(&image.tags.join(" "), 40)
                    },
                    bytes(image.size),
                    &image.id[..image.id.len().min(19)]
                ));
            }
            out
        }
    }
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

fn bytes(size: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// What a launch of the remote listener would resolve to, and who may connect.
///
/// `docker`'s shape and its reason: the answer depends on this machine — its profile, its
/// interfaces, what is already listening — so it cannot be asserted in a test, and a way to
/// print it is the difference between a feature that can be diagnosed and one that can only be
/// guessed at. Diff it against `ip addr` and against the Settings panel.
///
/// Also the standing proof, extended. `cide-headless` links `cide-remote` so that "only
/// `cide-app` may depend on tauri" covers it — this binary's own manifest records the same
/// omission being made for four crates across four milestones, and a crate it does not link is a
/// crate the proof does not cover.
fn remote() {
    let mut out = String::new();
    let profile = cide_core::profile::active();
    out.push_str("instance\n");
    out.push_str(&format!(
        "  profile       {}\n",
        profile.unwrap_or("default")
    ));
    out.push_str(&format!(
        "  name          {}\n",
        cide_core::remote::display_name()
    ));
    out.push_str(&format!(
        "  id            {}\n",
        cide_core::remote::instance_id()
    ));

    let derived = cide_core::remote::port_for_profile(profile);
    let path = cide_core::persist::state_dir().join("remote-devices.json");
    let store = cide_remote::DeviceStore::load(path.clone());

    out.push_str("\nport\n");
    out.push_str(&format!("  derived       {derived}\n"));
    match &store {
        Ok(store) => match store.remembered_port() {
            Some(port) if port != derived => out.push_str(&format!(
                "  remembered    {port}  (preferred — the derived one was taken once)\n"
            )),
            Some(port) => out.push_str(&format!("  remembered    {port}\n")),
            None => out.push_str("  remembered    none yet\n"),
        },
        Err(error) => out.push_str(&format!("  remembered    unreadable: {error}\n")),
    }

    out.push_str("\naddresses a device could use\n");
    let addresses = cide_core::remote::local_addresses();
    if addresses.is_empty() {
        // Not an error. A machine with no non-loopback interface is one nothing can reach, which
        // is worth saying in those words rather than as an empty list.
        out.push_str("  none — this machine has no address a phone could reach it on\n");
    }
    for address in &addresses {
        let verdict = cide_core::remote::bind_policy(*address);
        let note = match verdict {
            cide_core::remote::BindVerdict::Private => "private",
            cide_core::remote::BindVerdict::Unspecified => "unspecified",
            cide_core::remote::BindVerdict::Public => "PUBLIC — needs the separate toggle",
        };
        let shown = if address.is_ipv6() {
            format!("[{address}]")
        } else {
            address.to_string()
        };
        out.push_str(&format!("  {shown}:{derived}  ({note})\n"));
    }

    out.push_str("\npaired devices\n");
    match &store {
        Err(error) => out.push_str(&format!("  unreadable: {error}\n")),
        Ok(store) => {
            let devices = store.devices();
            if devices.is_empty() {
                out.push_str("  none — with nobody to serve, nothing is listening\n");
            }
            for device in devices {
                // No token and no hash, here as everywhere. There is nothing to redact because
                // `Device`'s `Debug` and this renderer both take the same view of what a device
                // *is* to anybody reading a screen.
                let seen = if device.last_seen_unix_ms == 0 {
                    "never connected".to_owned()
                } else {
                    format!("last seen {} ms", device.last_seen_unix_ms)
                };
                out.push_str(&format!(
                    "  {}  ({}) — {seen}{}\n",
                    device.name,
                    device.platform,
                    if device.last_addr.is_empty() {
                        String::new()
                    } else {
                        format!(" from {}", device.last_addr)
                    },
                ));
            }
        }
    }
    out.push_str(&format!("\nfile  {}\n", path.display()));
    emit(&out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_agents::{AgentsConfig, CideConfig, LoadedAgent};
    use cide_ipc::workspace::{Pane, ProjectRoot, ToolWindowState};
    use cide_ipc::{
        AgentDef, AgentId, Harness, SessionId, Side, TabId, TaskAuthor, TaskId, WindowLabel,
    };

    fn pane(title: &str, kind: PaneKind, role: PaneRole, session: bool) -> Pane {
        Pane {
            id: PaneId::new(),
            kind,
            role,
            session: session.then(SessionId::new),
            conversation: None,
            conversation_since: None,
            continues: None,
            title: title.into(),
            docker: None,
        }
    }

    /// Two tabs, one of them split twice: the shape the renderer has to get right — leaves
    /// at differing depths, a pane with no session, a primary alongside auxiliaries.
    fn fixture() -> Workspace {
        let first = pane("cide : claude", PaneKind::Claude, PaneRole::Primary, true);
        let second = pane("cide : bash", PaneKind::Shell, PaneRole::Auxiliary, true);
        let third = pane(
            "cide : claude — diff",
            PaneKind::Diff,
            PaneRole::Auxiliary,
            false,
        );
        let (first_id, second_id) = (first.id, second.id);

        let mut tree = layout::new_tree(first);
        layout::split(&mut tree, first_id, Axis::Row, Side::After, second).unwrap();
        layout::split(&mut tree, second_id, Axis::Col, Side::After, third).unwrap();

        let home = Tab {
            id: TabId::new(),
            kind: TabKind::ClaudeHome,
            tree,
        };
        let editor = Tab {
            id: TabId::new(),
            kind: TabKind::File {
                path: "/home/u/work/cide/lib.rs".into(),
                dirty: true,
            },
            tree: layout::new_tree(pane("lib.rs", PaneKind::Editor, PaneRole::Auxiliary, false)),
        };

        let project = Project {
            id: ProjectId::new(),
            name: "cide".into(),
            display_path: "~/work/cide".into(),
            dot: "var(--accent)".into(),
            roots: vec![ProjectRoot {
                path: "/home/u/work/cide".into(),
                label: "cide".into(),
            }],
            active_tab: home.id,
            tab_mru: vec![home.id],
            tabs: vec![home, editor],
            detached: Default::default(),
            dock_anchors: Default::default(),
            tool_window: ToolWindowState::default(),
            primary_session: SessionId::new(),
        };

        let mut ws = Workspace::default();
        ws.projects.insert(project.id, project);
        ws
    }

    /// The sole project of a fixture workspace.
    fn project(ws: &mut Workspace) -> &mut Project {
        ws.projects.values_mut().next().unwrap()
    }

    fn body(ws: &Workspace) -> Vec<String> {
        render_workspace(ws).lines().map(str::to_string).collect()
    }

    fn line_with<'a>(lines: &'a [String], needle: &str) -> &'a str {
        lines
            .iter()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no line containing {needle:?} in {lines:#?}"))
    }

    /// The column `needle` starts at, counted in characters. Box-drawing prefixes are
    /// multi-byte, so byte offsets differ between depths even when the columns line up.
    fn column_of(line: &str, needle: &str) -> usize {
        let byte = line
            .find(needle)
            .unwrap_or_else(|| panic!("no {needle:?} in {line:?}"));
        line[..byte].chars().count()
    }

    #[test]
    fn version_names_this_binary_and_the_crates_version() {
        let v = version();
        assert!(v.starts_with("cide-headless "), "{v}");
        assert!(v.ends_with(env!("CARGO_PKG_VERSION")), "{v}");
    }

    #[test]
    fn every_pane_is_numbered_by_its_depth_first_position() {
        let lines = body(&fixture());
        assert!(line_with(&lines, "cide : bash").contains("2 cide : bash"));
        assert!(line_with(&lines, "— diff").contains("3 cide : claude — diff"));
        // Each tab numbers its own panes, so the file tab starts again at one.
        assert!(line_with(&lines, "1 lib.rs").contains("1 lib.rs"));
    }

    #[test]
    fn splits_show_their_axis_and_ratio() {
        let mut ws = fixture();
        let tree = &mut project(&mut ws).tabs[0].tree;
        let LayoutNode::Split { id, .. } = tree.root else {
            panic!("the fixture splits twice, so its root is a split");
        };
        layout::set_ratio(tree, id, 0.57).unwrap();
        let lines = body(&ws);
        assert!(line_with(&lines, "split row").contains("split row 0.57"));
        assert!(line_with(&lines, "split col").contains("split col 0.5"));
    }

    #[test]
    fn pane_attributes_line_up_across_differing_tree_depths() {
        let lines = body(&fixture());
        let deep = column_of(line_with(&lines, "cide : bash"), "shell");
        let shallow = column_of(line_with(&lines, "1 lib.rs"), "editor");
        assert_eq!(deep, shallow);
    }

    #[test]
    fn a_long_structural_row_does_not_shift_the_attribute_columns() {
        let ws = fixture();
        let before = column_of(line_with(&body(&ws), "1 lib.rs"), "editor");

        let mut ws = ws;
        // A tab row far wider than any pane label. It carries no attributes, so it must
        // take no part in the alignment.
        project(&mut ws).tabs[1].kind = TabKind::File {
            path: "/home/u/work/cide/a_very_long_file_name_indeed.rs".into(),
            dirty: false,
        };
        assert_eq!(
            before,
            column_of(line_with(&body(&ws), "1 lib.rs"), "editor")
        );
    }

    #[test]
    fn the_pinned_tab_is_marked_and_a_closable_one_is_not() {
        let lines = body(&fixture());
        assert!(line_with(&lines, "▣ Claude").contains("[pinned]"));
        assert!(line_with(&lines, "▣ Claude").contains("[active]"));
        assert!(!line_with(&lines, "▣ lib.rs").contains("[pinned]"));
        assert!(line_with(&lines, "▣ lib.rs").contains("[dirty]"));
    }

    #[test]
    fn a_second_root_is_reported_on_the_project_row() {
        let mut ws = fixture();
        assert!(!line_with(&body(&ws), "~/work/cide").contains("roots"));

        project(&mut ws).roots.push(ProjectRoot {
            path: "/home/u/work/other".into(),
            label: "other".into(),
        });
        assert!(line_with(&body(&ws), "~/work/cide").contains("(2 roots)"));
    }

    #[test]
    fn session_ids_are_truncated_to_eight_characters() {
        let ws = fixture();
        let session = ws.projects.values().next().unwrap().tabs[0]
            .tree
            .panes
            .values()
            .find_map(|p| p.session)
            .unwrap()
            .to_string();
        let rendered = line_with(&body(&ws), "session ").to_string();
        assert!(rendered.contains(&session[..8]));
        assert!(!rendered.contains(&session));
    }

    #[test]
    fn a_pane_with_no_session_prints_no_session_column() {
        let lines = body(&fixture());
        assert!(!line_with(&lines, "— diff").contains("session"));
    }

    #[test]
    fn the_focused_pane_is_marked() {
        let mut ws = fixture();
        let tree = &mut project(&mut ws).tabs[0].tree;
        let target = *tree.panes.keys().nth(1).unwrap();
        layout::focus(tree, target).unwrap();
        let lines = body(&ws);
        assert!(line_with(&lines, "cide : bash").contains("focused"));
        assert!(!line_with(&lines, "— diff").contains("focused"));
    }

    #[test]
    fn a_leaf_whose_pane_is_missing_renders_instead_of_panicking() {
        let mut ws = fixture();
        let tree = &mut project(&mut ws).tabs[0].tree;
        let orphan = *tree.panes.keys().next().unwrap();
        tree.panes.shift_remove(&orphan);
        let lines = body(&ws);
        assert!(line_with(&lines, "<no pane").contains(&short(orphan)));
    }

    #[test]
    fn a_tree_that_would_not_restore_is_flagged_on_its_tab_row() {
        let mut ws = fixture();
        let tree = &mut project(&mut ws).tabs[0].tree;
        let orphan = *tree.panes.keys().next().unwrap();
        tree.panes.shift_remove(&orphan);
        assert!(line_with(&body(&ws), "▣ Claude").contains("[invalid:"));
    }

    #[test]
    fn a_window_pointing_at_a_closed_project_is_called_out() {
        let mut ws = fixture();
        let missing = ProjectId::new();
        ws.windows.insert(
            WindowLabel("shell:0c1d63d9-0be4-434e-be66-f64731309eab".into()),
            WindowRole::Shell {
                projects: vec![missing],
                active: Some(missing),
            },
        );
        let lines = body(&ws);
        // The uuid is cut down, and the dangling reference is named rather than dropped.
        assert!(line_with(&lines, "shell:0c1d63d9").contains("<no project"));
        assert!(!line_with(&lines, "shell:0c1d63d9").contains("f64731309eab"));
    }

    #[test]
    fn a_control_character_in_a_title_cannot_forge_a_row() {
        let mut ws = fixture();
        let tree = &mut project(&mut ws).tabs[1].tree;
        let id = *tree.panes.keys().next().unwrap();
        // A title that ends its own line and opens one carrying attributes no pane has.
        tree.panes[&id].title = "lib.rs\n 9 forged  claude  primary  focused".into();

        let lines = body(&ws);
        assert!(line_with(&lines, "forged").contains("1 lib.rs\\n 9 forged"));
        assert!(
            !lines.iter().any(|l| l.trim_start().starts_with("9 forged")),
            "a title must not be able to print a row of its own: {lines:#?}"
        );
    }

    #[test]
    fn no_rendered_line_carries_trailing_whitespace() {
        for line in body(&fixture()) {
            assert_eq!(line.trim_end(), line, "trailing space in {line:?}");
        }
    }

    #[test]
    fn commands_are_grouped_in_registry_order() {
        let registry = vec![
            Command::new("pane.split.right", "Split pane right", "Window"),
            Command::new("claude.new", "New Claude session", "Claude"),
            Command::new("pane.close", "Close pane", "Window").when("paneFocused"),
        ];
        let out = render_commands(&registry);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "Window");
        // Both Window commands sit under the one heading, and Claude follows them.
        assert!(lines[1].starts_with("  pane.split.right"));
        assert!(lines[2].starts_with("  pane.close"));
        assert!(lines[2].contains("when paneFocused"));
        assert_eq!(lines[3], "Claude");
        assert!(out.contains("3 command(s) in 2 group(s)"));
    }

    #[test]
    fn a_binding_shows_the_layer_that_contributed_it() {
        let bindings = vec![
            ResolvedBinding {
                key: "ctrl+p".into(),
                command: "picker.files".into(),
                when: None,
                args: None,
                layer: KeymapLayer::Default,
            },
            ResolvedBinding {
                key: "ctrl+k ctrl+s".into(),
                command: "settings.keymap".into(),
                when: Some("overlayOpen".into()),
                args: None,
                layer: KeymapLayer::User,
            },
        ];
        let out = render_keymap(&bindings);
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].contains("picker.files") && lines[0].contains("default"));
        assert!(lines[1].contains("user") && lines[1].contains("when overlayOpen"));
        // The command column is aligned even though the keys differ in length.
        assert_eq!(
            column_of(lines[0], "picker.files"),
            column_of(lines[1], "settings.keymap")
        );
        assert!(out.contains("2 binding(s)"));
    }

    // ------------------------------------------------------------- tasks and roles

    fn text_lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    fn task(
        id: &str,
        title: &str,
        status: TaskStatus,
        agent: Option<&str>,
        comments: usize,
    ) -> TaskRow {
        // A **row**, since M68: `cide-headless tasks` reads `.cide/tasks.json` and nothing else, so
        // the comment column comes from the index's own count rather than from a log this command
        // deliberately never opens. The parameter stays a `usize` of comments because that is what
        // the column is about; it lands as the count the index carries.
        TaskRow {
            id: TaskId(id.into()),
            title: title.into(),
            status,
            agent: agent.map(|a| AgentId(a.into())),
            change: None,
            links: Vec::new(),
            session: None,
            created_by: TaskAuthor::User,
            created_unix_ms: 1,
            updated_unix_ms: 2,
            comment_count: u32::try_from(comments).expect("a fixture's comment count"),
            attachment_count: 0,
        }
    }

    fn tracker(tasks: Vec<TaskRow>) -> TaskFile {
        TaskFile {
            schema_version: TaskFile::CURRENT_SCHEMA,
            rev: 7,
            next_id: 100,
            tasks,
        }
    }

    #[test]
    fn a_task_row_carries_its_status_assignee_title_and_comment_count() {
        let out = render_board(&tracker(vec![
            task(
                "t-1",
                "Wire the dispatch queue",
                TaskStatus::Doing,
                Some("developer"),
                3,
            ),
            task("t-2", "Write the panel tests", TaskStatus::Todo, None, 0),
        ]));
        let lines = text_lines(&out);
        assert!(lines[0].contains("rev 7") && lines[0].contains("2 task(s)"));

        let doing = line_with(&lines, "t-1");
        assert!(doing.contains("doing"));
        assert!(doing.contains("developer"));
        assert!(doing.contains("Wire the dispatch queue"));
        assert!(doing.contains("3 comments"));

        // An unassigned task prints no role and a task with no conversation prints no count —
        // both blank rather than a placeholder, so the rows that have one stand out.
        let todo = line_with(&lines, "t-2");
        assert!(!todo.contains("developer"));
        assert!(!todo.contains("comment"));
    }

    #[test]
    fn a_tracker_with_no_tasks_says_so_rather_than_printing_a_blank() {
        let out = render_board(&tracker(Vec::new()));
        assert_eq!(out.lines().count(), 1);
        assert!(out.contains("no tasks"));
        // And says which of the two empty states it is.
        assert!(out.contains("never had one"));
    }

    #[test]
    fn a_task_title_cannot_forge_a_row() {
        // Task titles are model-authored — an agent writes them through `cide_task_create` — so
        // this is the same defence `escape` gives a pane title, against a writer with more reason
        // to produce a newline.
        let out = render_board(&tracker(vec![task(
            "t-1",
            "Ship it\nt-9  done  developer  Already finished",
            TaskStatus::Todo,
            None,
            0,
        )]));
        let lines = text_lines(&out);
        assert!(line_with(&lines, "t-9").contains("Ship it\\nt-9"));
        assert!(
            !lines.iter().any(|l| l.starts_with("t-9")),
            "a title must not be able to print a row of its own: {lines:#?}"
        );
    }

    fn role(id: &str, unavailable: Option<&str>) -> LoadedAgent {
        LoadedAgent {
            def: AgentDef {
                id: AgentId(id.into()),
                label: defs::label_from_id(id),
                harness: Harness::Claude,
                scope: cide_ipc::agents::AgentScope::Project,
                description: "does the work".into(),
                system_prompt: "You are a developer.".into(),
                model: None,
                color: None,
                unavailable: unavailable.map(str::to_string),
                max_concurrent: 1,
                worktree: true,
            },
            origin: PathBuf::from(format!("/p/.cide/agents/{id}.md")),
            shadows: None,
            extras: Vec::new(),
            tools: Vec::new(),
            permission_mode: None,
            effort: None,
        }
    }

    fn catalog(agents: Vec<LoadedAgent>) -> Catalog {
        Catalog {
            agents,
            problems: Vec::new(),
        }
    }

    fn roster(config: CideConfig) -> ProjectAgents {
        ProjectAgents {
            config,
            catalog: Catalog::default(),
        }
    }

    #[test]
    fn a_role_shows_its_harness_its_file_and_whether_it_can_be_dispatched() {
        let out = render_roles(
            Path::new("/p"),
            &catalog(vec![
                role("developer", None),
                role("qa", Some("no “claude” on PATH")),
            ]),
        );
        let lines = text_lines(&out);
        assert!(lines[0].contains("2 role(s)"));

        let ok = line_with(&lines, "developer");
        assert!(ok.contains("claude"));
        assert!(ok.contains("available"));
        assert!(ok.contains("/p/.cide/agents/developer.md"));

        let greyed = line_with(&lines, "  qa ");
        assert!(greyed.contains("unavailable"));
        // The sentence is what makes a greyed row actionable, and it gets its own line.
        assert!(lines.iter().any(|l| l.trim() == "no “claude” on PATH"));
    }

    #[test]
    fn a_long_unavailable_sentence_does_not_shift_the_role_columns() {
        let short = render_roles(
            Path::new("/p"),
            &catalog(vec![role("developer", None), role("qa", None)]),
        );
        let long = render_roles(
            Path::new("/p"),
            &catalog(vec![
                role("developer", None),
                role("qa", Some(&"x".repeat(200))),
            ]),
        );
        let (short, long) = (text_lines(&short), text_lines(&long));
        assert_eq!(
            column_of(line_with(&short, "developer"), "claude"),
            column_of(line_with(&long, "developer"), "claude")
        );
    }

    #[test]
    fn a_shadowed_definition_is_never_silent() {
        let mut agent = role("developer", None);
        agent.shadows = Some(PathBuf::from("/home/u/.config/cide/agents/developer.md"));
        let out = render_roles(Path::new("/p"), &catalog(vec![agent]));
        assert!(out.contains("shadows /home/u/.config/cide/agents/developer.md"));
    }

    #[test]
    fn an_empty_roster_names_both_directories_it_read() {
        let out = render_roles(Path::new("/p"), &catalog(Vec::new()));
        assert!(out.contains("/p/.cide/agents"));
        assert!(out.contains(&defs::global_dir().display().to_string()));
    }

    #[test]
    fn the_config_block_tells_an_absent_file_from_one_that_reads_as_defaults() {
        let root = Path::new("/p");

        let absent = render_agents_config(root, false, &roster(CideConfig::default()));
        assert!(absent.contains("/p/.cide/config.json"));
        assert!(absent.contains("no file"));
        assert!(line_with(&text_lines(&absent), "enabled").contains("false"));

        // The file is there and every value is a default, which is also what a file that would
        // not parse looks like from out here. Saying so is the only honest answer available.
        let defaults = render_agents_config(root, true, &roster(CideConfig::default()));
        assert!(defaults.contains("unparseable"));

        let config = CideConfig {
            agents: AgentsConfig {
                enabled: true,
                ..AgentsConfig::default()
            },
            ..CideConfig::default()
        };
        let read = render_agents_config(root, true, &roster(config));
        assert!(!read.contains("unparseable"));
        assert!(line_with(&text_lines(&read), "enabled").contains("true"));
    }

    #[test]
    fn a_problem_carries_its_file_and_the_line_to_open_it_at() {
        let out = render_problems(&[
            AgentProblem {
                path: "/p/.cide/agents/qa.md".into(),
                line: Some(4),
                severity: Severity::Error,
                message: "`permission-mode: yolo` is not a mode".into(),
            },
            AgentProblem {
                path: "/p/.cide/agents".into(),
                line: None,
                severity: Severity::Warning,
                message: "this directory could not be read".into(),
            },
        ]);
        let lines = text_lines(&out);
        assert!(lines[0].contains("2 problem(s)"));

        let typo = line_with(&lines, "qa.md");
        assert!(typo.contains("qa.md:4"));
        assert!(typo.contains("error"));

        // A finding about a whole directory has no line, and none is invented for it.
        let dir = line_with(&lines, "could not be read");
        assert!(dir.contains("warning"));
        assert!(!dir.contains(":4"));
    }

    #[test]
    fn a_roster_with_nothing_wrong_with_it_says_that_too() {
        assert!(render_problems(&[]).contains("no problems"));
    }

    #[test]
    fn the_roster_prints_roles_then_config_then_problems() {
        let out = render_roster(
            Path::new("/p"),
            false,
            &ProjectAgents {
                config: CideConfig::default(),
                catalog: catalog(vec![role("developer", None)]),
            },
        );
        let roles = out.find("role(s)").expect("a roles block");
        let config = out.find("config  ").expect("a config block");
        let problems = out.find("no problems").expect("a problems block");
        assert!(roles < config && config < problems, "{out}");

        for line in out.lines() {
            assert_eq!(line.trim_end(), line, "trailing space in {line:?}");
        }
    }
}
