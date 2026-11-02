//! Narrow permission grants for the task bridge cide attaches to its workers.

/// Allow task discovery and reporting without changing the execution mode.
/// No other task mutations or MCP tools are granted.
/// Call only when cide has attached its own MCP bridge, never for a user server.
pub fn allow_worker_task_tools(args: &mut Vec<String>) {
    let mut allowed = Vec::new();
    let mut remaining = Vec::new();
    let mut input = std::mem::take(args).into_iter().peekable();
    while let Some(arg) = input.next() {
        if matches!(arg.as_str(), "--allowedTools" | "--allowed-tools") {
            while input.peek().is_some_and(|value| !value.starts_with('-')) {
                allowed.push(input.next().unwrap());
            }
        } else if let Some(value) = arg
            .strip_prefix("--allowedTools=")
            .or_else(|| arg.strip_prefix("--allowed-tools="))
        {
            allowed.push(value.to_owned());
        } else {
            remaining.push(arg);
        }
    }
    for tool in [
        "mcp__cide__cide_task_get",
        "mcp__cide__cide_task_list",
        "mcp__cide__cide_task_update",
        "mcp__cide__cide_task_comment",
    ] {
        if !allowed.iter().any(|value| value == tool) {
            allowed.push(tool.into());
        }
    }
    remaining.push("--allowedTools".into());
    remaining.extend(allowed);
    *args = remaining;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_grants_merge_without_changing_denies_or_modes() {
        let mut args: Vec<String> = [
            "--allowedTools=Read",
            "--permission-mode",
            "default",
            "--allowed-tools",
            "Bash(cargo test:*)",
            "--disallowedTools",
            "mcp__cide__cide_task_get",
            "--settings",
            "{\"theme\":\"light\"}",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        allow_worker_task_tools(&mut args);
        let once = args.clone();
        allow_worker_task_tools(&mut args);
        assert_eq!(args, once);
        assert_eq!(
            args,
            [
                "--permission-mode",
                "default",
                "--disallowedTools",
                "mcp__cide__cide_task_get",
                "--settings",
                "{\"theme\":\"light\"}",
                "--allowedTools",
                "Read",
                "Bash(cargo test:*)",
                "mcp__cide__cide_task_get",
                "mcp__cide__cide_task_list",
                "mcp__cide__cide_task_update",
                "mcp__cide__cide_task_comment",
            ]
        );
    }
}
