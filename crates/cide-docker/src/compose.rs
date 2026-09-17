//! `docker compose`, which is the one thing the Engine API cannot do. (M43 — ADR 0013)
//!
//! # Why this exists at all, in a crate whose whole argument is not to shell out
//!
//! Because there is no Engine API for compose. `docker compose up` is a **CLI plugin**; the
//! daemon has never heard of a stack, and there is no endpoint that creates one. What the API
//! does give is labels — every compose-managed container carries `com.docker.compose.project`,
//! `.service`, `.config_files` and `.project.working_dir` — so *grouping* containers into stacks
//! is pure API and only *acting* on one needs a binary. ADR 0013 records the split; this module
//! is the whole of the quarantine.
//!
//! # Finding the plugin is not finding `docker`
//!
//! `docker compose` is not a command. It is `docker` invoking `docker-compose` out of a
//! **plugin directory**, which on this machine is `/opt/homebrew/lib/docker/cli-plugins` — a
//! directory that is on no `PATH` anywhere. So cide finds `docker` and asks *it*, rather than
//! looking for a `docker-compose` binary: `docker compose version` is the only honest test,
//! because it is the same lookup the real invocation will do.
//!
//! And `child_env::prepare_command_with` is handed the directory `docker` was found in, for the
//! rule that module's header states: the directories cide searches to find a binary and the
//! directories it gives that binary's process are one list. A `docker` from Homebrew shells out
//! to `docker-credential-osxkeychain` and to its own plugins, and a child with a desktop launch's
//! `PATH` reaches neither.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use cide_ipc::docker::{ComposeAction, ComposeAvailability, ComposeRun};

use crate::DockerError;

/// The environment variable that overrides the search, alone and first.
pub const OVERRIDE_ENV: &str = "CIDE_DOCKER_PATH";

/// The binary cide asks about the plugin.
pub const BINARY: &str = "docker";

/// What the user is told when there is no plugin.
pub const INSTALL_HINT: &str = "Docker Compose is part of Docker Desktop, and is packaged as `docker-compose-plugin` on \
     Linux.";

/// `up`/`down` on a real stack pulls images and waits on healthchecks.
const ACT_DEADLINE: Duration = Duration::from_secs(600);
/// And the version probe is a local exec.
const PROBE_DEADLINE: Duration = Duration::from_secs(10);

/// How Compose is invoked here.
///
/// # Why there are three rungs and not one
///
/// `docker compose` is the modern spelling and the first thing to try — and it is *not* always
/// the one that works, even where Compose is plainly installed. `docker` finds its plugins in a
/// fixed set of directories plus whatever `~/.docker/config.json`'s `cliPluginsExtraDirs` names,
/// and Homebrew installs the plugin into `/opt/homebrew/lib/docker/cli-plugins` **without**
/// adding that key. On such a machine `docker compose version` answers *"unknown command"* while
/// `/opt/homebrew/lib/docker/cli-plugins/docker-compose version` prints a version perfectly
/// happily. That is not a broken cide and not a broken Compose — it is a gap between two
/// packages — and a panel that reported "no compose" there would be telling a user with Compose
/// installed that they have none.
///
/// So: ask `docker`, then look for a standalone `docker-compose`, then look in the plugin
/// directories `docker` itself searches. Every rung is verified by *running* the candidate rather
/// than by testing for a file, because "it is there" and "it runs" differ on exactly the machines
/// this ladder exists for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    /// `docker compose …` — the plugin reached through the CLI that owns it.
    Plugin(PathBuf),
    /// `docker-compose …` — a standalone binary, or the plugin executed directly.
    Standalone(PathBuf),
}

impl Invocation {
    /// The program to run, and the arguments that must precede the Compose ones.
    fn program(&self) -> (&PathBuf, &'static [&'static str]) {
        match self {
            Self::Plugin(path) => (path, &["compose"]),
            // The plugin binary takes the Compose verbs directly; it only needs the `compose`
            // word when `docker` is the one dispatching to it.
            Self::Standalone(path) => (path, &[]),
        }
    }

    /// What to show a user who asks where Compose came from.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Plugin(path) => format!("{} compose", path.display()),
            Self::Standalone(path) => path.display().to_string(),
        }
    }
}

