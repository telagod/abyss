use crate::graph::extractor::{LanguageRefExtractor, RawReference, RefKind};
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Tree};

pub struct PythonExtractor;

impl LanguageRefExtractor for PythonExtractor {
    fn extract(&self, tree: &Tree, source: &str) -> Vec<RawReference> {
        let mut refs = Vec::new();
        let scope_map = build_scope_map(&tree.root_node(), source);
        collect_refs(&tree.root_node(), source, &scope_map, &mut refs);
        refs
    }

    fn is_test_file(&self, path: &str) -> bool {
        let name = path.rsplit('/').next().unwrap_or(path);
        name.starts_with("test_") || name.ends_with("_test.py") || path.contains("/tests/")
    }

    fn resolve_import(&self, _import_path: &str, _workspace: &Path) -> Option<PathBuf> {
        None
    }
    fn language_name(&self) -> &'static str {
        "python"
    }
}

fn build_scope_map(root: &Node, source: &str) -> Vec<Option<String>> {
    let line_count = source.lines().count();
    let mut map: Vec<Option<String>> = vec![None; line_count + 1];

    fn walk(node: &Node, source: &str, current: &Option<String>, map: &mut Vec<Option<String>>) {
        let name = if node.kind() == "function_definition" || node.kind() == "class_definition" {
            node.child_by_field_name("name")
                .map(|n| source[n.start_byte()..n.end_byte()].to_string())
        } else {
            None
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
        "call" => {
            if let Some(func) = node.child_by_field_name("function") {
                match func.kind() {
                    "identifier" => {
                        let name = text(&func, source);
                        if !is_builtin_py(&name) {
                            refs.push(RawReference {
                                line,
                                source_symbol: enclosing.clone(),
                                target_name: name,
                                target_qualifier: None,
                                receiver_type: None,
                                kind: RefKind::Call,
                            });
                        }
                    }
                    "attribute" => {
                        if let (Some(obj), Some(attr)) = (
                            func.child_by_field_name("object"),
                            func.child_by_field_name("attribute"),
                        ) {
                            refs.push(RawReference {
                                line,
                                source_symbol: enclosing.clone(),
                                target_name: text(&attr, source),
                                target_qualifier: Some(text(&obj, source)),
                                receiver_type: None,
                                kind: RefKind::Call,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        "import_from_statement" => {
            if let Some(module) = node.child_by_field_name("module_name") {
                refs.push(RawReference {
                    line,
                    source_symbol: None,
                    target_name: text(&module, source),
                    target_qualifier: None,
                    receiver_type: None,
                    kind: RefKind::Import,
                });
            }
        }
        "import_statement" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "dotted_name" {
                    refs.push(RawReference {
                        line,
                        source_symbol: None,
                        target_name: text(&child, source),
                        target_qualifier: None,
                        receiver_type: None,
                        kind: RefKind::Import,
                    });
                }
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

fn is_builtin_py(name: &str) -> bool {
    matches!(
        name,
        "print"
            | "len"
            | "range"
            | "int"
            | "str"
            | "float"
            | "bool"
            | "list"
            | "dict"
            | "set"
            | "tuple"
            | "type"
            | "isinstance"
            | "issubclass"
            | "hasattr"
            | "getattr"
            | "setattr"
            | "delattr"
            | "super"
            | "property"
            | "staticmethod"
            | "classmethod"
            | "enumerate"
            | "zip"
            | "map"
            | "filter"
            | "sorted"
            | "reversed"
            | "min"
            | "max"
            | "sum"
            | "abs"
            | "round"
            | "open"
            | "input"
            | "repr"
            | "id"
            | "hash"
            | "callable"
            | "iter"
            | "next"
            | "vars"
            | "dir"
            | "Exception"
            | "ValueError"
            | "TypeError"
            | "KeyError"
            | "IndexError"
            | "AttributeError"
            | "RuntimeError"
            | "StopIteration"
            | "NotImplementedError"
            | "OSError"
            | "IOError"
            | "True"
            | "False"
            | "None"
    )
}
