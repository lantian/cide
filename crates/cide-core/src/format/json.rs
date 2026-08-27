//! Reformat JSONC: the same tokens, new whitespace. (M32)
//!
//! The builtin road of Reformat code, for the one builtin language whose files are routinely
//! minified and whose grammar is small enough to reprint exactly. cide's `json` language id
//! also claims `.jsonc` and declares `//` and `/* */` comments, and the files people actually
//! format — tsconfig, VS Code settings, this repo's own keybindings — carry comments,
//! duplicate keys, integers past 2^53 and trailing commas. `serde_json::to_string_pretty`
//! destroys every one of those silently: the parse succeeds, the output looks right, and the
//! comment (or the 64-bit id's low digits) is simply gone. `cide_core::scheme`'s
//! `strip_jsonc` is the same lexical knowledge pointed the other way — it *deletes* comments
//! so serde can read a theme; this module exists because deleting them is the failure.
//!
//! # The invariant: no token is added, removed, or rewritten
//!
//! `lex(reformat(x)) == lex(x)`, kinds and texts both — the formatter only moves whitespace
//! between tokens. That single promise is what makes the serde failure class unrepresentable:
//! comments, key order, duplicate keys, number spellings, string escapes and trailing commas
//! all survive because their tokens are emitted byte for byte. It is asserted as a test over
//! the whole corpus (`no_token_is_added_or_removed`), and every printer rule below is written
//! under it. (One deliberate nuance: the lexer itself trims a line comment's trailing spaces,
//! so the equality is exact rather than "equal modulo trailing whitespace".)
//!
//! Two consequences worth naming. A trailing comma is *preserved*, never repaired — repairing
//! it would turn a strict parser's error into a success, and changing what a downstream
//! consumer sees is not formatting. And scalar spellings are never validated: `1e310`,
//! `9007199254740993` and a bare unquoted key all pass through untouched, because the
//! formatter's job is whitespace and refusing them would reinterpret data it has no business
//! reading.
//!
//! # Iterative, never recursive
//!
//! The structure check and the printer are one loop over the token stream with an explicit
//! `Vec` for open containers. Not style: the input cap upstream (`MAX_FORMAT_BYTES`, 1 MiB)
//! still admits `[[[[…` half a million levels deep, and a recursive-descent printer would
//! overflow the blocking-pool thread's stack and abort the whole process on it.

use std::fmt;

/// The most output `reformat` will build before refusing.
///
/// Not a tuning knob — a crash guard, and the reason there is no depth limit. Indentation is
/// quadratic in nesting: every level adds one unit to every line below it, so a 1 MiB
/// `[[[[…` would print on the order of a hundred gigabytes of spaces if nothing counted.
/// The input cap bounds tokens, not indent × lines, so the output has to be bounded here.
/// Sixteen MiB is roughly an order of magnitude above what a real 1 MiB minified document
/// expands to at real-world nesting depths.
const MAX_OUTPUT_BYTES: usize = 16 << 20;

/// Where and why a reformat refused.
///
/// Positions are 1-based and counted in *characters*, because the sentence is read by a
/// person looking at a gutter, not by an LSP client — an LSP `Position` is 0-based UTF-16
/// code units, and converting to that here would make the number wrong in the one place a
/// human reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    pub line: usize,
    pub column: usize,
    pub what: &'static str,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (line {}, column {})",
            self.what, self.line, self.column
        )
    }
}

/// Reformat JSONC text: one member or element per line, comments kept where they were said.
///
/// `tab_size`/`insert_spaces` are the editor's own indent settings, taken as plain values so
/// the caller (`cide_app::cmd::format`) hands over exactly what it already reads for the LSP
/// road's `FormattingOptions` — a formatter told a width the user did not set disagrees with
/// the editor on every line.
///
/// A document with no tokens besides comments — empty, whitespace, or a license header
/// awaiting content — is returned unchanged rather than refused: the caller maps an identical
/// answer to silence, and a red "syntax error" on an empty scratch buffer would be noise.
pub fn reformat(text: &str, tab_size: u8, insert_spaces: bool) -> Result<String, JsonError> {
    let lexed = lex(text)?;
    if lexed
        .tokens
        .iter()
        .all(|token| matches!(token.kind, Kind::LineComment | Kind::BlockComment))
    {
        return Ok(text.to_string());
    }
    let unit = if insert_spaces {
        " ".repeat(usize::from(tab_size))
    } else {
        "\t".to_string()
    };
    // The document's own line ending wins; "\n" only when it never says "\r\n". Block-comment
    // interiors are emitted verbatim and may disagree — verbatim wins there, by the invariant.
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    print(text, &lexed, &unit, eol)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    OpenBrace,
    CloseBrace,
    OpenBracket,
    CloseBracket,
    Comma,
    Colon,
    Str,
    Scalar,
    LineComment,
    BlockComment,
}