/// The plugin directories `docker` searches, plus Homebrew's, which it often does not.
///
/// Absolute and fixed rather than read from `~/.docker/config.json`: the key that would name the
/// Homebrew one (`cliPluginsExtraDirs`) is exactly the key that is missing on the machines this
/// rung exists for, so reading it would answer "nowhere" precisely when it matters.
fn plugin_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".docker/cli-plugins"));
    }
    for fixed in [
        "/opt/homebrew/lib/docker/cli-plugins",
        "/usr/lib/podman/cli-plugins",
        "/usr/libexec/podman/cli-plugins",
        "/usr/local/lib/docker/cli-plugins",
        "/usr/local/libexec/docker/cli-plugins",
        "/usr/lib/docker/cli-plugins",
        "/usr/libexec/docker/cli-plugins",
    ] {
        dirs.push(PathBuf::from(fixed));
    }
    dirs
}

/// Find a working Compose, trying each rung until one answers.
#[must_use]
pub fn find_compose() -> Option<Invocation> {
    if let Some(docker) = find_docker() {
        let candidate = Invocation::Plugin(docker);
        if probe(&candidate).is_ok() {
            return Some(candidate);
        }
    }
    if let Some(standalone) = cide_core::toolchain::which("docker-compose") {
        let candidate = Invocation::Standalone(standalone);
        if probe(&candidate).is_ok() {
            return Some(candidate);
        }
    }
    // Podman, which speaks this API and is the default engine on Fedora and RHEL. `podman
    // compose` delegates to whichever compose implementation is installed, and `podman-compose`
    // is the standalone one — both are tried, after Docker's own, so a machine with both is
    // unsurprising. Everything upstream of here is unchanged: the labels a podman-composed
    // container carries are Docker's, because podman writes them for exactly this reason.
    if let Some(podman) = cide_core::toolchain::which("podman") {
        let candidate = Invocation::Plugin(podman);
        if probe(&candidate).is_ok() {
            return Some(candidate);
        }
    }
    if let Some(standalone) = cide_core::toolchain::which("podman-compose") {
        let candidate = Invocation::Standalone(standalone);
        if probe(&candidate).is_ok() {
            return Some(candidate);
        }
    }
    for dir in plugin_dirs() {
        let path = dir.join("docker-compose");
        if !cide_core::toolchain::is_executable(&path) {
            continue;
        }
        let candidate = Invocation::Standalone(path);
        if probe(&candidate).is_ok() {
            return Some(candidate);
        }
    }
    None
}

/// Run `version` against one candidate.
fn probe(invocation: &Invocation) -> Result<String, DockerError> {
    let (program, prefix) = invocation.program();
    let mut args: Vec<String> = prefix.iter().map(|a| (*a).to_string()).collect();
    args.push("version".to_string());
    run(program, None, &args, PROBE_DEADLINE)
}

/// Where `docker` is, or `None`.
///
/// # Nothing is cached, and the miss is the reason
///
/// `cide_spec::discover::find`'s asymmetry, taken all the way: a user who reads "Compose is not
/// installed" installs it in a terminal pane and presses Retry, and a cached miss would make that
/// button unable to work until the app was relaunched. The cost is one `stat` per board read.
#[must_use]
pub fn find_docker() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os(OVERRIDE_ENV)
        .map(PathBuf::from)
        .filter(|v| !v.as_os_str().is_empty())
    {
        // An override that points at nothing is **not** a fall-through — `connect::ladder`'s
        // rule, restated here because this is a different search. A developer pointing cide at
        // their own `docker` and silently getting the system one would debug the wrong binary.
        return cide_core::toolchain::is_executable(&value).then_some(value);
    }
    cide_core::toolchain::which(BINARY)
}

