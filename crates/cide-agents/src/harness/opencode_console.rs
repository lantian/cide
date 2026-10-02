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
    pub generation: Generation,
}

impl Api {
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
            generation,
        })
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
            .basic_auth("opencode", Some(&self.password))
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
            .basic_auth("opencode", Some(&self.password))
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
            .basic_auth("opencode", Some(&self.password))
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
            api.prompt("ses_fork", "continue", Some("provider/model"))
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
