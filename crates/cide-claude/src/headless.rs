//! The non-interactive lane: one prompt in, one answer out.
//!
//! Used for the jobs that want a language model but not a conversation — generating a commit
//! message from a staged diff, explaining a selection, a palette one-shot. Nothing here
//! allocates a PTY, registers a hook, resumes a session or touches the pane tree.
//!
//! # `--bare` is forbidden, and this is the one rule that would bite hardest
//!
//! `claude --help` on 2.1.226, verbatim:
//!
//! > `--bare`  Minimal mode: skip hooks, LSP, plugin sync, attribution, auto-memory,
//! > background prefetches, keychain reads, and CLAUDE.md auto-discovery. Sets
//! > `CLAUDE_CODE_SIMPLE=1`. **Anthropic auth is strictly `ANTHROPIC_API_KEY` or
//! > `apiKeyHelper` via `--settings` (OAuth and keychain are never read).**
//!
//! It reads superficially like exactly what a one-shot wants: no hooks, no plugins, no
//! CLAUDE.md. It is also the one flag that makes the run impossible for the users this app
//! is built for. A Claude Max subscriber authenticates through OAuth held in the keychain;
//! `--bare` refuses to read it and the run fails with an authentication error that has
//! nothing to do with what the user did. [`argv`] therefore never emits it and a test asserts
//! as much, because the flag is attractive enough that somebody will try it again.
//!
//! The sibling rule from [`crate`] holds here too and for the same reason: this module never
//! sets `ANTHROPIC_API_KEY` and never reads `~/.claude/.credentials.json`. The child
//! inherits its authentication by inheriting the environment.
//!
//! # The output envelope
//!
//! Documented, with the run it was read from, on [`cide_ipc::headless`]. Short version:
//! `--output-format json` prints a JSON **array** of transcript frames despite the help text
//! saying "single result", and the answer is the last frame with `"type":"result"`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cide_core::proxy::ProxyEnv;
use cide_ipc::{HeadlessError, HeadlessRequest, HeadlessResult};

/// How long a one-shot may run before it is killed.
///
/// Generous because the work is real — a commit message over a large diff is a genuine
/// model turn — but bounded, because the caller is a Tauri command holding a worker thread
/// and there is no UI in this app that could show a user a run that never ends.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);

/// How much of a child's stderr travels in an error. Enough to name the cause; not enough to
/// paste a user's environment into a toast.
const STDERR_KEEP: usize = 400;

/// How much of an unreadable stdout travels in [`HeadlessError::Malformed`].
const HEAD_KEEP: usize = 400;

/// Which built-in tools the child may use.
///
/// The default is [`ToolAccess::None`], and that is a security posture rather than a
/// performance one. Both named uses of this lane put everything the model needs *in the
/// prompt* — the staged diff, the selected text — so tools buy nothing, while a palette
/// one-shot that can reach `Bash` is an unattended process running commands in the user's
/// repository with no turn for them to approve. Callers that genuinely need a tool ask for
/// it by name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ToolAccess {
    /// `--tools ""`.
    #[default]
    None,
    /// Leave the CLI's own default tool set in place.
    Inherit,
    /// Exactly these built-in tools.
    Only(Vec<String>),
}

