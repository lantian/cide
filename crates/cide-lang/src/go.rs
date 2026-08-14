//! What Go declares, and how the walk finds it.
//!
//! Node kinds and field names are from `tree-sitter-go` 0.25.0's `node-types.json`, read rather
//! than remembered — and this grammar is exactly why [`crate::tables`] exists: 0.25 renamed
//! `method_spec` to `method_elem`, and a table still naming the old one would have produced
//! interfaces with no methods, on every file, with nothing anywhere reporting it.
//!
//! # Multiple names per declaration
//!
//! `const ( A = 1; B, C = 2, 3 )` and `var x, y int` declare two symbols from one node. Go's
//! spec nodes carry repeated `name` fields, so [`emit`] iterates them rather than taking the
//! first — taking the first would silently drop every second constant in a grouped block.

use cide_ipc::{Symbol, SymbolKind};
use tree_sitter::Node;

use crate::extract::{Emit, Shape, Walker};

/// Every node kind this module names, for the table-validation test.
pub(crate) const KINDS: &[&str] = &[
    "package_clause",
    "package_identifier",
    "function_declaration",
    "method_declaration",
    "type_declaration",
    "type_spec",
    "type_alias",
    "const_declaration",
    "const_spec",
    "var_declaration",
    "var_spec",
    "var_spec_list",
    "struct_type",
    "interface_type",
    "field_declaration_list",
    "field_declaration",
    "method_elem",
    "parameter_list",
    "source_file",
];