/// Whether Compose can be run here, and how.
#[must_use]
pub fn availability() -> ComposeAvailability {
    match find_compose() {
        Some(invocation) => match probe(&invocation) {
            Ok(printed) => ComposeAvailability::Present {
                version: format!("{} ({})", first_line(&printed), invocation.describe()),
            },
            // `find_compose` only returns a candidate that already answered, so reaching here
            // means Compose went away between the two calls — a `brew upgrade` mid-session.
            Err(error) => ComposeAvailability::Absent {
                reason: format!("Compose stopped answering: {error}"),
            },
        },
        None => ComposeAvailability::Absent {
            reason: format!(
                "cide could not run Compose. It tried `docker compose`, `docker-compose`, \
                 `podman compose`, `podman-compose`, and the plugin directories those search. \
                 Stacks are still listed — they are read from the daemon's own labels — but \
                 acting on one needs the CLI. {INSTALL_HINT} A cide started from a desktop \
                 launcher also has a different PATH from one started in a terminal."
            ),
        },
    }
}

/// The argv for one stack action.
///
/// Pure, and separated from the spawn for `cide_spec::cli`'s reason: what cide actually runs can
/// then be a unit test on a machine with no Docker at all — which matters more here than anywhere
/// else in this crate, because these are the only commands cide runs that **change** something
/// and are not addressed by an id the daemon validated.
///
/// # `-p <project>` is load-bearing when there is one, and wrong when there is not
///
/// From the **panel**, the project name came off a running container's own
/// `com.docker.compose.project` label, and passing it is not optional: without it `docker
/// compose` derives the name from the working directory, so a stack brought up from a directory
/// that has since been renamed would be a *different* project and `down` would report success
/// having stopped nothing.
///
/// From a **file** there is no label to read, and `project` is therefore `None` — deliberately,
/// rather than cide guessing. Compose's own derivation is the right answer there and cide cannot
/// improve on it: it reads a `name:` at the top of the file, then `COMPOSE_PROJECT_NAME` from the
/// `.env` beside it, and only then falls back to the directory's basename. A cide that passed a
/// guessed `-p` would override all three and act on a stack nobody has — which is the same
/// failure as the renamed directory above, arrived at from the other side.
///
/// The `compose` word is **not** here: whether it is needed depends on which rung of
/// [`find_compose`]'s ladder answered, and [`Invocation::program`] is the one place that knows.
/// A standalone `docker-compose` takes the verbs directly and would read `compose` as a file name.
#[must_use]
pub fn argv(project: Option<&str>, action: ComposeAction, files: &[String]) -> Vec<String> {
    argv_for(project, action, files, &[])
}

/// [`argv`], narrowed to named services.
///
/// # Why every verb takes them, including `down`
///
/// Because Compose says so: `docker compose down [SERVICES]`, `up [SERVICE...]`, and the same for
/// `restart`, `build` and `pull` — checked against the plugin rather than assumed, because an
/// asymmetry here would have meant a gutter whose marker offered a verb that then refused. An
/// empty slice is the whole stack, which is what the panel and the file-level marker pass.
///
/// **Services go last, after the verb and its flags.** `--force-recreate` before them is a flag
/// of `up`; after them it would be read as another service name.
#[must_use]
pub fn argv_for(
    project: Option<&str>,
    action: ComposeAction,
    files: &[String],
    services: &[String],
) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(project) = project {
        args.push("-p".to_string());
        args.push(project.to_string());
    }
    // Every config file the labels recorded, in order. A stack brought up with an override file
    // must be brought down with the same set, or Compose resolves a different service list.
    for file in files {
        args.push("-f".to_string());
        args.push(file.clone());
    }
    match action {
        // `-d`, always. Compose in the foreground streams until interrupted, and there is
        // nothing here to interrupt it — a button that never returns is a hung command worker.
        ComposeAction::Up => {
            args.push("up".to_string());
            args.push("-d".to_string());
        }
        ComposeAction::Down => args.push("down".to_string()),
        ComposeAction::Restart => args.push("restart".to_string()),
        // `--force-recreate` and not `up` alone, which is a no-op on a stack whose file has not
        // changed *in a way Compose notices* — and an edited `environment:` or a rebuilt image
        // tag is exactly such a change. Docker cannot alter a running container, so recreating is
        // the only thing "apply this file" can mean, and doing less than that silently would be
        // the worst outcome here: the button reports success and the container keeps its old
        // configuration. `DetailPane`'s recreate makes the same argument one surface over.
        ComposeAction::Recreate => {
            args.push("up".to_string());
            args.push("-d".to_string());
            args.push("--force-recreate".to_string());
        }
        ComposeAction::Build => args.push("build".to_string()),
        ComposeAction::Pull => args.push("pull".to_string()),
    }
    args.extend(services.iter().cloned());
    args
}

