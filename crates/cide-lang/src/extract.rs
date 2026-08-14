//! The cursor walk: a parse tree in, a [`Symbol`] tree out.
//!
//! One traversal, shared by both languages, with the per-language decisions delegated to
//! [`crate::rust`] and [`crate::go`]. See the crate docs for why this is a walk and not a query.
//!
//! # The rule that makes it affordable
//!
//! [`Walker::descend`] enters a node only when the language's table calls it a *container*.
//! Function bodies are never entered, and they are the overwhelming majority of the nodes in a
//! source file. That is not an optimisation bolted on afterwards — it is the reason a
//! whole-repository symbol walk is a background nicety rather than a stall.
//!
//! # Columns
//!
//! tree-sitter's `Point::column` is a **byte** offset. Everything downstream wants UTF-16 code
//! units — see `cide_ipc::symbols` — so [`Walker::point`] converts, per symbol, against the
//! line's own text. Invisible in every ASCII fixture, and off by one per non-ASCII character
//! before the name in every other file.

use cide_ipc::{FileOutline, Symbol, SymbolKind, SymbolSpan};
use tree_sitter::{Node, Parser, Tree};

use crate::Lang;

/// Ceilings, so one pathological file cannot decide how long a walk takes or how much it costs.
///
/// The same idiom as `cide_search::content::Limits`, and named rather than inlined for the same
/// reason: every one of these is a place where the honest answer becomes "we stopped", and a
/// caller has to be able to say so.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// 1 MiB, below content search's 2 MiB deliberately: a generated `bindings.rs` costs a full
    /// parse here where a grep costs a `memmem`, and nobody navigates to a symbol in one.
    pub max_file_bytes: u64,
    /// One generated file must not fill the whole budget and hide every hand-written one.
    pub max_symbols_per_file: usize,
    /// How deep the container stack may go before the walk stops descending. A pathological
    /// nesting must not recurse without bound; 24 is far past anything a human writes.
    pub max_depth: u16,
    /// The whole-project ceiling, enforced by [`crate::walk_symbols`] rather than here.
    ///
    /// 400k symbols is roughly 100 MB across the store and the fuzzy matcher — see the crate
    /// docs' arithmetic. Past it the walk stops and says so, because an index that silently
    /// stops at some size is worse than one that reports it did.
    pub max_symbols: usize,
    /// Per-file parse budget. One pathological file must not wedge a walker thread.
    pub parse_budget: std::time::Duration,
    /// Walker threads. 0 means one per core, which is `ignore`'s own default.
    pub threads: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_file_bytes: 1 << 20,
            max_symbols_per_file: 2_000,
            max_depth: 24,
            max_symbols: 400_000,
            parse_budget: std::time::Duration::from_millis(250),
            threads: 0,
        }
    }
}

/// The structure of one file, as the three-state answer the popup renders.
///
/// Never returns an error. A language with no grammar is [`FileOutline::Unsupported`] and a file
/// that could not be parsed is [`FileOutline::Failed`] — both carry a sentence the user reads.
/// Rejecting instead would make the overlay show `pendingCommand`'s "not registered in this
/// build" wording for a file that is merely TypeScript.
pub fn outline(path: &std::path::Path, text: &str, limits: Limits) -> FileOutline {
    let Some(lang) = Lang::of_path(path) else {
        return FileOutline::Unsupported {
            reason: unsupported_reason(path),
        };
    };
    if text.len() as u64 > limits.max_file_bytes {
        return FileOutline::Failed {
            reason: format!(
                "This file is too large to outline ({}).",
                human_bytes(text.len() as u64)
            ),
        };
    }
    let Some((symbols, truncated)) = outline_symbols(lang, text, limits) else {
        return FileOutline::Failed {
            reason: format!("The {} parser could not read this file.", lang.label()),
        };
    };
    FileOutline::Ready {
        language: lang.label().to_string(),
        symbols,
        truncated,
    }
}

