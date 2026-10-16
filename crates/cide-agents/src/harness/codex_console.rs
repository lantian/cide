//! The native Codex terminal and peer delivery share one app-server connection.
//!
//! Track the terminal's successful start/resume replies, rather than global thread-created
//! notifications: those also describe subagents and previously visited conversations. Internal
//! turn requests have their own ids; their acknowledgements never reach the terminal, while
//! every notification and every approval/question does. No screen recognition or hook-trust
//! bypass is needed to learn whether this conversation can receive another turn.
use cide_ipc::SessionId;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receiver {
    pub connection: u64,
    pub thread: String,
    pub generation: u64,
}

#[derive(Debug)]
pub struct Hook {
    pub kind: &'static str,
    pub payload: Value,
}

#[derive(Default, Debug)]
pub struct Protocol {
    connection: u64,
    active: Option<String>,
    generation: u64,
    ready: bool,
    initialized: bool,
    pending: HashMap<String, Request>,
    questions: HashSet<String>,
    last_answer: String,
    model: Option<String>,
    cwd: Option<String>,
    path: Option<String>,
}

#[derive(Debug)]
struct Request {
    method: String,
    user_prompt: bool,
}

impl Protocol {
    pub fn new(connection: u64) -> Self {
        Self {
            connection,
            ..Self::default()
        }
    }

    pub fn receiver(&self) -> Option<Receiver> {
        if !self.initialized
            || !self.ready
            || !self.questions.is_empty()
            || !self.pending.is_empty()
        {
            return None;
        }
        Some(Receiver {
            connection: self.connection,
            thread: self.active.clone()?,
            generation: self.generation,
        })
    }

    /// Observe native requests before forwarding them. A manual submit or conversation
    /// switch must close the delivery gate before the server has time to acknowledge it.
    pub fn from_terminal(&mut self, frame: &Value) {
        let id = frame.get("id").map(Value::to_string);
        let method = frame["method"].as_str();
        if let (Some(id), Some(method)) = (id.as_ref(), method) {
            match method {
                // The native TUI also starts ephemeral structured-output helpers on this
                // connection. They are not the conversation displayed in its terminal.
                "thread/start" if frame["params"]["ephemeral"] == true => {}
                "thread/start" | "thread/resume" | "thread/fork" => {
                    self.generation += 1;
                    self.ready = false;
                    self.active = None;
                    self.questions.clear();
                    self.pending.clear();
                    self.pending.insert(
                        id.clone(),
                        Request {
                            method: method.to_string(),
                            user_prompt: false,
                        },
                    );
                }
                "turn/start" | "turn/steer"
                    if self.active.is_some()
                        && frame["params"]["threadId"].as_str() == self.active.as_deref() =>
                {
                    self.ready = false;
                    let user_prompt = method == "turn/start"
                        && frame["params"]["input"].as_array().is_some_and(|input| {
                            input.iter().any(|part| {
                                part["type"] == "text"
                                    && part["text"]
                                        .as_str()
                                        .is_some_and(|text| !text.trim().is_empty())
                            })
                        });
                    self.pending.insert(
                        id.clone(),
                        Request {
                            method: method.to_string(),
                            user_prompt,
                        },
                    );
                }
                "thread/unsubscribe"
                    if frame["params"]["threadId"].as_str() == self.active.as_deref() =>
                {
                    self.generation += 1;
                    self.active = None;
                    self.ready = false;
                    self.pending.clear();
                }
                _ => {}
            }
        }
        if method == Some("initialized") {
            self.initialized = true;
        }
        if method.is_none()
            && let Some(id) = id
        {
            self.questions.remove(&id);
            // Resolving a question does not make a running turn idle.
        }
    }

