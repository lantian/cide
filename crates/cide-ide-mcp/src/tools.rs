//! The five tool handlers, and the schemas that advertise them.
//!
//! # Why these take arguments instead of a server
//!
//! Every handler here is given exactly what it needs — a broker, an event sender, the
//! connection and pane it is answering for — rather than a `&IdeServer`. A handler is then a
//! function over values, which means the interesting cases (a diff nobody answers, a
//! `close_tab` for a tab that never existed) can be tested without a socket, a listener or a
//! lockfile.
//!
//! # One of these blocks
//!
//! [`open_diff`] awaits a human. The agent's turn does not proceed until it returns, so it is
//! spawned as its own task by [`crate::server`] and must never be called from anywhere that
//! also has to keep reading the socket — the CLI sends `close_tab` down the same connection
//! that is waiting on the diff.

use std::collections::HashSet;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::diff_broker::{CancelReason, DiffBroker, DiffRequest};
use crate::protocol::{CloseTabParams, Content, DiffOutcome, OpenDiffParams, tool};
use crate::server::ServerEvent;

/// An MCP tool result: the `content` array plus the flag that says whether it is a failure.
///
/// `isError` is a field of the *result*, not a JSON-RPC error. The distinction matters: a
/// JSON-RPC error ends the CLI's call with an exception, whereas `isError` hands the agent a
/// message it can read and act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub content: Vec<Content>,
    pub is_error: bool,
}

impl ToolResult {
    pub fn ok(content: Vec<Content>) -> Self {
        Self {
            content,
            is_error: false,
        }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self::ok(vec![Content::text(text)])
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: vec![Content::text(message)],
            is_error: true,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({ "content": self.content, "isError": self.is_error })
    }
}

/// Where a handler puts the work only the application can do: opening a tab, revealing a file.
pub type Events = mpsc::Sender<ServerEvent>;

/// Emit, and report whether anyone was still listening.
///
/// A dropped receiver is not an error in itself — it means the app is going away — but
/// [`open_diff`] has to know, because an event nobody receives is a diff nobody will ever
/// answer.
async fn emit(events: &Events, event: ServerEvent) -> bool {
    events.send(event).await.is_ok()
}

/// Show a proposed edit and wait for the human.
///
/// The `openDiff` reply carries the user's decision as one of three sentinel strings; see
/// [`DiffOutcome`]. Everything between registering with the broker and the `await` below
/// exists so that the decision can arrive from anywhere — the diff view, a closing pane, a
/// quitting app — rather than only from the socket this call came in on.
pub async fn open_diff(
    arguments: &Value,
    broker: &DiffBroker,
    connection: u64,
    pane: Option<String>,
    events: &Events,
) -> ToolResult {
    let params: OpenDiffParams = match serde_json::from_value(arguments.clone()) {
        Ok(p) => p,
        Err(e) => return ToolResult::error(format!("openDiff: unusable arguments: {e}")),
    };

    let request = DiffRequest {
        // Ours, not the JSON-RPC id: the frontend answers diffs and knows nothing about
        // which connection asked.
        id: uuid::Uuid::new_v4().to_string(),
        params,
        pane,
    };
    let mut waiting = broker.open(request.clone(), connection);
    let id = request.id.clone();

    // The request is registered before it is announced, so a cancellation can land while the
    // announcement is still queued. `ServerEvent` is a bounded channel and a busy application
    // applies back-pressure here; waiting on the send alone would then survive the pane
    // closing, the socket dropping and the app quitting, and the agent's turn would never end.
    // Racing the two means every cancel path reaches this call however slow the frontend is.
    let announced = tokio::select! {
        delivered = emit(events, ServerEvent::DiffRequested(request)) => delivered,
        resolved = &mut waiting => {
            return ToolResult::ok(resolved.unwrap_or(DiffOutcome::Rejected).to_content());
        }
    };

    if !announced {
        // Nothing is draining the event stream, so no tab will ever open and no human will
        // ever answer. Withdrawing now turns a permanent hang into a refusal the agent can
        // report.
        broker.cancel(&id, CancelReason::Shutdown);
        return ToolResult::error("the editor is not accepting diffs");
    }

    match waiting.await {
        Ok(outcome) => ToolResult::ok(outcome.to_content()),
        // The broker dropped the sender without resolving. Rejection is the safe reading of
        // an unanswered diff: it leaves the file alone.
        Err(_) => ToolResult::ok(DiffOutcome::Rejected.to_content()),
    }
}

