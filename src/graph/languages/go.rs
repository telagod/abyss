use std::path::{Path, PathBuf};
use tree_sitter::{Node, Tree};

use crate::graph::extractor::{
    LanguageRefExtractor, RawReference, RefKind, VarTypes, build_scope_map, default_scope_name,
};

pub struct GoExtractor;

impl LanguageRefExtractor for GoExtractor {
    fn extract(&self, tree: &Tree, source: &str) -> Vec<RawReference> {
        let root = tree.root_node();
        let scope_map = build_scope_map(
            &root,
            source,
            &["function_declaration", "method_declaration"],
            |node, src| {
                default_scope_name(node, src)
                    .or_else(|| Some(format!("anon_L{}", node.start_position().row)))
            },
        );
        let mut refs = Vec::new();
        collect_refs(&root, source, &scope_map, &VarTypes::new(), &mut refs);
        refs
    }

    fn is_test_file(&self, path: &str) -> bool {
        path.ends_with("_test.go")
    }

    fn resolve_import(&self, import_path: &str, workspace: &Path) -> Option<PathBuf> {
        // Strip quotes
        let clean = import_path.trim_matches('"');
        // Standard library: no dots in first segment
        let first_seg = clean.split('/').next().unwrap_or("");
        if !first_seg.contains('.') {
            return None; // stdlib, don't resolve
        }
        // Try to find local path: strip module prefix, map to workspace
        // e.g. "github.com/user/repo/internal/service" → workspace/internal/service
        // Heuristic: find longest suffix that exists as a directory
        let segments: Vec<&str> = clean.split('/').collect();
        for start in 0..segments.len() {
            let local = segments[start..].join("/");
            let candidate = workspace.join(&local);
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
        None
    }

    fn language_name(&self) -> &'static str {
        "go"
    }
}