/// The symbols of a buffer already known to be `lang`, or `None` if the parse failed outright.
///
/// Split out of [`outline`] so the walk can call it without re-deciding the language per file,
/// and so tests can drive one language directly.
///
/// A file with **syntax errors still yields its symbols**. tree-sitter is error-tolerant by
/// design and a half-typed file is exactly when an outline matters most, so there is deliberately
/// no `root.has_error()` check here — that would blank the popup on the keystroke after `fn `.
pub fn outline_symbols(lang: Lang, text: &str, limits: Limits) -> Option<(Vec<Symbol>, bool)> {
    let mut parser = Parser::new();
    let tree = parse_with(&mut parser, lang, text, limits)?;
    Some(symbols_of(&tree, text, lang, limits))
}

/// Walk an already-parsed tree. Split out so the project walk can reuse one [`Parser`] per
/// thread instead of building one per file — a `Parser` carries the compiled grammar, and
/// rebuilding it 20,000 times is most of what a naive walk spends its time on.
pub(crate) fn symbols_of(
    tree: &Tree,
    text: &str,
    lang: Lang,
    limits: Limits,
) -> (Vec<Symbol>, bool) {
    let mut walker = Walker::new(text, lang, limits);
    let mut out = Vec::new();
    walker.descend(tree.root_node(), None, 0, &mut out);
    (out, walker.truncated)
}

/// Parse, giving up if the grammar is refused or the file exceeds its time budget.
pub(crate) fn parse_with(
    parser: &mut Parser,
    lang: Lang,
    text: &str,
    limits: Limits,
) -> Option<Tree> {
    // Only fails on an ABI mismatch, which `lib.rs`'s test would have caught first. Reported
    // rather than unwrapped because a failed parse must degrade to "no outline", never to a
    // panic inside a Tauri command.
    if let Err(error) = parser.set_language(&lang.language()) {
        tracing::error!(%error, language = lang.label(), "grammar rejected");
        return None;
    }

    // A budget rather than trust. tree-sitter is linear on ordinary source, but a file that is
    // one 400,000-character expression is not ordinary source, and on a walk it would hold a
    // walker thread for as long as it liked. `ControlFlow::Break` makes `parse_with_options`
    // return `None`, which the caller reports as `Failed` rather than as an empty outline.
    let deadline = std::time::Instant::now() + limits.parse_budget;
    // Bound to a `let` rather than written inline: `progress_callback` borrows it, and a closure
    // built inside the `Some(&mut …)` is a temporary that dies at the end of the statement.
    let mut over_budget = |_state: &tree_sitter::ParseState| {
        if std::time::Instant::now() >= deadline {
            std::ops::ControlFlow::Break(())
        } else {
            std::ops::ControlFlow::Continue(())
        }
    };
    let options = tree_sitter::ParseOptions {
        progress_callback: Some(&mut over_budget),
    };
    parser.parse_with_options(
        &mut |byte, _| text.as_bytes().get(byte..).unwrap_or(&[]),
        None,
        Some(options),
    )
}

/// `cide parses Rust and Go. This file is TOML.` — a sentence, not a status.
///
/// The language name comes from the extension rather than from a table of every language in the
/// world: naming what we do parse is the actionable half, and guessing at what the file *is*
/// adds a second thing that can be wrong.
fn unsupported_reason(path: &std::path::Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("cide outlines Rust and Go. This is a .{ext} file."),
        None => "cide outlines Rust and Go, by file extension. This file has none.".to_string(),
    }
}

fn human_bytes(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64)
    } else {
        format!("{} kB", bytes / 1024)
    }
}

/// What a language's table says about one node kind.
pub(crate) enum Shape {
    /// Emit a symbol, then descend into its body if it has one.
    Symbol(SymbolKind),
    /// Emit nothing, but walk through — `type_declaration` wrapping a `type_spec`, a
    /// `declaration_list` inside a `mod`, an `extern` block. Without this the items inside are
    /// invisible; with it as a *symbol* the outline grows a row for a node the user never wrote.
    Transparent,
    /// Not a declaration. Never descended into — this arm is what keeps the walk out of function
    /// bodies.
    Skip,
}

/// One symbol on its way into the outline.
///
/// A struct rather than seven positional arguments to [`Walker::push`]: two of them are
/// `Option<Node>` and two are `Option<String>`, and at a call site that is four values whose
/// order nothing but the compiler's type check would catch being swapped. (Clippy's
/// `too_many_arguments` noticed first; the readability was the actual reason to fix it.)
pub(crate) struct Emit<'t, 'c> {
    pub kind: SymbolKind,
    pub name: String,
    pub detail: Option<String>,
    pub container: Option<&'c str>,
    /// The whole declaration.
    pub node: Node<'t>,
    /// The name alone, when the grammar gives it a node. `None` for a Rust `impl`, whose
    /// selection then falls back to the header.
    pub name_node: Option<Node<'t>>,
}

