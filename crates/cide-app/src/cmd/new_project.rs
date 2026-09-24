//! The New project wizard's commands. (M97)
//!
//! Three of them, and the wizard in `ui/src/chrome/newProject/` is the only caller:
//!
//! * [`project_new_probe`] answers "what is at this path?" as the user types, so the location
//!   step can say *will be created*, *not empty*, *already a repository* before anything happens.
//! * [`project_pick_location`] is the folder picker, one folder, with the GTK chooser's New
//!   Folder row — the same parented dialog `project_pick` shows, for that command's reason.
//! * [`project_new`] does the work: create the folder, `git init`, set up OpenSpec, switch
//!   subagents on, brief the console, open the project. Each step is announced on
//!   `cide://project-new-progress` so the wizard's checklist moves while the user watches.
//!
//! # The console brief, and why it is not typed after the open
//!
//! On the Tasks road the project's console is handed a first line: draft the milestones, the
//! roles and the first tasks from the user's brief. The console's `claude` is spawned **by the
//! webview**, lazily, when its pane mounts (`TerminalPane` → `session_spawn`), which is some
//! frames after the open's broadcast and on a thread this command does not own. So the line is
//! not typed from here: it is left in [`SEEDS`] under the project's root *before* the open, and
//! `cmd::session::spawn_session` takes it when it spawns that project's primary console. Taking
//! is what makes it once-only — a restart, a respawn or a later reopen of the same folder never
//! replays a brief the product owner already acted on.
//!
//! The one case that road misses is a folder that was **already open** with a live console: no
//! spawn is coming. [`project_new`] checks for that after the open and types the line into the
//! live session itself.

use std::path::{Path, PathBuf};

use cide_core::CoreError;
use cide_ipc::{
    NewProjectKind, NewProjectOutcome, NewProjectProbe, NewProjectProgress, NewProjectRequest,
    NewProjectStep, NewProjectStepState, PaneKind, PaneRole,
};
use parking_lot::Mutex;
use tauri::{AppHandle, Manager as _, State};

use crate::state::SessionRegistry;
use crate::workspace_state::WorkspaceState;

type Result<T> = std::result::Result<T, CoreError>;

/// Console briefs waiting for their console, by canonical project root.
///
/// A `static` for `agent_rpc::NudgeCoalescer`'s reason: the reader is `spawn_session`, on every
/// console spawn in the process, and a seed found through `try_state` would be a brief silently
/// dropped whenever the lookup missed. It is empty for every launch that never ran the wizard,
/// and [`take_seed`] checks that before it touches the filesystem, so the common spawn pays one
/// uncontended lock and nothing else.
static SEEDS: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());

/// The brief left for the console of the project rooted at `root`, removed as it is read.
pub(crate) fn take_seed(root: &Path) -> Option<String> {
    let mut seeds = SEEDS.lock();
    if seeds.is_empty() {
        return None;
    }
    let root = canonical(root);
    let at = seeds.iter().position(|(held, _)| *held == root)?;
    Some(seeds.remove(at).1)
}

fn leave_seed(root: &Path, line: String) {
    let root = canonical(root);
    let mut seeds = SEEDS.lock();
    // A second wizard run on the same folder before the first console came up replaces the
    // first brief rather than queueing behind it: the console gets one opening line, and the
    // later one is what the user last asked for.
    seeds.retain(|(held, _)| *held != root);
    seeds.push((root, line));
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// `~` and `~/…` expanded against `$HOME`; everything else as typed, trimmed.
///
/// Only the bare tilde forms. `~user/` would need a passwd lookup for a spelling nobody types
/// into a New project box, and a path with `$VARS` in it is refused as relative below rather
/// than expanded — a wizard that runs a shell's expansion rules has to run all of them.
fn expand(typed: &str) -> PathBuf {
    let typed = typed.trim();
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty());
    match (typed, home) {
        ("~", Some(home)) => PathBuf::from(home),
        (rest, Some(home)) if rest.starts_with("~/") => PathBuf::from(home).join(&rest[2..]),
        _ => PathBuf::from(typed),
    }
}

/// The nearest ancestor of `path` (or `path` itself) that exists.
fn existing_ancestor(path: &Path) -> Option<&Path> {
    path.ancestors().find(|at| at.exists())
}

