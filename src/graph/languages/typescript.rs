use crate::graph::extractor::{LanguageRefExtractor, RawReference, RefKind};
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Tree};

pub struct TypeScriptExtractor;

impl LanguageRefExtractor for TypeScriptExtractor {
    fn extract(&self, tree: &Tree, source: &str) -> Vec<RawReference> {
        let mut refs = Vec::new();
        let root = tree.root_node();
        let scope_map = build_scope_map(&root, source);
        // Module-level declarations (const app = new Hono()) seed the map for
        // everything below; nested functions inherit and extend it.
        let mut vt = VarTypes::new();
        harvest_var_decls(&root, source, &mut vt);
        collect_refs(&root, source, &scope_map, &vt, None, &mut refs);
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

/// Lite per-scope variable → type map (mirrors the Go extractor): parameters
/// with type annotations, `const x = new T()`, and `this` → enclosing class.
/// No data-flow, no interfaces, no union types.
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
    if matches!(
        kind,
        "function_declaration" | "method_definition" | "arrow_function" | "function_expression"
    ) {
        let mut vt = var_types.clone();
        if let Some(params) = node.child_by_field_name("parameters") {
            harvest_parameters(&params, source, &mut vt);
        }
        if let Some(body) = node.child_by_field_name("body") {
            harvest_var_decls(&body, source, &mut vt);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            collect_refs(&child, source, scope_map, &vt, current_class, refs);
        }
        return;
    }

    // Class boundary: `this.m()` below resolves to this class.
    if kind == "class_declaration" || kind == "class" {
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
                                receiver_type: None,
                                kind: RefKind::Call,
                            });
                        }
                    }
                    "member_expression" => {
                        if let (Some(obj), Some(prop)) = (
                            func.child_by_field_name("object"),
                            func.child_by_field_name("property"),
                        ) {
                            let receiver_type = match obj.kind() {
                                "identifier" => var_types.get(&text(&obj, source)).cloned(),
                                "this" => current_class.map(String::from),
                                _ => None,
                            };
                            refs.push(RawReference {
                                line,
                                source_symbol: enclosing.clone(),
                                target_name: text(&prop, source),
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
        "new_expression" => {
            if let Some(constructor) = node.child_by_field_name("constructor") {
                refs.push(RawReference {
                    line,
                    source_symbol: enclosing.clone(),
                    target_name: text(&constructor, source),
                    target_qualifier: None,
                    receiver_type: None,
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
                    receiver_type: None,
                    kind: RefKind::TypeRef,
                });
            }
        }
        // JSX component usage: `<Foo />`, `<Foo.Bar />`, `<Foo bar={...} />`.
        //
        // v0.5.1 hono dogfood (L3): .tsx files had a higher unresolved-ref
        // rate than .ts because the tsx grammar IS wired up but the
        // visitor never fired on JSX-specific nodes. A component name in
        // an opening element IS a usage of the imported symbol; an
        // identifier inside a JSX expression attribute IS a usage of the
        // wrapped binding (hook, helper, constant). Both must surface so
        // the resolver gets a chance to bind them.
        //
        // We emit:
        //   * jsx_opening_element / jsx_self_closing_element → component
        //     name as a Call ref (it IS constructed at runtime, and the
        //     resolver treats Call edges the same way it treats `new T()`).
        //   * jsx_expression with a bare identifier child → Call ref so
        //     L0b (import binding) can bind it to the defining file. The
        //     normal recursion already handles call_expression /
        //     member_expression inside `{ ... }`, so we only manually fire
        //     on the standalone identifier case.
        "jsx_opening_element" | "jsx_self_closing_element" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                jsx_emit_element_name(&name_node, source, line, &enclosing, refs);
            }
        }
        "jsx_expression" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "identifier" {
                    let name = text(&child, source);
                    if !is_builtin_js(&name) {
                        refs.push(RawReference {
                            line: child.start_position().row as u32,
                            source_symbol: enclosing.clone(),
                            target_name: name,
                            target_qualifier: None,
                            receiver_type: None,
                            kind: RefKind::Call,
                        });
                    }
                }
            }
        }
        "import_statement" => {
            if let Some(src) = node.child_by_field_name("source") {
                let module = text(&src, source)
                    .trim_matches(|c| c == '\'' || c == '"')
                    .to_string();
                refs.push(RawReference {
                    line,
                    source_symbol: None,
                    target_name: module.clone(),
                    target_qualifier: None,
                    receiver_type: None,
                    kind: RefKind::Import,
                });
                // Named/default bindings: `import d, { a, b as c } from './x'`.
                // Each local name becomes an ImportBinding pointing at the
                // module — the strongest evidence a bare call can have.
                let mut cursor = node.walk();
                if let Some(clause) = node
                    .children(&mut cursor)
                    .find(|c| c.kind() == "import_clause")
                {
                    collect_import_bindings(&clause, source, &module, line, refs);
                }
            }
        }
        // Re-export: `export { a, b as c } from './x'` — a binding in THIS
        // file under the EXPORTED name, so importer→barrel chains can be
        // chased to the defining file.
        "export_statement" => {
            if let Some(src) = node.child_by_field_name("source") {
                let module = text(&src, source)
                    .trim_matches(|c| c == '\'' || c == '"')
                    .to_string();
                let mut cursor = node.walk();
                if let Some(clause) = node
                    .children(&mut cursor)
                    .find(|c| c.kind() == "export_clause")
                {
                    let mut ec = clause.walk();
                    for spec in clause.named_children(&mut ec) {
                        if spec.kind() != "export_specifier" {
                            continue;
                        }
                        let exported = spec
                            .child_by_field_name("alias")
                            .or_else(|| spec.child_by_field_name("name"));
                        if let Some(n) = exported {
                            refs.push(RawReference {
                                line,
                                source_symbol: None,
                                target_name: text(&n, source),
                                target_qualifier: Some(module.clone()),
                                receiver_type: None,
                                kind: RefKind::ImportBinding,
                            });
                        }
                    }
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

/// JSX element-name extraction. The grammar reports the name as either a
/// bare `identifier`, a member-style `nested_identifier` / `member_expression`
/// (`<foo.Bar />`), or a `jsx_namespace_name` (`<svg:circle />`). Bare
/// lowercase identifiers are HTML intrinsics (`<div>`, `<span>`) — those
/// shouldn't pollute the call graph, so we only emit Pascal-case bare
/// identifiers. Qualified names emit the rightmost component.
fn jsx_emit_element_name(
    name_node: &Node,
    source: &str,
    line: u32,
    enclosing: &Option<String>,
    refs: &mut Vec<RawReference>,
) {
    match name_node.kind() {
        "identifier" => {
            let name = text(name_node, source);
            // HTML intrinsics are lowercase; React components are Pascal-case.
            // First-letter-uppercase is the JSX convention and also the
            // cheapest filter — `if name.starts_with(uppercase)`.
            if name
                .chars()
                .next()
                .map(|c| c.is_ascii_uppercase())
                .unwrap_or(false)
                && !is_builtin_js(&name)
            {
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
        "nested_identifier" | "member_expression" => {
            // `<foo.Bar />` — emit `Bar` with qualifier `foo`. Matches the
            // way member_expression call sites are extracted above so the
            // resolver treats the two cases uniformly.
            let prop = name_node
                .child_by_field_name("property")
                .or_else(|| name_node.child_by_field_name("name"));
            let obj = name_node.child_by_field_name("object");
            if let Some(prop) = prop {
                let name = text(&prop, source);
                let qualifier = obj.map(|o| text(&o, source));
                refs.push(RawReference {
                    line,
                    source_symbol: enclosing.clone(),
                    target_name: name,
                    target_qualifier: qualifier,
                    receiver_type: None,
                    kind: RefKind::Call,
                });
            }
        }
        _ => {}
    }
}

/// Bindings out of an import_clause: default import (`import d from`),
/// named imports (`{ a, b as c }`). Namespace imports (`* as ns`) are
/// qualifier territory (L3), not bare-call bindings — skipped.
fn collect_import_bindings(
    clause: &Node,
    source: &str,
    module: &str,
    line: u32,
    refs: &mut Vec<RawReference>,
) {
    let mut push = |name: String| {
        refs.push(RawReference {
            line,
            source_symbol: None,
            target_name: name,
            target_qualifier: Some(module.to_string()),
            receiver_type: None,
            kind: RefKind::ImportBinding,
        });
    };
    let mut cursor = clause.walk();
    for child in clause.named_children(&mut cursor) {
        match child.kind() {
            "identifier" => push(text(&child, source)), // default import
            "named_imports" => {
                let mut nc = child.walk();
                for spec in child.named_children(&mut nc) {
                    if spec.kind() != "import_specifier" {
                        continue;
                    }
                    // local name = alias if present, else the imported name
                    let local = spec
                        .child_by_field_name("alias")
                        .or_else(|| spec.child_by_field_name("name"));
                    if let Some(n) = local {
                        push(text(&n, source));
                    }
                }
            }
            _ => {}
        }
    }
}

/// Parameters with type annotations: `(c: Context, e?: Engine)` → c/e typed.
fn harvest_parameters(params: &Node, source: &str, vt: &mut VarTypes) {
    let mut cursor = params.walk();
    for param in params.named_children(&mut cursor) {
        let pk = param.kind();
        if pk != "required_parameter" && pk != "optional_parameter" {
            continue;
        }
        let (Some(pattern), Some(ty)) = (
            param.child_by_field_name("pattern"),
            param.child_by_field_name("type"),
        ) else {
            continue;
        };
        if pattern.kind() != "identifier" {
            continue; // destructuring patterns: skip
        }
        if let Some(base) = base_ts_type_name(&ty, source) {
            vt.insert(text(&pattern, source), base);
        }
    }
}

/// `const x = new T()` declarations in a scope — skips nested functions,
/// which harvest their own.
fn harvest_var_decls(node: &Node, source: &str, vt: &mut VarTypes) {
    let kind = node.kind();
    if matches!(
        kind,
        "function_declaration" | "method_definition" | "arrow_function" | "function_expression"
    ) {
        return;
    }
    if kind == "variable_declarator"
        && let (Some(name), Some(value)) = (
            node.child_by_field_name("name"),
            node.child_by_field_name("value"),
        )
        && name.kind() == "identifier"
        && value.kind() == "new_expression"
        && let Some(ctor) = value.child_by_field_name("constructor")
    {
        let ty = match ctor.kind() {
            "identifier" => Some(text(&ctor, source)),
            // new ns.Type() → Type
            "member_expression" => ctor
                .child_by_field_name("property")
                .map(|p| text(&p, source)),
            _ => None,
        };
        if let Some(ty) = ty {
            vt.insert(text(&name, source), ty);
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        harvest_var_decls(&child, source, vt);
    }
}

/// Bare type name from a type_annotation: `: Context` → Context,
/// `: Hono<Env>` → Hono. Unions/predefined/complex types → None.
fn base_ts_type_name(annotation: &Node, source: &str) -> Option<String> {
    // type_annotation wraps the actual type node
    let mut cursor = annotation.walk();
    let inner = annotation.named_children(&mut cursor).next()?;
    match inner.kind() {
        "type_identifier" => {
            let name = text(&inner, source);
            if is_builtin_ts_type(&name) {
                None
            } else {
                Some(name)
            }
        }
        "generic_type" => {
            let name_node = inner.child_by_field_name("name")?;
            let name = text(&name_node, source);
            if is_builtin_ts_type(&name) {
                None
            } else {
                Some(name)
            }
        }
        _ => None,
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
