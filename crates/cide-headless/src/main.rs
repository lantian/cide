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
use cide_ipc::{Axis, PaneId, PaneKind, PaneRole, ProjectId, Task, TaskFile, TaskStatus};
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
  cide-headless agents <root>             render its subagent roles and config";

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
        "help" | "-h" | "--help" => emit(&format!("{USAGE}\n")),
        other => {
            eprintln!("cide-headless: unknown subcommand `{other}`");
            usage_and_exit();
        }
    }
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
    let program = args
        .next()
        .cloned()
        .unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()));

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
fn comment_count(task: &Task) -> String {
    match task.comments.len() {
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
    ] {
        rows.push(Row {
            label: format!("  {key}"),
            cells: vec![value],
        });
    }
    render_rows(&rows)
}

/// The `serde` spelling of an isolation mode, which is what the file on disk holds.
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

#[cfg(test)]
mod tests {
    use super::*;
    use cide_agents::{AgentsConfig, CideConfig, LoadedAgent};
    use cide_ipc::workspace::{Pane, ProjectRoot, ToolWindowState};
    use cide_ipc::{
        AgentDef, AgentId, Harness, SessionId, Side, TabId, TaskAuthor, TaskComment, TaskId,
        WindowLabel,
    };

    fn pane(title: &str, kind: PaneKind, role: PaneRole, session: bool) -> Pane {
        Pane {
            id: PaneId::new(),
            kind,
            role,
            session: session.then(SessionId::new),
            conversation: None,
            title: title.into(),
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
    ) -> Task {
        Task {
            id: TaskId(id.into()),
            title: title.into(),
            body: String::new(),
            status,
            agent: agent.map(|a| AgentId(a.into())),
            comments: (0..comments)
                .map(|i| TaskComment {
                    // Derived from the index, not minted: this fixture's output is compared
                    // against a fixed string, and a fresh uuid per run would never match.
                    id: cide_ipc::CommentId(format!("c-{i}")),
                    author: TaskAuthor::User,
                    text: format!("comment {i}"),
                    at_unix_ms: 1,
                    edited_at_unix_ms: None,
                    deleted: false,
                })
                .collect(),
            created_by: TaskAuthor::User,
            created_unix_ms: 1,
            updated_unix_ms: 2,
        }
    }

    fn tracker(tasks: Vec<Task>) -> TaskFile {
        TaskFile {
            schema_version: TaskFile::CURRENT_SCHEMA,
            rev: 7,
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
                description: "does the work".into(),
                system_prompt: "You are a developer.".into(),
                model: None,
                unavailable: unavailable.map(str::to_string),
                max_concurrent: 1,
            },
            origin: PathBuf::from(format!("/p/.cide/agents/{id}.md")),
            shadows: None,
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