/// The probe, as a pure function of the disk and one fact about the workspace.
fn probe_at(path: PathBuf, open_already: bool) -> NewProjectProbe {
    let mut probe = NewProjectProbe {
        path: path.to_string_lossy().into_owned(),
        open_already,
        ..Default::default()
    };
    if path.as_os_str().is_empty() {
        probe.problem = Some("Choose a folder for the project.".into());
        return probe;
    }
    if !path.is_absolute() {
        probe.problem = Some(
            "Type a full path — starting with / or ~ — or choose a folder with Browse.".into(),
        );
        return probe;
    }

    match std::fs::metadata(&path) {
        Ok(meta) => {
            probe.exists = true;
            probe.is_dir = meta.is_dir();
            if !probe.is_dir {
                probe.problem = Some(format!(
                    "{} is a file, not a folder. Choose another name.",
                    path.display()
                ));
                return probe;
            }
            // Capped: the number is only ever shown as "not empty (12 items)", and a folder with
            // a hundred thousand entries on a network mount is not worth a full walk to say so.
            probe.entries = std::fs::read_dir(&path)
                .map(|entries| entries.take(10_000).count() as u32)
                .unwrap_or(0);
            probe.has_git = path.join(".git").exists();
            probe.has_openspec = path.join("openspec").is_dir();
            probe.has_cide = path.join(".cide").is_dir();
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            probe.problem = Some(format!("{} cannot be read: {error}", path.display()));
            return probe;
        }
    }

    let Some(ancestor) = existing_ancestor(&path) else {
        probe.problem = Some(format!("Nothing on the way to {} exists.", path.display()));
        return probe;
    };
    if std::fs::metadata(ancestor).is_ok_and(|meta| meta.permissions().readonly()) {
        probe.problem = Some(format!(
            "{} is read-only, so cide cannot create the project there.",
            ancestor.display()
        ));
        return probe;
    }
    if !probe.has_git
        && let Some(enclosing) = cide_git::repo::discover_root(ancestor)
        && enclosing != canonical(&path)
    {
        probe.inside_repo = Some(enclosing.to_string_lossy().into_owned());
    }
    probe
}

/// Whether a project over `path` is open, by the same raw-path equality `open_project` uses to
/// decide it would activate instead of opening a second copy.
fn is_open(state: &WorkspaceState, path: &Path) -> bool {
    let canonical = canonical(path);
    state.with(|ws| {
        ws.projects.values().any(|project| {
            project
                .roots
                .first()
                .is_some_and(|root| root.path == path || root.path == canonical)
        })
    })
}

/// What is at `path`, for the location step. Read-only; creates nothing.
///
/// `async` and on the blocking pool because it stats, lists a directory, walks up for a
/// repository and — for `openspec_missing` — searches `PATH` and every Node installation
/// directory, any of which can be a network mount.
#[tauri::command(rename_all = "camelCase")]
pub async fn project_new_probe(
    state: State<'_, WorkspaceState>,
    path: String,
) -> Result<NewProjectProbe> {
    let path = expand(&path);
    let open_already = !path.as_os_str().is_empty() && is_open(&state, &path);
    tauri::async_runtime::spawn_blocking(move || {
        let mut probe = probe_at(path, open_already);
        probe.openspec_missing = cide_spec::discover::find().err().map(|why| why.sentence());
        probe
    })
    .await
    .map_err(|error| CoreError::Io(format!("probe: {error}")))
}