    pub fn from_server(&mut self, frame: &Value) -> Vec<Hook> {
        let mut hooks = Vec::new();
        if frame.get("method").is_none()
            && let Some(id) = frame.get("id").map(Value::to_string)
            && let Some(request) = self.pending.remove(&id)
        {
            if frame.get("error").is_some() {
                return hooks;
            }
            if matches!(
                request.method.as_str(),
                "thread/start" | "thread/resume" | "thread/fork"
            ) {
                let thread = &frame["result"]["thread"];
                if let Some(id) = thread["id"]
                    .as_str()
                    .filter(|id| id.parse::<SessionId>().is_ok())
                {
                    self.active = Some(id.to_string());
                    self.last_answer.clear();
                    self.model = frame["result"]["model"].as_str().map(str::to_string);
                    self.cwd = frame["result"]["cwd"].as_str().map(str::to_string);
                    self.path = thread["path"].as_str().map(str::to_string);
                    let mut payload = self.payload();
                    payload["source"] = json!("resume");
                    hooks.push(Hook {
                        kind: "SessionStart",
                        payload,
                    });
                    hooks.push(Hook {
                        kind: "statusline",
                        payload: self.readout(None),
                    });
                    hooks.extend(self.status(&thread["status"]));
                }
            } else if request.method == "turn/start" {
                self.ready = false;
                if request.user_prompt {
                    hooks.extend(self.hook("CideUserPrompt"));
                }
            } else if request.method == "turn/steer" {
                self.ready = false;
            }
        }
        let params = &frame["params"];
        if params["threadId"].as_str() != self.active.as_deref() || self.active.is_none() {
            return hooks;
        }
        if let Some(id) = frame.get("id").map(Value::to_string) {
            self.questions.insert(id);
            self.ready = false;
            hooks.extend(
                self.hook(
                    if frame["method"]
                        .as_str()
                        .is_some_and(|m| m.ends_with("requestApproval"))
                    {
                        "CodexApprovalPrompt"
                    } else {
                        "CideAwaitingInput"
                    },
                ),
            );
        }
        match frame["method"].as_str().unwrap_or("") {
            "thread/status/changed" => hooks.extend(self.status(&params["status"])),
            "turn/started" => {
                self.ready = false;
                self.last_answer.clear();
                hooks.extend(self.hook("UserPromptSubmit"));
            }
            "turn/completed" => {
                self.ready = true;
                hooks.extend(self.hook(if params["turn"]["status"] == "interrupted" {
                    "Interrupt"
                } else {
                    "Stop"
                }));
            }
            "serverRequest/resolved" => {
                if let Some(id) = params.get("requestId") {
                    self.questions.remove(&id.to_string());
                }
            }
            "thread/tokenUsage/updated" => hooks.push(Hook {
                kind: "statusline",
                payload: self.readout(Some(&params["tokenUsage"])),
            }),
            "item/completed" if params["item"]["type"] == "agentMessage" => {
                self.last_answer = params["item"]["text"].as_str().unwrap_or("").to_string();
            }
            "item/completed" if params["item"]["type"] == "fileChange" => {
                let mut payload = self.payload();
                payload["tool_name"] = json!("Write");
                for path in params["item"]["changes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|change| change["path"].as_str())
                {
                    payload["tool_input"] = json!({"file_path": path});
                    hooks.push(Hook {
                        kind: "PostToolUse",
                        payload: payload.clone(),
                    });
                }
            }
            _ => {}
        }
        hooks
    }

    fn status(&mut self, status: &Value) -> Vec<Hook> {
        self.ready = status["type"] == "idle";
        let kind = match status["type"].as_str() {
            // Native completion already emits Stop. A second Stop would clear Cide's
            // AwaitingInput attention marker by moving it back to Idle. Status supplies
            // readiness; only the turn event establishes that work finished.
            Some("idle") => return Vec::new(),
            Some("active")
                if status["activeFlags"]
                    .as_array()
                    .is_some_and(|flags| flags.iter().any(|v| v == "waitingOnApproval")) =>
            {
                "CodexApprovalPrompt"
            }
            Some("active")
                if status["activeFlags"]
                    .as_array()
                    .is_some_and(|flags| flags.iter().any(|v| v == "waitingOnUserInput")) =>
            {
                "CideAwaitingInput"
            }
            Some("active") => "UserPromptSubmit",
            _ => return Vec::new(),
        };
        self.hook(kind)
    }

    fn payload(&self) -> Value {
        let mut payload =
            json!({"session_id": self.active, "last_assistant_message": self.last_answer});
        if let Some(model) = &self.model {
            payload["model"] = json!(model);
        }
        if let Some(cwd) = &self.cwd {
            payload["cwd"] = json!(cwd);
        }
        if let Some(path) = &self.path {
            payload["transcript_path"] = json!(path);
        }
        payload
    }

    fn readout(&self, usage: Option<&Value>) -> Value {
        let mut status = json!({"harness": "codex", "model": {"id": self.model}});
        if let Some(usage) = usage {
            status["context_window"] = json!({
                "context_window_size": usage["modelContextWindow"],
                "current_usage": usage["last"]["totalTokens"],
                "total_input_tokens": usage["total"]["inputTokens"],
                "total_output_tokens": usage["total"]["outputTokens"]
            });
        }
        status
    }

    fn hook(&self, kind: &'static str) -> Vec<Hook> {
        self.active
            .as_ref()
            .map(|_| Hook {
                kind,
                payload: self.payload(),
            })
            .into_iter()
            .collect()
    }

    /// Called in the same forwarding loop as manual requests. False means nothing was sent;
    /// the peer lane can keep waiting. An accepted claim stays busy until native completion.
    pub fn claim(&mut self, receiver: &Receiver) -> bool {
        if self.receiver().as_ref() != Some(receiver) {
            return false;
        }
        self.ready = false;
        true
    }
}

pub fn turn_request(id: &str, receiver: &Receiver, body: &str) -> Value {
    json!({"id": id, "method": "turn/start", "params": {
        "threadId": receiver.thread, "input": [{"type": "text", "text": body}]
    }})
}

/// Only a matching, positive turn acknowledgement establishes delivery. Errors from the
/// server are definite rejection; malformed replies and transport loss are uncertain.
pub fn acknowledgement(frame: &Value) -> Result<(), (bool, String)> {
    if let Some(error) = frame.get("error") {
        return Err((
            false,
            format!(
                "Codex rejected the peer message: {}",
                error["message"].as_str().unwrap_or("protocol error")
            ),
        ));
    }
    let turn = &frame["result"]["turn"];
    if turn["id"].as_str().is_some_and(|id| !id.is_empty())
        && matches!(
            turn["status"].as_str(),
            Some("inProgress" | "completed" | "failed" | "interrupted")
        )
    {
        Ok(())
    } else {
        Err((true, "Codex did not confirm acceptance. Inspect the receiving conversation before resending.".into()))
    }
}

/// Config overrides must also reach the process that actually runs tools. TUI-only options,
/// native thread ids and prompts remain on the terminal's original argv.
pub fn server_overrides(args: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    let mut source = args.iter();
    while let Some(arg) = source.next() {
        if arg == "--" {
            break;
        }
        if matches!(arg.as_str(), "-c" | "--config" | "--enable" | "--disable") {
            if let Some(value) = source.next() {
                result.extend([arg.clone(), value.clone()]);
            }
        } else if let Some(value) = arg.strip_prefix("--config=") {
            result.extend(["-c".into(), value.to_string()]);
        } else if ["--enable=", "--disable="]
            .iter()
            .any(|prefix| arg.starts_with(prefix))
        {
            result.push(arg.clone());
        } else if let Some(value) = arg.strip_prefix("-c").filter(|value| !value.is_empty()) {
            result.extend(["-c".into(), value.trim_start_matches('=').to_string()]);
        }
    }
    result
}

/// Remote resume rejects interactive permission flags, even though its native protocol
/// accepts the equivalent invocation-local configuration. Keep policy on both the actual
/// server and the terminal's config overrides; never discard a reviewer's read-only policy.
pub fn remote_args(args: &[String]) -> Vec<String> {
    use cide_core::codex_cli::{push_config, toml_string};
    let mut result = Vec::new();
    let mut source = args.iter();
    while let Some(arg) = source.next() {
        if arg == "--" {
            result.push(arg.clone());
            result.extend(source.cloned());
            break;
        }
        let flag = arg.split('=').next().unwrap_or(arg);
        let key = match flag {
            "-s" | "--sandbox" => Some("sandbox_mode"),
            "-a" | "--ask-for-approval" => Some("approval_policy"),
            _ => None,
        };
        if let Some(key) = key {
            if let Some(value) = arg
                .split_once('=')
                .map(|(_, value)| value)
                .or_else(|| source.next().map(String::as_str))
            {
                push_config(&mut result, key, toml_string(value));
            }
        } else if (arg.starts_with("-s") || arg.starts_with("-a"))
            && !arg.starts_with("--")
            && arg.len() > 2
        {
            push_config(
                &mut result,
                if arg.starts_with("-s") {
                    "sandbox_mode"
                } else {
                    "approval_policy"
                },
                toml_string(arg[2..].trim_start_matches('=')),
            );
        } else if matches!(
            arg.as_str(),
            "--dangerously-bypass-approvals-and-sandbox" | "--yolo"
        ) {
            push_config(
                &mut result,
                "sandbox_mode",
                toml_string("danger-full-access"),
            );
            push_config(&mut result, "approval_policy", toml_string("never"));
        } else if matches!(arg.as_str(), "--approve-for-me" | "--full-auto") {
            push_config(&mut result, "sandbox_mode", toml_string("workspace-write"));
            push_config(&mut result, "approval_policy", toml_string("on-request"));
            if arg == "--approve-for-me" {
                push_config(
                    &mut result,
                    "approvals_reviewer",
                    toml_string("auto_review"),
                );
            }
        } else {
            result.push(arg.clone());
            if matches!(
                flag,
                "-c" | "--config"
                    | "-m"
                    | "--model"
                    | "-C"
                    | "--cd"
                    | "-p"
                    | "--profile"
                    | "--local-provider"
                    | "--add-dir"
            ) && !arg.contains('=')
                && let Some(value) = source.next()
            {
                result.push(value.clone());
            }
        }
    }
    result
}

/// Carry the server's resolved permission policy into native thread requests. Other
/// terminal settings stay on their existing path, including runtime model choices.
pub fn thread_policy(config: &Value, args: &[String]) -> Value {
    let mut policy = json!({"config":{},"sandbox":null,"permissions":null,"approvalPolicy":config["approval_policy"],"approvalsReviewer":config["approvals_reviewer"]});
    for key in [
        "sandbox_mode",
        "sandbox_workspace_write",
        "approval_policy",
        "approvals_reviewer",
        "permissions",
        "default_permissions",
    ] {
        if let Some(value) = config.get(key).filter(|value| !value.is_null()) {
            let mut value = value.clone();
            omit_null_config_fields(&mut value);
            policy["config"][key] = value;
        }
    }
    let overrides = server_overrides(args);
    let explicit_sandbox = overrides.windows(2).any(|pair| {
        pair[0] == "-c"
            && pair[1]
                .split_once('=')
                .is_some_and(|(key, _)| key.trim() == "sandbox_mode")
    });
    if !explicit_sandbox && let Some(permissions) = config["default_permissions"].as_str() {
        policy["permissions"] = json!(permissions);
    } else if let Some(sandbox) = config["sandbox_mode"].as_str() {
        policy["sandbox"] = json!(sandbox);
    }
    policy
}

/// config/read reports absent typed fields as JSON null. Thread config overrides
/// are converted to TOML, where null becomes an empty string: forwarding a profile's
/// workspace_roots:null makes even thread/start fail to deserialize the profile.
fn omit_null_config_fields(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            fields.retain(|_, value| !value.is_null());
            for value in fields.values_mut() {
                omit_null_config_fields(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                omit_null_config_fields(value);
            }
        }
        _ => {}
    }
}