#[derive(Debug, Clone, Copy)]
struct Token<'a> {
    kind: Kind,
    /// The token's source text, emitted verbatim — except a line comment, whose trailing
    /// whitespace the lexer trims so the output never carries invisible cargo (and so the
    /// invariant's lex-equality is exact rather than modulo-trailing-space).
    text: &'a str,
    line: usize,
    column: usize,
    /// Newlines in the whitespace gap before this token (for the first token, in the leading
    /// whitespace). Zero means "same line as whatever came before", which is what the printer
    /// reads as "this comment was trailing" and "keep at most one blank line" reads counts
    /// from.
    newlines_before: usize,
}

struct Lexed<'a> {
    /// A leading U+FEFF, remembered and re-emitted first — stripping it would change bytes a
    /// Windows-authored file deliberately carries.
    bom: bool,
    tokens: Vec<Token<'a>>,
    eof_line: usize,
    eof_column: usize,
}

fn lex(text: &str) -> Result<Lexed<'_>, JsonError> {
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let mut tokens = Vec::new();
    let mut chars = body.char_indices().peekable();
    let mut line = 1usize;
    let mut column = 1usize;
    let mut newlines = 0usize;

    while let Some(&(start, c)) = chars.peek() {
        match c {
            ' ' | '\t' | '\r' => {
                chars.next();
                column += 1;
            }
            '\n' => {
                chars.next();
                line += 1;
                column = 1;
                newlines += 1;
            }
            '{' | '}' | '[' | ']' | ',' | ':' => {
                let kind = match c {
                    '{' => Kind::OpenBrace,
                    '}' => Kind::CloseBrace,
                    '[' => Kind::OpenBracket,
                    ']' => Kind::CloseBracket,
                    ',' => Kind::Comma,
                    _ => Kind::Colon,
                };
                tokens.push(Token {
                    kind,
                    text: &body[start..=start],
                    line,
                    column,
                    newlines_before: newlines,
                });
                newlines = 0;
                chars.next();
                column += 1;
            }
            '"' => {
                let (open_line, open_column) = (line, column);
                chars.next();
                column += 1;
                let mut end = None;
                while let Some((i, d)) = chars.next() {
                    column += 1;
                    match d {
                        // A backslash escapes exactly the next character, whatever it is —
                        // escape *validity* is not this module's business (bytes go out
                        // verbatim), only where the string ends.
                        '\\' => {
                            if let Some((_, escaped)) = chars.next() {
                                column += 1;
                                if escaped == '\n' {
                                    line += 1;
                                    column = 1;
                                }
                            }
                        }
                        '"' => {
                            end = Some(i + d.len_utf8());
                            break;
                        }
                        // A raw newline before the closing quote is almost always a missing
                        // quote, and the *opening* quote is where a person can fix it — the
                        // newline itself is just where the damage became visible.
                        '\n' | '\r' => {
                            return Err(JsonError {
                                line: open_line,
                                column: open_column,
                                what: "the string opened here is not closed on its line",
                            });
                        }
                        _ => {}
                    }
                }
                let Some(end) = end else {
                    return Err(JsonError {
                        line: open_line,
                        column: open_column,
                        what: "the string opened here is not closed on its line",
                    });
                };
                tokens.push(Token {
                    kind: Kind::Str,
                    text: &body[start..end],
                    line: open_line,
                    column: open_column,
                    newlines_before: newlines,
                });
                newlines = 0;
            }
            '/' => {
                let (open_line, open_column) = (line, column);
                chars.next();
                column += 1;
                match chars.peek() {
                    Some(&(_, '/')) => {
                        chars.next();
                        column += 1;
                        let mut end = body.len();
                        while let Some(&(i, d)) = chars.peek() {
                            if d == '\n' {
                                end = i;
                                break;
                            }
                            chars.next();
                            column += 1;
                        }
                        tokens.push(Token {
                            kind: Kind::LineComment,
                            text: body[start..end].trim_end(),
                            line: open_line,
                            column: open_column,
                            newlines_before: newlines,
                        });
                        newlines = 0;
                    }
                    Some(&(_, '*')) => {
                        chars.next();
                        column += 1;
                        let mut end = None;
                        let mut starred = false;
                        for (i, d) in chars.by_ref() {
                            if d == '\n' {
                                line += 1;
                                column = 1;
                            } else {
                                column += 1;
                            }
                            if starred && d == '/' {
                                end = Some(i + 1);
                                break;
                            }
                            starred = d == '*';
                        }
                        let Some(end) = end else {
                            return Err(JsonError {
                                line: open_line,
                                column: open_column,
                                what: "the block comment opened here is never closed",
                            });
                        };
                        tokens.push(Token {
                            kind: Kind::BlockComment,
                            text: &body[start..end],
                            line: open_line,
                            column: open_column,
                            newlines_before: newlines,
                        });
                        newlines = 0;
                    }
                    _ => {
                        return Err(JsonError {
                            line: open_line,
                            column: open_column,
                            what: "unexpected '/'",
                        });
                    }
                }
            }
            _ => {
                // A scalar is a maximal run of anything that is not whitespace, structure, a
                // quote or a slash: numbers, `true`/`false`/`null`, and bare words. Spelling
                // is deliberately unvalidated — see the module header.
                let mut end = body.len();
                while let Some(&(i, d)) = chars.peek() {
                    if d.is_whitespace()
                        || matches!(d, '{' | '}' | '[' | ']' | ',' | ':' | '"' | '/')
                    {
                        end = i;
                        break;
                    }
                    chars.next();
                    column += 1;
                }
                tokens.push(Token {
                    kind: Kind::Scalar,
                    text: &body[start..end],
                    line,
                    column: column - body[start..end].chars().count(),
                    newlines_before: newlines,
                });
                newlines = 0;
            }
        }
    }

    Ok(Lexed {
        bom,
        tokens,
        eof_line: line,
        eof_column: column,
    })
}