pub(crate) struct Walker<'a> {
    pub(crate) src: &'a str,
    /// Byte offset of the start of each line, so a `Point` can be turned into a UTF-16 column
    /// with one slice instead of a scan from the top of the file.
    line_starts: Vec<usize>,
    lang: Lang,
    limits: Limits,
    emitted: usize,
    pub(crate) truncated: bool,
}

impl<'a> Walker<'a> {
    fn new(src: &'a str, lang: Lang, limits: Limits) -> Self {
        let mut line_starts = vec![0usize];
        line_starts.extend(src.match_indices('\n').map(|(i, _)| i + 1));
        Self {
            src,
            line_starts,
            lang,
            limits,
            emitted: 0,
            truncated: false,
        }
    }

    /// Walk the named children of `node`, appending whatever they declare to `out`.
    pub(crate) fn descend(
        &mut self,
        node: Node<'_>,
        container: Option<&str>,
        depth: u16,
        out: &mut Vec<Symbol>,
    ) {
        if depth > self.limits.max_depth {
            self.truncated = true;
            return;
        }
        let mut cursor = node.walk();
        // Collected first because the per-language handlers need their own cursors, and a
        // `TreeCursor` cannot be held across them. The vec is one level of a tree, never a file.
        let children: Vec<Node<'_>> = node.named_children(&mut cursor).collect();
        for child in children {
            if self.emitted >= self.limits.max_symbols_per_file {
                self.truncated = true;
                return;
            }
            match self.shape(child.kind()) {
                Shape::Skip => {}
                Shape::Transparent => self.descend(child, container, depth, out),
                Shape::Symbol(kind) => self.emit(child, kind, container, depth, out),
            }
        }
    }

    fn shape(&self, kind: &str) -> Shape {
        match self.lang {
            Lang::Rust => crate::rust::shape(kind),
            Lang::Go => crate::go::shape(kind),
        }
    }

    fn emit(
        &mut self,
        node: Node<'_>,
        kind: SymbolKind,
        container: Option<&str>,
        depth: u16,
        out: &mut Vec<Symbol>,
    ) {
        match self.lang {
            Lang::Rust => crate::rust::emit(self, node, kind, container, depth, out),
            Lang::Go => crate::go::emit(self, node, kind, container, depth, out),
        }
    }

    /// Record one symbol, and hand back its index so the caller can attach children to it.
    pub(crate) fn push(&mut self, out: &mut Vec<Symbol>, emit: Emit<'_, '_>) -> usize {
        self.emitted += 1;
        // The name's own span when there is one, the whole declaration when there is not (a Rust
        // `impl` has no name node). Never the body: landing the caret on `{` is not navigation.
        let selection = self.span(emit.name_node.unwrap_or(emit.node));
        out.push(Symbol {
            kind: emit.kind,
            name: emit.name,
            detail: emit.detail,
            container: emit.container.map(str::to_string),
            range: self.span(emit.node),
            selection,
            children: Vec::new(),
        });
        out.len() - 1
    }

    pub(crate) fn span(&self, node: Node<'_>) -> SymbolSpan {
        let (start_line, start_column) = self.point(node.start_position());
        let (end_line, end_column) = self.point(node.end_position());
        SymbolSpan {
            start_line,
            start_column,
            end_line,
            end_column,
        }
    }

    /// A tree-sitter `Point` as 1-based line and 1-based **UTF-16** column.
    ///
    /// `Point::column` is a byte offset into the line. Converting here, where the line's text is
    /// still in hand, is what keeps the caret out of the middle of a `é` — and the conversion
    /// costs a slice of one line rather than a scan of the file.
    fn point(&self, point: tree_sitter::Point) -> (u32, u32) {
        let line_start = self.line_starts.get(point.row).copied().unwrap_or(0);
        let byte = (line_start + point.column).min(self.src.len());
        // `get` rather than indexing: a `Point` inside a multi-byte character would panic on a
        // slice, and tree-sitter can produce one at the end of a truncated parse.
        let utf16 = self
            .src
            .get(line_start..byte)
            .map(|prefix| prefix.encode_utf16().count())
            .unwrap_or(point.column);
        (point.row as u32 + 1, utf16 as u32 + 1)
    }

