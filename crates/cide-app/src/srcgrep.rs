//! Comment stripping for the source assertions in this crate's tests.
//!
//! Several gates here are structural rather than behavioural — "`lib.rs` routes both
//! directions of `WindowEvent::Focused`", "the run loop handles `RunEvent::Exit`" — because
//! the thing being asserted is a *wiring* decision inside a closure that no test can call.
//! `windows.rs` explains at length why those are the honest gates for their subjects.
//!
//! A source assertion over raw text is only as good as its blindness to comments. A negative
//! assertion (`!source.contains(…)`) that reads the file whole matches the paragraph
//! explaining why the forbidden thing is forbidden, so it can never fail; a positive one goes
//! green on a line that was commented out. Both have happened in this repository. So every
//! source assertion in this crate runs over [`without_comments`] first, and each of them
//! additionally proves it fails with the feature removed.
//!
//! Test-only, and deliberately not a Rust lexer: it handles block comments, line comments,
//! and enough string-literal awareness that a `"//"` inside a string does not eat the rest of
//! the line. That is the whole of what these callers need — they run over files in this crate.

/// `source` with `//` and `/* */` comments removed.
///
/// Nested block comments are handled (Rust allows them and this crate uses them), and a `"`
/// opens a string literal in which `//` and `/*` are ordinary characters. Raw strings
/// (`r#"…"#`) are not modelled: none of the callers' files contains one whose body could be
/// mistaken for a comment, and adding the state machine for it would be code with no gate.
pub fn without_comments(source: &str) -> String {
    let bytes: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    let mut in_string = false;
    let mut depth = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        let next = bytes.get(i + 1).copied();
        if depth > 0 {
            if c == '*' && next == Some('/') {
                depth -= 1;
                i += 2;
                continue;
            }
            if c == '/' && next == Some('*') {
                depth += 1;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if in_string {
            if c == '\\' {
                i += 2;
                continue;
            }
            if c == '"' {
                in_string = false;
            }
            out.push(c);
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '/' && next == Some('/') {
            while i < bytes.len() && bytes[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && next == Some('*') {
            depth = 1;
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::without_comments;

    /// The stripper's own gate. Every caller's assertion is only as trustworthy as this, and
    /// the three cases below are the three that have actually caused a wrong verdict here.
    #[test]
    fn comments_are_stripped_and_strings_are_not() {
        assert_eq!(without_comments("a // b\nc"), "a \nc");
        assert_eq!(without_comments("a /* b */ c"), "a  c");
        assert_eq!(without_comments("a /* b /* c */ d */ e"), "a  e");
        // The trap that makes a negative source assertion useless: the word it forbids, in
        // the paragraph that says why it is forbidden.
        assert_eq!(without_comments("// RunEvent::Exit\nx"), "\nx");
        // …and the trap in the other direction: a string literal is code, not commentary, so
        // a failure message naming the forbidden token still counts. Callers that assert on a
        // token which also appears in their own failure message must split the test module
        // off first — `windows.rs` records that mistake.
        assert_eq!(
            without_comments(r#"let s = "a // b";"#),
            r#"let s = "a // b";"#
        );
        assert_eq!(
            without_comments(r#"let s = "a /* b";"#),
            r#"let s = "a /* b";"#
        );
    }
}