/// Withdraw one diff tab at the CLI's request.
///
/// An unknown tab is **success**. The CLI closes its tabs from `beforeExit` and from an
/// abort handler, both of which fire for tabs the user already dealt with; answering those
/// with an error would put a failure in the transcript of an otherwise clean exit.
pub async fn close_tab(arguments: &Value, broker: &DiffBroker, events: &Events) -> ToolResult {
    let params: CloseTabParams = match serde_json::from_value(arguments.clone()) {
        Ok(p) => p,
        Err(e) => return ToolResult::error(format!("close_tab: unusable arguments: {e}")),
    };

    broker.cancel_tab(&params.tab_name, CancelReason::ClientClosed);
    emit(
        events,
        ServerEvent::DiffWithdrawn {
            tab_name: params.tab_name,
        },
    )
    .await;

    // The CLI awaits this call and never inspects the body. An empty content array says so
    // honestly; anything else would look like a sentinel and invite someone to read it.
    ToolResult::ok(Vec::new())
}

/// Withdraw every diff tab **this connection** opened.
///
/// Not every diff in the application. The CLI sends this from its own shutdown path, and a
/// project with two Claude panes would otherwise have one pane's exit tear down the other
/// pane's live diff.
pub async fn close_all_diff_tabs(
    broker: &DiffBroker,
    connection: u64,
    events: &Events,
) -> ToolResult {
    // The broker keys cancellation by connection but does not expose the connection on the
    // way out, so the tab names come from the difference between before and after. A racing
    // resolution elsewhere can put one extra name in that difference; a `DiffWithdrawn` for
    // a tab that has already gone is a no-op in the app, which is the harmless direction.
    let before = broker.pending();
    broker.cancel_connection(connection, CancelReason::ClientClosed);
    let survivors: HashSet<String> = broker.pending().into_iter().map(|r| r.id).collect();

    for request in before {
        if !survivors.contains(&request.id) {
            emit(
                events,
                ServerEvent::DiffWithdrawn {
                    tab_name: request.params.tab_name,
                },
            )
            .await;
        }
    }

    ToolResult::ok(Vec::new())
}

/// Language-server diagnostics for a file, or for everything.
///
/// This is not a stub awaiting an implementation. cide runs no language server, so nothing in
/// this process has ever looked at the file; an empty list is the true answer to "what did
/// the language server find". The alternatives are both lies — an error claims something went
/// wrong, and invented entries claim work nobody did. The CLI parses `content[0].text` as a
/// JSON array of `{uri, diagnostics}`, so the empty array goes there as text.
pub fn get_diagnostics() -> ToolResult {
    ToolResult::text("[]")
}

/// Arguments to `openFile`.
///
/// Not in [`crate::protocol`] because the CLI never spells these names in a string the binary
/// exposes: `openFile` appears there as a tool name with no reachable call site. The aliases
/// are the hedge that follows from that — a wire spelling we guessed wrong would otherwise
/// make the whole tool inert rather than merely imprecise.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenFileArgs {
    #[serde(alias = "file_path", alias = "path")]
    file_path: String,
    #[serde(default, alias = "start_line")]
    start_line: Option<u32>,
    #[serde(default, alias = "end_line")]
    end_line: Option<u32>,
}