pub fn apply_policy(frame: &mut Value, policy: &Value) -> bool {
    let selects = matches!(
        frame["method"].as_str(),
        Some("thread/start" | "thread/resume" | "thread/fork")
    ) && frame["params"]["ephemeral"] != true;
    if selects && let Some(overrides) = policy.as_object() {
        for (key, value) in overrides {
            if key == "config" {
                let mut config = frame["params"]["config"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                if let Some(values) = value.as_object() {
                    config.extend(values.clone());
                }
                frame["params"]["config"] = json!(config);
            } else {
                frame["params"][key] = value.clone();
            }
        }
    }
    selects
}

#[cfg(test)]
mod tests {
    use super::*;
    const THREAD: &str = "00000000-0000-4000-8000-000000000001";

    fn opened() -> Protocol {
        let mut p = Protocol::default();
        p.from_terminal(&json!({"method": "initialized"}));
        p.from_terminal(&json!({"id": 1, "method": "thread/start"}));
        p.from_server(
            &json!({"id": 1, "result": {"thread": {"id": THREAD, "status": {"type": "idle"}}}}),
        );
        p
    }

    #[test]
    fn fresh_terminal_thread_accepts_an_exact_native_message() {
        let mut p = opened();
        let receiver = p.receiver().unwrap();
        let body = "Chat from Peer (opencode): Привет 世界 😀 ".repeat(1000);
        let request = turn_request("internal", &receiver, &body);
        assert_eq!(request["params"]["input"][0]["text"], body);
        assert_eq!(request["params"]["threadId"], THREAD);
        assert!(p.claim(&receiver));
        assert!(p.receiver().is_none());
        assert!(
            acknowledgement(&json!({"result":{"turn":{"id":"turn-1","status":"inProgress"}}}))
                .is_ok()
        );
    }

    #[test]
    fn busy_and_permission_waits_never_claim_a_receiver() {
        let mut p = opened();
        let old = p.receiver().unwrap();
        p.from_terminal(&json!({"id":2,"method":"turn/start","params":{"threadId":THREAD}}));
        assert!(!p.claim(&old));
        p.from_server(&json!({"id":2,"result":{"turn":{"id":"turn-1","status":"inProgress"}}}));
        p.from_server(&json!({"method":"turn/completed","params":{"threadId":THREAD}}));
        p.from_server(&json!({"id":55,"method":"item/commandExecution/requestApproval","params":{"threadId":THREAD}}));
        p.from_server(&json!({"method":"thread/status/changed","params":{"threadId":THREAD,"status":{"type":"idle"}}}));
        assert!(
            p.receiver().is_none(),
            "a pending dialog wins over stale idle events"
        );
        p.from_terminal(&json!({"id":55,"result":{"decision":"accept"}}));
        p.from_server(&json!({"method":"turn/completed","params":{"threadId":THREAD}}));
        assert!(p.receiver().is_some());
    }

    #[test]
    fn switches_and_subagents_cannot_receive_a_stale_reply() {
        let mut p = opened();
        let old = p.receiver().unwrap();
        p.from_server(&json!({"method":"thread/started","params":{"thread":{"id":"other"}}}));
        assert_eq!(p.receiver(), Some(old.clone()));
        p.from_terminal(&json!({"id":"temporary-structured","method":"thread/start","params":{"ephemeral":true}}));
        p.from_server(&json!({"id":"temporary-structured","result":{"thread":{"id":"00000000-0000-4000-8000-000000000002","status":{"type":"idle"}}}}));
        p.from_terminal(&json!({"id":"helper-turn","method":"turn/start","params":{"threadId":"00000000-0000-4000-8000-000000000002","input":[{"type":"text","text":"Summarize the session"}]}}));
        assert!(
            p.from_server(
                &json!({"id":"helper-turn","result":{"turn":{"id":"helper","status":"inProgress"}}})
            )
            .is_empty()
        );
        p.from_terminal(&json!({"id":3,"method":"thread/unsubscribe","params":{"threadId":"00000000-0000-4000-8000-000000000002"}}));
        assert_eq!(
            p.receiver(),
            Some(old.clone()),
            "native structured-output helpers must not replace the visible conversation"
        );
        p.from_terminal(&json!({"id":2,"method":"thread/resume","params":{"threadId":THREAD}}));
        assert!(!p.claim(&old));
        p.from_server(&json!({"id":2,"result":{"thread":{"id":THREAD,"status":{"type":"idle"}}}}));
        assert_ne!(p.receiver().unwrap().generation, old.generation);
        assert!(!p.claim(&old));
    }

    #[test]
    fn rejection_is_definite_but_an_unrecognised_reply_is_uncertain() {
        assert!(
            !acknowledgement(&json!({"error":{"message":"busy"}}))
                .unwrap_err()
                .0
        );
        assert!(acknowledgement(&json!({"result":{}})).unwrap_err().0);
    }

    #[test]
    fn server_gets_hooks_mcp_and_profiles_but_never_a_prompt() {
        let args = [
            "resume",
            "-m",
            "model",
            "-c",
            "hooks.SessionStart=[]",
            "--config=mcp_servers.cide.command=hook",
            "--enable",
            "feature",
            "--dangerously-bypass-hook-trust",
            THREAD,
            "--",
            "--config=not-a-setting",
        ];
        assert_eq!(
            server_overrides(&args.map(str::to_string)),
            [
                "-c",
                "hooks.SessionStart=[]",
                "-c",
                "mcp_servers.cide.command=hook",
                "--enable",
                "feature"
            ]
        );
    }

    #[test]
    fn remote_resume_retains_permission_policy_without_rejected_tui_flags() {
        let args = [
            "resume",
            "-s",
            "read-only",
            "--ask-for-approval=on-request",
            "-c",
            "approvals_reviewer=\"user\"",
            THREAD,
        ];
        let launch = remote_args(&args.map(str::to_string));
        assert_eq!(
            launch,
            [
                "resume",
                "-c",
                "sandbox_mode=\"read-only\"",
                "-c",
                "approval_policy=\"on-request\"",
                "-c",
                "approvals_reviewer=\"user\"",
                THREAD
            ]
        );
        assert!(
            server_overrides(&launch)
                .iter()
                .any(|arg| arg == "sandbox_mode=\"read-only\"")
        );
    }

    #[test]
    fn native_policy_preserves_profiles_and_other_runtime_settings() {
        let config = json!({"default_permissions":"cide-git","sandbox_mode":"read-only","approval_policy":"on-request","approvals_reviewer":"user","permissions":{"cide-git":{"filesystem":{"/repo/.git":"write"}}}});
        let policy = thread_policy(&config, &[]);
        assert_eq!(policy["permissions"], "cide-git");
        assert!(policy["sandbox"].is_null());
        let readonly = thread_policy(&config, &["-c".into(), "sandbox_mode=\"read-only\"".into()]);
        assert_eq!(readonly["sandbox"], "read-only");
        assert!(readonly["permissions"].is_null());
        let mut frame = json!({"method":"thread/resume","params":{"threadId":THREAD,"config":{"model_reasoning_effort":"high"}}});
        assert!(apply_policy(&mut frame, &readonly));
        assert_eq!(frame["params"]["config"]["model_reasoning_effort"], "high");
        assert_eq!(frame["params"]["sandbox"], "read-only");
        assert_eq!(frame["params"]["approvalPolicy"], "on-request");
        let mut helper = json!({"method":"thread/start","params":{"ephemeral":true}});
        let before = helper.clone();
        assert!(!apply_policy(&mut helper, &readonly));
        assert_eq!(helper, before);
    }

    #[test]
    fn native_profile_overrides_omit_absent_typed_fields() {
        let config = json!({"default_permissions":"cide-peer-reviewer","approval_policy":"never","permissions":{"cide-peer-reviewer":{"extends":":read-only","description":null,"workspace_roots":null,"network":null,"filesystem":{"/scratch":"write","glob_scan_max_depth":null}}}});
        let policy = thread_policy(&config, &[]);
        for method in ["thread/start", "thread/resume"] {
            let mut frame = json!({"method":method,"params":{}});
            assert!(apply_policy(&mut frame, &policy));
            assert_eq!(frame["params"]["permissions"], "cide-peer-reviewer");
            assert_eq!(
                frame["params"]["config"]["permissions"]["cide-peer-reviewer"],
                json!({"extends":":read-only","filesystem":{"/scratch":"write"}})
            );
        }
    }

    #[test]
    fn native_readouts_and_edits_enter_the_existing_hook_shapes() {
        let mut p = opened();
        let hooks = p.from_server(&json!({"method":"thread/tokenUsage/updated","params":{"threadId":THREAD,"tokenUsage":{"modelContextWindow":1000,"last":{"totalTokens":80},"total":{"inputTokens":50,"outputTokens":30}}}}));
        assert_eq!(hooks[0].kind, "statusline");
        assert_eq!(hooks[0].payload["context_window"]["current_usage"], 80);
        let hooks = p.from_server(&json!({"method":"item/completed","params":{"threadId":THREAD,"item":{"type":"fileChange","changes":[{"path":"/repo/a.rs"},{"path":"/repo/b.rs"}]}}}));
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[1].kind, "PostToolUse");
        assert_eq!(hooks[1].payload["tool_input"]["file_path"], "/repo/b.rs");
    }

    #[test]
    fn only_a_real_native_user_prompt_can_resume_a_paused_pair() {
        let mut p = opened();
        p.from_terminal(&json!({"id":2,"method":"turn/start","params":{"threadId":THREAD,"toolOutput":{"name":"check","output":"ok"},"input":[]}}));
        assert!(
            p.from_server(&json!({"id":2,"result":{"turn":{"id":"t1","status":"inProgress"}}}))
                .is_empty()
        );
        p.from_terminal(&json!({"id":3,"method":"turn/start","params":{"threadId":THREAD,"input":[{"type":"text","text":"Do the task"}]}}));
        let hooks =
            p.from_server(&json!({"id":3,"result":{"turn":{"id":"t2","status":"inProgress"}}}));
        assert_eq!(hooks[0].kind, "CideUserPrompt");
        p.from_terminal(&json!({"id":4,"method":"turn/start","params":{"threadId":THREAD,"input":[{"type":"text","text":"retired task"}]}}));
        p.from_terminal(&json!({"id":5,"method":"thread/start"}));
        assert!(
            p.from_server(
                &json!({"id":4,"result":{"turn":{"id":"retired","status":"inProgress"}}})
            )
            .is_empty()
        );
        assert!(p.receiver().is_none());
    }

    #[test]
    fn idle_status_does_not_clear_finished_turn_attention() {
        let mut p = opened();
        p.from_server(&json!({"method":"turn/started","params":{"threadId":THREAD}}));
        let hooks = p.from_server(&json!({"method":"turn/completed","params":{"threadId":THREAD,"turn":{"status":"completed"}}}));
        assert_eq!(hooks[0].kind, "Stop");
        assert!(p.from_server(&json!({"method":"thread/status/changed","params":{"threadId":THREAD,"status":{"type":"idle"}}})).is_empty());
        assert!(p.receiver().is_some());
        let hooks = p.from_server(&json!({"method":"turn/completed","params":{"threadId":THREAD,"turn":{"status":"interrupted"}}}));
        assert_eq!(hooks[0].kind, "Interrupt");
    }
}