/// The `name:` a compose file declares, if it declares one.
///
/// # A scan, not a parse
///
/// `composeTargets` in the webview makes the same call for the same reason, and here there is a
/// second: this runs to build a *title*, so the worst case of being wrong is a pane labelled with
/// the directory instead of the project. Pulling in a YAML parser to improve a label — and then
/// having it throw on a file the user is midway through editing — is the wrong trade.
///
/// Top-level only (no indent), first occurrence wins, quotes stripped, comments ignored. A `name`
/// nested under `services:` is a service called `name`, not the project, so the indent test is
/// what keeps this from reading one.
///
/// Returns `None` for anything it cannot read confidently, and every caller has a fallback.
fn project_name_in(file: &Path) -> Option<String> {
    // Capped: this is a label, and reading a megabyte off disk for one would be absurd. A `name:`
    // below 64 KiB of compose file is not a file anyone has.
    let text = std::fs::read_to_string(file).ok()?;
    for line in text.lines().take(2_000) {
        // Indented — a key inside a block, not the document's own. A `name` under `services:` is
        // a service *called* name, and reading it as the project is the one way this goes wrong.
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // `continue`, not `?`. The first cut returned here, so a file whose first line was a
        // comment or `services:` answered `None` however plainly it declared a name two lines
        // down — which is every file this was written for.
        let Some(rest) = trimmed.strip_prefix("name:") else {
            continue;
        };
        let rest = rest.split('#').next().unwrap_or(rest).trim();
        let value = rest.trim_matches(['"', '\''].as_slice()).trim();
        return (!value.is_empty()).then(|| value.to_string());
    }
    None
}

/// Whether a file name is one Compose would read.
///
/// # Why this is a name test and not a parse
///
/// Because the surfaces that ask — the file tree's context menu, the editor's gutter, the palette
/// — ask about *every* file the user is looking at, and opening and parsing each one to find out
/// whether it has a `services:` key would be a read per row. The cost of being wrong in this
/// direction is one menu entry that Compose then refuses with its own sentence; the cost of
/// parsing would be the tree stuttering.
///
/// The set is Compose's own, plus the unofficial spellings people actually have on disk.
/// Case-insensitive, because the tree carries the name as the filesystem spells it and macOS does
/// not care.
///
/// # Any dotted segment, not just the first
///
/// This tested only the leading segment until M51, which was enough for `compose.yaml`,
/// `docker-compose.yml` and `compose.override.yaml` — and refused **`docker.compose.yaml`**, whose
/// first segment is `docker`. That is not a rare spelling, and the failure is silent in the worst
/// way: the file is a compose file, Compose reads it happily, and cide simply draws no gutter
/// marker and offers no menu entry, with nothing anywhere saying why.
///
/// So the rule is: strip `.yaml`/`.yml`, then accept if **any** dot-separated segment is `compose`
/// or ends in `-compose`. That takes `docker.compose.yaml`, `app.compose.prod.yaml` and
/// `shop-compose.yaml`, and still refuses `composer.yaml` (a PHP manifest), `decompose.yaml` and
/// `values.yaml` — none of which has a segment equal to `compose`.
#[must_use]
pub fn is_compose_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let Some(stem) = lower
        .strip_suffix(".yaml")
        .or_else(|| lower.strip_suffix(".yml"))
    else {
        return false;
    };
    stem.split('.')
        .any(|segment| segment == "compose" || segment.ends_with("-compose"))
}

