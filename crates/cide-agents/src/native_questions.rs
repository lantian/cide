//! Lossless question/answer adapters. Transport and ownership stay in the host.
use cide_ipc::{
    QuestionChoices, QuestionOption, QuestionSelection, TaskQuestion, UserRequestQuestion,
    UserRequestResponse,
};
use serde_json::{Value, json};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    Codex,
    Claude,
    Qwen,
    OpenCode,
    Forms,
}

/// JSON-RPC ids are connection-local. The native turn/item identify the tool
/// across reconnects; old protocols get a fresh connection scope instead.
pub fn codex_key(frame: &Value, scope: &str) -> String {
    match (
        frame["params"]["turnId"].as_str(),
        frame["params"]["itemId"].as_str(),
    ) {
        (Some(turn), Some(item)) => json!([turn, item]).to_string(),
        _ => format!("{scope}:{}", frame["id"]),
    }
}

pub fn questions(input: &Value, dialect: Dialect) -> Result<Vec<UserRequestQuestion>, String> {
    let rows = input
        .as_array()
        .ok_or("The harness did not provide a question list")?;
    if rows.is_empty() {
        return Err("The harness provided no questions".into());
    }
    let mut ids = HashSet::new();
    rows.iter()
        .enumerate()
        .map(|(i, row)| {
            let id = row[if dialect == Dialect::Forms {
                "key"
            } else {
                "id"
            }]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| i.to_string());
            if !ids.insert(id.clone()) {
                return Err("Duplicate native question id".into());
            }
            let header = row["header"]
                .as_str()
                .or_else(|| row["title"].as_str())
                .unwrap_or("Question")
                .to_string();
            let text = row[if dialect == Dialect::Forms {
                "description"
            } else {
                "question"
            }]
            .as_str()
            .or_else(|| row["title"].as_str())
            .or_else(|| row["description"].as_str())
            .unwrap_or(&header)
            .to_string();
            let mut options = Vec::new();
            for (index, option) in row["options"].as_array().into_iter().flatten().enumerate() {
                let label = option["label"]
                    .as_str()
                    .ok_or("A native choice has no label")?;
                options.push(QuestionOption {
                    id: if dialect == Dialect::Forms {
                        option["value"]
                            .as_str()
                            .ok_or("A form choice has no value")?
                            .into()
                    } else {
                        index.to_string()
                    },
                    title: label.into(),
                    description: option["description"].as_str().map(str::to_string),
                    image: None,
                });
            }
            if dialect == Dialect::Forms && row["type"] == "boolean" {
                options = [true, false]
                    .into_iter()
                    .map(|v| QuestionOption {
                        id: v.to_string(),
                        title: if v { "Yes" } else { "No" }.into(),
                        description: None,
                        image: None,
                    })
                    .collect();
            }
            if dialect == Dialect::Forms
                && (!row["when"].as_array().is_none_or(Vec::is_empty)
                    || row["hidden"] == true
                    || !matches!(
                        row["type"].as_str(),
                        Some("string" | "multiselect" | "boolean" | "number" | "integer")
                    ))
            {
                return Err("This native form needs its original interface".into());
            }
            let multiple = row["multiSelect"] == true
                || row["multiple"] == true
                || row["type"] == "multiselect";
            let custom = row["custom"].as_bool().unwrap_or(true) && row["type"] != "boolean";
            let question = TaskQuestion::Choices(QuestionChoices {
                text,
                selection: if multiple {
                    QuestionSelection::Multiple
                } else {
                    QuestionSelection::Single
                },
                options,
            });
            question.validate()?;
            Ok(UserRequestQuestion {
                id,
                header,
                question,
                custom,
            })
        })
        .collect()
}

/// Preserve exact labels (and form values); the display-only option ids never reach a harness.
pub fn answer_values(
    questions: &[UserRequestQuestion],
    response: &UserRequestResponse,
) -> Option<Vec<Vec<String>>> {
    let UserRequestResponse::Answers { answers } = response else {
        return None;
    };
    questions
        .iter()
        .map(|question| {
            let answer = answers.iter().find(|answer| answer.id == question.id)?;
            let mut values = answer
                .selected_ids
                .iter()
                .map(|id| {
                    question
                        .question
                        .options()
                        .iter()
                        .find(|option| &option.id == id)
                        .map(|option| option.title.clone())
                })
                .collect::<Option<Vec<_>>>()?;
            if !answer.text.trim().is_empty() {
                values.push(answer.text.clone());
            }
            Some(values)
        })
        .collect()
}

pub fn codex_reply(
    id: &Value,
    questions: &[UserRequestQuestion],
    response: &UserRequestResponse,
) -> Option<Value> {
    let values = answer_values(questions, response)?;
    let answers: serde_json::Map<_, _> = questions
        .iter()
        .zip(values)
        .map(|(q, values)| (q.id.clone(), json!({"answers":values})))
        .collect();
    Some(json!({"id":id,"result":{"answers":answers}}))
}

