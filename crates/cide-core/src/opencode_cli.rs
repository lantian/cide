//! OpenCode launch tokens, shared by the settings readout and every child spawn.
use cide_ipc::OpencodeCli;

#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub args: Vec<String>,
    pub env: Vec<crate::child_env::EnvChange>,
    pub arg_notes: Vec<(usize, String)>,
    pub env_notes: Vec<(usize, String)>,
}

pub fn plan(cli: &OpencodeCli) -> Plan {
    let mut plan = Plan::default();
    let mut skip_value = false;
    for (index, arg) in cli.args.iter().enumerate() {
        let key = arg.split('=').next().unwrap_or(arg);
        let reserved = matches!(
            key,
            "--" | "--server"
                | "--standalone"
                | "--session"
                | "-s"
                | "--continue"
                | "-c"
                | "--fork"
                | "--prompt"
                | "--port"
                | "--hostname"
                | "--format"
                | "--dir"
                | "--attach"
                | "--password"
        );
        if skip_value || reserved {
            plan.arg_notes.push((
                index,
                "cide owns session routing and server connection arguments.".into(),
            ));
            skip_value = reserved
                && !arg.contains('=')
                && matches!(
                    key,
                    "--server"
                        | "--session"
                        | "-s"
                        | "--prompt"
                        | "--port"
                        | "--hostname"
                        | "--format"
                        | "--dir"
                        | "--attach"
                        | "--password"
                );
        } else {
            plan.args.push(arg.clone());
        }
    }
    for (index, var) in cli.env.iter().enumerate() {
        let name = var.name.trim();
        if name.is_empty()
            || name.contains('=')
            || name.contains('\0')
            || var.value.contains('\0')
            || name.starts_with("CIDE_")
            || matches!(
                name,
                "OPENCODE_CONFIG_CONTENT" | "OPENCODE_SERVER_PASSWORD" | "OPENCODE_SERVER_USERNAME"
            )
        {
            plan.env_notes.push((
                index,
                "This variable is invalid or belongs to cide’s console integration.".into(),
            ));
        } else {
            plan.env.push((name.into(), Some(var.value.clone())));
        }
    }
    plan
}

/// Reuse the executable lookup, with a harness-specific settings hint.
pub fn resolve(binary: &str) -> Result<std::path::PathBuf, String> {
    crate::claude_cli::resolve(binary).map_err(|e| {
        e.message()
            .replace("Claude Code", "OpenCode")
            .replace("No Claude binary", "No OpenCode binary")
            .replace("“claude” is the default", "“opencode” is the default")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reserved_routing_tokens_and_environment_are_filtered() {
        let cli = OpencodeCli {
            args: [
                "--session",
                "ses_other",
                "--server=http://elsewhere",
                "--fork",
                "--log-level",
                "debug",
                "--",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            env: vec![
                cide_ipc::ClaudeEnvVar {
                    name: "OPENCODE_SERVER_PASSWORD".into(),
                    value: "secret".into(),
                },
                cide_ipc::ClaudeEnvVar {
                    name: "OPENAI_BASE_URL".into(),
                    value: "http://gateway".into(),
                },
            ],
            ..Default::default()
        };
        let filtered = plan(&cli);
        assert_eq!(filtered.args, ["--log-level", "debug"]);
        assert_eq!(filtered.env.len(), 1);
        assert_eq!(filtered.env[0].0, "OPENAI_BASE_URL");
        assert_eq!(filtered.arg_notes.len(), 5);
        assert_eq!(filtered.env_notes.len(), 1);
    }
}
