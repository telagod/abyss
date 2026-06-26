use crate::graph::extractor::{
    LanguageRefExtractor, RawReference, RefKind, build_scope_map, default_scope_name,
};
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Tree};

pub struct JavaExtractor;

impl LanguageRefExtractor for JavaExtractor {
    fn extract(&self, tree: &Tree, source: &str) -> Vec<RawReference> {
        let mut refs = Vec::new();
        let scope_map = build_scope_map(
            &tree.root_node(),
            source,
            &["method_declaration", "constructor_declaration"],
            default_scope_name,
        );
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
                    target_name: path.clone(),
                    target_qualifier: None,
                    receiver_type: None,
                    kind: RefKind::Import,
                });
                // `import com.foo.Bar` binds the simple name Bar. Wildcard
                // imports (`com.foo.*`) bind nothing nameable — skipped.
                if let Some(simple) = path.rsplit('.').next()
                    && simple != "*"
                    && !simple.is_empty()
                {
                    refs.push(RawReference {
                        line,
                        source_symbol: None,
                        target_name: simple.to_string(),
                        target_qualifier: Some(path.clone()),
                        receiver_type: None,
                        kind: RefKind::ImportBinding,
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