/// A configured one-shot: the request plus how to run it.
#[derive(Debug, Clone)]
pub struct Headless {
    pub request: HeadlessRequest,
    /// Working directory. Decides which `CLAUDE.md` and settings the CLI picks up, so it
    /// should be the project root the prompt is about.
    pub cwd: PathBuf,
    pub tools: ToolAccess,
    pub timeout: Duration,
    /// The proxy environment this one-shot is spawned with.
    ///
    /// # This lane had no proxy at all until it had this field
    ///
    /// `ProxySettings` was described in its own doc as applying to "every child cide spawns",
    /// and it did not: the only production caller of the proxy pass was `session_spawn`. This
    /// lane builds its own [`Command`] and inherited cide's raw environment, so a corporate
    /// user whose panes worked through a configured proxy got a commit-message generation
    /// that tried to reach the API directly and hung. Not "not implemented" — the rule was a
    /// pure function the whole time — but never wired, which is the same outcome.
    ///
    /// Handed in as a resolved [`ProxyEnv`] rather than as `ProxySettings`, because this crate
    /// must not have an opinion about who is in scope. The caller reads
    /// `ProxyScope::claude` — a one-shot **is a claude**, and a user who takes `claude` out of
    /// scope means both — and this applies whatever it is given.
    pub proxy: ProxyEnv,
    /// The user's own environment additions, already filtered. (M16)
    ///
    /// Same argument as [`Self::proxy`] one field up, and the same shape: this crate must not
    /// have an opinion about which of the user's variables are allowed — that rule is
    /// `cide_core::claude_cli`, which the caller runs — and this applies whatever it is given.
    ///
    /// It reaches this lane and not only panes because `ProxyScope::claude` already settled the
    /// question for its own field: a one-shot **is a claude**, and a user who points `claude` at
    /// a gateway or a cloud provider means the commit-message generation too. The *arguments*
    /// deliberately do not travel; see `cide_app::cmd::settings::run_headless`.
    pub env: Vec<(String, Option<String>)>,
}

impl Headless {
    pub fn new(request: HeadlessRequest, cwd: impl Into<PathBuf>) -> Self {
        Self {
            request,
            cwd: cwd.into(),
            tools: ToolAccess::default(),
            timeout: DEFAULT_TIMEOUT,
            // Empty: cide sets nothing and removes nothing. The same default the pane lane
            // has for a workspace it cannot read, and the behaviour this lane had before the
            // field existed — so a caller that forgets to set it is no worse off than before,
            // rather than silently direct.
            proxy: ProxyEnv::default(),
            env: Vec::new(),
        }
    }

    pub fn env(mut self, env: Vec<(String, Option<String>)>) -> Self {
        self.env = env;
        self
    }

    pub fn tools(mut self, tools: ToolAccess) -> Self {
        self.tools = tools;
        self
    }

    /// The proxy environment for this run. See [`Self::proxy`].
    pub fn proxy(mut self, proxy: ProxyEnv) -> Self {
        self.proxy = proxy;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// The argument vector for one run, excluding the program itself.
///
/// A named function with tests rather than a run of `.arg()` calls at the call site: every
/// mistake available here is silent. A missing `--output-format json` yields prose that the
/// parser rejects as malformed; a stray `--bare` yields an authentication failure for every
/// subscription user; and the prompt is deliberately *not* in this vector at all.
///
/// The prompt goes on stdin. `-p` is a boolean flag and the prompt is a positional argument,
/// so `claude -p "<prompt>"` works — right up until the prompt embeds a staged diff and
/// passes `ARG_MAX`, or begins with `-` and is parsed as a flag. Both were verified to be
/// avoidable: with no positional argument the CLI reads the prompt from stdin, which is the
/// documented "useful for pipes" path.
pub fn argv(run: &Headless) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        "--output-format".to_string(),
        "json".to_string(),
        // A commit message is not a conversation. Without this every one-shot files a
        // transcript, and the `/resume` picker fills with entries the user never had.
        "--no-session-persistence".to_string(),
    ];

    if let Some(model) = &run.request.model {
        args.push("--model".into());
        args.push(model.clone());
    }

    if let Some(schema) = &run.request.json_schema {
        args.push("--json-schema".into());
        args.push(schema.to_string());
    }

    if let Some(budget) = &run.request.max_budget_usd {
        args.push("--max-budget-usd".into());
        args.push(budget.to_string());
    }

    // `--tools` is variadic, so it collects every following token that does not look like an
    // option. Putting it last means there is nothing left for it to swallow — with
    // `--tools "" --model opus`, commander stops at `--model`, but relying on that is a
    // wager on another tool's parser that costs nothing to avoid.
    match &run.tools {
        ToolAccess::None => {
            args.push("--tools".into());
            args.push(String::new());
        }
        ToolAccess::Only(names) => {
            args.push("--tools".into());
            args.extend(names.iter().cloned());
        }
        ToolAccess::Inherit => {}
    }