/// What the grammar expects next. Comments are legal in every state and change none of them.
#[derive(Debug, Clone, Copy)]
enum State {
    /// A value must follow: document start, or after `:`. A closer is *not* legal here —
    /// `continuation` remembers the after-`:` case, whose line the printer indents one unit
    /// past the member's own when a comment has broken it.
    Value { continuation: bool },
    /// After `[` or after `,` inside an array: a value, or `]` — which is what makes an empty
    /// array and a trailing comma both legal without a special case.
    ElementOrClose,
    /// After `{` or after `,` inside an object: a key, or `}`.
    KeyOrClose,
    /// After a key.
    Colon,
    /// After a completed value inside a container: `,` or the matching closer.
    AfterValue,
    /// After the document's one value. Only comments may follow: a second value is refused,
    /// which is also the guard that keeps a JSON-Lines file from being "formatted" into one
    /// giant broken document.
    End,
}

struct Open {
    bracket: char,
    /// The indent depth of the line this container's opener sits on. Members print at
    /// `base + 1` and the closer at `base` — kept explicitly rather than derived from stack
    /// height, because a continuation line (a value pushed off its member's line by a
    /// comment) opens containers one unit deeper than the stack alone would say.
    base: usize,
    line: usize,
    column: usize,
}

enum Sep {
    None,
    Space,
    Newline,
}