pub(crate) fn shape(kind: &str) -> Shape {
    match kind {
        "package_clause" => Shape::Symbol(SymbolKind::Package),
        "function_declaration" => Shape::Symbol(SymbolKind::Function),
        "method_declaration" => Shape::Symbol(SymbolKind::Method),
        // Resolved to Struct / Interface / TypeAlias by looking at the `type` field — the kind
        // string alone does not say which.
        "type_spec" => Shape::Symbol(SymbolKind::TypeAlias),
        "type_alias" => Shape::Symbol(SymbolKind::TypeAlias),
        "const_spec" => Shape::Symbol(SymbolKind::Constant),
        "var_spec" => Shape::Symbol(SymbolKind::Variable),
        "field_declaration" => Shape::Symbol(SymbolKind::Field),
        "method_elem" => Shape::Symbol(SymbolKind::Method),

        // The grouping wrappers. `type ( A struct{}; B int )` is one `type_declaration` holding
        // two `type_spec`s, and drawing a row for the wrapper would put a nameless node in the
        // outline between the file and its types.
        "type_declaration"
        | "const_declaration"
        | "var_declaration"
        | "var_spec_list"
        | "struct_type"
        | "interface_type"
        | "field_declaration_list"
        | "source_file" => Shape::Transparent,

        // Everything else, including every `block`. `import_declaration` is skipped rather than
        // walked: an import declares nothing this file owns.
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
    match node.kind() {
        // One node, several names.
        "const_spec" | "var_spec" | "field_declaration" => {
            emit_each_name(walker, node, kind, container, out);
            return;
        }
        _ => {}
    }

    // Two different containers, and conflating them was a bug: `own` is what *this* symbol
    // belongs to (a method belongs to its receiver), `child` is what its members will belong to
    // (a struct's fields belong to the struct). Only `type_spec` sets the second, and only
    // `method_declaration` sets the first.
    let (kind, name, detail, name_node, own_container, child_container) = match node.kind() {
        "package_clause" => {
            let name_node = Walker::named_child_of_kind(node, "package_identifier");
            let name = name_node
                .map(|n| walker.text(n))
                .unwrap_or("main")
                .to_string();
            (SymbolKind::Package, name, None, name_node, None, None)
        }
        "method_declaration" => {
            let name_node = node.child_by_field_name("name");
            let name = name_node.map(|n| walker.text(n)).unwrap_or("_").to_string();
            // `(*Server)` → `*Server`, `(s Server)` → `Server`.
            //
            // Sliced from the receiver's *type* node, not from the whole `parameter_list`:
            // `(s *Server)` would otherwise put the binding name in the breadcrumb, and Go to
            // Symbol would offer `s *Server.Serve`. The pointer star is kept, because
            // `func (s Server)` and `func (s *Server)` are different method sets and a reader
            // needs to see which one they are in.
            let receiver = node
                .child_by_field_name("receiver")
                .and_then(receiver_type)
                .map(|n| walker.collapsed(n.start_byte(), n.end_byte()));
            (
                SymbolKind::Method,
                name,
                signature(walker, node),
                name_node,
                receiver,
                None,
            )
        }
        "function_declaration" => {
            let name_node = node.child_by_field_name("name");
            let name = name_node.map(|n| walker.text(n)).unwrap_or("_").to_string();
            (
                SymbolKind::Function,
                name,
                signature(walker, node),
                name_node,
                None,
                None,
            )
        }
        "type_spec" | "type_alias" => {
            let name_node = node.child_by_field_name("name");
            let name = name_node.map(|n| walker.text(n)).unwrap_or("_").to_string();
            let underlying = node.child_by_field_name("type");
            let kind = match (node.kind(), underlying.map(|n| n.kind())) {
                ("type_alias", _) => SymbolKind::TypeAlias,
                (_, Some("struct_type")) => SymbolKind::Struct,
                (_, Some("interface_type")) => SymbolKind::Interface,
                _ => SymbolKind::TypeAlias,
            };
            let detail = underlying.map(|n| match n.kind() {
                "struct_type" => "struct".to_string(),
                "interface_type" => "interface".to_string(),
                _ => walker.collapsed(n.start_byte(), n.end_byte()),
            });
            let child = Some(name.clone());
            (kind, name, detail, name_node, None, child)
        }
        "method_elem" => {
            let name_node = node.child_by_field_name("name");
            let name = name_node.map(|n| walker.text(n)).unwrap_or("_").to_string();
            (
                SymbolKind::Method,
                name,
                signature(walker, node),
                name_node,
                None,
                None,
            )
        }
        _ => {
            let name_node = node.child_by_field_name("name");
            let name = name_node.map(|n| walker.text(n)).unwrap_or("_").to_string();
            (kind, name, None, name_node, None, None)
        }
    };

    let own = own_container.as_deref().or(container);
    let index = walker.push(
        out,
        Emit {
            kind,
            name,
            detail,
            container: own,
            node,
            name_node,
        },
    );

    // Only a type's underlying struct/interface is descended into — the one Go shape that holds
    // further declarations. `child_by_field_name("type")` is `None` for a function or a method,
    // whose body hangs off `body`, so a function body is no more a container here than in Rust.
    let Some(body) = node.child_by_field_name("type") else {
        return;
    };
    let inner = child_container.as_deref().or(container);
    let mut children = Vec::new();
    walker.descend(body, inner, depth + 1, &mut children);
    out[index].children = children;
}

/// `const ( A, B = 1, 2 )` — one symbol per `name` field.
fn emit_each_name(
    walker: &mut Walker<'_>,
    node: Node<'_>,
    kind: SymbolKind,
    container: Option<&str>,
    out: &mut Vec<Symbol>,
) {
    let mut cursor = node.walk();
    let names: Vec<Node<'_>> = node.children_by_field_name("name", &mut cursor).collect();

    if names.is_empty() {
        // An embedded struct field — `type A struct { B }` — has no `name` field at all. Named
        // from its type, because a row with no text is worse than a row named after what it
        // embeds, and that is what the user sees in the source anyway.
        if let Some(ty) = node.child_by_field_name("type") {
            let text = walker.collapsed(ty.start_byte(), ty.end_byte());
            walker.push(
                out,
                Emit {
                    kind,
                    name: text,
                    detail: None,
                    container,
                    node,
                    name_node: Some(ty),
                },
            );
        }
        return;
    }

    for name in names {
        let text = walker.text(name).to_string();
        walker.push(
            out,
            Emit {
                kind,
                name: text,
                detail: None,
                container,
                node,
                name_node: Some(name),
            },
        );
    }
}

/// The type inside a receiver's `(s *Server)`, skipping the binding name.
fn receiver_type(receiver: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = receiver.walk();
    let param = receiver
        .named_children(&mut cursor)
        .find(|c| c.kind() == "parameter_declaration")?;
    param.child_by_field_name("type")
}

/// `func Serve(addr string) error` — the declaration without its body.
fn signature(walker: &Walker<'_>, node: Node<'_>) -> Option<String> {
    let end = node
        .child_by_field_name("body")
        .map_or(node.end_byte(), |b| b.start_byte());
    let text = walker.collapsed(node.start_byte(), end);
    let text = text.trim_end_matches(['{', ' ']).trim_end();
    (!text.is_empty()).then(|| text.to_string())
}
