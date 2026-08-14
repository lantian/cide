//! What Rust declares, and how the walk finds it.
//!
//! Node kinds and field names are from `tree-sitter-rust` 0.24.2's `node-types.json`, read
//! rather than remembered. [`crate::tables`] fails the build if any of them stops existing.

use cide_ipc::{Symbol, SymbolKind};
use tree_sitter::Node;

use crate::extract::{Emit, Shape, Walker};

/// Every node kind this module names, for the table-validation test.
pub(crate) const KINDS: &[&str] = &[
    "mod_item",
    "struct_item",
    "enum_item",
    "union_item",
    "trait_item",
    "impl_item",
    "function_item",
    "function_signature_item",
    "const_item",
    "static_item",
    "type_item",
    "associated_type",
    "macro_definition",
    "foreign_mod_item",
    "enum_variant",
    "field_declaration",
    "declaration_list",
    "field_declaration_list",
    "ordered_field_declaration_list",
    "enum_variant_list",
    "source_file",
];

pub(crate) fn shape(kind: &str) -> Shape {
    match kind {
        "mod_item" => Shape::Symbol(SymbolKind::Module),
        "struct_item" => Shape::Symbol(SymbolKind::Struct),
        "enum_item" => Shape::Symbol(SymbolKind::Enum),
        "union_item" => Shape::Symbol(SymbolKind::Union),
        "trait_item" => Shape::Symbol(SymbolKind::Trait),
        "impl_item" => Shape::Symbol(SymbolKind::Impl),
        // Whether this is a `Function` or a `Method` depends on what encloses it, which `emit`
        // decides — the table cannot see the container from a kind string alone.
        "function_item" | "function_signature_item" => Shape::Symbol(SymbolKind::Function),
        "const_item" => Shape::Symbol(SymbolKind::Constant),
        "static_item" => Shape::Symbol(SymbolKind::Static),
        "type_item" | "associated_type" => Shape::Symbol(SymbolKind::TypeAlias),
        "macro_definition" => Shape::Symbol(SymbolKind::Macro),
        "enum_variant" => Shape::Symbol(SymbolKind::Variant),
        "field_declaration" => Shape::Symbol(SymbolKind::Field),

        // Walked through, never drawn. `extern "C" { … }` declares its contents at the level of
        // the block, not underneath a row nobody wrote; the four list nodes are the *bodies* of
        // the items above and exist only because the grammar needs somewhere to hang them.
        "foreign_mod_item"
        | "declaration_list"
        | "field_declaration_list"
        | "ordered_field_declaration_list"
        | "enum_variant_list"
        | "source_file" => Shape::Transparent,

        // Everything else, including every `block`. Not descending here is what keeps the walk
        // out of function bodies — see the crate docs.
        //
        // Deliberately skipped rather than handled: `use` and `extern crate` declare nothing the
        // file owns, `attribute_item` is a sibling of the item it decorates (so `#[cfg(test)]
        // mod tests` still reaches `mod_item` on the next iteration), and a `macro_invocation`
        // at item position declares things a walk cannot see without expanding it.
        _ => Shape::Skip,
    }
}

pub(crate) fn emit(
    walker: &mut Walker<'_>,
    node: Node<'_>,
    kind: SymbolKind,
    container: Option<&str>,
    depth: u16,
    out: &mut Vec<Symbol>,
) {
    let (kind, name, detail, name_node) = match kind {
        SymbolKind::Impl => {
            // `impl<T: Display> Trait<T> for Foo<T> where T: Clone`, whitespace-collapsed.
            //
            // A slice from the node's start to its body's start, rather than a reconstruction
            // from the `trait` and `type` fields. Reconstructing loses the generics, the
            // lifetimes and the `where` clause — and those are exactly what distinguishes the
            // three `impl Foo` blocks a generic type usually has, which is the whole reason the
            // breadcrumb names an impl at all.
            let body = node.child_by_field_name("body");
            let end = body.map_or(node.end_byte(), |b| b.start_byte());
            let header = walker.collapsed(node.start_byte(), end);
            // The selection lands on the header rather than the body: `impl` is a container, and
            // navigating to one should put the caret where the reader can see which impl it is.
            (SymbolKind::Impl, header.clone(), Some(header), None)
        }
        SymbolKind::Function => {
            let name_node = node.child_by_field_name("name");
            let name = name_node.map(|n| walker.text(n)).unwrap_or("_").to_string();
            // A `fn` is a method when its nearest container is an `impl` or a `trait`. Decided
            // from the parent chain rather than from a flag threaded down, so a free function
            // nested inside another function's module still reads as a function.
            let kind = if in_impl_or_trait(node) {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            };
            (kind, name, signature(walker, node), name_node)
        }
        _ => {
            let name_node = node.child_by_field_name("name");
            let name = name_node.map(|n| walker.text(n)).unwrap_or("_").to_string();
            (kind, name, None, name_node)
        }
    };

    let index = walker.push(
        out,
        Emit {
            kind,
            name: name.clone(),
            detail,
            container,
            node,
            name_node,
        },
    );

    // Only *containers* are descended into, and this test is load-bearing rather than tidy: a
    // `function_item`'s `body` field **is** its `block`, so descending on "has a body" walks
    // straight into every function in the file. That is the one thing the crate docs promise
    // does not happen, and it costs both the ~90% of nodes the walk exists to skip and an
    // outline full of a function's local `struct`s.
    //
    // `mod foo;` has no `body` at all and is a leaf, which is why this is still an `if let`.
    let descends = matches!(
        kind,
        SymbolKind::Module
            | SymbolKind::Struct
            | SymbolKind::Enum
            | SymbolKind::Union
            | SymbolKind::Trait
            | SymbolKind::Impl
            | SymbolKind::Variant
    );
    if !descends {
        return;
    }
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let inner = join(container, &name);
    let mut children = Vec::new();
    walker.descend(body, Some(&inner), depth + 1, &mut children);
    out[index].children = children;
}

/// Is the nearest enclosing *container* an `impl` or a `trait`?
///
/// Walks up through the body list nodes only. Stopping at the first thing that is neither is
/// what makes this "nearest container" rather than "anywhere above".
fn in_impl_or_trait(node: Node<'_>) -> bool {
    let mut parent = node.parent();
    while let Some(p) = parent {
        match p.kind() {
            "impl_item" | "trait_item" => return true,
            "declaration_list" => parent = p.parent(),
            _ => return false,
        }
    }
    false
}

/// `fn new(x: u32) -> Result<T>` — the declaration without its body.
///
/// Sliced rather than rebuilt from fields, so generics, `where` clauses, `async`, `unsafe` and
/// the return type all survive without this function knowing they exist.
fn signature(walker: &Walker<'_>, node: Node<'_>) -> Option<String> {
    let end = node
        .child_by_field_name("body")
        .map_or(node.end_byte(), |b| b.start_byte());
    let text = walker.collapsed(node.start_byte(), end);
    let text = text.trim_end_matches(['{', ' ']).trim_end();
    (!text.is_empty()).then(|| text.to_string())
}

/// `net` + `server` → `net::server`.
fn join(container: Option<&str>, name: &str) -> String {
    match container {
        Some(outer) => format!("{outer}::{name}"),
        None => name.to_string(),
    }
}
