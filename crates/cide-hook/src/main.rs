//! `cide-hook <event>` — bridges a Claude Code hook invocation to the running IDE.
//!
//! Reads the hook JSON payload on stdin and writes one newline-delimited frame to the unix
//! socket named by `CIDE_HOOK_SOCK`.
//!
//! # This program must never make a session worse
//!
//! It runs on the critical path of the user's Claude Code turn — before every tool call,
//! after every one, and around the statusline on a timer. Every failure mode here is
//! therefore an exit 0: a missing socket, a full socket, a refused connection, an unset
//! variable. A hook that errors is a hook that puts noise in someone's session or, worse,
//! blocks it. The IDE being gone is a *normal* state, not an error — the app can quit while
//! a `claude` child is still winding down.
//!
//! The one thing it must do faithfully is the statusline: `cide-hook statusline` runs the
//! user's own configured command and prints its stdout verbatim, so adopting cide never
//! costs someone the status line they already had. If that command fails, we print nothing
//! rather than printing an error into their status bar.
//!
//! It is a separate binary rather than a subcommand of the app so that spawning a hook costs
//! a ~1 MB process rather than a webview.
//!
//! # The one thing it refuses
//!
//! Since M18 there is a second half, and it is not a socket write: on `PreToolUse` this program
//! decides, locally, whether the tool call about to run is an agent hand-editing
//! `.cide/tasks.json`, and prints a `deny` on stdout when it is. That is the rule behind
//! `cide_tasks`'s one-process-one-mutex design, and [`guard`] holds the whole argument — including
//! why the decision needs no round trip to cide, and therefore costs the paragraph above nothing.
//!
//! # A third mode, with the opposite failure rule
//!
//! `cide-hook mcp` is a stdio MCP server bridging a `claude`/`opencode` child to the running
//! IDE over a *second* socket. It reuses this binary because it is already resolved beside
//! `current_exe()`, already shipped as `bundle.externalBin`, and already ~1 MB with one
//! dependency. Its rules are documented on [`mcp`], and the first of them contradicts the
//! paragraph above: an MCP server must **not** exit on failure, because a server that exits
//! mid-turn takes its tools out of the model's context. Read that module before assuming the
//! exit-0 rule applies there.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

/// How long to wait on the socket before giving up.
///
/// Short on purpose. The IDE is a local process on the other end of a unix socket; if it has
/// not accepted within this, it is wedged or gone, and continuing to wait would stall the
/// user's turn for a status update nobody is watching.
const SOCKET_TIMEOUT: Duration = Duration::from_millis(250);

fn main() {
    let event = std::env::args().nth(1).unwrap_or_default();
    if event.is_empty() {
        std::process::exit(0);
    }

    // Dispatched before stdin is drained, because `mcp` is the one mode whose stdin is a
    // stream rather than a payload: `read_to_string` below would block until the client
    // exited, which is exactly the moment the bridge is no longer needed.
    if event == "mcp" {
        mcp::run();
    }

    let mut payload = String::new();
    let _ = std::io::stdin().read_to_string(&mut payload);

    if event == "statusline" {
        // The user's own command, if they had one, is everything after the subcommand.
        let chained: Vec<String> = std::env::args().skip(2).collect();
        if !chained.is_empty() {
            passthrough(&chained, &payload);
        }
    }

    // The decision half, and deliberately before the socket half: it is local and cannot
    // block, whereas `forward` may spend up to `SOCKET_TIMEOUT` on a socket nobody is reading,
    // and the CLI is waiting on this stdout to know whether the tool may run. `deny_line` is
    // `None` for every tool call but the one it exists for, and `None` prints nothing at all.
    if let Some(line) = guard::deny_line(&event, &payload) {
        let mut out = std::io::stdout();
        let _ = out.write_all(&line);
        let _ = out.flush();
    }

    forward(&event, &payload);
    std::process::exit(0);
}

/// Run the user's statusline command and print its stdout verbatim.
///
/// The hook payload is piped to it on stdin, because that is how the CLI would have invoked
/// it — a chained command must not be able to tell it is being chained.
fn passthrough(argv: &[String], payload: &str) {
    // Through a shell, because the user configured a command line and may well have written
    // a pipeline or a `~` in it. This is the user's own string from their own settings, so it
    // is exactly as trusted as it was before cide was involved.
    let joined = argv.join(" ");
    let spawned = Command::new("sh")
        .arg("-c")
        .arg(&joined)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();

    let Ok(mut child) = spawned else { return };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload.as_bytes());
        // Dropped explicitly: a statusline command that reads to EOF would otherwise hang,
        // and it would hang inside the user's session.
        drop(stdin);
    }

    let Ok(out) = child.wait_with_output() else {
        return;
    };
    // Verbatim, including the absence of a trailing newline. Reformatting someone's status
    // line is not ours to do.
    let _ = std::io::stdout().write_all(&out.stdout);
}

/// Write one frame to the IDE, if it is listening.
fn forward(event: &str, payload: &str) {
    let Some(path) = std::env::var_os("CIDE_HOOK_SOCK") else {
        return;
    };

    // Read from our own environment rather than the payload: we are a direct child of the
    // `claude` cide spawned, so we inherit the id cide minted for it. The payload's
    // `session_id` is whatever conversation the CLI is currently running, which is a
    // different thing the moment the user resumes or types `/clear`.
    let spawned_as = std::env::var("CIDE_SESSION").ok();

    forward_to(Path::new(&path), event, payload, spawned_as);
}

/// The socket half proper, with its two environment reads hoisted into [`forward`].
///
/// Split out so a test can watch a real frame arrive on a real socket without touching the
/// environment — `set_var` is `unsafe` in edition 2024 and racy across parallel tests besides,
/// which is the same reason `mcp::open` takes its environment as a closure. The one thing this
/// has to keep true is that it is reached for every event, whatever [`guard`] decided.
fn forward_to(path: &Path, event: &str, payload: &str, spawned_as: Option<String>) {
    let Ok(mut stream) = UnixStream::connect(path) else {
        return;
    };
    let _ = stream.set_write_timeout(Some(SOCKET_TIMEOUT));

    // The payload is embedded as a JSON value when it parses, and as a string when it does
    // not. Refusing to forward an unparseable payload would lose the event entirely; the
    // reader can tell the difference and is better placed to decide what to do about it.
    let parsed: serde_json::Value =
        serde_json::from_str(payload).unwrap_or_else(|_| serde_json::Value::String(payload.into()));

    let frame = serde_json::json!({
        "event": event,
        "payload": parsed,
        "spawned_as": spawned_as,
    });
    let Ok(mut line) = serde_json::to_vec(&frame) else {
        return;
    };
    line.push(b'\n');

    let _ = stream.write_all(&line);
    let _ = stream.flush();
}

mod guard {
    //! The one tool call `cide-hook` refuses: an agent hand-editing `.cide/tasks.json`.
    //!
    //! # What this defends
    //!
    //! `cide_tasks`'s whole concurrency story is *one process, one mutex*. Every mutation of the
    //! tracker — from the primary session, from a subagent, from the `Tasks` panel — arrives as an
    //! `mcp__cide__cide_task_*` call, travels the agent-RPC socket, and funnels into a single
    //! `TaskStore::update`. That module's own header states the consequence in one sentence: *the
    //! moment an agent could `Edit .cide/tasks.json` directly, this would need real file locking*.
    //! It names two things that keep that from happening — the agent prompt preamble, and this.
    //!
    //! The store does have a repair for an out-of-process write (`cide_tasks::merge`, layer 3),
    //! and it is good, but it is a *repair*: it runs on a debounce, it reconciles only what it can
    //! still see, and a write that lands between the store's read and its write is lost work with
    //! nothing anywhere to say so. A rule is cheaper than a repair, and this is the rule.
    //!
    //! # Why the decision is made here and not over the socket
    //!
    //! `cide-hook`'s standing contract is *write to the socket and forget; every failure is exit
    //! 0*, because it sits on the critical path of the user's turn. A deny would break that
    //! contract if it needed an answer from cide — a round trip, a timeout, and a refusal that
    //! silently stops working the moment the IDE is a moment behind.
    //!
    //! It does not need one. The `PreToolUse` payload carries `tool_name`, `tool_input` and the
    //! session's `cwd`, which is everything the decision reads. So the two halves of this program
    //! stay separate and neither can break the other: the frame still goes to the socket exactly
    //! as it always did, whatever is decided here, and the decision is a pure function of stdin
    //! printed on stdout. That purity is also what makes every rule below testable.
    //!
    //! # The protocol, and how it was verified
    //!
    //! A hook answers on stdout with a JSON object; for `PreToolUse` the decision lives under
    //! `hookSpecificOutput`. The spellings were read out of the shipped CLI (2.1.235) rather than
    //! from memory, three times over: its embedded hook documentation (*"`permissionDecision` -
    //! "allow", "deny", or "ask" (PreToolUse only)"*), the zod schema it validates hook output
    //! with (`hookEventName: literal("PreToolUse"), permissionDecision, permissionDecisionReason,
    //! updatedInput`), and the *Expected schema* block it prints when that validation fails. Its
    //! own log line on the consuming side is `Hook <event> (<name>) returned permissionDecision:
    //! deny (reason: …)`, and its validator rejects anything else with `Unknown hook
    //! permissionDecision type: …. Valid types are: allow, deny, ask, defer`.
    //!
    //! `hookEventName` is mandatory and must equal the event being answered — the CLI refuses a
    //! mismatch with `Hook returned incorrect event name: expected '…' but got '…'` — which is why
    //! the literal below and the event this arms on are the same constant.
    //!
    //! # Two rules with teeth
    //!
    //! **Only `tasks.json`.** `.cide/config.json` and `.cide/agents/*.md` are the *user's* files,
    //! written to be hand-edited and committed; `.cide/worktrees/` is git's. cide owns part of
    //! that directory, not the directory, and an agent editing a role definition is a different
    //! question that this is not the place to answer.
    //!
    //! **Never deny anything else, ever.** This runs before every tool call in every Claude cide
    //! spawns, the user's own console pane included. Anything not positively identified as a write
    //! to that one file produces *no output at all* — not an `allow`, which would override a
    //! permission rule the user set, and not a diagnostic, because stdout here is a protocol.
    //! Every failure mode is silence: a malformed payload, an absent field, an unreadable path.

