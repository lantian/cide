//! A symbol's documentation as a page, from whatever the server can say. (M60)
//!
//! Two providers and one page shape (`cide_ipc::SymbolDocs`):
//!
//! * **Hover**, for every server. `textDocument/hover` is markdown by contract, and it is where
//!   a Java server puts javadoc, rust-analyzer rustdoc, gopls the standard library's comments
//!   — for symbols the user cannot open any more than they can open Godot's `Dictionary`.
//!   [`from_hover`] turns the reply into a page with a body and nothing else.
//! * **Godot's native symbols** ([`godot`]). Godot answers `textDocument/definition` for a
//!   built-in with an empty list — there is no file — and sends a `gdscript/show_native_symbol`
//!   notification on `textDocument/declaration` instead, carrying the class, its members and
//!   their documentation in the engine's BBCode. That is the richer page: a signature, a member
//!   list, and links between classes the same road can follow.
//!
//! The frontend draws one page for both. A third provider (a `jdt://` class file, a rustdoc
//! HTML page) would fill the same struct and change nothing downstream.

use std::time::Duration;

use serde_json::Value;

use crate::discover::Server;
use crate::server::{RequestError, Requester};

pub mod godot;

/// Which road a server's documentation comes by.
///
/// Keyed on the registry name, the way `config.rs` keys its fork options on `rust-analyzer`:
/// the Godot road speaks a vocabulary only Godot has (`gdscript/show_native_symbol`,
/// `textDocument/nativeSymbol`), and there is no manifest field that could declare a protocol
/// cide has no code for. Everything else is hover, which is the protocol's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Godot,
    Hover,
}

#[must_use]
pub fn provider_for(server: Server) -> Provider {
    if server.binary() == "godot" {
        Provider::Godot
    } else {
        Provider::Hover
    }
}

/// What a lookup came back with. The app turns each into its sentence.
#[derive(Debug)]
pub enum Outcome {
    Page(cide_ipc::SymbolDocs),
    /// The server answered and had nothing for this position or reference.
    Nothing,
    /// The server could not be asked, or did not answer: the app renders each variant.
    Failed(RequestError),
}

/// How long the Godot notification is given to follow the `declaration` reply.
///
/// The engine sends it *during* the request, so it is in the socket before the reply is; half a
/// second is a floor against a slow editor, not a wait. No notification inside it means the
/// editor setting `show_native_symbols_in_editor` is on — Godot opened its own Help tab instead —
/// and the hover road is the honest fallback.
const NATIVE_SYMBOL_GRACE: Duration = Duration::from_millis(500);

/// The method Godot's documentation arrives on.
pub const GODOT_NATIVE_SYMBOL: &str = "gdscript/show_native_symbol";

/// The page for the symbol at a position, by whichever road the server has.
///
/// The Godot road first where there is one — `declaration` and the notification it provokes —
/// and hover for every server, Godot included when its notification does not come. `word` is
/// the identifier under the caret, which a hover page is titled by because the reply names
/// nothing.
pub fn lookup(
    requester: &Requester,
    path: &std::path::Path,
    line: u32,
    column: u32,
    word: &str,
    timeout: Duration,
) -> Outcome {
    let server = requester.server();
    let source = server.binary();
    if provider_for(server) == Provider::Godot {
        let waiter = requester.expect_notification(GODOT_NATIVE_SYMBOL);
        let reply = requester.request(
            "textDocument/declaration",
            position_params(path, line, column),
            timeout,
        );
        match reply {
            // A dead server is a dead server whichever road; the hover below would only say it
            // again, later.
            Err(RequestError::ServerGone) => return Outcome::Failed(RequestError::ServerGone),
            Err(RequestError::Timeout) => return Outcome::Failed(RequestError::Timeout),
            _ => {}
        }
        match waiter.recv_timeout(NATIVE_SYMBOL_GRACE) {
            Ok(Ok(payload)) => {
                if let Some(page) = godot::page(&source, &payload) {
                    return Outcome::Page(page);
                }
            }
            Ok(Err(RequestError::ServerGone)) => return Outcome::Failed(RequestError::ServerGone),
            _ => {}
        }
        requester.forget_notification(GODOT_NATIVE_SYMBOL);
    }
    match requester.request(
        "textDocument/hover",
        position_params(path, line, column),
        timeout,
    ) {
        Ok(reply) => match from_hover(&source, word, &reply) {
            Some(page) => Outcome::Page(page),
            None => Outcome::Nothing,
        },
        // A server that does not implement hover answers `-32601`, and gopls answers a
        // JSON-RPC error for a position it has no opinion on: both are "nothing here", not a
        // broken server — `implementations` folds a refusal the same way.
        Err(RequestError::Failed(_)) => Outcome::Nothing,
        Err(error) => Outcome::Failed(error),
    }
}