    /// The source text of a node.
    pub(crate) fn text(&self, node: Node<'_>) -> &'a str {
        self.src.get(node.byte_range()).unwrap_or("")
    }

    /// A node's text with every run of whitespace collapsed to one space.
    ///
    /// For headers that a user wrote across several lines — a `where` clause, a long generic
    /// list. A breadcrumb with a newline in it breaks the status bar's single-line layout.
    pub(crate) fn collapsed(&self, from: usize, to: usize) -> String {
        self.src
            .get(from..to)
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The first named child of a given kind, for the shapes tree-sitter gives no field name to.
    pub(crate) fn named_child_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
        let mut cursor = node.walk();
        node.named_children(&mut cursor).find(|c| c.kind() == kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `kind name` per symbol, depth-first, indented by nesting — one string that shows both the
    /// contents and the shape, so an assertion reads like the outline the user would see.
    fn shape_of(lang: Lang, src: &str) -> String {
        let (symbols, _) = outline_symbols(lang, src, Limits::default()).expect("parse");
        let mut out = String::new();
        fn walk(symbols: &[Symbol], depth: usize, out: &mut String) {
            for s in symbols {
                out.push_str(&"  ".repeat(depth));
                out.push_str(&format!("{:?} {}\n", s.kind, s.name));
                walk(&s.children, depth + 1, out);
            }
        }
        walk(&symbols, 0, &mut out);
        out
    }

    fn flat(lang: Lang, src: &str) -> Vec<Symbol> {
        let (symbols, _) = outline_symbols(lang, src, Limits::default()).expect("parse");
        let mut out = Vec::new();
        fn walk(symbols: &[Symbol], out: &mut Vec<Symbol>) {
            for s in symbols {
                out.push(s.clone());
                walk(&s.children, out);
            }
        }
        walk(&symbols, &mut out);
        out
    }

    fn find<'a>(symbols: &'a [Symbol], name: &str) -> &'a Symbol {
        symbols
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("no symbol named {name} in {:?}", names(symbols)))
    }

    fn names(symbols: &[Symbol]) -> Vec<&str> {
        symbols.iter().map(|s| s.name.as_str()).collect()
    }

    // --- Rust ------------------------------------------------------------------------------

    #[test]
    fn rust_nests_modules_and_joins_their_containers() {
        let src = "mod a { mod b { fn c() {} } }";
        assert_eq!(
            shape_of(Lang::Rust, src),
            "Module a\n  Module b\n    Function c\n"
        );
        assert_eq!(
            find(&flat(Lang::Rust, src), "c").container.as_deref(),
            Some("a::b")
        );
    }

    #[test]
    fn a_generic_impl_keeps_its_where_clause_and_names_its_methods() {
        // The test that fails if an `impl` header is ever reconstructed from the `trait` and
        // `type` fields instead of sliced: a reconstruction drops `<T: Display>` and `where`.
        let src = "impl<T: Display> Trait<T> for Foo<T>\n where T: Clone\n{ fn f() {} }";
        let all = flat(Lang::Rust, src);
        let imp = &all[0];
        assert_eq!(imp.kind, SymbolKind::Impl);
        assert_eq!(
            imp.name,
            "impl<T: Display> Trait<T> for Foo<T> where T: Clone"
        );
        assert_eq!(imp.detail.as_deref(), Some(imp.name.as_str()));
        assert_eq!(
            find(&all, "f").container.as_deref(),
            Some("impl<T: Display> Trait<T> for Foo<T> where T: Clone")
        );
        assert_eq!(find(&all, "f").kind, SymbolKind::Method);
    }

    #[test]
    fn two_impls_of_one_type_stay_distinguishable() {
        // `Foo::new` and `Display::fmt for Foo` are different answers to "where is the caret",
        // and both are impl blocks in the same file. This is why `SymbolKind::Impl` exists.
        let src = "impl Foo { fn new() {} }\nimpl Display for Foo { fn fmt() {} }";
        let all = flat(Lang::Rust, src);
        assert_eq!(find(&all, "new").container.as_deref(), Some("impl Foo"));
        assert_eq!(
            find(&all, "fmt").container.as_deref(),
            Some("impl Display for Foo")
        );
    }

