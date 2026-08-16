//! Speed search: the one rule that decides whether a row's *name* matches what is being
//! typed into a sidebar tree, and where the highlight goes. (M15)
//!
//! # Why the rule is here, in Rust, rather than in the pure TypeScript module beside it
//!
//! The two sidebar trees disagree about almost everything structural — the explorer addresses
//! rows by integer index into a flattening Rust owns and holds only 200-row chunks of it, the
//! changes tree holds a complete `Row[]` addressed by string id — so there was never a shared
//! *data* layer to hang this on. What they do share is the question, and this project has
//! already paid twice for answering one question in two places.
//!
//! Two shapes were available, both with a precedent in this repository:
//!
//!   * `overlays/score.ts` — a line-for-line TypeScript port of `cide_core::commands::score`,
//!     kept in step by a check script asserting the same tier boundaries the Rust tests do.
//!   * `cmd::picker::picker_rank` — *one* Rust rule behind two commands, because
//!     "two overlays that rank the same query differently reads as a bug even when both
//!     answers are defensible on their own".
//!
//! This takes the second, and one fact decided it. The explorer's rows physically cannot be
//! matched in the webview: a match may be at row 40,000 of a 100k-row flattening with rows
//! 300–500 resident, so *something* in Rust has to walk the tree — and the moment that exists,
//! a TypeScript copy for the changes tree is a second answer to a question that already has
//! one. The changes tree therefore pays one IPC round trip per keystroke over a list of at
//! most a few thousand strings, which is exactly the price `picker_query` already ships with.
//!
//! # Why the spans are UTF-16 code units
//!
//! Because the only consumer is JavaScript, and `String.prototype.slice` indexes UTF-16 units.
//! A byte offset would agree for ASCII and diverge for everything else — one emoji before the
//! match puts the highlight four characters early — which is the same silent, encoding-shaped
//! bug `SearchModel::splitHighlight` had to grow a whole decode path for. Producing the
//! consumer's unit here means the frontend never converts and therefore can never convert
//! wrongly.
//!
//! # The matching rule itself
//!
//! **Case-insensitive substring, first occurrence, against the row's name only.** Not the
//! path: a query typed into a tree is about what is on the row, and matching the path would
//! light up every file in `crates/cide-fs/` for the query `fs`.
//!
//! Substring rather than IDEA's prefix-with-a-`*`-escape, which is two modes and therefore two
//! rules; substring subsumes prefix and is what makes this repository navigable (`tree` has to
//! find `FileTree.tsx`). And deliberately **not** fuzzy: the file picker (Ctrl+P) is the fuzzy
//! surface, and a tree whose Down key walked a relevance order rather than tree order would
//! stop meaning "the next one down the list", which is the entire gesture.
//!
//! Nothing here ranks. Match order is walk order, in both trees.

use cide_ipc::TreeMatch;

/// A prepared query.
///
/// Prepared once per keystroke rather than per row, which is what keeps a 100k-row walk down
/// to one `str` scan per name with no allocation at all on the ASCII path — and every filename
/// on the machines this runs on is ASCII. `Index::match_rows` calls [`Needle::find`] once per
/// visible row, so anything allocated in there is allocated a hundred thousand times.
pub struct Needle {
    /// The query lowercased as bytes, when it is ASCII. The fast path.
    ascii: Option<Vec<u8>>,
    /// The query as case-folded `char`s. The general path.
    folded: Vec<char>,
}

impl Needle {
    /// `None` for an empty query — an empty needle matches every row, which as a *speed search*
    /// answer means "everything is a match", i.e. nothing useful. The caller reads `None` as
    /// "there is no search", which is also the state after the last Backspace.
    #[must_use]
    pub fn new(query: &str) -> Option<Self> {
        if query.is_empty() {
            return None;
        }
        Some(Self {
            ascii: query
                .is_ascii()
                .then(|| query.as_bytes().to_ascii_lowercase()),
            folded: fold(query),
        })
    }