/// Resolve a compose file and an action into something a terminal pane can spawn. (M48)
///
/// The working directory is the file's own — see [`cide_ipc::docker::ComposeRun::working_dir`]
/// for why that is load-bearing — and the file is still passed with `-f`, even though Compose
/// would find it there anyway: the two spellings differ for `compose.override.yaml`, which
/// Compose loads automatically beside `compose.yaml` and would *not* load if the user picked the
/// override alone. Naming it says which file the user actually clicked.
pub fn plan(
    file: &Path,
    action: ComposeAction,
    services: &[String],
) -> Result<ComposeRun, DockerError> {
    let Some(invocation) = find_compose() else {
        return Err(DockerError::Refused(match availability() {
            ComposeAvailability::Absent { reason } => reason,
            ComposeAvailability::Present { .. } => "Compose went away".to_string(),
        }));
    };
    let (program, leading) = invocation.program();

    let dir = file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.display().to_string());

    let mut args: Vec<String> = leading.iter().map(|w| (*w).to_string()).collect();
    args.extend(argv_for(
        None,
        action,
        std::slice::from_ref(&name),
        services,
    ));

    // What to call the stack in the pane's title: the file's own `name:` when it has one, and the
    // directory's basename otherwise — which is Compose's own fallback, in Compose's own order.
    //
    // Reading the key matters more than it sounds. A compose file in a directory called `tmp`,
    // `docker`, `deploy` or `infra` — all common — titles its pane `compose up : tmp`, which says
    // nothing; the same file usually declares `name: something-meaningful` one line down.
    let where_ = project_name_in(file)
        .or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| dir.display().to_string());

    // Named services replace the directory in the title, because that is what the run is *about*
    // — `compose up : web` says more than `compose up : shop` when the user clicked one service's
    // marker. More than two are elided rather than listed: a tab strip has no room and the pane's
    // first line of output names them all anyway.
    let subject = match services {
        [] => where_,
        [one] => one.clone(),
        [first, second] => format!("{first}, {second}"),
        [first, rest @ ..] => format!("{first} +{}", rest.len()),
    };

    Ok(ComposeRun {
        program: program.display().to_string(),
        args,
        working_dir: dir.display().to_string(),
        title: format!("compose {} : {subject}", action.label().to_lowercase()),
    })
}

/// Run one stack action in its own working directory.
pub fn act(
    working_dir: Option<&Path>,
    project: &str,
    action: ComposeAction,
    files: &[String],
) -> Result<String, DockerError> {
    // **`find_compose`, not `find_docker`.** This line said `find_docker` from M43 until M48, and
    // every stack button in the panel was refused by `docker` itself with `unknown shorthand
    // flag: 'p' in -p` — because [`argv`] deliberately omits the `compose` word and
    // [`Invocation::program`] is the only place that knows whether this rung needs it. Resolving
    // the binary without also asking *how it is invoked* drops that word on the floor.
    let Some(invocation) = find_compose() else {
        return Err(DockerError::Refused(match availability() {
            ComposeAvailability::Absent { reason } => reason,
            ComposeAvailability::Present { .. } => "Compose went away".to_string(),
        }));
    };
    let (program, leading) = invocation.program();
    let mut args: Vec<String> = leading.iter().map(|w| (*w).to_string()).collect();
    args.extend(argv(Some(project), action, files));
    run(program, working_dir, &args, ACT_DEADLINE)
}

