//! Native planning is a worker lifecycle, independent of its execution permissions.
//!
//! Nothing here writes instructions into a prompt. A harness enters its own planning mode;
//! the host approves only that mode's completed plan, once per assignment.
use cide_ipc::SessionId;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Planning,
    Approving,
    Implementing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub generation: SessionId,
    pub phase: Phase,
}

impl Default for State {
    fn default() -> Self {
        Self {
            generation: SessionId::new(),
            phase: Phase::Planning,
        }
    }
}

impl State {
    pub fn pending(&self) -> bool {
        self.phase != Phase::Implementing
    }
}

/// Select the harness's built-in instructions, never a Cide-authored planning prompt.
pub fn codex_mode(model: &str, effort: Option<&str>, phase: Phase) -> Value {
    json!({"mode": if phase == Phase::Implementing { "default" } else { "plan" },
        "settings": { "model": model, "reasoning_effort": effort,
            "developer_instructions": null }})
}

/// A completed native proposed-plan item, not an update_plan checklist or arbitrary prose.
pub fn codex_proposal(frame: &Value) -> Option<&str> {
    (frame["method"] == "item/completed" && frame["params"]["item"]["type"] == "plan")
        .then(|| frame["params"]["item"]["text"].as_str())
        .flatten()
        .filter(|text| !text.trim().is_empty())
}

/// Codex's own plan approval submits this turn in Default mode. It can arrive
/// through the TUI instead of the host's injected acceptance request, so keep
/// the mode Codex selected and advance the worker to implementation.
pub fn codex_implementation_request(frame: &Value) -> bool {
    frame["method"] == "turn/start"
        && frame["params"]["collaborationMode"]["mode"] == "default"
        && frame["params"]["input"].as_array().is_some_and(|input| {
            input.len() == 1
                && input[0]["type"] == "text"
                && input[0]["text"] == "Implement the plan."
        })
}

/// A choice on a native plan-exit dialog. The caller must first correlate the dialog
/// with ExitPlanMode; a source file containing these words is never sufficient evidence.
pub fn terminal_choice(prompt: &cide_ipc::remote::PermissionPrompt, mode: &str) -> Option<u8> {
    let target = prompt.options.iter().filter(|o| {
        o.label.to_lowercase().starts_with("yes")
            && !o.label.to_lowercase().contains("clear context")
    });
    let matches = |label: &str| {
        let label = label.to_lowercase();
        match mode {
            "bypassPermissions" => {
                label.contains("bypass permissions") || label.contains("previous mode (yolo)")
            }
            "auto" => label.contains("use auto mode") || label.contains("previous mode (auto)"),
            "acceptEdits" => {
                label.contains("auto-accept edits") || label.contains("previous mode (auto-edit)")
            }
            "manual" | "default" => {
                label.contains("manually approve edits")
                    || label.contains("previous mode (default)")
            }
            "dontAsk" => label.contains("previous mode (dontask)"),
            _ => false,
        }
    };
    target
        .filter(|o| matches(&o.label))
        .map(|o| o.number)
        .next()
}

pub fn terminal_prompt(
    screen: &cide_ipc::screen::ScreenCapture,
    harness: cide_ipc::Harness,
) -> Option<cide_ipc::remote::PermissionPrompt> {
    let mut screen = screen.clone();
    if harness == cide_ipc::Harness::Qwen {
        // Qwen's native RadioButtonSelect uses U+203A rather than Claude's U+276F.
        for line in &mut screen.lines {
            for run in &mut line.runs {
                run.text = run.text.replace('›', "❯");
            }
        }
    }
    // The plan itself commonly contains numbered steps. Only the final affirmative
    // choice block belongs to the native confirmation; the caller has already correlated
    // it with ExitPlanMode. Keep the general permission parser's ambiguity refusal intact.
    let first = screen.lines.iter().rev().find_map(|line| {
        let text: String = line.runs.iter().map(|run| run.text.as_str()).collect();
        let label = text
            .trim_start_matches([' ', '│', '┃', '|'])
            .trim_start_matches(['❯', '>'])
            .trim_start();
        label
            .strip_prefix("1.")
            .is_some_and(|label| label.trim_start().starts_with("Yes"))
            .then_some(line.row)
    })?;
    screen.lines.retain(|line| line.row >= first);
    cide_claude::permission::parse(&screen, cide_ipc::SessionState::AwaitingPermission)
}

pub fn qwen_plan_result(value: &Value, call: &str) -> Option<bool> {
    if value["type"] != "user" || !value["parent_tool_use_id"].is_null() {
        return None;
    }
    value["message"]["content"]
        .as_array()?
        .iter()
        .find(|block| block["type"] == "tool_result" && block["tool_use_id"] == call)
        .and_then(|block| block["is_error"].as_bool())
        .map(|error| !error)
}