    use std::ffi::OsStr;
    use std::path::{Component, Path, PathBuf};

    use serde_json::Value;

    /// The only event this guard is armed on, and the `hookEventName` it answers with.
    ///
    /// One constant for both, because the CLI requires them to be equal and rejects the output
    /// outright when they are not.
    const PRE_TOOL_USE: &str = "PreToolUse";

    /// The tools that write a file at a path named in their input.
    ///
    /// Read out of the shipped CLI rather than guessed: its own dispatch reads `file_path` for
    /// `Read`, `Edit` and `Write`, `notebook_path` for `NotebookEdit`, and `command` for `Bash`.
    /// `Read` is absent from this list because it writes nothing, and `Bash` because a shell line
    /// is not a path — `sh -c 'echo … > .cide/tasks.json'` is a hole this hook does not close, and
    /// pretending otherwise by pattern-matching command strings would be the kind of half-rule
    /// that reads as protection and is not.
    ///
    /// `MultiEdit` is here although 2.1.235 registers no such tool: it is still in the CLI's own
    /// list of known tool names for permission rules, it is what other harnesses ship, and a name
    /// that never arrives costs one string comparison. `NotebookEdit` is here for the symmetric
    /// reason and can never actually match — the CLI refuses a path that is not an `.ipynb` — but
    /// the arm costs nothing and does not depend on that refusal staying true.
    const WRITERS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

    /// Where in `tool_input` a path is found. `NotebookEdit` is the one that spells it its own way.
    const PATH_KEYS: &[&str] = &["file_path", "notebook_path"];

    /// The two trailing components that identify the tracker: `<anything>/.cide/tasks.json`.
    ///
    /// Spelled here rather than imported from `cide_tasks::TASKS_RELATIVE`, which is the same two
    /// components. This binary has exactly one dependency and is ~1 MB *because* of that; linking
    /// the domain crate — and through it `cide-core`, `cide-ipc`, `serde`, `parking_lot` — into a
    /// process spawned once per tool call would cost far more than a duplicated pair of strings.
    /// A rename there must be mirrored here; it is a `pub const` with one spelling, so grep finds
    /// this.
    const TRACKER_DIR: &str = ".cide";
    const TRACKER_FILE: &str = "tasks.json";

    /// The directory holding one file per task: `.cide/tasks/<id>/task.json`. (M68)
    ///
    /// A second thing to match, and it is not optional. The tracker used to be one file, so denying
    /// writes to `.cide/tasks.json` denied writes to the tracker. Since M68 a task's body, log and
    /// history are in **its own file**, and an agent with `Write` could edit
    /// `.cide/tasks/t-5/task.json` directly — behind the single process that holds the only writer,
    /// whose next flush overwrites it or merges it unpredictably. That is exactly the loss
    /// [`DENY_REASON`] describes, arriving through a path this matcher had never heard of.
    ///
    /// Spelled here rather than imported from `cide_tasks::TASKS_DIR`, on `TRACKER_FILE`'s stated
    /// reason: this binary has one dependency and is ~1 MB *because* of that.
    const TRACKER_SUBDIR: &str = "tasks";

    /// What the agent is told, and why it names the tools.
    ///
    /// A refusal that does not say what to do instead is a refusal the model retries, or works
    /// around with `Bash`, having learned nothing. So this names the three tools that do the job
    /// the edit was reaching for, and says the thing that makes obeying rational: the tools write
    /// the same file, and the panel updates as they do.
    const DENY_REASON: &str = "cide owns the task tracker — .cide/tasks.json and everything under \
         .cide/tasks/: a single process holds the only writer for them, so an edit made behind that \
         process's back is either overwritten by its next write or merged unpredictably with it, \
         and the work in the edit is lost. Use the tracker's own tools, which write those same \
         files safely and update the Tasks panel as they do: mcp__cide__cide_task_update to change \
         a task's title, body or status, mcp__cide__cide_task_comment to append a comment, \
         mcp__cide__cide_task_assign to set the agent a task is for. mcp__cide__cide_task_create, \
         mcp__cide__cide_task_list and mcp__cide__cide_task_get are there as well. Nothing else \
         under .cide/ is restricted.";