    args
}

/// Run one prompt to completion.
///
/// Blocking. The caller is expected to be off the UI thread — in `cide-app` that is
/// `spawn_blocking`.
pub fn run(program: &Path, run: &Headless) -> Result<HeadlessResult, HeadlessError> {
    let mut command = Command::new(program);
    command
        .args(argv(run))
        .current_dir(&run.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    scrub_env(&mut command);
    // After `scrub_env`, so a scrub cannot undo what the user asked for. The passes answer
    // different questions and barely overlap: `scrub_env` removes cide's own interactive
    // variables (`CLAUDE_CODE_SSE_PORT`, `CIDE_HOOK_SOCK`, `TERM`, `COLUMNS`, `LINES`), and
    // every one of those names is refused by `cide_core::claude_cli` — so a list that reaches
    // here cannot resurrect one of them.
    //
    // Before the proxy, matching the pane lane's order in `cmd::session.rs::base_env`: the
    // proxy screen is the authority on proxy variables and the launch configuration refuses
    // them by name, so the two cannot collide — but the *order* being the same in both lanes is
    // what makes that one sentence true of cide rather than of one spawn site.
    for (name, value) in &run.env {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    run.proxy.apply(&mut command);
    // A one-shot outliving a `SIGKILL`ed cide is a `claude` nobody can see, spending money
    // against a prompt nobody will read the answer to. `arm` requires that the thread doing
    // the spawn outlives the child, which this function satisfies by construction rather than
    // by convention: the same thread blocks in `wait_with_deadline` below until the child is
    // gone. See `crate::orphans`.
    crate::orphans::arm(&mut command);

    let mut child = command.spawn().map_err(|e| HeadlessError::NotInstalled {
        detail: e.to_string(),
    })?;

    // stdin, stdout and stderr are all drained on their own threads before anything waits.
    // Doing any of it inline deadlocks in a way that only shows up on large inputs: a prompt
    // bigger than the pipe buffer (64 KiB here) blocks our write, while the child blocks
    // writing output nobody is reading. A commit-message prompt is exactly that size.
    let prompt = run.request.prompt.clone();
    let mut stdin = child.stdin.take();
    let writer = std::thread::spawn(move || {
        if let Some(mut pipe) = stdin.take() {
            // A broken pipe here means the child gave up on us — it will be reported by the
            // exit status, and there is nothing better to do with the error at this end.
            let _ = pipe.write_all(prompt.as_bytes());
        }
        // Dropped, and so closed: with the pipe still open the child waits for more prompt.
    });

    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());

    let status = wait_with_deadline(&mut child, run.timeout)?;
    let _ = writer.join();
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();

    // Parsed before the exit status is judged. A CLI that answered and then exited non-zero
    // has still answered, and `error_max_turns` arrives exactly that way — reporting it as
    // "claude exited 1" would throw away the text, the cost and the reason.
    match parse(&stdout) {
        Ok(result) => Ok(result),
        Err(malformed) => {
            if status.success() {
                Err(malformed)
            } else {
                Err(HeadlessError::Exited {
                    code: status.code(),
                    stderr: tail(&stderr, STDERR_KEEP),
                })
            }
        }
    }
}

/// Read a captured pipe to the end on its own thread.
fn drain(pipe: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut pipe) = pipe {
            let mut raw = Vec::new();
            let _ = pipe.read_to_end(&mut raw);
            text = String::from_utf8_lossy(&raw).into_owned();
        }
        text
    })
}

