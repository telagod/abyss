use std::path::{Path, PathBuf};
use tree_sitter::{Node, Tree};

use crate::graph::extractor::{
    LanguageRefExtractor, RawReference, RefKind, VarTypes, build_scope_map,
};

pub struct CExtractor;
pub struct CppExtractor;

impl LanguageRefExtractor for CExtractor {
    fn extract(&self, tree: &Tree, source: &str) -> Vec<RawReference> {
        extract_refs(tree, source, false)
    }

    fn is_test_file(&self, path: &str) -> bool {
        is_c_test_file(path)
    }

    fn resolve_import(&self, import_path: &str, workspace: &Path) -> Option<PathBuf> {
        resolve_include(import_path, workspace)
    }

    fn language_name(&self) -> &'static str {
        "c"
    }
}

impl LanguageRefExtractor for CppExtractor {
    fn extract(&self, tree: &Tree, source: &str) -> Vec<RawReference> {
        extract_refs(tree, source, true)
    }

    fn is_test_file(&self, path: &str) -> bool {
        is_c_test_file(path)
    }

    fn resolve_import(&self, import_path: &str, workspace: &Path) -> Option<PathBuf> {
        resolve_include(import_path, workspace)
    }

    fn language_name(&self) -> &'static str {
        "cpp"
    }
}

fn extract_refs(tree: &Tree, source: &str, is_cpp: bool) -> Vec<RawReference> {
    let root = tree.root_node();
    let scope_map = build_scope_map(&root, source, &["function_definition"], fn_def_name);
    let mut refs = Vec::new();
    collect_refs(
        &root,
        source,
        &scope_map,
        &VarTypes::new(),
        is_cpp,
        &mut refs,
    );
    refs
}

