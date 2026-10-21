//! OpenCode's console API. v1 and v2 have different routes and response envelopes.
use super::opencode::Generation;
use cide_ipc::SessionState;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader},
    path::Path,
    time::Duration,
};

#[derive(Clone)]
pub struct Api {
    client: reqwest::blocking::Client,
    url: String,
    password: String,
    user: String,
    pub generation: Generation,
}

impl Api {
    /// Only model-authored questions. Authentication/provider forms and native plan
    /// exits retain their own controllers.
    pub fn pending_questions(&self, id: &str) -> Result<Vec<Value>, String> {
        validate_id(id)?;
        let route = if self.generation == Generation::V2 {
            format!("/session/{id}/form")
        } else {
            "/question".into()
        };
        let requests = self.request(reqwest::Method::GET, &route, None)?;
        let mut result = Vec::new();
        for request in requests
            .get("items")
            .unwrap_or(&requests)
            .as_array()
            .into_iter()
            .flatten()
        {
            if request["sessionID"] != id {
                continue;
            }
            if self.generation == Generation::V2 {
                if request["metadata"]["kind"] == "question" {
                    result.push(request.clone());
                }
                continue;
            }
            let (Some(message), Some(call)) = (
                request["tool"]["messageID"].as_str(),
                request["tool"]["callID"].as_str(),
            ) else {
                continue;
            };
            if !message.starts_with("msg_")
                || !message
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_')
            {
                continue;
            }
            let content = self.request(
                reqwest::Method::GET,
                &format!("/session/{id}/message/{message}"),
                None,
            )?;
            if content["parts"].as_array().is_some_and(|parts| {
                parts
                    .iter()
                    .any(|p| p["type"] == "tool" && p["tool"] == "question" && p["callID"] == call)
            }) {
                result.push(request.clone());
            }
        }
        Ok(result)
    }
    pub fn reply_question(
        &self,
        session: &str,
        id: &str,
        answer: Option<&Value>,
    ) -> Result<(), String> {
        validate_id(session)?;
        if id.is_empty() || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
            return Err("Invalid native question id".into());
        }
        let (method, route) = if self.generation == Generation::V2 {
            (
                if answer.is_some() {
                    reqwest::Method::POST
                } else {
                    reqwest::Method::DELETE
                },
                format!(
                    "/session/{session}/form/{id}{}",
                    if answer.is_some() { "/reply" } else { "" }
                ),
            )
        } else {
            (
                reqwest::Method::POST,
                format!(
                    "/question/{id}/{}",
                    if answer.is_some() { "reply" } else { "reject" }
                ),
            )
        };
        self.request(method, &route, answer).map(|_| ())
    }
    pub fn new(url: String, password: String, generation: Generation) -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            client,
            url,
            password,
            user: "opencode".into(),
            generation,
        })
    }
    pub fn with_user(mut self, user: String) -> Self {
        self.user = user;
        self
    }
    fn path(&self, path: &str) -> String {
        format!(
            "{}{}{path}",
            self.url,
            if self.generation == Generation::V2 {
                "/api"
            } else {
                ""
            }
        )
    }
    pub fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, String> {
        let mut request = self
            .client
            .request(method, self.path(path))
            .basic_auth(&self.user, Some(&self.password))
            .timeout(Duration::from_secs(15));
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .map_err(|e| format!("OpenCode connection failed: {e}"))?;
        let status = response.status();
        let text = response.text().map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!(
                "OpenCode {path}: HTTP {status}: {}",
                text.chars().take(500).collect::<String>()
            ));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        let value: Value =
            serde_json::from_str(&text).map_err(|e| format!("OpenCode {path}: {e}"))?;
        Ok(if self.generation == Generation::V2 {
            value.get("data").cloned().unwrap_or(value)
        } else {
            value
        })
    }
    pub fn healthy(&self) -> bool {
        let guarded = if self.generation == Generation::V2 {
            "/openapi.json"
        } else {
            "/session"
        };
        let url = format!("{}{guarded}", self.url);
        let request = || self.client.get(&url).timeout(Duration::from_secs(2));
        request()
            .basic_auth(&self.user, Some(&self.password))
            .send()
            .is_ok_and(|r| r.status().is_success())
            && request()
                .send()
                .is_ok_and(|r| r.status() == reqwest::StatusCode::UNAUTHORIZED)
    }
    pub fn create(
        &self,
        cwd: &Path,
        title: Option<&str>,
        model: Option<&str>,
        agent: Option<&str>,
        unattended: bool,
    ) -> Result<String, String> {
        let mut body = json!({});
        if let Some(title) = title {
            body["title"] = json!(title);
        }
        if let Some(agent) = agent {
            body["agent"] = json!(agent);
        }
        if self.generation == Generation::V2 {
            body["location"] = json!({ "directory": cwd });
            if let Some(model) = model.and_then(model_ref) {
                body["model"] = model;
            }
            if unattended {
                body["permissions"] =
                    json!([{ "action": "*", "resource": "*", "effect": "allow" }]);
            }
        }
        let session = self.request(reqwest::Method::POST, "/session", Some(&body))?;
        session
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "OpenCode did not return a conversation id".into())
    }
    pub fn exists(&self, id: &str) -> Result<(), String> {
        validate_id(id)?;
        self.request(reqwest::Method::GET, &format!("/session/{id}"), None)
            .map(|_| ())
    }
    pub fn require_native_plan(&self) -> Result<(), String> {
        let agents = self.request(reqwest::Method::GET, "/agent", None)?;
        let agents = agents.get("items").unwrap_or(&agents);
        if agents.as_array().is_some_and(|agents| {
            agents
                .iter()
                .any(|agent| agent["id"] == "plan" || agent["name"] == "plan")
        }) {
            Ok(())
        } else {
            Err("This OpenCode installation exposes no native Plan agent. Native planning support is required for delegated workers.".into())
        }
    }
    pub fn select_agent(&self, id: &str, agent: &str) -> Result<(), String> {
        validate_id(id)?;
        if self.generation == Generation::V2 {
            self.request(
                reqwest::Method::POST,
                &format!("/session/{id}/agent"),
                Some(&json!({"agent":agent})),
            )?;
        }
        Ok(())
    }
    /// Only a native plan_exit tool's question can be answered automatically.
    pub fn approve_plan_question(&self, id: &str) -> Result<bool, String> {
        validate_id(id)?;
        if self.generation != Generation::V1 {
            return Ok(false);
        }
        let requests = self.request(reqwest::Method::GET, "/question", None)?;
        for request in requests
            .as_array()
            .into_iter()
            .flatten()
            .filter(|q| q["sessionID"] == id)
        {
            let Some(call) = request["tool"]["callID"].as_str() else {
                continue;
            };
            let Some(message) = request["tool"]["messageID"].as_str() else {
                continue;
            };
            if !message.starts_with("msg_")
                || !message
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                continue;
            }
            let content = self.request(
                reqwest::Method::GET,
                &format!("/session/{id}/message/{message}"),
                None,
            )?;
            let native = content["parts"].as_array().is_some_and(|parts| {
                parts.iter().any(|part| {
                    part["type"] == "tool" && part["tool"] == "plan_exit" && part["callID"] == call
                })
            });
            let Some(request_id) = request["id"]
                .as_str()
                .filter(|id| id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
            else {
                continue;
            };
            if native
                && request["questions"].as_array().is_some_and(|qs| {
                    qs.len() == 1
                        && qs[0]["options"].as_array().is_some_and(|options| {
                            options.iter().any(|option| option["label"] == "Yes")
                        })
                })
            {
                self.request(
                    reqwest::Method::POST,
                    &format!("/question/{request_id}/reply"),
                    Some(&json!({"answers":[["Yes"]]})),
                )?;
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub fn completed_native_plan(&self, id: &str) -> Result<Option<String>, String> {
        validate_id(id)?;
        let route = if self.generation == Generation::V2 {
            format!("/session/{id}/message?order=desc&limit=16")
        } else {
            format!("/session/{id}/message")
        };
        let messages = self.request(reqwest::Method::GET, &route, None)?;
        let messages = messages.get("items").unwrap_or(&messages);
        let Some(messages) = messages.as_array() else {
            return Ok(None);
        };
        // A newer user turn invalidates the previous proposal, even before its first
        // assistant message arrives. Never approve a plan from an earlier assignment.
        let latest: Box<dyn Iterator<Item = &Value>> = if self.generation == Generation::V2 {
            Box::new(messages.iter())
        } else {
            Box::new(messages.iter().rev())
        };
        let Some(last) = latest.into_iter().find(|message| {
            let info = message.get("info").unwrap_or(message);
            matches!(
                info["type"].as_str().or_else(|| info["role"].as_str()),
                Some("assistant" | "user")
            )
        }) else {
            return Ok(None);
        };
        let info = last.get("info").unwrap_or(last);
        let parts = last.get("parts").or_else(|| info.get("content"));
        let complete = (info["type"] == "assistant" || info["role"] == "assistant")
            && info["agent"] == "plan"
            && info["finish"] == "stop"
            && info.get("error").is_none_or(Value::is_null)
            && parts.and_then(Value::as_array).is_some_and(|parts| {
                parts.iter().any(|part| {
                    part["type"] == "text"
                        && part["text"]
                            .as_str()
                            .is_some_and(|text| !text.trim().is_empty())
                })
            });
        Ok(complete
            .then(|| info["id"].as_str().map(str::to_string))
            .flatten())
    }
    pub fn fork(&self, id: &str) -> Result<String, String> {
        validate_id(id)?;
        self.request(
            reqwest::Method::POST,
            &format!("/session/{id}/fork"),
            Some(&json!({})),
        )?
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "OpenCode did not return the forked conversation id".into())
    }
    pub fn instructions(&self, id: &str, text: &str) -> Result<(), String> {
        validate_id(id)?;
        if self.generation == Generation::V2 {
            self.request(
                reqwest::Method::PUT,
                &format!("/experimental/session/{id}/instructions/entries/cide"),
                Some(&json!({ "value": text })),
            )
            .map(|_| ())
        } else {
            Ok(())
        } // v1 instructions are carried by the server's configuration document.
    }
    pub fn prompt(&self, id: &str, text: &str, model: Option<&str>) -> Result<(), String> {
        self.prompt_as(id, text, model, None)
    }
    /// A v1 native prompt must retain the console's agent and selected model.
    pub fn prompt_as(
        &self,
        id: &str,
        text: &str,
        model: Option<&str>,
        agent: Option<&str>,
    ) -> Result<(), String> {
        validate_id(id)?;
        let (path, mut body) = if self.generation == Generation::V2 {
            (
                format!("/session/{id}/prompt"),
                json!({ "text": text, "resume": true }),
            )
        } else {
            (
                format!("/session/{id}/prompt_async"),
                json!({ "parts": [{ "type": "text", "text": text }] }),
            )
        };
        if let Some(agent) = agent {
            if self.generation == Generation::V2 {
                self.select_agent(id, agent)?;
            } else {
                body["agent"] = json!(agent);
            }
        }
        if self.generation == Generation::V1
            && let Some(model) = model.and_then(model_ref)
        {
            body["model"] = json!({ "providerID": model["providerID"], "modelID": model["id"] });
        }
        self.request(reqwest::Method::POST, &path, Some(&body))
            .map(|_| ())
    }
    pub fn state(&self, id: &str, worked: bool) -> Result<SessionState, String> {
        validate_id(id)?;
        if self.generation == Generation::V1 {
            let permissions = self.request(reqwest::Method::GET, "/permission", None)?;
            if permissions
                .as_array()
                .is_some_and(|p| p.iter().any(|p| p["sessionID"].as_str() == Some(id)))
            {
                return Ok(SessionState::AwaitingPermission);
            }
            let questions = self.request(reqwest::Method::GET, "/question", None)?;
            if questions
                .as_array()
                .is_some_and(|p| p.iter().any(|p| p["sessionID"].as_str() == Some(id)))
            {
                return Ok(SessionState::AwaitingInput);
            }
            let statuses = self.request(reqwest::Method::GET, "/session/status", None)?;
            return Ok(
                match statuses
                    .get(id)
                    .and_then(|s| s.get("type"))
                    .and_then(Value::as_str)
                {
                    Some("busy" | "retry") => SessionState::Busy,
                    _ if worked => SessionState::AwaitingInput,
                    _ => SessionState::Idle,
                },
            );
        }
        let permissions = self.request(
            reqwest::Method::GET,
            &format!("/session/{id}/permission"),
            None,
        )?;
        if permissions.as_array().is_some_and(|a| !a.is_empty()) {
            return Ok(SessionState::AwaitingPermission);
        }
        let forms = self.request(reqwest::Method::GET, &format!("/session/{id}/form"), None)?;
        if forms.as_array().is_some_and(|a| !a.is_empty()) {
            return Ok(SessionState::AwaitingInput);
        }
        let active = self.request(reqwest::Method::GET, "/session/active", None)?;
        Ok(if active.get(id).is_some() {
            SessionState::Busy
        } else if worked {
            SessionState::AwaitingInput
        } else {
            SessionState::Idle
        })
    }
    pub fn events(&self, mut on_event: impl FnMut(Value) -> bool) -> Result<(), String> {
        let response = self
            .client
            .get(self.path("/event"))
            .basic_auth(&self.user, Some(&self.password))
            .timeout(Duration::from_secs(30))
            .send()
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?;
        let mut reader = BufReader::new(response);
        let mut line = String::new();
        let mut data = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                if let Some(event) = decode_event(data.trim()) {
                    on_event(event);
                }
                break;
            }
            if line.trim().is_empty() {
                if !data.is_empty() {
                    if let Some(event) = decode_event(data.trim())
                        && !on_event(event)
                    {
                        return Ok(());
                    }
                    data.clear();
                }
            } else if let Some(value) = line.strip_prefix("data:") {
                data.push_str(value.trim_start());
                data.push('\n');
                if data.len() > 4 * 1024 * 1024 {
                    return Err("OpenCode event exceeds the size limit".into());
                }
            }
        }
        Ok(())
    }
}

