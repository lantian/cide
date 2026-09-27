//! What a role may do past a harness's sandbox: commands it runs unasked, resources it needs,
//! directories it may write. (M119)
//!
//! # Why a role needs this at all
//!
//! selfcraft's codex batch after M118: a sprite-artist whose whole pipeline is Blender, and a
//! ui-dev whose every task asks for an e2e capture under Xvfb, both dispatched into codex's
//! workspace-write sandbox, where neither can work — and the only way out cide offered was
//! `permission-mode: bypassPermissions`, which drops the sandbox entirely, needs
//! `agents.allowDangerousPermissions`, and which a Claude orchestrator's own auto-mode classifier
//! refuses to write ("Create Unsafe Agents"). These three keys are the narrow grants between
//! "sandboxed" and "no sandbox at all":
//!
//! * **`allow-commands`** — prefixes run without a prompt. On codex an execpolicy `prefix_rule`
//!   with `decision="allow"`, which codex also runs *unsandboxed* when every segment of the
//!   command matches one (`ExecPolicyManager`: `BypassSandboxFirstAttempt`). On claude,
//!   `Bash(<prefix>:*)` in `--allowedTools`.
//! * **`needs`** — [`SandboxNeed`]s; see its doc for the measurement that ties display and audio
//!   to codex's network switch.
//! * **`writable-dirs`** — extra writable roots, codex's `--add-dir`.
//!
//! The rules for what may be written are here, in one place, because two readers apply them —
//! [`crate::defs::read_definition`], which greys a role a file got wrong, and
//! [`crate::defs::validate`], which refuses a draft from the form or `cide_agent_update` — and a
//! value one of them accepted and the other refused would be a role that saves and then will
//! not run.

use std::path::PathBuf;

pub use cide_ipc::SandboxNeed;

/// A role's grant, as the loader read it. Empty is the ordinary state and means "the harness's
/// own sandbox, unchanged".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SandboxGrant {
    /// As written, one command prefix each. [`command_tokens`] splits one.
    pub allow_commands: Vec<String>,
    pub needs: Vec<SandboxNeed>,
    /// As written; [`SandboxGrant::writable_paths`] expands `~`.
    pub writable_dirs: Vec<String>,
}

impl SandboxGrant {
    pub fn is_empty(&self) -> bool {
        self.allow_commands.is_empty() && self.needs.is_empty() && self.writable_dirs.is_empty()
    }

    pub fn needs(&self, need: SandboxNeed) -> bool {
        self.needs.contains(&need)
    }

    /// Whether codex's sandbox must have its network switch on. Any need does: display and
    /// audio are `AF_UNIX` sockets, which codex's seccomp filter refuses exactly when the
    /// network is off.
    pub fn wants_network(&self) -> bool {
        !self.needs.is_empty()
    }

    /// [`Self::writable_dirs`] as paths, `~` expanded against `home`. An entry that is neither
    /// absolute nor `~/…` is dropped: [`check_writable_dir`] has already greyed the role over it.
    pub fn writable_paths(&self, home: Option<&std::path::Path>) -> Vec<PathBuf> {
        self.writable_dirs
            .iter()
            .filter_map(|dir| expand(dir, home))
            .collect()
    }
}

fn expand(dir: &str, home: Option<&std::path::Path>) -> Option<PathBuf> {
    if let Some(rest) = dir.strip_prefix("~/") {
        return home.map(|home| home.join(rest));
    }
    let path = PathBuf::from(dir);
    path.is_absolute().then_some(path)
}

/// `allow-commands:`'s value, split into entries. Commas only — an entry is a command with
/// arguments, so the space `split_list` also splits on is part of the value here. Brackets are
/// optional, as for every list key.
pub fn split_commands(value: &str) -> Vec<String> {
    let inner = value
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(value);
    inner
        .split(',')
        .map(|entry| entry.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|entry| !entry.is_empty())
        .collect()
}

/// `needs:`/`writable-dirs:`'s value, split into entries (commas or spaces).
pub fn split_words(value: &str) -> Vec<String> {
    let inner = value
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(value);
    inner
        .split([',', ' ', '\t'])
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
}

