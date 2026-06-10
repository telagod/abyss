use crate::graph::extractor::{LanguageRefExtractor, RawReference, RefKind};
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Tree};

pub struct TypeScriptExtractor;

impl LanguageRefExtractor for TypeScriptExtractor {
    fn extract(&self, tree: &Tree, source: &str) -> Vec<RawReference> {
        let mut refs = Vec::new();
        let scope_map = build_scope_map(&tree.root_node(), source);
        collect_refs(&tree.root_node(), source, &scope_map, &mut refs);
        refs
    }

    fn is_test_file(&self, path: &str) -> bool {
        path.contains(".test.") || path.contains(".spec.") || path.contains("__tests__")
    }

    fn resolve_import(&self, import_path: &str, workspace: &Path) -> Option<PathBuf> {
        let clean = import_path.trim_matches(|c| c == '\'' || c == '"');
        if !clean.starts_with('.') {
            return None;
        }
        let candidate = workspace.join(clean);
        for ext in &[".ts", ".tsx", ".js", ".jsx", "/index.ts", "/index.js"] {
            let p = PathBuf::from(format!("{}{ext}", candidate.display()));
            if p.exists() {
                return Some(p);
            }
        }
        None
    }

    fn language_name(&self) -> &'static str {
        "typescript"
    }
}

fn build_scope_map(root: &Node, source: &str) -> Vec<Option<String>> {
    let line_count = source.lines().count();
    let mut map: Vec<Option<String>> = vec![None; line_count + 1];

    fn walk(node: &Node, source: &str, current: &Option<String>, map: &mut Vec<Option<String>>) {
        let kind = node.kind();
        let name = match kind {
            "function_declaration" | "method_definition" | "arrow_function" => node
                .child_by_field_name("name")
                .map(|n| source[n.start_byte()..n.end_byte()].to_string()),
            _ => None,
        };
        let active = name.as_ref().or(current.as_ref());
        if let Some(n) = active {
            for line in node.start_position().row..=node.end_position().row.min(map.len() - 1) {
                if map[line].is_none() {
                    map[line] = Some(n.clone());
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            walk(
                &child,
                source,
                &name.clone().or_else(|| current.clone()),
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
    let line = node.start_position().row as u32;
    let enclosing = scope_map.get(line as usize).and_then(|s| s.clone());

    match kind {
        "call_expression" => {
            if let Some(func) = node.child_by_field_name("function") {
                match func.kind() {
                    "identifier" => {
                        let name = text(&func, source);
                        if !is_builtin_js(&name) {
                            refs.push(RawReference {
                                line,
                                source_symbol: enclosing.clone(),
                                target_name: name,
                                target_qualifier: None,
                                kind: RefKind::Call,
                            });
                        }
                    }
                    "member_expression" => {
                        if let (Some(obj), Some(prop)) = (
                            func.child_by_field_name("object"),
                            func.child_by_field_name("property"),
                        ) {
                            refs.push(RawReference {
                                line,
                                source_symbol: enclosing.clone(),
                                target_name: text(&prop, source),
                                target_qualifier: Some(text(&obj, source)),
                                kind: RefKind::Call,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        "new_expression" => {
            if let Some(constructor) = node.child_by_field_name("constructor") {
                refs.push(RawReference {
                    line,
                    source_symbol: enclosing.clone(),
                    target_name: text(&constructor, source),
                    target_qualifier: None,
                    kind: RefKind::Call,
                });
            }
        }
        "type_identifier" => {
            let name = text(node, source);
            if !is_builtin_ts_type(&name) {
                refs.push(RawReference {
                    line,
                    source_symbol: enclosing.clone(),
                    target_name: name,
                    target_qualifier: None,
                    kind: RefKind::TypeRef,
                });
            }
        }
        "import_statement" => {
            if let Some(src) = node.child_by_field_name("source") {
                refs.push(RawReference {
                    line,
                    source_symbol: None,
                    target_name: text(&src, source)
                        .trim_matches(|c| c == '\'' || c == '"')
                        .to_string(),
                    target_qualifier: None,
                    kind: RefKind::Import,
                });
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_refs(&child, source, scope_map, refs);
    }
}

fn text(node: &Node, source: &str) -> String {
    source[node.start_byte()..node.end_byte()].to_string()
}

fn is_builtin_js(name: &str) -> bool {
    matches!(
        name,
        "console"
            | "require"
            | "parseInt"
            | "parseFloat"
            | "setTimeout"
            | "setInterval"
            | "clearTimeout"
            | "clearInterval"
            | "Promise"
            | "Array"
            | "Object"
            | "JSON"
            | "Math"
            | "String"
            | "Number"
            | "Boolean"
            | "Symbol"
            | "Map"
            | "Set"
            | "Date"
            | "RegExp"
            | "Error"
            | "undefined"
            | "null"
            | "NaN"
            | "Infinity"
            | "isNaN"
            | "isFinite"
            | "encodeURIComponent"
            | "decodeURIComponent"
    )
}

fn is_builtin_ts_type(name: &str) -> bool {
    matches!(
        name,
        "string"
            | "number"
            | "boolean"
            | "void"
            | "any"
            | "unknown"
            | "never"
            | "null"
            | "undefined"
            | "object"
            | "symbol"
            | "bigint"
            | "Array"
            | "Promise"
            | "Record"
            | "Partial"
            | "Required"
            | "Readonly"
            | "Pick"
            | "Omit"
            | "Exclude"
            | "Extract"
            | "ReturnType"
            | "Parameters"
            | "InstanceType"
    )
}