fn collect_refs(
    node: &Node,
    source: &str,
    scope_map: &[Option<String>],
    var_types: &VarTypes,
    is_cpp: bool,
    refs: &mut Vec<RawReference>,
) {
    let kind = node.kind();

    if kind == "function_definition" {
        let mut vt = var_types.clone();
        harvest_fn_var_types(node, source, is_cpp, &mut vt);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            collect_refs(&child, source, scope_map, &vt, is_cpp, refs);
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
                        let name = text_of(&func_node, source);
                        if !(is_builtin_c(&name) || is_cpp && is_builtin_cpp(&name)) {
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
                    "field_expression" => {
                        if let Some(method) = func_node.child_by_field_name("field") {
                            let method_name = text_of(&method, source);
                            if let Some(obj) = func_node.child_by_field_name("argument") {
                                let qualifier = text_of(&obj, source);
                                let receiver_type = match obj.kind() {
                                    "identifier" => var_types.get(&qualifier).cloned(),
                                    "this" => var_types.get("this").cloned(),
                                    _ => None,
                                };
                                refs.push(RawReference {
                                    line,
                                    source_symbol: enclosing,
                                    target_name: method_name,
                                    target_qualifier: Some(qualifier),
                                    receiver_type,
                                    kind: RefKind::Call,
                                });
                            }
                        }
                    }
                    "qualified_identifier" if is_cpp => {
                        if let Some((ns, name)) = split_qualified_id(&func_node, source)
                            && !is_std_namespace(&ns)
                        {
                            refs.push(RawReference {
                                line,
                                source_symbol: enclosing,
                                target_name: name,
                                target_qualifier: Some(ns),
                                receiver_type: None,
                                kind: RefKind::Call,
                            });
                        }
                    }
                    "template_function" if is_cpp => {
                        if let Some(name_node) = func_node.child_by_field_name("name") {
                            match name_node.kind() {
                                "identifier" => {
                                    let name = text_of(&name_node, source);
                                    if !is_builtin_cpp(&name) {
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
                                "qualified_identifier" => {
                                    if let Some((ns, name)) = split_qualified_id(&name_node, source)
                                        && !is_std_namespace(&ns)
                                    {
                                        refs.push(RawReference {
                                            line,
                                            source_symbol: enclosing,
                                            target_name: name,
                                            target_qualifier: Some(ns),
                                            receiver_type: None,
                                            kind: RefKind::Call,
                                        });
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        "new_expression" if is_cpp => {
            if let Some(type_node) = node.child_by_field_name("type") {
                let line = node.start_position().row as u32;
                let enclosing = scope_map.get(line as usize).and_then(|s| s.clone());
                let type_name = base_type_name_c(&type_node, source);
                if let Some(name) = type_name
                    && !is_builtin_type_cpp(&name)
                {
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
        }

        "preproc_include" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "string_literal" {
                    let raw = text_of(&child, source);
                    let path = raw.trim_matches('"');
                    if !path.is_empty() {
                        refs.push(RawReference {
                            line: node.start_position().row as u32,
                            source_symbol: None,
                            target_name: path.to_string(),
                            target_qualifier: None,
                            receiver_type: None,
                            kind: RefKind::Import,
                        });
                    }
                }
                // system_lib_string (<stdio.h>) — skip, not resolvable in-repo
            }
        }

        "type_identifier" => {
            let name = text_of(node, source);
            if !(is_builtin_type_c(&name) || is_cpp && is_builtin_type_cpp(&name)) {
                let line = node.start_position().row as u32;
                let enclosing = scope_map.get(line as usize).and_then(|s| s.clone());
                // Skip type refs that are part of the defining declaration itself
                if !is_defining_type_context(node) {
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
        }

        "base_class_clause" if is_cpp => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "type_identifier" {
                    let name = text_of(&child, source);
                    if !is_builtin_type_cpp(&name) {
                        refs.push(RawReference {
                            line: node.start_position().row as u32,
                            source_symbol: None,
                            target_name: name,
                            target_qualifier: None,
                            receiver_type: None,
                            kind: RefKind::Inherit,
                        });
                    }
                }
            }
            // Don't recurse into base_class_clause children to avoid
            // double-counting type_identifiers already captured above
            return;
        }

        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_refs(&child, source, scope_map, var_types, is_cpp, refs);
    }
}

// --- Name extraction ---

fn fn_def_name(node: &Node, source: &str) -> Option<String> {
    let decl = node.child_by_field_name("declarator")?;
    let name_node = decl.child_by_field_name("declarator")?;
    match name_node.kind() {
        "identifier" | "field_identifier" => Some(text_of(&name_node, source)),
        "qualified_identifier" => {
            // Class::method — take the last identifier
            let mut cursor = name_node.walk();
            name_node
                .named_children(&mut cursor)
                .filter(|c| c.kind() == "identifier" || c.kind() == "destructor_name")
                .last()
                .map(|n| text_of(&n, source))
        }
        // pointer-to-function: (*func_ptr)(args) — nested declarators
        "parenthesized_declarator" => {
            let mut cursor = name_node.walk();
            name_node
                .named_children(&mut cursor)
                .find_map(|c| match c.kind() {
                    "identifier" => Some(text_of(&c, source)),
                    "pointer_declarator" => {
                        let mut inner = c.walk();
                        c.named_children(&mut inner)
                            .find(|i| i.kind() == "identifier")
                            .map(|i| text_of(&i, source))
                    }
                    _ => None,
                })
        }
        _ => None,
    }
}

/// Owner type for C++ methods defined outside their class:
/// `void Player::move(int dx)` → `Player`.
pub fn cpp_method_owner_type(fn_node: &Node, source: &str) -> Option<String> {
    let decl = fn_node.child_by_field_name("declarator")?;
    let name_node = decl.child_by_field_name("declarator")?;
    if name_node.kind() != "qualified_identifier" {
        return None;
    }
    let mut cursor = name_node.walk();
    for child in name_node.named_children(&mut cursor) {
        match child.kind() {
            "namespace_identifier" | "type_identifier" => {
                return Some(text_of(&child, source));
            }
            _ => {}
        }
    }
    None
}

fn split_qualified_id(node: &Node, source: &str) -> Option<(String, String)> {
    let name = find_deepest_identifier(node, source)?;
    let full = text_of(node, source);
    let ns = full.strip_suffix(&name)?.trim_end_matches(':').to_string();
    if ns.is_empty() {
        return None;
    }
    Some((ns, name))
}

fn find_deepest_identifier(node: &Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();
    for child in children.iter().rev() {
        match child.kind() {
            "identifier" | "destructor_name" => return Some(text_of(child, source)),
            "qualified_identifier" | "template_type" => {
                return find_deepest_identifier(child, source);
            }
            _ => {}
        }
    }
    None
}

// --- Receiver type inference ---

fn harvest_fn_var_types(fn_node: &Node, source: &str, is_cpp: bool, vt: &mut VarTypes) {
    // Parameters
    if let Some(decl) = fn_node.child_by_field_name("declarator")
        && let Some(params) = decl.child_by_field_name("parameters")
    {
        harvest_param_types(&params, source, vt);
    }
    // C++ `this` → enclosing class type
    if is_cpp && let Some(owner) = find_enclosing_class(fn_node, source) {
        vt.insert("this".to_string(), owner);
    }
    // Body locals
    if let Some(body) = fn_node.child_by_field_name("body") {
        harvest_body_decls(&body, source, is_cpp, vt);
    }
}

fn harvest_param_types(params: &Node, source: &str, vt: &mut VarTypes) {
    let mut cursor = params.walk();
    for param in params.children(&mut cursor) {
        if param.kind() != "parameter_declaration" {
            continue;
        }
        let ty = param_type_name(&param, source);
        let name = param_var_name(&param, source);
        if let (Some(ty), Some(name)) = (ty, name) {
            vt.insert(name, ty);
        }
    }
}

fn param_type_name(param: &Node, source: &str) -> Option<String> {
    // type field gives the type specifier
    param
        .child_by_field_name("type")
        .and_then(|t| base_type_name_c(&t, source))
}

fn param_var_name(param: &Node, source: &str) -> Option<String> {
    let decl = param.child_by_field_name("declarator")?;
    match decl.kind() {
        "identifier" => Some(text_of(&decl, source)),
        // pointer: *p, reference: &p
        "pointer_declarator" | "reference_declarator" => {
            let mut cursor = decl.walk();
            decl.named_children(&mut cursor)
                .find(|c| c.kind() == "identifier")
                .map(|c| text_of(&c, source))
        }
        _ => None,
    }
}

fn harvest_body_decls(node: &Node, source: &str, is_cpp: bool, vt: &mut VarTypes) {
    if node.kind() == "function_definition" {
        return; // nested function harvests its own scope
    }

    if node.kind() == "declaration" {
        // `Type var;` or `Type var = expr;` or `Type *var = ...;`
        if let Some(ty) = node
            .child_by_field_name("type")
            .and_then(|t| base_type_name_c(&t, source))
        {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                let var_name = match child.kind() {
                    "identifier" => Some(text_of(&child, source)),
                    "init_declarator" => child
                        .child_by_field_name("declarator")
                        .and_then(|d| declarator_var_name(&d, source)),
                    "pointer_declarator" | "reference_declarator" => {
                        declarator_var_name(&child, source)
                    }
                    _ => None,
                };
                if let Some(name) = var_name {
                    // For `new Type()`, override with the newed type
                    let effective_ty = if is_cpp {
                        child
                            .child_by_field_name("value")
                            .and_then(|v| infer_new_type(&v, source))
                            .unwrap_or_else(|| ty.clone())
                    } else {
                        ty.clone()
                    };
                    vt.insert(name, effective_ty);
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        harvest_body_decls(&child, source, is_cpp, vt);
    }
}

fn declarator_var_name(node: &Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(text_of(node, source)),
        "pointer_declarator" | "reference_declarator" => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .find(|c| c.kind() == "identifier")
                .map(|c| text_of(&c, source))
        }
        _ => None,
    }
}

fn infer_new_type(expr: &Node, source: &str) -> Option<String> {
    if expr.kind() == "new_expression" {
        return expr
            .child_by_field_name("type")
            .and_then(|t| base_type_name_c(&t, source));
    }
    None
}

fn find_enclosing_class(node: &Node, source: &str) -> Option<String> {
    let mut current = node.parent();
    while let Some(n) = current {
        if n.kind() == "class_specifier" || n.kind() == "struct_specifier" {
            return n.child_by_field_name("name").map(|nm| text_of(&nm, source));
        }
        current = n.parent();
    }
    None
}

fn base_type_name_c(ty: &Node, source: &str) -> Option<String> {
    match ty.kind() {
        "type_identifier" => {
            let name = text_of(ty, source);
            if is_builtin_type_c(&name) {
                None
            } else {
                Some(name)
            }
        }
        "struct_specifier" | "enum_specifier" | "union_specifier" => {
            ty.child_by_field_name("name").map(|n| text_of(&n, source))
        }
        "qualified_identifier" | "template_type" => {
            // std::vector<int> → skip; MyNs::Type → Type
            let full = text_of(ty, source);
            if full.starts_with("std::") {
                return None;
            }
            let mut cursor = ty.walk();
            ty.named_children(&mut cursor)
                .filter(|c| c.kind() == "type_identifier" || c.kind() == "identifier")
                .last()
                .map(|c| text_of(&c, source))
        }
        "primitive_type" | "sized_type_specifier" => None,
        _ => None,
    }
}

fn is_defining_type_context(node: &Node) -> bool {
    if let Some(parent) = node.parent() {
        let pk = parent.kind();
        // struct/class/enum definition: the name inside the specifier is the definition itself
        if (pk == "struct_specifier" || pk == "class_specifier" || pk == "enum_specifier")
            && parent
                .child_by_field_name("name")
                .is_some_and(|n| n.id() == node.id())
        {
            return true;
        }
        // typedef alias name: the last type_identifier in type_definition
        if pk == "type_definition"
            && parent
                .child_by_field_name("declarator")
                .is_some_and(|d| d.id() == node.id())
        {
            return true;
        }
    }
    false
}

// --- Include resolution ---

fn resolve_include(import_path: &str, workspace: &Path) -> Option<PathBuf> {
    let candidate = workspace.join(import_path);
    if candidate.is_file() {
        return Some(candidate);
    }
    // Try common include dirs
    for dir in &["include", "src", "lib"] {
        let candidate = workspace.join(dir).join(import_path);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

// --- Test file detection ---

fn is_c_test_file(path: &str) -> bool {
    let filename = path.rsplit('/').next().unwrap_or(path);
    let stem = filename.split('.').next().unwrap_or(filename);
    stem.ends_with("_test")
        || stem.ends_with("Test")
        || stem.ends_with("Tests")
        || stem.starts_with("test_")
        || stem.starts_with("Test")
        || path.contains("/test/")
        || path.contains("/tests/")
        || path.contains("/testing/")
        || path.starts_with("test/")
        || path.starts_with("tests/")
        || path.starts_with("testing/")
}

// --- Builtins ---

fn text_of(node: &Node, source: &str) -> String {
    source[node.start_byte()..node.end_byte()].to_string()
}

fn is_std_namespace(ns: &str) -> bool {
    ns == "std" || ns.starts_with("std::")
}

fn is_builtin_c(name: &str) -> bool {
    matches!(
        name,
        "printf"
            | "fprintf"
            | "sprintf"
            | "snprintf"
            | "scanf"
            | "sscanf"
            | "fscanf"
            | "puts"
            | "putchar"
            | "getchar"
            | "malloc"
            | "calloc"
            | "realloc"
            | "free"
            | "memcpy"
            | "memmove"
            | "memset"
            | "memcmp"
            | "strlen"
            | "strcpy"
            | "strncpy"
            | "strcmp"
            | "strncmp"
            | "strcat"
            | "strncat"
            | "strstr"
            | "strchr"
            | "strrchr"
            | "fopen"
            | "fclose"
            | "fread"
            | "fwrite"
            | "fseek"
            | "ftell"
            | "fflush"
            | "fgets"
            | "fputs"
            | "exit"
            | "abort"
            | "atexit"
            | "atoi"
            | "atof"
            | "atol"
            | "strtol"
            | "strtoul"
            | "strtod"
            | "abs"
            | "labs"
            | "qsort"
            | "bsearch"
            | "assert"
            | "sizeof"
            | "offsetof"
    )
}

fn is_builtin_cpp(name: &str) -> bool {
    is_builtin_c(name)
        || matches!(
            name,
            "cout"
                | "cin"
                | "cerr"
                | "clog"
                | "endl"
                | "move"
                | "forward"
                | "make_shared"
                | "make_unique"
                | "make_pair"
                | "make_tuple"
                | "static_cast"
                | "dynamic_cast"
                | "reinterpret_cast"
                | "const_cast"
                | "typeid"
                | "decltype"
        )
}

fn is_builtin_type_c(name: &str) -> bool {
    matches!(
        name,
        "int"
            | "char"
            | "short"
            | "long"
            | "float"
            | "double"
            | "void"
            | "unsigned"
            | "signed"
            | "size_t"
            | "ssize_t"
            | "ptrdiff_t"
            | "intptr_t"
            | "uintptr_t"
            | "int8_t"
            | "int16_t"
            | "int32_t"
            | "int64_t"
            | "uint8_t"
            | "uint16_t"
            | "uint32_t"
            | "uint64_t"
            | "bool"
            | "FILE"
            | "NULL"
    )
}

fn is_builtin_type_cpp(name: &str) -> bool {
    is_builtin_type_c(name)
        || matches!(
            name,
            "string"
                | "wstring"
                | "string_view"
                | "vector"
                | "map"
                | "unordered_map"
                | "set"
                | "unordered_set"
                | "list"
                | "deque"
                | "array"
                | "pair"
                | "tuple"
                | "optional"
                | "variant"
                | "any"
                | "shared_ptr"
                | "unique_ptr"
                | "weak_ptr"
                | "function"
                | "thread"
                | "mutex"
                | "atomic"
                | "nullptr_t"
                | "exception"
                | "runtime_error"
                | "logic_error"
                | "true_type"
                | "false_type"
        )
}
