//! The one thing a tree-sitter query would have given us for free.
//!
//! A bad `.scm` fails `Query::new` immediately and loudly. A stale node-kind string in a `match`
//! arm fails *silently*: the arm never matches, the walk emits nothing for that shape, and the
//! outline is simply missing a category of declaration with nothing anywhere reporting it.
//!
//! So the kind strings are data, and this module checks them against the grammar. It runs in CI
//! rather than at runtime, which is strictly better than the query's loudness — the failure lands
//! on whoever bumped the grammar, not on a user opening a file.
//!
//! It is not hypothetical. `tree-sitter-go` 0.25 renamed `method_spec` to `method_elem`; a table
//! still naming the old one would have produced interfaces with no methods, in every Go file in
//! every project, and every test that did not specifically open an interface would have passed.

use crate::Lang;

/// Does every node kind the extractor names still exist in its grammar?
///
/// Returns the offenders, so the failure message can name them rather than saying "something
/// changed". Public so it is callable from an integration test as well as the unit test below.
pub fn unknown_kinds(lang: Lang) -> Vec<&'static str> {
    let language = lang.language();
    let kinds = match lang {
        Lang::Rust => crate::rust::KINDS,
        Lang::Go => crate::go::KINDS,
    };
    kinds
        .iter()
        .copied()
        // `id_for_node_kind` answers 0 for a kind this grammar does not have. `named = true`
        // throughout, because every kind the extractor matches on is a named node — an anonymous
        // one would be punctuation, which no table here should ever mention.
        .filter(|kind| language.id_for_node_kind(kind, true) == 0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_node_kind_exists_in_its_grammar() {
        for lang in [Lang::Rust, Lang::Go] {
            let unknown = unknown_kinds(lang);
            assert!(
                unknown.is_empty(),
                "{} names node kinds its grammar does not have: {unknown:?}\n\
                 A grammar bump renamed them. Read the new `node-types.json` and update the \
                 KINDS table *and* the `shape` match — updating only the table makes this test \
                 pass while the extractor still emits nothing for that shape.",
                lang.label(),
            );
        }
    }

    #[test]
    fn the_tables_are_not_accidentally_empty() {
        // A `filter` over an empty slice is vacuously fine, so the test above would pass if
        // somebody deleted a table. These floors are far below the real counts.
        assert!(crate::rust::KINDS.len() >= 15);
        assert!(crate::go::KINDS.len() >= 15);
    }
}