    /// Where this needle first occurs in `label`, in **UTF-16 code units**.
    #[must_use]
    pub fn find(&self, label: &str) -> Option<(u32, u32)> {
        // The fast path, and it is the one every row of a real project takes. For ASCII, one
        // byte is one UTF-16 unit, so the byte offset *is* the answer and no table is built.
        if let (Some(needle), true) = (&self.ascii, label.is_ascii()) {
            let hay = label.as_bytes();
            if needle.len() > hay.len() {
                return None;
            }
            for start in 0..=(hay.len() - needle.len()) {
                if hay[start..start + needle.len()].eq_ignore_ascii_case(needle) {
                    return Some((start as u32, (start + needle.len()) as u32));
                }
            }
            return None;
        }

        // The general path. A parallel table of (folded char, UTF-16 offset), because the
        // folded form of a string is not the same length as the string — `İ` lowercases to two
        // scalars — so an offset taken in the folded string does not name a position in the
        // original. Building the table is what makes the answer an offset into what the user
        // can actually see.
        let mut hay: Vec<(char, u32)> = Vec::with_capacity(label.len());
        let mut at = 0u32;
        for ch in label.chars() {
            hay.push((lower(ch), at));
            at += ch.len_utf16() as u32;
        }
        let end_of_label = at;
        if self.folded.len() > hay.len() {
            return None;
        }
        for start in 0..=(hay.len() - self.folded.len()) {
            if (0..self.folded.len()).all(|i| hay[start + i].0 == self.folded[i]) {
                let from = hay[start].1;
                let to = hay
                    .get(start + self.folded.len())
                    .map_or(end_of_label, |&(_, offset)| offset);
                return Some((from, to));
            }
        }
        None
    }
}

/// One scalar's lowercase form, or the scalar itself.
///
/// `char::to_lowercase` yields an *iterator* because a handful of scalars lowercase to several
/// — `İ` (U+0130) is the well-known one. Taking the first is a deliberate simplification: it is
/// exact for ASCII, Latin-1, Greek and Cyrillic, i.e. for every filename anyone has ever typed
/// into this tree, and where it is inexact it fails by *not matching* rather than by producing
/// a span that does not describe the label. A wrong span is a highlight over the wrong glyphs;
/// a missed match is a row the user scrolls to instead. Only one of those is a lie.
fn lower(ch: char) -> char {
    ch.to_lowercase().next().unwrap_or(ch)
}

fn fold(text: &str) -> Vec<char> {
    text.chars().map(lower).collect()
}

