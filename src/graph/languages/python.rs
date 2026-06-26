use crate::graph::extractor::{
    LanguageRefExtractor, RawReference, RefKind, VarTypes, build_scope_map, default_scope_name,
};
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Tree};

pub struct PythonExtractor;

impl LanguageRefExtractor for PythonExtractor {
    fn extract(&self, tree: &Tree, source: &str) -> Vec<RawReference> {
        let mut refs = Vec::new();
        let root = tree.root_node();
        let scope_map = build_scope_map(
            &root,
            source,
            &["function_definition", "class_definition"],
            default_scope_name,
        );
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
            // Emit type_ref edges for each typed parameter, mirroring
            // Go/TS/Rust/Java/C++ which already surface typed params as
            // first-class type evidence. Cross-language symmetry surfaced
            // by the v0.5.1 docs-bundle agent.
            emit_param_type_refs(&params, source, &enclosing, refs);
        }
        // `def f() -> SomeType:` — the return-type annotation. tree-sitter
        // node field is `return_type`; the value beneath is a `type` node.
        if let Some(ret) = node.child_by_field_name("return_type") {
            emit_type_refs_from_annotation(&ret, source, &enclosing, refs);
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

        // Inheritance: `class Sub(Base, OtherBase):` → one Inherit ref per
        // base. Feeds the MRO walker in the resolver (L0e tier). Skip
        // `metaclass=...` / kwarg bases. For parameterized generics
        // (`Base[T]`), the base is a `subscript` node — unwrap its `value`
        // (the base name, before `[`) and treat it as the real base, but
        // skip typing-system markers (`Generic[T]`, `Protocol[T]`, …)
        // which aren't real superclasses. `attribute` bases (`click.Command`)
        // reuse the same import-binding path as call qualifiers.
        if let Some(bases) = node.child_by_field_name("superclasses") {
            let mut bc = bases.walk();
            for base in bases.named_children(&mut bc) {
                let resolved = match base.kind() {
                    "identifier" => Some((text(&base, source), None, base)),
                    "attribute" => extract_attribute_base(&base, source).map(|(n, q)| (n, q, base)),
                    "subscript" => {
                        // `Base[T]` → `value` is the base name (identifier
                        // or attribute), `slice` is the type args. Recurse
                        // into the value node, then apply the typing-marker
                        // denylist so `Generic[T]` / `Protocol[T]` / …
                        // don't leak into the inheritance graph.
                        base.child_by_field_name("value")
                            .and_then(|v| match v.kind() {
                                "identifier" => {
                                    let name = text(&v, source);
                                    if is_typing_marker(&name) {
                                        None
                                    } else {
                                        Some((name, None, v))
                                    }
                                }
                                "attribute" => {
                                    let (name, qualifier) = extract_attribute_base(&v, source)?;
                                    if is_typing_marker(&name) {
                                        None
                                    } else {
                                        Some((name, qualifier, v))
                                    }
                                }
                                _ => None,
                            })
                    }
                    // `metaclass=...`, comments → skip.
                    _ => None,
                };
                let Some((name, qualifier, anchor)) = resolved else {
                    continue;
                };
                if name.is_empty() || is_builtin_py_type(&name) {
                    continue;
                }
                refs.push(RawReference {
                    line: anchor.start_position().row as u32,
                    source_symbol: class_name.clone(),
                    target_name: name,
                    target_qualifier: qualifier,
                    receiver_type: None,
                    kind: RefKind::Inherit,
                });
            }
        }

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
        // `x: T = ...` (typed assignment) and `y: B` (class field annotation,
        // which the tree-sitter-python grammar also parses as an `assignment`
        // node with `type` field but no `right`). Both flavors must emit a
        // type_ref edge for first-class type-evidence — cross-language
        // symmetry with the Go/TS/Rust/Java/C++ extractors. Plain
        // `x = something` (no annotation) goes through the existing
        // var_types harvest path only, no type_ref.
        "assignment" => {
            if let Some(ann) = node.child_by_field_name("type") {
                emit_type_refs_from_annotation(&ann, source, &enclosing, refs);
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

/// Emit type_ref edges for each typed parameter under a `parameters` node.
/// Walks `typed_parameter` and `typed_default_parameter` shapes; ignores
/// untyped params. The annotation is fed through the same recursive
/// emitter used elsewhere so subscripted/attribute/typing-marker
/// handling stays consistent.
fn emit_param_type_refs(
    params: &Node,
    source: &str,
    enclosing: &Option<String>,
    refs: &mut Vec<RawReference>,
) {
    let mut cursor = params.walk();
    for param in params.named_children(&mut cursor) {
        let ty = match param.kind() {
            "typed_parameter" | "typed_default_parameter" => param.child_by_field_name("type"),
            _ => None,
        };
        if let Some(ty) = ty {
            emit_type_refs_from_annotation(&ty, source, enclosing, refs);
        }
    }
}

/// Walk a type annotation and emit one `RefKind::TypeRef` per real type
/// name encountered. Primitives and typing-system markers are dropped;
/// subscripted forms (`List[Foo]`, `Base[T]`) are unwrapped per the B3
/// generic-base convention — typing-marker wrappers recurse into the
/// type args (`List[Foo]` → emit Foo), real bases collapse to the base
/// name (`Base[T]` → emit Base). Bounded recursion (no fixpoint) since
/// Python type annotations are shallow in practice.
fn emit_type_refs_from_annotation(
    node: &Node,
    source: &str,
    enclosing: &Option<String>,
    refs: &mut Vec<RawReference>,
) {
    // `type` nodes wrap the actual expression; unwrap one level.
    let inner = if node.kind() == "type" {
        match node.named_child(0) {
            Some(n) => n,
            None => return,
        }
    } else {
        *node
    };
    let line = inner.start_position().row as u32;
    match inner.kind() {
        "identifier" => {
            let name = text(&inner, source);
            if name.is_empty() || is_builtin_py_type(&name) || is_typing_marker(&name) {
                return;
            }
            refs.push(RawReference {
                line,
                source_symbol: enclosing.clone(),
                target_name: name,
                target_qualifier: None,
                receiver_type: None,
                kind: RefKind::TypeRef,
            });
        }
        "attribute" => {
            if let Some((name, qualifier)) = extract_attribute_base(&inner, source) {
                if name.is_empty() || is_builtin_py_type(&name) || is_typing_marker(&name) {
                    return;
                }
                refs.push(RawReference {
                    line,
                    source_symbol: enclosing.clone(),
                    target_name: name,
                    target_qualifier: qualifier,
                    receiver_type: None,
                    kind: RefKind::TypeRef,
                });
            }
        }
        "string" => {
            // Forward references: `x: "Foo"` → emit Foo if not primitive.
            let raw = text(&inner, source);
            let name = raw.trim_matches(|c| c == '"' || c == '\'').to_string();
            if name.is_empty() || is_builtin_py_type(&name) || is_typing_marker(&name) {
                return;
            }
            // Forward refs may contain dotted paths or subscripts; only
            // emit when it's a bare identifier. Anything fancier is
            // dropped to avoid mis-parsing string contents.
            if name.chars().all(|c| c.is_alphanumeric() || c == '_') {
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
        "generic_type" => {
            // `Wrapper[T]` in type-annotation position — tree-sitter-python
            // wraps these as a `generic_type` node whose first child is the
            // base name (identifier or attribute) and second child is a
            // `type_parameter` node containing the type args (each as a
            // nested `type` node).
            //
            // Behavior: if the base is a typing marker (List, Dict,
            // Optional, Union, …) recurse into the type args so
            // `List[Foo]` surfaces `Foo` not `List`. If the base is a real
            // type, collapse to it (`Base[T]` → `Base`), matching the B3
            // inheritance unwrap.
            let mut cursor = inner.walk();
            let mut base_node = None;
            let mut type_params = Vec::new();
            for child in inner.named_children(&mut cursor) {
                match child.kind() {
                    "identifier" | "attribute" if base_node.is_none() => base_node = Some(child),
                    "type_parameter" => type_params.push(child),
                    _ => {}
                }
            }
            let base_name = base_node.and_then(|b| match b.kind() {
                "identifier" => Some(text(&b, source)),
                "attribute" => b.child_by_field_name("attribute").map(|a| text(&a, source)),
                _ => None,
            });
            let is_marker = base_name.as_deref().is_some_and(is_typing_marker);
            if is_marker {
                // Recurse into the type_parameter's nested `type` children.
                for tp in &type_params {
                    let mut tc = tp.walk();
                    for arg in tp.named_children(&mut tc) {
                        emit_type_refs_from_annotation(&arg, source, enclosing, refs);
                    }
                }
            } else if let Some(b) = base_node {
                emit_type_refs_from_annotation(&b, source, enclosing, refs);
            }
        }
        "subscript" => {
            // Subscript shape (e.g. `x[0]` accidentally appearing in a
            // type-annotation context, or older grammars) — apply the
            // same marker-vs-real logic via the `value` field.
            let value_node = inner.child_by_field_name("value");
            let value_name = value_node.and_then(|v| match v.kind() {
                "identifier" => Some(text(&v, source)),
                "attribute" => v.child_by_field_name("attribute").map(|a| text(&a, source)),
                _ => None,
            });
            let is_marker = value_name.as_deref().is_some_and(is_typing_marker);
            if is_marker {
                let mut sc = inner.walk();
                for child in inner.named_children(&mut sc) {
                    if child.id() == value_node.map(|v| v.id()).unwrap_or(0) {
                        continue;
                    }
                    emit_type_refs_from_annotation(&child, source, enclosing, refs);
                }
            } else if let Some(v) = value_node {
                emit_type_refs_from_annotation(&v, source, enclosing, refs);
            }
        }
        _ => {}
    }
}

/// `obj.Attr` → (Attr, Some(obj)). Falls back to (Attr, None) when the
/// receiver isn't a bare identifier (dotted paths, calls, etc.).
fn extract_attribute_base(attr_node: &Node, source: &str) -> Option<(String, Option<String>)> {
    let attr = attr_node.child_by_field_name("attribute")?;
    let obj = attr_node.child_by_field_name("object");
    let qualifier = match obj {
        Some(o) if o.kind() == "identifier" => Some(text(&o, source)),
        _ => None,
    };
    Some((text(&attr, source), qualifier))
}

/// PEP 484 / typing module markers that appear in base-class position but
/// aren't real superclasses (`class Foo(Generic[T])` declares that Foo is
/// generic, not that it inherits from Generic). Treating these as real
/// inherits pollutes the MRO walker with phantom edges and can flip
/// resolver tiers.
fn is_typing_marker(name: &str) -> bool {
    matches!(
        name,
        "Generic"
            | "Protocol"
            | "TypeVar"
            | "Callable"
            | "Union"
            | "Optional"
            | "List"
            | "Dict"
            | "Tuple"
            | "Set"
            | "FrozenSet"
            | "Type"
            | "ClassVar"
            | "Final"
            | "Annotated"
            | "Literal"
    )
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
