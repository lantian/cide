//! Scoped Git writes for editable Codex sessions. The profile is invocation-local; Codex,
//! rather than a second config loader in cide, decides whether it is compatible and allowed.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::{child_env, codex_cli};

pub const PROFILE_NAME: &str = "cide-git";
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitProfile {
    /// A whole inline table: splitting a dotted `.git` path into config-key components loses
    /// its quoting in Codex's `-c` parser.
    pub config: String,
}

impl GitProfile {
    pub fn new(git_dirs: &[PathBuf], writable_dirs: &[PathBuf], network: bool) -> Self {
        let mut dirs = git_dirs.to_vec();
        dirs.sort();
        dirs.dedup();
        let filesystem = dirs
            .iter()
            .map(|dir| {
                format!(
                    "{}=\"write\"",
                    codex_cli::toml_string(&dir.to_string_lossy())
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let roots = writable_dirs
            .iter()
            .map(|dir| format!("{}=true", codex_cli::toml_string(&dir.to_string_lossy())))
            .collect::<Vec<_>>()
            .join(",");
        Self {
            config: format!(
                "permissions.{PROFILE_NAME}={{extends=\":workspace\",filesystem={{{filesystem}}},workspace_roots={{{roots}}},network={{enabled={network}}} }}"
            ),
        }
    }

    pub fn args(&self) -> Vec<String> {
        vec![
            "-c".into(),
            self.config.clone(),
            "-c".into(),
            format!("default_permissions=\"{PROFILE_NAME}\""),
        ]
    }

    /// Codex highlights the named filesystem profile rather than a built-in approval mode.
    /// Show the explicitly selected reviewer in that row without changing its permissions.
    pub fn args_with_approval_reviewer(&self, automatic: bool) -> Vec<String> {
        let mut args = self.args();
        codex_cli::push_config(
            &mut args,
            &format!("permissions.{PROFILE_NAME}.description"),
            codex_cli::toml_string(if automatic {
                "Approve for me; workspace access with scoped Git writes"
            } else {
                "Ask for approval; workspace access with scoped Git writes"
            }),
        );
        args
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitPermissions {
    Applied(GitProfile),
    /// A compatibility problem: keep the original launch, and tell the person and the run.
    Skipped(String),
    /// Disabled, read-only, unrestricted, or not a Git checkout; no warning toast is needed.
    Inactive(String),
}

impl GitPermissions {
    pub fn profile(&self) -> Option<&GitProfile> {
        match self {
            Self::Applied(profile) => Some(profile),
            _ => None,
        }
    }

    pub fn warning(&self) -> Option<&str> {
        match self {
            Self::Skipped(reason) => Some(reason),
            _ => None,
        }
    }

    pub fn note(&self) -> &str {
        match self {
            Self::Applied(_) => {
                "Scoped Git permissions apply on the next launch; approval settings are unchanged."
            }
            Self::Skipped(reason) | Self::Inactive(reason) => reason,
        }
    }
}

/// Blocking, bounded preflight. Call on the app's blocking pool before assembling argv.
/// `env` is the same prepared environment as the eventual child, including role grants.
pub fn preflight(
    cli: &cide_ipc::CodexCli,
    cwd: &Path,
    git_dirs: &[PathBuf],
    writable_dirs: &[PathBuf],
    network: bool,
    env: &[child_env::EnvChange],
) -> GitPermissions {
    if !cli.inject.git_permissions {
        return GitPermissions::Inactive("Git permission injection is off.".into());
    }
    if git_dirs.is_empty() {
        return GitPermissions::Inactive("No Git metadata was resolved for this checkout.".into());
    }
    let launch = codex_cli::plan_here(cli);
    let mode = codex_cli::selected_permission_mode(cli, &launch.args);
    use cide_ipc::CodexPermissionMode as Mode;
    if matches!(
        mode,
        Mode::ReadOnly | Mode::FullAccess | Mode::CustomProfile
    ) {
        return GitPermissions::Inactive(
            "The selected Codex permissions do not use cide's workspace Git profile.".into(),
        );
    }
    let mut config_args = match probe_args(&launch.args) {
        Ok(args) => args,
        Err(result) => return result,
    };
    if matches!(mode, Mode::AskForApproval | Mode::ApproveForMe) {
        // The explicit workspace preset supersedes a default named profile, but legacy sandbox
        // config still uses the established fallback rather than mixing the two schemas.
        codex_cli::push_config(
            &mut config_args,
            "default_permissions",
            "\":workspace\"".into(),
        );
    }
    let binary = match codex_cli::resolve(&cli.binary) {
        Ok(path) => path,
        Err(_) => return skipped("the Codex binary could not be resolved"),
    };
    // Do not run a custom wrapper with a different subcommand. Scripts may pin policy or
    // forward arguments to another program; their owner retains control of the launch.
    let mut magic = [0; 4];
    if std::fs::File::open(&binary)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_err()
        || magic.starts_with(b"#!")
    {
        return skipped("a wrapper controls the Codex launch");
    }
    let profile = GitProfile::new(git_dirs, writable_dirs, network);
    let deadline = Instant::now() + PROBE_TIMEOUT;
    // Codex requires default_permissions whenever a [permissions] table is present. Read
    // the original policy first, then declare the candidate beside the compatible workspace
    // default in a second probe. Neither probe starts a thread or a model turn.
    let config = match probe(&binary, &config_args, cwd, None, env, deadline) {
        Ok((config, _)) => config,
        Err(reason) => return skipped(&reason),
    };
    if let Err(result) = config_compatibility(&config) {
        return result;
    }
    if explicitly_untrusted(&config, cwd, git_dirs) {
        return GitPermissions::Inactive(
            "The project is explicitly untrusted; its existing Codex permissions are preserved."
                .into(),
        );
    }
    match probe(&binary, &config_args, cwd, Some(&profile), env, deadline) {
        Ok((config, profiles)) => compatibility(&config, &profiles, profile),
        Err(reason) => skipped(&reason),
    }
}

fn skipped(reason: &str) -> GitPermissions {
    GitPermissions::Skipped(format!(
        "Cide's Git permission profile was not applied: {reason}. Existing Codex permissions are preserved; Git writes may require the supported approval route."
    ))
}

/// Only config overrides belong on the app-server probe. A config-profile selection or a
/// positional wrapper command is preserved without trying to reinterpret its launch.
fn probe_args(args: &[String]) -> Result<Vec<String>, GitPermissions> {
    let mut config = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-s" | "--sandbox" | "--approve-for-me" | "--full-auto" => {
                return Err(skipped("explicit legacy sandbox flags take precedence"));
            }
            "--dangerously-bypass-approvals-and-sandbox" | "--yolo" => {
                return Err(GitPermissions::Inactive(
                    "Codex is configured for unrestricted access.".into(),
                ));
            }
            "-p" | "--profile" | "--" | "--worktree" => {
                return Err(skipped(
                    "a custom profile, wrapper, or checkout controls the launch",
                ));
            }
            "-c" | "--config" => {
                let Some(value) = args.next() else {
                    return Err(skipped("a config override is incomplete"));
                };
                config.extend(["-c".into(), value.clone()]);
            }
            "--enable" | "--disable" => {
                let Some(value) = args.next() else {
                    return Err(skipped("a feature argument is incomplete"));
                };
                config.extend([arg.clone(), value.clone()]);
            }
            "-a" | "--ask-for-approval" | "-m" | "--model" | "--local-provider" | "--add-dir" => {
                if args.next().is_none() {
                    return Err(skipped("a launch argument is incomplete"));
                }
                // Additional writable roots must be inherited explicitly rather than lost.
                if arg == "--add-dir" {
                    return Err(skipped("user-supplied writable roots control the sandbox"));
                }
            }
            "--search" | "--no-daemon" | "--oss" | "--strict-config" => {}
            _ if arg.starts_with("--config=") || arg.starts_with("-c=") => {
                config.extend(["-c".into(), arg.split_once('=').unwrap().1.into()]);
            }
            _ if arg.starts_with("--model=") || arg.starts_with("--ask-for-approval=") => {}
            _ => return Err(skipped("custom launch arguments control the Codex launch")),
        }
    }
    Ok(config)
}

fn config_compatibility(config: &Value) -> Result<(), GitPermissions> {
    let Some(config) = config.get("config").and_then(Value::as_object) else {
        return Err(skipped("Codex returned an unknown effective-config format"));
    };
    if config.get("sandbox_mode").is_some_and(|v| !v.is_null())
        || config
            .get("sandbox_workspace_write")
            .is_some_and(|v| !v.is_null())
    {
        return Err(skipped(
            "legacy sandbox settings take precedence; migrate them to permission profiles to enable scoped Git writes",
        ));
    }
    match config.get("default_permissions").and_then(Value::as_str) {
        None | Some(":workspace") => {}
        Some(":read-only") => {
            return Err(GitPermissions::Inactive(
                "The selected Codex profile is read-only.".into(),
            ));
        }
        Some(":danger-full-access") => {
            return Err(GitPermissions::Inactive(
                "The selected Codex profile is unrestricted.".into(),
            ));
        }
        Some(_) => return Err(skipped("a custom Codex permission profile is selected")),
    }
    Ok(())
}

/// Codex looks up a linked worktree's trust at its main repository too. Inspect the effective
/// project table rather than changing trust to make the grant work.
fn explicitly_untrusted(config: &Value, cwd: &Path, git_dirs: &[PathBuf]) -> bool {
    let Some(projects) = config
        .pointer("/config/projects")
        .and_then(Value::as_object)
    else {
        return false;
    };
    let common = git_dirs
        .iter()
        .find(|dir| git_dirs.iter().all(|other| other.starts_with(dir)));
    [Some(cwd), common.and_then(|dir| dir.parent())]
        .into_iter()
        .flatten()
        .any(|root| {
            projects
                .iter()
                .filter(|(path, _)| root.starts_with(Path::new(path)))
                .max_by_key(|(path, _)| Path::new(path).components().count())
                .is_some_and(|(_, project)| {
                    project.get("trust_level").and_then(Value::as_str) == Some("untrusted")
                })
        })
}

fn compatibility(config: &Value, profiles: &Value, profile: GitProfile) -> GitPermissions {
    if let Err(result) = config_compatibility(config) {
        return result;
    }
    let Some(data) = profiles.get("data").and_then(Value::as_array) else {
        return skipped("this Codex does not report permission-profile availability");
    };
    match data
        .iter()
        .find(|item| item.get("id").and_then(Value::as_str) == Some(PROFILE_NAME))
    {
        Some(item) if item.get("allowed") == Some(&Value::Bool(true)) => {
            GitPermissions::Applied(profile)
        }
        Some(_) => skipped("managed requirements do not allow cide's Git profile"),
        None => skipped("this Codex does not support cide's Git profile"),
    }
}

/// Only requested JSON-RPC responses cross the reader channel; none are logged.
/// Both preflights share one deadline; killing and reaping unblocks the reader too.
fn probe(
    binary: &Path,
    config_args: &[String],
    cwd: &Path,
    profile: Option<&GitProfile>,
    env: &[child_env::EnvChange],
    deadline: Instant,
) -> Result<(Value, Value), String> {
    if Instant::now() >= deadline {
        return Err("the Codex configuration probe timed out".into());
    }
    let mut command = Command::new(binary);
    command
        .args(config_args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(profile) = profile {
        command.args([
            "-c",
            &profile.config,
            "-c",
            "default_permissions=\":workspace\"",
        ]);
    }
    command.args(["app-server", "--listen", "stdio://"]);
    child_env::prepare_command(&mut command);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    for (name, value) in env {
        match value {
            Some(value) => {
                command.env(name, value);
            }
            None => {
                command.env_remove(name);
            }
        }
    }
    let mut child = command
        .spawn()
        .map_err(|_| "the Codex configuration probe could not start".to_string())?;
    let stdout = child.stdout.take().expect("piped stdout");
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        // A bounded line avoids an incompatible or broken binary allocating without limit.
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            let read = reader.by_ref().take(4 * 1024 * 1024).read_line(&mut line);
            if !matches!(read, Ok(n) if n > 0) || !line.ends_with('\n') {
                break;
            }
            if let Ok(message) = serde_json::from_str::<Value>(&line)
                && message
                    .get("id")
                    .and_then(Value::as_u64)
                    .is_some_and(|id| (1..=3).contains(&id))
                && tx.send(message).is_err()
            {
                break;
            }
        }
    });
    let result = (|| {
        let stdin = child.stdin.as_mut().expect("piped stdin");
        let mut request = |id, method: &str, params: Value| -> Result<Value, String> {
            writeln!(
                stdin,
                "{}",
                json!({"id":id,"method":method,"params":params})
            )
            .and_then(|_| stdin.flush())
            .map_err(|_| "the Codex configuration probe closed".to_string())?;
            loop {
                let message = rx
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .map_err(|_| "the Codex configuration probe failed or timed out".to_string())?;
                if message.get("id") == Some(&json!(id)) {
                    let result = message
                        .get("result")
                        .cloned()
                        .ok_or_else(|| "Codex refused the permission-profile probe".into());
                    if result.is_ok() && id == 1 {
                        writeln!(stdin, "{}", json!({"method":"initialized"}))
                            .and_then(|_| stdin.flush())
                            .map_err(|_| "the Codex configuration probe closed".to_string())?;
                    }
                    return result;
                }
            }
        };
        request(
            1,
            "initialize",
            json!({"clientInfo":{"name":"cide-git-permissions","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}),
        )?;
        let config = request(2, "config/read", json!({"cwd":cwd,"includeLayers":false}))?;
        // The candidate is declared, but not selected, so this checks managed permission
        // requirements without replacing the user's active policy.
        let mut profiles = request(3, "permissionProfile/list", json!({"cwd":cwd}))?;
        while let Some(cursor) = profiles
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_owned)
        {
            if profiles
                .get("data")
                .and_then(Value::as_array)
                .is_some_and(|data| {
                    data.iter()
                        .any(|item| item.get("id").and_then(Value::as_str) == Some(PROFILE_NAME))
                })
            {
                break;
            }
            let next = request(
                3,
                "permissionProfile/list",
                json!({"cwd":cwd,"cursor":cursor}),
            )?;
            profiles = next;
        }
        // Drop unrelated profile metadata before returning to the caller.
        if let Some(data) = profiles.get_mut("data").and_then(Value::as_array_mut) {
            data.retain(|item| item.get("id").and_then(Value::as_str) == Some(PROFILE_NAME));
        }
        Ok((config, profiles))
    })();
    #[cfg(unix)]
    // SAFETY: this is the dedicated process group created above for this probe. Killing its
    // descendants closes inherited pipe handles too, so joining the reader stays bounded.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.join();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> GitProfile {
        GitProfile::new(
            &["/repo/.git".into(), "/repo/.git/worktrees/worker".into()],
            &[],
            false,
        )
    }

    #[test]
    fn only_standard_workspace_permissions_are_extended() {
        let available = json!({"data":[{"id":PROFILE_NAME,"allowed":true}]});
        for config in [json!({}), json!({"default_permissions":":workspace"})] {
            assert!(matches!(
                compatibility(&json!({"config":config}), &available, profile()),
                GitPermissions::Applied(_)
            ));
        }
        for config in [
            json!({"sandbox_mode":"workspace-write"}),
            json!({"sandbox_workspace_write":{}}),
            json!({"default_permissions":"custom"}),
        ] {
            assert!(
                compatibility(&json!({"config":config}), &available, profile())
                    .warning()
                    .is_some()
            );
        }
        for name in [":read-only", ":danger-full-access"] {
            assert!(matches!(
                compatibility(
                    &json!({"config":{"default_permissions":name}}),
                    &available,
                    profile()
                ),
                GitPermissions::Inactive(_)
            ));
        }
        assert!(
            compatibility(
                &json!({"config":{}}),
                &json!({"data":[{"id":PROFILE_NAME,"allowed":false}]}),
                profile()
            )
            .warning()
            .is_some()
        );
        assert!(
            compatibility(&json!({}), &available, profile())
                .warning()
                .is_some()
        );
    }

    #[test]
    fn explicit_project_trust_is_preserved_in_linked_worktrees() {
        let config = json!({"config":{"projects":{"/primary":{"trust_level":"untrusted"}}}});
        let dirs = vec![
            "/primary/.git/worktrees/worker".into(),
            "/primary/.git".into(),
        ];
        assert!(explicitly_untrusted(
            &config,
            Path::new("/elsewhere/worker"),
            &dirs
        ));
        assert!(!explicitly_untrusted(
            &config,
            Path::new("/other"),
            &["/other/.git".into()]
        ));
        let config = json!({"config":{"projects":{"/":{"trust_level":"untrusted"},"/primary":{"trust_level":"trusted"}}}});
        assert!(!explicitly_untrusted(
            &config,
            Path::new("/primary/worker"),
            &dirs
        ));
    }

    #[test]
    fn profile_quotes_paths_and_preserves_role_grants() {
        let profile = GitProfile::new(
            &[
                "/repo with \"quotes\"/.git".into(),
                "/repo with \"quotes\"/.git".into(),
            ],
            &["/cache".into()],
            true,
        );
        assert_eq!(profile.config.matches("=\"write\"").count(), 1);
        assert!(
            profile
                .config
                .contains(r#""/repo with \"quotes\"/.git"="write""#)
        );
        assert!(profile.config.contains("workspace_roots={\"/cache\"=true}"));
        assert!(profile.config.contains("network={enabled=true}"));
        assert_eq!(profile.args()[3], "default_permissions=\"cide-git\"");
    }

    #[test]
    fn custom_launches_are_preserved_without_probing() {
        for args in [
            vec!["-s", "workspace-write"],
            vec!["--approve-for-me"],
            vec!["-p", "custom"],
            vec!["agent", "codex", "--"],
            vec!["--add-dir", "/elsewhere"],
        ] {
            assert!(probe_args(&args.into_iter().map(str::to_owned).collect::<Vec<_>>()).is_err());
        }
        assert_eq!(
            probe_args(&[
                "--search".into(),
                "-a".into(),
                "never".into(),
                "-c".into(),
                "model=\"example\"".into()
            ])
            .unwrap(),
            vec!["-c", "model=\"example\""]
        );
    }

    #[test]
    fn disabling_git_permissions_never_starts_a_probe() {
        let mut cli = cide_ipc::CodexCli {
            binary: "/does/not/exist".into(),
            ..Default::default()
        };
        cli.inject.git_permissions = false;
        assert!(matches!(
            preflight(
                &cli,
                Path::new("/repo"),
                &["/repo/.git".into()],
                &[],
                false,
                &[]
            ),
            GitPermissions::Inactive(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn the_probe_reads_rpc_results_and_bounds_failure_cleanup() {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("cide-permission-rpc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let binary = root.join("fixture");
        let write = |body: &str| {
            std::fs::write(&binary, body).unwrap();
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        };
        write(
            "#!/bin/sh\nread -r request\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nread -r initialized\nread -r request\nprintf '%s\\n' '{\"id\":2,\"result\":{\"config\":{}}}'\nread -r request\nprintf '%s\\n' '{\"id\":3,\"result\":{\"data\":[{\"id\":\"cide-git\",\"allowed\":true}]}}'\n",
        );
        let (config, profiles) = probe(
            &binary,
            &[],
            &root,
            None,
            &[],
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        assert!(matches!(
            compatibility(&config, &profiles, profile()),
            GitPermissions::Applied(_)
        ));
        let cli = cide_ipc::CodexCli {
            binary: binary.to_string_lossy().into(),
            ..Default::default()
        };
        assert!(
            preflight(&cli, &root, &[root.join(".git")], &[], false, &[])
                .warning()
                .unwrap()
                .contains("wrapper")
        );
        write("#!/bin/sh\nsleep 20\n");
        let start = Instant::now();
        assert!(
            probe(
                &binary,
                &[],
                &root,
                None,
                &[],
                start + Duration::from_millis(100)
            )
            .is_err()
        );
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "probe cleanup exceeded its deadline"
        );
        write(
            "#!/bin/sh\nread -r request\nprintf '%s\\n' '{\"id\":1,\"error\":{\"code\":-32601,\"message\":\"unsupported\"}}'\nsleep 20\n",
        );
        assert!(
            probe(
                &binary,
                &[],
                &root,
                None,
                &[],
                Instant::now() + Duration::from_secs(1)
            )
            .unwrap_err()
            .contains("refused")
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
