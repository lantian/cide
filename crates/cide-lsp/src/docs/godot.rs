//! Godot's native-symbol documentation as a page. (M60)
//!
//! The engine keeps its class reference in BBCode — the same markup `editor/editor_help.cpp`
//! draws into the editor's Help tab — and its language server hands that BBCode over the wire
//! untouched, inside a `DocumentSymbol` whose `detail` is the declaration and whose `children`
//! are a class's members. [`page`] reads that shape; [`bbcode_to_markdown`] is the converter.
//!
//! # The tag set is the editor's, not the style guide's
//!
//! The list below is what `_add_text_to_rt` recognises, read off the engine rather than off the
//! documentation primer, because the primer describes what authors *should* write and the
//! renderer is what the XML actually contains. Formatting: `b i u s code codeblock codeblocks
//! gdscript csharp br center kbd color font url img lb rb param`. References: `method
//! constructor operator member signal enum constant annotation theme_item`, and a bare
//! `[ClassName]`, which the editor makes a link only when a class of that name exists — here,
//! when the word is shaped like one, since there is no class table on this side of the wire.
//!
//! # What is deliberately lossy
//!
//! `[u]` and `[color]` and `[center]` and `[font]` have no markdown and drop to their text;
//! `[csharp]` blocks are dropped whole, because this is a GDScript editor and the engine's own
//! Help tab hides them under the same setting; `[img]` becomes a note, because the path is
//! `res://` and nothing here can read it. Everything that *is* a link becomes one: a reference
//! is `#ref/<kind>/<Class.member>` (`super::reference`), which the page follows through
//! `textDocument/nativeSymbol` and which the preview never mistakes for a URL.
//!
//! # Plain text is escaped, and that is not optional
//!
//! Class docs are full of `snake_case`, `Vector2 * 2`, `a[0]` and lines that begin with `-`.
//! Handed to a markdown parser raw, `_` opens emphasis, `*` opens emphasis, `[` opens a link and a
//! leading `-` is a list. The engine's own hover conversion does not escape and lives with the
//! damage; this one escapes every character the parser would read, so what the author typed is
//! what the reader sees.

use serde_json::{Value, json};

use super::{fenced, longest_run, reference};