/// The page a `#ref/<kind>/<target>` names, for a server whose pages carry references.
///
/// Godot's do: `textDocument/nativeSymbol` answers a `DocumentSymbol` for a class or a member
/// by name. A hover page carries no references, so this is never asked of any other provider —
/// and answers `Nothing` rather than guessing if it ever is.
pub fn follow(requester: &Requester, kind: &str, target: &str, timeout: Duration) -> Outcome {
    let server = requester.server();
    if provider_for(server) != Provider::Godot {
        return Outcome::Nothing;
    }
    let Some(params) = godot::native_symbol_params(kind, target) else {
        return Outcome::Nothing;
    };
    match requester.request("textDocument/nativeSymbol", params, timeout) {
        Ok(reply) => match godot::page(&server.binary(), &reply) {
            Some(page) => Outcome::Page(page),
            None => Outcome::Nothing,
        },
        Err(RequestError::Failed(_)) => Outcome::Nothing,
        Err(error) => Outcome::Failed(error),
    }
}

/// `TextDocumentPositionParams`, from cide's 1-based line and 1-based UTF-16 column.
fn position_params(path: &std::path::Path, line: u32, column: u32) -> Value {
    serde_json::json!({
        "textDocument": { "uri": crate::convert::path_to_uri(path) },
        "position": { "line": line.saturating_sub(1), "character": column.saturating_sub(1) },
    })
}

/// The page a `textDocument/hover` reply makes: the hover's markdown as the body, the word under
/// the caret as the title, no members. `None` when the server had nothing to say.
pub fn from_hover(source: &str, title: &str, reply: &Value) -> Option<cide_ipc::SymbolDocs> {
    let markdown = hover_markdown(reply.get("contents")?)?;
    if markdown.trim().is_empty() {
        return None;
    }
    // A caller with no word under the caret (the definition fallback from a keystroke that
    // found none) still gets a page with a name: the hover's own first line, which for every
    // server that writes one is the declaration.
    let title = if title.trim().is_empty() {
        markdown
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with("```"))
            .map(|line| line.trim_start_matches('#').trim().trim_matches('`'))
            .find(|line| !line.is_empty())
            .map(|line| line.chars().take(48).collect::<String>())
            .unwrap_or_else(|| "Documentation".to_string())
    } else {
        title.to_string()
    };
    Some(cide_ipc::SymbolDocs {
        title,
        kind: "symbol".into(),
        signature: None,
        markdown,
        members: Vec::new(),
        links: Vec::new(),
        source: source.to_string(),
        reference: None,
    })
}

/// LSP hover `contents` as markdown, in each of the three shapes the protocol allows.
///
/// `MarkupContent { kind, value }` is the modern one and is markdown or plaintext by `kind`;
/// `MarkedString` is a string or `{ language, value }` (a fenced block in disguise); and an array
/// of `MarkedString` is joined with a blank line. Plaintext is fenced rather than trusted: a
/// server that said *plaintext* may have written `*` and `_` that markdown would eat.
pub fn hover_markdown(contents: &Value) -> Option<String> {
    match contents {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().filter_map(marked_string).collect();
            (!parts.is_empty()).then(|| parts.join("\n\n"))
        }
        Value::Object(_) => {
            if let Some(kind) = contents.get("kind").and_then(Value::as_str) {
                let value = contents.get("value").and_then(Value::as_str)?;
                return Some(if kind == "plaintext" {
                    fenced("", value)
                } else {
                    value.to_string()
                });
            }
            marked_string(contents)
        }
        _ => None,
    }
}

fn marked_string(item: &Value) -> Option<String> {
    match item {
        Value::String(text) => Some(text.clone()),
        Value::Object(_) => {
            let value = item.get("value").and_then(Value::as_str)?;
            let language = item.get("language").and_then(Value::as_str).unwrap_or("");
            Some(fenced(language, value))
        }
        _ => None,
    }
}