#[allow(clippy::too_many_lines)]
fn print(text: &str, lexed: &Lexed<'_>, unit: &str, eol: &str) -> Result<String, JsonError> {
    let mut out = String::with_capacity(text.len() + text.len() / 4);
    if lexed.bom {
        out.push('\u{feff}');
    }
    let mut stack: Vec<Open> = Vec::new();
    let mut state = State::Value {
        continuation: false,
    };
    // Depth of the output line currently being written — the value an opener records as its
    // `base`, whether it started the line or joined one.
    let mut line_depth = 0usize;
    // Depth of the line the current member's key sits on. Continuation lines anchor here
    // rather than at `line_depth`, or a run of comments between `:` and the value would drift
    // one unit deeper per line.
    let mut member_line = 0usize;
    let mut prev: Option<&Token<'_>> = None;

    for token in &lexed.tokens {
        let refuse = |what: &'static str| JsonError {
            line: token.line,
            column: token.column,
            what,
        };
        let comment = matches!(token.kind, Kind::LineComment | Kind::BlockComment);

        if !comment {
            match state {
                State::Value { .. } => match token.kind {
                    Kind::OpenBrace | Kind::OpenBracket | Kind::Str | Kind::Scalar => {}
                    _ => return Err(refuse("expected a value")),
                },
                State::ElementOrClose => match token.kind {
                    Kind::OpenBrace
                    | Kind::OpenBracket
                    | Kind::Str
                    | Kind::Scalar
                    | Kind::CloseBracket => {}
                    _ => return Err(refuse("expected a value or ']'")),
                },
                State::KeyOrClose => match token.kind {
                    Kind::Str | Kind::Scalar | Kind::CloseBrace => {}
                    _ => return Err(refuse("expected a key or '}'")),
                },
                State::Colon => match token.kind {
                    Kind::Colon => {}
                    _ => return Err(refuse("expected ':' after the key")),
                },
                State::AfterValue => {
                    let open = stack
                        .last()
                        .expect("AfterValue only exists inside a container")
                        .bracket;
                    match token.kind {
                        Kind::Comma => {}
                        Kind::CloseBrace if open == '{' => {}
                        Kind::CloseBracket if open == '[' => {}
                        _ => {
                            return Err(refuse(if open == '{' {
                                "expected ',' or '}'"
                            } else {
                                "expected ',' or ']'"
                            }));
                        }
                    }
                }
                State::End => return Err(refuse("a second value after the document's value")),
            }
        }

        let closer = matches!(token.kind, Kind::CloseBrace | Kind::CloseBracket);
        let continuation = matches!(state, State::Colon | State::Value { continuation: true });
        // Where this token's line indents, should it start one: a closer to its container's
        // base, a continuation one past its member's line, everything else to the member
        // depth of the innermost container (the document's value at zero).
        let target = if closer {
            stack
                .last()
                .expect("a validated closer has a container")
                .base
        } else if continuation {
            member_line + 1
        } else {
            stack.last().map_or(0, |open| open.base + 1)
        };

        let sep = match prev {
            None => Sep::None,
            // Nothing may share a line after a line comment — anything appended would become
            // part of the comment's text on the next lex. Checked before the attach rules, or
            // a comma would be swallowed.
            Some(p) if p.kind == Kind::LineComment => Sep::Newline,
            Some(p) => match token.kind {
                // `,` and `:` attach to whatever token preceded them, in token order — which
                // is what keeps `1 /* c */,` shaped like itself.
                Kind::Comma | Kind::Colon => Sep::None,
                Kind::CloseBrace if p.kind == Kind::OpenBrace => Sep::None,
                Kind::CloseBracket if p.kind == Kind::OpenBracket => Sep::None,
                // A comment that shared its line in the input stays trailing; one that had
                // its own line keeps it.
                Kind::LineComment | Kind::BlockComment => {
                    if token.newlines_before == 0 {
                        Sep::Space
                    } else {
                        Sep::Newline
                    }
                }
                _ if closer => Sep::Newline,
                _ if p.kind == Kind::Colon => Sep::Space,
                _ if p.kind == Kind::BlockComment && token.newlines_before == 0 => Sep::Space,
                _ => Sep::Newline,
            },
        };

        match sep {
            Sep::None => {}
            Sep::Space => out.push(' '),
            Sep::Newline => {
                // One blank line survives where the author left one or more — people group
                // tsconfig entries with them — but never right after an opener or before a
                // closer, where a blank is only ever leftover flab.
                let blank = token.newlines_before >= 2
                    && !matches!(
                        prev.map(|p| p.kind),
                        Some(Kind::OpenBrace | Kind::OpenBracket)
                    )
                    && !closer;
                if blank {
                    out.push_str(eol);
                }
                out.push_str(eol);
                for _ in 0..target {
                    out.push_str(unit);
                }
                line_depth = target;
            }
        }
        out.push_str(token.text);
        if out.len() > MAX_OUTPUT_BYTES {
            return Err(refuse("the formatted text would pass 16 MiB"));
        }

        if !comment {
            match token.kind {
                Kind::OpenBrace => {
                    stack.push(Open {
                        bracket: '{',
                        base: line_depth,
                        line: token.line,
                        column: token.column,
                    });
                    state = State::KeyOrClose;
                }
                Kind::OpenBracket => {
                    stack.push(Open {
                        bracket: '[',
                        base: line_depth,
                        line: token.line,
                        column: token.column,
                    });
                    state = State::ElementOrClose;
                }
                Kind::CloseBrace | Kind::CloseBracket => {
                    stack.pop();
                    state = if stack.is_empty() {
                        State::End
                    } else {
                        State::AfterValue
                    };
                }
                Kind::Comma => {
                    state = if stack
                        .last()
                        .expect("validated comma has a container")
                        .bracket
                        == '{'
                    {
                        State::KeyOrClose
                    } else {
                        State::ElementOrClose
                    };
                }
                Kind::Colon => {
                    state = State::Value { continuation: true };
                }
                Kind::Str | Kind::Scalar => match state {
                    State::KeyOrClose => {
                        member_line = line_depth;
                        state = State::Colon;
                    }
                    _ => {
                        state = if stack.is_empty() {
                            State::End
                        } else {
                            State::AfterValue
                        };
                    }
                },
                Kind::LineComment | Kind::BlockComment => unreachable!("comments handled above"),
            }
        }
        prev = Some(token);
    }

    match state {
        State::End => {}
        State::Colon => {
            return Err(JsonError {
                line: lexed.eof_line,
                column: lexed.eof_column,
                what: "expected ':' after the key",
            });
        }
        State::Value { .. } => {
            return Err(JsonError {
                line: lexed.eof_line,
                column: lexed.eof_column,
                what: "expected a value",
            });
        }
        // ElementOrClose, KeyOrClose, AfterValue: something is still open, and the opener is
        // the position a person can act on — in a large file "never closed at line 3" beats
        // "expected ']' at line 4000".
        State::ElementOrClose | State::KeyOrClose | State::AfterValue => {
            let open = stack
                .last()
                .expect("a non-final state at EOF has an open container");
            return Err(JsonError {
                line: open.line,
                column: open.column,
                what: if open.bracket == '{' {
                    "the '{' here is never closed"
                } else {
                    "the '[' here is never closed"
                },
            });
        }
    }

    // The input's own choice about a final newline is kept, both ways — cide has no live
    // `insertFinalNewline` setting, and imposing one here would make an inert preference look
    // wired (the same argument `cmd::format::options` makes for not sending it to a server).
    if text.ends_with('\n') {
        out.push_str(eol);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Valid inputs the two whole-corpus invariants run over. Grow it with every shape a
    /// regression teaches us about — both invariant tests pick it up automatically.
    const CORPUS: &[&str] = &[
        "{\"a\":1,\"b\":[true,false,null]}",
        "{\n  \"a\": 1\n}\n",
        "// header\n{\n  \"a\": 1, // trailing\n  /* own */\n  \"b\": [1, 2, 3],\n}\n",
        "[\n\n1,\n\n\n2\n]",
        "{\"a\":{},\"b\":[],\"c\":{\"d\":[[]]}}",
        "\u{feff}{\"a\":1}",
        "{\r\n\"a\": 1\r\n}\r\n",
        "5",
        "\"top\"",
        "/* only a comment */",
        "",
        "{\"dup\":1,\"dup\":2}",
        "{\"n\":9007199254740993,\"e\":1e310,\"f\":0.30000000000000004}",
        "{\"a\": /* inline */ 1}",
        "{\"a\": // why\n1}",
        "{\"a\" /* between */: 1}",
        "[1,2,3,]",
        "{bare: 1}",
        "{ // on the brace\n\"a\": [ { \"b\": \"c\\\"d\" } ]\n}",
        "{\n  /* one\n     two */\n  \"a\": 1\n}\n",
    ];

    fn spelled(text: &str) -> Vec<(Kind, String)> {
        lex(text)
            .expect("corpus entries lex")
            .tokens
            .iter()
            .map(|token| (token.kind, token.text.to_string()))
            .collect()
    }

    #[test]
    fn no_token_is_added_or_removed() {
        // THE ONE THAT MATTERS. The whole reason this module is a reprinter and not a
        // serde round-trip: comments, duplicate keys, number spellings, escapes and trailing
        // commas survive because the token stream is identical on both sides.
        for input in CORPUS {
            let output = reformat(input, 2, true).expect("corpus entries format");
            assert_eq!(
                spelled(&output),
                spelled(input),
                "tokens drifted for {input:?} -> {output:?}"
            );
        }
    }

    #[test]
    fn formatting_twice_is_formatting_once() {
        for input in CORPUS {
            for (tab, spaces) in [(2u8, true), (4, true), (4, false)] {
                let once = reformat(input, tab, spaces).expect("corpus entries format");
                let twice = reformat(&once, tab, spaces).expect("formatted output reformats");
                assert_eq!(twice, once, "not idempotent for {input:?}");
            }
        }
    }

    #[test]
    fn minified_input_expands_one_entry_per_line() {
        assert_eq!(
            reformat("{\"a\":1,\"b\":[true,null]}", 2, true).unwrap(),
            "{\n  \"a\": 1,\n  \"b\": [\n    true,\n    null\n  ]\n}"
        );
    }

    #[test]
    fn an_already_formatted_document_is_returned_byte_identical() {
        // What the road above maps to `Unchanged`: a reflexive Shift+Alt+F must not dirty
        // the tab.
        let formatted = "{\n  \"a\": 1,\n  \"b\": [\n    true,\n    null\n  ]\n}\n";
        assert_eq!(reformat(formatted, 2, true).unwrap(), formatted);
    }

    #[test]
    fn a_trailing_line_comment_stays_trailing() {
        let input = "{\n  \"a\": 1, // one\n  \"b\": 2\n}\n";
        assert_eq!(reformat(input, 2, true).unwrap(), input);
    }

    #[test]
    fn an_own_line_comment_stays_own_line_at_the_new_indent() {
        assert_eq!(
            reformat("{\"a\":1,\n// note\n\"b\":2}", 2, true).unwrap(),
            "{\n  \"a\": 1,\n  // note\n  \"b\": 2\n}"
        );
    }

    #[test]
    fn a_comment_on_the_opening_brace_line_stays_there() {
        assert_eq!(
            reformat("{ // hi\n\"a\":1}", 2, true).unwrap(),
            "{ // hi\n  \"a\": 1\n}"
        );
    }

    #[test]
    fn a_block_comment_interior_is_not_reindented() {
        // Aligned `*` columns and ASCII art survive; being misaligned against a *new* indent
        // is the lesser harm than being rewritten.
        let input = "{\n  /* one\n     two */\n  \"a\": 1\n}\n";
        assert_eq!(reformat(input, 2, true).unwrap(), input);
    }

    #[test]
    fn a_comment_only_file_is_left_alone() {
        for input in ["", "   \n\n", "// nothing here yet\n", "/* draft */"] {
            assert_eq!(reformat(input, 2, true).unwrap(), input);
        }
    }

    #[test]
    fn a_value_pushed_off_its_line_by_a_comment_indents_one_deeper() {
        // The continuation rule, and the anchor that keeps a run of comments from drifting
        // one unit deeper per line.
        assert_eq!(
            reformat("{\"a\": // why\n// more\n1}", 2, true).unwrap(),
            "{\n  \"a\": // why\n    // more\n    1\n}"
        );
    }

    #[test]
    fn number_and_string_bytes_are_never_reinterpreted() {
        let out = reformat(
            "[1e310,9007199254740993,0.30000000000000004,\"caf\\u00e9\"]",
            2,
            true,
        )
        .unwrap();
        for spelling in [
            "1e310",
            "9007199254740993",
            "0.30000000000000004",
            "\"caf\\u00e9\"",
        ] {
            assert!(out.contains(spelling), "{spelling} lost in {out}");
        }
    }

    #[test]
    fn duplicate_keys_survive() {
        let out = reformat("{\"dup\":1,\"dup\":2}", 2, true).unwrap();
        assert_eq!(out.matches("\"dup\"").count(), 2, "{out}");
    }

    #[test]
    fn a_trailing_comma_is_preserved_not_repaired() {
        // Repair would turn a strict parser's error into a success — a behaviour change, not
        // a formatting one.
        assert_eq!(reformat("[1,2,]", 2, true).unwrap(), "[\n  1,\n  2,\n]");
    }

    #[test]
    fn a_bare_key_is_tolerated() {
        assert_eq!(reformat("{a:1}", 2, true).unwrap(), "{\n  a: 1\n}");
    }

    #[test]
    fn empty_containers_stay_compact() {
        assert_eq!(
            reformat("{\"a\":{},\"b\":[]}", 2, true).unwrap(),
            "{\n  \"a\": {},\n  \"b\": []\n}"
        );
    }

    #[test]
    fn one_blank_line_survives_between_groups_and_runs_collapse() {
        assert_eq!(
            reformat("{\"a\":1,\n\n\n\n\"b\":2}", 2, true).unwrap(),
            "{\n  \"a\": 1,\n\n  \"b\": 2\n}"
        );
        // But never right after the opener or before the closer, where a blank is only flab.
        assert_eq!(
            reformat("{\n\n\"a\":1\n\n}", 2, true).unwrap(),
            "{\n  \"a\": 1\n}"
        );
    }

    #[test]
    fn crlf_input_stays_crlf() {
        assert_eq!(
            reformat("{\"a\":1}\r\n", 2, true).unwrap(),
            "{\r\n  \"a\": 1\r\n}\r\n"
        );
    }

    #[test]
    fn final_newline_presence_and_absence_both_survive() {
        assert_eq!(reformat("[1]", 2, true).unwrap(), "[\n  1\n]");
        assert_eq!(reformat("[1]\n", 2, true).unwrap(), "[\n  1\n]\n");
    }

    #[test]
    fn a_byte_order_mark_survives() {
        let out = reformat("\u{feff}{\"a\":1}", 2, true).unwrap();
        assert!(out.starts_with('\u{feff}'), "{out:?}");
    }

    #[test]
    fn tabs_are_used_when_insert_spaces_is_off() {
        assert_eq!(
            reformat("{\"a\":[1]}", 4, false).unwrap(),
            "{\n\t\"a\": [\n\t\t1\n\t]\n}"
        );
    }

    #[test]
    fn tab_size_is_honoured() {
        assert_eq!(
            reformat("{\"a\":1}", 4, true).unwrap(),
            "{\n    \"a\": 1\n}"
        );
    }

    #[test]
    fn a_missing_comma_is_refused_where_it_is_missing() {
        let error = reformat("{\"a\":1 \"b\":2}", 2, true).unwrap_err();
        assert_eq!(
            (error.line, error.column, error.what),
            (1, 8, "expected ',' or '}'")
        );
    }

    #[test]
    fn an_unterminated_string_is_refused_at_its_opening_quote() {
        // The newline is where the damage shows; the opening quote is where the fix goes.
        let error = reformat("{\"a\": \"oops\n}", 2, true).unwrap_err();
        assert_eq!((error.line, error.column), (1, 7));
        assert!(error.what.contains("not closed"), "{}", error.what);
    }

    #[test]
    fn an_unterminated_block_comment_is_refused() {
        let error = reformat("{\"a\": 1 /* hmm", 2, true).unwrap_err();
        assert_eq!((error.line, error.column), (1, 9));
        assert!(error.what.contains("never closed"), "{}", error.what);
    }

    #[test]
    fn mismatched_brackets_are_refused() {
        let error = reformat("{\"a\": [1}", 2, true).unwrap_err();
        assert_eq!(
            (error.line, error.column, error.what),
            (1, 9, "expected ',' or ']'")
        );
    }

    #[test]
    fn an_unclosed_container_is_refused_at_its_opener() {
        let error = reformat("{\"a\": {\"b\": 1\n", 2, true).unwrap_err();
        assert_eq!(
            (error.line, error.column, error.what),
            (1, 7, "the '{' here is never closed")
        );
    }

    #[test]
    fn a_second_top_level_value_is_refused() {
        // Also the JSON-Lines guard: a .json file holding one record per line must refuse,
        // not be reflowed into one giant broken document.
        let error = reformat("{\"a\":1}\n{\"a\":2}\n", 2, true).unwrap_err();
        assert_eq!((error.line, error.column), (2, 1), "{}", error.what);
    }

    #[test]
    fn deep_nesting_does_not_overflow_the_stack() {
        // Pins the iterative requirement: half a million levels is representable inside the
        // 1 MiB input cap, and a recursive printer dies here with no Err in sight. The
        // *answer* is a refusal (the output cap trips long before the brackets run out) —
        // what the test asserts is that there is an answer.
        let input = "[".repeat(500_000);
        assert!(reformat(&input, 2, true).is_err());
        let balanced = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
        assert!(reformat(&balanced, 2, true).is_err());
    }
}