/// The page for a `gdscript/show_native_symbol` payload or a `textDocument/nativeSymbol` reply.
///
/// A class page lists its members; a member page lists nothing (its `children` are its
/// parameters, which the signature already spells). `None` when the payload has no name.
pub fn page(source: &str, payload: &Value) -> Option<cide_ipc::SymbolDocs> {
    let name = payload.get("name").and_then(Value::as_str)?.trim();
    if name.is_empty() {
        return None;
    }
    let kind = kind_word(payload.get("kind").and_then(Value::as_u64).unwrap_or(0));
    let native_class = payload
        .get("native_class")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    // The class a bare reference in this page's prose resolves against.
    let class = if kind == "class" || native_class.is_empty() {
        name
    } else {
        native_class
    };
    let title = if kind == "class" || native_class.is_empty() || native_class == name {
        name.to_string()
    } else {
        format!("{native_class}.{name}")
    };
    let documentation = payload
        .get("documentation")
        .and_then(Value::as_str)
        .unwrap_or("");
    let members = if kind == "class" {
        payload
            .get("children")
            .and_then(Value::as_array)
            .map(|children| {
                children
                    .iter()
                    .filter_map(|child| member(child, class))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let member_name = (kind != "class").then_some(name);
    Some(cide_ipc::SymbolDocs {
        title,
        kind: kind.to_string(),
        signature: payload
            .get("detail")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_string),
        markdown: bbcode_to_markdown(documentation, class),
        members,
        links: vec![cide_ipc::DocsLink {
            label: "Online reference".into(),
            href: online_url(class, kind, member_name),
        }],
        source: source.to_string(),
        reference: Some(reference(
            kind,
            &match member_name {
                Some(member) => format!("{class}.{member}"),
                None => class.to_string(),
            },
        )),
    })
}

fn member(child: &Value, class: &str) -> Option<cide_ipc::DocsMember> {
    let name = child.get("name").and_then(Value::as_str)?.trim();
    if name.is_empty() {
        return None;
    }
    let kind = kind_word(child.get("kind").and_then(Value::as_u64).unwrap_or(0));
    Some(cide_ipc::DocsMember {
        name: name.to_string(),
        kind: kind.to_string(),
        signature: child
            .get("detail")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_string),
        markdown: bbcode_to_markdown(
            child
                .get("documentation")
                .and_then(Value::as_str)
                .unwrap_or(""),
            class,
        ),
        target: Some(reference(kind, &format!("{class}.{name}"))),
    })
}

/// The `textDocument/nativeSymbol` params that fetch a `#ref/<kind>/<target>` page.
///
/// Godot's `resolve_native_symbol` takes the class and a member name, the member name empty
/// for the class itself. A bare class reference is `Node`; a member reference is `Node.name`.
pub fn native_symbol_params(kind: &str, target: &str) -> Option<Value> {
    let (class, symbol) = if kind == "class" {
        (target, "")
    } else {
        target.rsplit_once('.').unwrap_or((target, ""))
    };
    if class.is_empty() {
        return None;
    }
    Some(json!({ "native_class": class, "symbol_name": symbol }))
}

/// The class reference online, anchored to the member when there is one.
///
/// The site's anchors are `class-<class>-<kind>-<member>`, lowercase, underscores to hyphens;
/// `@GlobalScope`'s page really is `class_@globalscope.html`.
pub fn online_url(class: &str, kind: &str, member: Option<&str>) -> String {
    let page = format!(
        "https://docs.godotengine.org/en/stable/classes/class_{}.html",
        class.to_ascii_lowercase()
    );
    match member {
        Some(name) if kind != "class" => format!(
            "{page}#class-{}-{}-{}",
            class.to_ascii_lowercase().replace('_', "-"),
            kind,
            name.to_ascii_lowercase().replace('_', "-")
        ),
        _ => page,
    }
}

/// LSP `SymbolKind` numbers as the words the page carries. Godot's server uses `Class`,
/// `Method`, `Property`, `Constant`, `Enum` and `Event` (a signal); the rest are for completeness.
fn kind_word(kind: u64) -> &'static str {
    match kind {
        5 => "class",
        6 => "method",
        7 | 8 => "property",
        9 => "constructor",
        10 => "enum",
        12 => "method",
        13 => "variable",
        14 => "constant",
        22 => "constant",
        24 => "signal",
        25 => "operator",
        _ => "symbol",
    }
}

/// Godot's class-reference BBCode as markdown. `class` is what a bare `[method name]` resolves
/// against — the page's own class.
pub fn bbcode_to_markdown(text: &str, class: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        let Some(open) = rest.find('[') else {
            escape_into(&mut out, rest);
            break;
        };
        escape_into(&mut out, &rest[..open]);
        rest = &rest[open..];
        let Some(close) = rest.find(']') else {
            // No closing bracket anywhere: a literal `[`, and the rest is prose.
            escape_into(&mut out, rest);
            break;
        };
        let tag = &rest[1..close];
        let after = &rest[close + 1..];
        // A tag's own text may not contain a bracket — `[a [b] c]` is a literal `[a ` then a tag.
        if tag.contains('[') {
            escape_into(&mut out, "[");
            rest = &rest[1..];
            continue;
        }
        let (consumed, emitted) = render_tag(tag, after, class);
        out.push_str(&emitted);
        rest = &after[consumed..];
    }
    tidy(&out)
}

/// Runs of blank lines folded to one, and the edges trimmed: a block emits its own blank
/// lines on both sides, the prose around it usually has newlines of its own, and the reader
/// should not see the sum.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut newlines = 0;
    for c in text.chars() {
        if c == '\n' {
            newlines += 1;
            if newlines <= 2 {
                out.push(c);
            }
        } else {
            newlines = 0;
            out.push(c);
        }
    }
    out.trim_matches('\n').to_string()
}