/// Reveal a file, optionally at a range.
pub async fn open_file(arguments: &Value, events: &Events) -> ToolResult {
    let args: OpenFileArgs = match serde_json::from_value(arguments.clone()) {
        Ok(a) => a,
        Err(e) => return ToolResult::error(format!("openFile: unusable arguments: {e}")),
    };

    // 0-based on the wire, 1-based everywhere a user can see. Converting here is what keeps
    // the rest of the codebase from having to remember which it holds. `saturating_add`
    // rather than `+` because a line number at `u32::MAX` is malformed input, not a reason
    // to abort a release build differently from a debug one.
    let event = ServerEvent::OpenFile {
        path: args.file_path,
        start_line: args.start_line.map(|l| l.saturating_add(1)),
        end_line: args.end_line.map(|l| l.saturating_add(1)),
    };

    if !emit(events, event).await {
        return ToolResult::error("the editor is not accepting files");
    }
    ToolResult::ok(Vec::new())
}

/// The `tools/list` payload: every tool in [`tool::ALL`], with a JSON-Schema for its input.
///
/// Driven off `ALL` rather than written out, so a tool added to the protocol module without a
/// schema here shows up as a test failure instead of an entry the CLI cannot call.
pub fn descriptors() -> Vec<Value> {
    tool::ALL
        .iter()
        .map(|name| {
            json!({
                "name": name,
                "description": description(name),
                "inputSchema": input_schema(name),
            })
        })
        .collect()
}

fn description(name: &str) -> &'static str {
    match name {
        tool::OPEN_DIFF => {
            "Show a proposed edit in the editor and wait for the user to accept, edit or reject it."
        }
        tool::CLOSE_TAB => "Close a diff tab by name.",
        tool::CLOSE_ALL_DIFF_TABS => "Close every diff tab this session opened.",
        tool::GET_DIAGNOSTICS => {
            "Language-server diagnostics, as a JSON array of {uri, diagnostics}."
        }
        tool::OPEN_FILE => "Reveal a file in the editor, optionally at a line range.",
        _ => "",
    }
}

