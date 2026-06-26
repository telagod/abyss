use crate::graph::extractor::{
    LanguageRefExtractor, RawReference, RefKind, VarTypes, build_scope_map, default_scope_name,
};
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Tree};

pub struct RustExtractor;

impl LanguageRefExtractor for RustExtractor {
    fn extract(&self, tree: &Tree, source: &str) -> Vec<RawReference> {
        let mut refs = Vec::new();
        let root = tree.root_node();
        let scope_map = build_scope_map(
            &root,
            source,
            &["function_item", "impl_item", "trait_item"],
            default_scope_name,
        );
        let vt = VarTypes::new();
        collect_refs(&root, source, &scope_map, &vt, None, &mut refs);
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

fn collect_refs(
    node: &Node,
    source: &str,
    scope_map: &[Option<String>],
    var_types: &VarTypes,
    current_impl: Option<&str>,
    refs: &mut Vec<RawReference>,
) {
    let kind = node.kind();
    let line = node.start_position().row as u32;
    let enclosing = scope_map.get(line as usize).and_then(|s| s.clone());

    // Impl boundary: `self.m()` below resolves to this type.
    if kind == "impl_item" {
        let impl_type = node
            .child_by_field_name("type")
            .and_then(|t| base_rust_type_name(&t, source));
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            collect_refs(
                &child,
                source,
                scope_map,
                var_types,
                impl_type.as_deref().or(current_impl),
                refs,
            );
        }
        return;
    }

    // Function boundary: extend the inherited map with own params + lets.
    if kind == "function_item" || kind == "closure_expression" {
        let mut vt = var_types.clone();
        if let Some(impl_type) = current_impl {
            vt.insert("self".into(), impl_type.to_string());
            vt.insert("Self".into(), impl_type.to_string());
        }
        if let Some(params) = node.child_by_field_name("parameters") {
            harvest_rust_parameters(&params, source, &mut vt);
        }
        if let Some(body) = node.child_by_field_name("body") {
            harvest_let_decls(&body, source, &mut vt);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            collect_refs(&child, source, scope_map, &vt, current_impl, refs);
        }
        return;
    }

    match kind {
        "call_expression" => {
            if let Some(func) = node.child_by_field_name("function") {
                let name = text(&func, source);
                // Split path: foo::bar::baz → qualifier=foo::bar, name=baz
                if let Some(pos) = name.rfind("::") {
                    let qualifier = &name[..pos];
                    // Associated function: the qualifier's last segment IS
                    // the receiver type (`IndexPipeline::new()`, every type
                    // has a `new` — name tiers alone pick the wrong one).
                    //
                    // v0.5.4: when the inferred receiver type is a Rust
                    // std-lib collection (Vec, Box, HashMap, …) we drop
                    // the receiver_type hint — there is no user symbol
                    // for it, so feeding `receiver_type=Vec` to L0d
                    // would silently bind to a user-defined `struct Vec`
                    // anywhere in the workspace. The same denylist is
                    // applied at the SQL guard level for the global
                    // tiers (L2/L4/L4b/L5); dropping it here pre-empts
                    // L0d's type-name lookup.
                    let receiver_type = qualifier
                        .rsplit("::")
                        .next()
                        .map(|s| s.split('<').next().unwrap_or(s).trim())
                        .filter(|s| {
                            s.chars().next().is_some_and(|c| c.is_uppercase())
                                && s.chars().all(|c| c.is_alphanumeric() || c == '_')
                        })
                        .filter(|s| !is_rust_collection_builtin(s))
                        .map(String::from);
                    refs.push(RawReference {
                        line,
                        source_symbol: enclosing.clone(),
                        target_name: name[pos + 2..].to_string(),
                        target_qualifier: Some(qualifier.to_string()),
                        receiver_type,
                        kind: RefKind::Call,
                    });
                } else if let Some(pos) = name.rfind('.') {
                    // Method call: receiver.method() — qualifier is the
                    // receiver expr; simple identifiers (incl. self) get the
                    // lite-inferred type.
                    let receiver_expr = &name[..pos];
                    let receiver_type = var_types.get(receiver_expr).cloned();
                    refs.push(RawReference {
                        line,
                        source_symbol: enclosing.clone(),
                        target_name: name[pos + 1..].to_string(),
                        target_qualifier: Some(receiver_expr.to_string()),
                        receiver_type,
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
            // `use a::b::{C, D as E};` — each bound name becomes an
            // ImportBinding (target_qualifier = full rust path). `pub use`
            // re-exports look identical, which is exactly what lets the
            // barrel chase follow `pub use repo::Repository` in mod.rs.
            if let Some(arg) = node.child_by_field_name("argument") {
                explode_use_tree(&arg, source, "", line, refs);
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_refs(&child, source, scope_map, var_types, current_impl, refs);
    }
}

/// Typed parameters: `(s: &Searcher, n: usize)` → s typed. `self` is handled
/// at the function boundary via the enclosing impl.
fn harvest_rust_parameters(params: &Node, source: &str, vt: &mut VarTypes) {
    let mut cursor = params.walk();
    for param in params.named_children(&mut cursor) {
        if param.kind() != "parameter" {
            continue;
        }
        let (Some(pattern), Some(ty)) = (
            param.child_by_field_name("pattern"),
            param.child_by_field_name("type"),
        ) else {
            continue;
        };
        let name_node = match pattern.kind() {
            "identifier" => Some(pattern),
            // `mut x: T` and friends — dig for the identifier
            _ => {
                let mut pc = pattern.walk();
                pattern
                    .named_children(&mut pc)
                    .find(|c| c.kind() == "identifier")
            }
        };
        if let (Some(name), Some(base)) = (name_node, base_rust_type_name(&ty, source)) {
            vt.insert(text(&name, source), base);
        }
    }
}

/// `let x = T::new()` / `let x = T { .. }` / `let x: T = ...` declarations in
/// a scope — skips nested functions/closures, which harvest their own.
fn harvest_let_decls(node: &Node, source: &str, vt: &mut VarTypes) {
    let kind = node.kind();
    if kind == "function_item" || kind == "closure_expression" {
        return;
    }
    if kind == "let_declaration"
        && let Some(pattern) = node.child_by_field_name("pattern")
    {
        let name_node = match pattern.kind() {
            "identifier" => Some(pattern),
            _ => {
                let mut pc = pattern.walk();
                pattern
                    .named_children(&mut pc)
                    .find(|c| c.kind() == "identifier")
            }
        };
        if let Some(name) = name_node {
            // Explicit annotation wins; else infer from the initializer.
            let ty = node
                .child_by_field_name("type")
                .and_then(|t| base_rust_type_name(&t, source))
                .or_else(|| {
                    node.child_by_field_name("value")
                        .and_then(|v| rust_initializer_type(&v, source))
                });
            if let Some(ty) = ty {
                vt.insert(text(&name, source), ty);
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        harvest_let_decls(&child, source, vt);
    }
}

/// `T::new(...)` / `T::default()` / `T { .. }` initializer → T, by the
/// CapWord path convention. Method-call chains and plain calls are not
/// inferred.
fn rust_initializer_type(value: &Node, source: &str) -> Option<String> {
    let cap = |s: &str| -> Option<String> {
        let base = s.split('<').next().unwrap_or(s).trim();
        let last = base.rsplit("::").next().unwrap_or(base);
        if last.chars().next().is_some_and(|c| c.is_uppercase())
            && last.chars().all(|c| c.is_alphanumeric() || c == '_')
            && !is_builtin_rust_type(last)
        {
            Some(last.to_string())
        } else {
            None
        }
    };
    match value.kind() {
        "call_expression" => {
            let func = value.child_by_field_name("function")?;
            let name = text(&func, source);
            // T::new() → the path's second-to-last segment is the type
            let (path, _fn) = name.rsplit_once("::")?;
            cap(path)
        }
        "struct_expression" => {
            let name = value.child_by_field_name("name")?;
            cap(&text(&name, source))
        }
        "reference_expression" => {
            let inner = value.child_by_field_name("value")?;
            rust_initializer_type(&inner, source)
        }
        _ => None,
    }
}

/// Bare type name from a type node: `Searcher` → Searcher, `&mut Searcher` →
/// Searcher, `Vec<T>`/builtins/complex types → None.
fn base_rust_type_name(ty: &Node, source: &str) -> Option<String> {
    match ty.kind() {
        "type_identifier" => {
            let name = text(ty, source);
            if is_builtin_rust_type(&name) {
                None
            } else {
                Some(name)
            }
        }
        "reference_type" => ty
            .child_by_field_name("type")
            .and_then(|inner| base_rust_type_name(&inner, source)),
        "generic_type" => {
            let name = text(&ty.child_by_field_name("type")?, source);
            if is_builtin_rust_type(&name) {
                None
            } else {
                Some(name)
            }
        }
        "scoped_type_identifier" => {
            let name = text(&ty.child_by_field_name("name")?, source);
            if is_builtin_rust_type(&name) {
                None
            } else {
                Some(name)
            }
        }
        _ => None,
    }
}

/// Recursively explode a use tree into per-name bindings.
/// `prefix` accumulates the path above this node (`a::b` for the list in
/// `use a::b::{...}`). Wildcards bind nothing nameable.
fn explode_use_tree(
    node: &Node,
    source: &str,
    prefix: &str,
    line: u32,
    refs: &mut Vec<RawReference>,
) {
    let join = |head: &str, tail: &str| {
        if head.is_empty() {
            tail.to_string()
        } else {
            format!("{head}::{tail}")
        }
    };
    let mut bind = |name: String, full_path: String| {
        if !name.is_empty() && name != "*" {
            refs.push(RawReference {
                line,
                source_symbol: None,
                target_name: name,
                target_qualifier: Some(full_path),
                receiver_type: None,
                kind: RefKind::ImportBinding,
            });
        }
    };
    match node.kind() {
        "identifier" => {
            let name = text(node, source);
            let full = join(prefix, &name);
            bind(name, full);
        }
        // `{self}` in a list re-binds the module itself under its last segment
        "self" => {
            if let Some(name) = prefix.rsplit("::").next() {
                bind(name.to_string(), prefix.to_string());
            }
        }
        "scoped_identifier" => {
            let full = join(prefix, &text(node, source));
            if let Some(name) = node.child_by_field_name("name") {
                bind(text(&name, source), full);
            }
        }
        "use_as_clause" => {
            if let (Some(path), Some(alias)) = (
                node.child_by_field_name("path"),
                node.child_by_field_name("alias"),
            ) {
                bind(text(&alias, source), join(prefix, &text(&path, source)));
            }
        }
        "scoped_use_list" => {
            let new_prefix = node
                .child_by_field_name("path")
                .map(|p| join(prefix, &text(&p, source)))
                .unwrap_or_else(|| prefix.to_string());
            if let Some(list) = node.child_by_field_name("list") {
                explode_use_tree(&list, source, &new_prefix, line, refs);
            }
        }
        "use_list" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                explode_use_tree(&child, source, prefix, line, refs);
            }
        }
        _ => {}
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

/// Rust std-lib collection / smart-pointer / wrapper types that user
/// code routinely calls associated functions on (`Vec::new`, `Box::new`,
/// `Arc::clone`, …) but for which we have no symbol in the workspace
/// index. When these appear as a path-call receiver type, suppressing
/// the receiver_type hint prevents L0d from binding the call to a
/// user-defined `struct Vec` / `enum Result` elsewhere in the repo.
/// Mirrors the SQL-level RUST_BUILTIN_GUARD applied at L2/L4/L4b/L5.
fn is_rust_collection_builtin(name: &str) -> bool {
    matches!(
        name,
        "Vec"
            | "HashMap"
            | "BTreeMap"
            | "HashSet"
            | "BTreeSet"
            | "VecDeque"
            | "Box"
            | "Rc"
            | "Arc"
            | "Cell"
            | "RefCell"
            | "Mutex"
            | "RwLock"
            | "Option"
            | "Result"
            | "String"
            | "Path"
            | "PathBuf"
            | "Cow"
    )
}