/// One tag: what it emits, and how much of `after` it swallowed (a block or span's body).
fn render_tag(tag: &str, after: &str, class: &str) -> (usize, String) {
    let (name, arg) = match tag.find([' ', '=']) {
        Some(at) => (&tag[..at], tag[at..].trim_start_matches([' ', '='])),
        None => (tag, ""),
    };
    let body_until = |closer: &str| -> (usize, &str) {
        match after.find(closer) {
            Some(end) => (end + closer.len(), &after[..end]),
            None => (after.len(), after),
        }
    };
    match name {
        "b" => (0, "**".into()),
        "/b" => (0, "**".into()),
        "i" | "/i" => (0, "*".into()),
        "s" | "/s" => (0, "~~".into()),
        "u" | "/u" | "center" | "/center" | "color" | "/color" | "font" | "/font"
        | "codeblocks" | "/codeblocks" => (0, String::new()),
        "br" => (0, "\\\n".into()),
        "lb" => (0, "\\[".into()),
        "rb" => (0, "\\]".into()),
        "code" => {
            let (used, body) = body_until("[/code]");
            (used, span(body))
        }
        "kbd" => {
            let (used, body) = body_until("[/kbd]");
            (used, span(body))
        }
        "codeblock" => {
            let (used, body) = body_until("[/codeblock]");
            let language = arg
                .split_whitespace()
                .find_map(|part| part.strip_prefix("lang="))
                .unwrap_or("gdscript");
            (used, block(language, body))
        }
        "gdscript" => {
            let (used, body) = body_until("[/gdscript]");
            (used, block("gdscript", body))
        }
        "csharp" => {
            let (used, _) = body_until("[/csharp]");
            (used, String::new())
        }
        "url" => {
            let (used, body) = body_until("[/url]");
            let href = if arg.is_empty() {
                body.trim()
            } else {
                arg.trim()
            };
            let mut text = String::new();
            escape_into(&mut text, body.trim());
            (used, format!("[{text}]({href})"))
        }
        "img" => {
            let (used, body) = body_until("[/img]");
            let mut text = String::new();
            escape_into(&mut text, body.trim());
            (used, format!("*(image: {text})*"))
        }
        "param" => (0, span(arg.trim())),
        "method" | "constructor" | "operator" | "member" | "signal" | "enum" | "constant"
        | "annotation" | "theme_item" => (0, link(name, arg.trim(), class)),
        _ if is_class_name(tag) => (0, link("class", tag, class)),
        _ => {
            // Unknown, or a stray closer: the literal bracketed text, escaped.
            let mut text = String::new();
            escape_into(&mut text, "[");
            escape_into(&mut text, tag);
            escape_into(&mut text, "]");
            (0, text)
        }
    }
}

/// A reference link: text the way the editor's Help tab spells it, target the way
/// [`super::reference`] does. A bare member resolves against the page's class; a method reads as
/// `name()`; a member of another class keeps its class in the text.
fn link(kind: &str, target: &str, class: &str) -> String {
    let (owner, member) = if kind == "class" {
        (target, "")
    } else {
        target.rsplit_once('.').unwrap_or((class, target))
    };
    let full = if member.is_empty() {
        owner.to_string()
    } else {
        format!("{owner}.{member}")
    };
    let mut text = String::new();
    let shown = if kind == "class" || owner != class {
        full.clone()
    } else {
        member.to_string()
    };
    escape_into(&mut text, &shown);
    if matches!(kind, "method" | "constructor" | "operator") {
        text.push_str("()");
    }
    format!("[{text}]({})", reference(kind, &full))
}

