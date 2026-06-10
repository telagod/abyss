use std::path::{Path, PathBuf};
use tree_sitter::{Node, Tree};

use crate::graph::extractor::{LanguageRefExtractor, RawReference, RefKind};

pub struct GoExtractor;

impl LanguageRefExtractor for GoExtractor {
    fn extract(&self, tree: &Tree, source: &str) -> Vec<RawReference> {
        let root = tree.root_node();
        let scope_map = build_scope_map(&root, source);
        let mut refs = Vec::new();
        collect_refs(&root, source, &scope_map, &mut refs);
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

/// Map each line to the enclosing function/method name
fn build_scope_map(root: &Node, source: &str) -> Vec<Option<String>> {
    let line_count = source.lines().count();
    let mut map: Vec<Option<String>> = vec![None; line_count + 1];

    fn walk(
        node: &Node,
        source: &str,
        current_func: &Option<String>,
        map: &mut Vec<Option<String>>,
    ) {
        let kind = node.kind();
        let func_name = if kind == "function_declaration" || kind == "method_declaration" {
            node.child_by_field_name("name")
                .map(|n| text_of(&n, source))
                .or_else(|| Some(format!("anon_L{}", node.start_position().row)))
        } else {
            None
        };

        let active = func_name.as_ref().or(current_func.as_ref());
        if let Some(name) = active {
            let start = node.start_position().row;
            let end = node.end_position().row;
            for line in start..=end.min(map.len() - 1) {
                if map[line].is_none() {
                    map[line] = Some(name.clone());
                }
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            walk(
                &child,
                source,
                &func_name.clone().or_else(|| current_func.clone()),
                map,
            );
        }
    }

    walk(root, source, &None, &mut map);
    map
}

fn collect_refs(
    node: &Node,
    source: &str,
    scope_map: &[Option<String>],
    refs: &mut Vec<RawReference>,
) {
    let kind = node.kind();

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
                            refs.push(RawReference {
                                line,
                                source_symbol: enclosing,
                                target_name: method,
                                target_qualifier: Some(qualifier),
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
                    kind: RefKind::Import,
                });
            }
        }

        _ => {}
    }

    // Recurse into children
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_refs(&child, source, scope_map, refs);
    }
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