/// Wait for the child, killing it if it outlives `timeout`.
///
/// Polling rather than a watchdog thread. The alternatives both fail on their own terms:
/// `wait_with_output` consumes the `Child`, leaving nothing to kill, and sharing the child
/// behind a mutex deadlocks because the waiting thread holds the lock the killer needs. A
/// 50 ms poll is invisible next to a call that takes seconds and needs no synchronisation
/// at all.
fn wait_with_deadline(
    child: &mut Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus, HeadlessError> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(e) => {
                return Err(HeadlessError::NotInstalled {
                    detail: e.to_string(),
                });
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(HeadlessError::TimedOut {
                seconds: timeout.as_secs(),
            });
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Remove the variables that make a child part of the *interactive* app.
///
/// Every one of these is set for panes and is wrong here:
///
/// * `CLAUDE_CODE_SSE_PORT` and `CLAUDE_CODE_AUTO_CONNECT_IDE` would connect this one-shot to
///   our IDE MCP server. It could then call `openDiff`, which blocks the agent's turn on a
///   human answering a tab — and there is no pane behind a commit-message generation for the
///   answer to come from. The run would hang until it timed out.
/// * `CIDE_HOOK_SOCK` would make it report `SessionStart`/`Stop` frames into the session
///   state machine, moving the status bar and the close confirmation for a session no pane
///   owns.
///
/// `ANTHROPIC_API_KEY` is deliberately *not* touched. Never setting one is the rule; removing
/// one the user set themselves would break the Console customers for whom it is the only
/// credential they have.
///
/// [`cide_core::child_env::prepare_command`] runs first and answers a different question — not
/// "which of cide's variables are wrong for a one-shot" but "which of them were never cide's
/// to pass on". A `claude` launched from the AppImage inherits a `PYTHONHOME` naming a prefix
/// with no Python in it, which kills every stdio MCP server this run would have loaded.
fn scrub_env(command: &mut Command) {
    cide_core::child_env::prepare_command(command);
    command
        .env_remove("CLAUDE_CODE_SSE_PORT")
        .env_remove("CLAUDE_CODE_AUTO_CONNECT_IDE")
        .env_remove("CIDE_HOOK_SOCK")
        // Not a terminal. Left set, the CLI's own environment probes see a TTY-shaped world
        // that the pipes contradict.
        .env_remove("TERM")
        .env_remove("COLUMNS")
        .env_remove("LINES");
}

/// Pull the answer out of what `--output-format json` printed.
///
/// Accepts both shapes: the array of transcript frames the CLI actually emits, and the single
/// object its `--help` claims. Within an array the **last** frame of `"type":"result"` wins;
/// frames are not positional, and two runs of the same command were observed to begin with
/// different ones.
pub fn parse(stdout: &str) -> Result<HeadlessResult, HeadlessError> {
    let text = stdout.trim();
    if text.is_empty() {
        return Err(HeadlessError::Malformed {
            detail: "no output".into(),
            head: String::new(),
        });
    }

    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| HeadlessError::Malformed {
            detail: e.to_string(),
            head: head(text, HEAD_KEEP),
        })?;

    let frame = match &value {
        serde_json::Value::Array(frames) => frames
            .iter()
            .rev()
            .find(|f| f.get("type").and_then(|t| t.as_str()) == Some("result")),
        serde_json::Value::Object(_) => Some(&value),
        _ => None,
    };

    let Some(frame) = frame else {
        return Err(HeadlessError::Malformed {
            detail: "no frame of type `result` in the output".into(),
            head: head(text, HEAD_KEEP),
        });
    };

    let str_field = |name: &str| frame.get(name).and_then(|v| v.as_str());
    let Some(answer) = str_field("result") else {
        return Err(HeadlessError::Malformed {
            detail: "the result frame carries no `result` field".into(),
            head: head(text, HEAD_KEEP),
        });
    };

    Ok(HeadlessResult {
        text: answer.to_string(),
        // Parsed opportunistically. A schema was either not asked for, or asked for and
        // honoured, or asked for and not honoured — and in the last case the caller still
        // gets the prose, which is more useful than an error saying the shape was wrong.
        structured: serde_json::from_str(answer).ok(),
        is_error: frame
            .get("is_error")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        subtype: str_field("subtype").unwrap_or("unknown").to_string(),
        session: str_field("session_id").map(str::to_string),
        cost_usd: frame.get("total_cost_usd").and_then(|v| v.as_f64()),
        duration_ms: frame.get("duration_ms").and_then(|v| v.as_u64()),
        num_turns: frame
            .get("num_turns")
            .and_then(|v| v.as_u64())
            .and_then(|n| u32::try_from(n).ok()),
    })
}

/// The first `keep` characters, on a character boundary.
fn head(text: &str, keep: usize) -> String {
    text.chars().take(keep).collect()
}

/// The last `keep` characters, on a character boundary.
fn tail(text: &str, keep: usize) -> String {
    let text = text.trim();
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(keep)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_of(request: HeadlessRequest) -> Headless {
        Headless::new(request, "/tmp")
    }

    #[test]
    fn the_bare_flag_is_never_emitted() {
        // The one regression this module exists to prevent: `--bare` refuses OAuth and
        // keychain reads, so every Claude Max user's one-shot would fail to authenticate.
        let mut request = HeadlessRequest::new("hello");
        request.model = Some("opus".into());
        request.max_budget_usd = Some(0.5);
        request.json_schema = Some(serde_json::json!({"type": "object"}));
        let args = argv(&run_of(request).tools(ToolAccess::Inherit));
        assert!(!args.iter().any(|a| a == "--bare"), "{args:?}");
    }

    #[test]
    fn the_prompt_is_not_in_the_argument_vector() {
        // It goes on stdin: a commit-message prompt embeds a diff and can pass ARG_MAX, and
        // one starting with `-` would be read as a flag.
        let args = argv(&run_of(HeadlessRequest::new("--dangerous looking prompt")));
        assert!(
            !args.iter().any(|a| a.contains("dangerous looking")),
            "{args:?}"
        );
    }

    #[test]
    fn json_output_is_always_requested() {
        let args = argv(&run_of(HeadlessRequest::new("hi")));
        let at = args
            .iter()
            .position(|a| a == "--output-format")
            .expect("set");
        assert_eq!(args[at + 1], "json");
        assert!(args.iter().any(|a| a == "-p"));
        assert!(args.iter().any(|a| a == "--no-session-persistence"));
    }

    #[test]
    fn tools_are_disabled_by_default_and_come_last() {
        let args = argv(&run_of(HeadlessRequest::new("hi")));
        assert_eq!(args[args.len() - 2], "--tools");
        assert_eq!(args[args.len() - 1], "");
    }

    #[test]
    fn inheriting_tools_emits_no_tools_flag() {
        let args = argv(&run_of(HeadlessRequest::new("hi")).tools(ToolAccess::Inherit));
        assert!(!args.iter().any(|a| a == "--tools"), "{args:?}");
    }

    #[test]
    fn named_tools_are_passed_through() {
        let args = argv(
            &run_of(HeadlessRequest::new("hi"))
                .tools(ToolAccess::Only(vec!["Read".into(), "Grep".into()])),
        );
        assert_eq!(&args[args.len() - 3..], ["--tools", "Read", "Grep"]);
    }

    #[test]
    fn a_schema_is_serialised_as_one_json_argument() {
        let mut request = HeadlessRequest::new("hi");
        request.json_schema = Some(serde_json::json!({"type": "object"}));
        let args = argv(&run_of(request));
        let at = args.iter().position(|a| a == "--json-schema").expect("set");
        assert_eq!(args[at + 1], r#"{"type":"object"}"#);
    }

    /// The real envelope, trimmed of the fields this module does not read. Captured from
    /// `claude -p --output-format json` on 2.1.226 — see the module docs.
    const REAL_OUTPUT: &str = r#"[
      {"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}},
      {"type":"assistant","message":{"content":[{"type":"text","text":"ok2"}]}},
      {"is_error":false,"num_turns":1,"session_id":"b1c56321-58a7-49a5-aa8e-35aa77fec9bc",
       "total_cost_usd":0.0772495,"subtype":"success","result":"ok2","type":"result",
       "duration_ms":4552}
    ]"#;

    #[test]
    fn the_real_envelope_parses() {
        let result = parse(REAL_OUTPUT).expect("the captured output parses");
        assert_eq!(result.text, "ok2");
        assert!(!result.is_error);
        assert_eq!(result.subtype, "success");
        assert_eq!(result.num_turns, Some(1));
        assert_eq!(result.duration_ms, Some(4552));
        assert_eq!(
            result.session.as_deref(),
            Some("b1c56321-58a7-49a5-aa8e-35aa77fec9bc")
        );
        assert!(result.cost_usd.is_some_and(|c| c > 0.0));
        // Prose is not structured output, and pretending otherwise would hand a caller a
        // JSON string where it expected an object.
        assert_eq!(result.structured, None);
    }

    #[test]
    fn a_bare_result_object_parses_too() {
        // What `--help` claims the format is. Accepted so that a release which makes the
        // help text true does not break this lane.
        let result = parse(r#"{"type":"result","subtype":"success","result":"hi"}"#).expect("ok");
        assert_eq!(result.text, "hi");
    }

    #[test]
    fn the_last_result_frame_wins() {
        let out = r#"[{"type":"result","result":"first","subtype":"success"},
                      {"type":"result","result":"second","subtype":"success"}]"#;
        assert_eq!(parse(out).expect("ok").text, "second");
    }

    #[test]
    fn structured_output_is_parsed_when_the_answer_is_json() {
        let out = r#"[{"type":"result","subtype":"success","result":"{\"summary\":\"fix\"}"}]"#;
        let result = parse(out).expect("ok");
        assert_eq!(
            result.structured,
            Some(serde_json::json!({"summary": "fix"}))
        );
    }

    #[test]
    fn a_failed_run_still_reports_its_text_and_cost() {
        let out = r#"[{"type":"result","subtype":"error_max_turns","is_error":true,
                       "result":"partial","total_cost_usd":0.01}]"#;
        let result = parse(out).expect("a failed run is still a readable result");
        assert!(result.is_error);
        assert_eq!(result.subtype, "error_max_turns");
        assert_eq!(result.text, "partial");
    }

    #[test]
    fn prose_output_is_malformed_rather_than_silently_empty() {
        let err = parse("I could not do that").expect_err("prose is not the json envelope");
        assert!(matches!(err, HeadlessError::Malformed { .. }));
    }

    #[test]
    fn an_array_without_a_result_frame_is_malformed() {
        let err = parse(r#"[{"type":"system","subtype":"init"}]"#).expect_err("no result frame");
        let HeadlessError::Malformed { detail, .. } = err else {
            panic!("expected Malformed");
        };
        assert!(detail.contains("result"), "{detail}");
    }

    #[test]
    fn empty_output_is_malformed() {
        assert!(matches!(
            parse("   \n"),
            Err(HeadlessError::Malformed { .. })
        ));
    }

    #[test]
    fn truncation_respects_character_boundaries() {
        // A panic here would turn a bad-output report into a crash inside the error path.
        assert_eq!(head("héllo", 3), "hél");
        assert_eq!(tail("héllo", 3), "llo");
        assert_eq!(tail("hé", 10), "hé");
    }
}

// ==========================================================================================
// The same lane on codex. (M93)
// ==========================================================================================

/// The one-shot lane on codex, for a user whose Settings → Harness is Codex: `codex exec`, one
/// turn, the prompt on stdin, the answer read off its JSONL. (M93)
///
/// The lane is "cide asks a model one thing" — a commit message — and a user who chose codex as
/// their console should not need claude installed for it. `exec`'s shape was measured in M44
/// (`cide_agents::harness::codex`'s history): `thread.started`, `turn.started`,
/// `item.completed` carrying the agent's message as `item.type == "agent_message"`,
/// `turn.completed` or `turn.failed`/`error` last.
///
/// * `--ephemeral`: no rollout on disk, the `--no-session-persistence` of this lane.
/// * `-s read-only`: the lane's `ToolAccess::None` — codex has no "no tools" switch, and a
///   read-only sandbox is the closest thing, since reading is what the model may want to do.
///   `Inherit` and `Only` keep the user's own sandbox.
/// * the schema, when there is one, goes to `--output-schema <file>`, which is the one flag here
///   that wants a file: written beside nothing else, removed afterwards.
/// * no budget: codex has no `--max-budget-usd`.
pub fn codex_argv(run: &Headless, schema_file: Option<&Path>) -> Vec<String> {
    let mut args = vec![
        "exec".to_string(),
        "--json".into(),
        "--ephemeral".into(),
        "--skip-git-repo-check".into(),
        "--color".into(),
        "never".into(),
        "-C".into(),
        run.cwd.to_string_lossy().into_owned(),
    ];
    args.extend(cide_core::codex_cli::quiet_start());
    if matches!(run.tools, ToolAccess::None) {
        args.push("-s".into());
        args.push("read-only".into());
    }
    if let Some(model) = &run.request.model {
        args.push("-m".into());
        args.push(model.clone());
    }
    if let Some(file) = schema_file {
        args.push("--output-schema".into());
        args.push(file.to_string_lossy().into_owned());
    }
    args
}

/// [`run`], on codex. Same environment rules, same deadline, same errors.
pub fn run_codex(program: &Path, run: &Headless) -> Result<HeadlessResult, HeadlessError> {
    let schema_file = match &run.request.json_schema {
        Some(schema) => {
            let file = std::env::temp_dir().join(format!(
                "cide-codex-schema-{}-{}.json",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::write(&file, schema.to_string()).map_err(|e| HeadlessError::NotInstalled {
                detail: format!("could not write the output schema for codex: {e}"),
            })?;
            Some(file)
        }
        None => None,
    };
    let result = run_codex_with(program, run, schema_file.as_deref());
    if let Some(file) = schema_file {
        let _ = std::fs::remove_file(file);
    }
    result
}

fn run_codex_with(
    program: &Path,
    run: &Headless,
    schema_file: Option<&Path>,
) -> Result<HeadlessResult, HeadlessError> {
    let mut command = Command::new(program);
    command
        .args(codex_argv(run, schema_file))
        .current_dir(&run.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    scrub_env(&mut command);
    for (name, value) in &run.env {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    run.proxy.apply(&mut command);
    crate::orphans::arm(&mut command);

    let mut child = command.spawn().map_err(|e| HeadlessError::NotInstalled {
        detail: e.to_string(),
    })?;
    // No positional prompt: `exec` reads its instructions from stdin when given none, which
    // keeps a long diff out of the argv (and out of `ps`).
    let prompt = run.request.prompt.clone();
    let mut stdin = child.stdin.take();
    let writer = std::thread::spawn(move || {
        if let Some(mut pipe) = stdin.take() {
            let _ = pipe.write_all(prompt.as_bytes());
        }
    });
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let status = wait_with_deadline(&mut child, run.timeout)?;
    let _ = writer.join();
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();

    match parse_codex(&stdout, run.request.json_schema.is_some()) {
        Ok(result) => Ok(result),
        Err(malformed) => {
            if status.success() {
                Err(malformed)
            } else {
                Err(HeadlessError::Exited {
                    code: status.code(),
                    stderr: tail(&stderr, STDERR_KEEP),
                })
            }
        }
    }
}

/// The answer in a `codex exec --json` stream: the **last** agent message, the thread, and
/// whether the turn failed. `structured` is the message parsed as JSON when a schema was asked
/// for, which is what `--output-schema` makes the message.
pub fn parse_codex(stdout: &str, structured: bool) -> Result<HeadlessResult, HeadlessError> {
    let mut text: Option<String> = None;
    let mut thread: Option<String> = None;
    let mut failure: Option<String> = None;
    let mut completed = false;
    for line in stdout.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match event.get("type").and_then(|t| t.as_str()) {
            Some("thread.started") => {
                thread = event
                    .get("thread_id")
                    .and_then(|t| t.as_str())
                    .map(str::to_string);
            }
            Some("item.completed") => {
                let item = event.get("item");
                if item.and_then(|i| i.get("type")).and_then(|t| t.as_str())
                    == Some("agent_message")
                    && let Some(said) = item.and_then(|i| i.get("text")).and_then(|t| t.as_str())
                {
                    text = Some(said.to_string());
                }
            }
            Some("turn.completed") => completed = true,
            Some("turn.failed") | Some("error") => {
                failure = event
                    .get("error")
                    .and_then(|e| e.get("message").or(Some(e)))
                    .or_else(|| event.get("message"))
                    .map(|m| {
                        m.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| m.to_string())
                    });
            }
            _ => {}
        }
    }
    if text.is_none() && failure.is_none() && !completed {
        return Err(HeadlessError::Malformed {
            detail: "codex printed no agent message and no turn end".to_string(),
            head: stdout.chars().take(HEAD_KEEP).collect(),
        });
    }
    let text = text.or_else(|| failure.clone()).unwrap_or_default();
    Ok(HeadlessResult {
        structured: if structured {
            serde_json::from_str(&text).ok()
        } else {
            None
        },
        is_error: failure.is_some(),
        subtype: if failure.is_some() {
            "error"
        } else {
            "success"
        }
        .to_string(),
        session: thread,
        cost_usd: None,
        duration_ms: None,
        num_turns: Some(1),
        text,
    })
}

#[cfg(test)]
mod codex_tests {
    use super::*;

    #[test]
    fn the_codex_lane_is_one_ephemeral_read_only_exec_with_the_prompt_on_stdin() {
        let mut request = HeadlessRequest::new("write a commit message");
        request.model = Some("gpt-5.5".into());
        let run = Headless::new(request, "/repo");
        let args = codex_argv(&run, Some(Path::new("/tmp/s.json")));
        assert_eq!(&args[..3], ["exec", "--json", "--ephemeral"]);
        assert!(args.windows(2).any(|w| w == ["-s", "read-only"]));
        assert!(args.windows(2).any(|w| w == ["-C", "/repo"]));
        assert!(args.windows(2).any(|w| w == ["-m", "gpt-5.5"]));
        assert!(
            args.windows(2)
                .any(|w| w == ["--output-schema", "/tmp/s.json"])
        );
        assert!(
            !args.iter().any(|a| a.contains("commit message")),
            "{args:?}"
        );
    }

    #[test]
    fn the_last_agent_message_is_the_answer() {
        let stream = r#"{"type":"thread.started","thread_id":"t-1"}
{"type":"turn.started"}
{"type":"item.completed","item":{"id":"i0","type":"reasoning","text":"thinking"}}
{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"first"}}
{"type":"item.completed","item":{"id":"i2","type":"agent_message","text":"{\"subject\":\"Fix it\"}"}}
{"type":"turn.completed","usage":{"input_tokens":1}}"#;
        let result = parse_codex(stream, true).expect("parsed");
        assert_eq!(result.text, "{\"subject\":\"Fix it\"}");
        assert_eq!(
            result.structured,
            Some(serde_json::json!({"subject": "Fix it"}))
        );
        assert_eq!(result.session.as_deref(), Some("t-1"));
        assert!(!result.is_error);

        let failed = parse_codex(
            r#"{"type":"turn.failed","error":{"message":"quota exceeded"}}"#,
            false,
        )
        .expect("a failure is an answer");
        assert!(failed.is_error);
        assert_eq!(failed.text, "quota exceeded");

        assert!(parse_codex("codex: some banner\n", false).is_err());
    }
}