/// The bare-tag rule: a word shaped like a class name — `Node2D`, `@GlobalScope`, `AABB` —
/// and nothing else. `[b]` and `[url=…]` never reach here; an unknown lowercase tag stays text.
fn is_class_name(tag: &str) -> bool {
    let mut chars = tag.chars();
    let first = match chars.next() {
        Some('@') => match chars.next() {
            Some(c) => c,
            None => return false,
        },
        Some(c) => c,
        None => return false,
    };
    first.is_ascii_uppercase() && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// An inline code span whose delimiter outgrows any backtick run inside it.
fn span(body: &str) -> String {
    let ticks = "`".repeat(longest_run(body, '`') + 1);
    let padded = if body.starts_with('`') || body.ends_with('`') {
        format!(" {body} ")
    } else {
        body.to_string()
    };
    format!("{ticks}{padded}{ticks}")
}

/// A fenced block, its common indentation removed: the XML the docs live in indents code by the
/// element's own nesting, and a block that kept it would draw every line pushed right.
fn block(language: &str, body: &str) -> String {
    let lines: Vec<&str> = body
        .lines()
        .skip_while(|line| line.trim().is_empty())
        .collect();
    let lines: Vec<&str> = {
        let mut trimmed = lines;
        while trimmed.last().is_some_and(|line| line.trim().is_empty()) {
            trimmed.pop();
        }
        trimmed
    };
    let indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    let dedented: Vec<&str> = lines
        .iter()
        .map(|line| {
            if line.len() >= indent {
                &line[indent..]
            } else {
                line.trim_start()
            }
        })
        .collect();
    let language = if language == "text" { "" } else { language };
    format!("\n\n{}\n\n", fenced(language, &dedented.join("\n")))
}

/// Prose, with every character the markdown parser would read escaped.
///
/// Always: `\ * _ [ ] ` ~`. At the start of a line (after indentation): `# > - +`, and the
/// `.`/`)` after a run of digits, so a sentence that starts with a year is not a numbered list.
fn escape_into(out: &mut String, text: &str) {
    let mut at_line_start = out.is_empty() || out.ends_with('\n');
    let mut digits_at_line_start = false;
    for c in text.chars() {
        if at_line_start {
            match c {
                ' ' | '\t' => {
                    out.push(c);
                    continue;
                }
                '#' | '>' | '-' | '+' => {
                    out.push('\\');
                    out.push(c);
                    at_line_start = false;
                    continue;
                }
                '0'..='9' => {
                    digits_at_line_start = true;
                    out.push(c);
                    at_line_start = false;
                    continue;
                }
                _ => at_line_start = false,
            }
        }
        if digits_at_line_start {
            match c {
                '0'..='9' => {
                    out.push(c);
                    continue;
                }
                '.' | ')' => {
                    out.push('\\');
                    out.push(c);
                    digits_at_line_start = false;
                    continue;
                }
                _ => digits_at_line_start = false,
            }
        }
        match c {
            '\\' | '*' | '_' | '[' | ']' | '`' | '~' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => {
                out.push('\n');
                at_line_start = true;
            }
            _ => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(text: &str) -> String {
        bbcode_to_markdown(text, "Node")
    }

    #[test]
    fn bold_italic_strike_and_the_tags_markdown_cannot_say() {
        assert_eq!(
            md("[b]Note:[/b] Dictionaries are passed by reference."),
            "**Note:** Dictionaries are passed by reference."
        );
        assert_eq!(md("[i]x[/i] [s]y[/s]"), "*x* ~~y~~");
        assert_eq!(
            md("[u]u[/u] [center]c[/center] [color=red]r[/color] [font=x]f[/font]"),
            "u c r f",
            "no markdown for these: the text survives, the tags do not"
        );
        assert_eq!(
            md("one[br]two"),
            "one\\\ntwo",
            "a hard break, the spelling the parser reads"
        );
    }

    #[test]
    fn code_spans_and_params_keep_their_text_verbatim() {
        assert_eq!(md("or [code]null[/code] if absent"), "or `null` if absent");
        assert_eq!(
            md("[code]a_b * c[/code]"),
            "`a_b * c`",
            "nothing inside a span is escaped"
        );
        assert_eq!(
            md("[code]a`b[/code]"),
            "``a`b``",
            "the delimiter outgrows the backtick"
        );
        assert_eq!(md("[kbd]Ctrl + C[/kbd]"), "`Ctrl + C`");
        assert_eq!(md("the value at [param key]"), "the value at `key`");
    }

    #[test]
    fn code_blocks_are_fenced_dedented_and_csharp_is_dropped() {
        let text = "Example:\n[codeblocks]\n[gdscript]\n    var d = {}\n    d[\"a\"] = 1\n[/gdscript]\n[csharp]\n    var d = new Dictionary();\n[/csharp]\n[/codeblocks]\nDone.";
        let got = md(text);
        assert!(
            got.contains("```gdscript\nvar d = {}\nd[\"a\"] = 1\n```"),
            "{got}"
        );
        assert!(!got.contains("Dictionary()"), "the C# block is gone: {got}");
        assert!(
            got.starts_with("Example:\n\n```gdscript"),
            "one blank line, however many the tags left: {got}"
        );
        assert!(got.ends_with("```\n\nDone."), "{got}");
        assert_eq!(
            md("[codeblock lang=text]\n\t\ta\n\t\t  b\n[/codeblock]"),
            "```\na\n  b\n```",
            "`lang=text` is an unlanguaged fence and the common tab indent is removed"
        );
        assert_eq!(
            md("[codeblock]\nprint(1)\n[/codeblock]"),
            "```gdscript\nprint(1)\n```",
            "a bare codeblock is GDScript"
        );
    }

    #[test]
    fn references_become_in_page_links_resolved_against_the_class() {
        assert_eq!(
            md("See [method Dictionary.get_or_add]."),
            "See [Dictionary.get\\_or\\_add()](#ref/method/Dictionary.get_or_add)."
        );
        assert_eq!(
            md("See [method add_child] and [member name]."),
            "See [add\\_child()](#ref/method/Node.add_child) and [name](#ref/member/Node.name).",
            "a bare member resolves against the page's own class and drops the class from the text"
        );
        assert_eq!(
            md("A [Node2D] is a [Node]."),
            "A [Node2D](#ref/class/Node2D) is a [Node](#ref/class/Node)."
        );
        assert_eq!(
            md("[constant Vector2.ZERO]"),
            "[Vector2.ZERO](#ref/constant/Vector2.ZERO)"
        );
        assert_eq!(md("[signal ready]"), "[ready](#ref/signal/Node.ready)");
        assert_eq!(
            md("[enum ProcessMode]"),
            "[ProcessMode](#ref/enum/Node.ProcessMode)"
        );
        assert_eq!(
            md("[annotation @GDScript.@export]"),
            "[@GDScript.@export](#ref/annotation/@GDScript.@export)"
        );
        assert_eq!(
            md("[@GlobalScope]"),
            "[@GlobalScope](#ref/class/@GlobalScope)"
        );
        assert_eq!(
            md("[theme_item font_color]"),
            "[font\\_color](#ref/theme_item/Node.font_color)"
        );
    }

    #[test]
    fn urls_images_and_literal_brackets() {
        assert_eq!(
            md("[url=https://x.y]the docs[/url]"),
            "[the docs](https://x.y)"
        );
        assert_eq!(md("[url]https://x.y[/url]"), "[https://x.y](https://x.y)");
        assert_eq!(md("[img]res://icon.png[/img]"), "*(image: res://icon.png)*");
        assert_eq!(md("[lb]i[rb]"), "\\[i\\]");
    }

    #[test]
    fn prose_is_escaped_where_the_parser_would_read_it() {
        assert_eq!(md("a_b * c"), "a\\_b \\* c");
        assert_eq!(md("# not a heading"), "\\# not a heading");
        assert_eq!(
            md("- not a list\n+ nor this\n> nor a quote"),
            "\\- not a list\n\\+ nor this\n\\> nor a quote"
        );
        assert_eq!(md("2024. A year"), "2024\\. A year");
        assert_eq!(md("a - b"), "a - b", "a dash mid-line is a dash");
        assert_eq!(md("x ~ y"), "x \\~ y");
    }

    #[test]
    fn unknown_tags_and_lone_brackets_stay_as_text() {
        assert_eq!(md("[foo bar]"), "\\[foo bar\\]");
        assert_eq!(
            md("[/b] stray"),
            "** stray",
            "a stray bold closer is still a delimiter"
        );
        assert_eq!(md("[/url] stray"), "\\[/url\\] stray");
        assert_eq!(md("a [ b"), "a \\[ b");
        assert_eq!(
            md("a [x [y] z"),
            "a \\[x [y](#ref/class/y) z".replace("[y](#ref/class/y)", "\\[y\\]"),
            "a bracket inside a tag makes the first one literal"
        );
    }

    #[test]
    fn a_class_payload_is_a_page_with_members_and_a_member_payload_is_a_leaf() {
        let class = serde_json::json!({
            "name": "Node", "kind": 5, "detail": "<Native> class Node extends Object",
            "documentation": "A [b]node[/b]. See [method add_child].",
            "native_class": "Node",
            "children": [
                { "name": "add_child", "kind": 6,
                  "detail": "func Node.add_child(node: Node, force_readable_name: bool = false) -> void",
                  "documentation": "Adds [param node] as a child." },
                { "name": "name", "kind": 7, "detail": "var Node.name: StringName",
                  "documentation": "" },
                { "name": "ready", "kind": 24, "detail": "signal Node.ready()", "documentation": "Emitted." },
                { "name": "", "kind": 6 },
            ],
        });
        let page = super::page("godot", &class).expect("page");
        assert_eq!(page.title, "Node");
        assert_eq!(page.kind, "class");
        assert_eq!(
            page.signature.as_deref(),
            Some("<Native> class Node extends Object")
        );
        assert_eq!(
            page.markdown,
            "A **node**. See [add\\_child()](#ref/method/Node.add_child)."
        );
        assert_eq!(page.members.len(), 3, "a nameless child is not a member");
        assert_eq!(page.members[0].kind, "method");
        assert_eq!(page.members[0].markdown, "Adds `node` as a child.");
        assert_eq!(
            page.members[0].target.as_deref(),
            Some("#ref/method/Node.add_child")
        );
        assert_eq!(page.members[1].kind, "property");
        assert_eq!(page.members[2].kind, "signal");
        assert_eq!(
            page.links[0].href,
            "https://docs.godotengine.org/en/stable/classes/class_node.html"
        );
        assert_eq!(page.source, "godot");
        assert_eq!(
            page.reference.as_deref(),
            Some("#ref/class/Node"),
            "how to ask for this page again"
        );

        let method = serde_json::json!({
            "name": "add_child", "kind": 6, "native_class": "Node",
            "detail": "func Node.add_child(node: Node) -> void",
            "documentation": "Adds a child.",
            "children": [ { "name": "node", "kind": 13, "detail": "Node" } ],
        });
        let page = super::page("godot", &method).expect("page");
        assert_eq!(page.title, "Node.add_child");
        assert_eq!(page.kind, "method");
        assert!(
            page.members.is_empty(),
            "a method's children are its parameters, not members"
        );
        assert_eq!(
            page.reference.as_deref(),
            Some("#ref/method/Node.add_child")
        );
        assert_eq!(
            page.links[0].href,
            "https://docs.godotengine.org/en/stable/classes/class_node.html#class-node-method-add-child"
        );
        assert!(super::page("godot", &serde_json::json!({ "kind": 5 })).is_none());
    }

    #[test]
    fn a_reference_becomes_the_native_symbol_request() {
        assert_eq!(
            native_symbol_params("class", "Node"),
            Some(serde_json::json!({ "native_class": "Node", "symbol_name": "" }))
        );
        assert_eq!(
            native_symbol_params("method", "Node.add_child"),
            Some(serde_json::json!({ "native_class": "Node", "symbol_name": "add_child" }))
        );
        assert_eq!(
            native_symbol_params("annotation", "@GDScript.@export"),
            Some(serde_json::json!({ "native_class": "@GDScript", "symbol_name": "@export" }))
        );
        assert_eq!(native_symbol_params("class", ""), None);
        assert_eq!(
            online_url("@GlobalScope", "method", Some("print_rich")),
            "https://docs.godotengine.org/en/stable/classes/class_@globalscope.html#class-@globalscope-method-print-rich"
        );
    }
}