pub fn validate_id(id: &str) -> Result<(), String> {
    if !id.starts_with("ses") || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err("Invalid OpenCode conversation id".into());
    }
    Ok(())
}
pub fn model_ref(model: &str) -> Option<Value> {
    let (provider, model) = model.split_once('/')?;
    let (id, variant) = model
        .split_once('#')
        .map_or((model, None), |(id, variant)| (id, Some(variant)));
    let mut value = json!({ "providerID": provider, "id": id });
    if let Some(variant) = variant {
        value["variant"] = json!(variant);
    }
    Some(value)
}
pub fn decode_event(line: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(line).ok()?;
    match value {
        Value::String(s) => serde_json::from_str(&s).ok(),
        value => Some(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    fn plan_api(
        generation: Generation,
        steps: Vec<(&'static str, &'static str, Option<Value>, Value)>,
    ) -> (Api, std::thread::JoinHandle<()>) {
        plan_api_for(generation, steps, "opencode")
    }

    fn plan_api_for(
        generation: Generation,
        steps: Vec<(&'static str, &'static str, Option<Value>, Value)>,
        user: &'static str,
    ) -> (Api, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for (method, path, expected, response) in steps {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert_eq!(line.trim(), format!("{method} {path} HTTP/1.1"));
                let mut length = 0;
                let mut auth = false;
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    let header = line.to_ascii_lowercase();
                    if let Some(value) = header.strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                    let expected = match user {
                        "mimocode" => "authorization: basic bwltb2nvzgu6c2vjcmv0",
                        _ => "authorization: basic b3blbmnvzgu6c2vjcmv0",
                    };
                    auth |= header.trim() == expected;
                }
                assert!(auth);
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                if let Some(expected) = expected {
                    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), expected);
                }
                let body = response.to_string();
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        (
            Api::new(format!("http://{address}"), "secret".into(), generation)
                .unwrap()
                .with_user(user.into()),
            server,
        )
    }

    #[test]
    fn mimo_native_plan_uses_its_own_server_identity() {
        let (api, server) = plan_api_for(
            Generation::V1,
            vec![("GET", "/agent", None, json!([{"name":"plan"}]))],
            "mimocode",
        );
        api.require_native_plan().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn native_v2_questions_exclude_provider_forms_and_reply_with_form_values() {
        let (api, server) = plan_api(
            Generation::V2,
            vec![
                (
                    "GET",
                    "/api/session/ses_work/form",
                    None,
                    json!({"data":[
                        {"id":"frm_auth","sessionID":"ses_work","metadata":{"kind":"oauth"},"fields":[]},
                        {"id":"frm_question","sessionID":"ses_work","metadata":{"kind":"question","tool":{"id":"call_1"}},"fields":[{"key":"q0","type":"string","title":"Language","description":"Which language?","options":[{"value":"ru","label":"Русский"}],"custom":true}]},
                        {"id":"frm_other","sessionID":"ses_other","metadata":{"kind":"question"}}
                    ]}),
                ),
                (
                    "POST",
                    "/api/session/ses_work/form/frm_question/reply",
                    Some(json!({"answer":{"q0":"ru"}})),
                    Value::Null,
                ),
                (
                    "DELETE",
                    "/api/session/ses_work/form/frm_cancel",
                    None,
                    Value::Null,
                ),
            ],
        );
        let pending = api.pending_questions("ses_work").unwrap();
        assert_eq!(pending.len(), 1);
        let questions = crate::native_questions::questions(
            &pending[0]["fields"],
            crate::native_questions::Dialect::Forms,
        )
        .unwrap();
        assert_eq!(questions[0].question.text(), "Which language?");
        let response = cide_ipc::UserRequestResponse::Answers {
            answers: vec![cide_ipc::UserRequestAnswer {
                id: "q0".into(),
                text: String::new(),
                selected_ids: vec!["ru".into()],
            }],
        };
        let body =
            crate::native_questions::form_reply(&pending[0]["fields"], &questions, &response)
                .unwrap();
        api.reply_question("ses_work", "frm_question", Some(&body))
            .unwrap();
        api.reply_question("ses_work", "frm_cancel", None).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn native_v1_questions_are_correlated_to_question_tools_and_can_resume_or_cancel() {
        let (api, server) = plan_api(
            Generation::V1,
            vec![
                (
                    "GET",
                    "/question",
                    None,
                    json!([
                        {"id":"que_plan","sessionID":"ses_work","tool":{"messageID":"msg_plan","callID":"exit"}},
                        {"id":"que_question","sessionID":"ses_work","tool":{"messageID":"msg_question","callID":"ask"},"questions":[{"header":"Choice","question":"Choose?","options":[{"label":"A"}]}]},
                        {"id":"que_other","sessionID":"ses_other","tool":{"messageID":"msg_other","callID":"ask"}}
                    ]),
                ),
                (
                    "GET",
                    "/session/ses_work/message/msg_plan",
                    None,
                    json!({"parts":[{"type":"tool","tool":"plan_exit","callID":"exit"}]}),
                ),
                (
                    "GET",
                    "/session/ses_work/message/msg_question",
                    None,
                    json!({"parts":[{"type":"tool","tool":"question","callID":"ask"}]}),
                ),
                (
                    "POST",
                    "/question/que_question/reply",
                    Some(json!({"answers":[["A"]]})),
                    Value::Null,
                ),
                ("POST", "/question/que_cancel/reject", None, Value::Null),
            ],
        );
        let pending = api.pending_questions("ses_work").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0]["id"], "que_question");
        api.reply_question(
            "ses_work",
            "que_question",
            Some(&json!({"answers":[["A"]]})),
        )
        .unwrap();
        api.reply_question("ses_work", "que_cancel", None).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn native_plan_switches_agent_through_the_v2_api_and_requires_native_support() {
        let (api, server) = plan_api(
            Generation::V2,
            vec![
                (
                    "GET",
                    "/api/agent",
                    None,
                    json!({"data":[{"id":"plan"},{"id":"build"}]}),
                ),
                (
                    "POST",
                    "/api/session/ses_work/agent",
                    Some(json!({"agent":"build"})),
                    Value::Null,
                ),
                (
                    "POST",
                    "/api/session/ses_work/prompt",
                    Some(json!({"text":"Implement the plan.","resume":true})),
                    Value::Null,
                ),
                ("GET", "/api/agent", None, json!({"data":[]})),
            ],
        );
        api.require_native_plan().unwrap();
        api.prompt_as("ses_work", "Implement the plan.", None, Some("build"))
            .unwrap();
        assert!(
            api.require_native_plan()
                .unwrap_err()
                .contains("no native Plan agent")
        );
        server.join().unwrap();
    }

    #[test]
    fn native_plan_questions_are_correlated_to_the_exact_session_and_tool() {
        let (api, server) = plan_api(
            Generation::V1,
            vec![
                (
                    "GET",
                    "/question",
                    None,
                    json!([
                        {"id":"que_other","sessionID":"ses_other","tool":{"messageID":"msg_other","callID":"c"}},
                        {"id":"que_human","sessionID":"ses_work","questions":[{"options":[{"label":"Yes"}]}]},
                        {"id":"que_tool","sessionID":"ses_work","tool":{"messageID":"msg_question","callID":"q"}},
                        {"id":"que_plan","sessionID":"ses_work","tool":{"messageID":"msg_plan","callID":"p"},"questions":[{"options":[{"label":"Yes"},{"label":"No"}]}]}
                    ]),
                ),
                (
                    "GET",
                    "/session/ses_work/message/msg_question",
                    None,
                    json!({"parts":[{"type":"tool","tool":"question","callID":"q"}]}),
                ),
                (
                    "GET",
                    "/session/ses_work/message/msg_plan",
                    None,
                    json!({"parts":[{"type":"tool","tool":"plan_exit","callID":"p"}]}),
                ),
                (
                    "POST",
                    "/question/que_plan/reply",
                    Some(json!({"answers":[["Yes"]]})),
                    Value::Null,
                ),
                ("GET", "/question", None, json!([])),
            ],
        );
        assert!(api.approve_plan_question("ses_work").unwrap());
        assert!(!api.approve_plan_question("ses_work").unwrap());
        server.join().unwrap();
    }

    #[test]
    fn a_native_plan_requires_a_successful_plan_agent_response() {
        let (api, server) = plan_api(
            Generation::V2,
            vec![
                (
                    "GET",
                    "/api/session/ses_work/message?order=desc&limit=16",
                    None,
                    json!({"data":[{"id":"msg_plan","type":"assistant","agent":"plan","finish":"stop","content":[{"type":"text","text":"A completed plan"}]}]}),
                ),
                (
                    "GET",
                    "/api/session/ses_work/message?order=desc&limit=16",
                    None,
                    json!({"data":[{"id":"msg_plan","type":"assistant","agent":"build","finish":"stop","content":[{"type":"text","text":"plan mentioned in ordinary output"}]}]}),
                ),
                (
                    "GET",
                    "/api/session/ses_work/message?order=desc&limit=16",
                    None,
                    json!({"data":[{"id":"msg_plan","type":"assistant","agent":"plan","finish":"error","error":{"message":"failed"},"content":[{"type":"text","text":"unfinished"}]}]}),
                ),
                (
                    "GET",
                    "/api/session/ses_work/message?order=desc&limit=16",
                    None,
                    json!({"data":[{"type":"user","content":[{"type":"text","text":"A new assignment"}]},{"id":"msg_plan","type":"assistant","agent":"plan","finish":"stop","content":[{"type":"text","text":"An earlier plan"}]}]}),
                ),
            ],
        );
        assert_eq!(
            api.completed_native_plan("ses_work").unwrap().as_deref(),
            Some("msg_plan")
        );
        assert!(api.completed_native_plan("ses_work").unwrap().is_none());
        assert!(api.completed_native_plan("ses_work").unwrap().is_none());
        assert!(api.completed_native_plan("ses_work").unwrap().is_none());
        server.join().unwrap();
    }

    /// Exercise the HTTP boundary, not just the JSON helpers. Connections are deliberately
    /// closed between responses, including the event stream, to cover reconnectable EOF.
    #[test]
    fn native_session_lifecycle_and_state_over_authenticated_http() {
        for generation in [Generation::V1, Generation::V2] {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let address = listener.local_addr().unwrap();
            let prefix = if generation == Generation::V2 {
                "/api"
            } else {
                ""
            };
            let envelope = |value: Value| {
                if generation == Generation::V2 {
                    json!({"data": value})
                } else {
                    value
                }
            };
            let mut steps = vec![
                (
                    "POST",
                    format!("{prefix}/session"),
                    envelope(json!({"id":"ses_original"})),
                ),
                (
                    "GET",
                    format!("{prefix}/session/ses_original"),
                    envelope(json!({"id":"ses_original"})),
                ),
                (
                    "POST",
                    format!("{prefix}/session/ses_original/fork"),
                    envelope(json!({"id":"ses_fork"})),
                ),
            ];
            if generation == Generation::V2 {
                steps.push((
                    "PUT",
                    format!("{prefix}/experimental/session/ses_fork/instructions/entries/cide"),
                    Value::Null,
                ));
                steps.push((
                    "POST",
                    format!("{prefix}/session/ses_fork/agent"),
                    Value::Null,
                ));
            }
            steps.push((
                "POST",
                format!(
                    "{prefix}/session/ses_fork/{}",
                    if generation == Generation::V2 {
                        "prompt"
                    } else {
                        "prompt_async"
                    }
                ),
                Value::Null,
            ));
            if generation == Generation::V2 {
                // Permission takes priority over both forms and active work.
                steps.push((
                    "GET",
                    "/api/session/ses_fork/permission".into(),
                    envelope(json!([{"id":"permission"}])),
                ));
                steps.push((
                    "GET",
                    "/api/session/ses_fork/permission".into(),
                    envelope(json!([])),
                ));
                steps.push((
                    "GET",
                    "/api/session/ses_fork/form".into(),
                    envelope(json!([])),
                ));
                steps.push((
                    "GET",
                    "/api/session/active".into(),
                    envelope(json!({"ses_fork":{"type":"running"}})),
                ));
                steps.push((
                    "GET",
                    "/api/session/ses_fork/permission".into(),
                    envelope(json!([])),
                ));
                steps.push((
                    "GET",
                    "/api/session/ses_fork/form".into(),
                    envelope(json!([])),
                ));
                steps.push(("GET", "/api/session/active".into(), envelope(json!({}))));
            } else {
                steps.push((
                    "GET",
                    "/permission".into(),
                    json!([{"sessionID":"ses_fork"}]),
                ));
                for status in [json!({"ses_fork":{"type":"busy"}}), json!({})] {
                    steps.push(("GET", "/permission".into(), json!([])));
                    steps.push(("GET", "/question".into(), json!([])));
                    steps.push(("GET", "/session/status".into(), status));
                }
            }
            let server = std::thread::spawn(move || {
                for (method, path, response) in steps {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    assert_eq!(line.trim(), format!("{method} {path} HTTP/1.1"));
                    let mut length = 0;
                    let mut authenticated = false;
                    loop {
                        line.clear();
                        reader.read_line(&mut line).unwrap();
                        if line.trim().is_empty() {
                            break;
                        }
                        let header = line.to_ascii_lowercase();
                        if let Some(value) = header.strip_prefix("content-length:") {
                            length = value.trim().parse().unwrap();
                        }
                        if header.trim() == "authorization: basic b3blbmnvzgu6c2vjcmv0" {
                            authenticated = true;
                        }
                    }
                    assert!(
                        authenticated,
                        "private server credentials must accompany every call"
                    );
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    if path == format!("{prefix}/session") {
                        let body: Value = serde_json::from_slice(&body).unwrap();
                        if generation == Generation::V2 {
                            assert_eq!(body["location"]["directory"], "/project");
                            assert_eq!(body["model"]["variant"], "high");
                            assert!(
                                body.get("permissions").is_none(),
                                "attended consoles retain approvals"
                            );
                        }
                    }
                    if path.ends_with("/prompt") || path.ends_with("/prompt_async") {
                        let body: Value = serde_json::from_slice(&body).unwrap();
                        if generation == Generation::V1 {
                            assert_eq!(body["agent"], "cide-peer-reviewer");
                            assert_eq!(body["model"]["providerID"], "provider");
                            assert_eq!(body["model"]["modelID"], "model");
                        } else {
                            assert!(
                                body.get("agent").is_none(),
                                "v2 selects the agent through its native endpoint"
                            );
                        }
                    }
                    if path.ends_with("/agent") {
                        assert_eq!(
                            serde_json::from_slice::<Value>(&body).unwrap(),
                            json!({"agent":"cide-peer-reviewer"})
                        );
                    }
                    let body = response.to_string();
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                }
                let body = "data: {\"type\":\"session.created\",\"data\":{\"sessionID\":\"ses_after_clear\"}}\n\n";
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            let api = Api::new(format!("http://{address}"), "secret".into(), generation).unwrap();
            assert_eq!(
                api.create(
                    Path::new("/project"),
                    None,
                    Some("provider/model#high"),
                    None,
                    false
                )
                .unwrap(),
                "ses_original"
            );
            api.exists("ses_original").unwrap();
            assert_eq!(api.fork("ses_original").unwrap(), "ses_fork");
            api.instructions("ses_fork", "Use cide tools").unwrap();
            api.prompt_as(
                "ses_fork",
                "continue",
                Some("provider/model"),
                Some("cide-peer-reviewer"),
            )
            .unwrap();
            assert_eq!(
                api.state("ses_fork", false).unwrap(),
                SessionState::AwaitingPermission
            );
            assert_eq!(api.state("ses_fork", false).unwrap(), SessionState::Busy);
            assert_eq!(
                api.state("ses_fork", true).unwrap(),
                SessionState::AwaitingInput
            );
            let mut seen = Vec::new();
            api.events(|event| {
                seen.push(event);
                false
            })
            .unwrap();
            assert_eq!(seen[0]["data"]["sessionID"], "ses_after_clear");
            server.join().unwrap();
        }
    }
    #[test]
    fn generation_routes_and_native_events() {
        let v1 = Api::new("http://127.0.0.1:1".into(), "secret".into(), Generation::V1).unwrap();
        let v2 = Api::new("http://127.0.0.1:1".into(), "secret".into(), Generation::V2).unwrap();
        assert!(v1.path("/session").ends_with("/session"));
        assert!(v2.path("/session").ends_with("/api/session"));
        let event =
            decode_event(r#"{"type":"session.created","data":{"sessionID":"ses_test"}}"#).unwrap();
        assert_eq!(event["data"]["sessionID"], "ses_test");
        assert!(validate_id("ses_test").is_ok());
        assert!(validate_id("ses/../../auth").is_err());
        assert_eq!(model_ref("provider/model#high").unwrap()["variant"], "high");
    }
}
