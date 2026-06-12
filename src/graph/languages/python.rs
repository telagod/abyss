use crate::graph::extractor::{LanguageRefExtractor, RawReference, RefKind};
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Tree};

pub struct PythonExtractor;

impl LanguageRefExtractor for PythonExtractor {
    fn extract(&self, tree: &Tree, source: &str) -> Vec<RawReference> {
        let mut refs = Vec::new();
        let root = tree.root_node();
        let scope_map = build_scope_map(&root, source);
        // Module-level assignments (ctx = Context(...)) seed the map for
        // everything below; nested functions inherit and extend it.
        let mut vt = VarTypes::new();
        harvest_assignments(&root, source, &mut vt);
        collect_refs(&root, source, &scope_map, &vt, None, &mut refs);
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

/// Lite per-scope variable → type map (mirrors the Go/TS extractors):
/// parameters with annotations, `x = Type(...)` constructor assignments,
/// `x: Type = ...` annotated assignments, and `self`/`cls` → enclosing class.
/// No data-flow, no Protocols, no unions.
type VarTypes = std::collections::HashMap<String, String>;

fn collect_refs(
    node: &Node,
    source: &str,
    scope_map: &[Option<String>],
    var_types: &VarTypes,
    current_class: Option<&str>,
    refs: &mut Vec<RawReference>,
) {
    let kind = node.kind();
    let line = node.start_position().row as u32;
    let enclosing = scope_map.get(line as usize).and_then(|s| s.clone());

    // Function boundary: extend the inherited map with own params + locals.
    if kind == "function_definition" {
        let mut vt = var_types.clone();
        if let Some(class) = current_class {
            vt.insert("self".into(), class.to_string());
            vt.insert("cls".into(), class.to_string());
        }
        if let Some(params) = node.child_by_field_name("parameters") {
            harvest_parameters(&params, source, &mut vt);
        }
        if let Some(body) = node.child_by_field_name("body") {
            harvest_assignments(&body, source, &mut vt);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            collect_refs(&child, source, scope_map, &vt, current_class, refs);
        }
        return;
    }

    // Class boundary: `self.m()` below resolves to this class.
    if kind == "class_definition" {
        let class_name = node.child_by_field_name("name").map(|n| text(&n, source));
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            collect_refs(
                &child,
                source,
                scope_map,
                var_types,
                class_name.as_deref().or(current_class),
                refs,
            );
        }
        return;
    }

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
                            let receiver_type = match obj.kind() {
                                "identifier" => var_types.get(&text(&obj, source)).cloned(),
                                _ => None,
                            };
                            refs.push(RawReference {
                                line,
                                source_symbol: enclosing.clone(),
                                target_name: text(&attr, source),
                                target_qualifier: Some(text(&obj, source)),
                                receiver_type,
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
                let module_path = text(&module, source);
                refs.push(RawReference {
                    line,
                    source_symbol: None,
                    target_name: module_path.clone(),
                    target_qualifier: None,
                    receiver_type: None,
                    kind: RefKind::Import,
                });
                // `from .mod import a, b as c` — each LOCAL name becomes an
                // ImportBinding pointing at the module. Wildcards bind
                // nothing nameable.
                let mut cursor = node.walk();
                for name in node.children_by_field_name("name", &mut cursor) {
                    let local = match name.kind() {
                        "dotted_name" => Some(text(&name, source)),
                        "aliased_import" => {
                            name.child_by_field_name("alias").map(|a| text(&a, source))
                        }
                        _ => None,
                    };
                    if let Some(local) = local
                        && !local.contains('.')
                        && !local.is_empty()
                    {
                        refs.push(RawReference {
                            line,
                            source_symbol: None,
                            target_name: local,
                            target_qualifier: Some(module_path.clone()),
                            receiver_type: None,
                            kind: RefKind::ImportBinding,
                        });
                    }
                }
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
        collect_refs(&child, source, scope_map, var_types, current_class, refs);
    }
}

/// Annotated parameters: `def f(ctx: Context, n: int = 0)` → ctx typed.
fn harvest_parameters(params: &Node, source: &str, vt: &mut VarTypes) {
    let mut cursor = params.walk();
    for param in params.named_children(&mut cursor) {
        let (name_node, type_node) = match param.kind() {
            // typed_parameter: identifier is the first named child, no field
            "typed_parameter" => {
                let mut pc = param.walk();
                let name = param
                    .named_children(&mut pc)
                    .find(|c| c.kind() == "identifier");
                (name, param.child_by_field_name("type"))
            }
            "typed_default_parameter" => (
                param.child_by_field_name("name"),
                param.child_by_field_name("type"),
            ),
            _ => continue,
        };
        let (Some(name), Some(ty)) = (name_node, type_node) else {
            continue;
        };
        if let Some(base) = base_py_type_name(&ty, source) {
            vt.insert(text(&name, source), base);
        }
    }
}

/// `x = Type(...)` and `x: Type = ...` assignments in a scope — skips nested
/// functions and classes, which harvest their own.
fn harvest_assignments(node: &Node, source: &str, vt: &mut VarTypes) {
    let kind = node.kind();
    if kind == "function_definition" || kind == "class_definition" {
        return;
    }
    if kind == "assignment"
        && let Some(left) = node.child_by_field_name("left")
        && left.kind() == "identifier"
    {
        // `x: Type = ...` — annotation beats constructor inference.
        let ty = if let Some(ann) = node.child_by_field_name("type") {
            base_py_type_name(&ann, source)
        } else if let Some(right) = node.child_by_field_name("right") {
            constructor_type_name(&right, source)
        } else {
            None
        };
        if let Some(ty) = ty {
            vt.insert(text(&left, source), ty);
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        harvest_assignments(&child, source, vt);
    }
}

/// `Type(...)` / `mod.Type(...)` call → Type, by the CapWord constructor
/// convention. Lowercase callees (factory functions) are not inferred.
fn constructor_type_name(value: &Node, source: &str) -> Option<String> {
    if value.kind() != "call" {
        return None;
    }
    let func = value.child_by_field_name("function")?;
    let name = match func.kind() {
        "identifier" => text(&func, source),
        "attribute" => text(&func.child_by_field_name("attribute")?, source),
        _ => return None,
    };
    if name.chars().next().is_some_and(|c| c.is_uppercase()) && !is_builtin_py(&name) {
        Some(name)
    } else {
        None
    }
}

/// Bare type name from an annotation: `Context` → Context, `"Context"`
/// (forward ref) → Context, `t.Context` → Context. Subscripted/union/complex
/// types → None.
fn base_py_type_name(annotation: &Node, source: &str) -> Option<String> {
    // `type` nodes wrap the actual expression; unwrap one level.
    let inner = if annotation.kind() == "type" {
        annotation.named_child(0)?
    } else {
        *annotation
    };
    let name = match inner.kind() {
        "identifier" => text(&inner, source),
        "attribute" => text(&inner.child_by_field_name("attribute")?, source),
        "string" => text(&inner, source)
            .trim_matches(|c| c == '"' || c == '\'')
            .to_string(),
        _ => return None,
    };
    if name.is_empty() || is_builtin_py_type(&name) {
        None
    } else {
        Some(name)
    }
}

fn text(node: &Node, source: &str) -> String {
    source[node.start_byte()..node.end_byte()].to_string()
}

fn is_builtin_py_type(name: &str) -> bool {
    matches!(
        name,
        "int"
            | "str"
            | "float"
            | "bool"
            | "bytes"
            | "list"
            | "dict"
            | "set"
            | "tuple"
            | "object"
            | "None"
            | "Any"
            | "Optional"
            | "Union"
            | "Callable"
            | "Iterator"
            | "Iterable"
            | "Sequence"
            | "Mapping"
            | "List"
            | "Dict"
            | "Set"
            | "Tuple"
    )
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
