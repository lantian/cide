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
    // OpenCode's port-only editor path omits the lockfile's authorization token.
    // Let its native discovery read the credential; cide routes tools by session.
    plan.env
        .extend(IDE_PORT_ENV.map(|name| (name.into(), None)));
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
            || IDE_PORT_ENV.contains(&name)
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

pub const IDE_PORT_ENV: [&str; 2] = ["CLAUDE_CODE_SSE_PORT", "OPENCODE_EDITOR_SSE_PORT"];

/// Insert editor context into the existing composer without submitting a turn.
/// Keep unsaved text intact; terminal control bytes could end the paste and must be refused.
pub fn editor_context(
    path: &std::path::Path,
    root: Option<&std::path::Path>,
    text: &str,
    start: Option<u32>,
    end: Option<u32>,
) -> Result<Vec<u8>, String> {
    let shown = root
        .and_then(|root| path.strip_prefix(root).ok())
        .unwrap_or(path);
    let lines = match (start, end) {
        (Some(a), Some(b)) if a != b => format!(" (lines {a}-{b})"),
        (Some(a), _) => format!(" (line {a})"),
        _ => String::new(),
    };
    let mut context = format!("@{}{lines} ", shown.display());
    if start.is_some() && !text.is_empty() {
        context.push_str("\n\n");
        context.push_str(text);
    }
    if context
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err("The selected context contains terminal control characters and cannot be pasted safely.".into());
    }
    Ok(format!("\x1b[200~{context}\x1b[201~").into_bytes())
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
    fn editor_ports_cannot_override_authenticated_discovery() {
        let cli = OpencodeCli {
            env: IDE_PORT_ENV
                .iter()
                .map(|name| cide_ipc::ClaudeEnvVar {
                    name: (*name).into(),
                    value: "12345".into(),
                })
                .collect(),
            ..Default::default()
        };
        let launch = plan(&cli);
        assert_eq!(launch.env_notes.len(), 2);
        for name in IDE_PORT_ENV {
            assert!(launch.env.contains(&(name.into(), None)));
            assert!(
                !launch
                    .env
                    .iter()
                    .any(|(key, value)| key == name && value.is_some())
            );
        }
    }

    #[test]
    fn editor_context_preserves_unsaved_lines_without_submitting() {
        let paste = editor_context(
            std::path::Path::new("/repo/src/a.rs"),
            Some(std::path::Path::new("/repo")),
            "  let value = 7;\n\tunsaved();",
            Some(10),
            Some(11),
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(paste).unwrap(),
            "\x1b[200~@src/a.rs (lines 10-11) \n\n  let value = 7;\n\tunsaved();\x1b[201~"
        );
        assert!(
            editor_context(
                std::path::Path::new("/repo/a"),
                None,
                "\x1b[201~\r",
                Some(1),
                Some(1)
            )
            .is_err()
        );
        assert_eq!(
            editor_context(std::path::Path::new("/repo/a"), None, "", None, None).unwrap(),
            b"\x1b[200~@/repo/a \x1b[201~"
        );
    }
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
        assert_eq!(filtered.env.len(), 3);
        assert_eq!(filtered.env[2].0, "OPENAI_BASE_URL");
        assert_eq!(filtered.arg_notes.len(), 5);
        assert_eq!(filtered.env_notes.len(), 1);
    }
}