/// One allowed command's tokens: whitespace-separated, with `'…'` or `"…"` keeping a token's
/// spaces. No escapes — [`check_command`] refuses a backslash, so there is nothing to unescape.
pub fn command_tokens(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for ch in command.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => {
                quote = Some(ch);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    tokens.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, c) => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        tokens.push(current);
    }
    tokens
}

/// First tokens that would allow everything: a shell or a launcher runs whatever follows it, so
/// allowing `bash` or `env` is `bypassPermissions` spelled so nobody notices. `timeout` is here
/// because it is the one selfcraft's runs actually wrap things in.
pub const REFUSED_FIRST: &[&str] = &[
    "sudo", "su", "doas", "sh", "bash", "zsh", "dash", "fish", "env", "timeout", "nohup", "setsid",
    "xargs", "exec", "eval", "command",
];

/// Why `command` cannot be an allowed command, or `Ok`.
pub fn check_command(command: &str) -> Result<(), String> {
    const METACHARACTERS: &[char] = &['|', ';', '&', '$', '`', '>', '<', '\\', '\n', '(', ')'];
    if let Some(bad) = command.chars().find(|c| METACHARACTERS.contains(c)) {
        return Err(format!(
            "`{command}` contains `{bad}`. An allowed command is a program and the arguments it \
             starts with, matched as a prefix — not a shell line; a pipe, a redirect or a `$` \
             would make it match something else than it reads as."
        ));
    }
    if command.matches('\'').count() % 2 == 1 || command.matches('"').count() % 2 == 1 {
        return Err(format!("`{command}` has an unclosed quote."));
    }
    let tokens = command_tokens(command);
    let Some(first) = tokens.first() else {
        return Err("an allowed command cannot be empty.".to_string());
    };
    let program = first.rsplit('/').next().unwrap_or(first);
    if REFUSED_FIRST.contains(&program) {
        return Err(format!(
            "`{command}` would allow everything: `{program}` runs whatever follows it. Allow the \
             program it runs instead — and run it directly, since codex only lets a command out \
             of its sandbox when every part of it is allowed."
        ));
    }
    Ok(())
}

/// A `needs:` word, or why it is not one.
pub fn parse_need(word: &str) -> Result<SandboxNeed, String> {
    SandboxNeed::ALL
        .into_iter()
        .find(|need| need.as_str() == word)
        .ok_or_else(|| {
            format!(
                "`{word}` is not something a role can need. Known: {}.",
                SandboxNeed::ALL.map(SandboxNeed::as_str).join(", ")
            )
        })
}

/// Why `dir` cannot be a writable directory, or `Ok`.
pub fn check_writable_dir(dir: &str) -> Result<(), String> {
    let absolute = dir.starts_with('/') || dir.starts_with("~/");
    if !absolute {
        return Err(format!(
            "`{dir}` is relative. The worktree itself is already writable; a directory outside \
             it is written absolute or from `~/`."
        ));
    }
    let trimmed = dir.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "~" || dir == "~/" {
        return Err(format!(
            "`{dir}` would make the whole {} writable, which is no sandbox at all. Name the \
             directory the work writes.",
            if trimmed.is_empty() {
                "disk"
            } else {
                "home directory"
            }
        ));
    }
    if dir.split('/').any(|part| part == "..") {
        return Err(format!(
            "`{dir}` contains `..`; write the directory it means."
        ));
    }
    // Kernel filesystems. codex hides `.git` and `.codex` inside every writable root it is
    // given, and under `/dev` bwrap cannot create them: measured on 0.157.1, `writable_roots =
    // ["/dev/dri", "/dev/nvidia0", …]` or `["/dev"]` fails **every** command of the run with
    // `bwrap: Can't mkdir /dev/dri/.git: Permission denied` (a device node is not even a
    // directory). selfcraft asked for exactly this to get Vulkan on the GPU; the way that works
    // is `allow-commands`, whose commands run outside the sandbox with the real `/dev`.
    for kernel in ["/dev", "/proc", "/sys"] {
        if trimmed == kernel || trimmed.starts_with(&format!("{kernel}/")) {
            return Err(format!(
                "`{dir}` is under `{kernel}`, which codex's sandbox cannot take as a writable root: \
                 it tries to hide `.git` inside it, fails, and every command of the run fails with \
                 it. For the GPU (`/dev/dri`, `/dev/nvidia*`), allow the command that needs it with \
                 `allow-commands:` instead — it then runs outside the sandbox, with the real devices."
            ));
        }
    }
    Ok(())
}

