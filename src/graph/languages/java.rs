use crate::graph::extractor::{LanguageRefExtractor, RawReference, RefKind};
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Tree};

pub struct JavaExtractor;

impl LanguageRefExtractor for JavaExtractor {
    fn extract(&self, tree: &Tree, source: &str) -> Vec<RawReference> {
        let mut refs = Vec::new();
        let scope_map = build_scope_map(&tree.root_node(), source);
        collect_refs(&tree.root_node(), source, &scope_map, &mut refs);
        refs
    }

    fn is_test_file(&self, path: &str) -> bool {
        path.contains("/src/test/")
            || path.contains("/test/")
            || path.ends_with("Test.java")
            || path.ends_with("Tests.java")
            || path.ends_with("IT.java")
    }

    fn resolve_import(&self, _import_path: &str, _workspace: &Path) -> Option<PathBuf> {
        None
    }

    fn language_name(&self) -> &'static str {
        "java"
    }
}

/// Map each line to the enclosing method/constructor/class name.
fn build_scope_map(root: &Node, source: &str) -> Vec<Option<String>> {
    let line_count = source.lines().count();
    let mut map: Vec<Option<String>> = vec![None; line_count + 1];

    fn walk(node: &Node, source: &str, current: &Option<String>, map: &mut Vec<Option<String>>) {
        let kind = node.kind();
        // Only methods/constructors name a scope: including class_declaration
        // would claim every line of the class body before methods are visited.
        let name = if matches!(kind, "method_declaration" | "constructor_declaration") {
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
        "method_invocation" => {
            // obj.method(args) → qualifier = obj; method(args) → no qualifier
            if let Some(name_node) = node.child_by_field_name("name") {
                let qualifier = node.child_by_field_name("object").map(|o| text(&o, source));
                refs.push(RawReference {
                    line,
                    source_symbol: enclosing.clone(),
                    target_name: text(&name_node, source),
                    target_qualifier: qualifier,
                    receiver_type: None,
                    kind: RefKind::Call,
                });
            }
        }
        "object_creation_expression" => {
            // new Foo(...) → constructor call on the type name
            if let Some(type_node) = node.child_by_field_name("type") {
                let name = text(&type_node, source);
                // Generic types: new ArrayList<String>() → ArrayList
                let bare = name.split('<').next().unwrap_or(&name).to_string();
                if !is_builtin_java_type(&bare) {
                    refs.push(RawReference {
                        line,
                        source_symbol: enclosing.clone(),
                        target_name: bare,
                        target_qualifier: None,
                        receiver_type: None,
                        kind: RefKind::Call,
                    });
                }
            }
        }
        "type_identifier" => {
            let name = text(node, source);
            if !is_builtin_java_type(&name) {
                refs.push(RawReference {
                    line,
                    source_symbol: enclosing.clone(),
                    target_name: name,
                    target_qualifier: None,
                    receiver_type: None,
                    kind: RefKind::TypeRef,
                });
            }
        }
        "import_declaration" => {
            // import [static] com.foo.Bar; → com.foo.Bar
            let raw = text(node, source);
            let path = raw
                .trim_start_matches("import")
                .trim_start_matches(char::is_whitespace)
                .trim_start_matches("static")
                .trim()
                .trim_end_matches(';')
                .trim()
                .to_string();
            if !path.is_empty() {
                refs.push(RawReference {
                    line,
                    source_symbol: None,
                    target_name: path,
                    target_qualifier: None,
                    receiver_type: None,
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

fn is_builtin_java_type(name: &str) -> bool {
    matches!(
        name,
        "String"
            | "Object"
            | "Integer"
            | "Long"
            | "Short"
            | "Byte"
            | "Double"
            | "Float"
            | "Boolean"
            | "Character"
            | "Void"
            | "Number"
            | "Math"
            | "System"
            | "Thread"
            | "Runnable"
            | "Exception"
            | "RuntimeException"
            | "IllegalArgumentException"
            | "IllegalStateException"
            | "NullPointerException"
            | "UnsupportedOperationException"
            | "Throwable"
            | "Error"
            | "List"
            | "ArrayList"
            | "LinkedList"
            | "Map"
            | "HashMap"
            | "LinkedHashMap"
            | "TreeMap"
            | "Set"
            | "HashSet"
            | "TreeSet"
            | "Collection"
            | "Collections"
            | "Arrays"
            | "Iterator"
            | "Iterable"
            | "Optional"
            | "Stream"
            | "Comparable"
            | "Comparator"
            | "StringBuilder"
            | "StringBuffer"
            | "CharSequence"
    )
}