/// A fenced block whose fence is longer than any backtick run inside it.
pub(crate) fn fenced(language: &str, body: &str) -> String {
    let fence = "`".repeat(longest_run(body, '`').max(2) + 1);
    format!(
        "{fence}{language}\n{}\n{fence}",
        body.trim_end_matches('\n')
    )
}

pub(crate) fn longest_run(text: &str, ch: char) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        if c == ch {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest
}

/// The in-page reference fragment: `#ref/method/Node.add_child`.
///
/// A fragment and not a scheme, because the markdown preview classifies a link by shape
/// (`links.ts`): a scheme is *external* and drawn with the leave-the-app marker, a fragment is the
/// page's own business and handed back to the host — which is exactly what a reference is.
pub fn reference(kind: &str, target: &str) -> String {
    format!("#ref/{kind}/{target}")
}

/// The inverse of [`reference`]: `(kind, target)`, or `None` for any other link.
pub fn parse_reference(href: &str) -> Option<(&str, &str)> {
    let rest = href.strip_prefix("#ref/")?;
    let (kind, target) = rest.split_once('/')?;
    if kind.is_empty() || target.is_empty() {
        return None;
    }
    Some((kind, target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_hover_reply_becomes_a_page_in_each_of_the_protocols_three_shapes() {
        let markup =
            json!({ "contents": { "kind": "markdown", "value": "**HashMap**\n\nA map." } });
        let page = from_hover("jdtls", "HashMap", &markup).expect("page");
        assert_eq!(page.title, "HashMap");
        assert_eq!(page.markdown, "**HashMap**\n\nA map.");
        assert_eq!(page.source, "jdtls");
        assert!(page.members.is_empty() && page.links.is_empty());

        let plain = json!({ "contents": { "kind": "plaintext", "value": "a_b * c" } });
        assert_eq!(
            hover_markdown(&plain["contents"]).as_deref(),
            Some("```\na_b * c\n```"),
            "plaintext is fenced, or markdown eats its stars"
        );
        let legacy = json!({ "contents": [
            { "language": "rust", "value": "pub fn len(&self) -> usize" },
            "Returns the number of elements.",
        ] });
        assert_eq!(
            hover_markdown(&legacy["contents"]).as_deref(),
            Some("```rust\npub fn len(&self) -> usize\n```\n\nReturns the number of elements.")
        );
        assert_eq!(
            hover_markdown(&json!("plain string")).as_deref(),
            Some("plain string")
        );
    }

    #[test]
    fn a_page_asked_for_with_no_word_is_titled_by_the_hovers_first_line() {
        let reply = json!({ "contents": { "kind": "markdown", "value": "```rust\npub struct Vec<T>\n```\n\nA growable array." } });
        let page = from_hover("rust-analyzer", "", &reply).expect("page");
        assert_eq!(
            page.title, "pub struct Vec<T>",
            "the fence line is skipped, the declaration is not"
        );
        let bare = from_hover("x", "  ", &json!({ "contents": "# Heading\nbody" })).expect("page");
        assert_eq!(bare.title, "Heading");
    }

    #[test]
    fn an_empty_hover_is_no_page() {
        assert!(from_hover("x", "y", &json!({ "contents": "   " })).is_none());
        assert!(from_hover("x", "y", &json!({ "contents": [] })).is_none());
        assert!(from_hover("x", "y", &json!(null)).is_none());
    }

    #[test]
    fn a_fence_outgrows_the_backticks_inside_it() {
        assert_eq!(fenced("rust", "a"), "```rust\na\n```");
        assert_eq!(fenced("", "x ``` y"), "````\nx ``` y\n````");
    }

    #[test]
    fn a_reference_round_trips_and_nothing_else_parses_as_one() {
        assert_eq!(
            reference("method", "Node.add_child"),
            "#ref/method/Node.add_child"
        );
        assert_eq!(
            parse_reference("#ref/method/Node.add_child"),
            Some(("method", "Node.add_child"))
        );
        assert_eq!(parse_reference("#ref/class/Node"), Some(("class", "Node")));
        assert_eq!(parse_reference("#heading"), None);
        assert_eq!(parse_reference("https://x"), None);
        assert_eq!(parse_reference("#ref//x"), None);
        assert_eq!(parse_reference("#ref/x/"), None);
    }
}