/// Ask for one folder to create the project in. `None` means cancelled.
///
/// `project_pick`'s parented picker — see that command for why it is not the dialog plugin's JS
/// `open()` — asked for a single folder. The GTK arm carries the New Folder row, which is what
/// makes "choose where the project goes" and "make the folder for it" one gesture.
#[tauri::command(rename_all = "camelCase")]
pub async fn project_pick_location(
    app: AppHandle,
    window: tauri::WebviewWindow,
) -> Result<Option<String>> {
    let (tx, rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
    crate::cmd::project::show_picker(
        &app,
        window,
        crate::cmd::project::PickerSpec {
            title: "Choose the project folder",
            accept: "Select",
            folders: true,
            multiple: false,
            filter: None,
        },
        tx,
    )?;
    let picked = tauri::async_runtime::spawn_blocking(move || rx.recv().unwrap_or_default())
        .await
        .map_err(|error| CoreError::Io(format!("folder picker: {error}")))?;
    Ok(picked
        .into_iter()
        .next()
        .map(|path| path.to_string_lossy().into_owned()))
}

/// The console's opening line on the Tasks road.
///
/// Written to be read by the product owner, who already has the roster paragraph and the cide
/// tools: it says *what to do first and in what order*, and ends by handing the turn back — the
/// user has not seen a single milestone yet, and dispatching work against a plan nobody agreed
/// to is the outcome the milestone feature exists to prevent. The tool names are the namespaced
/// ones, for `orchestrator_paragraph`'s measured reason.
///
/// # Why it is a numbered checklist, and what each number is paying for
///
/// The first version said "define milestones, create roles, create the first milestone's tasks"
/// and the first real run of it (`~/work/temp/test3`) did exactly that: four milestones, two
/// tasks, both under the first — and three milestones with nothing under them, which the user
/// read, rightly, as a plan that was not finished. So:
///
/// * **Tasks for every milestone**, each `subtaskOf` its milestone's task. `placement` sends a
///   later milestone's tasks to the inbox, and the line says that is expected — without it a
///   model reads "it went to the inbox" as a failure and stops creating them.
/// * **Gate scripts before `define`**, listed in `guardPaths`. A gate command naming a file that
///   does not exist fails for the wrong reason for the whole life of the milestone.
/// * **A first commit.** The same run left a repository with no commits: every subagent's
///   worktree branches from `HEAD`, so none of the roles it had just created could have started.
///   `project_new` does not make that commit itself — libgit2 needs an identity it may not find,
///   and the files worth committing are the ones the console is about to write.
/// * **A check at the end**, against the board rather than the model's memory of it: every task
///   it created is `subtaskOf` a milestone, or it is fixed with `cide_task_link`.
///
/// One line, through `agent_rpc::one_line`, because a newline typed into a PTY is an Enter.
pub(crate) fn brief_line(brief: Option<&str>, has_openspec: bool) -> String {
    let spec = if has_openspec {
        " This project also uses OpenSpec: read openspec/ first, and keep the milestones and \
         tasks in step with its specs."
    } else {
        ""
    };
    let opening = match brief.map(str::trim).filter(|brief| !brief.is_empty()) {
        Some(brief) => format!(
            "This is a new project, just created in cide, and you are its product owner. Here \
             is what I want to build: <<< {brief} >>>{spec} Set the project up in this order and \
             do not skip a step."
        ),
        None => format!(
            "This is a new project, just created in cide, and you are its product owner. It is \
             empty.{spec} First ask me what I want to build, in a few short questions, and wait \
             for my answers. Then set the project up in this order and do not skip a step."
        ),
    };
    let steps = "(1) Survey: call mcp__cide__cide_agents_list and mcp__cide__cide_milestones \
         (action get) to see what already exists. \
         (2) Milestones: plan 2 to 5 of them, in order, each a result a program can check. \
         Write the gate scripts first (for example under tests/gates/), so every gate command \
         exists and exits 0 only when its milestone is really met; then call \
         mcp__cide__cide_milestones with action define, listing those script paths in \
         guardPaths and setting verify to the command every branch must pass before it is \
         merged. define puts one task on the board per milestone; note each milestone's task \
         id (action get shows them again). \
         (3) Roles: create the few roles the work needs with mcp__cide__cide_agent_create — \
         for example an implementer and a reviewer — each with a system prompt that names this \
         project's stack and how to check its own work. \
         (4) Tasks: for EVERY milestone, not only the first, create the tasks that together make \
         its gate pass, with mcp__cide__cide_task_create. Link each one to its milestone's task \
         at creation (links: [{link: subtaskOf, target: <that milestone's task id>}]), give it a \
         body that says what done looks like, and add blockedBy links where order matters. \
         Tasks under the active milestone land in todo; tasks under later milestones land in \
         the inbox until their milestone becomes active — that is expected, so create them \
         anyway. A task that is not subtaskOf a milestone's task is not part of the plan. \
         (5) Commit: make the first git commit (the gate scripts and .cide/). Subagents work \
         in git worktrees branched from it, and with no commit none of them can start. \
         (6) Check: list the board with mcp__cide__cide_task_list, once with status todo and \
         once with status inbox, and confirm that every task you created is subtaskOf a \
         milestone's task; link any that is not with mcp__cide__cide_task_link. \
         Do not assign or dispatch anything. Finish by summarising the milestones, the roles \
         and the tasks under each milestone, and ask me whether to start.";
    crate::agent_rpc::one_line(&format!("{opening} {steps}"))
}

/// Create a project: every step the wizard asked for, in order, then open it.
///
/// # Which failures stop it
///
/// Only the ones that leave nothing to open: the folder could not be created, or the open itself
/// failed. `git init`, OpenSpec and subagents are each reported and skipped past — a project
/// with no `openspec/` because the CLI is missing is still the project the user named, and the
/// panel that sets OpenSpec up is one click away inside it. The outcome lists every failure so
/// the wizard can say which of them happened even if it missed the event.
///
/// Subagents are attempted *after* git, because enabling them is refused on a directory that is
/// not a repository (`cmd::agents::worktree_refusal`) — the wizard forces `git_init` on whenever
/// agents are on, and this order is what makes that sufficient.
#[tauri::command(rename_all = "camelCase")]
pub async fn project_new(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    request: NewProjectRequest,
) -> Result<NewProjectOutcome> {
    let path = expand(&request.path);
    let open_already = is_open(&state, &path);
    let probe = {
        let path = path.clone();
        tauri::async_runtime::spawn_blocking(move || probe_at(path, open_already))
            .await
            .map_err(|error| CoreError::Io(format!("probe: {error}")))?
    };
    if let Some(problem) = probe.problem {
        return Err(CoreError::Io(problem));
    }

    let mut failures = Vec::new();
    let mut report = |step: NewProjectStep, outcome: std::result::Result<(), String>| {
        let progress = match outcome {
            Ok(()) => NewProjectProgress {
                step,
                state: NewProjectStepState::Done,
                detail: None,
            },
            Err(detail) => NewProjectProgress {
                step,
                state: NewProjectStepState::Failed,
                detail: Some(detail),
            },
        };
        crate::emit::project_new_progress(&app, &progress);
        if progress.state == NewProjectStepState::Failed {
            failures.push(progress);
        }
    };
    let start = |step: NewProjectStep| {
        crate::emit::project_new_progress(
            &app,
            &NewProjectProgress {
                step,
                state: NewProjectStepState::Running,
                detail: None,
            },
        );
    };

    let kind = request.kind;
    let agents = request.agents || kind == NewProjectKind::Tasks;

    // Folder. Fatal: there is nothing to do anything else in.
    start(NewProjectStep::Folder);
    let created = {
        let path = path.clone();
        blocking(move || {
            std::fs::create_dir_all(&path).map_err(|error| {
                CoreError::Io(format!("could not create {}: {error}", path.display()))
            })?;
            Ok(canonical(&path))
        })
        .await
    };
    let root = match created {
        Ok(root) => {
            report(NewProjectStep::Folder, Ok(()));
            root
        }
        Err(error) => {
            report(NewProjectStep::Folder, Err(error.to_string()));
            return Err(error);
        }
    };

    if request.git_init || agents {
        start(NewProjectStep::Git);
        let at = root.clone();
        let outcome = blocking(move || {
            cide_git::repo::init(&at)
                .map(|_| ())
                .map_err(|error| CoreError::Io(error.to_string()))
        })
        .await;
        report(NewProjectStep::Git, outcome.map_err(|e| e.to_string()));
    }

    if kind == NewProjectKind::Spec && !root.join("openspec").is_dir() {
        start(NewProjectStep::Openspec);
        let at = root.clone();
        let context = request.spec_context.clone();
        let outcome = blocking(move || crate::cmd::spec::init_at(&at, context.as_deref())).await;
        report(NewProjectStep::Openspec, outcome.map_err(|e| e.to_string()));
    }

    if agents {
        start(NewProjectStep::Agents);
        let at = root.clone();
        let outcome = blocking(move || crate::cmd::agents::enable_at(&at)).await;
        report(NewProjectStep::Agents, outcome.map_err(|e| e.to_string()));
    }

    // Left before the open, so the webview's spawn — which the open's broadcast sets off —
    // cannot run first and find nothing. See the module header.
    let briefing = kind == NewProjectKind::Tasks;
    if briefing {
        leave_seed(
            &root,
            brief_line(request.brief.as_deref(), root.join("openspec").is_dir()),
        );
    }

    start(NewProjectStep::Open);
    let project = match crate::cmd::project::open_project_on_main_thread(
        app.clone(),
        vec![root.clone()],
        None,
    )
    .await
    {
        Ok(project) => {
            report(NewProjectStep::Open, Ok(()));
            project
        }
        Err(error) => {
            // Nothing will spawn this console now; a brief left behind would be typed into the
            // next project opened over the same folder, days later, out of nowhere.
            let _ = take_seed(&root);
            report(NewProjectStep::Open, Err(error.to_string()));
            return Ok(NewProjectOutcome {
                project: None,
                path: root.to_string_lossy().into_owned(),
                failures,
            });
        }
    };

    if briefing {
        start(NewProjectStep::Brief);
        brief_live_console(&app, &state, project, &root);
        // Done means *handed over*: typed into a live console, or waiting for the one the pane
        // is about to spawn. Whether the model then does it is the console's to show.
        report(NewProjectStep::Brief, Ok(()));
    }

    Ok(NewProjectOutcome {
        project: Some(project),
        path: root.to_string_lossy().into_owned(),
        failures,
    })
}

/// Type the waiting brief into the project's console if that console is **already running** —
/// the folder was open before the wizard ran, so no spawn is coming to take it.
///
/// A console that is not running yet is left alone: its spawn takes the seed. If the spawn won
/// the race and took it already, there is nothing here to take and nothing is typed twice.
fn brief_live_console(
    app: &AppHandle,
    state: &WorkspaceState,
    project: cide_ipc::ProjectId,
    root: &Path,
) {
    let Some(registry) = app.try_state::<SessionRegistry>() else {
        return;
    };
    let console = state.with(|ws| {
        cide_core::workspace::project(ws, project)
            .ok()?
            .tabs
            .first()?
            .tree
            .panes
            .values()
            .find(|pane| pane.kind == PaneKind::Claude && pane.role == PaneRole::Primary)?
            .session
    });
    let Some(session) = console else {
        return;
    };
    let Some(pty) = registry.get(session).filter(|pty| !pty.has_exited()) else {
        return;
    };
    let Some(line) = take_seed(root) else {
        return;
    };
    if let Some(bytes) = crate::agent_rpc::submit(&line) {
        crate::agents::type_submitted_line(app, session, &pty, bytes);
    }
}

async fn blocking<T>(work: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T>
where
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| CoreError::Io(format!("the project worker did not finish: {error}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cide-new-project-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_path_that_does_not_exist_is_usable_and_says_nothing_is_there() {
        let dir = scratch("missing").join("deeper");
        let probe = probe_at(dir.clone(), false);
        assert_eq!(probe.problem, None);
        assert!(!probe.exists);
        assert_eq!(probe.entries, 0);
    }

    #[test]
    fn a_relative_or_empty_path_is_refused_with_a_sentence() {
        assert!(
            probe_at(PathBuf::from("projects/x"), false)
                .problem
                .is_some()
        );
        assert!(probe_at(PathBuf::new(), false).problem.is_some());
    }

    #[test]
    fn a_file_is_refused_and_a_full_folder_is_counted() {
        let dir = scratch("full");
        std::fs::create_dir_all(dir.join("openspec")).unwrap();
        std::fs::write(dir.join("README.md"), "hi").unwrap();
        let probe = probe_at(dir.clone(), false);
        assert_eq!(probe.problem, None);
        assert!(probe.is_dir && probe.has_openspec);
        assert_eq!(probe.entries, 2);

        let file = probe_at(dir.join("README.md"), false);
        assert!(file.problem.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tilde_expands_against_home() {
        let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) else {
            return;
        };
        assert_eq!(expand("~"), PathBuf::from(&home));
        assert_eq!(expand(" ~/p/x "), PathBuf::from(&home).join("p/x"));
        assert_eq!(expand("/abs"), PathBuf::from("/abs"));
    }

    #[test]
    fn a_seed_is_taken_once_and_only_for_its_own_root() {
        let dir = scratch("seed");
        std::fs::create_dir_all(&dir).unwrap();
        leave_seed(&dir, "first".into());
        leave_seed(&dir, "second".into());
        assert_eq!(take_seed(&dir.join("elsewhere")), None);
        // The later brief replaced the earlier one, and it comes out exactly once.
        assert_eq!(take_seed(&dir).as_deref(), Some("second"));
        assert_eq!(take_seed(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_brief_is_one_line_and_names_the_tools_it_asks_for() {
        let line = brief_line(Some("a CLI\nthat\r\ntracks  habits"), true);
        assert!(!line.contains('\n') && !line.contains('\r'));
        assert!(line.contains("a CLI that tracks habits"));
        for tool in [
            "mcp__cide__cide_milestones",
            "mcp__cide__cide_agent_create",
            "mcp__cide__cide_task_create",
            "mcp__cide__cide_task_list",
            "mcp__cide__cide_task_link",
        ] {
            assert!(line.contains(tool), "{tool} missing from {line}");
        }
        assert!(line.contains("openspec/"));
        let asking = brief_line(Some("   "), false);
        assert!(asking.contains("ask me what I want to build"));
        assert!(!asking.contains("openspec/"));
    }

    /// The three things the first real run of the brief left out (`~/work/temp/test3`): tasks
    /// under the later milestones, a commit for worktrees to branch from, and a check of the
    /// links against the board. Pinned as phrases, so trimming the line cannot drop one quietly.
    #[test]
    fn the_brief_asks_for_every_milestones_tasks_a_first_commit_and_a_check() {
        let line = brief_line(Some("a site"), false);
        for needle in [
            "for EVERY milestone, not only the first",
            "subtaskOf",
            "land in the inbox until their milestone becomes active",
            "make the first git commit",
            "guardPaths",
            "Do not assign or dispatch anything",
        ] {
            assert!(line.contains(needle), "`{needle}` missing from {line}");
        }
    }
}