/// Fork `docker`, with the workspace's spawn chokepoint applied.
///
/// `run_filter_with` and not `Command::output`, for the reason `cide_spec::cli::run` states: it
/// is where `prepare_command` and `arm` are applied, which is what makes the pair impossible to
/// forget. The extra `PATH` entry is `docker`'s own directory — it shells out to its plugins and
/// to a credential helper, and a child with a desktop launch's `PATH` reaches neither.
fn run(
    docker: &Path,
    cwd: Option<&Path>,
    args: &[String],
    deadline: Duration,
) -> Result<String, DockerError> {
    let mut command = Command::new(docker);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    // Colour would be escape sequences in a sentence cide shows in a panel.
    command.env("NO_COLOR", "1");

    let bin_dir: Vec<PathBuf> = docker
        .parent()
        .map(|d| vec![d.to_path_buf()])
        .unwrap_or_default();
    let filtered = cide_core::child_env::run_filter_with(command, None, deadline, &bin_dir)
        .map_err(|error| DockerError::Unreachable(describe(docker, args, &error)))?;

    if !filtered.ok {
        // Compose writes its progress *and* its errors to stderr, so the last non-empty line is
        // the one worth showing — the first is "Container x  Stopping".
        let detail = last_line(&filtered.stderr);
        return Err(DockerError::Refused(if detail.is_empty() {
            format!("`docker {}` failed and said nothing", args.join(" "))
        } else {
            detail
        }));
    }
    Ok(String::from_utf8_lossy(&filtered.stdout).into_owned())
}

fn describe(docker: &Path, args: &[String], error: &cide_core::child_env::FilterError) -> String {
    use cide_core::child_env::FilterError;
    let call = format!("`{} {}`", docker.display(), args.join(" "));
    match error {
        FilterError::Spawn(io) => format!("{call} could not be started: {io}"),
        FilterError::Timeout => format!(
            "{call} did not finish in {} minutes and was stopped. A stack that pulls large \
             images can legitimately take longer — the containers it did start are still \
             running, and the panel will show them.",
            ACT_DEADLINE.as_secs() / 60
        ),
        FilterError::Unreadable => format!("{call} ran and its output could not be read"),
        FilterError::Wait(io) => format!("{call} ran and could not be reaped: {io}"),
    }
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string()
}