/// Push `label`'s match, if any, and say whether the caller must stop.
///
/// `true` means "the limit is full **and** there was more to find", which is the only honest
/// reading of the truncation flag the overlay prints as `first 1000 matches`. Returning `true`
/// merely because the vector reached `limit` would claim truncation for a query whose last
/// match happened to be the thousandth.
pub fn push_match(
    needle: &Needle,
    label: &str,
    row: u32,
    limit: usize,
    out: &mut Vec<TreeMatch>,
) -> bool {
    let Some((start, end)) = needle.find(label) else {
        return false;
    };
    if out.len() >= limit {
        return true;
    }
    out.push(TreeMatch { row, start, end });
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(label: &str, query: &str) -> Option<(u32, u32)> {
        Needle::new(query).and_then(|n| n.find(label))
    }

    #[test]
    fn the_span_is_where_the_query_sits_in_the_name() {
        assert_eq!(find("FileTree.tsx", "tree"), Some((4, 8)));
        assert_eq!(find("FileTree.tsx", "FileTree.tsx"), Some((0, 12)));
        assert_eq!(find("FileTree.tsx", "x"), Some((11, 12)));
    }

    #[test]
    fn matching_is_case_insensitive_in_both_directions() {
        // Typing lowercase has to find the capitalised file, which is the whole reason a user
        // types `tree` rather than `Tree`; and a shouted query has to find a lowercase name.
        assert_eq!(find("FileTree.tsx", "filetree"), Some((0, 8)));
        assert_eq!(find("readme.md", "README"), Some((0, 6)));
        assert_eq!(find("Cargo.toml", "CARGO.TOML"), Some((0, 10)));
    }

    #[test]
    fn it_is_a_substring_and_not_a_prefix() {
        // The decision recorded in the module header. A prefix rule would answer `None` here,
        // and finding `FileTree.tsx` by typing `tree` is the gesture this feature is for.
        assert_eq!(find("check-tree-status.mjs", "status"), Some((11, 17)));
    }

    #[test]
    fn only_the_first_occurrence_is_reported() {
        // Match order is walk order and each row has one span; a second occurrence on the same
        // row would need a second highlight and a second entry in the `3 of 17` counter, which
        // would make Down mean two different things depending on which row it is on.
        assert_eq!(find("aXbXc", "x"), Some((1, 2)));
    }

    #[test]
    fn nothing_matches_an_empty_query() {
        // `Needle::new` refuses it rather than every row matching. The state after the last
        // Backspace is "no search", not "everything".
        assert!(Needle::new("").is_none());
    }

    #[test]
    fn a_query_longer_than_the_name_does_not_match_or_panic() {
        // The arithmetic here is `hay.len() - needle.len()`, which underflows on `usize` and
        // would abort the blocking worker in a debug build the first time anyone typed past
        // the end of a short filename — which is every query, on its way past `go.rs`.
        assert_eq!(find("go.rs", "go.rs.bak"), None);
        assert_eq!(find("", "a"), None);
    }

    #[test]
    fn the_span_is_in_utf16_units_because_javascript_slices_in_utf16() {
        // `🦀` is two UTF-16 units and four UTF-8 bytes. A byte offset would put the highlight
        // two characters late here, silently, in exactly the way `splitHighlight`'s header
        // describes — and the frontend must not have to know which unit it was handed.
        assert_eq!("🦀".len(), 4);
        assert_eq!(find("🦀crab.rs", "crab"), Some((2, 6)));
        // A two-byte scalar: `é` is one UTF-16 unit and two UTF-8 bytes.
        assert_eq!(find("café.rs", "rs"), Some((5, 7)));
        // And the non-ASCII general path must agree with the ASCII fast path on ASCII input
        // that happens to sit in a non-ASCII label.
        assert_eq!(find("naïve.rs", "ve"), Some((3, 5)));
    }

    #[test]
    fn case_folding_reaches_beyond_ascii() {
        // `str::eq_ignore_ascii_case` is the fast path and would answer `None` for both of
        // these; the general path exists so a Cyrillic or Greek filename is searchable at all.
        assert_eq!(find("Привет.txt", "привет"), Some((0, 6)));
        assert_eq!(find("ΣΊΣΥΦΟΣ.md", "ίσυ"), Some((1, 4)));
    }

    #[test]
    fn a_match_at_the_very_end_reports_the_labels_length() {
        // The `hay.get(start + len)` branch: with no character after the match there is no
        // offset to read, and the end of the label is the answer. Getting this wrong reports a
        // zero-width span, which renders as no highlight at all on the row that matched best.
        assert_eq!(find("café", "fé"), Some((2, 4)));
    }

    #[test]
    fn truncation_means_there_was_more_and_not_merely_that_the_limit_was_reached() {
        let needle = Needle::new("a").expect("non-empty");
        let mut out = Vec::new();
        assert!(!push_match(&needle, "a", 0, 2, &mut out));
        assert!(!push_match(&needle, "a", 1, 2, &mut out));
        // The vector is full, but this row does not match — so nothing was lost and the
        // overlay must not claim it was.
        assert!(!push_match(&needle, "zzz", 2, 2, &mut out));
        // This one does, and now the count on screen is a lie unless it says so.
        assert!(push_match(&needle, "a", 3, 2, &mut out));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn a_zero_limit_still_reports_that_matches_exist() {
        // How the composed handler asks the second source "is there anything past the seam"
        // once the first has filled the budget.
        let needle = Needle::new("a").expect("non-empty");
        let mut out = Vec::new();
        assert!(push_match(&needle, "a", 0, 0, &mut out));
        assert!(out.is_empty());
    }
}