    #[test]
    fn a_declaration_only_module_is_a_leaf_rather_than_a_missing_row() {
        // `mod foo;` has no `body` field at all, which is why the descent is an `if let`.
        let all = flat(Lang::Rust, "mod foo;");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].kind, SymbolKind::Module);
        assert!(all[0].children.is_empty());
    }

    #[test]
    fn the_shapes_a_first_draft_forgets() {
        let src = r#"
macro_rules! m { () => {} }
const K: u32 = 1;
static S: u32 = 2;
type Alias = u32;
union U { a: u32 }
unsafe extern "C" { fn c_thing(); }
#[cfg(test)]
mod tests { fn t() {} }
"#;
        let all = flat(Lang::Rust, src);
        assert_eq!(find(&all, "m").kind, SymbolKind::Macro);
        assert_eq!(find(&all, "K").kind, SymbolKind::Constant);
        assert_eq!(find(&all, "S").kind, SymbolKind::Static);
        assert_eq!(find(&all, "Alias").kind, SymbolKind::TypeAlias);
        assert_eq!(find(&all, "U").kind, SymbolKind::Union);
        // `extern` blocks are transparent: `c_thing` is declared at the block's level, not
        // underneath a row nobody wrote.
        assert_eq!(find(&all, "c_thing").container, None);
        // An outline that hides the test module is wrong — and the attribute is a *sibling* of
        // `mod_item`, so a walk that treated `attribute_item` as opaque would lose it.
        assert_eq!(find(&all, "tests").kind, SymbolKind::Module);
        assert_eq!(find(&all, "t").container.as_deref(), Some("tests"));
    }

    #[test]
    fn a_trait_declares_signatures_and_associated_types() {
        let all = flat(Lang::Rust, "trait T { fn a(&self); type Item; }");
        // `function_signature_item` — a `fn` with no body — must still appear, or every trait in
        // the workspace outlines as empty.
        assert_eq!(find(&all, "a").kind, SymbolKind::Method);
        assert_eq!(find(&all, "Item").kind, SymbolKind::TypeAlias);
        assert_eq!(find(&all, "a").container.as_deref(), Some("T"));
    }

    #[test]
    fn struct_fields_and_enum_variants_reach_the_outline() {
        let src = "struct S { a: u32, b: u32 }\nenum E { X, Y(u32) }";
        let all = flat(Lang::Rust, src);
        assert_eq!(find(&all, "a").kind, SymbolKind::Field);
        assert_eq!(find(&all, "a").container.as_deref(), Some("S"));
        assert_eq!(find(&all, "X").kind, SymbolKind::Variant);
        assert_eq!(find(&all, "Y").container.as_deref(), Some("E"));
    }

    #[test]
    fn a_raw_identifier_keeps_its_prefix() {
        // `r#fn` is the name in the source and the name a user searches for.
        let all = flat(Lang::Rust, "fn r#fn() {}");
        assert_eq!(all[0].name, "r#fn");
    }

    #[test]
    fn a_file_that_stops_parsing_halfway_still_yields_the_half_that_parsed() {
        // tree-sitter is error-tolerant by design and a half-typed file is exactly when an
        // outline matters most. A `root.has_error()` check here would blank the popup on the
        // keystroke after `fn `.
        let all = flat(Lang::Rust, "fn good() {}\nfn broken( {\n");
        assert!(names(&all).contains(&"good"), "{:?}", names(&all));
    }

    #[test]
    fn a_signature_survives_without_its_body() {
        let all = flat(
            Lang::Rust,
            "pub async fn spawn(x: u32) -> Result<T> { todo!() }",
        );
        assert_eq!(
            all[0].detail.as_deref(),
            Some("pub async fn spawn(x: u32) -> Result<T>")
        );
    }

    // --- columns, the highest-risk detail --------------------------------------------------

    #[test]
    fn a_column_is_utf16_units_and_not_bytes() {
        // tree-sitter counts bytes. `é` is two bytes and one UTF-16 unit; `日` is three and one.
        // Getting this wrong is invisible in every ASCII fixture and puts the caret one position
        // early per non-ASCII character before the name.
        let src = "fn café_x() {}";
        let all = flat(Lang::Rust, src);
        // `fn ` is 3 chars, so the name starts at UTF-16 column 4 — the byte column would be 4
        // too here, since the accent is *inside* the name.
        assert_eq!(all[0].selection.start_column, 4);

        // Now with the non-ASCII *before* the name, where the two disagree.
        let src = "// é\nfn after() {}";
        let all = flat(Lang::Rust, src);
        assert_eq!(all[0].selection.start_line, 2);
        assert_eq!(all[0].selection.start_column, 4);

        // And on the same line as the name.
        let src = "struct 日本 { x: u32 }";
        let all = flat(Lang::Rust, src);
        let field = find(&all, "x");
        // `struct 日本 { ` — 7 ASCII + 2 CJK (one UTF-16 unit each) + 3 = 12 units before `x`.
        assert_eq!(field.selection.start_column, 13, "{field:?}");
    }

    #[test]
    fn crlf_does_not_shift_line_numbers() {
        let all = flat(Lang::Rust, "fn a() {}\r\nfn b() {}\r\n");
        assert_eq!(find(&all, "b").selection.start_line, 2);
    }

    #[test]
    fn a_selection_is_the_name_and_the_range_is_the_whole_declaration() {
        // Collapsing the two would put the caret on the `p` of `pub fn foo` when you navigate to
        // it, and lose every caret sitting inside a function body from the breadcrumb.
        let all = flat(Lang::Rust, "pub fn foo() {\n    let x = 1;\n}");
        let foo = &all[0];
        assert_eq!(foo.selection.start_line, 1);
        assert_eq!(foo.selection.start_column, 8, "the caret lands on `foo`");
        assert_eq!(foo.range.start_column, 1, "the range starts at `pub`");
        assert_eq!(foo.range.end_line, 3, "and covers the body");
    }

    // --- Go --------------------------------------------------------------------------------

    #[test]
    fn go_methods_carry_their_receiver_as_a_container() {
        let src =
            "package p\nfunc (s *Server) Serve() {}\nfunc (s Server) Name() string { return \"\" }";
        let all = flat(Lang::Go, src);
        // The pointer star is kept: `Server` and `*Server` are different method sets.
        assert_eq!(find(&all, "Serve").container.as_deref(), Some("*Server"));
        assert_eq!(find(&all, "Name").container.as_deref(), Some("Server"));
        // And the binding name is not in there — `(s *Server)` must not become `s *Server`.
        assert_eq!(find(&all, "Serve").kind, SymbolKind::Method);
    }

    #[test]
    fn a_grouped_type_declaration_resolves_struct_from_interface() {
        // The test that catches tree-sitter-go 0.25's `method_spec` → `method_elem` rename: `Do`
        // is reachable only through the new kind name.
        let src = "package p\ntype (\n\tA struct{ x int }\n\tB interface{ Do() error }\n)";
        let all = flat(Lang::Go, src);
        assert_eq!(find(&all, "A").kind, SymbolKind::Struct);
        assert_eq!(find(&all, "B").kind, SymbolKind::Interface);
        assert_eq!(find(&all, "Do").kind, SymbolKind::Method);
        assert_eq!(find(&all, "Do").container.as_deref(), Some("B"));
        assert_eq!(find(&all, "x").container.as_deref(), Some("A"));
    }

    #[test]
    fn one_spec_node_with_several_names_is_several_symbols() {
        // Taking the first `name` field would silently drop every second constant in a grouped
        // block — `B` and `C` here, and `y` below.
        let src = "package p\nconst (\n\tA = 1\n\tB, C = 2, 3\n)\nvar x, y int";
        let all = flat(Lang::Go, src);
        for name in ["A", "B", "C"] {
            assert_eq!(find(&all, name).kind, SymbolKind::Constant, "{name}");
        }
        for name in ["x", "y"] {
            assert_eq!(find(&all, name).kind, SymbolKind::Variable, "{name}");
        }
    }

    #[test]
    fn go_generics_and_aliases_parse() {
        let src = "package p\nfunc Map[T any](xs []T) []T { return xs }\ntype Alias = int";
        let all = flat(Lang::Go, src);
        assert_eq!(find(&all, "Map").kind, SymbolKind::Function);
        assert!(
            find(&all, "Map")
                .detail
                .as_deref()
                .unwrap()
                .contains("[T any]"),
            "{:?}",
            find(&all, "Map").detail
        );
        assert_eq!(find(&all, "Alias").kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn an_embedded_field_is_named_after_what_it_embeds() {
        // `type A struct { B }` has no `name` field at all. A row with no text is worse than a
        // row named after the type, which is what the user reads in the source anyway.
        let all = flat(Lang::Go, "package p\ntype A struct {\n\tB\n\tc int\n}");
        assert_eq!(find(&all, "B").kind, SymbolKind::Field);
        assert_eq!(find(&all, "c").kind, SymbolKind::Field);
    }

    #[test]
    fn the_package_clause_is_the_files_root_container() {
        let all = flat(Lang::Go, "package server\nfunc F() {}");
        assert_eq!(find(&all, "server").kind, SymbolKind::Package);
    }

    #[test]
    fn a_build_tag_is_a_comment_and_the_file_is_indexed_anyway() {
        // Build constraints are deliberately not evaluated — see the crate docs. `foo_linux.go`
        // and `foo_darwin.go` both contribute their `open`, disambiguated by path.
        let all = flat(Lang::Go, "//go:build linux\n\npackage p\n\nfunc open() {}");
        assert_eq!(find(&all, "open").kind, SymbolKind::Function);
    }

    #[test]
    fn imports_declare_nothing_this_file_owns() {
        let all = flat(
            Lang::Go,
            "package p\nimport (\n\t\"fmt\"\n\tx \"os\"\n)\nfunc F() {}",
        );
        assert!(!names(&all).contains(&"fmt"), "{:?}", names(&all));
        assert!(!names(&all).contains(&"x"), "{:?}", names(&all));
    }

    // --- the walk's own rules ---------------------------------------------------------------

    #[test]
    fn nothing_inside_a_function_body_reaches_the_outline() {
        // The rule that makes a whole-repository walk affordable. A nested `fn` is a declaration
        // Rust genuinely allows, and it is deliberately *not* indexed: descending into bodies is
        // ~90% of the nodes in a source file.
        let all = flat(
            Lang::Rust,
            "fn outer() {\n    fn inner() {}\n    struct Local;\n}",
        );
        assert_eq!(names(&all), vec!["outer"]);
    }

    #[test]
    fn a_file_past_the_per_file_cap_reports_that_it_stopped() {
        let src: String = (0..50).map(|i| format!("fn f{i}() {{}}\n")).collect();
        let limits = Limits {
            max_symbols_per_file: 10,
            ..Limits::default()
        };
        let (symbols, truncated) = outline_symbols(Lang::Rust, &src, limits).expect("parse");
        assert!(truncated, "a cap that stops silently is a cap that lies");
        assert!(symbols.len() <= 10, "{}", symbols.len());
    }

    // --- the three-state answer --------------------------------------------------------------

    #[test]
    fn a_language_with_no_grammar_is_unsupported_rather_than_empty() {
        let outline = outline(
            std::path::Path::new("Cargo.toml"),
            "[package]\n",
            Limits::default(),
        );
        let FileOutline::Unsupported { reason } = outline else {
            panic!("{outline:?}");
        };
        assert!(reason.contains(".toml"), "{reason}");
        // The claim must never read like "this file declares nothing".
        assert!(!reason.to_lowercase().contains("no symbols"), "{reason}");
    }

    #[test]
    fn a_file_past_the_size_cap_is_failed_and_not_unsupported() {
        // Two different facts about a file that both draw an empty list. A bug report needs the
        // right one.
        let big = "fn a() {}\n".repeat(200_000);
        let outline = outline(std::path::Path::new("big.rs"), &big, Limits::default());
        let FileOutline::Failed { reason } = outline else {
            panic!("{outline:?}");
        };
        assert!(reason.contains("too large"), "{reason}");
    }

    #[test]
    fn a_rust_file_that_declares_nothing_is_a_reportable_zero() {
        let outline = outline(
            std::path::Path::new("lib.rs"),
            "use std::fmt;\n",
            Limits::default(),
        );
        let FileOutline::Ready {
            language,
            symbols,
            truncated,
        } = outline
        else {
            panic!("{outline:?}");
        };
        assert_eq!(language, "Rust");
        assert!(symbols.is_empty());
        assert!(!truncated);
    }
}