pub fn claude_reply(
    input: &Value,
    questions: &[UserRequestQuestion],
    response: &UserRequestResponse,
) -> Value {
    let Some(values) = answer_values(questions, response) else {
        return json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"The user cancelled this question. Do not assume an answer."}});
    };
    let mut updated = input.clone();
    updated["answers"] = Value::Object(
        questions
            .iter()
            .zip(values)
            .map(|(q, values)| (q.question.text().into(), json!(values.join(", "))))
            .collect(),
    );
    json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":updated}})
}

pub fn form_reply(
    fields: &Value,
    questions: &[UserRequestQuestion],
    response: &UserRequestResponse,
) -> Result<Value, String> {
    let UserRequestResponse::Answers { answers } = response else {
        return Err("Question cancelled".into());
    };
    let mut result = serde_json::Map::new();
    for (q, field) in questions
        .iter()
        .zip(fields.as_array().ok_or("Missing form fields")?)
    {
        let answer = answers
            .iter()
            .find(|answer| answer.id == q.id)
            .ok_or("Missing form answer")?;
        let mut values = answer.selected_ids.clone();
        if !answer.text.trim().is_empty() {
            values.push(answer.text.clone());
        }
        let first = values.first().ok_or("Empty form answer")?;
        let value = match field["type"].as_str() {
            Some("boolean") => json!(first.parse::<bool>().map_err(|_| "Expected Yes or No")?),
            Some("integer") => json!(
                first
                    .parse::<i64>()
                    .map_err(|_| "Expected a whole number")?
            ),
            Some("number") => {
                let value = first.parse::<f64>().map_err(|_| "Expected a number")?;
                if !value.is_finite() {
                    return Err("Expected a finite number".into());
                }
                json!(value)
            }
            Some("multiselect") => json!(values),
            _ => json!(values.join(", ")),
        };
        result.insert(q.id.clone(), value);
    }
    Ok(json!({"answer":result}))
}