fn input_schema(name: &str) -> Value {
    match name {
        // Snake_case here and camelCase in the tool name is the protocol's inconsistency,
        // reproduced rather than corrected: the CLI sends exactly these keys.
        tool::OPEN_DIFF => json!({
            "type": "object",
            "properties": {
                "old_file_path": { "type": "string" },
                "new_file_path": { "type": "string" },
                "new_file_contents": { "type": "string" },
                "tab_name": { "type": "string" },
            },
            "required": ["old_file_path", "new_file_path", "new_file_contents", "tab_name"],
        }),
        tool::CLOSE_TAB => json!({
            "type": "object",
            "properties": { "tab_name": { "type": "string" } },
            "required": ["tab_name"],
        }),
        tool::CLOSE_ALL_DIFF_TABS => json!({ "type": "object", "properties": {} }),
        // The CLI calls this both with `{uri: "file://…"}` and with `{}`, so `uri` is
        // declared and optional.
        tool::GET_DIAGNOSTICS => json!({
            "type": "object",
            "properties": { "uri": { "type": "string" } },
        }),
        tool::OPEN_FILE => json!({
            "type": "object",
            "properties": {
                "filePath": { "type": "string" },
                "startLine": { "type": "integer" },
                "endLine": { "type": "integer" },
            },
            "required": ["filePath"],
        }),
        // Unreachable while `descriptors` walks `ALL`; an empty schema is the answer that
        // cannot mislead a client into sending arguments we would ignore.
        _ => json!({ "type": "object" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff_args(path: &str, tab: &str) -> Value {
        json!({
            "old_file_path": path,
            "new_file_path": path,
            "new_file_contents": "new contents",
            "tab_name": tab,
        })
    }

    #[test]
    fn every_advertised_tool_has_a_schema_and_a_description() {
        let listed = descriptors();
        assert_eq!(listed.len(), tool::ALL.len());

        for (entry, expected) in listed.iter().zip(tool::ALL) {
            assert_eq!(entry["name"], json!(expected));
            assert_eq!(entry["inputSchema"]["type"], json!("object"));
            assert!(
                !entry["description"].as_str().unwrap_or_default().is_empty(),
                "{expected} is advertised with no description"
            );
        }
    }

    #[test]
    fn the_five_are_the_only_five() {
        // Seven other names circulated in planning for this milestone. None of them are in
        // the shipped CLI, so serving them would be handlers no client ever calls.
        let names: Vec<String> = descriptors()
            .iter()
            .filter_map(|d| d["name"].as_str().map(str::to_owned))
            .collect();
        assert_eq!(
            names,
            [
                "openDiff",
                "close_tab",
                "closeAllDiffTabs",
                "getDiagnostics",
                "openFile"
            ]
        );
    }

    #[tokio::test]
    async fn get_diagnostics_answers_an_empty_list() {
        let result = get_diagnostics();
        assert!(!result.is_error);
        assert_eq!(result.content, vec![Content::text("[]")]);
        // The CLI runs `JSON.parse` on this text and iterates the result.
        let parsed: Value = serde_json::from_str("[]").expect("the answer is valid JSON");
        assert_eq!(parsed, json!([]));
    }

    #[tokio::test]
    async fn closing_a_tab_that_was_never_open_succeeds() {
        let broker = DiffBroker::new();
        let (tx, mut rx) = mpsc::channel(4);

        let result = close_tab(&json!({ "tab_name": "never-existed" }), &broker, &tx).await;

        assert!(
            !result.is_error,
            "the CLI's beforeExit must not see a failure"
        );
        // The app is still told, because a tab it is showing may have outlived the broker
        // entry that created it.
        assert!(matches!(
            rx.try_recv(),
            Ok(ServerEvent::DiffWithdrawn { tab_name }) if tab_name == "never-existed"
        ));
    }

    #[tokio::test]
    async fn closing_a_tab_rejects_the_diff_waiting_on_it() {
        let broker = DiffBroker::new();
        let (tx, mut rx) = mpsc::channel(4);

        let diff = tokio::spawn({
            let broker = broker.clone();
            let tx = tx.clone();
            async move { open_diff(&diff_args("/f.rs", "tab-1"), &broker, 1, None, &tx).await }
        });

        let Some(ServerEvent::DiffRequested(_)) = rx.recv().await else {
            panic!("the app is told to open a tab before anything waits on it");
        };

        close_tab(&json!({ "tab_name": "tab-1" }), &broker, &tx).await;

        let result = diff.await.expect("the handler task finished");
        assert_eq!(result.content, DiffOutcome::Rejected.to_content());
    }

    #[tokio::test]
    async fn a_diff_answers_with_whatever_the_user_chose() {
        let broker = DiffBroker::new();
        let (tx, mut rx) = mpsc::channel(4);

        let diff = tokio::spawn({
            let broker = broker.clone();
            let tx = tx.clone();
            async move {
                open_diff(
                    &diff_args("/f.rs", "tab-1"),
                    &broker,
                    1,
                    Some("pane-a".into()),
                    &tx,
                )
                .await
            }
        });

        let Some(ServerEvent::DiffRequested(request)) = rx.recv().await else {
            panic!("expected a diff request");
        };
        assert_eq!(request.pane.as_deref(), Some("pane-a"));
        broker.resolve(
            &request.id,
            DiffOutcome::Saved {
                contents: "what the user left behind".into(),
            },
        );

        let result = diff.await.expect("the handler task finished");
        assert!(!result.is_error);
        // Order is load-bearing: the CLI writes `content[1].text` to the file.
        assert_eq!(result.content[0], Content::text("FILE_SAVED"));
        assert_eq!(
            result.content[1],
            Content::text("what the user left behind")
        );
    }

    #[tokio::test]
    async fn a_cancel_ends_the_call_even_while_the_app_is_not_listening() {
        // Capacity one and nothing reading it: the `DiffRequested` send blocks. The pane then
        // closes. Without the race in `open_diff`, this call would sit on the send for ever
        // and the agent's turn would never finish.
        let broker = DiffBroker::new();
        let (tx, _rx) = mpsc::channel(1);
        let blocker = tx.clone();
        blocker
            .send(ServerEvent::DiffWithdrawn {
                tab_name: "filler".into(),
            })
            .await
            .expect("the channel takes its one event");

        let diff = tokio::spawn({
            let broker = broker.clone();
            async move { open_diff(&diff_args("/f.rs", "tab-1"), &broker, 1, None, &tx).await }
        });

        // The handler has to have registered before cancelling means anything.
        while broker.is_empty() {
            tokio::task::yield_now().await;
        }
        broker.cancel_tab("tab-1", CancelReason::PaneClosed);

        let result = tokio::time::timeout(std::time::Duration::from_secs(5), diff)
            .await
            .expect("the call ends rather than waiting on the stalled app")
            .expect("the handler task finished");
        assert_eq!(result.content, DiffOutcome::Rejected.to_content());
    }

    #[tokio::test]
    async fn a_diff_nobody_can_receive_is_refused_rather_than_awaited() {
        let broker = DiffBroker::new();
        let (tx, rx) = mpsc::channel(4);
        drop(rx);

        let result = open_diff(&diff_args("/f.rs", "tab-1"), &broker, 1, None, &tx).await;

        assert!(result.is_error);
        assert!(
            broker.is_empty(),
            "a refused diff must not stay in the registry"
        );
    }

    #[tokio::test]
    async fn close_all_leaves_another_connections_diff_alone() {
        let broker = DiffBroker::new();
        let (tx, mut rx) = mpsc::channel(8);

        let mine = tokio::spawn({
            let broker = broker.clone();
            let tx = tx.clone();
            async move { open_diff(&diff_args("/mine.rs", "tab-mine"), &broker, 1, None, &tx).await }
        });
        let Some(ServerEvent::DiffRequested(_)) = rx.recv().await else {
            panic!("expected the first diff");
        };

        let theirs = tokio::spawn({
            let broker = broker.clone();
            let tx = tx.clone();
            async move {
                open_diff(
                    &diff_args("/theirs.rs", "tab-theirs"),
                    &broker,
                    2,
                    None,
                    &tx,
                )
                .await
            }
        });
        let Some(ServerEvent::DiffRequested(other)) = rx.recv().await else {
            panic!("expected the second diff");
        };

        close_all_diff_tabs(&broker, 1, &tx).await;

        assert_eq!(
            mine.await.expect("the handler task finished").content,
            DiffOutcome::Rejected.to_content()
        );
        assert_eq!(
            broker.len(),
            1,
            "the other pane's claude is still waiting on its own diff"
        );

        broker.resolve(&other.id, DiffOutcome::TabClosed);
        assert_eq!(
            theirs.await.expect("the handler task finished").content,
            DiffOutcome::TabClosed.to_content()
        );
    }

    #[tokio::test]
    async fn opening_a_file_moves_the_lines_to_one_based() {
        let (tx, mut rx) = mpsc::channel(4);

        let result = open_file(
            &json!({ "filePath": "/src/main.rs", "startLine": 0, "endLine": 9 }),
            &tx,
        )
        .await;

        assert!(!result.is_error);
        match rx.try_recv() {
            Ok(ServerEvent::OpenFile {
                path,
                start_line,
                end_line,
            }) => {
                assert_eq!(path, "/src/main.rs");
                // The wire's line 0 is the first line, which every user calls line 1.
                assert_eq!(start_line, Some(1));
                assert_eq!(end_line, Some(10));
            }
            other => panic!("expected an OpenFile event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn opening_a_file_without_a_range_mentions_the_whole_file() {
        let (tx, mut rx) = mpsc::channel(4);

        open_file(&json!({ "file_path": "/src/lib.rs" }), &tx).await;

        assert!(matches!(
            rx.try_recv(),
            Ok(ServerEvent::OpenFile { path, start_line: None, end_line: None }) if path == "/src/lib.rs"
        ));
    }

    #[tokio::test]
    async fn arguments_that_do_not_fit_are_an_error_result_not_a_panic() {
        let broker = DiffBroker::new();
        let (tx, _rx) = mpsc::channel(4);

        assert!(
            open_diff(&json!({ "tab_name": "only-this" }), &broker, 1, None, &tx)
                .await
                .is_error
        );
        assert!(close_tab(&json!({}), &broker, &tx).await.is_error);
        assert!(open_file(&json!({ "startLine": 3 }), &tx).await.is_error);
    }
}
