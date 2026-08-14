//! Which language a file is, and what it declares.
//!
//! Two jobs, one crate, because the second cannot be done without the first: an outline needs a
//! grammar, and picking a grammar is the extension table. `ui/src/editor/languages.ts` mirrors
//! that table on the other side of the wire for the status bar's readout.
//!
//! # Why a cursor walk rather than a tree-sitter query
//!
//! Both grammars ship a `queries/tags.scm`, and neither is usable: the Rust one classifies
//! `impl` as `@reference.implementation` — a *reference*, not a container — and has no
//! `const_item`/`static_item`; the Go one captures every type *mention* rather than every
//! declaration. So the real choice was our own query against our own walk, and the walk wins on
//! four counts:
//!
//! 1. **A query pass is a whole-tree pass; this is not.** [`extract`] descends only into
//!    container bodies and **never enters a function body**, which is roughly 90% of the nodes
//!    in a source file. Over a 20k-file repository that constant decides whether the project
//!    index is a background nicety or a stall.
//! 2. **Nesting falls out for free.** A query returns a flat bag of captures that has to be
//!    re-nested by byte containment with a stack — a second algorithm to get wrong, and nesting
//!    *is* the feature: it is what makes a breadcrumb and a tree-shaped popup possible.
//! 3. **Header slicing.** `impl<T: Display> Trait<T> for Foo<T> where T: Clone` and Go's
//!    `(*Server)` are byte slices from a node's start to its body's start. A cursor has both
//!    offsets in hand; a query would need extra captures per shape.
//! 4. No runtime query-compile failure surface for a data file.
//!
//! The one thing a query would have bought is loudness when a grammar renames a node — a bad
//! `.scm` fails `Query::new` immediately, where a stale string in a `match` arm silently yields
//! nothing. That is bought back by [`tables::every_node_kind_exists`], which fails in CI rather
//! than at runtime, and it is not hypothetical: tree-sitter-go 0.25 renamed `method_spec` to
//! `method_elem`, and a table still naming the old one would have produced interfaces with no
//! methods, on every file, with nothing anywhere reporting it.
//!
//! # What this deliberately does not do
//!
//! * **Resolve anything.** There are no references, no definitions and no types here. `Go to
//!   definition` needs a language server; this crate only knows what a file *declares*.
//! * **Evaluate Go build constraints.** `//go:build linux` is a comment, so `foo_linux.go` and
//!   `foo_darwin.go` both contribute their `func open`. Disambiguating them is a compiler's job;
//!   the picker distinguishes them by the path each row carries.
//! * **Expand macros.** A `lazy_static! { … }` declares things a walk cannot see, and pretending
//!   otherwise would need an expander.
//!
//! # No subprocess, so ADR 0007 has nothing to say here
//!
//! The grammars are statically linked C. This crate contains no `Command::new` and no
//! `SpawnSpec`, so the `child_env` rule every spawn site in the workspace must obey does not
//! apply to it. Stated because the rule is prominent enough in `CLAUDE.md` that its absence
//! here would otherwise read as an oversight.

mod error;
mod extract;
mod go;
mod rust;
pub mod tables;
mod walk;

use std::path::Path;

pub use error::SymbolError;
pub use extract::{Limits, outline, outline_symbols};
pub use walk::{Admits, WalkOutcome, WalkRoot, WalkedFile, walk_symbols};

/// A language this crate can parse.
///
/// Two, and the enum is closed on purpose: every variant costs a C parser table in the binary
/// and a kind table that has to be kept true against a grammar release. Adding a third is a
/// deliberate act, not a configuration change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Rust,
    Go,
}

impl Lang {
    /// The language of a path, by extension.
    ///
    /// Extension only — no content sniffing and no shebang. The callers are a walk over a
    /// repository (where opening a file to guess at it would defeat the extension gate that
    /// makes the walk affordable) and an editor that already knows what it opened.
    pub fn of_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "rs" => Some(Self::Rust),
            "go" => Some(Self::Go),
            _ => None,
        }
    }

    /// What the status bar calls it. Matches `ui/src/editor/languages.ts`'s label exactly, so a
    /// `FileOutline::Ready { language }` and the readout beside it cannot disagree.
    pub fn label(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Go => "Go",
        }
    }

    /// The grammar.
    ///
    /// `Language::new` over the grammar crate's `LANGUAGE` constant, which is a
    /// `tree_sitter_language::LanguageFn` — the shim that lets a grammar built against CLI 0.25
    /// link into core 0.26 without either crate depending on the other's version. See the
    /// workspace manifest's comment.
    pub fn language(self) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter::Language::new(tree_sitter_rust::LANGUAGE),
            Self::Go => tree_sitter::Language::new(tree_sitter_go::LANGUAGE),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_map_to_the_two_languages_and_nothing_else() {
        assert_eq!(Lang::of_path(Path::new("src/main.rs")), Some(Lang::Rust));
        assert_eq!(Lang::of_path(Path::new("cmd/serve.go")), Some(Lang::Go));
        assert_eq!(Lang::of_path(Path::new("Cargo.toml")), None);
        assert_eq!(Lang::of_path(Path::new("README")), None);
        // A leading dot makes a hidden file, not an extension — the same rule
        // `ui/src/editor/languages.ts` states. `Path::extension` already agrees.
        assert_eq!(Lang::of_path(Path::new(".rs")), None);
    }

    #[test]
    fn both_grammars_load_and_report_an_abi_this_core_accepts() {
        // The check that a grammar bump would break loudly. `Language::new` cannot fail, but an
        // ABI outside the core's accepted range makes `Parser::set_language` return an error —
        // which, without this, would surface as every file in the workspace having no symbols.
        for lang in [Lang::Rust, Lang::Go] {
            let mut parser = tree_sitter::Parser::new();
            parser.set_language(&lang.language()).unwrap_or_else(|e| {
                panic!("{} grammar rejected by this tree-sitter: {e}", lang.label())
            });
        }
    }
}