#[derive(Debug, PartialEq, Eq)]
pub struct QwenDialog {
    pub question: usize,
    pub selected: usize,
    pub empty_custom: bool,
}
fn compact(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Called only after a correlated ask_user_question control request. Arbitrary
/// numbered output, a composer, and permission/plan-exit menus are never dialogs.
pub fn qwen_dialog(
    screen: &cide_ipc::screen::ScreenCapture,
    questions: &[UserRequestQuestion],
) -> Option<QwenDialog> {
    let lines: Vec<String> = screen
        .lines
        .iter()
        .map(|line| {
            line.runs
                .iter()
                .map(|r| r.text.as_str())
                .collect::<String>()
        })
        .collect();
    let body = compact(&lines.join("\n"));
    if !body.contains("Navigate") || !body.contains("Enter:") {
        return None;
    }
    let (selected_row, selected) = lines.iter().enumerate().rev().find_map(|(row, line)| {
        let rest = line
            .trim_start_matches([' ', '│', '┃', '|'])
            .strip_prefix('❯')
            .or_else(|| {
                line.trim_start_matches([' ', '│', '┃', '|'])
                    .strip_prefix('›')
            })?
            .trim();
        let rest = rest
            .strip_prefix("[ ]")
            .or_else(|| rest.strip_prefix("[✓]"))
            .unwrap_or(rest)
            .trim();
        let (number, label) = rest.split_once('.')?;
        Some((
            row,
            (number.parse::<usize>().ok()?.checked_sub(1)?, label.trim()),
        ))
    })?;
    if body.contains("▸ Submit")
        && body.contains("Ready to submit your answers?")
        && selected.0 <= 1
    {
        return Some(QwenDialog {
            question: questions.len(),
            selected: selected.0,
            empty_custom: false,
        });
    }
    let prefix = compact(&lines[..=selected_row].join("\n"));
    let mut matches: Vec<_> = questions
        .iter()
        .enumerate()
        .filter_map(|(index, q)| {
            prefix
                .rfind(&compact(q.question.text()))
                .map(|position| (position, index))
        })
        .collect();
    matches.sort_unstable();
    let (position, index) = *matches.last()?;
    if matches.iter().filter(|(p, _)| *p == position).count() != 1 {
        return None;
    }
    let count = questions[index].question.options().len();
    if selected.0 > count {
        return None;
    }
    // Every predefined choice must be visible: a clipped or changed dialog costs
    // automatic delivery, never permission to guess its layout.
    if questions[index]
        .question
        .options()
        .iter()
        .any(|option| !body.contains(&compact(&option.title)))
    {
        return None;
    }
    Some(QwenDialog {
        question: index,
        selected: selected.0,
        empty_custom: selected.0 == count
            && (selected.1.is_empty() || selected.1 == "Type something..."),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::UserRequestAnswer;
    #[test]
    fn a_batch_preserves_ids_labels_unicode_and_custom_answers() {
        let qs = questions(&json!([
            {"id":"language","header":"Language","question":"Which?","options":[{"label":"Русский","description":"Native"}]},
            {"id":"name","question":"Name?"}
        ]), Dialect::Codex).unwrap();
        let response = UserRequestResponse::Answers {
            answers: vec![
                UserRequestAnswer {
                    id: "name".into(),
                    text: "Line 1\nLine 2".into(),
                    selected_ids: vec![],
                },
                UserRequestAnswer {
                    id: "language".into(),
                    text: String::new(),
                    selected_ids: vec!["0".into()],
                },
            ],
        };
        assert_eq!(
            codex_reply(&json!(42), &qs, &response).unwrap(),
            json!({"id":42,"result":{"answers":{"language":{"answers":["Русский"]},"name":{"answers":["Line 1\nLine 2"]}}}})
        );
        let input = json!({"questions":[{"question":"original"}],"metadata":{"source":"test"}});
        let reply = claude_reply(&input, &qs, &response);
        assert_eq!(
            reply["hookSpecificOutput"]["updatedInput"]["questions"],
            input["questions"]
        );
        assert_eq!(
            reply["hookSpecificOutput"]["updatedInput"]["answers"]["Which?"],
            "Русский"
        );
        assert!(codex_reply(&json!(42), &qs, &UserRequestResponse::Cancel).is_none());
        assert_eq!(
            claude_reply(&input, &qs, &UserRequestResponse::Cancel)["hookSpecificOutput"]["permissionDecision"],
            "deny"
        );
    }
    #[test]
    fn codex_question_identity_follows_the_native_call_instead_of_reused_rpc_ids() {
        let mut frame = json!({"id":1,"params":{"turnId":"t1","itemId":"ask"}});
        let first = codex_key(&frame, "socket1");
        frame["id"] = json!(9);
        assert_eq!(codex_key(&frame, "socket2"), first);
        frame["params"]["turnId"] = json!("t2");
        assert_ne!(codex_key(&frame, "socket2"), first);
        assert_ne!(
            codex_key(&json!({"id":1}), "socket1"),
            codex_key(&json!({"id":1}), "socket2")
        );
    }
    #[test]
    fn form_values_are_not_replaced_by_display_labels() {
        let fields = json!([{ "key":"size", "title":"Size", "type":"string", "custom":false, "options":[{"label":"Large", "value":"l"}] }]);
        let qs = questions(&fields, Dialect::Forms).unwrap();
        assert!(!qs[0].custom);
        let response = UserRequestResponse::Answers {
            answers: vec![UserRequestAnswer {
                id: "size".into(),
                text: String::new(),
                selected_ids: vec!["l".into()],
            }],
        };
        assert_eq!(
            form_reply(&fields, &qs, &response).unwrap(),
            json!({"answer":{"size":"l"}})
        );
        assert!(questions(&json!([{"id":"same"},{"id":"same"}]), Dialect::Codex).is_err());
    }
    fn screen(lines: &[&str]) -> cide_ipc::screen::ScreenCapture {
        serde_json::from_value(json!({"info":{"cols":100,"rows":30,"alt":false,"appCursor":false,"bracketedPaste":true,"mouse":"off"},
            "lines":lines.iter().enumerate().map(|(row,text)|json!({"row":row,"runs":[{"text":text}]})).collect::<Vec<_>>() })).unwrap()
    }
    #[test]
    fn qwen_navigation_requires_the_correlated_question_and_native_controls() {
        let qs = questions(&json!([{ "header":"Language", "question":"Which language?", "options":[{"label":"Русский"},{"label":"English"}]}]), Dialect::Qwen).unwrap();
        assert_eq!(
            qwen_dialog(
                &screen(&[
                    "Language",
                    "Which language?",
                    "❯ 1. Русский",
                    "  2. English",
                    "  3. Type something...",
                    "↑/↓: Navigate | Enter: Select | Esc: Cancel"
                ]),
                &qs
            ),
            Some(QwenDialog {
                question: 0,
                selected: 0,
                empty_custom: false
            })
        );
        assert_eq!(
            qwen_dialog(
                &screen(&[
                    "Which language?",
                    "  1. Русский",
                    "  2. English",
                    "❯ 3. Type something...",
                    "↑/↓: Navigate | Enter: Select | Esc: Cancel"
                ]),
                &qs
            ),
            Some(QwenDialog {
                question: 0,
                selected: 2,
                empty_custom: true
            })
        );
        for lines in [
            vec!["Which language?", "❯ 1. Русский", "2. English"],
            vec![
                "Another question?",
                "❯ 1. Русский",
                "2. English",
                "Navigate | Enter: Select",
            ],
            vec![
                "Which language?",
                "❯ 1. Changed choice",
                "Navigate | Enter: Select",
            ],
        ] {
            assert!(qwen_dialog(&screen(&lines), &qs).is_none());
        }
        assert_eq!(
            qwen_dialog(
                &screen(&[
                    "▸ Submit",
                    "Your answers:",
                    "Language: Русский",
                    "Ready to submit your answers?",
                    "❯ 1. Submit answers",
                    "  2. Cancel",
                    "Navigate | Enter: Select"
                ]),
                &qs
            )
            .unwrap()
            .question,
            1
        );
    }
}
