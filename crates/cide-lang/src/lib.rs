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

/// Is the declaration at `line` a method belonging to an **interface** rather than to a concrete
/// type?
///
/// The discriminator behind Go to definition's one language-specific behaviour, and the reason it
/// can be language-specific without a `match` on the language: only Go's outline produces
/// [`SymbolKind::Interface`] with method children, so a Rust file answers `false` by construction
/// rather than by exemption.
///
/// # Why this question, and not "is the file Go"
///
/// The report was that Go to definition on a Go method call lands on the interface. gopls is
/// right — `textDocument/definition` resolves to where the callee is *declared* — but it is not
/// what the user wanted, and `textDocument/implementation` is the question that is. The obvious
/// fix, "in Go, ask for implementations first", is wrong in both languages at once: in Rust it
/// turns Ctrl+B on a struct name into a jump to an `impl` block, and in Go it turns Ctrl+B on an
/// interface *type name* — `io.Reader` — into a jump to some implementor, when the declaration is
/// exactly what a reader of that line wants.
///
/// Both of those are type names; the case the user is complaining about is a *method*. So the
/// test is neither the language nor the caret, but where the answer landed: a definition that
/// arrived inside an interface's method set is the one, and only, case where a concrete
/// implementation is the better answer. A type name lands on the type spec — which is *outside*
/// the brace block that holds the methods — and is left alone.
///
/// `line` is 1-based, matching [`cide_ipc::SymbolSpan`] and every position on this wire.
pub fn interface_method_at(outline: &cide_ipc::FileOutline, line: u32) -> bool {
    let cide_ipc::FileOutline::Ready { symbols, .. } = outline else {
        return false;
    };
    symbols.iter().any(|symbol| holds_method_at(symbol, line))
}

/// Depth-first: does `symbol` — or anything inside it — hold a method at `line`?
///
/// Recursive rather than a scan of the flat index, because "whose child is this" is the entire
/// question and the flat list is precisely where that is thrown away.
fn holds_method_at(symbol: &cide_ipc::Symbol, line: u32) -> bool {
    if !spans(&symbol.range, line) {
        return false;
    }
    if symbol.kind == cide_ipc::SymbolKind::Interface
        && symbol
            .children
            .iter()
            .any(|child| spans(&child.selection, line))
    {
        return true;
    }
    symbol.children.iter().any(|c| holds_method_at(c, line))
}

fn spans(span: &cide_ipc::SymbolSpan, line: u32) -> bool {
    span.start_line <= line && line <= span.end_line
}

#[cfg(test)]
mod interface_method_tests {
    use super::*;

    /// Real Go, because the whole predicate is a claim about what tree-sitter-go produces and a
    /// hand-built `Symbol` tree would only test the walk.
    const GO: &str = r#"package p

type Reader interface {
	Read(p []byte) (int, error)
	Close() error
}

type File struct{ name string }

func (f *File) Read(p []byte) (int, error) { return 0, nil }

func (f *File) Close() error { return nil }
"#;

    fn outline_of(src: &str) -> cide_ipc::FileOutline {
        outline(std::path::Path::new("p.go"), src, Limits::default())
    }

    fn line_of(src: &str, needle: &str) -> u32 {
        src.lines()
            .position(|l| l.contains(needle))
            .map(|i| i as u32 + 1)
            .unwrap_or_else(|| panic!("no line containing {needle:?}"))
    }

    /// The case the report is about: the definition landed on a method inside `interface { … }`,
    /// so a concrete implementation is the better answer and Go to definition asks for one.
    #[test]
    fn a_method_inside_an_interface_is_the_case_that_wants_an_implementation() {
        let outline = outline_of(GO);
        for method in ["Read(p []byte)", "Close() error\n"] {
            let line = line_of(GO, method.trim_end_matches('\n'));
            assert!(
                interface_method_at(&outline, line),
                "line {line} ({method:?}) is an interface method"
            );
        }
    }

    /// The case that must NOT trigger, and the reason this is not simply "is the file Go".
    ///
    /// `type Reader interface {` is where Ctrl+B on `io.Reader` lands. Redirecting it to an
    /// implementor would take the declaration away from the gesture whose whole name is "go to
    /// definition" — the Go-side twin of the Rust regression this predicate exists to avoid.
    #[test]
    fn an_interface_type_name_is_left_alone() {
        let outline = outline_of(GO);
        assert!(
            !interface_method_at(&outline, line_of(GO, "type Reader interface")),
            "the type spec is outside the brace block that holds the methods"
        );
    }

    /// A concrete method with the same name as an interface one. Nothing to redirect to: the
    /// definition already *is* the implementation.
    #[test]
    fn a_concrete_method_is_not_an_interface_method() {
        let outline = outline_of(GO);
        for concrete in [
            "func (f *File) Read",
            "func (f *File) Close",
            "type File struct",
        ] {
            let line = line_of(GO, concrete);
            assert!(
                !interface_method_at(&outline, line),
                "line {line} ({concrete:?}) is concrete"
            );
        }
    }

    /// Rust answers `false` by construction rather than by an exemption, which is what lets the
    /// caller skip a language check entirely.
    #[test]
    fn rust_has_no_interface_methods_to_find() {
        let src = "trait Reader { fn read(&self) -> usize; }\nstruct F;\nimpl Reader for F { fn read(&self) -> usize { 0 } }\n";
        let outline = outline(std::path::Path::new("p.rs"), src, Limits::default());
        for line in 1..=3 {
            assert!(!interface_method_at(&outline, line), "line {line}");
        }
    }

    /// A file with no parser, or one that failed to parse, is not an interface method.
    #[test]
    fn an_outline_that_is_not_ready_answers_no() {
        let outline = outline(std::path::Path::new("p.toml"), "x = 1\n", Limits::default());
        assert!(!interface_method_at(&outline, 1));
    }
}