/// The codex execpolicy rules file for `commands`: one `prefix_rule` each, `decision="allow"`.
/// Format checked against codex 0.157.1's `codex execpolicy check --rules`.
pub fn codex_rules(commands: &[String]) -> String {
    let mut out = String::from(
        "# Written by cide from this role's `allow-commands:` (M119). Regenerated at every spawn.\n",
    );
    for command in commands {
        let tokens = command_tokens(command);
        for spelling in spellings(&tokens) {
            let pattern = spelling
                .iter()
                .map(|token| starlark_string(token))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(
                "prefix_rule(pattern=[{pattern}], decision=\"allow\")\n"
            ));
        }
    }
    out
}

/// Every argv an allowed command may arrive as. codex matches tokens literally, so a script
/// allowed as `tools/ci/runners/e2e.sh` would not match the `./tools/ci/runners/e2e.sh` a model
/// is as likely to type — and would run sandboxed with no word said. A relative path to a
/// program gets both spellings; a bare name (`blender`) or an absolute path gets one.
fn spellings(tokens: &[String]) -> Vec<Vec<String>> {
    let mut out = vec![tokens.to_vec()];
    if let Some(first) = tokens.first()
        && first.contains('/')
        && !first.starts_with('/')
    {
        let other = match first.strip_prefix("./") {
            Some(bare) => bare.to_string(),
            None => format!("./{first}"),
        };
        let mut alt = tokens.to_vec();
        alt[0] = other;
        out.push(alt);
    }
    out
}

/// Where in a checkout cide puts a codex run's rules: codex's project layer, which it reads for
/// a trusted project — measured on 0.157.1 from a linked worktree under `.cide/worktrees/`,
/// with the trust keyed on the main repository's root. Inside codex's own sandbox `.codex` is
/// a tmpfs, so a sandboxed command can neither read this file nor grant itself more by writing
/// one.
pub const CODEX_RULES_FILE: &str = ".codex/rules/cide.rules";

/// Write (or, for no commands, remove) `checkout`'s [`CODEX_RULES_FILE`], and keep it out of
/// every commit through the repository's `info/exclude`. Returns whether a rules file is there
/// now.
///
/// Removed rather than left when the role no longer allows anything: a grant withdrawn in the
/// role file must not go on working in a checkout that a later run of the same role reuses.
/// `info/exclude` rather than `.gitignore` because it is cide's file, not the project's, and a
/// `.gitignore` edit is a change a run would commit. Git reads a linked worktree's excludes
/// from the **common** directory, so the one line covers every checkout — and the main one,
/// where a `.codex/rules/cide.rules` of the user's own would be ignored too; the name is cide's.
pub fn write_codex_rules(
    checkout: &std::path::Path,
    common_dir: Option<&std::path::Path>,
    commands: &[String],
) -> std::io::Result<bool> {
    let path = checkout.join(CODEX_RULES_FILE);
    if commands.is_empty() {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, codex_rules(commands))?;
    if let Some(common) = common_dir {
        let exclude = common.join("info").join("exclude");
        let line = format!("/{CODEX_RULES_FILE}");
        let current = std::fs::read_to_string(&exclude).unwrap_or_default();
        if !current.lines().any(|l| l.trim() == line) {
            if let Some(parent) = exclude.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut text = current;
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str("# cide: a codex run's allowed commands (M119)\n");
            text.push_str(&line);
            text.push('\n');
            std::fs::write(&exclude, text)?;
        }
    }
    Ok(true)
}