/// Navigation, rather than digit+Enter: a digit may already accept a dialog on some
/// releases, in which case the extra Enter would act on the following screen.
pub fn choice_keys(from: u8, to: u8, app_cursor: bool) -> Vec<Vec<u8>> {
    let (up, down): (&[u8], &[u8]) = if app_cursor {
        (b"\x1bOA", b"\x1bOB")
    } else {
        (b"\x1b[A", b"\x1b[B")
    };
    let key = if to >= from { down } else { up };
    let mut keys = vec![key.to_vec(); usize::from(from.abs_diff(to))];
    keys.push(vec![b'\r']);
    keys
}

/// Tracks one native Codex planning turn. Helpers, checklists and interrupted turns
/// cannot authorize the implementation continuation.
#[derive(Default)]
pub struct Codex {
    thread: String,
    turn: Option<String>,
    proposal: bool,
    model: String,
    effort: Option<String>,
}
impl Codex {
    pub fn prepare(&mut self, frame: &mut Value, model: &str) -> bool {
        if frame["method"] != "turn/start" {
            return false;
        }
        self.thread = frame["params"]["threadId"]
            .as_str()
            .unwrap_or_default()
            .into();
        self.model = frame["params"]["collaborationMode"]["settings"]["model"]
            .as_str()
            .or_else(|| frame["params"]["model"].as_str())
            .unwrap_or(model)
            .into();
        self.effort = frame["params"]["collaborationMode"]["settings"]["reasoning_effort"]
            .as_str()
            .or_else(|| frame["params"]["effort"].as_str())
            .map(str::to_string);
        self.turn = None;
        self.proposal = false;
        frame["params"]["collaborationMode"] =
            codex_mode(&self.model, self.effort.as_deref(), Phase::Planning);
        true
    }
    pub fn observe(&mut self, frame: &Value, id: &str) -> Option<Value> {
        if frame["params"]["threadId"].as_str() != Some(self.thread.as_str()) {
            return None;
        }
        match frame["method"].as_str()? {
            "turn/started" => {
                self.turn = frame["params"]["turn"]["id"].as_str().map(str::to_string);
                self.proposal = false;
            }
            "item/completed"
                if frame["params"]["turnId"].as_str() == self.turn.as_deref()
                    && self.turn.is_some() =>
            {
                self.proposal |= codex_proposal(frame).is_some();
            }
            "turn/completed"
                if self.turn.is_some()
                    && frame["params"]["turn"]["id"].as_str() == self.turn.as_deref() =>
            {
                let approved = self.proposal && frame["params"]["turn"]["status"] == "completed";
                self.turn = None;
                self.proposal = false;
                if approved {
                    // This is the native TUI's acceptance turn, not a planning instruction.
                    return Some(json!({"id":id,"method":"turn/start","params":{
                        "threadId":self.thread,"input":[{"type":"text","text":"Implement the plan."}],
                        "collaborationMode":codex_mode(&self.model,self.effort.as_deref(),Phase::Implementing)
                    }}));
                }
            }
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_modes_preserve_the_model_and_use_builtin_instructions() {
        for phase in [Phase::Planning, Phase::Approving, Phase::Implementing] {
            let mode = codex_mode("chosen", Some("high"), phase);
            assert_eq!(mode["settings"]["model"], "chosen");
            assert_eq!(mode["settings"]["reasoning_effort"], "high");
            assert!(mode["settings"]["developer_instructions"].is_null());
            assert_eq!(
                mode["mode"],
                if phase == Phase::Implementing {
                    "default"
                } else {
                    "plan"
                }
            );
        }
    }

    #[test]
    fn only_a_completed_native_proposal_can_be_approved() {
        assert!(
            codex_proposal(&json!({"method":"turn/plan/updated","params":{"plan":[]}})).is_none()
        );
        assert!(codex_proposal(&json!({"method":"item/completed","params":{"item":{"type":"agentMessage","text":"A plan"}}})).is_none());
        assert!(
            codex_proposal(
                &json!({"method":"item/completed","params":{"item":{"type":"plan","text":" "}}})
            )
            .is_none()
        );
        assert_eq!(
            codex_proposal(
                &json!({"method":"item/completed","params":{"item":{"type":"plan","text":"Change it"}}})
            ),
            Some("Change it")
        );
    }

    #[test]
    fn codex_native_approval_is_an_implementation_turn() {
        let request = json!({"method":"turn/start","params":{
            "threadId":"main",
            "collaborationMode":{"mode":"default"},
            "input":[{"type":"text","text":"Implement the plan."}]
        }});
        assert!(codex_implementation_request(&request));
        let mut wrong = request.clone();
        wrong["params"]["collaborationMode"]["mode"] = json!("plan");
        assert!(!codex_implementation_request(&wrong));
        wrong = request.clone();
        wrong["params"]["input"][0]["text"] = json!("Implement something else.");
        assert!(!codex_implementation_request(&wrong));
    }

    #[test]
    fn planning_state_round_trips_without_restarting_implementation() {
        let mut state = State::default();
        assert!(state.pending());
        state.phase = Phase::Implementing;
        let restored: State =
            serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        assert_eq!(restored, state);
        assert!(!restored.pending());
        assert_ne!(State::default().generation, state.generation);
    }

    #[test]
    fn native_dialog_choices_do_not_pick_an_unrelated_yes() {
        use cide_ipc::remote::{PermissionPrompt, PromptOption};
        let prompt = |labels: &[&str]| PermissionPrompt {
            question: vec![],
            options: labels
                .iter()
                .enumerate()
                .map(|(i, label)| PromptOption {
                    number: (i + 1) as u8,
                    label: label.to_string(),
                })
                .collect(),
            selected: Some(1),
            digest: String::new(),
        };
        assert_eq!(
            terminal_choice(
                &prompt(&[
                    "Yes, manually approve edits",
                    "Yes, and use auto mode",
                    "No, keep planning"
                ]),
                "auto"
            ),
            Some(2)
        );
        assert_eq!(
            terminal_choice(&prompt(&["Yes, and don't ask again", "No"]), "auto"),
            None
        );
        assert_eq!(
            terminal_choice(&prompt(&["Yes, and auto-accept edits", "No"]), "auto"),
            None,
            "approving a plan must preserve the resolved execution permissions"
        );
        assert_eq!(
            terminal_choice(
                &prompt(&[
                    "Yes, restore previous mode (auto)",
                    "Yes, and auto-accept edits"
                ]),
                "auto"
            ),
            Some(1)
        );
        assert_eq!(
            choice_keys(2, 1, false),
            vec![b"\x1b[A".to_vec(), vec![b'\r']]
        );
    }

    #[test]
    fn codex_accepts_only_the_current_successful_native_plan_once() {
        let mut adapter = Codex::default();
        let mut request = json!({"id":1,"method":"turn/start","params":{"threadId":"main","input":[{"type":"text","text":"Fix it"}]}});
        adapter.prepare(&mut request, "chosen");
        assert_eq!(request["params"]["input"][0]["text"], "Fix it");
        assert_eq!(request["params"]["collaborationMode"]["mode"], "plan");
        adapter.observe(
            &json!({"method":"turn/started","params":{"threadId":"main","turn":{"id":"t"}}}),
            "approve",
        );
        assert!(adapter.observe(&json!({"method":"item/completed","params":{"threadId":"other","turnId":"t","item":{"type":"plan","text":"bad"}}}), "approve").is_none());
        adapter.observe(&json!({"method":"item/completed","params":{"threadId":"main","turnId":"t","item":{"type":"plan","text":"Fix it"}}}), "approve");
        let done = json!({"method":"turn/completed","params":{"threadId":"main","turn":{"id":"t","status":"completed"}}});
        let approval = adapter.observe(&done, "approve").unwrap();
        assert_eq!(approval["params"]["collaborationMode"]["mode"], "default");
        assert!(adapter.observe(&done, "again").is_none());
    }

    #[test]
    fn qwen_native_selection_marker_is_recognized_without_changing_the_claude_parser() {
        let screen: cide_ipc::screen::ScreenCapture = serde_json::from_value(json!({
            "info":{"cols":100,"rows":10,"alt":false,"appCursor":false,"bracketedPaste":false,"mouse":"off"},
            "lines":[
                {"row":1,"runs":[{"text":"1. Inspect the current implementation"}]},
                {"row":2,"runs":[{"text":"2. Change the implementation and verify it"}]},
                {"row":6,"runs":[{"text":"› 1. Yes, restore previous mode (auto)"}]},
                {"row":7,"runs":[{"text":"  2. Yes, and auto-accept edits"}]},
                {"row":8,"runs":[{"text":"  3. No, keep planning (esc)"}]}
            ]
        })).unwrap();
        assert!(terminal_prompt(&screen, cide_ipc::Harness::Claude).is_none());
        let prompt = terminal_prompt(&screen, cide_ipc::Harness::Qwen).unwrap();
        assert_eq!(prompt.selected, Some(1));
        assert_eq!(terminal_choice(&prompt, "auto"), Some(1));
    }

    #[test]
    fn qwen_permission_acceptance_is_not_the_plan_execution_acknowledgement() {
        assert_eq!(
            qwen_plan_result(
                &json!({"type":"control_response","response":{"response":{"allowed":true}}}),
                "exit"
            ),
            None
        );
        let result = |id, error| json!({"type":"user","parent_tool_use_id":null,"message":{"content":[{"type":"tool_result","tool_use_id":id,"is_error":error}]}});
        assert_eq!(qwen_plan_result(&result("other", false), "exit"), None);
        assert_eq!(qwen_plan_result(&result("exit", true), "exit"), Some(false));
        assert_eq!(qwen_plan_result(&result("exit", false), "exit"), Some(true));
    }
}