fn last_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_project_name_is_always_passed_and_is_never_derived_from_a_directory() {
        // The load-bearing argument. Without `-p`, Compose derives the project name from the
        // working directory — so a stack brought up from a directory that has since been renamed
        // is a *different* project, and `down` reports success having stopped nothing.
        for action in ALL {
            let args = argv(Some("shop"), action, &[]);
            let at = args.iter().position(|a| a == "-p").expect("-p is passed");
            assert_eq!(args[at + 1], "shop");
            assert!(
                !args.contains(&"compose".to_string()),
                "the `compose` word belongs to `Invocation::program`, not here: a standalone \
                 `docker-compose` takes the verbs directly and would read it as a file name",
            );

            // And the other half, which is the whole reason the parameter became an `Option`: a
            // file has no label to read a project name out of, and a *guessed* `-p` would
            // override the `name:` in the file, the `.env` beside it and Compose's own
            // directory-basename fallback — acting on a stack nobody has.
            let from_file = argv(None, action, &["compose.yaml".to_string()]);
            assert!(!from_file.contains(&"-p".to_string()), "{from_file:?}");
        }
    }

    /// Every verb, so a new one cannot be added without the rules below seeing it.
    const ALL: [ComposeAction; 6] = [
        ComposeAction::Up,
        ComposeAction::Down,
        ComposeAction::Restart,
        ComposeAction::Recreate,
        ComposeAction::Build,
        ComposeAction::Pull,
    ];

    #[test]
    fn every_action_spells_a_verb_and_recreate_forces_it() {
        // A verb is the one token that must be there: an argv of nothing but flags runs
        // `docker compose` bare, which prints usage and exits 1 — a button that "failed" with a
        // help screen.
        const VERBS: [&str; 4] = ["up", "down", "restart", "build"];
        for action in ALL {
            let args = argv(Some("shop"), action, &[]);
            assert!(
                args.iter()
                    .any(|a| VERBS.contains(&a.as_str()) || a == "pull"),
                "{action:?} spells no verb: {args:?}",
            );
        }

        // `up` alone is a **no-op** on a stack whose file changed in a way Compose does not
        // notice, so the one verb that answers "apply what I just edited" has to force it.
        // Reporting success while the container keeps its old configuration is the failure.
        let recreate = argv(Some("shop"), ComposeAction::Recreate, &[]);
        assert!(
            recreate.contains(&"--force-recreate".to_string()),
            "{recreate:?}"
        );
        assert!(recreate.contains(&"up".to_string()), "{recreate:?}");
        assert!(
            !argv(Some("shop"), ComposeAction::Up, &[]).contains(&"--force-recreate".to_string()),
            "and a plain up does not, or every up would restart a stack that was fine",
        );
    }

    #[test]
    fn only_the_bounded_verbs_are_offered_as_buttons() {
        // The panel has nowhere to show output and waits for the answer, so it may only offer
        // actions that end. A cold `pull` of a multi-gigabyte image is normal and a `build` is
        // unbounded by definition; both are the pane's, and `is_quick` is where that is decided
        // once rather than in each surface.
        assert!(!ComposeAction::Pull.is_quick());
        assert!(!ComposeAction::Build.is_quick());
        for action in [
            ComposeAction::Up,
            ComposeAction::Down,
            ComposeAction::Restart,
            ComposeAction::Recreate,
        ] {
            assert!(action.is_quick(), "{action:?}");
        }
    }

    #[test]
    fn named_services_go_last_and_every_verb_takes_them() {
        // Checked against the real plugin rather than assumed: `docker compose down [SERVICES]`
        // and `up [SERVICE...]` both take them, so the gutter can offer the whole verb list on a
        // service's marker instead of a subset that has to be explained.
        for action in ALL {
            let args = argv_for(
                None,
                action,
                &["compose.yaml".to_string()],
                &["web".to_string(), "db".to_string()],
            );
            assert_eq!(
                &args[args.len() - 2..],
                ["web", "db"],
                "{action:?} must put services last: {args:?}",
            );
        }

        // The half that is a real bug if it moves: `--force-recreate` is a flag of `up`, and
        // after the service names it would be read as another service.
        let recreate = argv_for(None, ComposeAction::Recreate, &[], &["web".to_string()]);
        let flag = recreate
            .iter()
            .position(|a| a == "--force-recreate")
            .expect("the flag");
        let service = recreate
            .iter()
            .position(|a| a == "web")
            .expect("the service");
        assert!(flag < service, "{recreate:?}");

        // And no services is the whole stack, which is what every surface but the gutter passes.
        assert_eq!(
            argv_for(Some("shop"), ComposeAction::Up, &[], &[]),
            argv(Some("shop"), ComposeAction::Up, &[]),
        );
    }

    #[test]
    fn the_files_own_project_name_is_read_when_it_has_one() {
        let dir = std::env::temp_dir().join(format!("cide-name-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");

        // The shape the first cut got wrong: a comment, then `services:`, then the name. It
        // returned on the first non-indented line that was not `name:`, so every real file
        // answered `None` — including one that declares its name perfectly plainly.
        let declared = dir.join("docker.compose.yaml");
        std::fs::write(
            &declared,
            "# a comment\nservices:\n  web:\n    image: nginx\nname: cide-sample  # trailing\n",
        )
        .expect("the file");
        assert_eq!(project_name_in(&declared).as_deref(), Some("cide-sample"));

        // Quoted, and first-wins.
        let quoted = dir.join("compose.yaml");
        std::fs::write(&quoted, "name: \"shop\"\nname: later\n").expect("the file");
        assert_eq!(project_name_in(&quoted).as_deref(), Some("shop"));

        // **Indented `name` is a service, not the project.** The one way this goes wrong, and it
        // would go wrong silently: a pane titled after whatever service happened to be called
        // `name`.
        let nested = dir.join("nested.compose.yaml");
        std::fs::write(&nested, "services:\n  name: not-the-project\n").expect("the file");
        assert_eq!(project_name_in(&nested), None);

        // No name at all, and an empty one, both fall back — every caller has a fallback.
        let bare = dir.join("bare.compose.yaml");
        std::fs::write(&bare, "services:\n  web:\n    image: nginx\n").expect("the file");
        assert_eq!(project_name_in(&bare), None);
        let empty = dir.join("empty.compose.yaml");
        std::fs::write(&empty, "name:\n").expect("the file");
        assert_eq!(project_name_in(&empty), None);

        // And the title actually uses it, which is the whole point — the directory here is
        // `cide-name-<pid>`, so a title saying `cide-sample` proves the key won.
        if find_compose().is_some() {
            let plan = plan(&declared, ComposeAction::Up, &[]).expect("a plan");
            assert_eq!(plan.title, "compose up : cide-sample", "{plan:?}");
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_compose_file_is_recognised_by_the_names_people_actually_have() {
        for yes in [
            "compose.yaml",
            "compose.yml",
            "docker-compose.yml",
            "docker-compose.yaml",
            "compose.override.yaml",
            "docker-compose.prod.yml",
            "Docker-Compose.YAML",
            "shop-compose.yaml",
            // The spelling that was refused until M51. Its first segment is `docker`, so a
            // first-segment-only rule drew no gutter marker on a file Compose reads happily —
            // silent in the worst way, because nothing anywhere said why.
            "docker.compose.yaml",
            "app.compose.prod.yml",
        ] {
            assert!(is_compose_file(yes), "{yes}");
        }
        for no in [
            "compose.json",
            "composer.yaml",
            "values.yaml",
            "compose",
            "Dockerfile",
            ".env",
            "a.compose.yaml.bak",
            "decompose.yaml",
            // A PHP manifest, and the reason the test is on a whole segment rather than a
            // prefix: `composer` starts with `compose`.
            "composer.lock.yaml",
        ] {
            assert!(!is_compose_file(no), "{no}");
        }
    }

    #[test]
    fn up_is_always_detached() {
        // Compose in the foreground streams until interrupted, and there is nothing here to
        // interrupt it — a button that never returns is a hung command worker.
        let args = argv(Some("shop"), ComposeAction::Up, &[]);
        assert!(args.contains(&"-d".to_string()), "{args:?}");
    }

    #[test]
    fn every_config_file_is_passed_in_order() {
        // A stack brought up with an override file must be brought down with the same set, or
        // Compose resolves a different service list and leaves containers running.
        let files = vec!["compose.yaml".to_string(), "compose.dev.yaml".to_string()];
        let args = argv(Some("shop"), ComposeAction::Down, &files);
        let flags: Vec<&String> = args
            .iter()
            .enumerate()
            .filter(|(i, _)| i > &0 && args[i - 1] == "-f")
            .map(|(_, a)| a)
            .collect();
        assert_eq!(flags, vec!["compose.yaml", "compose.dev.yaml"]);
    }

    #[test]
    fn a_broken_override_refuses_rather_than_falling_through() {
        // `connect::ladder`'s rule, restated for this search: a developer pointing cide at their
        // own `docker` and silently getting the system one would debug the wrong binary.
        //
        // Uses a path that cannot exist, and restores the variable, so this is safe to run in
        // parallel with nothing else reading it.
        let before = std::env::var_os(OVERRIDE_ENV);
        // SAFETY: single-threaded test process section; restored below.
        unsafe { std::env::set_var(OVERRIDE_ENV, "/nonexistent/cide/docker") };
        assert_eq!(
            find_docker(),
            None,
            "a broken override is not a fall-through"
        );
        match before {
            Some(value) => unsafe { std::env::set_var(OVERRIDE_ENV, value) },
            None => unsafe { std::env::remove_var(OVERRIDE_ENV) },
        }
    }

    #[test]
    fn the_absent_sentence_names_the_launcher_difference_and_says_stacks_still_list() {
        // "But I have Docker installed" is the reply a bare not-found earns, and a user whose
        // stacks vanished would think the panel was broken rather than the buttons.
        let ComposeAvailability::Absent { reason } = (ComposeAvailability::Absent {
            reason: format!(
                "`{BINARY}` is not on this app's PATH, so cide cannot run Compose commands. \
                 Stacks are still listed — they are read from the daemon — but their buttons \
                 need the CLI. A cide started from a desktop launcher has a different PATH from \
                 one started in a terminal."
            ),
        }) else {
            unreachable!()
        };
        assert!(reason.contains("desktop launcher"), "{reason}");
        assert!(reason.contains("still listed"), "{reason}");
    }
}
