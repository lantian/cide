//! Finds `@role` mentions in task prose.
//!
//! The grammar is [`crate::defs::valid_name`]'s, and that is why this module lives here rather
//! than in `cide-tasks`, which owns the prose being scanned: the id alphabet is this crate's
//! rule (it doubles as a path- and branch-safety rule), `cide-tasks` does not depend on
//! `cide-agents`, and a second copy of the alphabet would be the drift.
//!
//! Pure and I/O-free on purpose — the caller (`cide-app`'s task triggers) decides which of the
//! returned names are real roles by consulting a freshly loaded catalog; this function has no
//! idea what exists and must not, or every parse would cost a `read_dir`.
//!
//! Deliberately **no markdown awareness**: a mention inside a code fence or inline code span
//! matches. The cost is a spurious dispatch the user can stop and an assignment they can undo;
//! the alternative is a markdown parser in a domain crate, growing towards every renderer's
//! disagreements about nesting. The comment log renders as plain text anyway.

use cide_ipc::AgentId;

use crate::defs;

/// Every `@role` in `text` whose name passes [`defs::valid_name`], first-seen order, deduplicated.
///
/// An `@` opens a mention only at the start of the text or after a non-alphanumeric character —
/// which is what keeps `user@example.com` from mentioning a role named `example` (the `r` before
/// the `@` closes it), while `(@developer)` and a leading `@developer` both open. The token is
/// the maximal run of `[a-z0-9-]` after the `@`, and it must *end at a word boundary* too:
/// `@devFoo` and `@qa_check` are somebody's identifier, not a mention of `dev` or `qa`, so a
/// token whose run stops against `[A-Za-z0-9_]` is dropped whole rather than truncated into a
/// role name nobody wrote. What survives both cuts still has to pass [`defs::valid_name`].
pub fn mentions_in(text: &str) -> Vec<AgentId> {
    let mut found: Vec<AgentId> = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'@' {
            i += 1;
            continue;
        }
        let opens = match i.checked_sub(1).map(|p| bytes[p]) {
            None => true,
            // Bytes, not chars: an ASCII alphanumeric is always its own byte, and any byte of a
            // multi-byte character is non-ASCII, so this closes after `e` in `name@qa` without
            // decoding. A non-ASCII *letter* before the `@` (`café@qa`) opens — role ids are
            // ASCII, mentions of them will be typed after ASCII punctuation or whitespace in
            // practice, and erring towards opening costs a mention the user can see and undo
            // where erring towards closing costs one they cannot find.
            Some(prev) => !prev.is_ascii_alphanumeric(),
        };
        let start = i + 1;
        let mut end = start;
        while end < bytes.len()
            && (bytes[end].is_ascii_lowercase()
                || bytes[end].is_ascii_digit()
                || bytes[end] == b'-')
        {
            end += 1;
        }
        i = end.max(i + 1);
        if !opens || end == start {
            continue;
        }
        // The word-boundary cut from the doc above: a run stopped by a word character means the
        // author was writing `@devFoo`, not `@dev` with `Foo` after it.
        if bytes
            .get(end)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        {
            continue;
        }
        let name = &text[start..end];
        if defs::valid_name(name) && !found.iter().any(|id| id.0 == name) {
            found.push(AgentId(name.to_string()));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str) -> Vec<String> {
        mentions_in(text).into_iter().map(|id| id.0).collect()
    }

    #[test]
    fn the_mention_grammar_is_the_role_name_grammar() {
        // Plain, punctuation-adjacent, and leading — all open.
        assert_eq!(names("ask @developer to look"), ["developer"]);
        assert_eq!(names("(@developer)"), ["developer"]);
        assert_eq!(names("@developer, then @qa."), ["developer", "qa"]);
        assert_eq!(names("@code-reviewer first"), ["code-reviewer"]);

        // An email address is prose, not a mention: the `r` before the `@` closes it.
        assert_eq!(
            names("mail user@example.com about it"),
            Vec::<String>::new()
        );

        // Uppercase is not in the id alphabet, so `@Developer` scans as an empty token followed
        // by prose; `@dev-Foo` and `@qa_check` stop against a word character and are dropped
        // whole rather than truncated into `dev-` or `qa`.
        assert_eq!(names("@Developer @dev-Foo @qa_check"), Vec::<String>::new());

        // A bare `@`, and a name over the 32-char cap, are dropped by `valid_name`.
        assert_eq!(names("an @ alone"), Vec::<String>::new());
        assert_eq!(names(&format!("@{}", "a".repeat(33))), Vec::<String>::new());

        // First-seen order, deduplicated.
        assert_eq!(
            names("@qa then @developer then @qa again"),
            ["qa", "developer"]
        );

        // Multi-byte text before an `@` neither panics nor opens differently: `…` is not
        // alphanumeric, so the mention stands.
        assert_eq!(names("wait…@qa"), ["qa"]);
        assert_eq!(names("naïve@qa"), Vec::<String>::new());
    }
}