    /// The decision line to print, or `None` for every other tool call in the world.
    ///
    /// # Three fields the CLI offers that this does not use
    ///
    /// `continue: false` with a `stopReason` stops the *whole turn*, not the one tool call. That
    /// is a wildly disproportionate answer to an agent reaching for the wrong file: it would end a
    /// turn that may be nine-tenths done, over something the model can simply do another way in
    /// its next thought. `decision: "block"` is the older spelling and the CLI's own docs mark it
    /// *deprecated for PreToolUse, use hookSpecificOutput.permissionDecision instead*. And
    /// `systemMessage` would put a line in the user's UI on every refusal, which is a notification
    /// about a correction the model is about to make by itself — the frame going to cide over the
    /// socket is already the record, and it goes to something that can decide what to do with it.
    ///
    /// # The user's own console is a Claude too, and it is denied as well
    ///
    /// `$CIDE_RUN` is set on a dispatched subagent and unset on a pane, so the rule *could* be
    /// scoped to subagents. It deliberately is not, and the losing argument is worth stating: a
    /// human who tells their own session to repair a mangled tracker has a legitimate reason, and
    /// the pane is the lane where a person is watching and can be asked.
    ///
    /// Three things decide it the other way. The console pane is the *orchestrator* — the writer
    /// most likely to reach for the file and the one whose divergence hurts most, because that is
    /// precisely when the store is open, live, and about to write over it. The escape hatch the
    /// exemption would buy already exists and does not need a tool call: cide's own editor, any
    /// other editor, `git checkout -- .cide/tasks.json`, and `cide_tasks::load`'s own repair,
    /// which moves an unparseable file aside and starts clean without anybody editing anything.
    /// And a rule that reads the environment is a rule that behaves differently in two lanes for
    /// reasons invisible in the payload — the same tool call allowed here and refused there, which
    /// is the kind of difference nobody can debug from a transcript. The decision stays a pure
    /// function of stdin.
    pub fn deny_line(event: &str, payload: &str) -> Option<Vec<u8>> {
        if event != PRE_TOOL_USE {
            return None;
        }

        // Parsed here as well as in `super::forward`, on purpose. Sharing one parse would tie the
        // decision to the reporting half, and the whole point is that either can fail alone. A
        // hook payload is small and this process was just forked; a second parse is not the cost.
        let value: Value = serde_json::from_str(payload).ok()?;
        let tool = value.get("tool_name")?.as_str()?;
        if !WRITERS.contains(&tool) {
            return None;
        }

        let input = value.get("tool_input")?.as_object()?;
        let raw = PATH_KEYS.iter().find_map(|key| input.get(*key)?.as_str())?;
        if !is_tracker(value.get("cwd").and_then(Value::as_str), raw) {
            return None;
        }

        let line = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": PRE_TOOL_USE,
                "permissionDecision": "deny",
                "permissionDecisionReason": DENY_REASON,
            }
        });
        // Serialised rather than formatted, so the reason's punctuation cannot break the JSON.
        let mut bytes = serde_json::to_vec(&line).ok()?;
        bytes.push(b'\n');
        Some(bytes)
    }

    /// Does this tool input name the tracker?
    ///
    /// # Resolve before comparing
    ///
    /// A string compare is a hole: `.cide/tasks.json`, `./.cide/tasks.json`,
    /// `../.cide/tasks.json` from a subdirectory, an absolute path and a symlink are all the same
    /// file and only one of them looks like it. So the path is joined onto the payload's `cwd`
    /// when it is relative — `cwd` is the session's working directory and therefore the base the
    /// CLI itself will resolve against — and then reduced by [`tracker_tail`], which drops `.`
    /// components and pops on `..`.
    ///
    /// # Why the test is the two trailing components and not equality with a computed path
    ///
    /// This process does not know the project root. It knows a `cwd`, which may be the project, a
    /// subdirectory of it, or an agent's worktree under `.cide/worktrees/<agent>` — three
    /// different roots, all of which have a tracker that some `TaskStore` may own. Testing for a
    /// file named `tasks.json` inside a directory named `.cide` catches all three and needs no
    /// root at all. It is also the *conservative textual* fallback the resolution needs, because
    /// it works on a path that does not exist yet — which `Write` creating the first tracker in a
    /// project is exactly.
    ///
    /// # And then the filesystem, when it can answer
    ///
    /// Lexical reduction is wrong for one case: `..` that traverses a symlinked directory resolves
    /// differently on disk than on paper. So a path that survives the lexical test is offered to
    /// `canonicalize` as well, which follows every link — and, when the file does not exist yet,
    /// its parent is, which catches a symlinked `.cide/`. Either answer denying is enough; both
    /// failing to resolve leaves the lexical verdict standing rather than turning a missing file
    /// into permission.
    fn is_tracker(cwd: Option<&str>, raw: &str) -> bool {
        let raw = Path::new(raw);
        let joined: PathBuf = match cwd {
            Some(cwd) if !cwd.is_empty() && raw.is_relative() => Path::new(cwd).join(raw),
            _ => raw.to_path_buf(),
        };

        if tracker_tail(&joined) {
            return true;
        }

        // The link-following answers. Both are best-effort: a path we cannot stat is simply not
        // identified, and an unidentified path is allowed, per the module's second rule.
        if joined.canonicalize().is_ok_and(|real| tracker_tail(&real)) {
            return true;
        }
        if joined.file_name() == Some(OsStr::new(TRACKER_FILE))
            && let Some(parent) = joined.parent()
            && let Ok(real) = parent.canonicalize()
        {
            return real.file_name() == Some(OsStr::new(TRACKER_DIR));
        }
        false
    }

    /// Does this path name the tracker — either half of it — once `.` and `..` are resolved on paper?
    ///
    /// Two shapes since M68: `.cide/tasks.json`, the index, and **anything at all** under
    /// `.cide/tasks/`. The second is deliberately a prefix rather than a match on
    /// `<id>/task.json`, because everything under a task's directory belongs to the tracker: the
    /// content file, and the attachment bytes beside it. An agent that may not rewrite a comment
    /// may not rewrite the screenshot attached to it either.
    ///
    /// `Path::components` already drops `.` and duplicate separators but keeps `..`, which is the
    /// component that matters here, so the popping is done by hand. A `..` with nothing to pop is
    /// dropped rather than kept: only the tail is ever inspected, so how far above the base the
    /// path starts cannot change the answer.
    fn tracker_tail(path: &Path) -> bool {
        let mut stack: Vec<&OsStr> = Vec::new();
        for part in path.components() {
            match part {
                Component::CurDir => {}
                Component::ParentDir => {
                    stack.pop();
                }
                Component::Normal(name) => stack.push(name),
                // A root or a prefix restarts the path; anything gathered before it was relative
                // to a base this path just abandoned.
                Component::RootDir | Component::Prefix(_) => stack.clear(),
            }
        }
        if matches!(
            stack.as_slice(),
            [.., dir, file] if *dir == OsStr::new(TRACKER_DIR) && *file == OsStr::new(TRACKER_FILE)
        ) {
            return true;
        }
        // `<anything>/.cide/tasks/…` — the content files and the attachment bytes under them. The
        // window is scanned rather than only the tail, because the interesting pair can be at any
        // depth: `.cide/tasks/t-5/task.json` has two components after it and
        // `.cide/tasks/t-5/attachments/a-1/shot.png` has four.
        stack
            .windows(2)
            .any(|pair| pair[0] == OsStr::new(TRACKER_DIR) && pair[1] == OsStr::new(TRACKER_SUBDIR))
            && stack.len() > 2
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A realistic `PreToolUse` payload: the CLI sends `session_id`, `transcript_path`, `cwd`,
        /// `permission_mode`, `hook_event_name`, `tool_name`, `tool_input` and `tool_use_id`.
        fn payload(tool: &str, key: &str, path: &str, cwd: &str) -> String {
            serde_json::json!({
                "session_id": "3f0d1b1e-0000-4000-8000-000000000000",
                "transcript_path": "/home/dev/.claude/projects/x/y.jsonl",
                "cwd": cwd,
                "permission_mode": "acceptEdits",
                "hook_event_name": "PreToolUse",
                "tool_name": tool,
                "tool_use_id": "toolu_01",
                "tool_input": { key: path, "old_string": "a", "new_string": "b" },
            })
            .to_string()
        }

        fn denied(tool: &str, path: &str, cwd: &str) -> bool {
            deny_line(PRE_TOOL_USE, &payload(tool, "file_path", path, cwd)).is_some()
        }

        fn temp_dir(name: &str) -> PathBuf {
            let dir =
                std::env::temp_dir().join(format!("cide-hook-guard-{}-{name}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("a temp dir");
            dir
        }

        #[test]
        fn an_edit_and_a_write_to_the_tracker_are_both_refused() {
            // Both tools, because an agent told "no" to one reaches for the other, and a rule
            // that covers half the writers is a rule that only delays the divergence.
            assert!(denied(
                "Edit",
                "/home/dev/proj/.cide/tasks.json",
                "/home/dev/proj"
            ));
            assert!(denied(
                "Write",
                "/home/dev/proj/.cide/tasks.json",
                "/home/dev/proj"
            ));
            assert!(denied(
                "MultiEdit",
                "/home/dev/proj/.cide/tasks.json",
                "/home/dev/proj"
            ));
        }

        #[test]
        fn a_relative_and_a_dot_dot_path_resolve_to_the_same_refusal() {
            // The three spellings of one file. A string compare against ".cide/tasks.json" would
            // catch the first and let the other two through, which is the hole this exists for.
            assert!(denied("Edit", ".cide/tasks.json", "/home/dev/proj"));
            assert!(denied("Edit", "./.cide/./tasks.json", "/home/dev/proj"));
            assert!(denied(
                "Edit",
                "../.cide/tasks.json",
                "/home/dev/proj/crates"
            ));
            assert!(denied(
                "Edit",
                "crates/../.cide/tasks.json",
                "/home/dev/proj"
            ));
        }

        #[test]
        fn a_symlink_to_the_tracker_is_followed() {
            // A link is the one spelling no amount of string work can see through, which is why
            // `is_tracker` asks the filesystem as well as the components.
            let dir = temp_dir("symlink");
            let cide = dir.join(TRACKER_DIR);
            std::fs::create_dir_all(&cide).expect("mkdir .cide");
            std::fs::write(cide.join(TRACKER_FILE), "{}").expect("write tracker");
            let link = dir.join("shortcut.json");
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(cide.join(TRACKER_FILE), &link).expect("symlink");

            assert!(denied("Edit", "shortcut.json", &dir.display().to_string()));

            let _ = std::fs::remove_file(&link);
            let _ = std::fs::remove_dir_all(&cide);
            let _ = std::fs::remove_dir(&dir);
        }

        #[test]
        fn a_tracker_that_does_not_exist_yet_is_still_the_tracker() {
            // `Write` creating the first tracker in a project is the ordinary case, and a rule
            // that only fires on an existing file would miss precisely the first divergence.
            let dir = temp_dir("absent");
            let cide = dir.join(TRACKER_DIR);
            std::fs::create_dir_all(&cide).expect("mkdir .cide");
            let _ = std::fs::remove_file(cide.join(TRACKER_FILE));

            assert!(denied(
                "Write",
                ".cide/tasks.json",
                &dir.display().to_string()
            ));

            let _ = std::fs::remove_dir_all(&cide);
            let _ = std::fs::remove_dir(&dir);
        }

        #[test]
        fn the_rest_of_the_cide_directory_is_the_users_to_edit() {
            // Denying more than the one file would be cide taking over a directory it owns part
            // of: `config.json` and the role definitions are written to be hand-edited.
            let cwd = "/home/dev/proj";
            assert!(!denied("Edit", ".cide/config.json", cwd));
            assert!(!denied("Write", ".cide/agents/developer.md", cwd));
            assert!(!denied("Edit", ".cide/agents/qa.md", cwd));
            // The archived copy `cide_tasks::load` leaves behind when it repairs a broken file is
            // exactly what a human may want to salvage by hand.
            assert!(!denied("Edit", ".cide/tasks.corrupt-1.json", cwd));
            // And a `tasks.json` that is not under `.cide/` is somebody else's file entirely.
            assert!(!denied("Edit", "tasks.json", cwd));
            assert!(!denied("Edit", "docs/tasks.json", cwd));
        }

        #[test]
        fn an_ordinary_source_file_is_not_the_trackers_business() {
            let cwd = "/home/dev/proj";
            assert!(!denied("Edit", "crates/cide-hook/src/main.rs", cwd));
            assert!(!denied("Write", "/home/dev/proj/README.md", cwd));
            assert!(!denied("Write", "/etc/hosts", cwd));
        }

        #[test]
        fn a_missing_path_a_malformed_payload_and_an_unknown_tool_say_nothing() {
            // Every one of these is a state that occurs; every one of them must be silence,
            // because stdout is a protocol and a diagnostic printed here is a parse error in
            // somebody's session.
            let no_path = serde_json::json!({
                "cwd": "/home/dev/proj",
                "tool_name": "Edit",
                "tool_input": { "old_string": "a" },
            })
            .to_string();
            assert_eq!(deny_line(PRE_TOOL_USE, &no_path), None);

            let no_input = serde_json::json!({ "cwd": "/x", "tool_name": "Write" }).to_string();
            assert_eq!(deny_line(PRE_TOOL_USE, &no_input), None);

            assert_eq!(deny_line(PRE_TOOL_USE, "not json at all"), None);
            assert_eq!(deny_line(PRE_TOOL_USE, ""), None);
            assert_eq!(deny_line(PRE_TOOL_USE, "[1,2,3]"), None);

            // A tool that does not write, pointed at the tracker: `Read` and `Bash` are not this
            // hook's business, and denying a read of the tracker would be a plain regression.
            assert!(!denied("Read", ".cide/tasks.json", "/home/dev/proj"));
            assert!(!denied("Glob", ".cide/tasks.json", "/home/dev/proj"));
            let bash = serde_json::json!({
                "cwd": "/home/dev/proj",
                "tool_name": "Bash",
                "tool_input": { "command": "cat .cide/tasks.json" },
            })
            .to_string();
            assert_eq!(deny_line(PRE_TOOL_USE, &bash), None);
        }

        #[test]
        fn no_other_event_is_guarded() {
            // The CLI calls this binary for all ten events with a match-all matcher. A decision
            // printed under any other event is output the CLI rejects with "Hook returned
            // incorrect event name", and under `Stop` it would be output in a lane with no
            // decision at all.
            let denied_payload = payload("Edit", "file_path", ".cide/tasks.json", "/home/dev/proj");
            for event in [
                "PostToolUse",
                "PostToolBatch",
                "Stop",
                "SessionStart",
                "statusline",
            ] {
                assert_eq!(deny_line(event, &denied_payload), None, "{event}");
            }
        }

        #[test]
        fn the_deny_carries_the_clis_spelling_and_names_the_tools() {
            let line = deny_line(
                PRE_TOOL_USE,
                &payload("Edit", "file_path", ".cide/tasks.json", "/home/dev/proj"),
            )
            .expect("the tracker is denied");

            // One JSON object, one line, nothing else — the CLI treats output that does not
            // start with `{` as plain text and the decision is lost.
            let text = String::from_utf8(line).expect("utf-8");
            assert!(text.ends_with('\n'), "{text}");
            assert_eq!(text.lines().count(), 1, "{text}");

            let value: Value = serde_json::from_str(&text).expect("json");
            let out = &value["hookSpecificOutput"];
            assert_eq!(out["hookEventName"], "PreToolUse");
            assert_eq!(out["permissionDecision"], "deny");

            let reason = out["permissionDecisionReason"]
                .as_str()
                .expect("a reason is a string");
            for tool in [
                "mcp__cide__cide_task_update",
                "mcp__cide__cide_task_comment",
                "mcp__cide__cide_task_assign",
            ] {
                assert!(reason.contains(tool), "{reason}");
            }
        }

        #[test]
        fn a_notebook_edit_names_its_path_the_other_way() {
            // It cannot actually reach a `.json` file — the CLI refuses a path that is not an
            // `.ipynb` — but the arm must not depend on that refusal staying true, and reading
            // `file_path` from a tool that never sends one would be a rule that silently never
            // fires.
            let raw = payload(
                "NotebookEdit",
                "notebook_path",
                ".cide/tasks.json",
                "/home/dev/proj",
            );
            assert!(deny_line(PRE_TOOL_USE, &raw).is_some());
        }

        #[test]
        fn a_path_with_no_usable_base_is_still_identified() {
            // `cwd` is always sent, but a payload that lost it must not turn a relative path into
            // permission: the two trailing components are the whole test and need no base.
            assert!(is_tracker(None, ".cide/tasks.json"));
            assert!(is_tracker(Some(""), "sub/.cide/tasks.json"));
            assert!(!is_tracker(None, ".cide/config.json"));
        }
    }
}

mod mcp {
    //! `cide-hook mcp` — a stdio MCP server that is a **dumb pipe** to the running cide.
    //!
    //! # Why this exists as its own server
    //!
    //! The orchestration and task tools cannot ride on the IDE-integration MCP server. That
    //! connection is a fixed vocabulary the CLI drives, and `cide-ide-mcp`'s own
    //! `the_five_are_the_only_five` test records why: anything else there would be *"handlers
    //! no client ever calls"*. So the tools are served as an ordinary stdio MCP server,
    //! attached to a session with `--mcp-config` (verified end to end against the real CLI
    //! with an inline JSON string), which is also what makes them reachable from opencode
    //! through `OPENCODE_CONFIG_CONTENT.mcp`. One server definition, three consumers.
    //!
    //! # Why it knows nothing about the tools
    //!
    //! `initialize`, `tools/list` and `tools/call` are **all proxied verbatim**. The bridge
    //! never parses a tool name, never holds a schema, and cannot be out of date about one.
    //! The tool list therefore has exactly one definition, in the app, where the registries
    //! live. Linking the vocabulary in here would cost binary size and — much worse —
    //! introduce version skew: a `cide-hook` left over from an older install would advertise
    //! tools the app has since removed, and the model would call them.
    //!
    //! The only JSON this module inspects is the envelope: whether a line is a request, a
    //! notification or a response, and which id it carries. Nothing about the payload.
    //!
    //! # The protocol
    //!
    //! ## Socket
    //!
    //! `$CIDE_AGENT_SOCK`, read from the environment exactly as `$CIDE_HOOK_SOCK` is by
    //! [`super::forward`]. cide binds it at `$XDG_RUNTIME_DIR/cide-agents-<pid>.sock` and sets
    //! the variable on every child it spawns. Unset, empty, or refused → see *Degradation*.
    //! The value is read as a `String` rather than an `OsString`: a socket path that is not
    //! UTF-8 degrades to the empty-tools server rather than being connected to, which is a
    //! path cide never produces and a failure mode that is safe if it ever appears.
    //!
    //! ## Framing
    //!
    //! Newline-delimited JSON, one object per line, in both directions — the same framing as
    //! the hook socket and for the reason `cide_app::hooks` states there: a writer that dies
    //! mid-write costs one truncated line the reader discards, whereas a length prefix
    //! desynchronises the stream for ever. That argument is why a write that times out
    //! part-way through a line is treated as *fatal to the connection* below, and never
    //! retried: the remaining bytes cannot be resumed, and sending the next line after a
    //! partial one would splice two messages together.
    //!
    //! ## One long-lived connection
    //!
    //! Unlike the hook path — which is connection-per-frame because each hook is a fresh
    //! short-lived process — the bridge connects **once** and keeps the connection for the
    //! whole MCP session. Two reasons. An MCP session is a conversation: request ids are only
    //! meaningful against the connection that issued them, so replies must come back down the
    //! same one. And the app binds *identity* to the connection, from the header line below;
    //! reconnecting per message would re-run that binding for every message, or lose it.
    //!
    //! ## The header line, written once, before any JSON-RPC traffic
    //!
    //! ```json
    //! {"hello":"cide-mcp","v":1,"run":"<$CIDE_RUN or null>","session":"<$CIDE_SESSION or null>","pid":1234}
    //! ```
    //!
    //! It is one line in the same framing (key order is JSON's, i.e. immaterial). `v` is the
    //! header's schema version, bumped when a field is added or removed, so the app may refuse
    //! a version it does not know rather than guess. `pid` is this bridge's own pid, from
    //! `getpid`. `run` and `session` are `null` when the variable is unset *or empty*.
    //!
    //! **Both ids come from the bridge's own environment and never from a payload it is
    //! handed** — the identical argument [`super::forward`]'s `spawned_as` makes: a caller
    //! must not be able to compose its own identity, or an agent can sign a comment as the
    //! user, or dispatch as though it were the orchestrator.
    //!
    //! That is what lets the app scope the tool list *structurally*: a connection naming a
    //! **run** is answered with the task tools; a connection naming the project's **primary
    //! session** is answered with the orchestration tools as well. Agent-dispatches-agent is
    //! closed by construction rather than by prompting.
    //!
    //! ## After the header
    //!
    //! Every line from stdin is written to the socket; every line from the socket is written
    //! to stdout. Verbatim in both directions — not re-serialised, which would reorder keys
    //! and could round-trip a number differently. Blank lines are skipped.
    //!
    //! ## Shutdown
    //!
    //! stdin reaching EOF is the client going away, and the only clean end: the bridge waits
    //! briefly for any request it has already forwarded to be answered, then exits 0. It never
    //! exits for any other reason while the client is alive.
    //!
    //! # Degradation
    //!
    //! `cide-hook`'s standing rule is that every failure is an exit 0, because it runs on the
    //! critical path of a user's turn. **This mode must degrade differently**, and the
    //! difference is the whole design: an MCP server that exits mid-turn takes its tools out
    //! of the model's context and leaves the CLI logging a broken server. So it never exits
    //! while the client is alive.
    //!
    //! | state | `initialize` | `tools/list` | `ping` | anything else | notifications |
    //! | --- | --- | --- | --- | --- | --- |
    //! | connected | proxied | proxied | proxied | proxied | proxied, no reply |
    //! | never connected (`NoSocket`) | valid result | `{"tools":[]}` | `{}` | error `-32603` | dropped, no reply |
    //! | died mid-session (`Lost`) | error `-32603` | error `-32603` | `{}` | error `-32603` | dropped, no reply |
    //!
    //! The two degraded states answer differently on purpose. Before the handshake the client
    //! has been told nothing, so a clean handshake advertising zero tools is the honest
    //! picture and the client shows a server with no tools instead of a crash at every
    //! launch. After a loss the client already holds a tool list, and an error naming cide is
    //! the honest answer to a call it may retry later — not a claim that the tools never
    //! existed. `ping` is answered locally in both, because it asks whether *this process* is
    //! alive and it is; an error there invites a client to kill the server it is asking about.
    //!
    //! A request that was already forwarded when the connection died is answered too, from
    //! the pending list — otherwise a `tools/call` in flight at that moment hangs a turn for
    //! ever, which is the failure this whole section exists to prevent.
    //!
    //! **Notifications never get a reply.** A JSON-RPC message with no id (or a null one) is a
    //! notification — `notifications/initialized` is the one every session sends — and
    //! answering it is a protocol error; conversely, failing to answer a *request* makes the
    //! CLI wait for ever. Telling the two apart is the one piece of parsing here that has to
    //! be right, and it has its own tests.
    //!
    //! # Two rules with teeth
    //!
    //! **Nothing but JSON-RPC ever reaches stdout.** stdout *is* the transport; a stray
    //! `println!` is a protocol violation. Diagnostics go to stderr, which the CLI captures
    //! into its own log.
    //!
    //! **The socket carries a write timeout.** cide can `SIGSTOP` a paused agent's whole
    //! process group, and this bridge is inside that group — so it can be frozen with bytes
    //! still queued. Without a timeout an app thread writing into the frozen socket blocks the
    //! moment the kernel buffer fills, and pausing one agent wedges an app thread; the
    //! symmetric timeout here keeps a wedged or frozen *app* from blocking this pump, which
    //! would stop it answering anything at all. Both ends need one. A write that does time out
    //! kills the connection rather than retrying, per *Framing* above.

    use std::io::{BufRead, BufReader, ErrorKind, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex, MutexGuard};
    use std::time::Duration;

    use serde_json::{Value, json};

    /// The header's tag, and its schema version. Bump `v` when a field is added or removed.
    const HELLO: &str = "cide-mcp";
    const HELLO_VERSION: u32 = 1;

    /// The three variables cide sets on a child. All are read from the environment, never
    /// from a payload — see the module doc.
    const SOCK_VAR: &str = "CIDE_AGENT_SOCK";
    const RUN_VAR: &str = "CIDE_RUN";
    const SESSION_VAR: &str = "CIDE_SESSION";

    /// How long a write into the app's socket may block before the connection is written off.
    ///
    /// Deliberately much longer than [`super::SOCKET_TIMEOUT`]'s 250 ms: losing a hook frame
    /// costs a status update nobody is watching, whereas losing this write costs a tool call
    /// out of the model's turn, and the app may legitimately be a moment behind. Bounded all
    /// the same, because unbounded is the deadlock the module doc describes.
    const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

    /// JSON-RPC's own code for "the server hit something it could not handle".
    ///
    /// Not `-32601` (method not found): the method almost certainly *does* exist — in an app
    /// we cannot currently reach — and saying otherwise would be a lie the client caches.
    const INTERNAL_ERROR: i32 = -32603;

    /// What a client is told when cide is not reachable. Names cide, so the message read out
    /// of a CLI log points at the right process.
    const NO_CIDE: &str = "cide is not running: the IDE that serves these tools is unreachable";

    /// Echoed back when a client sends `initialize` without a `protocolVersion`.
    ///
    /// The same constant `cide-ide-mcp` carries, and used the same way: the reply echoes the
    /// client's version and only falls back to this, because inventing a version the client
    /// did not offer ends the handshake.
    const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

    /// Why the link is down, which decides how requests are answered.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Down {
        /// `$CIDE_AGENT_SOCK` was unset, refused, or the header never went out: we have never
        /// spoken to cide, and the client has not yet been told anything.
        NoSocket,
        /// The connection existed and then died. The client already holds a tool list.
        Lost,
    }

    enum Link {
        Up(UnixStream),
        Down(Down),
    }

    /// A JSON-RPC line from the client, classified by envelope only.
    #[derive(Debug, PartialEq, Eq)]
    enum Inbound {
        /// Has a method and a non-null id: a reply is owed.
        Request {
            id: Value,
            method: String,
            params: Value,
        },
        /// Has a method and no usable id: a reply is a protocol error.
        Notification { method: String },
        /// A reply to something the *server* asked. Nothing is owed.
        Response,
        /// Not JSON, not an object, or an object that is neither of the above.
        Malformed,
    }

    struct Bridge {
        link: Mutex<Link>,
        /// Ids of requests written to the socket and not yet answered. Drained into errors if
        /// the connection dies, so no forwarded request is left hanging.
        pending: Mutex<Vec<Value>>,
        /// stdout, behind a mutex because both the reader thread (proxied replies) and the
        /// stdin pump (locally generated errors) write to it, and two interleaved writes would
        /// splice two JSON-RPC messages into one unparseable line.
        out: Mutex<Box<dyn Write + Send>>,
    }

    /// What to do after releasing the link lock. Computed under the lock, acted on outside it,
    /// because `go_down` takes the same lock.
    enum Outcome {
        Sent,
        Died,
        Offline(Down),
    }

    /// Run the bridge until stdin reaches EOF.
    pub fn run() -> ! {
        let link = open(|key| std::env::var(key).ok(), std::process::id());
        let bridge = Arc::new(Bridge::new(Box::new(std::io::stdout()), link));
        Arc::clone(&bridge).start_reader();

        pump(&bridge, std::io::stdin().lock());

        // EOF on stdin is the client going away, which is the one clean end.
        //
        // A short bounded wait first, and only while something is actually owed: a client that
        // writes its last request and closes stdin in the same breath — which is exactly what
        // a pipe-driven harness does — would otherwise lose the reply that was still in
        // flight. The usual shutdown owes nothing and this costs it nothing. Bounded, because
        // an app that never answers must not be able to keep this process alive.
        bridge.drain(WRITE_TIMEOUT);

        // Exit 0 without waiting for the reader thread: it may be parked on a socket read that
        // will never return, and there is nobody left to write its output to.
        std::process::exit(0);
    }

    /// Connect, arm the write timeout, and send the header.
    ///
    /// The environment is a closure rather than direct `std::env` reads so the whole path —
    /// including the header — is testable against a fake environment without `set_var`, which
    /// is `unsafe` in edition 2024 and racy across parallel tests besides.
    fn open(env: impl Fn(&str) -> Option<String>, pid: u32) -> Link {
        let Some(path) = env(SOCK_VAR).filter(|value| !value.is_empty()) else {
            eprintln!("cide-hook mcp: {SOCK_VAR} is unset; serving an empty tool list");
            return Link::Down(Down::NoSocket);
        };

        let stream = match UnixStream::connect(&path) {
            Ok(stream) => stream,
            Err(err) => {
                eprintln!("cide-hook mcp: cannot connect to {path}: {err}");
                return Link::Down(Down::NoSocket);
            }
        };

        if let Err(err) = stream.set_write_timeout(Some(WRITE_TIMEOUT)) {
            // Refusing the connection is the conservative answer: without the timeout this
            // process can be blocked indefinitely by a frozen peer, and a bridge that has
            // stopped reading stdin is worse than one with no tools.
            eprintln!("cide-hook mcp: no write timeout on {path}: {err}");
            return Link::Down(Down::NoSocket);
        }

        let header = header_line(&env, pid);
        let mut socket: &UnixStream = &stream;
        if header.is_empty()
            || socket
                .write_all(&header)
                .and_then(|()| socket.flush())
                .is_err()
        {
            // `NoSocket` rather than `Lost`: the client has been told nothing yet, so the
            // honest degradation is a clean handshake with no tools.
            eprintln!("cide-hook mcp: the header write failed; serving an empty tool list");
            return Link::Down(Down::NoSocket);
        }

        Link::Up(stream)
    }

    /// The one line written before any JSON-RPC traffic. See the module doc for its meaning.
    fn header_line(env: impl Fn(&str) -> Option<String>, pid: u32) -> Vec<u8> {
        // Empty is treated as absent: a child spawned with `CIDE_RUN=` is not a run, and
        // `""` would bind the connection to an identity that does not exist.
        let id_of = |key: &str| env(key).filter(|value| !value.is_empty());

        let header = json!({
            "hello": HELLO,
            "v": HELLO_VERSION,
            "run": id_of(RUN_VAR),
            "session": id_of(SESSION_VAR),
            "pid": pid,
        });

        let Ok(mut line) = serde_json::to_vec(&header) else {
            // Unreachable for an object of strings, an integer and nulls. Empty rather than a
            // partial line, because the caller reads emptiness as "do not use this socket".
            return Vec::new();
        };
        line.push(b'\n');
        line
    }

    /// Read the client's lines until EOF.
    fn pump(bridge: &Bridge, input: impl BufRead) {
        for line in input.lines() {
            match line {
                Ok(line) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    bridge.handle(&line);
                }
                // A line that is not UTF-8 is a broken client, not a dead one: the bytes have
                // already been consumed, so skipping cannot loop. Any other read error is the
                // pipe itself failing, and there is nothing further to read.
                Err(err) if err.kind() == ErrorKind::InvalidData => {
                    eprintln!("cide-hook mcp: dropped a line that was not UTF-8");
                }
                Err(err) => {
                    eprintln!("cide-hook mcp: stdin ended: {err}");
                    break;
                }
            }
        }
    }

    impl Bridge {
        fn new(out: Box<dyn Write + Send>, link: Link) -> Self {
            Self {
                link: Mutex::new(link),
                pending: Mutex::new(Vec::new()),
                out: Mutex::new(out),
            }
        }

        /// Start the socket→stdout half, if there is a socket.
        fn start_reader(self: Arc<Self>) {
            let cloned = match &*lock(&self.link) {
                Link::Up(stream) => stream.try_clone().ok(),
                Link::Down(_) => None,
            };

            match cloned {
                Some(stream) => {
                    std::thread::spawn(move || self.read_loop(stream));
                }
                None => {
                    // `go_down` only flips a link that is *up*, so this is a no-op in the
                    // ordinary "there was never a socket" case, and the honest answer in the
                    // freak case where `try_clone` failed on a live connection: with no reader
                    // thread every proxied request would hang for ever, which is strictly
                    // worse than having no tools.
                    self.go_down(Down::Lost);
                }
            }
        }

        fn read_loop(self: Arc<Self>, stream: UnixStream) {
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                // The only inspection on this side: a reply settles the id it answers, so the
                // pending list holds exactly the requests still owed one.
                if let Some(id) = response_id(&line) {
                    self.settle(&id);
                }
                self.emit_raw(line.as_bytes());
            }
            // EOF or a read error: cide has gone. Not an exit — see the module doc.
            self.go_down(Down::Lost);
        }

        /// Forward one client line, or answer it here if the link is down.
        fn handle(&self, raw: &str) {
            let inbound = classify(raw);

            let outcome = {
                let link = lock(&self.link);
                match &*link {
                    Link::Up(stream) => {
                        // Recorded *while the link lock is held*, so a reader thread that
                        // notices the death cannot drain the pending list in the window
                        // between this push and the write and leave the id stranded with
                        // nobody left to answer it.
                        if let Inbound::Request { id, .. } = &inbound {
                            lock(&self.pending).push(id.clone());
                        }
                        let mut socket: &UnixStream = stream;
                        match write_line(&mut socket, raw.as_bytes()) {
                            Ok(()) => Outcome::Sent,
                            Err(err) => {
                                eprintln!("cide-hook mcp: the socket write failed: {err}");
                                Outcome::Died
                            }
                        }
                    }
                    Link::Down(why) => Outcome::Offline(*why),
                }
            };

            match outcome {
                Outcome::Sent => {}
                // The request this call just pushed is in `pending`, so the drain answers it;
                // there is deliberately no second path for the message that saw the failure.
                Outcome::Died => self.go_down(Down::Lost),
                Outcome::Offline(why) => self.answer_here(&inbound, why),
            }
        }

        /// Answer a request from here, because there is no cide to proxy it to.
        fn answer_here(&self, inbound: &Inbound, why: Down) {
            let Inbound::Request { id, method, params } = inbound else {
                if let Inbound::Notification { method } = inbound {
                    // Dropped, never answered: replying to a notification is a protocol error.
                    eprintln!("cide-hook mcp: dropped {method}, cide is unreachable");
                }
                return;
            };

            let reply = match (method.as_str(), why) {
                ("initialize", Down::NoSocket) => ok(id, initialize_result(params)),
                ("tools/list", Down::NoSocket) => ok(id, json!({ "tools": [] })),
                // Answered in both degraded states: `ping` asks whether this process is alive,
                // and it is.
                ("ping", _) => ok(id, json!({})),
                _ => error(id, INTERNAL_ERROR, NO_CIDE),
            };
            self.emit(&reply);
        }

        /// Mark the link down and answer every request that will now never get a reply.
        fn go_down(&self, why: Down) {
            {
                let mut link = lock(&self.link);
                // Only ever Up → Down. A second caller must not overwrite `NoSocket` with
                // `Lost`, which would change how the client is answered for the rest of the
                // session.
                if let Link::Up(_) = &*link {
                    *link = Link::Down(why);
                }
            }

            // Drained unconditionally, even when another thread flipped the state first: the
            // loser of that race may still have pushed an id, and the winner's drain has
            // already run.
            let orphans = std::mem::take(&mut *lock(&self.pending));
            for id in orphans {
                self.emit(&error(&id, INTERNAL_ERROR, NO_CIDE));
            }
        }

        /// Wait, up to `grace`, for every forwarded request to be answered.
        fn drain(&self, grace: Duration) {
            let deadline = std::time::Instant::now() + grace;
            loop {
                if lock(&self.pending).is_empty() {
                    return;
                }
                if std::time::Instant::now() >= deadline {
                    // Not an error and not worth a reply: the client has closed stdin, so
                    // there is nowhere for these answers to go even if they arrive.
                    eprintln!("cide-hook mcp: exiting with answers still owed");
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn settle(&self, id: &Value) {
            let mut pending = lock(&self.pending);
            if let Some(at) = pending.iter().position(|owed| owed == id) {
                pending.swap_remove(at);
            }
        }

        fn emit(&self, message: &Value) {
            let Ok(line) = serde_json::to_vec(message) else {
                return;
            };
            self.emit_raw(&line);
        }

        /// Write one line to stdout. Verbatim for proxied traffic — re-serialising would
        /// reorder keys and can change how a number reads, and this end has no business
        /// rewriting either.
        fn emit_raw(&self, line: &[u8]) {
            let mut out = lock(&self.out);
            let _ = write_line(&mut *out, line);
        }
    }

    /// One `write_all` of the payload and its newline together.
    ///
    /// Assembled into a single buffer first so that concurrent writers cannot interleave a
    /// message with somebody else's newline, and so that a partial write is a partial line
    /// rather than a message missing its terminator.
    fn write_line(sink: &mut impl Write, line: &[u8]) -> std::io::Result<()> {
        let mut framed = Vec::with_capacity(line.len() + 1);
        framed.extend_from_slice(line);
        framed.push(b'\n');
        sink.write_all(&framed)?;
        sink.flush()
    }

    /// Classify a client line by its envelope. Nothing below `method`, `id` and `params` is
    /// looked at, and `params` only so a degraded `initialize` can echo the version.
    fn classify(raw: &str) -> Inbound {
        let Ok(Value::Object(message)) = serde_json::from_str::<Value>(raw) else {
            return Inbound::Malformed;
        };

        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return if message.contains_key("result") || message.contains_key("error") {
                Inbound::Response
            } else {
                Inbound::Malformed
            };
        };

        match message.get("id") {
            // A null id is treated as no id. JSON-RPC allows `"id": null` in a request, MCP
            // forbids it, and a reply carrying a null id cannot be matched by anyone — so the
            // safe reading is "expects no reply".
            Some(id) if !id.is_null() => Inbound::Request {
                id: id.clone(),
                method: method.to_owned(),
                params: message.get("params").cloned().unwrap_or(Value::Null),
            },
            _ => Inbound::Notification {
                method: method.to_owned(),
            },
        }
    }

    /// The id a line from the socket settles, if it is a reply at all.
    fn response_id(raw: &str) -> Option<Value> {
        let Ok(Value::Object(message)) = serde_json::from_str::<Value>(raw) else {
            return None;
        };
        if message.contains_key("method") {
            return None;
        }
        if !(message.contains_key("result") || message.contains_key("error")) {
            return None;
        }
        message.get("id").filter(|id| !id.is_null()).cloned()
    }

    fn initialize_result(params: &Value) -> Value {
        let version = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_PROTOCOL_VERSION);

        json!({
            "protocolVersion": version,
            // `tools` has to be present or the client never sends `tools/list` — and an empty
            // list is the entire point of this answer.
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "cide-bridge", "version": env!("CARGO_PKG_VERSION") },
        })
    }

    fn ok(id: &Value, result: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": id, "result": result })
    }

    fn error(id: &Value, code: i32, message: &str) -> Value {
        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
    }

    /// A poisoned mutex means another thread panicked. That is not a reason to take this
    /// process down — the whole point of the module is that the client keeps a live server —
    /// and the data behind each of these is a plain buffer or list, so recovering it is safe.
    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::Cursor;
        use std::os::unix::net::UnixListener;
        use std::path::PathBuf;

        /// stdout, redirected into a buffer the test can read.
        #[derive(Clone)]
        struct Sink(Arc<Mutex<Vec<u8>>>);

        impl Sink {
            fn new() -> Self {
                Self(Arc::new(Mutex::new(Vec::new())))
            }

            fn lines(&self) -> Vec<Value> {
                let bytes = lock(&self.0).clone();
                String::from_utf8(bytes)
                    .expect("stdout is utf-8")
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .map(|line| {
                        serde_json::from_str(line).expect("stdout is one json object per line")
                    })
                    .collect()
            }
        }

        impl Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                lock(&self.0).extend_from_slice(buf);
                Ok(buf.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        fn offline(why: Down) -> (Arc<Bridge>, Sink) {
            let sink = Sink::new();
            let bridge = Arc::new(Bridge::new(Box::new(sink.clone()), Link::Down(why)));
            (bridge, sink)
        }

        fn fake_env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
            let owned: Vec<(String, String)> = pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect();
            move |key| {
                owned
                    .iter()
                    .find(|(name, _)| name == key)
                    .map(|(_, value)| value.clone())
            }
        }

        #[test]
        fn a_request_a_notification_and_a_response_are_told_apart() {
            assert_eq!(
                classify(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"a":1}}"#),
                Inbound::Request {
                    id: json!(1),
                    method: "tools/list".into(),
                    params: json!({ "a": 1 }),
                }
            );
            assert_eq!(
                classify(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
                Inbound::Notification {
                    method: "notifications/initialized".into(),
                }
            );
            assert_eq!(
                classify(r#"{"jsonrpc":"2.0","id":7,"result":{"ok":true}}"#),
                Inbound::Response
            );
            assert_eq!(
                classify(r#"{"jsonrpc":"2.0","id":7,"error":{"code":-1,"message":"no"}}"#),
                Inbound::Response
            );
        }

        #[test]
        fn a_request_keeps_a_string_id_as_a_string() {
            // The id is echoed, never parsed: a client that numbers its requests "req-1"
            // must get "req-1" back, or every reply is unmatchable.
            let Inbound::Request { id, .. } =
                classify(r#"{"jsonrpc":"2.0","id":"req-1","method":"ping"}"#)
            else {
                panic!("a string id is still a request");
            };
            assert_eq!(id, json!("req-1"));
        }

        #[test]
        fn a_null_id_is_a_notification_and_junk_is_malformed() {
            assert_eq!(
                classify(r#"{"jsonrpc":"2.0","id":null,"method":"notifications/cancelled"}"#),
                Inbound::Notification {
                    method: "notifications/cancelled".into(),
                }
            );
            assert_eq!(classify("not json at all"), Inbound::Malformed);
            assert_eq!(classify("[1,2,3]"), Inbound::Malformed);
            assert_eq!(classify(r#"{"jsonrpc":"2.0"}"#), Inbound::Malformed);
        }

        #[test]
        fn only_a_reply_settles_a_pending_id() {
            assert_eq!(
                response_id(r#"{"jsonrpc":"2.0","id":3,"result":{}}"#),
                Some(json!(3))
            );
            assert_eq!(
                response_id(r#"{"jsonrpc":"2.0","id":"a","error":{"code":-32603,"message":"x"}}"#),
                Some(json!("a"))
            );
            // A server-initiated request travels the same direction as a reply and must not
            // be mistaken for one.
            assert_eq!(
                response_id(r#"{"jsonrpc":"2.0","id":3,"method":"roots/list"}"#),
                None
            );
            assert_eq!(response_id(r#"{"jsonrpc":"2.0","id":3}"#), None);
            assert_eq!(response_id("half a line"), None);
        }

        #[test]
        fn the_header_names_the_ids_from_the_environment() {
            let line = header_line(
                fake_env(&[
                    ("CIDE_RUN", "run-7"),
                    ("CIDE_SESSION", "01923f00-0000-7000-8000-000000000001"),
                    // A payload-shaped decoy: nothing but the three names above is read.
                    ("CIDE_SESSION_ID", "not-this-one"),
                ]),
                4242,
            );
            assert!(line.ends_with(b"\n"), "the header is one framed line");
            assert_eq!(
                line.iter().filter(|byte| **byte == b'\n').count(),
                1,
                "the header must not contain an embedded newline"
            );

            let header: Value = serde_json::from_slice(&line).expect("the header is json");
            assert_eq!(header["hello"], HELLO);
            assert_eq!(header["v"], HELLO_VERSION);
            assert_eq!(header["run"], "run-7");
            assert_eq!(header["session"], "01923f00-0000-7000-8000-000000000001");
            assert_eq!(header["pid"], 4242);
        }

        #[test]
        fn an_absent_or_empty_id_is_null_in_the_header() {
            let line = header_line(fake_env(&[("CIDE_RUN", "")]), 1);
            let header: Value = serde_json::from_slice(&line).expect("the header is json");
            // Empty is absent: binding a connection to `""` would give it an identity that
            // matches no run and no session.
            assert_eq!(header["run"], Value::Null);
            assert_eq!(header["session"], Value::Null);
            assert!(
                header.get("run").is_some(),
                "the key is present, the value is null"
            );
        }

        #[test]
        fn the_error_envelope_keeps_the_id_and_carries_no_result() {
            let envelope = error(&json!("req-1"), INTERNAL_ERROR, NO_CIDE);
            assert_eq!(envelope["jsonrpc"], "2.0");
            assert_eq!(envelope["id"], "req-1");
            assert_eq!(envelope["error"]["code"], -32603);
            assert!(
                envelope["error"]["message"]
                    .as_str()
                    .expect("a message")
                    .contains("cide"),
                "the message must name the process that is missing"
            );
            assert!(
                envelope.get("result").is_none(),
                "a JSON-RPC reply carries result or error, never both"
            );

            let good = ok(&json!(2), json!({ "tools": [] }));
            assert!(good.get("error").is_none());
            assert_eq!(good["result"]["tools"], json!([]));
        }

        #[test]
        fn a_bridge_that_never_connected_still_speaks_mcp() {
            let (bridge, sink) = offline(Down::NoSocket);
            pump(
                &bridge,
                Cursor::new(concat!(
                    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}"#,
                    "\n",
                    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                    "\n",
                    r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
                    "\n",
                    r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"cide_task_list"}}"#,
                    "\n",
                )),
            );

            let out = sink.lines();
            // Three replies for three requests, and nothing at all for the notification.
            assert_eq!(out.len(), 3, "got {out:?}");
            assert_eq!(out[0]["id"], 1);
            assert_eq!(out[0]["result"]["protocolVersion"], "2025-11-25");
            assert!(out[0]["result"]["capabilities"]["tools"].is_object());
            assert_eq!(out[1]["id"], 2);
            assert_eq!(out[1]["result"]["tools"], json!([]));
            assert_eq!(out[2]["id"], 3);
            assert_eq!(out[2]["error"]["code"], -32603);
        }

        #[test]
        fn an_initialize_without_a_version_falls_back_rather_than_inventing() {
            let (bridge, sink) = offline(Down::NoSocket);
            pump(
                &bridge,
                Cursor::new("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n"),
            );
            let out = sink.lines();
            assert_eq!(
                out[0]["result"]["protocolVersion"],
                DEFAULT_PROTOCOL_VERSION
            );
        }

        #[test]
        fn a_lost_link_errors_where_a_missing_one_answers() {
            let (bridge, sink) = offline(Down::Lost);
            pump(
                &bridge,
                Cursor::new(concat!(
                    r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
                    "\n",
                    r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
                    "\n",
                )),
            );

            let out = sink.lines();
            assert_eq!(out.len(), 2);
            // The client already holds a tool list; claiming there are none would be a
            // different lie from "cide is gone".
            assert_eq!(out[0]["error"]["code"], -32603);
            // `ping` is about this process, which is alive in both degraded states.
            assert_eq!(out[1]["result"], json!({}));
        }

        #[test]
        fn malformed_and_unanswerable_lines_produce_no_output() {
            let (bridge, sink) = offline(Down::NoSocket);
            pump(
                &bridge,
                Cursor::new(concat!(
                    "\n",
                    "half a line\n",
                    r#"{"jsonrpc":"2.0","id":9,"result":{}}"#,
                    "\n",
                )),
            );
            // Nothing is owed for junk or for a reply, and writing anything here would put a
            // line on the transport that the client cannot match to a request.
            assert!(sink.lines().is_empty());
        }

        fn temp_dir(name: &str) -> PathBuf {
            let dir =
                std::env::temp_dir().join(format!("cide-hook-mcp-{}-{name}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("a temp dir");
            dir
        }

        #[test]
        fn a_connection_that_dies_leaves_no_request_unanswered() {
            let dir = temp_dir("dies");
            let path = dir.join("agents.sock");
            let _ = std::fs::remove_file(&path);
            let listener = UnixListener::bind(&path).expect("bind");

            let env = fake_env(&[
                ("CIDE_AGENT_SOCK", &path.display().to_string()),
                ("CIDE_RUN", "run-7"),
            ]);
            let link = open(&env, 4242);
            assert!(matches!(link, Link::Up(_)), "the listener is up");

            // The app side of the handshake: read the header, then hang up mid-session.
            let (server, _) = listener.accept().expect("accept");
            let mut header = String::new();
            BufReader::new(&server)
                .read_line(&mut header)
                .expect("a header line");
            let header: Value = serde_json::from_str(&header).expect("the header is json");
            assert_eq!(header["hello"], HELLO);
            assert_eq!(header["run"], "run-7");
            assert_eq!(header["session"], Value::Null);
            drop(server);
            drop(listener);

            let sink = Sink::new();
            let bridge = Arc::new(Bridge::new(Box::new(sink.clone()), link));
            Arc::clone(&bridge).start_reader();
            pump(
                &bridge,
                Cursor::new("{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"tools/call\"}\n"),
            );

            // Whichever half notices the death — the failing write here, or the reader thread
            // reaching EOF — the client gets exactly one answer for id 9 and never hangs.
            let mut out = sink.lines();
            for _ in 0..200 {
                if !out.is_empty() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
                out = sink.lines();
            }
            assert_eq!(out.len(), 1, "exactly one answer for one request: {out:?}");
            assert_eq!(out[0]["id"], 9);
            assert_eq!(out[0]["error"]["code"], -32603);

            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_dir(&dir);
        }

        #[test]
        fn a_drain_returns_at_once_when_nothing_is_owed_and_gives_up_when_something_is() {
            let (bridge, _sink) = offline(Down::NoSocket);
            let started = std::time::Instant::now();
            bridge.drain(Duration::from_secs(5));
            assert!(
                started.elapsed() < Duration::from_millis(500),
                "an ordinary shutdown owes nothing and must not pay for the grace period"
            );

            lock(&bridge.pending).push(json!(1));
            let started = std::time::Instant::now();
            bridge.drain(Duration::from_millis(30));
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "an app that never answers must not keep this process alive"
            );
        }

        #[test]
        fn a_refused_socket_degrades_instead_of_failing() {
            let dir = temp_dir("refused");
            let path = dir.join("nothing-here.sock");
            let _ = std::fs::remove_file(&path);

            let link = open(
                fake_env(&[("CIDE_AGENT_SOCK", &path.display().to_string())]),
                1,
            );
            assert!(matches!(link, Link::Down(Down::NoSocket)));

            // And an unset variable is the same state, which is what makes launching cide-hook
            // with no IDE running produce a server with no tools rather than a crash.
            assert!(matches!(open(fake_env(&[]), 1), Link::Down(Down::NoSocket)));
            let _ = std::fs::remove_dir(&dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cide-hook-forward-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp dir");
        dir
    }

    /// The two halves are independent, and this is the assertion that says so.
    ///
    /// A payload that is about to be denied still produces exactly the frame an allowed one
    /// does. If a deny ever suppressed the frame it would take the session's state transition
    /// with it — `cide_claude::state` moves a session to `Busy` on `PreToolUse` — and the pane
    /// would look idle through a turn it is actually running. It would also lose the one record
    /// anybody has that the refusal happened.
    #[test]
    fn the_frame_goes_out_whatever_the_decision_is() {
        let dir = temp_dir("frame");
        let path = dir.join("hooks.sock");
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind");

        let denied = serde_json::json!({
            "cwd": "/home/dev/proj",
            "tool_name": "Edit",
            "tool_input": { "file_path": ".cide/tasks.json" },
        })
        .to_string();
        let allowed = serde_json::json!({
            "cwd": "/home/dev/proj",
            "tool_name": "Edit",
            "tool_input": { "file_path": "src/main.rs" },
        })
        .to_string();

        // The decision half says one thing about these two payloads...
        assert!(guard::deny_line("PreToolUse", &denied).is_some());
        assert!(guard::deny_line("PreToolUse", &allowed).is_none());

        // ...and the socket half says the same thing about both.
        for payload in [&denied, &allowed] {
            forward_to(&path, "PreToolUse", payload, Some("sess-1".to_owned()));

            let (stream, _) = listener.accept().expect("accept");
            let mut line = String::new();
            BufReader::new(stream)
                .read_line(&mut line)
                .expect("one frame");

            let frame: serde_json::Value = serde_json::from_str(&line).expect("a json frame");
            assert_eq!(frame["event"], "PreToolUse");
            assert_eq!(frame["spawned_as"], "sess-1");
            assert_eq!(frame["payload"]["tool_name"], "Edit");
        }

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    /// An unparseable payload is forwarded as a string rather than dropped, and that is still
    /// true for an event the guard also inspects — the guard's own silence about it changes
    /// nothing here.
    #[test]
    fn an_unparseable_payload_is_still_reported() {
        let dir = temp_dir("junk");
        let path = dir.join("hooks.sock");
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind");

        assert!(guard::deny_line("PreToolUse", "not json").is_none());
        forward_to(&path, "PreToolUse", "not json", None);

        let (stream, _) = listener.accept().expect("accept");
        let mut line = String::new();
        BufReader::new(stream)
            .read_line(&mut line)
            .expect("one frame");
        let frame: serde_json::Value = serde_json::from_str(&line).expect("a json frame");
        assert_eq!(frame["payload"], "not json");
        assert_eq!(frame["spawned_as"], serde_json::Value::Null);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
