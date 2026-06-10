use crate::graph::extractor::{LanguageRefExtractor, RawReference, RefKind};
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Tree};

pub struct RustExtractor;

impl LanguageRefExtractor for RustExtractor {
    fn extract(&self, tree: &Tree, source: &str) -> Vec<RawReference> {
        let mut refs = Vec::new();
        let scope_map = build_scope_map(&tree.root_node(), source);
        collect_refs(&tree.root_node(), source, &scope_map, &mut refs);
        refs
    }

    fn is_test_file(&self, path: &str) -> bool {
        path.contains("/tests/") || path.ends_with("_test.rs")
    }

    fn resolve_import(&self, _import_path: &str, _workspace: &Path) -> Option<PathBuf> {
        None
    }
    fn language_name(&self) -> &'static str {
        "rust"
    }
}

fn build_scope_map(root: &Node, source: &str) -> Vec<Option<String>> {
    let line_count = source.lines().count();
    let mut map: Vec<Option<String>> = vec![None; line_count + 1];

    fn walk(node: &Node, source: &str, current: &Option<String>, map: &mut Vec<Option<String>>) {
        let kind = node.kind();
        let name = if kind == "function_item" || kind == "impl_item" || kind == "trait_item" {
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
        "call_expression" => {
            if let Some(func) = node.child_by_field_name("function") {
                let name = text(&func, source);
                // Split path: foo::bar::baz → qualifier=foo::bar, name=baz
                if let Some(pos) = name.rfind("::") {
                    refs.push(RawReference {
                        line,
                        source_symbol: enclosing.clone(),
                        target_name: name[pos + 2..].to_string(),
                        target_qualifier: Some(name[..pos].to_string()),
                        receiver_type: None,
                        kind: RefKind::Call,
                    });
                } else if let Some(pos) = name.rfind('.') {
                    // Method call: receiver.method() — qualifier is the receiver expr
                    refs.push(RawReference {
                        line,
                        source_symbol: enclosing.clone(),
                        target_name: name[pos + 1..].to_string(),
                        target_qualifier: Some(name[..pos].to_string()),
                        receiver_type: None,
                        kind: RefKind::Call,
                    });
                } else if !is_builtin_rust(&name) {
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
        }
        "type_identifier" => {
            let name = text(node, source);
            if !is_builtin_rust_type(&name) {
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
        "use_declaration" => {
            let path = text(node, source)
                .replace("use ", "")
                .replace(';', "")
                .trim()
                .to_string();
            refs.push(RawReference {
                line,
                source_symbol: None,
                target_name: path,
                target_qualifier: None,
                receiver_type: None,
                kind: RefKind::Import,
            });
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

fn is_builtin_rust(name: &str) -> bool {
    matches!(
        name,
        "println"
            | "print"
            | "eprintln"
            | "eprint"
            | "format"
            | "write"
            | "writeln"
            | "vec"
            | "panic"
            | "todo"
            | "unimplemented"
            | "unreachable"
            | "assert"
            | "assert_eq"
            | "assert_ne"
            | "debug_assert"
            | "dbg"
            | "cfg"
            | "include"
            | "include_str"
            | "include_bytes"
            | "Some"
            | "None"
            | "Ok"
            | "Err"
    )
}

fn is_builtin_rust_type(name: &str) -> bool {
    matches!(
        name,
        "u8" | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "f32"
            | "f64"
            | "bool"
            | "char"
            | "str"
            | "String"
            | "Vec"
            | "Box"
            | "Rc"
            | "Arc"
            | "Option"
            | "Result"
            | "HashMap"
            | "HashSet"
            | "BTreeMap"
            | "BTreeSet"
            | "Self"
            | "Sized"
            | "Send"
            | "Sync"
            | "Copy"
            | "Clone"
            | "Debug"
            | "Display"
            | "Default"
            | "Iterator"
            | "IntoIterator"
            | "From"
            | "Into"
            | "TryFrom"
            | "TryInto"
            | "AsRef"
            | "AsMut"
            | "Fn"
            | "FnMut"
            | "FnOnce"
            | "Future"
            | "Pin"
            | "Cow"
            | "PhantomData"
    )
}