fn starlark_string(token: &str) -> String {
    let mut out = String::from("\"");
    for ch in token.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `--allowedTools` entries for claude: `Bash(<prefix>:*)`.
pub fn claude_allowed_tools(commands: &[String]) -> Vec<String> {
    commands
        .iter()
        .map(|command| format!("Bash({}:*)", command_tokens(command).join(" ")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_split_on_commas_only() {
        assert_eq!(
            split_commands("[blender -b,  tools/ci/runners/e2e.sh , godot --headless]"),
            vec!["blender -b", "tools/ci/runners/e2e.sh", "godot --headless"]
        );
    }

    #[test]
    fn a_quoted_token_keeps_its_space() {
        assert_eq!(
            command_tokens(r#"blender -b --python "my worker.py""#),
            vec!["blender", "-b", "--python", "my worker.py"]
        );
    }

    #[test]
    fn a_shell_line_or_a_launcher_is_refused() {
        assert!(check_command("blender -b").is_ok());
        assert!(check_command("tools/ci/runners/e2e.sh").is_ok());
        for bad in [
            "bash",
            "/usr/bin/env python3",
            "timeout 55s blender",
            "a | b",
            "rm $X",
            "sh -c x",
        ] {
            assert!(check_command(bad).is_err(), "{bad}");
        }
        assert!(check_command("blender \"open").is_err());
    }

    #[test]
    fn needs_are_the_three_words() {
        assert_eq!(parse_need("display"), Ok(SandboxNeed::Display));
        assert!(
            parse_need("gpu")
                .unwrap_err()
                .contains("display, audio, network")
        );
    }

    #[test]
    fn a_writable_dir_is_somewhere_in_particular() {
        assert!(check_writable_dir("~/.cache/godot").is_ok());
        assert!(check_writable_dir("/srv/assets").is_ok());
        for bad in [
            "/",
            "~",
            "~/",
            "cache",
            "/srv/../etc",
            "/dev",
            "/dev/dri",
            "/sys/class",
        ] {
            assert!(check_writable_dir(bad).is_err(), "{bad}");
        }
        let grant = SandboxGrant {
            writable_dirs: vec!["~/.cache/godot".into(), "/srv".into()],
            ..SandboxGrant::default()
        };
        assert_eq!(
            grant.writable_paths(Some(std::path::Path::new("/home/u"))),
            vec![PathBuf::from("/home/u/.cache/godot"), PathBuf::from("/srv")]
        );
    }

    #[test]
    fn the_rules_file_is_one_prefix_rule_per_command() {
        let rules = codex_rules(&["blender -b".into(), "tools/ci/runners/e2e.sh".into()]);
        assert!(rules.contains(r#"prefix_rule(pattern=["blender", "-b"], decision="allow")"#));
        assert!(
            rules.contains(r#"prefix_rule(pattern=["tools/ci/runners/e2e.sh"], decision="allow")"#)
        );
        // …and as `./tools/…`, which codex would otherwise not match. (M119 follow-up)
        assert!(
            rules.contains(
                r#"prefix_rule(pattern=["./tools/ci/runners/e2e.sh"], decision="allow")"#
            )
        );
        assert_eq!(
            rules.matches("\"blender\"").count(),
            1,
            "a bare name has one spelling"
        );
    }

    #[test]
    fn the_rules_file_is_written_excluded_and_withdrawn() {
        let dir = std::env::temp_dir().join(format!("cide-rules-{}", std::process::id()));
        let checkout = dir.join("wt");
        let common = dir.join("common");
        std::fs::create_dir_all(&checkout).expect("mkdir");
        assert!(
            write_codex_rules(&checkout, Some(&common), &["blender -b".into()]).expect("write")
        );
        assert!(checkout.join(CODEX_RULES_FILE).is_file());
        // Twice: the exclude line is added once.
        write_codex_rules(&checkout, Some(&common), &["blender -b".into()]).expect("write");
        let exclude = std::fs::read_to_string(common.join("info/exclude")).expect("exclude");
        assert_eq!(
            exclude.matches("/.codex/rules/cide.rules").count(),
            1,
            "{exclude}"
        );
        assert!(!write_codex_rules(&checkout, Some(&common), &[]).expect("withdraw"));
        assert!(!checkout.join(CODEX_RULES_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claude_gets_bash_prefixes() {
        assert_eq!(
            claude_allowed_tools(&["blender -b".into()]),
            vec!["Bash(blender -b:*)"]
        );
    }
}