fn collect_refs(
    node: &Node,
    source: &str,
    scope_map: &[Option<String>],
    var_types: &VarTypes,
    refs: &mut Vec<RawReference>,
) {
    let kind = node.kind();

    // Function boundary: build this function's var-type map (inheriting the
    // enclosing one — closures capture) and recurse with it.
    if kind == "function_declaration" || kind == "method_declaration" || kind == "func_literal" {
        let mut vt = var_types.clone();
        harvest_fn_var_types(node, source, &mut vt);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            collect_refs(&child, source, scope_map, &vt, refs);
        }
        return;
    }

    match kind {
        "call_expression" => {
            if let Some(func_node) = node.child_by_field_name("function") {
                let line = node.start_position().row as u32;
                let enclosing = scope_map.get(line as usize).and_then(|s| s.clone());

                match func_node.kind() {
                    "identifier" => {
                        // Direct call: foo()
                        let name = text_of(&func_node, source);
                        if !is_builtin_go(&name) {
                            refs.push(RawReference {
                                line,
                                source_symbol: enclosing,
                                target_name: name,
                                target_qualifier: None,
                                receiver_type: None,
                                kind: RefKind::Call,
                            });
                        }
                    }
                    "selector_expression" => {
                        // Qualified call: pkg.Func() or obj.Method()
                        if let (Some(operand), Some(field)) = (
                            func_node.child_by_field_name("operand"),
                            func_node.child_by_field_name("field"),
                        ) {
                            let qualifier = text_of(&operand, source);
                            let method = text_of(&field, source);
                            // Operand naming a known local/param/receiver is a
                            // typed method call, not a package-qualified call.
                            let receiver_type = if operand.kind() == "identifier" {
                                var_types.get(&qualifier).cloned()
                            } else {
                                None
                            };
                            refs.push(RawReference {
                                line,
                                source_symbol: enclosing,
                                target_name: method,
                                target_qualifier: Some(qualifier),
                                receiver_type,
                                kind: RefKind::Call,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }

        "type_identifier" => {
            let name = text_of(node, source);
            let line = node.start_position().row as u32;
            let enclosing = scope_map.get(line as usize).and_then(|s| s.clone());
            // Skip common built-in types
            if !is_builtin_type_go(&name) {
                refs.push(RawReference {
                    line,
                    source_symbol: enclosing,
                    target_name: name,
                    target_qualifier: None,
                    receiver_type: None,
                    kind: RefKind::TypeRef,
                });
            }
        }

        "qualified_type" => {
            if let (Some(pkg), Some(name_node)) = (
                node.child_by_field_name("package"),
                node.child_by_field_name("name"),
            ) {
                let line = node.start_position().row as u32;
                let enclosing = scope_map.get(line as usize).and_then(|s| s.clone());
                refs.push(RawReference {
                    line,
                    source_symbol: enclosing,
                    target_name: text_of(&name_node, source),
                    target_qualifier: Some(text_of(&pkg, source)),
                    receiver_type: None,
                    kind: RefKind::TypeRef,
                });
            }
        }

        "import_spec" => {
            // import "path/to/pkg" or alias "path/to/pkg"
            if let Some(path_node) = node.child_by_field_name("path") {
                let import_path = text_of(&path_node, source);
                let line = node.start_position().row as u32;
                refs.push(RawReference {
                    line,
                    source_symbol: None,
                    target_name: import_path.trim_matches('"').to_string(),
                    target_qualifier: None,
                    receiver_type: None,
                    kind: RefKind::Import,
                });
            }
        }

        _ => {}
    }

    // Recurse into children
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_refs(&child, source, scope_map, var_types, refs);
    }
}

/// Collect receiver, parameters, and local declarations of a function node
/// into the var-type map. Skips nested func_literals — they harvest their own.
fn harvest_fn_var_types(fn_node: &Node, source: &str, vt: &mut VarTypes) {
    // Method receiver: func (c *Context) M(...)
    if let Some(recv) = fn_node.child_by_field_name("receiver") {
        harvest_parameter_list(&recv, source, vt);
    }
    // Parameters: func f(e *Engine, n int)
    if let Some(params) = fn_node.child_by_field_name("parameters") {
        harvest_parameter_list(&params, source, vt);
    }
    // Body locals
    if let Some(body) = fn_node.child_by_field_name("body") {
        harvest_body_decls(&body, source, vt);
    }
}

fn harvest_parameter_list(list: &Node, source: &str, vt: &mut VarTypes) {
    let mut cursor = list.walk();
    for param in list.children(&mut cursor) {
        let pk = param.kind();
        if pk != "parameter_declaration" && pk != "variadic_parameter_declaration" {
            continue;
        }
        let Some(ty) = param
            .child_by_field_name("type")
            .and_then(|t| base_type_name(&t, source))
        else {
            continue;
        };
        // One declaration can bind several names: func f(a, b *T)
        let mut pc = param.walk();
        for child in param.children(&mut pc) {
            if child.kind() == "identifier" {
                vt.insert(text_of(&child, source), ty.clone());
            }
        }
    }
}

fn harvest_body_decls(node: &Node, source: &str, vt: &mut VarTypes) {
    let kind = node.kind();
    if kind == "func_literal" {
        return; // nested function harvests its own scope
    }

    match kind {
        // x := T{...} / x := &T{...} / x := NewT(...)
        "short_var_declaration" => {
            if let (Some(left), Some(right)) = (
                node.child_by_field_name("left"),
                node.child_by_field_name("right"),
            ) {
                let names: Vec<Node> = named_children_of_kind(&left, "identifier");
                let exprs: Vec<Node> = named_children(&right);
                if names.len() == exprs.len() {
                    for (name, expr) in names.iter().zip(exprs.iter()) {
                        if let Some(ty) = infer_expr_type(expr, source) {
                            vt.insert(text_of(name, source), ty);
                        }
                    }
                }
            }
        }
        // var x T / var x = T{...}
        "var_spec" => {
            let ty = node
                .child_by_field_name("type")
                .and_then(|t| base_type_name(&t, source))
                .or_else(|| {
                    node.child_by_field_name("value")
                        .and_then(|v| named_children(&v).into_iter().next())
                        .and_then(|e| infer_expr_type(&e, source))
                });
            if let Some(ty) = ty {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "identifier" {
                        vt.insert(text_of(&child, source), ty.clone());
                    }
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        harvest_body_decls(&child, source, vt);
    }
}

/// Static type of an initializer expression, lite rules only.
fn infer_expr_type(expr: &Node, source: &str) -> Option<String> {
    match expr.kind() {
        // T{...} / pkg.T{...}
        "composite_literal" => expr
            .child_by_field_name("type")
            .and_then(|t| base_type_name(&t, source)),
        // &T{...}
        "unary_expression" => {
            let operand = expr.child_by_field_name("operand")?;
            infer_expr_type(&operand, source)
        }
        // NewT(...) / pkg.NewT(...) → T (constructor naming convention)
        "call_expression" => {
            let func = expr.child_by_field_name("function")?;
            let fname = match func.kind() {
                "identifier" => text_of(&func, source),
                "selector_expression" => text_of(&func.child_by_field_name("field")?, source),
                _ => return None,
            };
            let stripped = fname.strip_prefix("New")?;
            if stripped.is_empty() || !stripped.starts_with(char::is_uppercase) {
                return None; // bare New() / newFoo — too weak a signal
            }
            Some(stripped.to_string())
        }
        _ => None,
    }
}

/// Base (bare) type name of a Go type node: strips pointers, generics, and
/// package qualifiers. Containers/maps/funcs yield None — method calls on
/// those don't resolve to in-repo owners.
fn base_type_name(ty: &Node, source: &str) -> Option<String> {
    match ty.kind() {
        "type_identifier" => {
            let name = text_of(ty, source);
            if is_builtin_type_go(&name) {
                None
            } else {
                Some(name)
            }
        }
        "qualified_type" => ty
            .child_by_field_name("name")
            .and_then(|n| base_type_name(&n, source)),
        "pointer_type" | "generic_type" | "parenthesized_type" => {
            // unwrap to inner type node
            let inner = ty
                .child_by_field_name("type")
                .or_else(|| named_children(ty).into_iter().next())?;
            base_type_name(&inner, source)
        }
        _ => None,
    }
}

/// Receiver base type of a Go method_declaration — shared with the chunker so
/// symbol definitions record their owner type (symbols.scope).
pub fn go_method_receiver_type(method_node: &Node, source: &str) -> Option<String> {
    let recv = method_node.child_by_field_name("receiver")?;
    let mut cursor = recv.walk();
    for param in recv.children(&mut cursor) {
        if param.kind() == "parameter_declaration"
            && let Some(ty) = param.child_by_field_name("type")
        {
            return base_type_name(&ty, source);
        }
    }
    None
}

fn named_children<'a>(node: &Node<'a>) -> Vec<Node<'a>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

fn named_children_of_kind<'a>(node: &Node<'a>, kind: &str) -> Vec<Node<'a>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|c| c.kind() == kind)
        .collect()
}

fn text_of(node: &Node, source: &str) -> String {
    source[node.start_byte()..node.end_byte()].to_string()
}

fn is_builtin_go(name: &str) -> bool {
    matches!(
        name,
        "len"
            | "cap"
            | "make"
            | "new"
            | "append"
            | "copy"
            | "delete"
            | "close"
            | "panic"
            | "recover"
            | "print"
            | "println"
            | "complex"
            | "real"
            | "imag"
    )
}

fn is_builtin_type_go(name: &str) -> bool {
    matches!(
        name,
        "int"
            | "int8"
            | "int16"
            | "int32"
            | "int64"
            | "uint"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "uintptr"
            | "float32"
            | "float64"
            | "complex64"
            | "complex128"
            | "string"
            | "bool"
            | "byte"
            | "rune"
            | "error"
            | "any"
            | "interface"
            | "struct"
            | "map"
            | "chan"
            | "func"
    )
}
